//! Block audits with the real tracker and projected blocks over the track tests' mock node, and
//! the stored row against the routes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use bitcoin::Amount;
use bitcoin::hashes::Hash;
use engine::mempool::{self, Source};
use engine::projected;
use engine::store::params;
use node::rpc::model::{BlockTx, Coinbase};
use serde_json::Value;
use tokio::task::JoinHandle;

use super::*;
use crate::config::Config;
use crate::ws::Sockets;
use crate::ws::track::tests::{Mock, coin};

struct Setup {
    ctx: Arc<NetworkContext>,
    dir: PathBuf,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Setup {
    /// A network with a store in a fresh directory and its engines on `mock`.
    fn start(mock: &Mock) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rbitcoin-space-audits-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let data_dir = dir.to_string_lossy().into_owned();
        let vars: HashMap<&str, &str> = [
            ("REGTEST_RPC_HOST", "127.0.0.1"),
            ("REGTEST_RPC_PORT", "1"),
            ("REGTEST_RPC_USER", "u"),
            ("REGTEST_RPC_PASSWORD", "p"),
            ("REGTEST_ESPLORA_URL", "http://127.0.0.1:1"),
            ("REGTEST_ELECTRUM_ADDR", "127.0.0.1:1"),
            ("DATA_DIR", data_dir.as_str()),
        ]
        .into_iter()
        .collect();
        let config = Config::from_vars(|k| vars.get(k).map(|v| (*v).to_owned())).unwrap();
        let sockets = Sockets::new(&config.ws, Duration::from_secs(1));
        let ctx = Arc::new(NetworkContext::new(&config.networks[0], &config, &sockets).unwrap());
        let (tracker, t1) = Tracker::start(
            mock.clone(),
            mempool::Config {
                gap: Duration::from_millis(10),
                heartbeat: Duration::from_millis(100),
                max_backoff: Duration::from_millis(50),
                ..mempool::Config::default()
            },
        );
        let (projector, t2) = Projector::start(
            tracker.clone(),
            mock.clone(),
            projected::Config {
                max_backoff: Duration::from_millis(50),
                ..projected::Config::default()
            },
        );
        ctx.track.attach(&tracker, &projector);
        Self {
            ctx,
            dir,
            tasks: vec![t1, t2],
        }
    }

    fn engines(&self) -> (&Tracker, &Projector) {
        let (t, p) = self.ctx.track.engines().unwrap();
        (t, p)
    }

    fn store(&self) -> &Store {
        &self.ctx.blocks.as_ref().unwrap().store
    }

    /// Waits until the projection is on the mock's tip, current, and holds `txs` transactions.
    async fn settle(&self, mock: &Mock, txs: usize) {
        let (_, projector) = self.engines();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let p = projector.current();
            let held: usize = p.blocks.iter().map(|b| b.txs.len()).sum();
            if p.tip == Some(mock.tip()) && !p.stale && held == txs {
                return;
            }
            assert!(Instant::now() < deadline, "the engines never settled");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn audit(&self, mock: &Mock, hash: BlockHash) -> Result<Totals, &'static str> {
        let (tracker, projector) = self.engines();
        audit_new(
            self.store(),
            tracker,
            projector,
            &rpc_block(mock, hash).await,
        )
        .await
    }

    /// The block's row in the block table, as `follow` would write it.
    async fn row(&self, height: u32, hash: BlockHash) {
        self.store()
            .write(move |sql| {
                sql.prepare(
                    "INSERT OR REPLACE INTO blocks (height, hash, time, tx_count, size, weight, \
                     paying_weight, fees, subsidy, reward, difficulty, coinbase_tag, payouts) \
                     VALUES (?1, ?2, ?1, 1, 1, 1, 1, 0, 0, 0, 1.0, x'', x'')",
                )?
                .execute(params![height, &hash.as_byte_array()[..]])?;
                Ok(())
            })
            .await
            .unwrap();
    }
}

/// The block as `getblock` at verbosity 2 gives it to the summary.
async fn rpc_block(mock: &Mock, hash: BlockHash) -> Block {
    let b = Source::block(mock, hash).await.unwrap();
    let tx = b
        .txdata
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (fee, vsize) = if i == 0 {
                (None, 100)
            } else {
                let m = mock.get(t.compute_txid());
                (Some(Amount::from_sat(m.fee)), m.vsize)
            };
            let vsize = u32::try_from(vsize).unwrap();
            BlockTx {
                txid: t.compute_txid(),
                fee,
                vsize,
                weight: vsize * 4,
                value: Amount::ZERO,
            }
        })
        .collect();
    Block {
        hash,
        height: b.header.time,
        time: b.header.time,
        mediantime: 0,
        size: 0,
        weight: 0,
        previousblockhash: Some(b.header.prev_blockhash),
        tx,
        coinbase: Coinbase::default(),
    }
}

async fn json(res: Response) -> (StatusCode, Value) {
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_block_is_audited_and_the_routes_serve_what_was_stored() {
    let mock = Mock::new();
    let s = Setup::start(&mock);
    let ids: Vec<Txid> = [5000, 4000, 3000, 100]
        .iter()
        .enumerate()
        .map(|(i, fee)| mock.send(&[coin(u32::try_from(i).unwrap())], 1, *fee, 200))
        .collect();
    s.settle(&mock, 4).await;

    // The miner leaves out the cheapest, which has waited far longer than a minute.
    let h1 = mock.mine(&ids[..3]);
    s.engines().0.block_found(mock.tip());
    let t = s.audit(&mock, h1).await.unwrap();
    assert_eq!((t.matched.txs, t.missing.txs), (3, 1));
    assert_eq!(t.match_rate, 75.0);
    assert_eq!((t.expected.fees, t.actual.fees), (12_100, 12_000));
    let stored = audit::by_hash(s.store(), h1).await.unwrap().unwrap();
    assert_eq!((stored.totals, stored.height), (t, 1));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        u64::from(stored.seen_at).abs_diff(now) < 60,
        "{}",
        stored.seen_at
    );

    let ctx = || State(Arc::clone(&s.ctx));
    let hash = h1.to_string();
    let (status, one) = json(score(ctx(), Path(hash.clone())).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        one,
        json!({"hash": hash, "matchRate": 75.0, "expectedFees": 12_100, "expectedWeight": 3200})
    );
    let (_, d) = json(detail(ctx(), Path(hash.clone())).await).await;
    assert_eq!(d["missingTxs"], json!([ids[3].to_string()]));
    assert_eq!(
        (d["height"].clone(), d["listsComplete"].clone()),
        (json!(1), json!(true))
    );
    assert_eq!(d["matchRate"], json!(75.0));
    let (_, app) = json(app(ctx(), Path(hash.clone())).await).await;
    assert_eq!(app["match_rate"], json!(75.0));
    assert_eq!(app["missing"], json!({"txs": 1, "vsize": 200}));
    assert_eq!(app["txs"][0]["txid"], json!(ids[3].to_string()));
    assert_eq!(app["txs"][0]["kind"], json!("missing"));
    let (_, left_out) = json(tx_audit(ctx(), Path((hash.clone(), ids[3].to_string()))).await).await;
    assert_eq!(
        left_out,
        json!({"seen": true, "expected": true, "added": false, "prioritized": false,
               "conflict": false, "accelerated": false, "firstSeen": mock.get(ids[3]).time})
    );
    let (_, mined) = json(tx_audit(ctx(), Path((hash.clone(), ids[0].to_string()))).await).await;
    assert_eq!(
        (mined["expected"].clone(), mined["seen"].clone()),
        (json!(true), json!(true))
    );

    // Pages and the series list it once its row is the table's block at its height.
    let (_, page) = json(scores_top(ctx()).await).await;
    assert_eq!(page, json!([]));
    s.row(1, h1).await;
    let (_, page) = json(scores_top(ctx()).await).await;
    assert_eq!(page, json!([one]));
    let (_, page) = json(scores(ctx(), Path("16".into())).await).await;
    assert_eq!(page, json!([one]));
    let (_, series) = json(predictions(ctx(), Path("all".into())).await).await;
    assert_eq!(series, json!([[1, 1, 75.0]]));
    let (status, _) = json(predictions(ctx(), Path("5y".into())).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // No audit is its own answer.
    let other = BlockHash::all_zeros().to_string();
    let (status, body) = json(score(ctx(), Path(other.clone())).await).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::NOT_FOUND,
            json!({"error": "audit not available"})
        )
    );
    let (status, _) = json(tx_audit(ctx(), Path((other, ids[0].to_string()))).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json(detail(ctx(), Path("zz".into())).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_audit_after_a_start_a_stale_projection_or_a_reorg() {
    let mock = Mock::new();
    let a = mock.send(&[coin(0)], 1, 1000, 200);
    // The block came before the server had any projection: nothing to compare it with.
    let h1 = mock.mine(&[a]);
    let s = Setup::start(&mock);
    s.settle(&mock, 0).await;
    assert_eq!(s.audit(&mock, h1).await, Err("no_projection"));

    // The node stops answering, so the projection on h1 goes stale; the block found meanwhile
    // is not audited from it.
    let b = mock.send(&[coin(1)], 1, 1000, 200);
    s.settle(&mock, 1).await;
    mock.set_down(true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !s.engines().1.current().stale {
        assert!(Instant::now() < deadline, "never stale");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let h2 = mock.mine(&[b]);
    mock.set_down(false);
    s.engines().0.tip_changed();
    s.settle(&mock, 0).await;
    assert_eq!(s.audit(&mock, h2).await, Err("stale"));

    // A block that is audited, then replaced by a sibling: the sibling was not what the
    // projection on their parent gave way to.
    let c = mock.send(&[coin(2)], 1, 1000, 200);
    let d = mock.send(&[coin(3)], 1, 900, 200);
    s.settle(&mock, 2).await;
    let h3 = mock.mine(&[c]);
    s.engines().0.block_found(mock.tip());
    s.settle(&mock, 1).await;
    assert_eq!(s.audit(&mock, h3).await.unwrap().match_rate, 50.0);
    mock.disconnect();
    let sibling = mock.mine(&[d]);
    s.engines().0.tip_changed();
    s.settle(&mock, 1).await;
    assert_eq!(s.audit(&mock, sibling).await, Err("superseded"));
    assert!(audit::by_hash(s.store(), sibling).await.unwrap().is_none());
    assert!(audit::by_hash(s.store(), h3).await.unwrap().is_some());
}
