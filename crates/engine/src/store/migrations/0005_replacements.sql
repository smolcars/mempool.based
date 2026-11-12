-- The replacement record (docs/STORE.md, "replacements"; docs/MEMPOOL.md, "The replacement
-- record"): what replaced what, as the mempool tracker saw it, so the record survives a restart.
-- Observed live, never rebuildable. Bounded by REPLACEMENTS_KEPT: each write deletes the rows
-- older than the newest that many.
CREATE TABLE replacements (
    n INTEGER PRIMARY KEY,           -- the order recorded, newest highest
    txid BLOB NOT NULL UNIQUE,       -- the replaced transaction, 32 bytes
    by BLOB NOT NULL,                -- its replacement
    seen INTEGER NOT NULL,           -- Unix seconds we saw it replaced
    fee INTEGER NOT NULL,            -- the replaced transaction's base fee (sats), sigop-adjusted
    vsize INTEGER NOT NULL,          -- vsize, weight, total output value (sats), the node's accept
    weight INTEGER NOT NULL,         -- time (Unix seconds) and whether it signalled BIP 125 (0, 1)
    value INTEGER NOT NULL,
    time INTEGER NOT NULL,
    rbf INTEGER NOT NULL,
    by_fee INTEGER,                  -- the same for the replacement; all NULL when the tracker
    by_vsize INTEGER,                -- never held it (confirmed in the block that removed the
    by_weight INTEGER,               -- original)
    by_value INTEGER,
    by_time INTEGER,
    by_rbf INTEGER,
    mined INTEGER NOT NULL DEFAULT 0 -- 1 once the replacement was seen confirmed
) STRICT;

-- Marking a replacement mined, and walking a history backwards.
CREATE INDEX replacements_by ON replacements (by);
