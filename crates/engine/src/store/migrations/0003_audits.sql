-- Block audits (docs/STORE.md, "audits"; the method is docs/MEMPOOL.md, "Block audits"): how each
-- block we watched arrive compared with the block we expected on its parent. Observed live, never
-- rebuildable: a block that came while the server was down, or before it had a projection, has no
-- row, and that is the "no audit" answer. Rows are keyed by hash, so a block that leaves the best
-- chain keeps its audit; lists by height join `blocks` to show only the best chain's.
CREATE TABLE audits (
    hash BLOB PRIMARY KEY,           -- 32 bytes, the order of blocks.hash
    height INTEGER NOT NULL,
    time INTEGER NOT NULL,           -- the header's timestamp: the match rate over time
    seen_at INTEGER NOT NULL,        -- Unix seconds we learnt of the block: "fresh" is measured from it
    match_rate REAL NOT NULL,        -- percent, rounded down to two decimals
    expected_txs INTEGER NOT NULL,   -- our first projected block on the parent
    expected_vsize INTEGER NOT NULL,
    expected_weight INTEGER NOT NULL,
    expected_fees INTEGER NOT NULL,
    actual_txs INTEGER NOT NULL,     -- the mined block, coinbase left out
    actual_vsize INTEGER NOT NULL,
    actual_weight INTEGER NOT NULL,
    actual_fees INTEGER NOT NULL,
    matched_txs INTEGER NOT NULL,    -- each category's transactions and virtual size
    matched_vsize INTEGER NOT NULL,
    missing_txs INTEGER NOT NULL,
    missing_vsize INTEGER NOT NULL,
    displaced_txs INTEGER NOT NULL,
    displaced_vsize INTEGER NOT NULL,
    fresh_txs INTEGER NOT NULL,
    fresh_vsize INTEGER NOT NULL,
    conflicted_txs INTEGER NOT NULL,
    conflicted_vsize INTEGER NOT NULL,
    added_txs INTEGER NOT NULL,
    added_vsize INTEGER NOT NULL,
    unseen_txs INTEGER NOT NULL,
    unseen_vsize INTEGER NOT NULL,
    txs BLOB NOT NULL                -- every category but matched, at most 1,000 45-byte records
) STRICT;

-- Pages of scores by height.
CREATE INDEX audits_height ON audits (height);
