-- The lowest fee band split at 0.12 and 0.15 sat/vB (docs/STORE.md, "Fee bands"). `band0` keeps
-- meaning all virtual size under 0.2 sat/vB; these hold the parts of it in [0.12, 0.15) and
-- [0.15, 0.2), so under 0.12 is `band0` minus both. NULL where the split was not recorded: a
-- row stored before this migration, or a roll-up of rows not all of which knew it.
ALTER TABLE blocks ADD COLUMN band0_12_15 INTEGER;
ALTER TABLE blocks ADD COLUMN band0_15_20 INTEGER;
ALTER TABLE mempool_history ADD COLUMN band0_12_15 INTEGER;
ALTER TABLE mempool_history ADD COLUMN band0_15_20 INTEGER;
