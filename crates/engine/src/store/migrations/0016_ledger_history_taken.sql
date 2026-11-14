-- When each kept script's whole history was last taken from the node (docs/STORE.md, "ledger"), so
-- a history dropped for the bound is taken again at most once a day: the lists are heavy for the
-- node. Unix seconds; NULL when never taken whole.
ALTER TABLE ledger_scripts ADD COLUMN history_taken INTEGER;
