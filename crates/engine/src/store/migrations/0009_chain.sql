-- The chain this file belongs to (docs/STORE.md, "chain"): its genesis block's hash, written
-- once. A file written for another chain is refused when it is opened.
CREATE TABLE chain (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    -- 32 bytes in the bitcoin crate's internal order, as `blocks.hash`.
    genesis BLOB NOT NULL
) STRICT;
