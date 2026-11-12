//! Readiness at `/readyz`, and the node check behind it. `/healthz` stays liveness: cheap and
//! independent of the node. Readiness says, per network, whether the node can be reached and how
//! old our view of the chain is. It is answered from memory: the Electrum client's state, and one
//! RPC call every [`CHECK_EVERY`] per network made by [`watch`], whatever the number of requests.
//! The first check runs at startup, so a refused password is logged at once (`rpc_auth_failed`).
//! It also says whether the node has its address index ([`watch_address_index`]) and whether the
//! store's writes commit; neither counts towards `ready`, since the explorer serves without them.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use logging::events::{
    AddressIndexOff, AddressIndexOn, RpcAuthFailed, RpcReachable, RpcUnreachable, RpcWrongNetwork,
};
use node::electrum::Event as ElectrumEvent;
use node::esplora::{Error as EsploraError, ScriptHash, ScriptRef};
use node::rpc::Error as RpcError;
use serde_json::{Map, json};
use tokio::sync::broadcast::error::RecvError;

use crate::NetworkContext;

/// Time between two RPC checks of one network.
pub const CHECK_EVERY: Duration = Duration::from_secs(30);

/// What the last RPC check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rpc {
    /// No check has finished yet.
    Pending,
    Ok,
    /// The node refused our user and password.
    AuthFailed,
    /// The node answered for another chain than the network configured.
    WrongNetwork,
    /// Timeout, connection failure, a full work queue or nonsense: the label says which.
    Unreachable(&'static str),
}

impl Rpc {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ok => "ok",
            Self::AuthFailed => "auth_failed",
            Self::WrongNetwork => "wrong_network",
            Self::Unreachable(_) => "unreachable",
        }
    }
}

/// One network's node check, held in [`NetworkContext`].
#[derive(Debug)]
pub struct NodeCheck(Mutex<(Rpc, Option<Instant>)>);

impl Default for NodeCheck {
    fn default() -> Self {
        Self(Mutex::new((Rpc::Pending, None)))
    }
}

impl NodeCheck {
    fn get(&self) -> (Rpc, Option<Instant>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, rpc: Rpc) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = (rpc, Some(Instant::now()));
    }
}

/// Checks the network's RPC now and every [`CHECK_EVERY`], forever. Logs a refused password each
/// time it starts being refused, a node on another chain each time it is found, an unreachable node at the 1st, 2nd, 4th… failure in a row, and
/// the first answer and each recovery. Never stops the server: the node may simply be down.
pub async fn watch(ctx: Arc<NetworkContext>) {
    let network = ctx.config.network.name();
    let mut failures = 0u32;
    loop {
        let was = ctx.node.get().0;
        let now = match ctx.rpc.blockchain_info().await {
            Ok(info) if info.chain != ctx.config.network.rpc_chain() => {
                if was != Rpc::WrongNetwork {
                    let node_chain: String = info.chain.chars().take(16).collect();
                    logging::emit(&RpcWrongNetwork {
                        network,
                        node_chain: &node_chain,
                    });
                }
                Rpc::WrongNetwork
            }
            Ok(info) => {
                if was != Rpc::Ok {
                    logging::emit(&RpcReachable {
                        network,
                        height: info.blocks,
                        failures,
                    });
                }
                failures = 0;
                Rpc::Ok
            }
            Err(RpcError::Auth) => {
                if was != Rpc::AuthFailed {
                    logging::emit(&RpcAuthFailed { network });
                }
                Rpc::AuthFailed
            }
            Err(e) => {
                let error = match e {
                    RpcError::Timeout => "timeout",
                    RpcError::Busy => "busy",
                    RpcError::Decode(_) => "decode",
                    _ => "unavailable",
                };
                failures = failures.saturating_add(1);
                if failures.is_power_of_two() {
                    logging::emit(&RpcUnreachable {
                        network,
                        error,
                        failures,
                    });
                }
                Rpc::Unreachable(error)
            }
        };
        ctx.node.set(now);
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

/// Whether the node has its address index, as last asked: `unknown`, `on` or `off`.
#[derive(Debug, Default)]
pub struct AddressIndex(AtomicU8);

impl AddressIndex {
    fn label(&self) -> &'static str {
        match self.0.load(Ordering::Relaxed) {
            1 => "on",
            2 => "off",
            _ => "unknown",
        }
    }
}

/// A scripthash no script hashes to, so asking for its stats is one index lookup that finds
/// nothing: the cheapest question the address routes answer.
const NO_SCRIPT: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Asks the node whether it has its address index (`--sh-index`) once Electrum is first ready,
/// then after every reconnect, since the node may have been restarted with other flags, and every
/// [`CHECK_EVERY`] while the question has no answer. Logs the first answer and each change.
pub async fn watch_address_index(ctx: Arc<NetworkContext>) {
    let Ok(probe) = NO_SCRIPT.parse::<ScriptHash>() else {
        return;
    };
    // Subscribed before the first question, so a reconnect during it is still heard.
    let mut events = ctx.electrum.events();
    if ctx.electrum.health().wait_for(|h| h.ready).await.is_err() {
        return;
    }
    let network = ctx.config.network.name();
    loop {
        let found = match ctx
            .esplora
            .script_stats(ScriptRef::ScriptHash(probe), None)
            .await
        {
            Ok(_) => Some(1),
            Err(EsploraError::AddressIndexDisabled) => Some(2),
            // No answer says nothing about the index.
            Err(_) => None,
        };
        if let Some(found) = found
            && ctx.address_index.0.swap(found, Ordering::Relaxed) != found
        {
            if found == 1 {
                logging::emit(&AddressIndexOn { network });
            } else {
                logging::emit(&AddressIndexOff { network });
            }
        }
        let retry = tokio::time::sleep(if found.is_some() {
            Duration::MAX
        } else {
            CHECK_EVERY
        });
        tokio::pin!(retry);
        loop {
            tokio::select! {
                e = events.recv() => match e {
                    // A lagged receiver may have missed a reconnect.
                    Ok(ElectrumEvent::Resynced) | Err(RecvError::Lagged(_)) => break,
                    Ok(_) => {}
                    Err(RecvError::Closed) => return,
                },
                () = &mut retry => break,
            }
        }
    }
}

/// The store's state for `/readyz`: `off` without `DATA_DIR`, `open_failed`, `failing` while
/// SQLite refuses writes, else `ok`; and how long ago a write last committed.
fn store(ctx: &NetworkContext) -> (&'static str, Option<u64>) {
    match &ctx.blocks {
        Some(t) => {
            let h = t.store.write_health();
            let state = if h.failures > 0 { "failing" } else { "ok" };
            (state, h.last_ok.map(|t| t.elapsed().as_secs()))
        }
        None if ctx.data_dir => ("open_failed", None),
        None => ("off", None),
    }
}

/// `GET /readyz`: `200` when every enabled network's node answers RPC with our credentials and
/// its Electrum connection is up, `503` otherwise. The body says which, per network.
pub fn readyz(networks: &[Arc<NetworkContext>]) -> Response {
    let unix_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut all_ready = true;
    let mut out = Map::new();
    for ctx in networks {
        let (rpc, checked) = ctx.node.get();
        let electrum = ctx.electrum.health().borrow().ready;
        let tip = ctx.electrum.tip();
        let ready = rpc == Rpc::Ok && electrum;
        let (store, written) = store(ctx);
        all_ready &= ready;
        let n = json!({
            "ready": ready,
            "rpc": rpc.label(),
            "rpc_error": match rpc {
                Rpc::Unreachable(error) => Some(error),
                _ => None,
            },
            "rpc_checked_secs_ago": checked.map(|c| c.elapsed().as_secs()),
            "electrum": electrum,
            "tip_height": tip.as_ref().map(|t| t.height),
            // How old the newest block we know of is, by its own timestamp.
            "tip_age_secs": tip.map(|t| unix_now.saturating_sub(u64::from(t.header.time))),
            // Informational only: the explorer is ready whatever the Lightning peer is doing.
            "lightning": ctx.lightning.as_ref().map_or("off", |l| l.state()),
            // Informational too: without them address pages, or recorded history, stop.
            "address_index": ctx.address_index.label(),
            "store": store,
            "store_written_secs_ago": written,
        });
        out.insert(ctx.config.network.name().to_owned(), n);
    }
    let status = if all_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = json!({
        "status": if all_ready { "ready" } else { "not_ready" },
        "networks": out,
    });
    (status, Json(body)).into_response()
}
