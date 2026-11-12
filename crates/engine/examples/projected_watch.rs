//! Runs the tracker and the projected-blocks engine against a live node, read-only, and
//! measures how well the first projected block agrees with the node: against its own
//! `getblocktemplate` every `CHECK_SECS`, and against every block that arrives.
//!
//! `NET=MAINNET SECS=1200 cargo run --release -p engine --example projected_watch`
//! reads `{NET}_RPC_HOST`, `_RPC_PORT`, `_RPC_USER`, `_RPC_PASSWORD` and `_ESPLORA_URL`.
//! `FIXTURE=path` also records `fixtures/projected/mainnet.json` once, then carries on.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// A command-line proof program: its printed output is its purpose.
#![allow(clippy::print_stdout)]

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use bitcoin::Txid;
use engine::mempool::{Config as TrackerConfig, NodeSource, Tracker};
use engine::projected::{Config, Event, Projector};
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

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 {
        100.0
    } else {
        a as f64 * 100.0 / b as f64
    }
}

#[tokio::main]
async fn main() {
    let net = std::env::var("NET").unwrap_or_else(|_| "MAINNET".into());
    let secs: u64 = std::env::var("SECS").map_or(1200, |s| s.parse().unwrap());
    let check = Duration::from_secs(std::env::var("CHECK_SECS").map_or(30, |s| s.parse().unwrap()));
    let fixture = std::env::var("FIXTURE").ok();
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
    let source = NodeSource {
        rpc: rpc(),
        esplora: Esplora::new(&var(&net, "ESPLORA_URL")).unwrap(),
    };
    // Each part gets its own client, so each byte count is its own.
    let (clusters, checker) = (rpc(), rpc());
    let started = Instant::now();
    let rss_before = rss_mb();
    let (tracker, tracker_task) = Tracker::start(source, TrackerConfig::default());
    let (projector, task) = Projector::start(tracker.clone(), clusters, Config::default());
    let mut events = projector.events();
    let (_, mut deltas) = projector.subscribe();

    let (mut updates, mut calls, mut bytes, mut asked) = (0u64, 0u64, 0u64, 0u64);
    let (mut pack_total, mut pack_max, mut fetch_max) =
        (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let (mut set_total, mut delta_count) = (0usize, 0u64);
    let mut synced_at: Option<Instant> = None;
    // Calls, bytes, pack time and updates after the first full projection.
    let mut steady = (0u64, 0u64, Duration::ZERO, 0u32);
    let mut alternative: Vec<(Duration, u64)> = Vec::new();
    let mut last_check = Instant::now();
    let mut last_tip = None;
    // The projection published before the current one: the last on the old tip when a block lands.
    let mut before = projector.current();
    let mut template_scores: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut seen: HashSet<Txid> = HashSet::new();
    let mut block_scores: Vec<(u32, f64, f64, f64, f64)> = Vec::new();
    let mut recorded = fixture.is_none();
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => break,
            d = deltas.recv() => {
                let Ok(d) = d else { (_, deltas) = projector.subscribe(); continue };
                delta_count += 1;
                set_total += d.set.len();
                seen.extend(d.set.iter().map(|(t, _)| *t));
                if net == "REGTEST" {
                    let p = projector.current();
                    println!("projection seq {} tip {:?} stale {}", p.seq, p.tip.map(|t| t.height), p.stale);
                    for (i, b) in p.blocks.iter().enumerate().filter(|(_, b)| !b.txs.is_empty()) {
                        println!("  block {i}: {} txs, {} vB, fees {}, rates {}..{} median {}", b.summary.txs, b.summary.vsize, b.summary.fees, b.summary.min_rate, b.summary.max_rate, b.summary.median_rate);
                        for (t, r) in &b.txs {
                            println!("    {t} fee {} vsize {} rate {:.3} sat/vB", r.fee, r.vsize, f64::from(r.rate) / 1000.0);
                        }
                    }
                }
            }
            e = events.recv() => {
                let Ok(e) = e else { continue };
                let Event::Update(s) = e else { println!("{e:?}"); continue };
                updates += 1;
                calls += u64::from(s.calls);
                bytes += s.bytes;
                asked += s.asked as u64;
                pack_total += s.pack;
                pack_max = pack_max.max(s.pack);
                if synced_at.is_some() {
                    fetch_max = fetch_max.max(s.fetch);
                    steady.0 += u64::from(s.calls);
                    steady.1 += s.bytes;
                    steady.2 += s.pack;
                    steady.3 += 1;
                }
                if synced_at.is_none() && s.txs > 0 {
                    synced_at = Some(Instant::now());
                    println!("first full projection after {:?}: {s:?}", started.elapsed());
                }
                let p = projector.current();
                // A new block: how much of it was in our first projected block just before.
                if let (Some(old), Some(new)) = (last_tip, p.tip) && old != new && synced_at.is_some() {
                    let block = checker.block(new.hash).await.unwrap();
                    let expected = block.previousblockhash.and_then(|h| projector.expected(&h));
                    if let Some(exp) = expected {
                        let ours: HashMap<Txid, u32> = exp.block.txs.iter().map(|(t, r)| (*t, r.vsize)).collect();
                        let mined: Vec<_> = block.tx.iter().skip(1).collect();
                        let hit: Vec<_> = mined.iter().filter(|t| ours.contains_key(&t.txid)).collect();
                        let mined_w: u64 = mined.iter().map(|t| u64::from(t.weight)).sum();
                        let hit_w: u64 = hit.iter().map(|t| u64::from(t.weight)).sum();
                        let mined_set: HashSet<Txid> = mined.iter().map(|t| t.txid).collect();
                        let ours_hit: Vec<_> = exp.block.txs.iter().filter(|(t, _)| mined_set.contains(t)).collect();
                        let ours_hit_vsize: u64 = ours_hit.iter().map(|(_, r)| u64::from(r.vsize)).sum();
                        let score = (
                            new.height,
                            pct(hit.len() as u64, mined.len() as u64),
                            pct(hit_w, mined_w),
                            pct(ours_hit.len() as u64, exp.block.txs.len() as u64),
                            pct(ours_hit_vsize, exp.block.summary.vsize),
                        );
                        let never: Vec<_> = mined.iter().filter(|t| !seen.contains(&t.txid)).collect();
                        let never_w: u64 = never.iter().map(|t| u64::from(t.weight)).sum();
                        println!("  of the block, {} txs ({:.2}% of its weight) never reached our mempool", never.len(), pct(never_w, mined_w));
                        // Tie-aware, as for the template: a mined transaction we had in a later
                        // block counts when an expected one the miner left out has the same rate and fee.
                        let records: HashMap<Txid, engine::projected::Placed> = before.blocks.iter().flat_map(|b| b.txs.iter().copied()).collect();
                        let mut spare: HashMap<(u32, u64), usize> = HashMap::new();
                        for (t, r) in &exp.block.txs {
                            if !mined_set.contains(t) { *spare.entry((r.rate, r.fee)).or_default() += 1; }
                        }
                        let (mut swapped, mut swapped_w, mut later, mut later_w) = (0u64, 0u64, 0u64, 0u64);
                        for t in mined.iter().filter(|t| !ours.contains_key(&t.txid)) {
                            let Some(r) = records.get(&t.txid) else { continue };
                            if let Some(n) = spare.get_mut(&(r.rate, r.fee)).filter(|n| **n > 0) {
                                *n -= 1;
                                swapped += 1;
                                swapped_w += u64::from(t.weight);
                            } else {
                                later += 1;
                                later_w += u64::from(t.weight);
                            }
                        }
                        println!(
                            "  tie-aware: {:.2}% by tx and {:.2}% by weight of the block were in our first block or tied with one there; {later} txs ({:.2}% of its weight) we had placed later; projection seq {} (expected from seq {})",
                            pct(hit.len() as u64 + swapped, mined.len() as u64), pct(hit_w + swapped_w, mined_w), pct(later_w, mined_w), before.seq, exp.seq,
                        );
                        println!(
                            "BLOCK {}: {} txs mined, {} expected; of the block {:.2}% by tx and {:.2}% by weight were in our first projected block; {:.2}% of it by tx and {:.2}% by vsize got mined. {s:?}",
                            new.height, mined.len(), exp.block.txs.len(), score.1, score.2, score.3, score.4
                        );
                        block_scores.push(score);
                    } else {
                        println!("BLOCK {} with no expected block (parent {:?})", new.height, block.previousblockhash);
                    }
                }
                last_tip = p.tip;
                before = p.clone();
                if synced_at.is_none() || last_check.elapsed() < check { continue; }
                last_check = Instant::now();
                // Our first block against the node's own template, as soon as we published.
                // The alternative: the node's template on every update. Its cost, measured.
                let (t0, b0) = (Instant::now(), checker.bytes_received());
                let template = checker.block_template().await.unwrap();
                alternative.push((t0.elapsed(), checker.bytes_received() - b0));
                let diagram = checker.mempool_feerate_diagram().await.unwrap();
                if p.tip.map(|t| t.hash) != Some(template.previousblockhash) {
                    println!("CHECK skipped: the node is on a block our tracker has not seen yet");
                    continue;
                }
                let ours: HashSet<Txid> = p.blocks[0].txs.iter().map(|(t, _)| *t).collect();
                let tpl_w: u64 = template.transactions.iter().map(|t| u64::from(t.weight)).sum();
                let both: Vec<_> = template.transactions.iter().filter(|t| ours.contains(&t.txid)).collect();
                let both_w: u64 = both.iter().map(|t| u64::from(t.weight)).sum();
                let by_tx = pct(both.len() as u64, template.transactions.len() as u64);
                let by_weight = pct(both_w, tpl_w);
                // Tie-aware: a template transaction we placed in a later block counts as agreeing
                // when one of ours that the template left out has the same rate and fee, since
                // the node picks among those in hash-map order.
                let tpl_ids: HashSet<Txid> = template.transactions.iter().map(|t| t.txid).collect();
                let mut spare: HashMap<(u32, u64), usize> = HashMap::new();
                for (t, r) in &p.blocks[0].txs {
                    if !tpl_ids.contains(t) { *spare.entry((r.rate, r.fee)).or_default() += 1; }
                }
                let (mut swapped, mut swapped_w) = (0u64, 0u64);
                for t in template.transactions.iter().filter(|t| !ours.contains(&t.txid)) {
                    if let Some(pos) = projector.position(&t.txid)
                        && let Some(n) = spare.get_mut(&(pos.placed.rate, pos.placed.fee)).filter(|n| **n > 0)
                    {
                        *n -= 1;
                        swapped += 1;
                        swapped_w += u64::from(t.weight);
                    }
                }
                let tie_tx = pct(both.len() as u64 + swapped, template.transactions.len() as u64);
                let tie_w = pct(both_w + swapped_w, tpl_w);
                println!("  tie-aware: {tie_tx:.2}% by tx, {tie_w:.2}% by weight");
                template_scores.push((by_tx, by_weight, tie_tx, tie_w));
                // Where the disagreement sits: rates of what only one side has, against the
                // lowest rate in our block (the boundary).
                let tpl_set: HashSet<Txid> = template.transactions.iter().map(|t| t.txid).collect();
                let boundary = p.blocks[0].summary.min_rate;
                let missing: Vec<u32> = template.transactions.iter().filter(|t| !ours.contains(&t.txid)).filter_map(|t| projector.position(&t.txid).map(|x| x.placed.rate)).collect();
                let unknown = template.transactions.iter().filter(|t| projector.position(&t.txid).is_none()).count();
                let extra: Vec<u32> = p.blocks[0].txs.iter().filter(|(t, _)| !tpl_set.contains(t)).map(|(_, r)| r.rate).collect();
                let rates = |v: &[u32]| { let mut m: std::collections::BTreeMap<u32, usize> = Default::default(); for r in v { *m.entry(*r).or_default() += 1; } m.into_iter().rev().take(6).collect::<Vec<_>>() };
                println!("  only in template: {} ({} unknown to us) rates {:?} | only in ours: {} rates {:?} | boundary {boundary} | tpl min fee rate {:?}", missing.len() + unknown, unknown, rates(&missing), extra.len(), rates(&extra), template.transactions.iter().map(|t| u64::from(t.fee.to_sat() as u32) * 4000 / u64::from(t.weight).max(1)).min());
                println!(
                    "CHECK t={:>5}s template {} txs {} WU, ours {} txs {} WU: {:.2}% of template txs and {:.2}% of its weight are in ours; {:.2}% of ours in it | chunks ours {} node diagram {} | mempool {} | update: pack {:?} fetch {:?} | rss {:.0} MB",
                    started.elapsed().as_secs(),
                    template.transactions.len(), tpl_w, p.blocks[0].txs.len(), p.blocks[0].summary.weight,
                    by_tx, by_weight, pct(both.len() as u64, ours.len() as u64),
                    s.chunks, diagram.len(), s.txs, s.pack, s.fetch, rss_mb(),
                );
                if !recorded && let Some(path) = &fixture {
                    recorded = record(&checker, &template, &diagram, path).await;
                }
            }
        }
    }
    task.abort();
    tracker_task.abort();
    let since = synced_at
        .map_or(Duration::ZERO, |t| t.elapsed())
        .as_secs_f64()
        .max(1.0);
    let mean = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let tx: Vec<f64> = template_scores.iter().map(|s| s.0).collect();
    let w: Vec<f64> = template_scores.iter().map(|s| s.1).collect();
    let ttx: Vec<f64> = template_scores.iter().map(|s| s.2).collect();
    let tw: Vec<f64> = template_scores.iter().map(|s| s.3).collect();
    let minutes = since / 60.0;
    println!(
        "steady state: {} updates, {:.1} calls/min, {:.1} KB/min, pack mean {:?} | template alternative: mean {:?}, {:.1} MB per call",
        steady.3,
        steady.0 as f64 / minutes,
        steady.1 as f64 / 1000.0 / minutes,
        steady.2 / steady.3.max(1),
        alternative.iter().map(|a| a.0).sum::<Duration>()
            / u32::try_from(alternative.len().max(1)).unwrap(),
        alternative.iter().map(|a| a.1 as f64).sum::<f64>() / 1e6 / alternative.len().max(1) as f64,
    );
    println!(
        "tie-aware template agreement: by tx mean {:.2}% min {:.2}%, by weight mean {:.2}% min {:.2}%",
        mean(&ttx),
        ttx.iter().copied().fold(100.0, f64::min),
        mean(&tw),
        tw.iter().copied().fold(100.0, f64::min),
    );
    println!(
        "done after {:?}: {updates} updates, {delta_count} deltas ({:.0} records each), pack mean {:?} max {:?}, fetch max {:?} after sync | cluster calls {calls}, {asked} asked, {} MB (all, sync included) | template agreement over {} checks: by tx mean {:.2}% min {:.2}%, by weight mean {:.2}% min {:.2}% | blocks {block_scores:?} | rss {:.0} MB (start {:.0} MB) | per minute since sync: {:.1} calls",
        started.elapsed(),
        set_total as f64 / delta_count.max(1) as f64,
        pack_total / u32::try_from(updates.max(1)).unwrap(),
        pack_max,
        fetch_max,
        bytes / 1_000_000,
        tx.len(),
        mean(&tx),
        tx.iter().copied().fold(100.0, f64::min),
        mean(&w),
        w.iter().copied().fold(100.0, f64::min),
        rss_mb(),
        rss_before,
        calls as f64 * 60.0 / since,
    );
}

/// Writes a small consistent slice of the node's state: the template's first transactions,
/// every member of their clusters, and the diagram's points for those chunks. Returns false
/// if the node moved under it; the next check tries again.
async fn record(
    rpc: &Rpc,
    template: &node::rpc::model::BlockTemplate,
    diagram: &[node::rpc::model::ChunkPoint],
    path: &str,
) -> bool {
    let head: Vec<Txid> = template
        .transactions
        .iter()
        .take(120)
        .map(|t| t.txid)
        .collect();
    let clusters: Vec<_> = rpc
        .mempool_clusters(&head)
        .await
        .unwrap()
        .into_iter()
        .filter_map(Result::ok)
        .collect();
    let mut seen = HashSet::new();
    let clusters: Vec<_> = clusters
        .into_iter()
        .filter(|c| seen.insert(c.chunks[0].txs[0]))
        .collect();
    let members: Vec<Txid> = clusters
        .iter()
        .flat_map(|c| c.chunks.iter().flat_map(|c| c.txs.iter().copied()))
        .collect();
    let entries = rpc.mempool_entries(&members).await.unwrap();
    let Some(entries) = entries.into_iter().collect::<Result<Vec<_>, _>>().ok() else {
        return false;
    };
    // Cut the template where a chunk ends, and take that many diagram points.
    let chunk_of: HashMap<Txid, (usize, usize, u64, i64)> = clusters
        .iter()
        .enumerate()
        .flat_map(|(i, c)| {
            c.chunks.iter().enumerate().flat_map(move |(j, ch)| {
                ch.txs
                    .iter()
                    .map(move |t| (*t, (i, j, ch.chunkweight, ch.chunkfee.to_sat())))
            })
        })
        .collect();
    let mut cut = 0;
    let mut points = Vec::new();
    for (i, t) in head.iter().enumerate() {
        let Some(c) = chunk_of.get(t) else {
            return false;
        };
        if head.get(i + 1).and_then(|n| chunk_of.get(n)) != Some(c) {
            cut = i + 1;
            points.push((c.2, c.3));
        }
    }
    let diag: Vec<(u64, i64)> = diagram
        .iter()
        .take(points.len())
        .map(|p| (p.weight, p.fee.to_sat() as i64))
        .collect();
    if diag != points {
        let i = diag
            .iter()
            .zip(&points)
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        println!(
            "fixture: diagram and template disagree at chunk {i} of {}: {:?} vs {:?}",
            points.len(),
            &diag[i..(i + 3).min(diag.len())],
            &points[i..(i + 3).min(points.len())]
        );
        return false;
    }
    let txs: Vec<serde_json::Value> = members
        .iter()
        .zip(&entries)
        .map(|(t, e)| serde_json::json!([t, e.fees.base.to_sat(), e.vsize, e.weight]))
        .collect();
    let clusters_json: Vec<serde_json::Value> = clusters
        .iter()
        .map(|c| {
            serde_json::json!({
                "clusterweight": c.clusterweight,
                "txcount": c.txcount,
                "chunks": c.chunks.iter().map(|ch| serde_json::json!({
                    "chunkfee": ch.chunkfee.to_btc(),
                    "chunkweight": ch.chunkweight,
                    "txs": ch.txs,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let out = serde_json::json!({
        "source": format!("mainnet, template for height {}", template.height),
        "txs": txs,
        "clusters": clusters_json,
        "diagram": points,
        "template": &head[..cut],
    });
    std::fs::write(path, serde_json::to_string(&out).unwrap()).unwrap();
    println!(
        "fixture: wrote {path}: {} txs, {} clusters, {} chunks, {} template txs",
        members.len(),
        clusters.len(),
        points.len(),
        cut
    );
    true
}
