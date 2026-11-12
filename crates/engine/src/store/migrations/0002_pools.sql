-- Who mined each block (docs/STORE.md, "blocks"; the rules are in docs/API.md, "Mining").
-- Set from the coinbase when a row is written; 0 is "Unknown". Rows written before this
-- migration read 0 until `engine::blocks::reattribute` redoes them, which it does whenever
-- `pool_list.version` differs from the build's list.
ALTER TABLE blocks ADD COLUMN pool INTEGER NOT NULL DEFAULT 0;

-- A pool's blocks, newest first: the index carries the height, the row's key.
CREATE INDEX blocks_pool ON blocks (pool);

-- The pool list and rules the `pool` column was last fully computed with. One row at most.
CREATE TABLE pool_list (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    version TEXT NOT NULL
) STRICT;
