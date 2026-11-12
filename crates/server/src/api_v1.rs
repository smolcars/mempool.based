//! mempool.space-shaped routes under `/api/v1` (`docs/API.md`, `docs/compat/api-v1.md`).
//!
//! The pattern for every route here: field names and types as observed on the reference backend,
//! values computed by us; answered from memory that a background task keeps current, never by a
//! call to the node per request; `{"error": …}` and a 503 with `Retry-After` until that memory is
//! filled; the chain-tip headers naming the tip the answer is as of; query strings ignored.

use std::sync::Arc;
use std::time::SystemTime;

use axum::extract::State;
use axum::http::header::{AGE, CACHE_CONTROL};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::NetworkContext;
use crate::difficulty::{Answer, unix_ms};
use crate::error::ApiError;

const NOT_READY: ApiError = ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "not ready yet; retry");

/// The answers change at most once a block, but time-based fields move every second.
const CACHE: &str = "public, max-age=1";

/// In the reference's field order. Times are Unix milliseconds except `previousTime` (seconds).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DifficultyAdjustment {
    progress_percent: f64,
    difficulty_change: f64,
    estimated_retarget_date: u64,
    remaining_blocks: u32,
    remaining_time: u64,
    previous_retarget: f64,
    previous_time: u64,
    next_retarget_height: u32,
    time_avg: u64,
    /// Kept for clients that read them: we make no adjustment and no offset.
    adjusted_time_avg: u64,
    time_offset: i64,
    expected_blocks: f64,
}

pub fn routes() -> Router<Arc<NetworkContext>> {
    Router::new()
        .route("/api/v1/difficulty-adjustment", get(difficulty_adjustment))
        .merge(crate::mining::routes())
}

async fn difficulty_adjustment(State(ctx): State<Arc<NetworkContext>>) -> Response {
    let Some(answer) = ctx.difficulty.current() else {
        return NOT_READY.into_response();
    };
    let now = SystemTime::now();
    let e = answer.period.estimate(unix_ms(now));
    let body = DifficultyAdjustment {
        progress_percent: e.progress_percent,
        difficulty_change: e.difficulty_change,
        estimated_retarget_date: e.estimated_retarget_ms,
        remaining_blocks: e.remaining_blocks,
        remaining_time: e.remaining_ms,
        previous_retarget: e.previous_retarget,
        previous_time: e.previous_time,
        next_retarget_height: e.next_retarget_height,
        time_avg: e.time_avg_ms,
        adjusted_time_avg: e.time_avg_ms,
        time_offset: 0,
        expected_blocks: e.expected_blocks,
    };
    let mut res = Json(body).into_response();
    stamp(&mut res, &ctx, &answer, now);
    res
}

/// Cache headers, the tip the answer is as of, and `Age` when the node cannot confirm the answer
/// is current (disconnected, or a newer tip not yet computed): seconds since it was computed.
fn stamp(res: &mut Response, ctx: &NetworkContext, answer: &Answer, now: SystemTime) {
    let h = res.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static(CACHE));
    if let Ok(tip) = HeaderValue::from_str(&answer.tip.to_string()) {
        h.insert("x-bitcoin-chain-tip", tip);
    }
    h.insert(
        "x-bitcoin-chain-tip-height",
        HeaderValue::from(answer.period.tip_height()),
    );
    let current = ctx.electrum.health().borrow().ready
        && ctx.electrum.tip().is_some_and(|t| t.hash == answer.tip);
    if !current {
        let age = now
            .duration_since(answer.computed)
            .unwrap_or_default()
            .as_secs();
        h.insert(AGE, HeaderValue::from(age));
    }
}
