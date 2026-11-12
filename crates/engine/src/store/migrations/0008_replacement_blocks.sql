-- Where each replacement was confirmed (docs/STORE.md, "replacements"), so the record can follow
-- a reorg. `mined` becomes 0 (not mined), 1 (mined) or 2 (unknown: its block left the best chain
-- and where it is now is not known yet). Rows written before this migration keep mined = 1 with
-- no block: they are left as they were, and no reorg re-examines them.
ALTER TABLE replacements ADD COLUMN height INTEGER; -- the confirming block's height, when mined = 1
ALTER TABLE replacements ADD COLUMN block BLOB;     -- its hash, 32 bytes
