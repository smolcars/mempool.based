//! Answers kept in memory (`docs/API.md`, "Answers kept in memory"): one cache per network for
//! every answer that is the same for every client until something specific changes.
//!
//! - **Store-backed routes** ([`KEPT`]) are kept under the store's version of the tables they read
//!   ([`Store::version`]). A write marks its tables before it changes them, so an answer is
//!   served from memory only while no write has touched them since it was computed.
//! - **Immutable node answers** (`crate::esplora`) are kept once their block is buried.
//!
//! The cache is bounded by bytes. New entries go to a probation segment and move to a protected
//! one on their second use, so a client asking for many distinct keys only churns probation
//! (segmented LRU). Concurrent identical misses share one computation (SECURITY.md rule 8).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::body::{Body, Bytes};
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use engine::store::Table;
use tokio::sync::OnceCell;

use crate::NetworkContext;
use crate::error::ApiError;
use crate::esplora::{ApiOutcome, Source};

/// An entry larger than this share of the budget is not kept (512 KB at the default 32 MB):
/// a raw block or a long list would push out hundreds of small answers.
const MAX_SHARE: usize = 64;
/// The protected segment's share of the budget, in fifths.
const PROTECTED_FIFTHS: usize = 4;
/// Bytes counted per entry beyond its key, headers and body: the maps' nodes and the `Arc`.
const ENTRY_OVERHEAD: usize = 192;

const BLOCKS: &[Table] = &[Table::Blocks, Table::Audits];
const AUDITS: &[Table] = &[Table::Audits, Table::Blocks];

/// Store-backed routes whose answer is kept, by matched path, with the tables it reads. Every
/// other input of these answers is fixed by a hash (block headers, block summaries) or by the
/// build (the pool list). Routes whose answer also moves with the clock or the node's tip are
/// not here: mining's refreshed answers, `predictions`, the statistics.
const KEPT: &[(&str, &[Table])] = &[
    ("/api/v1/blocks", BLOCKS),
    ("/api/v1/blocks/{height}", BLOCKS),
    ("/api/v1/block/{hash}", BLOCKS),
    ("/app/block/{hash}/summary", BLOCKS),
    ("/app/blocks/summary", BLOCKS),
    ("/api/v1/mining/blocks/audit/score/{hash}", AUDITS),
    ("/api/v1/mining/blocks/audit/scores", AUDITS),
    ("/api/v1/mining/blocks/audit/scores/{height}", AUDITS),
    ("/api/v1/mining/blocks/audit/{hash}", AUDITS),
    ("/api/v1/block/{hash}/audit-summary", AUDITS),
    ("/api/v1/block/{hash}/tx/{txid}/audit", AUDITS),
    ("/app/block/{hash}/audit", AUDITS),
];

/// Marks a `200` that must not be kept: it rests on something besides its route's tables.
#[derive(Debug, Clone, Copy)]
pub struct Unkept;

/// A response as kept: everything the handler produced except its body stream.
#[derive(Debug)]
pub struct Kept {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    outcome: Option<ApiOutcome>,
}

impl Kept {
    /// The handler's response as a [`Kept`], when it is a `200` that may be kept; else it back.
    async fn take(res: Response) -> Result<Self, Box<Response>> {
        if res.status() != StatusCode::OK || res.extensions().get::<Unkept>().is_some() {
            return Err(Box::new(res));
        }
        let outcome = res.extensions().get::<ApiOutcome>().copied();
        let (parts, body) = res.into_parts();
        // Every kept route builds its body in memory, so this does not wait.
        let Ok(body) = axum::body::to_bytes(body, usize::MAX).await else {
            return Err(Box::new(ApiError::INTERNAL.into_response()));
        };
        Ok(Self {
            status: parts.status,
            headers: parts.headers,
            body,
            outcome,
        })
    }

    /// From a node answer's parts.
    pub fn new(headers: HeaderMap, body: Bytes, outcome: Option<ApiOutcome>) -> Self {
        Self {
            status: StatusCode::OK,
            headers,
            body,
            outcome,
        }
    }

    pub fn body(&self) -> &Bytes {
        &self.body
    }

    fn size(&self) -> usize {
        let headers: usize = self
            .headers
            .iter()
            .map(|(k, v)| k.as_str().len() + v.len() + 32)
            .sum();
        self.body.len() + headers
    }

    /// As a response; `source`, when given, replaces the one the computation recorded.
    pub fn response(&self, source: Option<Source>) -> Response {
        let mut res = Response::new(Body::from(self.body.clone()));
        *res.status_mut() = self.status;
        *res.headers_mut() = self.headers.clone();
        if let Some(mut o) = self.outcome {
            if let Some(s) = source {
                o.source = s;
            }
            res.extensions_mut().insert(o);
        }
        res
    }
}

/// A computation in flight: `Some` once it gave a keepable answer, `None` if it did not.
type Flight = Arc<OnceCell<Option<Arc<Kept>>>>;

/// A flight in the map, and the requests holding it.
#[derive(Debug)]
struct Held {
    flight: Flight,
    holders: usize,
}

/// One network's kept answers.
#[derive(Debug)]
pub struct Answers {
    lru: Mutex<Lru>,
    /// By key and version: a request still at an older version never displaces a newer flight.
    flights: Mutex<HashMap<(String, u64), Held>>,
}

impl Answers {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            lru: Mutex::new(Lru::new(max_bytes)),
            flights: Mutex::new(HashMap::new()),
        }
    }

    /// The answer kept for `key` at `version`. One kept at another version is dropped.
    pub fn get(&self, key: &str, version: u64) -> Option<Arc<Kept>> {
        lock(&self.lru).get(key, version)
    }

    /// Keeps `kept` for `key` at `version`, unless it is larger than its share of the budget.
    pub fn insert(&self, key: String, version: u64, kept: Arc<Kept>) {
        lock(&self.lru).insert(key, version, kept);
    }

    /// Bytes held, as counted, and entries. For tests and measurements.
    pub fn held(&self) -> (usize, usize) {
        let lru = lock(&self.lru);
        (lru.bytes, lru.map.len())
    }

    /// `compute`'s answer for `key`, from memory while `version` is the one it was kept at.
    /// `None` (a write is open) computes without keeping or sharing. A computation already
    /// running for the same key and version is joined; if it gives nothing keepable (an error,
    /// a refusal, an [`Unkept`] answer), the joiner computes its own, as without the cache.
    pub async fn serve<F>(&self, key: String, version: Option<u64>, compute: F) -> Response
    where
        F: Future<Output = Response>,
    {
        let Some(version) = version else {
            return compute.await;
        };
        if let Some(k) = self.get(&key, version) {
            return k.response(Some(Source::Cached));
        }
        let leave = Leave {
            answers: self,
            key: (key, version),
        };
        let cell = self.join(&leave.key);
        let mut compute = Some(compute);
        let mut own: Option<Response> = None;
        let (mut drove, mut hit) = (false, false);
        let (slot, own_ref, drove_ref, hit_ref) = (&mut compute, &mut own, &mut drove, &mut hit);
        let key_ref = &leave.key.0;
        let shared = cell
            .get_or_init(|| async move {
                // Kept by a flight that finished between this request's lookup and its join.
                if let Some(k) = self.get(key_ref, version) {
                    *hit_ref = true;
                    return Some(k);
                }
                *drove_ref = true;
                let res = match slot.take() {
                    Some(c) => c.await,
                    None => return None,
                };
                match Kept::take(res).await {
                    Ok(k) => {
                        let k = Arc::new(k);
                        self.insert(key_ref.clone(), version, Arc::clone(&k));
                        Some(k)
                    }
                    Err(res) => {
                        *own_ref = Some(*res);
                        None
                    }
                }
            })
            .await
            .clone();
        match (shared, own, compute) {
            (_, Some(res), _) => res,
            (Some(k), None, _) if hit => k.response(Some(Source::Cached)),
            (Some(k), None, _) => k.response((!drove).then_some(Source::Coalesced)),
            (None, None, Some(c)) => c.await,
            (None, None, None) => ApiError::INTERNAL.into_response(),
        }
    }

    /// The flight for `key` at its version, joined or started; the caller is counted as a holder
    /// until its [`Leave`] drops.
    fn join(&self, key: &(String, u64)) -> Flight {
        let mut flights = lock(&self.flights);
        let held = flights.entry(key.clone()).or_insert_with(|| Held {
            flight: Arc::new(OnceCell::new()),
            holders: 0,
        });
        held.holders += 1;
        Arc::clone(&held.flight)
    }

    /// Computations in flight, for tests.
    pub fn flights(&self) -> usize {
        lock(&self.flights).len()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Drops a request's hold on its flight; the last holder removes it, finished or not, so a
/// computation that panicked or was abandoned is started afresh by the next request. Holders are
/// counted under the map's lock, so two requests finishing together cannot both leave it behind.
struct Leave<'a> {
    answers: &'a Answers,
    key: (String, u64),
}

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        let mut flights = lock(&self.answers.flights);
        let Some(h) = flights.get_mut(&self.key) else {
            return;
        };
        h.holders = h.holders.saturating_sub(1);
        if h.holders == 0 {
            flights.remove(&self.key);
        }
    }
}

/// Middleware on a network's routes: answers the routes in [`KEPT`] through the network's cache.
pub async fn keep(State(ctx): State<Arc<NetworkContext>>, req: Request, next: Next) -> Response {
    let prefix = ctx.config.network.path_prefix();
    let tables = req
        .extensions()
        .get::<MatchedPath>()
        .and_then(|m| m.as_str().strip_prefix(prefix))
        .and_then(|m| KEPT.iter().find(|(p, _)| *p == m))
        .map(|(_, t)| *t);
    let reading = req.method() == Method::GET || req.method() == Method::HEAD;
    let (Some(tables), true, Some(table)) = (tables, reading, ctx.blocks.as_ref()) else {
        return next.run(req).await;
    };
    // `/api/v1` routes ignore the query string; `/app` routes read it or refuse it.
    let uri = req.uri();
    let key = match uri.query() {
        Some(q) if uri.path().starts_with("/app/") => format!("{}?{q}", uri.path()),
        _ => uri.path().to_owned(),
    };
    // Read before the answer's own reads, so a write that lands meanwhile changes it.
    let version = table.store.version(tables);
    ctx.answers.serve(key, version, next.run(req)).await
}

/// A segmented LRU of [`Kept`] answers, bounded by bytes.
#[derive(Debug)]
struct Lru {
    map: HashMap<String, Entry>,
    /// Entries by last use, oldest first: probation holds those used once since kept.
    probation: BTreeMap<u64, String>,
    protected: BTreeMap<u64, String>,
    clock: u64,
    bytes: usize,
    protected_bytes: usize,
    max_bytes: usize,
}

#[derive(Debug)]
struct Entry {
    kept: Arc<Kept>,
    version: u64,
    size: usize,
    used: u64,
    protected: bool,
}

impl Lru {
    fn new(max_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            probation: BTreeMap::new(),
            protected: BTreeMap::new(),
            clock: 0,
            bytes: 0,
            protected_bytes: 0,
            max_bytes,
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn get(&mut self, key: &str, version: u64) -> Option<Arc<Kept>> {
        let e = self.map.get(key)?;
        if e.version != version {
            self.remove(key);
            return None;
        }
        let (used, was_protected) = (e.used, e.protected);
        let now = self.tick();
        if was_protected {
            self.protected.remove(&used);
        } else {
            self.probation.remove(&used);
        }
        let e = self.map.get_mut(key)?;
        e.used = now;
        e.protected = true;
        let (kept, size) = (Arc::clone(&e.kept), e.size);
        self.protected.insert(now, key.to_owned());
        if !was_protected {
            self.protected_bytes += size;
        }
        // An overfull protected segment hands its oldest back to probation, as newly used there.
        while self.protected_bytes > self.max_bytes / 5 * PROTECTED_FIFTHS {
            let Some((_, oldest)) = self.protected.pop_first() else {
                break;
            };
            let t = self.tick();
            if let Some(o) = self.map.get_mut(&oldest) {
                o.protected = false;
                o.used = t;
                self.protected_bytes -= o.size;
                self.probation.insert(t, oldest);
            }
        }
        Some(kept)
    }

    fn insert(&mut self, key: String, version: u64, kept: Arc<Kept>) {
        let size = kept.size() + 2 * key.len() + ENTRY_OVERHEAD;
        self.remove(&key);
        if size > self.max_bytes / MAX_SHARE {
            return;
        }
        let used = self.tick();
        self.probation.insert(used, key.clone());
        self.map.insert(
            key,
            Entry {
                kept,
                version,
                size,
                used,
                protected: false,
            },
        );
        self.bytes += size;
        while self.bytes > self.max_bytes {
            let oldest = match self.probation.first_key_value() {
                Some((_, k)) => k.clone(),
                None => match self.protected.first_key_value() {
                    Some((_, k)) => k.clone(),
                    None => break,
                },
            };
            self.remove(&oldest);
        }
    }

    fn remove(&mut self, key: &str) {
        let Some(e) = self.map.remove(key) else {
            return;
        };
        self.bytes -= e.size;
        if e.protected {
            self.protected.remove(&e.used);
            self.protected_bytes -= e.size;
        } else {
            self.probation.remove(&e.used);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    fn ok(body: &str) -> Response {
        let mut res = Response::new(Body::from(body.to_owned()));
        res.extensions_mut().insert(ApiOutcome {
            route: "test",
            source: Source::Stored,
            upstream_error: None,
        });
        res
    }

    async fn body(res: Response) -> String {
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    fn source(res: &Response) -> Source {
        res.extensions().get::<ApiOutcome>().unwrap().source
    }

    fn kept(n: usize) -> Arc<Kept> {
        Arc::new(Kept::new(
            HeaderMap::new(),
            Bytes::from(vec![b'x'; n]),
            None,
        ))
    }

    #[tokio::test]
    async fn an_answer_is_kept_for_its_version_only() {
        let a = Answers::new(1 << 20);
        let computed = AtomicUsize::new(0);
        let run = |v: Option<u64>, text: &'static str| {
            a.serve("/k".into(), v, async {
                computed.fetch_add(1, Ordering::SeqCst);
                ok(text)
            })
        };
        let first = run(Some(2), "one").await;
        assert_eq!(source(&first), Source::Stored);
        assert_eq!(body(first).await, "one");
        let again = run(Some(2), "unused").await;
        assert_eq!(source(&again), Source::Cached);
        assert_eq!(body(again).await, "one");
        assert_eq!(computed.load(Ordering::SeqCst), 1);
        // Another version: computed again, and the old answer never served again.
        assert_eq!(body(run(Some(4), "two").await).await, "two");
        assert_eq!(body(run(Some(4), "unused").await).await, "two");
        assert!(a.get("/k", 2).is_none());
        // A write open: computed, not kept.
        assert_eq!(body(run(None, "three").await).await, "three");
        assert_eq!(computed.load(Ordering::SeqCst), 3);
        // Errors and `Unkept` answers are not kept.
        let err = a
            .serve("/e".into(), Some(0), async {
                ApiError::INTERNAL.into_response()
            })
            .await;
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let mut unkept = ok("u");
        unkept.extensions_mut().insert(Unkept);
        let _ = a.serve("/u".into(), Some(0), async { unkept }).await;
        assert!(a.get("/e", 0).is_none() && a.get("/u", 0).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_misses_share_one_computation() {
        let a = Arc::new(Answers::new(1 << 20));
        let computed = Arc::new(AtomicUsize::new(0));
        let calls: Vec<_> = (0..100)
            .map(|_| {
                let (a, computed) = (Arc::clone(&a), Arc::clone(&computed));
                tokio::spawn(async move {
                    let res = a
                        .serve("/k".into(), Some(0), async {
                            computed.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            ok("shared")
                        })
                        .await;
                    body(res).await
                })
            })
            .collect();
        for c in calls {
            assert_eq!(c.await.unwrap(), "shared");
        }
        assert_eq!(computed.load(Ordering::SeqCst), 1);
        assert_eq!(a.flights(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_request_at_an_older_version_never_displaces_a_newer_flight() {
        let a = Arc::new(Answers::new(1 << 20));
        let computed = Arc::new(AtomicUsize::new(0));
        let slow = |v: u64, text: &'static str| {
            let (a, computed) = (Arc::clone(&a), Arc::clone(&computed));
            tokio::spawn(async move {
                let res = a
                    .serve("/k".into(), Some(v), async {
                        computed.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        ok(text)
                    })
                    .await;
                body(res).await
            })
        };
        let newer = slow(4, "four");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let older = slow(2, "two");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let joiner = slow(4, "unused");
        assert_eq!(newer.await.unwrap(), "four");
        assert_eq!(older.await.unwrap(), "two");
        assert_eq!(joiner.await.unwrap(), "four");
        assert_eq!(computed.load(Ordering::SeqCst), 2);
        assert_eq!(a.flights(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_computation_is_not_shared_and_a_panic_poisons_nothing() {
        let a = Arc::new(Answers::new(1 << 20));
        // A joiner of a computation that gave an error computes its own.
        let (a1, a2) = (Arc::clone(&a), Arc::clone(&a));
        let failing = tokio::spawn(async move {
            a1.serve("/k".into(), Some(0), async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                ApiError::OVERLOADED.into_response()
            })
            .await
            .status()
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let joiner = tokio::spawn(async move {
            body(a2.serve("/k".into(), Some(0), async { ok("own") }).await).await
        });
        assert_eq!(failing.await.unwrap(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(joiner.await.unwrap(), "own");
        // A computation that panics leaves nothing behind; the next request computes.
        let a3 = Arc::clone(&a);
        let panicked = tokio::spawn(async move {
            a3.serve("/p".into(), Some(0), async { panic!("boom") })
                .await
        })
        .await;
        assert!(panicked.is_err());
        assert_eq!(a.flights(), 0);
        let res = a.serve("/p".into(), Some(0), async { ok("after") }).await;
        assert_eq!(body(res).await, "after");
        assert_eq!(
            body(a.serve("/p".into(), Some(0), async { ok("x") }).await).await,
            "after"
        );
    }

    #[test]
    fn the_cache_stays_under_its_budget_and_large_answers_are_not_kept() {
        let max = 64 << 10;
        let a = Answers::new(max);
        for i in 0..1_000 {
            a.insert(format!("/k/{i}"), 0, kept(500));
            assert!(a.held().0 <= max);
        }
        assert!(a.held().1 > 50);
        a.insert("/big".into(), 0, kept(max / MAX_SHARE));
        assert!(a.get("/big", 0).is_none());
    }

    #[test]
    fn many_distinct_keys_cannot_push_out_answers_in_use() {
        let max = 1 << 20;
        let a = Answers::new(max);
        for i in 0..100 {
            a.insert(format!("/hot/{i}"), 0, kept(1_000));
            assert!(a.get(&format!("/hot/{i}"), 0).is_some());
        }
        // A hostile client: 100,000 keys asked once each, ten times the budget.
        for i in 0..100_000 {
            a.insert(format!("/cold/{i}"), 0, kept(1_000));
        }
        assert!(a.held().0 <= max);
        for i in 0..100 {
            assert!(a.get(&format!("/hot/{i}"), 0).is_some(), "hot {i} evicted");
        }
    }
}
