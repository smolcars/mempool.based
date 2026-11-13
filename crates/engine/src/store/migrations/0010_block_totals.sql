-- Running totals of the block table (docs/STORE.md, "Totals"), so that a read over all of history
-- sums a few thousand buckets instead of scanning every row. Each bucket is what the scan it
-- replaces computes for that bucket's rows, by the same statement, and is recomputed from its rows
-- in the same write as any change to them: the triggers below list the buckets a row change
-- touches, and the store recomputes them before it commits (`engine::blocks::totals::settle`).

-- Blocks per bucket. `kind` 0 is a UTC day, keyed by `time / 86400`; any other kind `n` is `n`
-- heights, keyed by `height / n` (the block series' points: 18, 36, 72, 144, 216 and 1008).
CREATE TABLE block_totals (
    kind INTEGER NOT NULL,
    key INTEGER NOT NULL,
    blocks INTEGER NOT NULL,
    first_height INTEGER NOT NULL,
    last_height INTEGER NOT NULL,
    first_time INTEGER NOT NULL,
    last_time INTEGER NOT NULL,
    tx_count INTEGER NOT NULL,
    size INTEGER NOT NULL,
    weight INTEGER NOT NULL,
    fees INTEGER NOT NULL,
    subsidy INTEGER NOT NULL,
    reward INTEGER NOT NULL,
    -- Means over the bucket's rows, as `avg()` gives them; the fee rates over its rated rows.
    difficulty REAL NOT NULL,
    rated INTEGER NOT NULL,
    fee_min REAL,
    fee_p10 REAL,
    fee_p25 REAL,
    fee_median REAL,
    fee_p75 REAL,
    fee_p90 REAL,
    fee_max REAL,
    PRIMARY KEY (kind, key)
) STRICT, WITHOUT ROWID;

-- The buckets that reach a height range: those whose last height is at or above its start.
CREATE INDEX block_totals_reach ON block_totals (kind, last_height);

-- Blocks per pool per bucket. `kind` 1008 is keyed by `height / 1008`; `kind` 7 is a week from
-- Monday 00:00 UTC, keyed by `(time - 345600) / 604800`. A pool with no block in a bucket has no
-- row.
CREATE TABLE pool_totals (
    kind INTEGER NOT NULL,
    key INTEGER NOT NULL,
    pool INTEGER NOT NULL,
    blocks INTEGER NOT NULL,
    -- Blocks holding only their coinbase.
    empty INTEGER NOT NULL,
    reward INTEGER NOT NULL,
    PRIMARY KEY (kind, key, pool)
) STRICT, WITHOUT ROWID;

-- Buckets a write has touched, as (kind, key) with the kinds above; emptied before it commits.
CREATE TABLE totals_dirty (
    kind INTEGER NOT NULL,
    key INTEGER NOT NULL
) STRICT;

CREATE TRIGGER blocks_totals_insert AFTER INSERT ON blocks BEGIN
    INSERT INTO totals_dirty (kind, key) VALUES
        (0, NEW.time / 86400), (7, (NEW.time - 345600) / 604800), (18, NEW.height / 18),
        (36, NEW.height / 36), (72, NEW.height / 72), (144, NEW.height / 144),
        (216, NEW.height / 216), (1008, NEW.height / 1008);
END;

-- Fires for the rows `INSERT OR REPLACE` removes too: the writer sets `recursive_triggers`.
CREATE TRIGGER blocks_totals_delete AFTER DELETE ON blocks BEGIN
    INSERT INTO totals_dirty (kind, key) VALUES
        (0, OLD.time / 86400), (7, (OLD.time - 345600) / 604800), (18, OLD.height / 18),
        (36, OLD.height / 36), (72, OLD.height / 72), (144, OLD.height / 144),
        (216, OLD.height / 216), (1008, OLD.height / 1008);
END;

CREATE TRIGGER blocks_totals_update AFTER UPDATE ON blocks BEGIN
    INSERT INTO totals_dirty (kind, key) VALUES
        (0, OLD.time / 86400), (7, (OLD.time - 345600) / 604800), (18, OLD.height / 18),
        (36, OLD.height / 36), (72, OLD.height / 72), (144, OLD.height / 144),
        (216, OLD.height / 216), (1008, OLD.height / 1008),
        (0, NEW.time / 86400), (7, (NEW.time - 345600) / 604800), (18, NEW.height / 18),
        (36, NEW.height / 36), (72, NEW.height / 72), (144, NEW.height / 144),
        (216, NEW.height / 216), (1008, NEW.height / 1008);
END;

-- Rows written before this migration are totalled afterwards, newest first, in writes of a few
-- thousand heights (`engine::blocks::fill_totals`): filling a full mainnet table in one
-- transaction would hold the writer for seconds. While this row exists the reads scan the block
-- table as before; heights at and above `below` are totalled, those under it not yet.
CREATE TABLE totals_pending (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    below INTEGER NOT NULL
) STRICT;

INSERT INTO totals_pending (id, below) SELECT 0, max(height) + 1 FROM blocks HAVING count(*) > 0;
