//! `GET /app/tx/:txid/relatives`: the group a mempool transaction will be mined with
//! (`docs/FRONTEND.md`).
//!
//! Answered from memory, never from the node: the cluster and its chunks are the node's own, as
//! the projected blocks last asked for them (`getmempoolcluster`, at most one update behind), and
//! which member spends which comes from the inputs the mempool tracker holds for each. So a
//! request costs a few hash-map lookups under the engines' read locks, and needs no caps of its
//! own beyond the ordinary request limits.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bitcoin::Txid;
use serde_json::{Value, json};

use crate::ws::track::{MEMPOOL_OFF, MEMPOOL_SYNCING};
use crate::{ApiError, NetworkContext, esplora};

/// In the tracker but not yet placed by the projected blocks: a moment after it arrived.
const NOT_PLACED: ApiError = ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "not placed yet");
const NOT_IN_MEMPOOL: ApiError = ApiError::new(StatusCode::NOT_FOUND, "not in the mempool");

pub async fn route(
    State(ctx): State<Arc<NetworkContext>>,
    Path(txid): Path<String>,
) -> Result<Response, ApiError> {
    let txid = esplora::txid(&txid)?;
    if !ctx.track.mempool_enabled() {
        return Err(MEMPOOL_OFF);
    }
    // The projected blocks' lock is held through a pack (up to about 170 ms on mainnet): wait
    // for it on a blocking thread, not an async worker.
    let body = tokio::task::spawn_blocking(move || answer(&ctx, txid))
        .await
        .map_err(|_| ApiError::INTERNAL)??;
    let mut res = (StatusCode::OK, Json(body)).into_response();
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

fn answer(ctx: &NetworkContext, txid: Txid) -> Result<Value, ApiError> {
    let (tracker, projector) = ctx.track.engines().ok_or(MEMPOOL_SYNCING)?;
    let (synced, known) = tracker.read(|m| (m.tip().is_some(), m.get(&txid).is_some()));
    if !synced {
        return Err(MEMPOOL_SYNCING);
    }
    if !known {
        return Err(NOT_IN_MEMPOOL);
    }
    let group = projector.group(&txid).ok_or(NOT_PLACED)?;
    let stale = projector.current().stale;
    let members: Vec<Txid> = group
        .chunks
        .iter()
        .flat_map(|c| c.txs.iter().map(|(t, _)| *t))
        .collect();
    let (links, records) = tracker.read(|m| {
        let records: Vec<Option<(u32, u64, u32)>> = members
            .iter()
            .map(|t| m.get(t).map(|i| (i.time, i.fee.to_sat(), i.vsize)))
            .collect();
        (m.links(&members), records)
    });
    let me = members.iter().position(|t| *t == txid);
    let ids = |pick: &dyn Fn(&(usize, usize)) -> Option<usize>| -> Vec<Txid> {
        links
            .iter()
            .filter_map(pick)
            .filter_map(|i| members.get(i).copied())
            .collect()
    };
    let parents = ids(&|(p, c)| (Some(*c) == me).then_some(*p));
    let children = ids(&|(p, c)| (Some(*p) == me).then_some(*c));
    let mut txs = Vec::with_capacity(members.len());
    let mut i = 0;
    for (n, chunk) in group.chunks.iter().enumerate() {
        for (t, placed) in &chunk.txs {
            let parents: Vec<&Txid> = links
                .iter()
                .filter(|(_, c)| *c == i)
                .filter_map(|(p, _)| members.get(*p))
                .collect();
            // A member the tracker no longer holds left a moment ago: no figures.
            let r = records.get(i).copied().flatten();
            txs.push(json!({
                "txid": t,
                "chunk": n,
                "time": r.map(|r| r.0),
                "fee": r.map(|r| r.1),
                "vsize": r.map(|r| r.2),
                "block": placed.map(|p| p.block),
                "parents": parents,
            }));
            i += 1;
        }
    }
    let chunks: Vec<Value> = group
        .chunks
        .iter()
        .map(|c| {
            json!({
                "fee": c.fee,
                "vsize": c.weight.div_ceil(4),
                "rate": c.rate,
                "txs": c.txs.iter().map(|(t, _)| t).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "txid": txid,
        "stale": stale,
        "chunk": group.chunk,
        "parents": parents,
        "children": children,
        "chunks": chunks,
        "txs": txs,
    }))
}
