//! Runs the mempool tracker against a live node, read-only, and prints its events, plus a
//! comparison with the node's `getmempoolinfo` and this process's memory every `CHECK_SECS`.
//!
//! `NET=MAINNET SECS=600 cargo run --release -p engine --example mempool_watch`
//! reads `{NET}_RPC_HOST`, `_RPC_PORT`, `_RPC_USER`, `_RPC_PASSWORD` and `_ESPLORA_URL`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use engine::mempool::{Config, Event, NodeSource, Removal, Tracker};
use node::esplora::Esplora;
use node::rpc::Rpc;

fn var(net: &str, name: &str) -> String {
    std::env::var(format!("{net}_{name}")).unwrap_or_else(|_| panic!("{net}_{name} is not set"))
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

#[tokio::main]
async fn main() {
    let net = std::env::var("NET").unwrap_or_else(|_| "MAINNET".into());
    let secs: u64 = std::env::var("SECS").map_or(600, |s| s.parse().unwrap());
    let check = Duration::from_secs(std::env::var("CHECK_SECS").map_or(30, |s| s.parse().unwrap()));
    let rpc = |limits| {
        Rpc::with_limits(
            &var(&net, "RPC_HOST"),
            var(&net, "RPC_PORT").parse().unwrap(),
            &var(&net, "RPC_USER"),
            &var(&net, "RPC_PASSWORD"),
            limits,
        )
        .unwrap()
    };
    let limits = node::rpc::Limits {
        timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let source = NodeSource {
        rpc: rpc(limits),
        esplora: Esplora::new(&var(&net, "ESPLORA_URL")).unwrap(),
    };
    // A separate client for the checks, so the tracker's byte counts stay its own.
    let checker = rpc(limits);
    let started = Instant::now();
    let rss_before = rss_mb();
    let (tracker, task) = Tracker::start(source, Config::default());
    let mut events = tracker.events();
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut cycles, mut bytes, mut calls, mut slowest) = (0u32, 0u64, 0u64, Duration::ZERO);
    let mut last_check = Instant::now();
    let (_, mut deltas) = tracker.subscribe(|_| ());
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => break,
            d = deltas.recv() => {
                let Ok(d) = d else { continue };
                for (txid, r) in &d.removed {
                    match r {
                        Removal::Replaced { by } => println!("replaced {txid} by {by}"),
                        Removal::Evicted => println!("evicted {txid}"),
                        Removal::Confirmed { .. } => {}
                    }
                }
                if d.removed.len() > 100 || (!d.added.is_empty() && d.added.len() <= 3 && net == "REGTEST") {
                    println!("delta seq {} tip {} +{} -{}", d.seq, d.tip.height, d.added.len(), d.removed.len());
                }
                if net == "REGTEST" {
                    for (t, i) in &d.added { println!("  added {t} fee {} vsize {} value {}", i.fee.to_sat(), i.vsize, i.value.to_sat()); }
                    for (t, r) in &d.removed { println!("  removed {t} {r:?}"); }
                }
            }
            e = events.recv() => {
                let Ok(e) = e else { continue };
                match &e {
                    Event::Cycle(s) => {
                        cycles += 1;
                        bytes += s.bytes;
                        calls += u64::from(s.calls);
                        slowest = slowest.max(s.duration);
                        *reasons.entry("confirmed").or_default() += s.confirmed;
                        *reasons.entry("replaced").or_default() += s.replaced;
                        *reasons.entry("evicted").or_default() += s.evicted;
                        if s.confirmed + s.replaced + s.evicted > 50 || s.blocks > 0 || s.added > 1000 {
                            println!("{e:?}");
                        }
                        if last_check.elapsed() >= check {
                            last_check = Instant::now();
                            let info = checker.mempool_info().await.unwrap();
                            let (ours, vsize, fees) = tracker.read(|m| (m.len(), m.vsize(), m.fees()));
                            println!(
                                "check t={:>4}s ours {ours} node {} diff {:+} | vsize ours {vsize} node {} | fees ours {} node {} | cycle {:?} {} B {} calls | rss {:.0} MB",
                                started.elapsed().as_secs(),
                                info.size,
                                ours as i64 - info.size as i64,
                                info.bytes,
                                fees.to_sat(),
                                info.total_fee.to_sat(),
                                s.duration,
                                s.bytes,
                                s.calls,
                                rss_mb(),
                            );
                        }
                    }
                    _ => println!("{e:?}"),
                }
            }
        }
    }
    task.abort();
    let size = tracker.read(|m| m.len());
    println!(
        "done after {:?}: {cycles} cycles, {} MB read, {calls} calls, slowest {slowest:?}, {size} txs, removals {reasons:?}, rss {:.0} MB (start {:.0} MB)",
        started.elapsed(),
        bytes / 1_000_000,
        rss_mb(),
        rss_before,
    );
}
