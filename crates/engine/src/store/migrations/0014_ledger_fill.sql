-- The node's newest history page before each kept script's seed block (docs/STORE.md, "ledger"):
-- asked once per script, so a script added up without it (from a totals answer, or whose page
-- failed) lists its past transactions too.

-- tx_count at the seed block, so tx_count minus this is the transactions kept since: when the
-- rows since the seed are that many, none is missing and the page joins them without a gap. NULL
-- when not known: a script seeded before this migration whose seed block the undo record no
-- longer covers.
ALTER TABLE ledger_scripts ADD COLUMN seeded_tx_count INTEGER;
-- 1 once the page was joined, or found not to be needed or not to join without a gap.
ALTER TABLE ledger_scripts ADD COLUMN filled INTEGER NOT NULL DEFAULT 0;

-- Scripts seeded before: the count at the seed from the undo record of the blocks since, when it
-- reaches back that far (it holds every block above the chain's lowest row).
UPDATE ledger_scripts SET seeded_tx_count = tx_count - (
    SELECT coalesce(sum(u.tx_count), 0) FROM ledger_undo u
    WHERE u.sh = ledger_scripts.sh AND u.height > ledger_scripts.seeded_height
) WHERE seeded_height >= (SELECT min(height) FROM ledger_chain);

-- A script holding rows at or below its seed block was seeded with its page.
UPDATE ledger_scripts SET filled = 1 WHERE EXISTS (
    SELECT 1 FROM ledger_txs t WHERE t.sh = ledger_scripts.sh
    AND t.height <= ledger_scripts.seeded_height
);
