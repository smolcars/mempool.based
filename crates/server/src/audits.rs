//! Block audits (`docs/MEMPOOL.md`, "Block audits"): computed once per new block from the
//! `getblock` the block summary reads anyway and the projection the projector kept for the
//! block's parent, stored, and served by the routes here (`docs/API.md`, "Block audits").
//!
//! Only a block that is the tip when its summary is computed is audited, and only from a
//! projection that was current, on its parent, and replaced by this very block. Anything else
//! has no audit, which every route says as such.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bitcoin::{BlockHash, Txid};
use engine::audit::{self, Audit, AuditTx, Kind, Mined, Totals};
use engine::mempool::Tracker;
use engine::mining::Interval;
use engine::projected::{Expected, Projector};
use engine::store::Store;
use logging::events::{BlockAuditSkipped, BlockAudited};
use node::rpc::model::Block;
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::esplora::{ApiOutcome, Source};
use crate::{ApiError, IMMUTABLE, NetworkContext};

/// How long a new block's audit waits for the projector to move onto it. On mainnet it does
/// within about a second of the announcement, before the summary's `getblock` returns.
const WAIT: Duration = Duration::from_secs(5);
/// Blocks in a page of scores, as the reference pages them.
const PAGE: u32 = 16;
const NO_TABLE: ApiError = ApiError::new(
    StatusCode::SERVICE_UNAVAILABLE,
    "block audits need a block table on this server",
);
const NO_AUDIT: ApiError = ApiError::new(StatusCode::NOT_FOUND, "audit not available");
const CACHE: &str = "public, max-age=1";

/// The audit of `b`, a block whose `getblock` the summary just read: the stored one, or one
/// computed now if the projector moved onto `b` from a projection on its parent. `None` when it
/// has none. Whoever computes the summary first, a client or the new-block worker, gets the
/// same answer: eligibility is the projector's record, not who heard of the block when.
pub async fn for_block(ctx: &NetworkContext, b: &Block) -> Option<Totals> {
    let store = &ctx.blocks.as_ref()?.store;
    if let Ok(mut found) = audit::by_hashes(store, vec![b.hash]).await
        && let Some(Some(a)) = found.pop()
    {
        return Some(a.totals);
    }
    let (tracker, projector) = ctx.track.engines()?;
    let started = Instant::now();
    let network = ctx.config.network.name();
    match audit_new(store, tracker, projector, b).await {
        Ok(t) => {
            logging::emit(&BlockAudited {
                network,
                height: b.height,
                match_rate: t.match_rate,
                expected_txs: t.expected.txs,
                matched_txs: t.matched.txs,
                missing_txs: t.missing.txs,
                displaced_txs: t.displaced.txs,
                fresh_txs: t.fresh.txs,
                conflicted_txs: t.conflicted.txs,
                added_txs: t.added.txs,
                unseen_txs: t.unseen.txs,
                duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            });
            Some(t)
        }
        Err(reason) => {
            // Said for a block that is the tip now; an older one filled in at startup or after
            // a gap was simply not watched.
            let is_tip = ctx.electrum.tip().is_some_and(|t| t.hash == b.hash)
                || projector.current().tip.is_some_and(|t| t.hash == b.hash);
            if is_tip {
                logging::emit(&BlockAuditSkipped {
                    network,
                    height: b.height,
                    reason,
                });
            }
            None
        }
    }
}

/// Audits and stores a block that just arrived, or says why it cannot be audited (the
/// `block_audit_skipped` reasons in `docs/LOGGING.md`).
async fn audit_new(
    store: &Store,
    tracker: &Tracker,
    projector: &Projector,
    b: &Block,
) -> Result<Totals, &'static str> {
    let parent = b.previousblockhash.ok_or("no_projection")?;
    let e = expected(projector, parent).await.ok_or("no_projection")?;
    if e.stale {
        return Err("stale");
    }
    // The projection gave way to another block: a sibling (a reorg) or a later one (a burst).
    if e.next.hash != b.hash {
        return Err("superseded");
    }
    let mined: Vec<Mined> =
        b.tx.iter()
            .skip(1)
            .map(|t| Mined {
                txid: t.txid,
                fee: t.fee.map_or(0, |f| f.to_sat()),
                vsize: t.vsize,
                weight: t.weight,
            })
            .collect();
    let tracker = tracker.clone();
    let hash = b.hash;
    // When the projector learnt of this block: the same moment for every caller.
    let seen_at = e.at;
    // Off the async threads: a few milliseconds over a full block, under the tracker's lock.
    let compared = tokio::task::spawn_blocking(move || {
        tracker.read(|m| {
            // Accept times are only right while the mempool is the one just after this block.
            (m.tip().map(|t| t.hash) == Some(hash)).then(|| {
                audit::compare(
                    &e.block,
                    &e.left,
                    &mined,
                    audit::BLOCK_WEIGHT,
                    seen_at,
                    |t| m.get(t).map(|r| (r.time, r.weight)),
                )
            })
        })
    })
    .await
    .ok()
    .flatten();
    let (totals, txs) = compared.ok_or("superseded")?;
    let a = Audit {
        id: b.hash,
        height: b.height,
        time: b.time,
        seen_at,
        totals,
        txs,
    };
    audit::write(store, a).await.map_err(|_| "store")?;
    Ok(totals)
}

/// The projection kept for `parent`, once the projector has moved off it (at most [`WAIT`]).
async fn expected(p: &Projector, parent: BlockHash) -> Option<Arc<Expected>> {
    let (now, mut sub) = p.subscribe();
    let mut tip = now.tip.map(|t| t.hash);
    let moved = async {
        while tip == Some(parent) {
            tip = match sub.recv().await {
                Ok(d) => d.tip.map(|t| t.hash),
                Err(RecvError::Lagged(_)) => p.current().tip.map(|t| t.hash),
                Err(RecvError::Closed) => return,
            };
        }
    };
    let _ = tokio::time::timeout(WAIT, moved).await;
    p.expected(&parent)
}

/// The audits of these blocks, in order, for the block facts, and whether they were read: `None`
/// where there is none or the read failed, since a block's facts never wait on its audit. An
/// answer built on a failed read is not kept (`crate::answers`).
pub async fn totals(ctx: &NetworkContext, hashes: &[BlockHash]) -> (Vec<Option<Totals>>, bool) {
    let rows = match &ctx.blocks {
        Some(t) => audit::by_hashes(&t.store, hashes.to_vec()).await.ok(),
        None => Some(vec![None; hashes.len()]),
    };
    match rows {
        Some(rows) => (
            rows.into_iter().map(|a| a.map(|a| a.totals)).collect(),
            true,
        ),
        None => (vec![None; hashes.len()], false),
    }
}

pub fn routes() -> Router<Arc<NetworkContext>> {
    Router::new()
        .route("/api/v1/mining/blocks/audit/score/{hash}", get(score))
        .route("/api/v1/mining/blocks/audit/scores", get(scores_top))
        .route("/api/v1/mining/blocks/audit/scores/{height}", get(scores))
        .route("/api/v1/mining/blocks/audit/{hash}", get(detail))
        .route(
            "/api/v1/mining/blocks/predictions/{interval}",
            get(predictions),
        )
        .route("/api/v1/block/{hash}/audit-summary", get(detail))
        .route("/api/v1/block/{hash}/tx/{txid}/audit", get(tx_audit))
        .route("/app/block/{hash}/audit", get(app))
}

type Ctx = State<Arc<NetworkContext>>;

fn store(ctx: &NetworkContext) -> Result<&Store, ApiError> {
    Ok(&ctx.blocks.as_ref().ok_or(NO_TABLE)?.store)
}

fn failed(_: engine::store::Error) -> ApiError {
    ApiError::INTERNAL
}

fn block_hash(s: &str) -> Result<BlockHash, ApiError> {
    crate::esplora::block_hash(s)
}

async fn find(ctx: &NetworkContext, hash: &str) -> Result<Audit, ApiError> {
    let hash = block_hash(hash)?;
    audit::by_hash(store(ctx)?, hash)
        .await
        .map_err(failed)?
        .ok_or(NO_AUDIT)
}

fn reply(body: &impl Serialize, cache: &'static str, route: &'static str) -> Response {
    let mut res = Json(body).into_response();
    res.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    res.extensions_mut().insert(ApiOutcome {
        route,
        source: Source::Stored,
        upstream_error: None,
    });
    res
}

/// The reference's score of one block.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Score {
    hash: String,
    match_rate: f64,
    expected_fees: u64,
    expected_weight: u64,
}

async fn score(State(ctx): Ctx, Path(hash): Path<String>) -> Response {
    let run = async {
        let a = find(&ctx, &hash).await?;
        let body = Score {
            hash: a.id.to_string(),
            match_rate: a.totals.match_rate,
            expected_fees: a.totals.expected.fees,
            expected_weight: a.totals.expected.weight,
        };
        Ok::<_, ApiError>(reply(&body, IMMUTABLE, "audit_score"))
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn scores_top(State(ctx): Ctx) -> Response {
    page(&ctx, None).await
}

async fn scores(State(ctx): Ctx, Path(height): Path<String>) -> Response {
    match crate::mining::number(&height, "invalid height") {
        Ok(h) => page(&ctx, Some(h)).await,
        Err(e) => e.into_response(),
    }
}

/// The [`PAGE`] heights ending at `at` (the table's top without it), lowest first; heights
/// without an audit on the best chain are left out.
async fn page(ctx: &NetworkContext, at: Option<u32>) -> Response {
    let run = async {
        let store = store(ctx)?;
        let at = match at {
            Some(h) => h,
            None => match engine::blocks::top(store).await.map_err(failed)? {
                Some((h, _)) => h,
                None => return Ok(reply(&json!([]), CACHE, "audit_scores")),
            },
        };
        let rows = audit::scored_between(store, at.saturating_sub(PAGE - 1), at)
            .await
            .map_err(failed)?;
        let body: Vec<Score> = rows
            .iter()
            .map(|s| Score {
                hash: s.hash.to_string(),
                match_rate: s.match_rate,
                expected_fees: s.expected_fees,
                expected_weight: s.expected_weight,
            })
            .collect();
        Ok::<_, ApiError>(reply(&body, CACHE, "audit_scores"))
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

/// `[[time, height, matchRate]]`, lowest height first, for the blocks of the best chain
/// stamped within the interval.
async fn predictions(State(ctx): Ctx, Path(i): Path<String>) -> Response {
    let run = async {
        let i = Interval::parse(&i).ok_or(ApiError::bad_request("invalid interval"))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let rows = audit::scored_since(store(&ctx)?, i.start(now))
            .await
            .map_err(failed)?;
        let body: Vec<(u32, u32, f64)> = rows
            .iter()
            .map(|s| (s.time, s.height, s.match_rate))
            .collect();
        Ok::<_, ApiError>(reply(&body, CACHE, "audit_predictions"))
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

/// The reference's audit summary, as far as our categories answer it (`docs/API.md`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Detail {
    height: u32,
    id: String,
    timestamp: u32,
    unseen_txs: Vec<String>,
    missing_txs: Vec<String>,
    added_txs: Vec<String>,
    fresh_txs: Vec<String>,
    /// Ours: expected and left out, but the room went to transactions paying at least as much.
    displaced_txs: Vec<String>,
    /// Ours: expected, and gone from our mempool when the block came without being in it.
    conflicted_txs: Vec<String>,
    /// Ours: whether the lists hold every transaction of their kind.
    lists_complete: bool,
    match_rate: f64,
    expected_fees: u64,
    expected_weight: u64,
}

fn of_kind(txs: &[AuditTx], kind: Kind) -> Vec<String> {
    txs.iter()
        .filter(|t| t.kind == kind)
        .map(|t| t.txid.to_string())
        .collect()
}

async fn detail(State(ctx): Ctx, Path(hash): Path<String>) -> Response {
    let run = async {
        let a = find(&ctx, &hash).await?;
        let body = Detail {
            height: a.height,
            id: a.id.to_string(),
            timestamp: a.time,
            unseen_txs: of_kind(&a.txs, Kind::Unseen),
            missing_txs: of_kind(&a.txs, Kind::Missing),
            added_txs: of_kind(&a.txs, Kind::Added),
            fresh_txs: of_kind(&a.txs, Kind::Fresh),
            displaced_txs: of_kind(&a.txs, Kind::Displaced),
            conflicted_txs: of_kind(&a.txs, Kind::Conflicted),
            lists_complete: complete(&a),
            match_rate: a.totals.match_rate,
            expected_fees: a.totals.expected.fees,
            expected_weight: a.totals.expected.weight,
        };
        Ok::<_, ApiError>(reply(&body, IMMUTABLE, "audit_detail"))
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

fn complete(a: &Audit) -> bool {
    usize::try_from(a.totals.listed()).is_ok_and(|n| n == a.txs.len())
}

/// The reference's per-transaction answer. A transaction in none of our lists is answered as
/// matched (expected and mined): we keep only the lists, and, as on the reference, nothing
/// checks that it is in the block.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TxAudit {
    seen: bool,
    expected: bool,
    added: bool,
    prioritized: bool,
    conflict: bool,
    accelerated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_seen: Option<u32>,
}

async fn tx_audit(State(ctx): Ctx, Path((hash, txid)): Path<(String, String)>) -> Response {
    let run = async {
        let txid: Txid = crate::esplora::txid(&txid)?;
        let a = find(&ctx, &hash).await?;
        let found = a.txs.iter().find(|t| t.txid == txid);
        let kind = found.map(|t| t.kind);
        let body = TxAudit {
            seen: kind != Some(Kind::Unseen),
            expected: !matches!(kind, Some(Kind::Added | Kind::Unseen)),
            added: matches!(kind, Some(Kind::Added | Kind::Unseen)),
            prioritized: false,
            conflict: kind == Some(Kind::Conflicted),
            accelerated: false,
            first_seen: found.and_then(|t| t.first_seen),
        };
        Ok::<_, ApiError>(reply(&body, IMMUTABLE, "audit_tx"))
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

/// `/app/block/{hash}/audit`: the stored audit whole, for the block page.
#[derive(Serialize)]
struct AppAudit<'a> {
    #[serde(flatten)]
    audit: &'a Audit,
    complete: bool,
}

async fn app(State(ctx): Ctx, Path(hash): Path<String>) -> Response {
    let run = async {
        let a = find(&ctx, &hash).await?;
        let body = AppAudit {
            audit: &a,
            complete: complete(&a),
        };
        Ok::<_, ApiError>(reply(&body, IMMUTABLE, "app_audit"))
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod tests;
