-- The score the server keeps of its own fee recommendations (docs/STORE.md, "fee_samples and
-- fee_blocks"; docs/MEMPOOL.md, "The score the server keeps"): what was served every 30 s and,
-- for every block, the rate that sufficed to get in. Observed live, never rebuildable. Rows
-- older than 30 days (samples) and 31 days (blocks) are deleted as new ones are written.
CREATE TABLE fee_samples (
    time INTEGER PRIMARY KEY, -- Unix seconds the answer was read
    served INTEGER NOT NULL,  -- 0 ours, 1 the node's estimates, 2 nothing
    high INTEGER,             -- ours, sat per 1,000 vB, for 1, 3, 6 and 144 blocks;
    medium INTEGER,           -- NULL unless served is 0
    low INTEGER,
    no_rush INTEGER
) STRICT;

CREATE TABLE fee_blocks (
    height INTEGER PRIMARY KEY, -- a block that replaced another at its height replaces its row
    time INTEGER NOT NULL,      -- Unix seconds the server saw it
    clearing INTEGER            -- sat per 1,000 vB; NULL when no projection before it held any
                                -- of its transactions, or it came while the server was down
) STRICT;
