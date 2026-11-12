//! Read-only smoke test of the Esplora client against a live node: decodes every route group
//! and compares typed and raw latency on large responses. Never broadcasts.
//!
//! `ESPLORA_URL=… NETWORK=bitcoin|signet|testnet4 cargo run -p node --example esplora_probe`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
// A manual probe: printing is its output.
#![allow(clippy::print_stdout)]

use std::fmt::Debug;
use std::time::{Duration, Instant};

use bitcoin::{Address, Network};
use node::esplora::{
    BlockTxsStart, Error, Esplora, OutPosition, OutPositions, Route, ScriptRef, Stamped,
};

fn report<T: Debug>(name: &str, r: &Result<Stamped<T>, Error>) {
    match r {
        Ok(s) => {
            let tip = s
                .tip
                .map_or("unstamped".into(), |t| format!("tip {}", t.height));
            println!("ok    {name:<24} {tip}");
        }
        Err(e) => println!("ERROR {name:<24} {e}"),
    }
}

async fn median(mut f: impl AsyncFnMut() -> usize) -> (Duration, usize) {
    let mut times = Vec::new();
    let mut size = 0;
    for _ in 0..5 {
        let t = Instant::now();
        size = f().await;
        times.push(t.elapsed());
    }
    times.sort();
    (times[2], size)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = std::env::var("ESPLORA_URL").expect("ESPLORA_URL");
    let network: Network = std::env::var("NETWORK")
        .expect("NETWORK")
        .parse()
        .expect("network");
    let c = Esplora::new(&url).unwrap();

    let tip = c.tip_height().await;
    report("tip height", &tip);
    report("tip hash", &c.tip_hash().await);
    report("blocks", &c.blocks(None).await);
    let height = tip.unwrap().value - 6;
    let hash = c.block_hash(height).await.unwrap().value;
    report("block", &c.block(hash).await);
    report("block header", &c.block_header(hash).await);
    report("block status", &c.block_status(hash).await);
    report("block txids", &c.block_txids(hash).await);
    let txs = c.block_txs(hash, BlockTxsStart::FIRST).await;
    report("block txs", &txs);
    report("block raw", &c.block_raw(hash).await);

    let txs = txs.unwrap().value;
    let tx = txs
        .iter()
        .find(|t| !t.vin.iter().any(|i| i.is_coinbase))
        .unwrap_or(&txs[0]);
    report("tx", &c.tx(tx.txid).await);
    report("tx status", &c.tx_status(tx.txid, None).await);
    report("tx status asof", &c.tx_status(tx.txid, Some(hash)).await);
    report("tx hex", &c.tx_hex(tx.txid).await);
    report("tx raw", &c.tx_raw(tx.txid).await);
    report("tx merkle proof", &c.tx_merkle_proof(tx.txid).await);
    report("tx outspend", &c.tx_outspend(tx.txid, 0, None).await);
    report("tx outspends", &c.tx_outspends(tx.txid, None).await);

    let pos = OutPosition {
        height,
        index: 0,
        vout: 0,
    };
    report("output at", &c.output_at(pos, None).await);
    let batch = OutPositions::new(vec![pos, OutPosition { vout: 999, ..pos }]).unwrap();
    report("outputs at (batch)", &c.outputs_at(batch, None).await);

    let addr = tx
        .vout
        .iter()
        .find_map(|o| o.scriptpubkey_address.as_deref())
        .expect("an address output");
    let addr = ScriptRef::Address(
        addr.parse::<Address<_>>()
            .unwrap()
            .require_network(network)
            .unwrap(),
    );
    report("address stats", &c.script_stats(addr.clone(), None).await);
    report("address utxo", &c.script_utxo(addr.clone(), None).await);
    report("address txs", &c.script_txs(addr.clone(), None, None).await);
    report(
        "address txs chain",
        &c.script_txs_chain(addr.clone(), None, None).await,
    );
    report(
        "address txs mempool",
        &c.script_txs_mempool(addr.clone()).await,
    );
    report(
        "address txs summary",
        &c.script_txs_summary(addr, None, None).await,
    );

    report("mempool", &c.mempool().await);
    report("mempool txids", &c.mempool_txids().await);
    report("mempool recent", &c.mempool_recent().await);
    report("fee estimates", &c.fee_estimates().await);
    println!("internal routes available: {}", c.has_internal());

    println!("\nmedian of 5, typed decode vs raw bytes:");
    let routes = [
        ("block raw", Route::BlockRaw(hash)),
        ("block txids", Route::BlockTxids(hash)),
        ("mempool txids", Route::MempoolTxids),
    ];
    for (name, route) in routes {
        let (raw, size) = median(async || c.raw(&route).await.unwrap().body.len()).await;
        let (typed, _) = median(async || match &route {
            Route::BlockRaw(h) => c.block_raw(*h).await.unwrap().value.txdata.len(),
            Route::BlockTxids(h) => c.block_txids(*h).await.unwrap().value.len(),
            _ => c.mempool_txids().await.unwrap().value.len(),
        })
        .await;
        // Decode alone, on bytes already fetched: the CPU the raw path saves.
        let body = c.raw(&route).await.unwrap().body;
        let t = Instant::now();
        let n = match route {
            Route::BlockRaw(_) => bitcoin::consensus::deserialize::<bitcoin::Block>(&body)
                .unwrap()
                .txdata
                .len(),
            _ => serde_json::from_slice::<Vec<bitcoin::Txid>>(&body)
                .unwrap()
                .len(),
        };
        let decode = t.elapsed();
        println!(
            "{name:<14} {size:>9} bytes {n:>6} items  raw {raw:>8.1?}  typed {typed:>8.1?}  decode only {decode:>7.2?}"
        );
    }
}
