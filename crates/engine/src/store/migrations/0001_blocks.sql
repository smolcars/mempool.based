-- One row per block on the best chain, derived from the node (docs/STORE.md, "blocks").
-- Totals and the coinbase are always set; the fee-rate columns are NULL until `rated` is 1.
CREATE TABLE blocks (
    height INTEGER PRIMARY KEY,
    -- 32 bytes in the bitcoin crate's internal order (the reverse of how it is printed).
    hash BLOB NOT NULL UNIQUE,
    -- Header timestamp, Unix seconds.
    time INTEGER NOT NULL,
    -- Every transaction, the coinbase included.
    tx_count INTEGER NOT NULL,
    size INTEGER NOT NULL,
    weight INTEGER NOT NULL,
    -- Weight of every transaction but the coinbase.
    paying_weight INTEGER NOT NULL,
    -- Sats.
    fees INTEGER NOT NULL,
    subsidy INTEGER NOT NULL,
    -- Sum of the coinbase's outputs: what the miner took.
    reward INTEGER NOT NULL,
    difficulty REAL NOT NULL,
    -- The coinbase input's script, at most 100 bytes. Chain data: never interpreted here.
    coinbase_tag BLOB NOT NULL,
    -- The coinbase's paying output scripts, each as one length byte and the script.
    payouts BLOB NOT NULL,
    -- 1 once the fee rates and bands below are known.
    rated INTEGER NOT NULL DEFAULT 0,
    -- Size-weighted fee rates in sat/vB; NULL when unknown or the block pays no fees.
    fee_min REAL,
    fee_p10 REAL,
    fee_p25 REAL,
    fee_median REAL,
    fee_p75 REAL,
    fee_p90 REAL,
    fee_max REAL,
    -- Virtual size paying in each of the design's ten fee bands, cheapest first.
    band0 INTEGER,
    band1 INTEGER,
    band2 INTEGER,
    band3 INTEGER,
    band4 INTEGER,
    band5 INTEGER,
    band6 INTEGER,
    band7 INTEGER,
    band8 INTEGER,
    band9 INTEGER
) STRICT;

-- Periods asked by time ("the last 30 days") resolve to heights through this.
CREATE INDEX blocks_time ON blocks (time);
-- The fee-rate backfill finds its next block here without scanning rated rows.
CREATE INDEX blocks_unrated ON blocks (height) WHERE rated = 0;
