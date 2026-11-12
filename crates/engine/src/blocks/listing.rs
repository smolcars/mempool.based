//! A block's transactions with each one's txid, fee, weight and output value: what the block
//! summaries, the audit and the fee-rate backfill read (`docs/RBITCOIN.md`).
//!
//! rbitcoin after v0.9.2 lists them at Esplora's `/block/:hash/txs/summary`, about 0.2 MB for a
//! full mainnet block. v0.9.2 answers that path `400`, and there the only source is `getblock`
//! at verbosity 2, about 10 MB. Each network's [`Lister`] learns which one its node serves and
//! gives the same [`Block`] from either.

use std::sync::atomic::{AtomicU8, Ordering};

use bitcoin::BlockHash;
use node::esplora::{self, Esplora};
use node::rpc::model::{Block, BlockTx, Coinbase};
use node::rpc::{self, Rpc};

/// Where a block's transactions came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Esplora's `/block/:hash/txs/summary`, with the block's header and coinbase beside it.
    Route,
    /// JSON-RPC `getblock` at verbosity 2.
    GetBlock,
}

impl Via {
    /// A fixed label for logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Route => "summary_route",
            Self::GetBlock => "getblock",
        }
    }
}

/// What a [`Lister`] reports, for the server to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The source in use, each time [`Lister::detect`] gets an answer and whenever a block's
    /// answer changes it.
    Source { via: Via, why: Why },
    /// The route failed for one block, which was listed by `getblock` instead. The source stays.
    FellBack { reason: &'static str },
}

/// Why the source is what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// [`Lister::detect`] asked the node.
    Probe,
    /// The route listed a block while the source was still open.
    Answered,
    /// The node refused the route itself (a `4xx` other than `404`): it no longer serves it.
    Refused,
}

impl Why {
    pub fn label(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Answered => "answered",
            Self::Refused => "refused",
        }
    }
}

const OPEN: u8 = 0;
const ROUTE: u8 = 1;
const GETBLOCK: u8 = 2;

/// Which source lists a network's blocks. One per network, shared by everything that lists them,
/// whatever node clients each uses.
pub struct Lister {
    state: AtomicU8,
    observer: Box<dyn Fn(Event) + Send + Sync>,
}

impl std::fmt::Debug for Lister {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lister").field("via", &self.via()).finish()
    }
}

impl Default for Lister {
    fn default() -> Self {
        Self::new(|_| {})
    }
}

impl Lister {
    /// Open until the node says: the first block listed tries the route.
    pub fn new(observer: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self {
            state: AtomicU8::new(OPEN),
            observer: Box::new(observer),
        }
    }

    /// Starts at `via` and reports nothing: for measurements and tests that compare the two.
    /// [`Via::Route`] still falls back for a block the route fails on.
    pub fn fixed(via: Via) -> Self {
        let l = Self::default();
        l.state.store(code(via), Ordering::Relaxed);
        l
    }

    /// The source in use; `None` while open.
    pub fn via(&self) -> Option<Via> {
        match self.state.load(Ordering::Relaxed) {
            ROUTE => Some(Via::Route),
            GETBLOCK => Some(Via::GetBlock),
            _ => None,
        }
    }

    /// Asks the node whether it serves the route, by listing its genesis block (one row). Call it
    /// at startup and whenever the node connection is re-established, since a node can be
    /// upgraded or downgraded under us. Reports [`Event::Source`] on an answer; a node that does
    /// not answer leaves the source open, and the next block listed tries the route.
    pub async fn detect(&self, esplora: &Esplora) -> Option<Via> {
        let found = match esplora.block_hash(0).await {
            Ok(genesis) => match esplora.block_txs_summary(genesis.value).await {
                // 404 is the route's own answer for a block it cannot list (a pruned genesis);
                // v0.9.2 refuses the path itself, with 400.
                Ok(_) | Err(esplora::Error::NotFound) => Some(Via::Route),
                Err(esplora::Error::Rejected { .. }) => Some(Via::GetBlock),
                Err(_) => None,
            },
            Err(_) => None,
        };
        match found {
            Some(via) => {
                self.state.store(code(via), Ordering::Relaxed);
                (self.observer)(Event::Source {
                    via,
                    why: Why::Probe,
                });
            }
            None => self.state.store(OPEN, Ordering::Relaxed),
        }
        found
    }

    /// The block, from the route unless the node is known not to serve it, else from `getblock`.
    /// Any failure of the route falls back to `getblock` for this block; a refusal of the route
    /// also switches the network to `getblock` until the next [`Lister::detect`]. The errors are
    /// `getblock`'s.
    pub async fn block(
        &self,
        rpc: &Rpc,
        esplora: &Esplora,
        hash: BlockHash,
    ) -> (Result<Block, rpc::Error>, Via) {
        if self.state.load(Ordering::Relaxed) != GETBLOCK {
            match from_route(esplora, hash).await {
                Ok(block) => {
                    self.settle(Via::Route, Why::Answered);
                    return (Ok(block), Via::Route);
                }
                Err(Miss::Refused) => self.settle(Via::GetBlock, Why::Refused),
                Err(Miss::Failed(reason)) => (self.observer)(Event::FellBack { reason }),
            }
        }
        // shortcut: the fallback for nodes without the route (rbitcoin v0.9.2 and older); when a
        // release with it becomes the minimum, raise the pin and delete this and `Via::GetBlock`.
        (rpc.block(hash).await, Via::GetBlock)
    }

    fn settle(&self, via: Via, why: Why) {
        if self.state.swap(code(via), Ordering::Relaxed) != code(via) {
            (self.observer)(Event::Source { via, why });
        }
    }
}

fn code(via: Via) -> u8 {
    match via {
        Via::Route => ROUTE,
        Via::GetBlock => GETBLOCK,
    }
}

enum Miss {
    Refused,
    Failed(&'static str),
}

fn failed(e: &esplora::Error) -> Miss {
    match e {
        esplora::Error::Rejected { .. } => Miss::Refused,
        esplora::Error::NotFound => Miss::Failed("not_found"),
        esplora::Error::ResponseTooBig => Miss::Failed("too_big"),
        esplora::Error::Decode(_) => Miss::Failed("malformed"),
        _ => Miss::Failed("unavailable"),
    }
}

/// The block as `getblock` would give it, from the route, the block's Esplora header (height,
/// times, sizes, parent) and its coinbase (who mined it): three small calls. The header and
/// coinbase must agree with the rows, or the block is listed by `getblock` instead.
async fn from_route(esplora: &Esplora, hash: BlockHash) -> Result<Block, Miss> {
    let rows = esplora
        .block_txs_summary(hash)
        .await
        .map_err(|e| failed(&e))?
        .value;
    let first = rows.first().ok_or(Miss::Failed("malformed"))?;
    // A refusal here is not the route's: the node answered it a moment ago.
    let other = |e: esplora::Error| match failed(&e) {
        Miss::Refused => Miss::Failed("unavailable"),
        m => m,
    };
    let header = esplora.block(hash).await.map_err(other)?.value;
    let coinbase = esplora.tx_raw(first.txid).await.map_err(other)?.value;
    let mismatch = Miss::Failed("mismatch");
    if header.id != hash
        || usize::try_from(header.tx_count) != Ok(rows.len())
        || coinbase.compute_txid() != first.txid
        || !coinbase.is_coinbase()
    {
        return Err(mismatch);
    }
    let (Ok(time), Ok(mediantime)) = (
        u32::try_from(header.timestamp),
        u32::try_from(header.mediantime),
    ) else {
        return Err(Miss::Failed("malformed"));
    };
    let tx = rows
        .iter()
        .enumerate()
        .map(|(i, r)| BlockTx {
            txid: r.txid,
            // `getblock` gives the coinbase no fee; the route gives it 0.
            fee: (i > 0).then_some(r.fee),
            // BIP141 virtual size, not sigop-adjusted: what `getblock` gives as `vsize`.
            vsize: r.weight.div_ceil(4),
            weight: r.weight,
            value: r.value,
        })
        .collect();
    let script = coinbase
        .input
        .into_iter()
        .next()
        .map(|i| i.script_sig)
        .unwrap_or_default();
    Ok(Block {
        hash,
        height: header.height,
        time,
        mediantime,
        size: header.size,
        weight: header.weight,
        previousblockhash: header.previousblockhash,
        tx,
        coinbase: Coinbase {
            script,
            outputs: coinbase.output,
        },
    })
}
