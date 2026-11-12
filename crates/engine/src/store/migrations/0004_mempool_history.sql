-- Mempool history (docs/STORE.md, "mempool_history"): what the mempool looked like, sampled
-- once a minute while the mempool engine runs, and rolled into coarser rows as it ages: minutes
-- into hours after 8 days, hours into days after 400. Every value column is a sum over the
-- row's `samples` minutes, so a roll-up adds rows together exactly and a mean is the sum over
-- `samples`. A minute with no row was not sampled: the engine was off, stale or syncing.
CREATE TABLE mempool_history (
    -- 0 a minute, 1 an hour, 2 a UTC day.
    level INTEGER NOT NULL,
    -- Unix seconds: the minute the state was read at, or the start of the hour or day.
    time INTEGER NOT NULL,
    -- Minutes sampled in the row: 1 for a minute, up to 60 for an hour, 1,440 for a day.
    samples INTEGER NOT NULL,
    -- The mempool as the projected blocks hold it: transactions, virtual size, fees in sats.
    count INTEGER NOT NULL,
    vsize INTEGER NOT NULL,
    fees INTEGER NOT NULL,
    -- The node's mempoolminfee in sat per 1,000 vB.
    min_fee INTEGER NOT NULL,
    -- Virtual size in each of the design's ten fee bands (docs/DESIGN.md), by effective rate.
    band0 INTEGER NOT NULL,
    band1 INTEGER NOT NULL,
    band2 INTEGER NOT NULL,
    band3 INTEGER NOT NULL,
    band4 INTEGER NOT NULL,
    band5 INTEGER NOT NULL,
    band6 INTEGER NOT NULL,
    band7 INTEGER NOT NULL,
    band8 INTEGER NOT NULL,
    band9 INTEGER NOT NULL,
    -- Flow over the minute before each sample: transactions and virtual size that entered,
    -- that left for any reason, and of those that left, the ones confirmed in a block.
    in_count INTEGER NOT NULL,
    in_vsize INTEGER NOT NULL,
    out_count INTEGER NOT NULL,
    out_vsize INTEGER NOT NULL,
    mined_count INTEGER NOT NULL,
    mined_vsize INTEGER NOT NULL,
    PRIMARY KEY (level, time)
) STRICT, WITHOUT ROWID;
