-- The whole confirmed history of each kept script (docs/STORE.md, "ledger"): the node's list of
-- its transactions, asked once and pinned to a block, then the rows of every block applied since.

-- 0: not taken yet; 1: being taken (rows not served, dropped on a restart); 2: whole; 3: the node
-- refused it for size; 4: dropped to keep the stored history within its bound, or too large for it.
ALTER TABLE ledger_scripts ADD COLUMN history INTEGER NOT NULL DEFAULT 0;
-- The block the node's list was pinned to: rows above it come from the blocks the ledger applied.
ALTER TABLE ledger_scripts ADD COLUMN history_height INTEGER;
-- The key of the script's rows in ledger_history, new for each list taken.
ALTER TABLE ledger_scripts ADD COLUMN hid INTEGER;

-- One row per confirmed transaction of a script, in the node's order: height, then txid bytes.
CREATE TABLE ledger_history (
    hid INTEGER NOT NULL,
    height INTEGER NOT NULL,
    txid BLOB NOT NULL, -- 32 bytes, internal order
    PRIMARY KEY (hid, height, txid)
) STRICT, WITHOUT ROWID;
