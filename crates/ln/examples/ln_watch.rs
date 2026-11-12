//! Runs the gossip peer against the public Lightning network and prints its events, with this
//! process's memory and the node's cost at every stats line. Outbound connections only; no channel
//! is ever opened. Funding outputs are looked up on the rbitcoin node at `ESPLORA_URL` (read-only).
//!
//! `ESPLORA_URL=http://… LN_DIR=/some/dir SECS=1800 cargo run --release -p ln --example ln_watch`
//! `LOOKUP_MS=n` sets the least time between two lookup requests (default 500).
//! `PEERS=<pubkey>@<ip>:<port>,…` replaces the default peers; `MAX_PEERS=n` changes how many
//! are kept connected. `TOP=20` prints, at the end, the
//! best-connected nodes that announce a clearnet address, and checks the default peers against
//! their announcements.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A command-line proof program: its printed output is its purpose.
#![allow(clippy::print_stdout)]

use std::time::{Duration, Instant};

use ln::{Config, Event, Level, Network, NodeId, Peer, PeerAddr, SocketAddress};

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

fn alias(a: &[u8; 32]) -> String {
    // Printed through Debug so control characters in an alias cannot reach the terminal.
    format!("{:?}", String::from_utf8_lossy(a).trim_end_matches('\0'))
}

#[tokio::main]
async fn main() {
    let dir = std::env::var("LN_DIR").expect("LN_DIR is not set");
    let secs: u64 = std::env::var("SECS").map_or(600, |s| s.parse().unwrap());
    let top: usize = std::env::var("TOP").map_or(0, |s| s.parse().unwrap());
    let esplora =
        node::esplora::Esplora::new(&std::env::var("ESPLORA_URL").expect("ESPLORA_URL is not set"))
            .unwrap();
    let mut config = Config::new(Network::Bitcoin, dir.into());
    if let Ok(ms) = std::env::var("LOOKUP_MS") {
        config.lookup_interval = Duration::from_millis(ms.parse().unwrap());
    }
    if let Ok(p) = std::env::var("PEERS") {
        config.peers = p
            .split(',')
            .map(|s| s.parse::<PeerAddr>().unwrap())
            .collect();
    }
    if let Ok(n) = std::env::var("MAX_PEERS") {
        config.max_peers = n.parse().unwrap();
    }
    config.log_level = Level::Info;

    let started = Instant::now();
    let (peer, load) = Peer::start(config.clone(), esplora.clone()).unwrap();
    println!(
        "{:>7.1}s start node_id={} load={load:?} rss_mb={:.0}",
        0.0,
        peer.node_id(),
        rss_mb()
    );
    let mut events = peer.events();
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        let e = tokio::select! {
            () = &mut deadline => break,
            _ = tokio::signal::ctrl_c() => break,
            e = events.recv() => e,
        };
        let t = started.elapsed().as_secs_f64();
        match e {
            Ok(Event::Stats(s)) => println!(
                "{t:>7.1}s stats nodes={} channels={} pending={} peers={} msg/s={:.0} kB/s={:.1} rss_mb={:.0} node_kb={} validation={:?} totals={:?}",
                s.nodes,
                s.channels,
                s.pending_channels,
                s.peers,
                s.messages_per_sec,
                s.bytes_per_sec / 1000.0,
                rss_mb(),
                esplora.bytes_received() / 1000,
                s.validation,
                s.totals
            ),
            Ok(Event::Ldk(r)) => println!(
                "{t:>7.1}s ldk {:?} {}:{} {:?}",
                r.level, r.module, r.line, r.message
            ),
            Ok(e) => println!("{t:>7.1}s {e:?}"),
            Err(e) => println!("{t:>7.1}s events {e:?}"),
        }
    }

    peer.read(|g| {
        let (mut one, mut both, mut announced) = (0, 0, 0);
        for (_, c) in g.channels().unordered_iter() {
            match (c.one_to_two.is_some(), c.two_to_one.is_some()) {
                (true, true) => both += 1,
                (false, false) => {}
                _ => one += 1,
            }
        }
        for (_, n) in g.nodes().unordered_iter() {
            announced += usize::from(n.announcement_info.is_some());
        }
        let capacity: u64 = g
            .channels()
            .unordered_iter()
            .filter_map(|(_, c)| c.capacity_sats)
            .sum();
        println!(
            "graph channels={} with_policy_both={both} with_policy_one={one} nodes={} announced_nodes={announced} capacity_btc={:.2}",
            g.channels().len(),
            g.nodes().len(),
            capacity as f64 / 1e8
        );
        // A few channels to check by hand against their funding outputs.
        for (scid, c) in g.channels().unordered_iter().step_by(7919).take(5) {
            println!(
                "sample scid={}x{}x{} capacity_sats={:?}",
                scid >> 40,
                (scid >> 16) & 0xff_ffff,
                scid & 0xffff,
                c.capacity_sats
            );
        }
    });
    println!(
        "status {:?} node_kb={}",
        peer.status().borrow().validation,
        esplora.bytes_received() / 1000
    );
    if top > 0 {
        peer.read(|g| {
            let mut by_channels: Vec<(usize, NodeId)> = g
                .nodes()
                .unordered_iter()
                .map(|(id, n)| (n.channels.len(), *id))
                .collect();
            by_channels.sort_unstable_by(|a, b| b.cmp(a));
            let clearnet = |a: &SocketAddress| {
                matches!(
                    a,
                    SocketAddress::TcpIpV4 { .. } | SocketAddress::TcpIpV6 { .. }
                )
            };
            let mut shown = 0;
            for (n, id) in by_channels {
                let Some(info) = g.node(&id).and_then(|n| n.announcement_info.as_ref()) else {
                    continue;
                };
                let addrs: Vec<String> = info
                    .addresses()
                    .iter()
                    .filter(|a| clearnet(a))
                    .map(|a| a.to_string())
                    .collect();
                if addrs.is_empty() {
                    continue;
                }
                println!(
                    "top {n:>5} channels {id} {} {addrs:?}",
                    alias(&info.alias().0)
                );
                shown += 1;
                if shown == top {
                    break;
                }
            }
            for p in &config.peers {
                let node = g.node(&NodeId::from_pubkey(&p.node_id));
                let ann = node.and_then(|n| n.announcement_info.as_ref());
                let announced: Vec<String> = ann
                    .map(|i| i.addresses().iter().map(|a| a.to_string()).collect())
                    .unwrap_or_default();
                println!(
                    "default {p} channels={} alias={} announced={announced:?} matches={}",
                    node.map_or(0, |n| n.channels.len()),
                    ann.map_or_else(String::new, |i| alias(&i.alias().0)),
                    announced.contains(&p.addr.to_string()),
                );
            }
        });
    }

    let bytes = peer.shutdown().await.unwrap();
    println!(
        "{:>7.1}s stopped graph_bytes={bytes}",
        started.elapsed().as_secs_f64()
    );
}
