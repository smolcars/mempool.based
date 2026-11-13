//! Mempool history (`docs/API.md`, "Mempool statistics"): runs the sampler that stores one
//! sample a minute in the network's store, and serves the series under
//! `/api/v1/statistics/:interval` in the reference's shape and `/app/mempool/history/:interval`
//! in ours. Answers come from the store, never the node, and are kept in memory per interval
//! until the next sample is written or the minute turns.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use engine::history::{self, Interval, MINUTE, Point, Series};
use engine::mempool::Tracker;
use engine::projected::{Projector, STORED_BANDS};
use engine::store;
use logging::events::{
    MempoolHistoryFailed, MempoolHistoryResumed, MempoolHistoryRolledUp, MempoolHistorySkipped,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::error::ApiError;
use crate::esplora::{ApiOutcome, Source};
use crate::ws::{Frame, Hub, Snapshot, Topic};
use crate::{Network, NetworkContext};

const NO_TABLE: ApiError = ApiError::new(
    StatusCode::SERVICE_UNAVAILABLE,
    "mempool history needs a data directory on this server",
);
const CACHE: &str = "public, max-age=1";
/// The newest sample is sent as the inflow rate while it is at most this old: the minute just
/// sampled and the next, whose sample may still be on its way.
const FRESH: u64 = 2 * MINUTE;

/// The reference's ranges are whole sat/vB with these lower bounds; ours are the ten stored bands
/// (`Bands::stored`), whose first is all under 0.2 whether or not its split is known, so a point
/// maps the same before and after the split; summing its three parts instead could round a range
/// differently. Each band goes, whole, to the range holding its lower bound: bands 0 to 3 (under 1)
/// to `[0, 1)`, 4 and 5 (1 to 2) to `[1, 2)`, 6 to `[2, 3)`, 7 (3 to 5) to `[3, 4)`, 8 (5 to
/// 10) to `[5, 6)` and 9 (10 and up) to `[10, 12)`. Totals at 1, 2, 3, 5 and 10 sat/vB are
/// exact; the ranges inside a band read 0 (`docs/API.md`).
const REFERENCE_RANGES: usize = 39;
const BAND_RANGE: [usize; STORED_BANDS] = [0, 0, 0, 0, 1, 1, 2, 3, 5, 8];

/// One network's history answers, held in [`NetworkContext`].
#[derive(Debug, Default)]
pub struct History {
    answers: Mutex<HashMap<(Shape, Interval), Arc<Answer>>>,
    /// One computation at a time per network, whoever asks.
    compute: tokio::sync::Mutex<()>,
    /// Samples written since start: an answer computed before the latest is out of date.
    written: Arc<AtomicU64>,
    failures: AtomicU32,
    /// The newest sample written since start: `vBytesPerSecond` on `/api/v1/ws`.
    pub(crate) latest: Arc<Mutex<Option<Point>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Shape {
    Reference,
    App,
}

#[derive(Debug)]
struct Answer {
    written: u64,
    minute: u64,
    body: Bytes,
    from: Option<u64>,
    complete: bool,
}

pub fn routes() -> Router<Arc<NetworkContext>> {
    Router::new().route("/api/v1/statistics/{interval}", get(statistics))
}

pub fn app_routes() -> Router<Arc<NetworkContext>> {
    Router::new().route("/app/mempool/history/{interval}", get(app_history))
}

pub(crate) async fn statistics(
    State(ctx): State<Arc<NetworkContext>>,
    Path(i): Path<String>,
) -> Response {
    serve(&ctx, Shape::Reference, &i, "statistics").await
}

async fn app_history(State(ctx): State<Arc<NetworkContext>>, Path(i): Path<String>) -> Response {
    serve(&ctx, Shape::App, &i, "mempool_history").await
}

async fn serve(ctx: &NetworkContext, shape: Shape, i: &str, route: &'static str) -> Response {
    let Some(interval) = Interval::parse(i) else {
        return ApiError::bad_request("invalid interval").into_response();
    };
    let Some(table) = &ctx.blocks else {
        return NO_TABLE.into_response();
    };
    let h = &ctx.history;
    let key = (shape, interval);
    let current = |a: &Answer, now: u64| {
        a.written == h.written.load(Ordering::Acquire) && a.minute == now / MINUTE
    };
    if let Some(a) = h.get(key).filter(|a| current(a, history::unix_now())) {
        return respond(&a, route, Source::Cached);
    }
    let _one = h.compute.lock().await;
    let now = history::unix_now();
    if let Some(a) = h.get(key).filter(|a| current(a, now)) {
        return respond(&a, route, Source::Cached);
    }
    let written = h.written.load(Ordering::Acquire);
    match history::series(&table.store, interval, now).await {
        Ok(s) => {
            h.failures.store(0, Ordering::Relaxed);
            let mut map = h.answers.lock().unwrap_or_else(PoisonError::into_inner);
            let mut mine = None;
            for (sh, body) in [
                (
                    Shape::Reference,
                    serde_json::to_vec(&reference(&s, interval)),
                ),
                (Shape::App, serde_json::to_vec(&app(&s, interval))),
            ] {
                let a = Arc::new(Answer {
                    written,
                    minute: now / MINUTE,
                    body: Bytes::from(body.unwrap_or_default()),
                    from: s.earliest(),
                    complete: s.complete(),
                });
                if sh == shape {
                    mine = Some(Arc::clone(&a));
                }
                map.insert((sh, interval), a);
            }
            drop(map);
            match mine {
                Some(a) => respond(&a, route, Source::Stored),
                None => ApiError::INTERNAL.into_response(),
            }
        }
        Err(e) => {
            let failures = h.failures.fetch_add(1, Ordering::Relaxed).saturating_add(1);
            if failures.is_power_of_two() {
                logging::emit(&MempoolHistoryFailed {
                    network: ctx.config.network.name(),
                    error: match e {
                        store::Error::TimedOut => "timed_out",
                        _ => "store",
                    },
                    failures,
                });
            }
            match e {
                store::Error::TimedOut => ApiError::OVERLOADED,
                _ => ApiError::INTERNAL,
            }
            .into_response()
        }
    }
}

impl History {
    /// What entered the mempool per second, in sigop-adjusted virtual bytes, over the minute
    /// before the newest sample: `None` before the first sample since start, without a store,
    /// or when the newest is more than [`FRESH`] old (the sampler is skipping minutes).
    pub fn inflow(&self, now: u64) -> Option<u64> {
        let p = (*self.latest.lock().unwrap_or_else(PoisonError::into_inner))?;
        (now.saturating_sub(p.time) <= FRESH)
            .then(|| (p.sums.flow.in_vsize as f64 / MINUTE as f64).round() as u64)
    }

    fn get(&self, key: (Shape, Interval)) -> Option<Arc<Answer>> {
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
    }
}

fn respond(a: &Answer, route: &'static str, source: Source) -> Response {
    let mut res = (
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        a.body.clone(),
    )
        .into_response();
    let h = res.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static(CACHE));
    if let Some(from) = a.from {
        h.insert("x-data-from-time", HeaderValue::from(from));
    }
    h.insert(
        "x-data-complete",
        HeaderValue::from_static(if a.complete { "true" } else { "false" }),
    );
    res.extensions_mut().insert(ApiOutcome {
        route,
        source,
        upstream_error: None,
    });
    res
}

/// A mean as JSON: whole numbers as integers, others to three decimals.
fn num(x: f64) -> Value {
    let x = (x * 1000.0).round() / 1000.0;
    if x.fract() == 0.0 && x >= 0.0 && x < 2f64.powi(53) {
        json!(x as u64)
    } else {
        json!(x)
    }
}

/// The point's mean of a sum over its sampled minutes.
fn mean(p: &Point, sum: u64) -> f64 {
    sum as f64 / p.samples.max(1) as f64
}

/// One point of `/api/v1/statistics`, fields in the reference's order.
#[derive(Serialize)]
struct ReferencePoint {
    added: u64,
    count: Value,
    vbytes_per_second: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    mempool_byte_weight: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_fee: Option<Value>,
    min_fee: Value,
    vsizes: Vec<u64>,
}

/// The reference's shape: newest first; `mempool_byte_weight` and `total_fee` only for the two
/// shortest intervals, as the reference sends them.
fn reference(s: &Series, interval: Interval) -> Vec<ReferencePoint> {
    let short = matches!(interval, Interval::TwoHours | Interval::Day);
    s.points
        .iter()
        .rev()
        .map(|p| reference_point(p, short))
        .collect()
}

/// One point in the reference's shape: an element of `/api/v1/statistics`, and the
/// `live-2h-chart` message on `/api/v1/ws`, so the two cannot disagree.
fn reference_point(p: &Point, short: bool) -> ReferencePoint {
    let t = &p.sums;
    let mut vsizes = [0.0; REFERENCE_RANGES];
    for (band, sum) in t.bands.stored.iter().enumerate() {
        if let Some(v) = BAND_RANGE.get(band).and_then(|r| vsizes.get_mut(*r)) {
            *v += mean(p, *sum);
        }
    }
    let vsizes = vsizes.iter().map(|v| v.round() as u64).collect();
    let per_second = mean(p, t.flow.in_vsize) / MINUTE as f64;
    ReferencePoint {
        added: p.time,
        count: num(mean(p, t.count)),
        vbytes_per_second: num(per_second),
        mempool_byte_weight: short.then(|| num(4.0 * mean(p, t.vsize))),
        total_fee: short.then(|| num(mean(p, t.fees))),
        min_fee: num(mean(p, t.min_fee) / 1000.0),
        vsizes,
    }
}

/// A sample was written: kept as the newest for `vBytesPerSecond`, and sent as `live-2h-chart`,
/// the point `/api/v1/statistics/2h` now lists first, encoded once for every socket wanting it.
pub(crate) fn sampled(latest: &Mutex<Option<Point>>, hub: &Hub, sample: &Point) {
    *latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(*sample);
    // Serialized from the struct, not through `json!`, so the fields keep the reference's order.
    #[derive(Serialize)]
    struct Live {
        #[serde(rename = "live-2h-chart")]
        point: ReferencePoint,
    }
    let Ok(msg) = serde_json::to_vec(&Live {
        point: reference_point(sample, true),
    }) else {
        return;
    };
    // No state for new subscribers: the reference sends nothing on `want`, only each new sample.
    hub.publish(Topic::V1LiveChart, &Frame::text(&msg), Snapshot::Keep);
}

/// Our own shape, for our chart: oldest first, every field a mean per sampled minute.
fn app(s: &Series, interval: Interval) -> Value {
    let points: Vec<Value> = s
        .points
        .iter()
        .map(|p| {
            let (t, f) = (&p.sums, &p.sums.flow);
            let bands: Vec<Value> = t.bands.list().iter().map(|b| num(mean(p, *b))).collect();
            json!({
                "time": p.time,
                "samples": p.samples,
                "count": num(mean(p, t.count)),
                "vsize": num(mean(p, t.vsize)),
                "fees": num(mean(p, t.fees)),
                "min_fee_rate": num(mean(p, t.min_fee)),
                "bands": bands,
                "in_count": num(mean(p, f.in_count)),
                "in_vsize": num(mean(p, f.in_vsize)),
                "out_count": num(mean(p, f.out_count)),
                "out_vsize": num(mean(p, f.out_vsize)),
                "mined_count": num(mean(p, f.mined_count)),
                "mined_vsize": num(mean(p, f.mined_vsize)),
            })
        })
        .collect();
    json!({
        "interval": interval.name(),
        "from": s.from,
        "to": s.to,
        "step": s.step,
        "first": s.first,
        "minutes": s.minutes,
        "sampled": s.sampled,
        "complete": s.complete(),
        "points": points,
    })
}

/// Starts the network's sampler when it has a store, and turns what it reports into log lines:
/// a roll-up when one moved rows, skipped minutes at the 1st, 2nd, 4th… in a row, and the
/// first minute sampled after them. Each sample written is kept for `vBytesPerSecond` and sent
/// as `live-2h-chart` on `/api/v1/ws`.
pub fn start(
    ctx: &NetworkContext,
    tracker: &Tracker,
    projector: &Projector,
) -> Option<JoinHandle<()>> {
    let store = Arc::clone(&ctx.blocks.as_ref()?.store);
    let network = ctx.config.network;
    let written = Arc::clone(&ctx.history.written);
    let latest = Arc::clone(&ctx.history.latest);
    let hub = Arc::clone(&ctx.hub);
    let skipped = AtomicU32::new(0);
    let observer = move |e: &history::Event| {
        if let history::Event::Written { sample, .. } = e {
            sampled(&latest, &hub, sample);
        }
        report(network, &written, &skipped, e);
    };
    let config = history::Config {
        observer: Arc::new(observer),
        ..history::Config::default()
    };
    Some(history::start(
        tracker.clone(),
        projector.clone(),
        store,
        config,
    ))
}

fn report(network: Network, written: &AtomicU64, skipped: &AtomicU32, e: &history::Event) {
    let network = network.name();
    match e {
        history::Event::Written {
            rolled, elapsed, ..
        } => {
            written.fetch_add(1, Ordering::Release);
            let missed = skipped.swap(0, Ordering::Relaxed);
            if missed > 0 {
                logging::emit(&MempoolHistoryResumed {
                    network,
                    skipped: missed,
                });
            }
            if rolled.minutes > 0 || rolled.hours > 0 {
                logging::emit(&MempoolHistoryRolledUp {
                    network,
                    minutes: rolled.minutes,
                    hours: rolled.hours,
                    duration_ms: elapsed.as_secs_f64() * 1000.0,
                });
            }
        }
        history::Event::Skipped { reason, .. } => {
            let n = skipped.fetch_add(1, Ordering::Relaxed).saturating_add(1);
            if n.is_power_of_two() {
                logging::emit(&MempoolHistorySkipped {
                    network,
                    reason: reason.label(),
                    skipped: n,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use engine::history::{Flow, Totals};
    use engine::projected::{BANDS, Bands};

    use super::*;

    /// A point stored before the lowest band was split: the ten stored bands alone.
    fn point(samples: u64, stored: [u64; STORED_BANDS]) -> Point {
        Point {
            time: 1_800_000_000,
            samples,
            sums: Totals {
                count: 3 * samples,
                vsize: stored.iter().sum(),
                fees: 10 * samples,
                min_fee: 100 * samples,
                bands: Bands {
                    stored,
                    split: None,
                },
                flow: Flow {
                    in_vsize: 120 * samples,
                    ..Flow::default()
                },
            },
        }
    }

    fn series(points: Vec<Point>) -> Series {
        Series {
            from: 0,
            to: 0,
            step: 60,
            points,
            first: None,
            sampled: 0,
            minutes: 0,
        }
    }

    #[test]
    fn our_bands_go_whole_to_the_reference_range_of_their_lower_bound() {
        let bands = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512];
        let v = json!(reference(
            &series(vec![point(1, bands)]),
            Interval::TwoHours
        ));
        let p = &v[0];
        let vsizes: Vec<u64> = serde_json::from_value(p["vsizes"].clone()).unwrap();
        assert_eq!(vsizes.len(), REFERENCE_RANGES);
        let mut want = vec![0; REFERENCE_RANGES];
        want[0] = 1 + 2 + 4 + 8; // under 1 sat/vB
        want[1] = 16 + 32; // 1 to 2
        want[2] = 64; // 2 to 3
        want[3] = 128; // 3 to 5, reported at 3 to 4
        want[5] = 256; // 5 to 10, reported at 5 to 6
        want[8] = 512; // 10 and up, reported at 10 to 12
        assert_eq!(vsizes, want);
        // Nothing is lost or made up: the ranges sum to the mempool's vsize, four times which
        // is the weight the reference reports.
        assert_eq!(vsizes.iter().sum::<u64>(), 1023);
        assert_eq!(p["mempool_byte_weight"], json!(4092));
        assert_eq!(p["count"], json!(3));
        assert_eq!(p["vbytes_per_second"], json!(2));
        assert_eq!(p["min_fee"], json!(0.1));
        assert_eq!(p["total_fee"], json!(10));
    }

    #[test]
    fn longer_intervals_average_and_drop_the_reference_s_extra_fields() {
        let bands = [600, 0, 0, 0, 0, 0, 0, 0, 0, 7];
        let r = reference(
            &series(vec![point(1, bands), point(3, bands)]),
            Interval::Week,
        );
        // Serialized in the reference's field order.
        let text = serde_json::to_string(&r[0]).unwrap();
        assert!(
            text.starts_with(r#"{"added":1800000000,"count":3,"vbytes_per_second":2,"min_fee":0.1,"vsizes":[200,"#),
            "{text}"
        );
        let short =
            serde_json::to_string(&reference(&series(vec![point(1, bands)]), Interval::Day))
                .unwrap();
        assert!(
            short.contains(
                r#""vbytes_per_second":2,"mempool_byte_weight":2428,"total_fee":10,"min_fee":0.1,"#
            ),
            "{short}"
        );
        let v = json!(r);
        // Newest first; three samples' sums become means.
        assert_eq!(v[0]["count"], json!(3));
        assert_eq!(v[0]["vsizes"][0], json!(200));
        assert_eq!(v[0]["vsizes"][8], json!(2)); // 7 / 3, rounded
        let a = app(&series(vec![point(3, bands)]), Interval::Week);
        assert_eq!(a["points"][0]["bands"][9], json!(2.333));
        assert_eq!(a["points"][0]["samples"], json!(3));
    }

    #[test]
    fn the_reference_answer_is_the_same_whether_the_lowest_band_is_split_or_not() {
        // The same mempool, sampled with the split and without it, over awkward sample counts
        // whose means do not divide evenly.
        for samples in [1, 3, 6, 7, 1_440] {
            let twelve: [u64; BANDS] = [
                1_000_001, 333_333, 166_667, 5, 7, 11, 13, 17, 19, 23, 29, 31,
            ]
            .map(|b| b * samples);
            let unsplit = point(samples, Bands::new(twelve).stored);
            let mut split = unsplit;
            split.sums.bands = Bands::new(twelve);
            assert_eq!(
                split.sums.bands.stored[0],
                twelve[0] + twelve[1] + twelve[2]
            );
            for interval in [Interval::TwoHours, Interval::Week] {
                let one = |p: Point| serde_json::to_string(&reference(&series(vec![p]), interval));
                assert_eq!(one(split).unwrap(), one(unsplit).unwrap(), "{samples}");
            }
            // Ours sends the 12 bands when the split is known and the ten when it is not.
            let a = app(&series(vec![split, unsplit]), Interval::TwoHours);
            assert_eq!(a["points"][0]["bands"].as_array().unwrap().len(), 12);
            assert_eq!(a["points"][1]["bands"].as_array().unwrap().len(), 10);
            assert_eq!(a["points"][0]["bands"][1], json!(333_333));
        }
    }
}
