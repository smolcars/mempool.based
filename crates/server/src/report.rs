//! Turns what the node clients report as values into log events. The clients themselves never
//! log: they stay free of global state and testable, and the server decides what is worth a line.

use logging::events::{
    ChainReorg, ElectrumConnected, ElectrumDisconnected, ElectrumLinesSkipped, ElectrumResynced,
    ElectrumUnreachable,
};
use node::electrum::{self, Event, Health};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::config::Network;

/// Logs one Electrum client's connection changes, resyncs and reorgs for the life of the process.
/// Call it once per client, where the client is created.
/// Repeated failures are summarised: a count is written each time it reaches a power of two.
pub fn report_electrum(network: Network, client: &electrum::Client) -> JoinHandle<()> {
    let network = network.name();
    let mut health = client.health();
    let mut events = client.events();
    let client = client.clone();
    tokio::spawn(async move {
        let mut last = Health::default();
        loop {
            tokio::select! {
                changed = health.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let now = health.borrow_and_update().clone();
                    health_changed(network, &client, &last, &now);
                    last = now;
                }
                event = events.recv() => match event {
                    Ok(Event::Resynced) => logging::emit(&ElectrumResynced { network }),
                    Ok(Event::Reorg { fork_height }) => {
                        logging::emit(&ChainReorg { network, fork_height });
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return,
                },
            }
        }
    })
}

fn health_changed(network: &'static str, client: &electrum::Client, was: &Health, now: &Health) {
    if now.ready && !was.ready {
        let version = client.server_version();
        let (server, protocol) = version
            .as_ref()
            .map_or(("", ""), |v| (v.server.as_str(), v.protocol.as_str()));
        logging::emit(&ElectrumConnected {
            network,
            server,
            protocol,
        });
    }
    if was.ready && !now.ready {
        logging::emit(&ElectrumDisconnected { network });
    }
    if let Some(attempts) = crossed_power_of_two(was.failures, now.failures) {
        logging::emit(&ElectrumUnreachable { network, attempts });
    }
    if let Some(total) = crossed_power_of_two(was.skipped_lines, now.skipped_lines) {
        logging::emit(&ElectrumLinesSkipped { network, total });
    }
}

/// `Some(now)` when a count rose past a power of two since it was last seen. Updates can be
/// coalesced, so this compares magnitudes instead of testing `now` itself.
fn crossed_power_of_two<T: Copy + Ord + Into<u64>>(was: T, now: T) -> Option<T> {
    let (a, b) = (was.into(), now.into());
    (b > a && (a == 0 || b.ilog2() > a.ilog2())).then_some(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_are_summarised_at_powers_of_two() {
        let logged: Vec<u32> = (0..100u32)
            .filter_map(|n| crossed_power_of_two(n, n + 1))
            .collect();
        assert_eq!(logged, [1, 2, 4, 8, 16, 32, 64]);
        // Coalesced jumps still report once per power crossed.
        assert_eq!(crossed_power_of_two(3u32, 6), Some(6));
        assert_eq!(crossed_power_of_two(5u32, 7), None);
        assert_eq!(
            crossed_power_of_two(4u32, 0),
            None,
            "a reset is not a failure"
        );
    }
}
