//! The transaction routes against the real tracker and projected blocks on the mock node of
//! `ws/track/tests.rs`: exact answers, the reference's shapes (`fixtures/api-v1`), and inputs a
//! client should not be able to get past the parsers. The HTTP-level and prevouts tests are in
//! `tests/transactions.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use axum::extract::{Path, RawQuery, State};
use bitcoin::OutPoint;
use bitcoin::hashes::Hash;
use serde_json::{Value, json};

use super::*;
use crate::config::Network;
use crate::ws::track::tests::{Mock, Server, coin, context, context_in};

const FIXTURES: &str = include_str!("../../../../fixtures/api-v1/transactions.json");

fn fixture(name: &str) -> Value {
    let all: Value = serde_json::from_str(FIXTURES).unwrap();
    all[name].clone()
}

/// Same keys at every level and the same JSON types, except that a number may be whole or not
/// and `null` stands for anything. Arrays compare their first items.
fn same_shape(ours: &Value, theirs: &Value, at: &str) {
    match (ours, theirs) {
        (Value::Object(a), Value::Object(b)) => {
            let (ka, kb): (Vec<_>, Vec<_>) = (a.keys().collect(), b.keys().collect());
            assert_eq!(ka, kb, "keys at {at}");
            for (k, v) in a {
                same_shape(v, &b[k], &format!("{at}.{k}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if let (Some(x), Some(y)) = (a.first(), b.first()) {
                same_shape(x, y, &format!("{at}[0]"));
            }
        }
        (Value::Null, _) | (_, Value::Null) => {}
        (a, b) => assert_eq!(
            std::mem::discriminant(a),
            std::mem::discriminant(b),
            "type at {at}: {a} against {b}"
        ),
    }
}

async fn text(r: Response) -> (u16, String) {
    let status = r.status().as_u16();
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn answer(r: Result<Response, ApiError>) -> (u16, String) {
    text(r.unwrap_or_else(IntoResponse::into_response)).await
}

async fn rbf_of(s: &Server, t: Txid) -> (u16, String) {
    answer(rbf(State(Arc::clone(&s.ctx)), Path(t.to_string())).await).await
}

async fn cpfp_json(s: &Server, t: Txid) -> Value {
    let (status, body) = answer(cpfp(State(Arc::clone(&s.ctx)), Path(t.to_string())).await).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

async fn times(s: &Server, q: &str) -> (u16, String) {
    answer(transaction_times(State(Arc::clone(&s.ctx)), RawQuery(Some(q.to_owned()))).await).await
}

/// A tree node exactly as the route writes it, from the mock's own figures. Weight is four
/// times the mock's vsize, so `vsize` is whole.
fn node(mock: &Mock, t: Txid, parent: Option<Txid>, tx_full: Option<bool>, full: bool) -> String {
    let m = mock.get(t);
    let value: u64 = m.tx.output.iter().map(|o| o.value.to_sat()).sum();
    let rbf = m.tx.is_explicitly_rbf();
    let rate = num(m.fee as f64 / m.vsize as f64);
    let tx_full = tx_full.map_or(String::new(), |f| format!(",\"fullRbf\":{f}"));
    let interval = parent.map_or(String::new(), |p| {
        format!(",\"interval\":{}", mock.get(p).time as i64 - m.time as i64)
    });
    format!(
        "{{\"tx\":{{\"txid\":\"{t}\",\"fee\":{},\"vsize\":{},\"value\":{value},\"rate\":{rate},\"time\":{},\"rbf\":{rbf}{tx_full}}},\"time\":{}{interval},\"fullRbf\":{full},\"replaces\":[",
        m.fee, m.vsize, m.time, m.time
    )
}

#[tokio::test]
async fn replacement_trees_follow_a_chain_to_its_mined_end() {
    let mock = Mock::new();
    // A does not signal; B and C do. Each replaces the one before on the same input.
    let a = mock.send(&[coin(1)], 1, 500, 200);
    let s = Server::start(&mock, |_| {});
    s.settle().await;
    let b = mock.send_with(&[coin(1)], 1, 1000, 200, true);
    s.settle().await;
    let c = mock.send_with(&[coin(1)], 2, 2000, 250, true);
    let lone = mock.send(&[coin(2)], 1, 300, 150);
    s.settle().await;

    let tree = format!(
        "{}{}{}]}}]}}]}}",
        node(&mock, c, None, Some(false), true),
        node(&mock, b, Some(c), Some(true), true),
        node(&mock, a, Some(b), None, true),
    );
    // The same tree from any member; `replaces` is what that one replaced itself.
    for (t, direct) in [
        (a, "null".to_owned()),
        (b, format!("[\"{a}\"]")),
        (c, format!("[\"{b}\"]")),
    ] {
        let (status, body) = rbf_of(&s, t).await;
        assert_eq!(status, 200);
        assert_eq!(
            body,
            format!("{{\"replacements\":{tree},\"replaces\":{direct}}}")
        );
    }
    assert_eq!(
        rbf_of(&s, lone).await.1,
        r#"{"replacements":null,"replaces":null}"#
    );
    let ctx = || State(Arc::clone(&s.ctx));
    assert_eq!(
        answer(replacements(ctx()).await).await.1,
        format!("[{tree}]")
    );
    assert_eq!(
        answer(full_rbf_replacements(ctx()).await).await.1,
        format!("[{tree}]")
    );

    // First seen: from the tracker while in the mempool, from the record after; 0 otherwise.
    let unknown = Txid::from_byte_array([7; 32]);
    let q = format!("txId[]={a}&txId[]={c}&txId%5B%5D={lone}&txId[]={unknown}&txId[]={a}&x=1");
    let t = |id| mock.get(id).time;
    assert_eq!(
        times(&s, &q).await,
        (200, format!("[{},{},{},0,{}]", t(a), t(c), t(lone), t(a)))
    );

    // Mined: the newest replacement and its node say so, at the end of each.
    mock.mine(&[c]);
    s.settle().await;
    let mined = tree
        .replacen(
            ",\"fullRbf\":false}",
            ",\"fullRbf\":false,\"mined\":true}",
            1,
        )
        .strip_suffix('}')
        .map(|t| format!("{t},\"mined\":true}}"))
        .unwrap();
    assert_eq!(
        answer(replacements(ctx()).await).await.1,
        format!("[{mined}]")
    );
    assert_eq!(
        times(&s, &format!("txId[]={c}")).await.1,
        format!("[{}]", t(c))
    );

    // The reference's shapes, mined and not.
    let ours: Value = serde_json::from_str(&rbf_of(&s, b).await.1).unwrap();
    same_shape(&ours, &fixture("rbf"), "rbf");

    // A reorg undoes the block and C is back in the mempool: the tree is no longer mined.
    mock.disconnect();
    s.settle().await;
    assert_eq!(
        answer(replacements(ctx()).await).await.1,
        format!("[{tree}]")
    );
    let none: Value = serde_json::from_str(&rbf_of(&s, lone).await.1).unwrap();
    same_shape(&none, &fixture("rbf_none"), "rbf_none");
    let times: Value = serde_json::from_str(&times(&s, &q).await.1).unwrap();
    same_shape(&times, &fixture("transaction_times"), "transaction_times");
}

#[tokio::test]
async fn a_replacement_that_went_straight_into_a_block_is_mined() {
    let mock = Mock::new();
    let a = mock.send(&[coin(1)], 1, 500, 200);
    let s = Server::start(&mock, |_| {});
    s.settle().await;
    // B spends A's input and is mined before the tracker ever lists it.
    let b = mock.send(&[coin(1)], 1, 1000, 200);
    mock.mine(&[b]);
    s.settle().await;
    let body: Value = serde_json::from_str(&rbf_of(&s, a).await.1).unwrap();
    let root = &body["replacements"];
    assert_eq!(root["tx"]["txid"], json!(b.to_string()));
    assert_eq!(
        (&root["mined"], &root["tx"]["fee"]),
        (&json!(true), &Value::Null)
    );
}

#[tokio::test]
async fn a_prevouts_batch_starts_no_read_past_its_deadline() {
    // Nothing listens there: a read that started would fail as unavailable, not as late.
    let esplora = Esplora::new("http://127.0.0.1:1").unwrap();
    let wanted = [(Txid::from_byte_array([1; 32]), 0)];
    let late = std::time::Instant::now();
    let r = lookup(&esplora, &wanted, &HashSet::new(), late).await;
    assert!(matches!(r, Err(Error::Timeout)), "{r:?}");
}

#[tokio::test]
async fn a_replacement_removing_several_and_signalled_ones_stay_out_of_full_rbf() {
    let mock = Mock::new();
    let a = mock.send_with(&[coin(1)], 1, 500, 200, true);
    let b = mock.send_with(&[coin(2)], 1, 500, 200, true);
    // A child of A, which leaves with it.
    let kid = mock.send_with(&[OutPoint::new(a, 0)], 1, 500, 200, true);
    let s = Server::start(&mock, |_| {});
    s.settle().await;
    let r = mock.send_with(&[coin(1), coin(2)], 1, 9000, 300, true);
    s.settle().await;
    let body: Value = serde_json::from_str(&rbf_of(&s, kid).await.1).unwrap();
    let tree = &body["replacements"];
    assert_eq!(tree["tx"]["txid"], json!(r.to_string()));
    let mut gone: Vec<String> = tree["replaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["tx"]["txid"].as_str().unwrap().to_owned())
        .collect();
    gone.sort();
    let mut want = vec![a.to_string(), b.to_string(), kid.to_string()];
    want.sort();
    assert_eq!(gone, want, "everything it removed, its child included");
    assert_eq!(tree["fullRbf"], json!(false));
    let ctx = State(Arc::clone(&s.ctx));
    assert_eq!(answer(full_rbf_replacements(ctx).await).await.1, "[]");
    // The reference's shape for a tree one level deep, unmined.
    let list: Value = serde_json::from_str(
        &answer(replacements(State(Arc::clone(&s.ctx))).await)
            .await
            .1,
    )
    .unwrap();
    same_shape(&list, &fixture("replacements"), "replacements");
}

#[tokio::test]
async fn cpfp_is_the_clusters_own_chunks() {
    let mock = Mock::new();
    let parent = mock.send(&[coin(1)], 2, 100, 100);
    let child = mock.send(&[OutPoint::new(parent, 0)], 1, 10_000, 100);
    let grandchild = mock.send(&[OutPoint::new(child, 0)], 1, 50, 100);
    let lone = mock.send(&[coin(2)], 1, 250, 100);
    // The node mines parent and child as one chunk, the grandchild after on its own.
    mock.join(&[&[parent, child], &[grandchild]]);
    let s = Server::start(&mock, |_| {});
    s.settle().await;

    let rel = |t: Txid| {
        let m = mock.get(t);
        json!({"txid": t.to_string(), "fee": m.fee, "weight": m.vsize * 4})
    };
    let p = cpfp_json(&s, parent).await;
    assert_eq!(
        p,
        json!({
            "ancestors": [],
            "bestDescendant": rel(child),
            "descendants": [rel(child), rel(grandchild)],
            "effectiveFeePerVsize": 50.5,
            "fee": 100,
            "adjustedVsize": 100,
        })
    );
    // The one field we leave out: we count no sigops.
    let mut reference = fixture("cpfp");
    assert!(
        reference
            .as_object_mut()
            .unwrap()
            .remove("sigops")
            .is_some()
    );
    same_shape(&p, &reference, "cpfp");
    let g = cpfp_json(&s, grandchild).await;
    assert_eq!(g["ancestors"], json!([rel(parent), rel(child)]));
    assert_eq!(
        (&g["bestDescendant"], &g["effectiveFeePerVsize"]),
        (&Value::Null, &json!(0.5))
    );
    let k = cpfp_json(&s, child).await;
    assert_eq!(
        (&k["ancestors"], &k["descendants"], &k["bestDescendant"]),
        (
            &json!([rel(parent)]),
            &json!([rel(grandchild)]),
            &Value::Null
        ),
        "the grandchild is in a later chunk: it lifts nothing"
    );
    // A child in its parent's chunk that pays less lifts nothing: no best descendant.
    let rich = mock.send(&[coin(3)], 1, 1000, 100);
    let poor = mock.send(&[OutPoint::new(rich, 0)], 1, 10, 100);
    mock.join(&[&[rich, poor]]);
    s.settle().await;
    let r = cpfp_json(&s, rich).await;
    assert_eq!(
        (&r["bestDescendant"], &r["descendants"]),
        (&Value::Null, &json!([rel(poor)]))
    );
    let l = cpfp_json(&s, lone).await;
    assert_eq!(l["effectiveFeePerVsize"], json!(2.5));
    assert_eq!(l["descendants"], json!([]));
    // Not in the mempool: the reference's empty answer. Confirmed, the same.
    let none = json!({"ancestors": []});
    assert_eq!(cpfp_json(&s, Txid::from_byte_array([9; 32])).await, none);
    same_shape(&none, &fixture("cpfp_none"), "cpfp_none");
    mock.mine(&[lone]);
    s.settle().await;
    assert_eq!(cpfp_json(&s, lone).await, none);
}

#[tokio::test]
async fn with_the_engine_off_or_syncing_the_mempool_routes_say_so() {
    let off = context("127.0.0.1:1", false, |_| {});
    let t = Txid::from_byte_array([1; 32]).to_string();
    let st = || State(Arc::clone(&off));
    for r in [
        replacements(st()).await,
        full_rbf_replacements(st()).await,
        rbf(st(), Path(t.clone())).await,
        cpfp(st(), Path(t.clone())).await,
        transaction_times(st(), RawQuery(Some(format!("txId[]={t}")))).await,
    ] {
        assert_eq!(
            answer(r).await,
            (
                503,
                r#"{"error":"the mempool engine is off on this server"}"#.into()
            )
        );
    }
    // A bad txid is refused before anything else.
    assert_eq!(answer(rbf(st(), Path("xyz".into())).await).await.0, 400);
    // Engines not attached yet: syncing.
    let on = context("127.0.0.1:1", true, |_| {});
    let r = cpfp(State(on), Path(t)).await;
    assert_eq!(
        answer(r).await,
        (503, r#"{"error":"not ready yet; retry"}"#.into())
    );
}

#[test]
fn the_times_query_is_strict_and_bounded() {
    let t = Txid::from_byte_array([1; 32]);
    let ok = |q: &str| times_query(Some(q));
    assert_eq!(ok(""), Ok(Vec::new()));
    assert_eq!(ok(&format!("txId[]={t}&txId[]={t}")), Ok(vec![t, t]));
    assert_eq!(
        ok(&format!("cache=1&txid[]=zz&txId%5b%5d={t}")),
        Ok(vec![t])
    );
    let bad = Err(ApiError::bad_request("invalid txId"));
    for v in [
        "",
        "zz",
        &"0".repeat(63),
        &"0".repeat(65),
        &format!("0x{}", "0".repeat(62)),
    ] {
        assert_eq!(ok(&format!("txId[]={v}")), bad, "{v}");
    }
    let many = |n: usize| vec![format!("txId[]={t}"); n].join("&");
    assert_eq!(ok(&many(MAX_TIMES)).map(|v| v.len()), Ok(MAX_TIMES));
    assert_eq!(
        ok(&many(MAX_TIMES + 1)),
        Err(ApiError::bad_request("at most 100 txids"))
    );
    assert!(ok(&many(10_000)).is_err());
}

#[test]
fn addresses_are_validated_by_our_parser_as_the_reference_answers() {
    let want = fixture("validate_address");
    for (addr, reference) in want.as_object().unwrap() {
        let ours = match validate(addr, Network::Mainnet) {
            Some(v) => serde_json::to_value(v).unwrap(),
            None => serde_json::to_value(Invalid { isvalid: false }).unwrap(),
        };
        assert_eq!(&ours, reference, "{addr}");
    }
    // Another network's address is invalid here, whatever the network.
    assert!(
        validate(
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq",
            Network::Regtest
        )
        .is_none()
    );
    assert!(validate(&"1".repeat(10_000), Network::Mainnet).is_none());
    // Pay to anchor counts as a script and names no program, as the node answered on regtest.
    let p2a = validate("bc1pfeessrawgf", Network::Mainnet).unwrap();
    assert!(p2a.isscript && p2a.iswitness);
    assert_eq!((p2a.witness_program, p2a.witness_version), (None, None));
}

#[tokio::test]
async fn the_replacement_record_survives_a_restart_with_a_store() {
    let dir = std::env::temp_dir().join(format!("rbitcoin-space-c1b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.to_str().unwrap().to_owned();
    let mock = Mock::new();
    let a = mock.send(&[coin(1)], 1, 500, 200);
    let start = || {
        Server::on(
            context_in(
                "127.0.0.1:1",
                "http://127.0.0.1:1",
                true,
                Some(&path),
                |_| {},
            ),
            &mock,
        )
    };
    let s = start();
    s.settle().await;
    let b = mock.send_with(&[coin(1)], 1, 1000, 200, true);
    s.settle().await;
    let before = rbf_of(&s, a).await.1;
    assert!(before.contains(&b.to_string()));
    // Written by the store's writer, off the tracker's path: wait for it.
    let store = Arc::clone(&s.ctx.blocks.as_ref().unwrap().store);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while engine::replacements::db::load(&store, 10)
        .await
        .unwrap()
        .is_empty()
    {
        assert!(tokio::time::Instant::now() < deadline, "never stored");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    drop((s, store));

    // A new server on the same file knows what replaced `a`, on every route that says it.
    let s = start();
    s.settle().await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while rbf_of(&s, a).await.1 != before {
        assert!(tokio::time::Instant::now() < deadline, "never restored");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let app = crate::replacements::route(State(Arc::clone(&s.ctx)), Path(a.to_string()))
        .await
        .unwrap();
    let app: Value = serde_json::from_str(&text(app).await.1).unwrap();
    assert_eq!(app["replaced_by"][0]["by"], json!(b.to_string()));
    let t = mock.get(a).time;
    assert_eq!(times(&s, &format!("txId[]={a}")).await.1, format!("[{t}]"));
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
