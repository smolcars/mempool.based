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
//! - `RESCORE=fees.jsonl` scores a recorded run again without a node.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// A command-line proof program: its printed output is its purpose.
#![allow(clippy::print_stdout)]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoin::Txid;
use engine::fees::{self, Depth, Inflow, LEVELS, Recommender};
use engine::mempool::{Config as TrackerConfig, NodeSource, Tracker};
use engine::projected::{Config, Projection, Projector};
use node::esplora::Esplora;
use node::rpc::Rpc;
use serde_json::{Value, json};

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

/// The rate that sufficed to get into a block: the 1st percentile by vsize of the effective
/// rates of its transactions we held, so one prioritised or out-of-band transaction does not
/// set it. `None` when we held none of them (an empty block): no rate sufficed.
fn clearing(rates: &mut [(u32, u64)]) -> Option<u32> {
    rates.sort_unstable();
    let total: u64 = rates.iter().map(|(_, v)| v).sum();
    let mut below = 0;
    rates
        .iter()
        .find(|(_, v)| {
            below += v;
            below * 100 >= total
        })
        .map(|(r, _)| *r)
}

#[tokio::main]
async fn main() {
    if let Ok(path) = std::env::var("RESCORE") {
        let text = std::fs::read_to_string(path).unwrap();
        score(
            &text
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect::<Vec<Value>>(),
        );
        return;
    }
    let net = std::env::var("NET").unwrap_or_else(|_| "MAINNET".into());
    let secs = env_secs("SECS", 4500);
    let sample = Duration::from_secs(env_secs("SAMPLE_SECS", 30));
    let ref_every = env_secs("REF_SECS", 300);
    let reference_url = std::env::var("REFERENCE_URL").ok();
    let mut fixture = std::env::var("FIXTURE").ok();
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
                    for (_, i) in &d.added {
                        rec.arrived(t, i.fee.to_sat(), i.vsize);
                        let rate = u32::try_from(i.fee.to_sat() * 1000 / u64::from(i.vsize.max(1))).unwrap_or(u32::MAX);
                        arrivals.push_back((t, rate, i.vsize));
                    }
                    while arrivals.front().is_some_and(|a| t - a.0 > 31 * 60) {
                        arrivals.pop_front();
                    }
                }
                Err(_) => {
                    rec.gap(now());
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
                let v = json!({
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
                write(v, &mut lines);
            }
        }
    }
    score(&lines);
}

/// The blocks from `from_height + 1` to `new`, each with the rate that sufficed to get in,
/// judged by the rates `before` (the last projection on the old tip) gave its transactions.
async fn blocks(
    esplora: &Esplora,
    before: &Projection,
    from_height: u32,
    new: node::esplora::ChainTip,
) -> Vec<Value> {
    let rates: HashMap<Txid, (u32, u64)> = before
        .blocks
        .iter()
        .flat_map(|b| b.txs.iter())
        .map(|(t, p)| (*t, (p.rate, u64::from(p.vsize))))
        .collect();
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
        let mut held: Vec<(u32, u64)> = txids
            .value
            .iter()
            .skip(1)
            .filter_map(|t| rates.get(t).copied())
            .collect();
        let min = held.iter().map(|(r, _)| *r).min();
        out.push(json!({
            "block": height,
            "t": now(),
            "txs": txids.value.len().saturating_sub(1),
            "held": held.len(),
            "min": min,
            "clearing": clearing(&mut held),
        }));
    }
    out
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

/// Scores every sample's levels against the blocks that came after it.
fn score(lines: &[Value]) {
    let mut blocks: Vec<(u64, Option<u32>)> = lines
        .iter()
        .filter(|v| v.get("block").is_some())
        .map(|v| {
            (
                v["t"].as_u64().unwrap(),
                v["clearing"].as_u64().map(|c| c as u32),
            )
        })
        .collect();
    blocks.sort_by_key(|b| b.0);
    let samples: Vec<&Value> = lines.iter().filter(|v| v.get("block").is_none()).collect();
    println!("\n{} samples, {} blocks", samples.len(), blocks.len());
    // All samples where ours was served, then only the moments the reference was asked.
    for (title, with_ref) in [
        ("every sample with our own answer", false),
        ("moments the reference was asked", true),
    ] {
        println!("\n{title}");
        println!(
            "{:<10} {:<8} {:>5} {:>8} {:>16} {:>16} {:>14}",
            "source", "level", "n", "met", "mean overpay", "median overpay", "mean short"
        );
        for source in ["ours", "node", "reference"] {
            for (i, level) in LEVELS.iter().take(3).enumerate() {
                let n = level.blocks as usize;
                let (mut met, mut total, mut over, mut short) =
                    (0usize, 0usize, Vec::new(), Vec::new());
                for s in &samples {
                    if s["ours"].is_null() || (with_ref && s["reference"].is_null()) {
                        continue;
                    }
                    let Some(rate) = s[source][i].as_u64() else {
                        continue;
                    };
                    let t = s["t"].as_u64().unwrap();
                    let after: Vec<_> = blocks.iter().filter(|b| b.0 > t).take(n).collect();
                    if after.len() < n {
                        continue;
                    }
                    // The cheapest rate that got into one of the next `n` blocks: one step above
                    // a block's clearing rate.
                    let sufficed = after
                        .iter()
                        .filter_map(|b| b.1)
                        .map(|c| u64::from(c) + 1)
                        .min();
                    total += 1;
                    match sufficed {
                        Some(need) if rate >= need => {
                            met += 1;
                            over.push((rate - need) as f64 / 1000.0);
                        }
                        Some(need) => short.push((need - rate) as f64 / 1000.0),
                        None => short.push(f64::NAN),
                    }
                }
                if total == 0 {
                    continue;
                }
                over.sort_by(f64::total_cmp);
                let mean = |v: &[f64]| {
                    if v.is_empty() {
                        f64::NAN
                    } else {
                        v.iter().sum::<f64>() / v.len() as f64
                    }
                };
                println!(
                    "{source:<10} {:<8} {total:>5} {:>7.1}% {:>12.3} sat/vB {:>12.3} sat/vB {:>10.3} sat/vB",
                    level.name,
                    met as f64 * 100.0 / total as f64,
                    mean(&over),
                    over.get(over.len() / 2).copied().unwrap_or(f64::NAN),
                    mean(&short),
                );
            }
        }
    }
}
