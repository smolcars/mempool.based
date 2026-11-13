//! Tests fee recommendations against the blocks that follow them, read-only: runs the tracker,
//! the projected blocks and `engine::fees` against a live node, records every `SAMPLE_SECS` what
//! we, the node's `/fee-estimates` and (optionally) the reference backend recommend, and for each
//! block mined the rate that sufficed to get into it. At the end it scores every level against
//! its horizon. `docs/MEMPOOL.md`, "Fee recommendations", has the method and the results.
//!
//! `NET=MAINNET SECS=4500 OUT=fees.jsonl cargo run --release -p engine --example fee_watch`
//! reads `{NET}_RPC_HOST`, `_RPC_PORT`, `_RPC_USER`, `_RPC_PASSWORD` and `_ESPLORA_URL`.
//! - `REFERENCE_URL=<base>/v1` also asks `<base>/v1/fees/precise` every `REF_SECS` (300), with
//!   `curl`. Keep it rare: the reference is someone else's server.
//! - `FIXTURE=path` records the method's inputs once, after the warm-up, for the fixture test.
//! - `INPUTS=1` also records what a replay needs: every arrival (`{"arrived": t, "txs": [[rate,
//!   vsize], ...]}`, `{"gap": t}` when some were missed) and each sample's whole depth
//!   (`"depth": [[rate, vsize], ...]`). About 25 MB an hour at `SAMPLE_SECS=10`.
//! - `RESCORE=fees.jsonl` scores a recorded run again without a node; `MAX_GAP_SECS` (7200)
//!   is the longest gap between blocks taken as real, `FROM` and `TO` (Unix seconds) limit the
//!   samples scored.
//! - `REPLAY=fees.jsonl` replays a run recorded with `INPUTS=1` through the method at several
//!   tunings and scores each beside what was served, with the same `FROM` and `TO`;
//!   `REPLAY_OUT=path` writes the replayed samples, without their depth; `SERVED` names the
//!   tuning the recording was served with (`one@0.9`, or `quarter@0.8` before 2026-11-13). With `TRIM=path` it
//!   writes the regression fixture instead (`fixtures/fees/README.md`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// A command-line proof program: its printed output is its purpose.
#![allow(clippy::print_stdout)]

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoin::Txid;
use bitcoin::hashes::Hash;
use engine::fees::score::{self, Block, Tally};
use engine::fees::{self, Depth, Inflow, LEVELS, Recommender};
use engine::mempool::{Config as TrackerConfig, NodeSource, Tracker};
use engine::projected::{Config, Projection, Projector};
use node::esplora::Esplora;
use node::rpc::Rpc;
use serde_json::{Value, json};

/// The recorded sources, in the order the scorer prints them.
const SOURCES: [&str; 3] = ["ours", "node", "reference"];

fn var(net: &str, name: &str) -> String {
    std::env::var(format!("{net}_{name}")).unwrap_or_else(|_| panic!("{net}_{name} is not set"))
}

fn env_secs(name: &str, default: u64) -> u64 {
    std::env::var(name).map_or(default, |s| s.parse().unwrap())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Five rates in sat per 1,000 vB: the four levels and the minimum.
fn five(r: fees::Rates) -> [u32; 5] {
    let l = r.levels;
    [l[0], l[1], l[2], l[3], r.minimum]
}

/// The reference's `/v1/fees/precise`, in sat per 1,000 vB.
fn reference(base: &str) -> Option<[u32; 5]> {
    let out = std::process::Command::new("curl")
        .args(["-s", "--max-time", "10", &format!("{base}/fees/precise")])
        .output()
        .ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let mut r = [0u32; 5];
    for (slot, key) in r.iter_mut().zip([
        "fastestFee",
        "halfHourFee",
        "hourFee",
        "economyFee",
        "minimumFee",
    ]) {
        *slot = (v[key].as_f64()? * 1000.0).round() as u32;
    }
    Some(r)
}

#[tokio::main]
async fn main() {
    if let Ok(path) = std::env::var("REPLAY") {
        let text = std::fs::read_to_string(path).unwrap();
        let lines = text.lines().map(|l| serde_json::from_str(l).unwrap());
        match std::env::var("TRIM") {
            Ok(out) => trim(&lines.collect::<Vec<Value>>(), &out),
            Err(_) => replay(lines),
        }
        return;
    }
    if let Ok(path) = std::env::var("RESCORE") {
        let text = std::fs::read_to_string(path).unwrap();
        score(
            &text
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect::<Vec<Value>>(),
            &SOURCES,
        );
        return;
    }
    let net = std::env::var("NET").unwrap_or_else(|_| "MAINNET".into());
    let secs = env_secs("SECS", 4500);
    let sample = Duration::from_secs(env_secs("SAMPLE_SECS", 30));
    let ref_every = env_secs("REF_SECS", 300);
    let reference_url = std::env::var("REFERENCE_URL").ok();
    let mut fixture = std::env::var("FIXTURE").ok();
    let inputs = std::env::var("INPUTS").is_ok_and(|v| v == "1");
    let mut out =
        std::fs::File::create(std::env::var("OUT").unwrap_or_else(|_| "fees.jsonl".into()))
            .unwrap();
    let rpc = || {
        Rpc::with_limits(
            &var(&net, "RPC_HOST"),
            var(&net, "RPC_PORT").parse().unwrap(),
            &var(&net, "RPC_USER"),
            &var(&net, "RPC_PASSWORD"),
            node::rpc::Limits {
                timeout: Duration::from_secs(30),
                ..Default::default()
            },
        )
        .unwrap()
    };
    let esplora = Esplora::new(&var(&net, "ESPLORA_URL")).unwrap();
    let source = NodeSource {
        rpc: rpc(),
        esplora: esplora.clone(),
    };
    let (tracker, _t1) = Tracker::start(source, TrackerConfig::default());
    let (projector, _t2) = Projector::start(tracker.clone(), rpc(), Config::default());

    let mut rec = Recommender::default();
    // Arrivals kept for the fixture: (time, rate, vsize) over the inflow window.
    let mut arrivals: VecDeque<(u64, u32, u32)> = VecDeque::new();
    let (mut synced, mut deltas) = tracker.subscribe(|m| m.tip().is_some());
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut sampler = tokio::time::interval(sample);
    let mut last_ref = 0u64;
    let mut before: std::sync::Arc<Projection> = projector.current();
    let mut ours: Option<fees::Rates> = None;
    let mut lines: Vec<Value> = Vec::new();
    let mut write = |v: Value, lines: &mut Vec<Value>| {
        writeln!(out, "{v}").unwrap();
        lines.push(v);
    };
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => break,
            d = deltas.recv() => match d {
                Ok(d) => {
                    // The first delta of a tracker that had not synced is the whole mempool, not arrivals.
                    if !synced {
                        synced = true;
                        continue;
                    }
                    let t = now();
                    let mut added = Vec::new();
                    for (_, i) in &d.added {
                        rec.arrived(t, i.fee.to_sat(), i.vsize);
                        let rate = u32::try_from(i.fee.to_sat() * 1000 / u64::from(i.vsize.max(1))).unwrap_or(u32::MAX);
                        arrivals.push_back((t, rate, i.vsize));
                        added.push((rate, i.vsize));
                    }
                    if inputs && !added.is_empty() {
                        write(json!({"arrived": t, "txs": added}), &mut lines);
                    }
                    while arrivals.front().is_some_and(|a| t - a.0 > 31 * 60) {
                        arrivals.pop_front();
                    }
                }
                Err(_) => {
                    rec.gap(now());
                    if inputs {
                        write(json!({"gap": now()}), &mut lines);
                    }
                    (synced, deltas) = tracker.subscribe(|m| m.tip().is_some());
                }
            },
            _ = tick.tick() => {
                let p = projector.current();
                let floor = tracker.node_info().map_or(0, |i| u32::try_from(i.mempoolminfee.to_sat()).unwrap_or(u32::MAX));
                ours = rec.update(now(), &p, floor);
                if let (Some(old), Some(new)) = (before.tip, p.tip) && old != new {
                    for v in blocks(&esplora, &before, old.height, new).await {
                        println!("{v}");
                        write(v, &mut lines);
                    }
                }
                // Once the arrivals fill the window, so the fixture is a steady state.
                if let (Some(path), Some(_)) = (&fixture, ours)
                    && rec.inflow().minutes() >= fees::INFLOW_MINUTES
                {
                    record_fixture(path, &p, floor, &arrivals);
                    println!("fixture recorded to {path}");
                    fixture = None;
                }
                before = p;
            }
            _ = sampler.tick() => {
                let t = now();
                let floor = tracker.node_info().map_or(0, |i| u32::try_from(i.mempoolminfee.to_sat()).unwrap_or(u32::MAX));
                let node = esplora.fee_estimates().await.ok().and_then(|e| fees::from_node(&e.value, floor));
                let reference = match &reference_url {
                    Some(base) if t >= last_ref + ref_every => {
                        last_ref = t;
                        reference(base)
                    }
                    _ => None,
                };
                let mut v = json!({
                    "t": t,
                    "tip": before.tip.map(|t| t.height),
                    "txs": before.blocks.iter().map(|b| b.summary.txs).sum::<u32>(),
                    "inflow_minutes": rec.inflow().minutes(),
                    "floor": tracker.node_info().map(|i| i.mempoolminfee.to_sat()),
                    "bottoms": before.blocks.iter().map(|b| b.summary.min_rate).collect::<Vec<_>>(),
                    "ours": ours.map(five),
                    "node": node.map(five),
                    "reference": reference,
                });
                println!("{v}");
                if inputs {
                    v["depth"] = json!(depth_of(&before));
                    // What the live recommender treats as an outage, for the replay to do alike.
                    v["stale"] = json!(before.stale || before.tip.is_none());
                }
                write(v, &mut lines);
            }
        }
    }
    score(&lines, &SOURCES);
}

/// The blocks from `from_height + 1` to `new`, each with the rate that sufficed to get in,
/// judged by the rates `before` (the last projection on the old tip) gave its transactions.
async fn blocks(
    esplora: &Esplora,
    before: &Projection,
    from_height: u32,
    new: node::esplora::ChainTip,
) -> Vec<Value> {
    let first = if new.height > from_height {
        from_height + 1
    } else {
        new.height
    };
    let mut out = Vec::new();
    for height in first..=new.height {
        let Ok(hash) = esplora.block_hash(height).await else {
            continue;
        };
        let Ok(txids) = esplora.block_txids(hash.value).await else {
            continue;
        };
        let in_block: HashSet<Txid> = txids.value.iter().skip(1).copied().collect();
        let mut held = score::held(before, |t| in_block.contains(t));
        let min = held.iter().map(|(r, _)| *r).min();
        out.push(json!({
            "block": height,
            "t": now(),
            "txs": txids.value.len().saturating_sub(1),
            "held": held.len(),
            "min": min,
            "clearing": score::clearing(&mut held),
        }));
    }
    out
}

/// A projection's transactions as `(rate, vsize)` pairs, one per distinct rate, highest first.
fn depth_of(p: &Projection) -> Vec<(u32, u64)> {
    let mut depth: BTreeMap<u32, u64> = BTreeMap::new();
    for (_, t) in p.blocks.iter().flat_map(|b| b.txs.iter()) {
        *depth.entry(t.rate).or_default() += u64::from(t.vsize);
    }
    depth.into_iter().rev().collect()
}

/// The method's inputs at one moment, and its answer, for `crates/engine/src/fees/tests.rs`.
fn record_fixture(path: &str, p: &Projection, floor: u32, arrivals: &VecDeque<(u64, u32, u32)>) {
    let mut depth: BTreeMap<u32, u64> = BTreeMap::new();
    for (_, t) in p.blocks.iter().flat_map(|b| b.txs.iter()) {
        *depth.entry(t.rate).or_default() += u64::from(t.vsize);
    }
    let end = now();
    let start = end.saturating_sub(fees::INFLOW_MINUTES as u64 * 60);
    let mut minutes: Vec<BTreeMap<u32, u64>> = vec![BTreeMap::new(); fees::INFLOW_MINUTES];
    for &(t, rate, vsize) in arrivals.iter().filter(|a| a.0 >= start && a.0 < end) {
        let m = usize::try_from((t - start) / 60)
            .unwrap()
            .min(fees::INFLOW_MINUTES - 1);
        *minutes[m].entry(rate).or_default() += u64::from(vsize);
    }
    // Replayed exactly as the fixture test does.
    let mut inflow = Inflow::default();
    inflow.roll(0);
    for (m, arrivals) in minutes.iter().enumerate() {
        for (&rate, &vsize) in arrivals {
            inflow.record(
                m as u64 * 60,
                rate,
                u32::try_from(vsize).unwrap_or(u32::MAX),
            );
        }
    }
    inflow.roll(fees::INFLOW_MINUTES as u64 * 60);
    let expected = fees::recommend(
        &Depth::new(depth.iter().map(|(r, v)| (*r, *v))),
        &inflow,
        floor,
    );
    let v = json!({
        "tip": p.tip.map(|t| t.height),
        "floor": floor,
        "depth": depth.into_iter().collect::<Vec<_>>(),
        "inflow": minutes.into_iter().map(|m| m.into_iter().collect::<Vec<_>>()).collect::<Vec<_>>(),
        "projected_min_rates": p.blocks.iter().map(|b| b.summary.min_rate).collect::<Vec<_>>(),
        "expected": five(expected),
    });
    std::fs::write(path, v.to_string()).unwrap();
}

/// The tunings a replay compares: the confidence on block times, and the busiest minutes left
/// out of the arrivals (a quarter, one, none).
fn tunings() -> Vec<(String, fees::Tuning)> {
    let left_out: [(&str, fees::LeftOut); 3] = [
        ("quarter", |n| if n >= 5 { (n / 4).max(1) } else { 0 }),
        ("one", |n| usize::from(n >= 5)),
        ("none", |_| 0),
    ];
    let mut out = Vec::new();
    for (name, f) in left_out {
        for confidence in [0.8, 0.85, 0.9, 0.95, 0.975, 0.99] {
            out.push((
                format!("{name}@{confidence}"),
                fees::Tuning {
                    confidence,
                    busiest_left_out: f,
                },
            ));
        }
    }
    out
}

/// Replays recorded inputs: the arrivals into one inflow, each sample's depth through every
/// tuning and its own smoothing, then scores them all with what was served. A sample where ours
/// was not served is left as it was, for every tuning alike. An outage of the projection resets
/// the arrivals and smoothing as the live recommender does (recorded since `stale` was).
fn replay(lines: impl Iterator<Item = Value>) {
    let tunings = tunings();
    // The tuning the recording was served with: `one@0.9` since 2026-11-13, `quarter@0.8` before.
    let served = std::env::var("SERVED").unwrap_or_else(|_| "one@0.9".into());
    assert!(
        tunings.iter().any(|(n, _)| *n == served),
        "no tuning {served}"
    );
    let mut smoothers = vec![fees::Smoother::default(); tunings.len()];
    let mut inflow = Inflow::default();
    // As `Recommender::update`: since when the projection was stale or had no tip.
    let mut unobserved: Option<u64> = None;
    let mut out = Vec::new();
    let (mut same, mut apart, mut compared) = ([0usize; 3], [0f64; 3], 0usize);
    for mut v in lines {
        if let Some(t) = v["arrived"].as_u64() {
            for tx in v["txs"].as_array().unwrap() {
                let (rate, vsize) = (tx[0].as_u64().unwrap(), tx[1].as_u64().unwrap());
                inflow.record(t, rate as u32, vsize as u32);
            }
            continue;
        }
        if let Some(t) = v["gap"].as_u64() {
            inflow.gap(t);
            continue;
        }
        if let Some(Value::Array(depth)) = v.as_object_mut().and_then(|o| o.remove("depth")) {
            let t = v["t"].as_u64().unwrap();
            if v["stale"].as_bool() == Some(true) {
                unobserved.get_or_insert(t);
                smoothers.iter_mut().for_each(fees::Smoother::reset);
                out.push(v);
                continue;
            }
            if let Some(since) = unobserved.take()
                && t.saturating_sub(since) >= fees::OUTAGE_SECS
            {
                inflow = Inflow::default();
            }
            inflow.roll(t);
            if !inflow.ready() {
                smoothers.iter_mut().for_each(fees::Smoother::reset);
            }
            if !v["ours"].is_null() && inflow.ready() {
                let depth = Depth::new(
                    depth
                        .iter()
                        .map(|p| (p[0].as_u64().unwrap() as u32, p[1].as_u64().unwrap())),
                );
                let floor = v["floor"].as_u64().unwrap() as u32;
                let tip = v["tip"].as_u64().map(|h| node::esplora::ChainTip {
                    hash: bitcoin::BlockHash::all_zeros(),
                    height: h as u32,
                });
                for ((name, tuning), smoother) in tunings.iter().zip(&mut smoothers) {
                    let raw = fees::recommend_tuned(&depth, &inflow, floor, tuning);
                    v[name.as_str()] = json!(five(smoother.push(t, tip, raw)));
                }
                // How closely the replay of the tuning served then follows what was served.
                compared += 1;
                for i in 0..3 {
                    let (a, b) = (v[served.as_str()][i].as_f64(), v["ours"][i].as_f64());
                    same[i] += usize::from(a == b);
                    apart[i] += (a.unwrap() - b.unwrap()).abs() / 1000.0;
                }
            }
        }
        out.push(v);
    }
    // The live answer was smoothed over a tick every 2 s, the replay over these samples only.
    for (i, level) in LEVELS.iter().take(3).enumerate() {
        println!(
            "replayed {served} against what was served, {}: equal at {:.1}% of {compared} samples, {:.3} sat/vB apart on average",
            level.name,
            same[i] as f64 * 100.0 / compared as f64,
            apart[i] / compared as f64,
        );
    }
    if let Ok(path) = std::env::var("REPLAY_OUT") {
        let text: String = out.iter().map(|v| format!("{v}\n")).collect();
        std::fs::write(path, text).unwrap();
    }
    let mut sources = vec!["ours", "node", "reference"];
    sources.extend(tunings.iter().map(|(n, _)| n.as_str()));
    score(&out, &sources);
}

/// `(rate, vsize)` pairs.
type Pairs = Vec<(u64, u64)>;

/// A rate above 0.2 sat/vB to two significant figures, rounded down; below it, as it is: the
/// mempool's floor and its cheap end, where one step decides, stay exact.
fn coarse(rate: u64) -> u64 {
    if rate <= 200 {
        return rate;
    }
    let mut p = 1;
    while rate / p >= 100 {
        p *= 10;
    }
    rate / p * p
}

/// Writes the regression fixture for `crates/engine/src/fees/tests.rs` from a recording made with
/// `INPUTS=1`: one sample every `TRIM_EVERY` (180) s between `FROM` and `TO` where ours was
/// served, each with its depth down to six blocks (below that no level it scores can fit), the
/// arrivals by minute from 31 minutes before the first, and the blocks up to the sixth after the
/// last. Every rate is [`coarse`], blocks' clearing rates too, so the fixture is consistent.
fn trim(lines: &[Value], path: &str) {
    let (from, to, every) = (
        env_secs("FROM", 0),
        env_secs("TO", u64::MAX),
        env_secs("TRIM_EVERY", 180),
    );
    // The replay's inflow counts minutes from the first line that reaches it.
    let anchor = lines
        .iter()
        .find_map(|v| {
            v["arrived"]
                .as_u64()
                .or(v.get("depth").and(v["t"].as_u64()))
        })
        .unwrap();
    // (time, floor, depth)
    let mut samples: Vec<(u64, u64, Pairs)> = Vec::new();
    for v in lines {
        let t = v["t"].as_u64().unwrap_or(0);
        // The fixture's test replays no outage of the projection.
        assert!(
            v["stale"].as_bool() != Some(true) || !(from..to).contains(&t),
            "the projection was stale at {t}"
        );
        if v.get("depth").is_none()
            || v["ours"].is_null()
            || !(from..to).contains(&t)
            || samples.last().is_some_and(|s| t < s.0 + every)
        {
            continue;
        }
        let mut depth: Vec<(u64, u64)> = Vec::new();
        let mut total = 0;
        for p in v["depth"].as_array().unwrap() {
            let (rate, vsize) = (coarse(p[0].as_u64().unwrap()), p[1].as_u64().unwrap());
            match depth.last_mut() {
                Some(last) if last.0 == rate => last.1 += vsize,
                _ => depth.push((rate, vsize)),
            }
            total += vsize;
            if total >= 6 * fees::BLOCK_VSIZE {
                break;
            }
        }
        samples.push((t, v["floor"].as_u64().unwrap(), depth));
    }
    let (first, last) = (samples[0].0, samples.last().unwrap().0);
    // Arrivals under every sample's six-block cut cannot move a level it scores.
    let cut = samples
        .iter()
        .filter_map(|s| s.2.last())
        .map(|p| p.0)
        .min()
        .unwrap();
    let start = anchor + first.saturating_sub(anchor + 31 * 60) / 60 * 60;
    let mut minutes: Vec<BTreeMap<u64, u64>> =
        vec![BTreeMap::new(); ((last - start) / 60 + 1) as usize];
    for v in lines {
        if let Some(t) = v["gap"].as_u64() {
            assert!(!(start..last).contains(&t), "arrivals were missed at {t}");
        }
        let Some(t) = v["arrived"].as_u64().filter(|t| (start..last).contains(t)) else {
            continue;
        };
        for tx in v["txs"].as_array().unwrap() {
            let rate = coarse(tx[0].as_u64().unwrap());
            if rate >= cut {
                *minutes[((t - start) / 60) as usize]
                    .entry(rate)
                    .or_default() += tx[1].as_u64().unwrap();
            }
        }
    }
    let mut blocks: Vec<(u64, Option<u64>)> = lines
        .iter()
        .filter(|v| v.get("block").is_some() && v["t"].as_u64().unwrap() > first)
        .map(|v| (v["t"].as_u64().unwrap(), v["clearing"].as_u64().map(coarse)))
        .collect();
    blocks.sort_unstable();
    blocks.truncate(blocks.partition_point(|b| b.0 <= last) + 6);
    // Pairs flattened, `rate, vsize, rate, vsize …`: a quarter smaller.
    let flat = |pairs: &mut dyn Iterator<Item = (u64, u64)>| -> Vec<u64> {
        pairs.flat_map(|(r, v)| [r, v]).collect()
    };
    let minutes: Vec<Vec<u64>> = minutes
        .into_iter()
        .map(|m| flat(&mut m.into_iter()))
        .collect();
    let samples: Vec<(u64, u64, Vec<u64>)> = samples
        .into_iter()
        .map(|(t, floor, depth)| (t, floor, flat(&mut depth.into_iter())))
        .collect();
    let v = json!({ "start": start, "minutes": minutes, "samples": samples, "blocks": blocks });
    std::fs::write(path, format!("{v}\n")).unwrap();
    println!(
        "{} samples, {} minutes, {} blocks, {} bytes",
        samples.len(),
        minutes.len(),
        blocks.len(),
        v.to_string().len()
    );
}

fn pct(met: usize, total: usize) -> String {
    if total == 0 {
        return "-".into();
    }
    format!("{:.1}% of {total}", met as f64 * 100.0 / total as f64)
}

/// Three decimals, or `-` when there was nothing to average.
fn num(x: f64) -> String {
    if x.is_finite() {
        format!("{x:.3}")
    } else {
        "-".into()
    }
}

/// Scores every sample's levels, for each of `sources`, against the blocks that came after it.
/// Samples are those where ours was served (the others are the node's estimates under our
/// name), less those whose horizon reaches into an outage.
fn score(lines: &[Value], sources: &[&str]) {
    let mut blocks: Vec<Block> = lines
        .iter()
        .filter(|v| v.get("block").is_some())
        .map(|v| Block {
            height: v["block"].as_u64().unwrap() as u32,
            t: v["t"].as_u64().unwrap(),
            clearing: v["clearing"].as_u64().map(|c| c as u32),
        })
        .collect();
    blocks.sort_by_key(|b| b.t);
    let (from, to) = (env_secs("FROM", 0), env_secs("TO", u64::MAX));
    let samples: Vec<&Value> = lines
        .iter()
        .filter(|v| v.get("bottoms").is_some() && !v["ours"].is_null())
        .filter(|v| (from..to).contains(&v["t"].as_u64().unwrap()))
        .collect();
    let out = score::outages(&blocks, env_secs("MAX_GAP_SECS", 7200));
    println!(
        "\n{} samples with our own answer, {} blocks ({} without a clearing rate)",
        samples.len(),
        blocks.len(),
        blocks.iter().filter(|b| b.clearing.is_none()).count()
    );
    for (from, to) in &out {
        println!("left out: {from} to {to}, a gap between blocks and the inflow window after it");
    }
    for (title, with_ref) in [
        ("every sample with our own answer", false),
        ("moments the reference was asked", true),
    ] {
        println!("\n{title}");
        println!(
            "{:<12} {:<7} {:>5} {:>6} {:>15} {:>6} {:>8} {:>8} {:>8} {:>5} {:>16} {:>16}",
            "source",
            "level",
            "n",
            "met",
            "95% interval",
            "blocks",
            "overpay",
            "median",
            "short",
            "unsc.",
            "met when faster",
            "met when slower"
        );
        for (i, level) in LEVELS.iter().take(3).enumerate() {
            let n = level.blocks as usize;
            let mut left_out = 0;
            for source in sources {
                let mut tally = Tally::default();
                for s in &samples {
                    if with_ref && s["reference"].is_null() {
                        continue;
                    }
                    // A fit on the samples before `TO` must not see the blocks after it.
                    let rate = s[*source][i].as_u64().map(|r| r as u32);
                    tally.count(score::outcome(
                        s["t"].as_u64().unwrap(),
                        rate,
                        n,
                        &blocks,
                        &out,
                        to,
                    ));
                }
                left_out = tally.left_out;
                if tally.total == 0 {
                    if tally.unscored > 0 {
                        println!(
                            "{source:<12} {:<7} {:>5} {:>6} {:>15} {:>6} {:>8} {:>8} {:>8} {:>5}",
                            level.name, 0, "-", "-", "-", "-", "-", "-", tally.unscored
                        );
                    }
                    continue;
                }
                let (lo, hi) = tally.interval();
                println!(
                    "{source:<12} {:<7} {:>5} {:>5.1}% {:>6.1}% - {:>5.1}% {:>6} {:>8} {:>8} {:>8} {:>5} {:>16} {:>16}",
                    level.name,
                    tally.total,
                    tally.met as f64 * 100.0 / tally.total as f64,
                    lo * 100.0,
                    hi * 100.0,
                    tally.groups.len(),
                    num(tally.mean_over()),
                    num(tally.median_over()),
                    num(tally.mean_short()),
                    tally.unscored,
                    pct(tally.faster.0, tally.faster.1),
                    pct(tally.slower.0, tally.slower.1),
                );
            }
            println!(
                "{:<12} {:<7} {left_out:>5} samples left out (outage)",
                "", level.name
            );
        }
    }
    println!(
        "\nmet: share of samples whose level paid at least one step above the clearing rate of one of\nthe next N blocks. 95% interval clustered on the block each sample waited for. overpay\n(mean, median) when met and short (mean) when missed, sat/vB. unsc.: no block of the horizon\nhas a clearing rate. faster: the N-th block came within N x 10 minutes."
    );
}
