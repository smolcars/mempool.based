#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::print_stderr)]

use std::collections::HashMap;

use super::frame::{self, Incoming};
use super::*;
use crate::config::Network;
use axum::http::header::{HOST, ORIGIN};

/// A client frame as it goes on the wire: masked with a fixed key.
fn masked(b0: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![b0];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xFFFF => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let key = [0x37, 0xfa, 0x21, 0x3d];
    out.extend_from_slice(&key);
    out.extend(payload.iter().zip(key.iter().cycle()).map(|(b, k)| b ^ k));
    out
}

async fn read(bytes: &[u8], max: usize) -> Result<Incoming, Close> {
    let mut rd = bytes;
    frame::read(&mut rd, max).await
}

#[test]
fn accept_key_matches_rfc_6455() {
    assert_eq!(
        frame::accept_key(b"dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
    assert!(frame::valid_key(b"dGhlIHNhbXBsZSBub25jZQ=="));
    for bad in [
        &b""[..],
        b"dGhlIHNhbXBsZSBub25jZQ=",
        b"dGhlIHNhbXBsZSBub25j\r\n==",
        b"dGhlIHNhbXBsZSBub25jZQ=x",
    ] {
        assert!(!frame::valid_key(bad), "{bad:?}");
    }
}

#[test]
fn frames_use_the_shortest_length_encoding() {
    for (len, head) in [(0, 2), (125, 2), (126, 4), (65_535, 4), (65_536, 10)] {
        let f = Frame::encode(frame::BINARY, &vec![7; len]);
        assert_eq!(f.len(), head + len, "{len}");
        assert_eq!(f.as_bytes()[0], 0x82);
    }
    let close = Frame::close(Close::SLOW);
    assert_eq!(&close.as_bytes()[..4], &[0x88, 10, 0x0F, 0xA3]);
    assert_eq!(&close.as_bytes()[4..], b"too slow");
}

#[tokio::test]
async fn client_frames_are_unmasked_and_checked() {
    let f = read(&masked(0x81, b"hello"), 16).await.unwrap();
    assert_eq!(
        (f.fin, f.opcode, f.payload.as_slice()),
        (true, frame::TEXT, &b"hello"[..])
    );
    let long = vec![b'a'; 300];
    assert_eq!(read(&masked(0x01, &long), 300).await.unwrap().payload, long);

    // Unmasked, reserved bits, unknown opcode, fragmented or oversized control frames.
    assert_eq!(read(&[0x81, 0x00], 16).await, Err(Close::PROTOCOL));
    assert_eq!(read(&masked(0xC1, b"x"), 16).await, Err(Close::PROTOCOL));
    assert_eq!(read(&masked(0x83, b"x"), 16).await, Err(Close::PROTOCOL));
    assert_eq!(read(&masked(0x09, b"x"), 16).await, Err(Close::PROTOCOL));
    assert_eq!(
        read(&masked(0x89, &[0; 126]), 1024).await,
        Err(Close::PROTOCOL)
    );
    // A control frame may exceed a small message cap up to 125 bytes.
    assert!(read(&masked(0x89, &[0; 125]), 16).await.is_ok());
    // Too big is refused from the header alone, before the payload arrives.
    assert_eq!(read(&masked(0x81, &[0; 17]), 16).await, Err(Close::TOO_BIG));
    let huge = [0x81, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    assert_eq!(read(&huge, 16).await, Err(Close::TOO_BIG));
    // Lengths must use the shortest encoding.
    let mut long_form = vec![0x89, 0xFE, 0x00, 0x00];
    long_form.extend_from_slice(&[0; 4]);
    assert_eq!(read(&long_form, 1024).await, Err(Close::PROTOCOL));
    let mut longer_form = vec![0x81, 0xFF, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF];
    longer_form.extend_from_slice(&[0; 4]);
    assert_eq!(read(&longer_form, 1 << 20).await, Err(Close::PROTOCOL));
    // Truncated input is a lost connection.
    let whole = masked(0x81, b"hello");
    assert_eq!(read(&whole[..whole.len() - 1], 16).await, Err(Close::GONE));
    assert_eq!(read(&[], 16).await, Err(Close::GONE));
}

fn conn(queue: usize) -> (Arc<Conn>, mpsc::Receiver<Frame>) {
    let (tx, rx) = mpsc::channel(queue);
    let conn = Conn {
        id: rand_id(),
        queue: tx,
        closing: watch::Sender::new(None),
    };
    (Arc::new(conn), rx)
}

fn rand_id() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn hub() -> Hub {
    Hub::new(
        Network::Regtest,
        Sockets::new(&WsLimits::default(), Duration::from_secs(1)),
    )
}

#[test]
fn subscribers_get_the_snapshot_then_shared_bytes_and_slow_ones_are_closed() {
    let hub = hub();
    let first = Frame::text(b"1");
    let (a, mut a_rx) = conn(4);
    hub.subscribe(Topic::Blocks, &a);
    assert!(a_rx.try_recv().is_err(), "no snapshot yet");

    let fanout = hub.publish(Topic::Blocks, &first, Snapshot::Replace);
    assert_eq!((fanout.subscribers, fanout.dropped_slow), (1, 0));
    // The same allocation reaches every subscriber.
    let got = a_rx.try_recv().unwrap();
    assert!(std::ptr::eq(got.as_bytes(), first.as_bytes()));

    let (b, mut b_rx) = conn(1);
    hub.subscribe(Topic::Blocks, &b);
    assert_eq!(b_rx.try_recv().unwrap(), first);

    // b's queue holds one frame: the second publish fills it, the third closes it as slow.
    hub.publish(Topic::Blocks, &Frame::text(b"2"), Snapshot::Keep);
    let third = hub.publish(Topic::Blocks, &Frame::text(b"3"), Snapshot::Keep);
    assert_eq!(third.dropped_slow, 1);
    assert_eq!(b.reason(), Some(Close::SLOW));
    assert_eq!(a.reason(), None);
    // Already closing: not counted again.
    assert_eq!(
        hub.publish(Topic::Blocks, &Frame::text(b"4"), Snapshot::Clear)
            .dropped_slow,
        0
    );

    let (c, mut c_rx) = conn(4);
    hub.subscribe(Topic::Blocks, &c);
    assert!(c_rx.try_recv().is_err(), "snapshot was cleared");
    hub.unsubscribe(Topic::Blocks, c.id);
    hub.publish(Topic::Blocks, &Frame::text(b"5"), Snapshot::Keep);
    assert!(c_rx.try_recv().is_err());
}

/// Limits with `max` subscriptions, two of them projected blocks' transactions.
fn subs_limits(max: usize) -> WsLimits {
    WsLimits {
        max_subscriptions: max,
        max_block_subscriptions: 2,
        ..WsLimits::default()
    }
}

/// Tracking on [`hub`], with the mempool engine on but not attached, and no Electrum client.
fn track() -> Arc<Tracking> {
    Arc::new(Tracking::new(
        Network::Regtest,
        Arc::new(hub()),
        None,
        true,
        &WsLimits::default(),
    ))
}

/// A regtest address paying to a made-up witness script hash.
pub(super) fn address(n: u8) -> String {
    use bitcoin::hashes::Hash;
    let script = bitcoin::ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([n; 32]));
    bitcoin::Address::from_script(&script, bitcoin::Network::Regtest)
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn requests_are_parsed_strictly_and_subscriptions_capped() {
    let track = track();
    let hub = track.hub();
    let (conn, mut rx) = conn(8);
    let mut subs = Subscriptions {
        track: &track,
        conn: &conn,
        topics: HashSet::new(),
    };
    for bad in [
        "",
        "null",
        "[]",
        r#"{"op":"subscribe"}"#,
        r#"{"op":"subscribe","topics":"blocks"}"#,
        r#"{"op":"subscribe","topics":["nope"]}"#,
        r#"{"op":"unsubscribe","topics":["Blocks"]}"#,
        r#"{"op":"shout"}"#,
        r#"{"topics":["blocks"]}"#,
        r#"{"op":"subscribe","topics":["projected-txs:7"]}"#,
        r#"{"op":"subscribe","topics":["projected-txs:07"]}"#,
        r#"{"op":"subscribe","topics":["projected-txs:"]}"#,
        r#"{"op":"subscribe","topics":["projected-txs:-1"]}"#,
        r#"{"op":"subscribe","topics":["projected-txs"]}"#,
        // Tracked topics: strict hex in lower case, an address of this network only.
        r#"{"op":"subscribe","topics":["tx:"]}"#,
        r#"{"op":"subscribe","topics":["tx:0000000000000000000000000000000000000000000000000000000000000001 "]}"#,
        r#"{"op":"subscribe","topics":["tx:000000000000000000000000000000000000000000000000000000000000000"]}"#,
        r#"{"op":"subscribe","topics":["tx:000000000000000000000000000000000000000000000000000000000000000A"]}"#,
        r#"{"op":"subscribe","topics":["tx:+00000000000000000000000000000000000000000000000000000000000000a"]}"#,
        r#"{"op":"subscribe","topics":["address:"]}"#,
        r#"{"op":"subscribe","topics":["address:bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"]}"#,
        r#"{"op":"subscribe","topics":["address:bcrt1qnotanaddress"]}"#,
        r#"{"op":"subscribe","topics":["tx"]}"#,
    ] {
        assert_eq!(
            subs.handle(bad, &subs_limits(8)),
            Err(Close::BAD_MESSAGE),
            "{bad}"
        );
    }
    assert_eq!(subs.handle(r#"{"op":"ping"}"#, &subs_limits(8)), Ok(()));
    assert_eq!(rx.try_recv().unwrap(), Frame::text(br#"{"type":"pong"}"#));
    // Tracked transactions and addresses have caps of their own inside the total.
    let id = |n: u8| format!("\"tx:{}\"", format!("{n:02x}").repeat(32));
    let tracked = WsLimits {
        max_tracked_txs: 2,
        max_tracked_addresses: 1,
        ..subs_limits(8)
    };
    let sub = |names: &str| format!(r#"{{"op":"subscribe","topics":[{names}]}}"#);
    assert_eq!(
        subs.handle(&sub(&format!("{},{}", id(1), id(2))), &tracked),
        Ok(())
    );
    assert_eq!(
        subs.handle(&sub(&id(3)), &tracked),
        Err(Close::TOO_MANY_SUBSCRIPTIONS)
    );
    let first = format!("\"address:{}\"", address(1).to_uppercase());
    assert_eq!(subs.handle(&sub(&first), &tracked), Ok(()));
    assert_eq!(
        subs.handle(&sub(&format!("\"address:{}\"", address(2))), &tracked),
        Err(Close::TOO_MANY_SUBSCRIPTIONS)
    );
    assert_eq!(track.tracked(), (2, 1));
    // Without a node connection the address is `unavailable`, under its name as the server
    // writes it.
    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(frame.as_bytes()).into_owned();
    assert!(
        text.contains(&format!("\"address:{}\"", address(1))),
        "{text}"
    );
    assert!(text.contains("unavailable"), "{text}");
    let unsub = format!(r#"{{"op":"unsubscribe","topics":[{},{first}]}}"#, id(1));
    assert_eq!(subs.handle(&unsub, &tracked), Ok(()));
    assert_eq!(track.tracked(), (1, 0));
    assert_eq!(
        subs.handle(
            &format!(r#"{{"op":"unsubscribe","topics":[{}]}}"#, id(2)),
            &tracked
        ),
        Ok(())
    );

    assert_eq!(
        subs.handle(r#"{"op":"subscribe","topics":["blocks"]}"#, &subs_limits(0)),
        Err(Close::TOO_MANY_SUBSCRIPTIONS)
    );
    let sub = r#"{"op":"subscribe","topics":["blocks","blocks"]}"#;
    assert_eq!(subs.handle(sub, &subs_limits(1)), Ok(()));
    // Subscribing again is a no-op, not a second subscription.
    assert_eq!(subs.handle(sub, &subs_limits(1)), Ok(()));
    // Projected blocks' transactions have a cap of their own inside the total.
    let txs = |names: &str| format!(r#"{{"op":"subscribe","topics":[{names}]}}"#);
    let all = subs_limits(8);
    assert_eq!(
        subs.handle(
            &txs(r#""mempool","projected","transactions","projected-txs:0""#),
            &all
        ),
        Ok(())
    );
    assert_eq!(
        subs.handle(&txs(r#""projected-txs:6","projected-txs:0""#), &all),
        Ok(())
    );
    assert_eq!(
        subs.handle(&txs(r#""projected-txs:1""#), &all),
        Err(Close::TOO_MANY_SUBSCRIPTIONS)
    );
    let swap = r#"{"op":"unsubscribe","topics":["projected-txs:6","mempool","projected","transactions","projected-txs:0"]}"#;
    assert_eq!(subs.handle(swap, &all), Ok(()));
    assert_eq!(subs.handle(&txs(r#""projected-txs:1""#), &all), Ok(()));
    assert_eq!(
        subs.handle(r#"{"op":"unsubscribe","topics":["projected-txs:1"]}"#, &all),
        Ok(())
    );
    assert_eq!(
        hub.publish(Topic::Blocks, &Frame::text(b"x"), Snapshot::Keep)
            .subscribers,
        1
    );
    drop(subs);
    assert_eq!(
        hub.publish(Topic::Blocks, &Frame::text(b"x"), Snapshot::Keep)
            .subscribers,
        0
    );
}

#[test]
fn client_close_frames_are_checked() {
    let close = |code: u16, reason: &[u8]| {
        let mut p = code.to_be_bytes().to_vec();
        p.extend_from_slice(reason);
        peer_close(&p)
    };
    assert_eq!(peer_close(b""), Close::NORMAL);
    assert_eq!(close(1000, b"bye"), Close::NORMAL);
    assert_eq!(close(1001, b""), Close::NORMAL);
    assert_eq!(close(4999, b""), Close::NORMAL);
    assert_eq!(peer_close(&[0x03]), Close::PROTOCOL);
    for reserved in [0, 999, 1004, 1005, 1006, 1015, 2999, 5000] {
        assert_eq!(close(reserved, b""), Close::PROTOCOL, "{reserved}");
    }
    assert_eq!(close(1000, &[0xff]), Close::INVALID_UTF8);
}

/// xorshift64*: a deterministic generator, so a failing stream can be replayed from its index.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// One client frame, mostly well formed: a random opcode, sometimes reserved bits, no mask, a
/// non-minimal or absurd length, or a payload that is not what its header says.
fn fuzz_frame(rng: &mut Rng, out: &mut Vec<u8>) {
    let opcode = match rng.below(10) {
        0..=3 => frame::TEXT,
        4 => frame::CONTINUATION,
        5 => frame::PING,
        6 => frame::PONG,
        7 => frame::CLOSE,
        8 => frame::BINARY,
        _ => rng.below(16) as u8,
    };
    let fin = if rng.chance(3) { 0 } else { 0x80 };
    let rsv = if rng.chance(20) {
        0x10 << rng.below(3)
    } else {
        0
    };
    let payload = match rng.below(6) {
        0 => br#"{"op":"ping"}"#.to_vec(),
        1 => br#"{"op":"subscribe","topics":["blocks"]}"#.to_vec(),
        2 => 1000u16.to_be_bytes().to_vec(),
        3 => vec![b' '; rng.below(300)],
        _ => {
            let n = rng.below(140);
            rng.bytes(n)
        }
    };
    out.push(fin | rsv | opcode);
    let mask_bit = if rng.chance(30) { 0 } else { 0x80 };
    let len = payload.len() as u64;
    match rng.below(12) {
        0 => {
            out.push(mask_bit | 127);
            let absurd = [u64::MAX, 1 << 63, 1 << 32, 0x1_0000, len];
            out.extend_from_slice(&absurd[rng.below(absurd.len())].to_be_bytes());
        }
        1 => {
            out.push(mask_bit | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ if len < 126 => out.push(mask_bit | len as u8),
        _ if len <= 0xFFFF => {
            out.push(mask_bit | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            out.push(mask_bit | 127);
            out.extend_from_slice(&len.to_be_bytes());
        }
    }
    let key = rng.bytes(4);
    if mask_bit != 0 {
        out.extend_from_slice(&key);
    }
    out.extend(payload.iter().zip(key.iter().cycle()).map(|(b, k)| b ^ k));
}

/// A byte stream: several frames, then sometimes bit flips, a cut, or plain noise.
fn fuzz_stream(rng: &mut Rng) -> Vec<u8> {
    if rng.chance(10) {
        let n = rng.below(64);
        return rng.bytes(n);
    }
    let mut out = Vec::new();
    for _ in 0..1 + rng.below(12) {
        fuzz_frame(rng, &mut out);
    }
    if !out.is_empty() && rng.chance(3) {
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(out.len());
            out[i] ^= 1 << rng.below(8);
        }
    }
    if !out.is_empty() && rng.chance(4) {
        out.truncate(rng.below(out.len()));
    }
    out
}

/// A reader over a byte slice that records the largest buffer it is asked to fill. The parser
/// reads each payload straight into a buffer of the payload's size, so this is the largest
/// payload it allocated.
struct Probe<'a> {
    data: &'a [u8],
    largest: usize,
}

impl tokio::io::AsyncRead for Probe<'_> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.largest = self.largest.max(buf.remaining());
        let n = buf.remaining().min(self.data.len());
        let (now, rest) = self.data.split_at(n);
        buf.put_slice(now);
        self.data = rest;
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn the_parser_survives_random_and_mutated_streams() {
    const STREAMS: u64 = 50_000;
    let defined = [
        Close::NORMAL,
        Close::PROTOCOL,
        Close::UNSUPPORTED,
        Close::INVALID_UTF8,
        Close::TOO_BIG,
        Close::BAD_MESSAGE,
        Close::TOO_MANY_SUBSCRIPTIONS,
        Close::GONE,
    ];
    let track = track();
    let started = Instant::now();
    let mut seen = HashMap::new();
    let mut rng = Rng(0x5EED_CAFE_F00D_D00D);
    for i in 0..STREAMS {
        let max = [16, 125, 200, 1024][rng.below(4)];
        let limits = WsLimits {
            max_message_bytes: max,
            messages_per_sec: 1_000_000,
            ..WsLimits::default()
        };
        let stream = fuzz_stream(&mut rng);
        let mut probe = Probe {
            data: &stream,
            largest: 0,
        };
        let (conn, _rx) = conn(1024);
        let opened = Instant::now();
        read_loop(
            &mut probe,
            &track,
            &conn,
            &limits,
            opened,
            &AtomicU64::new(0),
        )
        .await;
        let reason = conn.reason();
        assert!(
            reason.is_some_and(|r| defined.contains(&r)),
            "stream {i}: {reason:?}"
        );
        assert!(
            probe.largest <= max.max(125),
            "stream {i}: a {}-byte read with a {max}-byte limit",
            probe.largest
        );
        *seen.entry(reason.map(|r| r.code)).or_insert(0u32) += 1;
    }
    // Every outcome the generator aims at was reached, so the streams are not all rejected early.
    for code in [1000, 1002, 1003, 1006, 1007, 1009, 4000] {
        assert!(
            seen.contains_key(&Some(code)),
            "{code} never reached: {seen:?}"
        );
    }
    eprintln!("{STREAMS} streams in {:?}: {seen:?}", started.elapsed()); // test output only
}

#[test]
fn reassembly_never_holds_more_than_the_limit() {
    for (max, step) in [(1024, 1), (1024, 7), (1000, 333), (16, 16)] {
        let mut buf = vec![b'a'; step];
        while buf.len() + step <= max {
            append(&mut buf, &vec![b'a'; step], max).unwrap();
            assert!(buf.capacity() <= max, "{} > {max}", buf.capacity());
        }
        assert_eq!(append(&mut buf, &vec![b'a'; max], max), Err(Close::TOO_BIG));
    }
}

#[test]
fn appended_changes_follow_the_state_until_folded_into_a_new_one() {
    let hub = hub();
    let topic = Topic::ProjectedTxs(0);
    hub.publish(topic, &Frame::text(b"s1"), Snapshot::Replace);
    hub.publish(topic, &Frame::text(b"d1"), Snapshot::Append);
    hub.publish(topic, &Frame::text(b"d2"), Snapshot::Append);
    let (a, mut a_rx) = conn(8);
    hub.subscribe(topic, &a);
    for want in [&b"s1"[..], b"d1", b"d2"] {
        assert_eq!(a_rx.try_recv().unwrap(), Frame::text(want));
    }
    // Folding sends nothing to existing subscribers.
    hub.set_snapshot(topic, Frame::text(b"s2"));
    assert!(a_rx.try_recv().is_err());
    let (b, mut b_rx) = conn(8);
    hub.subscribe(topic, &b);
    assert_eq!(b_rx.try_recv().unwrap(), Frame::text(b"s2"));
    assert!(b_rx.try_recv().is_err());
    assert_eq!(hub.subscribers(topic), 2);
}

#[test]
fn only_this_site_may_open_a_socket_from_a_browser() {
    let check = |origins: &[&str], host: Option<&str>| {
        let mut h = HeaderMap::new();
        for o in origins {
            h.append(ORIGIN, HeaderValue::from_str(o).unwrap());
        }
        if let Some(host) = host {
            h.insert(HOST, HeaderValue::from_str(host).unwrap());
        }
        same_origin(&h).is_ok()
    };
    // No Origin: not a browser.
    assert!(check(&[], Some("explorer.example")));
    assert!(check(&[], None));
    for (origin, host) in [
        ("https://explorer.example", "explorer.example"),
        ("https://Explorer.Example", "explorer.example"),
        ("https://explorer.example", "explorer.example:443"),
        ("https://explorer.example:8443", "explorer.example:8443"),
        ("http://127.0.0.1:5173", "127.0.0.1:5173"),
        ("http://[::1]:8080", "[::1]:8080"),
        ("http://other-name.example", "other-name.example"),
    ] {
        assert!(check(&[origin], Some(host)), "{origin} {host}");
    }
    for (origins, host) in [
        (&["https://evil.example"][..], Some("explorer.example")),
        (
            &["https://explorer.example.evil.example"],
            Some("explorer.example"),
        ),
        (&["https://explorer.example:8443"], Some("explorer.example")),
        (&["http://127.0.0.1:5173"], Some("127.0.0.1:8080")),
        (&["null"], Some("explorer.example")),
        (&["ws://explorer.example"], Some("explorer.example")),
        (&["https://"], Some("")),
        (&["https://explorer.example"], None),
        (
            &["https://explorer.example", "https://explorer.example"],
            Some("explorer.example"),
        ),
    ] {
        assert!(!check(origins, host), "{origins:?} {host:?}");
    }
}
