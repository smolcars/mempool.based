-- Lightning (docs/STORE.md, "ln_channels" and "ln_statistics"): what the gossip peer observed
-- live and cannot be rebuilt later, since gossip forgets a channel once it closes.

-- One row per public channel the peer verified: its open, and its close once the funding
-- output is spent. Kept as the peer's history says, undone records included: a reorg that
-- undoes an open deletes the row, one that undoes a close clears its close columns.
CREATE TABLE ln_channels (
    -- Short channel id: funding height << 40 | transaction index << 16 | output index. Ordering
    -- by it orders channels by where they were funded.
    scid INTEGER PRIMARY KEY,
    -- The two nodes, as 33-byte compressed public keys, in the announcement's order.
    node_one BLOB NOT NULL,
    node_two BLOB NOT NULL,
    -- The funding output's value, sats.
    capacity INTEGER NOT NULL,
    -- The funding block's hash (internal byte order) and header time, Unix seconds.
    open_hash BLOB NOT NULL,
    open_time INTEGER NOT NULL,
    -- The closing block and transaction. Null while the channel is open.
    close_height INTEGER,
    close_hash BLOB,
    close_time INTEGER,
    close_txid BLOB,
    -- 0 cooperative, 1 force, 2 penalty, 3 other (docs/LIGHTNING.md, "How a close is
    -- classified"). A breach found later keeps its kind and fills the penalty columns.
    close_kind INTEGER,
    -- The closing transaction's fee, sats; null when the node could not say.
    close_fee INTEGER,
    -- A force close found to be a breach: the revocation sweep and its block.
    penalty_txid BLOB,
    penalty_height INTEGER,
    penalty_hash BLOB,
    penalty_time INTEGER
) STRICT;

-- The newest closes first.
CREATE INDEX ln_channels_closed ON ln_channels (close_height, scid) WHERE close_height IS NOT NULL;

-- The network as the verified graph showed it every ten minutes, rolled into hours after 8 days
-- and days after 400, as mempool_history is. Every value column is a sum over the row's
-- `samples` snapshots, so a roll-up adds rows together exactly: a mean is the sum over
-- `samples`, and a median column holds the snapshots' medians summed (their mean, not the
-- median over the period).
CREATE TABLE ln_statistics (
    -- 0 a ten-minute snapshot, 1 an hour, 2 a UTC day.
    level INTEGER NOT NULL,
    -- Unix seconds: the snapshot's ten minutes, or the start of the hour or day.
    time INTEGER NOT NULL,
    samples INTEGER NOT NULL,
    -- Verified channels, nodes with a channel, and the channels' capacity in sats.
    channels INTEGER NOT NULL,
    nodes INTEGER NOT NULL,
    capacity INTEGER NOT NULL,
    -- Nodes by the addresses they announce: Tor only, clearnet only, both, none.
    tor_nodes INTEGER NOT NULL,
    clearnet_nodes INTEGER NOT NULL,
    clearnet_tor_nodes INTEGER NOT NULL,
    unannounced_nodes INTEGER NOT NULL,
    -- The median channel capacity, sats.
    med_capacity INTEGER NOT NULL,
    -- Channel policies (one per direction announced) counted in the averages, and their fee
    -- rates (ppm) and base fees (msat) summed: the mean is the sum over `policies`. A policy
    -- over 1,000,000 ppm or 1,000,000 msat refuses to route and is left out.
    policies INTEGER NOT NULL,
    fee_rate INTEGER NOT NULL,
    base_fee INTEGER NOT NULL,
    -- The median fee rate (ppm) and base fee (msat) over every policy.
    med_fee_rate INTEGER NOT NULL,
    med_base_fee INTEGER NOT NULL,
    PRIMARY KEY (level, time)
) STRICT, WITHOUT ROWID;
