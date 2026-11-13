//! Projected blocks: the blocks that would be mined next, from the node's own mining chunks.
//! Design, agreement with the node and costs: `docs/MEMPOOL.md`, "Projected blocks".
//!
//! ```no_run
//! # async fn f(tracker: engine::mempool::Tracker, rpc: node::rpc::Rpc, txid: bitcoin::Txid) {
//! use engine::projected::{Config, Projector};
//! let (projector, _task) = Projector::start(tracker, rpc, Config::default());
//! let (projection, mut deltas) = projector.subscribe();
//! while let Ok(delta) = deltas.recv().await {
//!     // delta.seq follows projection.seq without a gap; apply delta.removed, then delta.set.
//! }
//! let where_it_is = projector.position(&txid);
//! # }
//! ```

mod model;
mod projector;

pub(crate) use model::BAND_EDGES;
pub use model::{
    BANDS, Bands, BlockSummary, Delta, Group, GroupChunk, Placed, Position, ProjectedBlock,
    Projection, STORED_BANDS, band,
};
pub use projector::{ClusterSource, Config, Event, Expected, Projector, Subscription, UpdateStats};

#[cfg(test)]
pub(crate) mod tests;
