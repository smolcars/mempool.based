-- The address ledger (docs/STORE.md, "ledger"): running totals the server keeps for a bounded set
-- of very large addresses, whose totals the node refuses or takes minutes to give. Seeded once
-- from the node's totals pinned to a block, then kept by reading each new block. All tracked
-- scripts are exact at one block, the highest row of ledger_chain. Rebuildable: each script can
-- be seeded again from the node.

-- One row per tracked script.
CREATE TABLE ledger_scripts (
    sh BLOB PRIMARY KEY,            -- Electrum scripthash, 32 bytes as shown (byte-reversed SHA-256)
    script BLOB NOT NULL,           -- the scriptPubKey, matched against each block's outputs
    funded_count INTEGER NOT NULL,  -- chain_stats as the node's /address/:a gives them
    funded_sum INTEGER NOT NULL,    -- sats
    spent_count INTEGER NOT NULL,
    spent_sum INTEGER NOT NULL,     -- sats
    tx_count INTEGER NOT NULL,
    seeded_height INTEGER NOT NULL, -- the block the node's totals were pinned to: tracking began
    seeded_time INTEGER NOT NULL,   -- that block's header time, Unix seconds
    viewed INTEGER NOT NULL         -- Unix seconds a reader last asked for it; the least recent
                                    -- is dropped when the ledger is full
) STRICT, WITHOUT ROWID;

-- The blocks applied, newest last: the last 100, for finding a reorg's fork and undoing it.
CREATE TABLE ledger_chain (
    height INTEGER PRIMARY KEY,
    hash BLOB NOT NULL,  -- 32 bytes, internal order
    time INTEGER NOT NULL -- header time, Unix seconds
) STRICT;

-- What each of the last 100 blocks added to each script it touched: subtracted on a disconnect.
CREATE TABLE ledger_undo (
    sh BLOB NOT NULL,
    height INTEGER NOT NULL,
    funded_count INTEGER NOT NULL,
    funded_sum INTEGER NOT NULL,
    spent_count INTEGER NOT NULL,
    spent_sum INTEGER NOT NULL,
    tx_count INTEGER NOT NULL,
    PRIMARY KEY (sh, height)
) STRICT, WITHOUT ROWID;

-- Each script's newest confirmed transactions (at most 200), as the node's /txs/summary rows.
CREATE TABLE ledger_txs (
    sh BLOB NOT NULL,
    height INTEGER NOT NULL,
    position INTEGER NOT NULL, -- in the block, coinbase 0
    txid BLOB NOT NULL,        -- 32 bytes, internal order
    value INTEGER NOT NULL,    -- net sats for the script: funded minus spent
    time INTEGER NOT NULL,     -- header time, Unix seconds
    PRIMARY KEY (sh, height, position)
) STRICT, WITHOUT ROWID;

-- The balance chart: each script's balance at the last block it changed in, one point per 144
-- heights, from the block tracking began.
CREATE TABLE ledger_points (
    sh BLOB NOT NULL,
    span INTEGER NOT NULL,     -- height / 144
    height INTEGER NOT NULL,
    time INTEGER NOT NULL,     -- header time, Unix seconds
    balance INTEGER NOT NULL,  -- sats, funded_sum minus spent_sum at that block
    PRIMARY KEY (sh, span)
) STRICT, WITHOUT ROWID;
