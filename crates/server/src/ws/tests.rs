#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::frame::{self, Incoming};
use super::*;
use crate::config::Network;

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

#[test]
fn requests_are_parsed_strictly_and_subscriptions_capped() {
    let hub = hub();
    let (conn, mut rx) = conn(8);
    let mut subs = Subscriptions {
        hub: &hub,
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
    ] {
        assert_eq!(subs.handle(bad, 8), Err(Close::BAD_MESSAGE), "{bad}");
    }
    assert_eq!(subs.handle(r#"{"op":"ping"}"#, 8), Ok(()));
    assert_eq!(rx.try_recv().unwrap(), Frame::text(br#"{"type":"pong"}"#));

    assert_eq!(
        subs.handle(r#"{"op":"subscribe","topics":["blocks"]}"#, 0),
        Err(Close::TOO_MANY_SUBSCRIPTIONS)
    );
    let sub = r#"{"op":"subscribe","topics":["blocks","blocks"]}"#;
    assert_eq!(subs.handle(sub, 1), Ok(()));
    // Subscribing again is a no-op, not a second subscription.
    assert_eq!(subs.handle(sub, 1), Ok(()));
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
