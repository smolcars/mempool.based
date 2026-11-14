//! What the ledger reads from the node.

use std::future::Future;

use bitcoin::{BlockHash, Script};
use node::esplora::model::{ScriptInfo, TxSummary};
use node::esplora::{self, Esplora, Route, ScriptRef, ScriptSize};
use node::rpc::model::PrevoutBlock;
use node::rpc::{self, Rpc};

use super::{At, Totals};
use crate::mempool::{SourceError, esplora_err, rpc_err};

/// Everything [`super::follow`] and [`super::seed`] read from the node. [`NodeSource`] is the real
/// one; tests mock it. Each method makes its calls one after another.
pub trait Source: Send + Sync + 'static {
    /// The node's best height.
    fn height(&self) -> impl Future<Output = Result<u32, SourceError>> + Send;
    /// The best chain's block at `height`; `None` past the tip.
    fn at(&self, height: u32) -> impl Future<Output = Result<Option<At>, SourceError>> + Send;
    /// The block with every input's spent output.
    fn block(
        &self,
        hash: BlockHash,
    ) -> impl Future<Output = Result<PrevoutBlock, SourceError>> + Send;
    /// The script's confirmed totals as of block `at`, or the node's size refusal with the counts
    /// it gave. The one call that can take the node minutes.
    fn totals(
        &self,
        script: &Script,
        at: BlockHash,
    ) -> impl Future<Output = Result<Result<Totals, Option<ScriptSize>>, SourceError>> + Send;
    /// The script's newest page of confirmed history as of block `at`, newest first.
    fn page(
        &self,
        script: &Script,
        at: BlockHash,
    ) -> impl Future<Output = Result<Vec<TxSummary>, SourceError>> + Send;
}

/// The node over JSON-RPC (heights, hashes, `getblock` verbosity 3) and Esplora (totals and the
/// history page, pinned with `?asof=`). Give it clients of its own: one RPC slot with a timeout
/// that fits a full block, and an Esplora client whose timeout fits a seed.
#[derive(Clone)]
pub struct NodeSource {
    pub rpc: Rpc,
    pub esplora: Esplora,
}

/// A script as the address routes take it. The scripthash form is used, so any script works,
/// whether or not it has an address.
fn script_ref(script: &Script) -> ScriptRef {
    ScriptRef::ScriptHash(esplora::ScriptHash::from_script(script))
}

/// The answer must be stamped with the block asked for: anything else is not pinned there.
fn pinned<T>(s: esplora::Stamped<T>, at: BlockHash) -> Result<T, SourceError> {
    match s.tip {
        Some(t) if t.hash == at => Ok(s.value),
        _ => Err(SourceError::Failed(
            "answer not pinned to the block asked".into(),
        )),
    }
}

impl Source for NodeSource {
    async fn height(&self) -> Result<u32, SourceError> {
        Ok(self.rpc.blockchain_info().await.map_err(rpc_err)?.blocks)
    }

    async fn at(&self, height: u32) -> Result<Option<At>, SourceError> {
        let hash = match self.rpc.block_hash(height).await {
            Ok(h) => h,
            Err(rpc::Error::Rpc { code: -8, .. }) => return Ok(None),
            Err(e) => return Err(rpc_err(e)),
        };
        let header = match self.rpc.block_header(hash).await {
            Ok(h) => h,
            // Reorged away between the two calls.
            Err(rpc::Error::Rpc { code: -5, .. }) => return Ok(None),
            Err(e) => return Err(rpc_err(e)),
        };
        if header.confirmations < 0 || header.height != height {
            return Ok(None);
        }
        Ok(Some(At {
            height,
            hash,
            time: header.time,
        }))
    }

    async fn block(&self, hash: BlockHash) -> Result<PrevoutBlock, SourceError> {
        self.rpc.block_with_prevouts(hash).await.map_err(rpc_err)
    }

    async fn totals(
        &self,
        script: &Script,
        at: BlockHash,
    ) -> Result<Result<Totals, Option<ScriptSize>>, SourceError> {
        let route = Route::ScriptStats {
            script: script_ref(script),
            asof: Some(at),
        };
        let raw = self.esplora.raw(&route).await.map_err(esplora_err)?;
        if raw.status.as_u16() != 200 {
            return match esplora::Error::from_status(raw.status, &raw.body) {
                esplora::Error::TooLarge => Ok(Err(ScriptSize::from_refusal(&raw.body))),
                e => Err(esplora_err(e)),
            };
        }
        let info: ScriptInfo = serde_json::from_slice(&raw.body)
            .map_err(|e| SourceError::Failed(format!("totals: {e}")))?;
        let stamped = esplora::Stamped {
            value: info,
            tip: raw.tip,
        };
        Ok(Ok(pinned(stamped, at)?.chain_stats.into()))
    }

    async fn page(&self, script: &Script, at: BlockHash) -> Result<Vec<TxSummary>, SourceError> {
        let s = self
            .esplora
            .script_txs_summary(script_ref(script), None, Some(at))
            .await
            .map_err(esplora_err)?;
        pinned(s, at)
    }
}
