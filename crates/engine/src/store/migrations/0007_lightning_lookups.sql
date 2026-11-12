-- Lightning lookups (docs/STORE.md, "ln_channels"): a node's channels, channels by the
-- transaction that funded or closed them, and the funding transaction itself, which the peer
-- never learns (docs/LIGHTNING.md, "Why the script and not the outpoint").

-- The funding transaction's id (internal byte order), asked of the node once per channel by a
-- background task. Null until then, and cleared whenever the funding block changes.
ALTER TABLE ln_channels ADD COLUMN open_txid BLOB;

-- A node's channels, open or closed, from either end.
CREATE INDEX ln_channels_node_one ON ln_channels (node_one);
CREATE INDEX ln_channels_node_two ON ln_channels (node_two);

-- Channels by transaction: what a transaction opened or closed.
CREATE INDEX ln_channels_open_txid ON ln_channels (open_txid) WHERE open_txid IS NOT NULL;
CREATE INDEX ln_channels_close_txid ON ln_channels (close_txid) WHERE close_txid IS NOT NULL;

-- The channels whose funding transaction is still to be asked for: empty once caught up.
CREATE INDEX ln_channels_unfunded ON ln_channels (scid) WHERE open_txid IS NULL;
