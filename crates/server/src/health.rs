//! Readiness at `/readyz`, and the node check behind it. `/healthz` stays liveness: cheap and
//! independent of the node. Readiness says, per network, whether the node can be reached and how
//! old our view of the chain is. It is answered from memory: the Electrum client's state, and one
//! RPC call every [`CHECK_EVERY`] per network made by [`watch`], whatever the number of requests.
//! The first check runs at startup, so a refused password is logged at once (`rpc_auth_failed`).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use logging::events::{RpcAuthFailed, RpcReachable, RpcUnreachable};
use node::rpc::Error as RpcError;
use serde_json::{Map, json};

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
    /// Timeout, connection failure, a full work queue or nonsense: the label says which.
    Unreachable(&'static str),
}

impl Rpc {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ok => "ok",
            Self::AuthFailed => "auth_failed",
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
/// time it starts being refused, an unreachable node at the 1st, 2nd, 4th… failure in a row, and
/// the first answer and each recovery. Never stops the server: the node may simply be down.
pub async fn watch(ctx: Arc<NetworkContext>) {
    let network = ctx.config.network.name();
    let mut failures = 0u32;
    loop {
        let was = ctx.node.get().0;
        let now = match ctx.rpc.blockchain_info().await {
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
