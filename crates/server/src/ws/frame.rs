//! RFC 6455 framing, server side only: the opening handshake's accept key, encoding the frames we
//! send, and reading the frames a client sends. No extensions and no subprotocols are negotiated.

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt};

use super::Close;

pub const CONTINUATION: u8 = 0x0;
pub const TEXT: u8 = 0x1;
pub const BINARY: u8 = 0x2;
pub const CLOSE: u8 = 0x8;
pub const PING: u8 = 0x9;
pub const PONG: u8 = 0xA;

/// Largest payload of a control frame (close, ping, pong).
const MAX_CONTROL: usize = 125;

/// A frame encoded once and shared by every connection it is sent to: cloning is a reference
/// count, and writers put these bytes on the wire as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame(Arc<[u8]>);

impl Frame {
    pub fn text(json: &[u8]) -> Self {
        Self::encode(TEXT, json)
    }

    pub fn close(close: Close) -> Self {
        let mut payload = close.code.to_be_bytes().to_vec();
        payload.extend_from_slice(close.reason.as_bytes());
        payload.truncate(MAX_CONTROL);
        Self::encode(CLOSE, &payload)
    }

    pub fn ping() -> Self {
        Self::encode(PING, b"")
    }

    /// Echoes a client's ping. Its payload was capped at [`MAX_CONTROL`] when read.
    pub fn pong(payload: &[u8]) -> Self {
        Self::encode(PONG, payload)
    }

    /// One unfragmented, unmasked frame (servers never mask).
    pub fn encode(opcode: u8, payload: &[u8]) -> Self {
        let len = payload.len();
        let mut out = Vec::with_capacity(len + 10);
        out.push(0x80 | opcode);
        if len < 126 {
            out.push(len as u8);
        } else if let Ok(short) = u16::try_from(len) {
            out.push(126);
            out.extend_from_slice(&short.to_be_bytes());
        } else {
            out.push(127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        out.extend_from_slice(payload);
        Self(out.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Bytes on the wire, header included.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One frame as a client sent it, unmasked.
#[derive(Debug, PartialEq, Eq)]
pub struct Incoming {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// Reads one client frame. The declared length is checked against `max` (data frames) or 125
/// (control frames) before anything is allocated, so a client cannot make us buffer more.
pub async fn read<R: AsyncRead + Unpin>(rd: &mut R, max: usize) -> Result<Incoming, Close> {
    let mut head = [0u8; 2];
    rd.read_exact(&mut head).await.map_err(|_| Close::GONE)?;
    let [b0, b1] = head;
    let fin = b0 & 0x80 != 0;
    let opcode = b0 & 0x0F;
    // No extension was negotiated, so the reserved bits must be clear; clients must mask.
    if b0 & 0x70 != 0 || b1 & 0x80 == 0 {
        return Err(Close::PROTOCOL);
    }
    let control = opcode >= 0x8;
    if !matches!(opcode, CONTINUATION | TEXT | BINARY | CLOSE | PING | PONG) {
        return Err(Close::PROTOCOL);
    }
    let len = match b1 & 0x7F {
        126 => {
            let mut b = [0u8; 2];
            rd.read_exact(&mut b).await.map_err(|_| Close::GONE)?;
            u64::from(u16::from_be_bytes(b))
        }
        127 => {
            let mut b = [0u8; 8];
            rd.read_exact(&mut b).await.map_err(|_| Close::GONE)?;
            u64::from_be_bytes(b)
        }
        n => u64::from(n),
    };
    // RFC 6455 section 5.2: a length must use the shortest encoding that holds it.
    if matches!(b1 & 0x7F, 126) && len < 126 || matches!(b1 & 0x7F, 127) && len <= 0xFFFF {
        return Err(Close::PROTOCOL);
    }
    if control && (!fin || len > MAX_CONTROL as u64) {
        return Err(Close::PROTOCOL);
    }
    let len = match usize::try_from(len) {
        Ok(n) if n <= max || control => n,
        _ => return Err(Close::TOO_BIG),
    };
    let mut mask = [0u8; 4];
    rd.read_exact(&mut mask).await.map_err(|_| Close::GONE)?;
    let mut payload = vec![0u8; len];
    rd.read_exact(&mut payload).await.map_err(|_| Close::GONE)?;
    for (byte, m) in payload.iter_mut().zip(mask.iter().cycle()) {
        *byte ^= m;
    }
    Ok(Incoming {
        fin,
        opcode,
        payload,
    })
}

/// `Sec-WebSocket-Accept` for a client's `Sec-WebSocket-Key`.
pub fn accept_key(key: &[u8]) -> String {
    const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut input = key.to_vec();
    input.extend_from_slice(GUID);
    base64(&sha1(&input))
}

/// Whether `key` is a valid `Sec-WebSocket-Key`: 16 bytes in base64, so 22 characters and `==`.
pub fn valid_key(key: &[u8]) -> bool {
    key.len() == 24
        && key.ends_with(b"==")
        && key
            .iter()
            .take(22)
            .all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/')
}

/// SHA-1, needed only for the handshake's accept key (RFC 6455 section 4.2.2). Not used for security.
// Every index is bounded by a constant loop range over fixed-size arrays, never by input.
#[allow(clippy::indexing_slicing)]
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (word, bytes) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *chunk = word.to_be_bytes();
    }
    out
}

#[allow(clippy::indexing_slicing)] // `n & 63` is always inside the 64-byte alphabet.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sym = |n: u32| char::from(ALPHABET[(n & 63) as usize]);
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        out.push(sym(n >> 18));
        out.push(sym(n >> 12));
        out.push(if chunk.len() > 1 { sym(n >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(n) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_and_base64_match_known_vectors() {
        let hex = |d: [u8; 20]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        // 56 bytes: the length no longer fits the first block, so padding takes a second.
        let two_blocks = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            hex(sha1(two_blocks)),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(sha1(&million)),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
        for (input, out) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), out);
        }
    }
}
