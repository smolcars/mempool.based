//! History pages of a kept large address whose whole history the ledger holds (docs/API.md,
//! "Very large addresses"): `/txs`, `/txs/chain[/:last_seen]` and `/txs/summary[/:last_seen]` in
//! the node's shapes, the rows from the store and only the transactions on the page from the node,
//! by txid. The node is never asked for a page of the address itself, which costs it a join of the
//! address's whole history each time.

use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use engine::ledger::{self as book, HistoryRow, PageRead};
use engine::store::Store;
use node::esplora::model::{Tx, TxStatus};
use node::esplora::{ChainTip, Error, Esplora, RawResponse, Route, ScriptHash};
use serde_json::{Value, json};

use crate::NetworkContext;
use crate::esplora::{NOT_KEPT, heavy_call, respond};
use crate::ledger::Ledger;
use crate::limits::Client;

/// Rows a page holds, as the node's.
const PAGE: usize = 25;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `/txs`: unconfirmed rows, then a chain page.
    Txs,
    /// `/txs/chain`: transactions.
    Chain,
    /// `/txs/summary`: compact rows.
    Summary,
}

/// The page `route` asks of `sh`, a kept script, from the store; `None` when its whole history is
/// not held, for the caller to refuse as before. One call under the address routes' cost limit:
/// at most one unconfirmed list, one status lookup for a cursor, one lookup per transaction on the
/// page, one at a time, and the status of the block the page is exact at.
pub(crate) async fn page(
    ctx: &NetworkContext,
    client: &Client,
    ledger: &Arc<Ledger>,
    sh: ScriptHash,
    route: &Route,
    key: &str,
) -> Option<Response> {
    let (kind, script, after, asof) = match route {
        Route::ScriptTxs {
            script,
            after,
            asof,
        } => (Kind::Txs, script, *after, *asof),
        Route::ScriptTxsChain {
            script,
            last_seen,
            asof,
        } => (Kind::Chain, script, *last_seen, *asof),
        Route::ScriptTxsSummary {
            script,
            last_seen,
            asof,
        } => (Kind::Summary, script, *last_seen, *asof),
        _ => return None,
    };
    let tracked = ledger.totals(sh).await?;
    let store = Arc::clone(ledger.store());
    // Whole, and the block asked one the ledger holds, before anything is asked of the node.
    match book::history_page(&store, sh, asof, None, 0).await.ok()? {
        PageRead::Page { .. } => {}
        PageRead::AsofNotKept => return Some(NOT_KEPT.into_response()),
        PageRead::NotWhole | PageRead::CursorMissing => return None,
    }
    let hex = tracked.script.to_hex_string();
    let mempool =
        (kind == Kind::Txs && asof.is_none()).then(|| Route::ScriptTxsMempool(script.clone()));
    let mut key = key.to_owned();
    key.insert_str(0, "kept:");
    let answer = heavy_call(ctx, client, key, None, move |esplora| async move {
        serve(&store, &esplora, sh, kind, &hex, mempool, after, asof).await
    })
    .await;
    Some(match answer {
        Ok((answer, source)) => respond(route, answer, source),
        Err(refused) => *refused,
    })
}

/// The node's answer for an `after_txid` it does not know, or that is not in the history.
fn cursor_missing() -> RawResponse {
    RawResponse {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        content_type: None,
        tip: None,
        body: Bytes::from_static(b"after_txid not found"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    store: &Store,
    esplora: &Esplora,
    sh: ScriptHash,
    kind: Kind,
    hex: &str,
    mempool: Option<Route>,
    after: Option<bitcoin::Txid>,
    asof: Option<bitcoin::BlockHash>,
) -> Result<RawResponse, Error> {
    let mut out: Vec<Value> = Vec::new();
    let unconfirmed: Vec<Value> = match &mempool {
        Some(r) => {
            let raw = esplora.raw(r).await?;
            if raw.status != StatusCode::OK {
                return Ok(raw);
            }
            serde_json::from_slice(&raw.body).map_err(|e| Error::Decode(e.to_string()))?
        }
        None => Vec::new(),
    };
    let txid_of = |v: &Value| v.get("txid").and_then(Value::as_str).map(str::to_owned);
    let mut chain_after = None;
    match after {
        None => out = unconfirmed,
        // A cursor among the unconfirmed rows: the rows after it, then the first chain page.
        Some(id)
            if let Some(i) = unconfirmed
                .iter()
                .position(|v| txid_of(v) == Some(id.to_string())) =>
        {
            out = unconfirmed.into_iter().skip(i + 1).collect();
        }
        Some(id) => {
            let raw = esplora
                .raw(&Route::TxStatus {
                    txid: id,
                    asof: None,
                })
                .await?;
            if raw.status == StatusCode::NOT_FOUND {
                return Ok(cursor_missing());
            }
            if raw.status != StatusCode::OK {
                return Ok(raw);
            }
            let status: TxStatus =
                serde_json::from_slice(&raw.body).map_err(|e| Error::Decode(e.to_string()))?;
            match status.block_height.filter(|_| status.confirmed) {
                Some(height) => chain_after = Some(HistoryRow { height, txid: id }),
                // Unconfirmed, and not this address's: `/txs` starts the chain again, as the
                // node does; a chain page has no such cursor.
                None if kind == Kind::Txs => {}
                None => return Ok(cursor_missing()),
            }
        }
    }
    let (at, rows) = match book::history_page(store, sh, asof, chain_after, PAGE)
        .await
        .map_err(|_| Error::Unavailable("store".into()))?
    {
        PageRead::Page { at, rows } => (at, rows),
        PageRead::CursorMissing => return Ok(cursor_missing()),
        // Dropped or reorged away since it was checked.
        PageRead::NotWhole | PageRead::AsofNotKept => return Err(Error::ChainViewMoved),
    };
    for row in rows {
        let raw = esplora.raw(&Route::Tx(row.txid)).await?;
        if raw.status != StatusCode::OK {
            // Not found: the node's chain left the ledger's block under it.
            return Err(Error::ChainViewMoved);
        }
        let tx: Tx = serde_json::from_slice(&raw.body).map_err(|e| Error::Decode(e.to_string()))?;
        if !tx.status.confirmed || tx.status.block_height != Some(row.height) {
            return Err(Error::ChainViewMoved);
        }
        out.push(match kind {
            Kind::Txs | Kind::Chain => {
                serde_json::from_slice(&raw.body).map_err(|e| Error::Decode(e.to_string()))?
            }
            Kind::Summary => summary(&tx, hex)?,
        });
    }
    // Last, so a reorg during the lookups is seen: the block the page is exact at must still be on
    // the node's best chain, and with it every block below that the rows were confirmed in.
    let raw = esplora.raw(&Route::BlockStatus(at.hash)).await?;
    let on_best = raw.status == StatusCode::OK
        && serde_json::from_slice::<Value>(&raw.body)
            .ok()
            .and_then(|v| v.get("in_best_chain").and_then(Value::as_bool))
            == Some(true);
    if !on_best {
        return Err(Error::ChainViewMoved);
    }
    let body = serde_json::to_vec(&out).map_err(|e| Error::Decode(e.to_string()))?;
    Ok(RawResponse {
        status: StatusCode::OK,
        content_type: Some(HeaderValue::from_static("application/json")),
        tip: Some(ChainTip {
            hash: at.hash,
            height: at.height,
        }),
        body: Bytes::from(body),
    })
}

/// A `/txs/summary` row: net sats for the script, the block's height and time. The node's
/// `tx_position` is left out: the store does not hold it, and the field is optional.
fn summary(tx: &Tx, hex: &str) -> Result<Value, Error> {
    // Below the node's prune point its inputs may be partial or gone: the value cannot be counted.
    if tx.pruned {
        return Err(Error::Decode("a pruned transaction".into()));
    }
    let mut value: i64 = 0;
    for vin in &tx.vin {
        if vin.is_coinbase {
            continue;
        }
        // An input whose spent output the node no longer has cannot be counted.
        let prev = vin
            .prevout
            .as_ref()
            .ok_or_else(|| Error::Decode("a spent output is missing".into()))?;
        if prev.scriptpubkey == hex {
            value = value.saturating_sub(i64::try_from(prev.value).unwrap_or(i64::MAX));
        }
    }
    for o in &tx.vout {
        if o.scriptpubkey == hex {
            value = value.saturating_add(i64::try_from(o.value).unwrap_or(i64::MAX));
        }
    }
    Ok(json!({
        "txid": tx.txid.to_string(),
        "value": value,
        "height": tx.status.block_height,
        "time": tx.status.block_time.unwrap_or(0),
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn tx(pruned: bool, vin: Value) -> Tx {
        serde_json::from_value(json!({
            "txid": "11".repeat(32), "version": 2, "locktime": 0, "vin": vin, "pruned": pruned,
            "vout": [{"scriptpubkey": "51", "scriptpubkey_asm": "", "scriptpubkey_type": "unknown",
                "value": 400}],
            "status": {"confirmed": true, "block_height": 7, "block_time": 1_700_000_000}
        }))
        .unwrap()
    }

    #[test]
    fn a_summary_counts_both_sides_and_refuses_a_pruned_transaction() {
        let spends = json!([{"txid": "22".repeat(32), "vout": 0, "is_coinbase": false,
            "prevout": {"scriptpubkey": "51", "scriptpubkey_asm": "",
                "scriptpubkey_type": "unknown", "value": 1000}}]);
        let row = summary(&tx(false, spends), "51").unwrap();
        assert_eq!(row["value"], -600);
        assert_eq!(row["height"], 7);
        assert!(row.get("tx_position").is_none());
        // Pruned with its inputs gone: +400 would be wrong, so it is not counted at all.
        assert!(summary(&tx(true, json!([])), "51").is_err());
    }
}
