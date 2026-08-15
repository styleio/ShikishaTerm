//! A minimal WebSocket server (RFC 6455).
//!
//! Used to stream screen-mirroring (VNC-equivalent) frames and receive
//! finger-trace input with low latency. Since we control both ends (this
//! server and the shell.rs client) and the transport is confined to a
//! private network (LAN / Tailscale), we grab the raw socket via tiny_http's
//! upgrade and implement only framing and the handshake ourselves. This
//! avoids adding an external dependency.
//!
//! Supports text / binary / ping / pong / close frames, and a single frame
//! up to 64KiB. Fragmentation (continuation frames) is not handled, since
//! the side we talk to never sends them.

use std::io::{Read, Write};

use base64::Engine as _;
const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Build the handshake response key. Appends the fixed GUID to the client's
/// Sec-WebSocket-Key, then SHA-1 hashes and base64-encodes it (the fixed
/// transform defined by RFC 6455).
pub fn accept_key(client_key: &str) -> String {
    const MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut input = client_key.as_bytes().to_vec();
    input.extend_from_slice(MAGIC.as_bytes());
    B64.encode(sha1(&input))
}

/// Frame type. How the payload is interpreted depends on this opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

/// Build a frame to send from the server (no mask is applied — that's the
/// server-side convention).
pub fn encode(op: Op, payload: &[u8]) -> Vec<u8> {
    let opcode: u8 = match op {
        Op::Text => 0x1,
        Op::Binary => 0x2,
        Op::Close => 0x8,
        Op::Ping => 0x9,
        Op::Pong => 0xA,
    };
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | opcode); // FIN=1
    let len = payload.len();
    if len < 126 {
        out.push(len as u8);
    } else if len <= 0xFFFF {
        out.push(126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

/// Read one frame from the client (a client frame is always masked).
/// An oversized frame (>64KiB) or a missing mask is treated as a protocol
/// violation and closed with an error.
pub fn read_frame<R: Read>(r: &mut R) -> std::io::Result<(Op, Vec<u8>)> {
    let mut hdr = [0u8; 2];
    r.read_exact(&mut hdr)?;
    let opcode = hdr[0] & 0x0F;
    let masked = hdr[1] & 0x80 != 0;
    let mut len = (hdr[1] & 0x7F) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        r.read_exact(&mut ext)?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        r.read_exact(&mut ext)?;
        len = u64::from_be_bytes(ext) as usize;
    }
    if !masked {
        return Err(err("client frame was not masked"));
    }
    if len > 64 * 1024 {
        return Err(err("frame too large"));
    }
    let mut mask = [0u8; 4];
    r.read_exact(&mut mask)?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i & 3];
    }
    let op = match opcode {
        0x1 => Op::Text,
        0x2 => Op::Binary,
        0x8 => Op::Close,
        0x9 => Op::Ping,
        0xA => Op::Pong,
        other => return Err(err(&format!("unknown opcode {other}"))),
    };
    Ok((op, payload))
}

fn err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

/// SHA-1. Used only to build the handshake response key (not for
/// security-sensitive purposes). A straightforward implementation of
/// RFC 3174, kept in-house to avoid adding a dependency.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// A thin wrapper for writing server-side frames. Holds the raw socket until
/// closed.
pub struct WsWriter<W: Write> {
    inner: W,
}

impl<W: Write> WsWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }
    pub fn send_text(&mut self, s: &str) -> std::io::Result<()> {
        self.inner.write_all(&encode(Op::Text, s.as_bytes()))?;
        self.inner.flush()
    }
    pub fn send_binary(&mut self, b: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(&encode(Op::Binary, b))?;
        self.inner.flush()
    }
    pub fn send_pong(&mut self, payload: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(&encode(Op::Pong, payload))?;
        self.inner.flush()
    }
    pub fn send_close(&mut self) -> std::io::Result<()> {
        self.inner.write_all(&encode(Op::Close, &[]))?;
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_known_vectors() {
        // RFC 3174 / well-known test vectors
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
    }

    #[test]
    fn accept_key_matches_rfc_example() {
        // Example from RFC 6455 4.2.2
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn server_frame_roundtrips_through_client_reader() {
        // A body built by the server, re-wrapped with a client-side mask, should
        // come back out through read_frame
        let payload = b"hello \xf0\x9f\x91\x8b".to_vec(); // includes an emoji
        let masked = mask_as_client(Op::Text, &payload);
        let (op, got) = read_frame(&mut &masked[..]).unwrap();
        assert_eq!(op, Op::Text);
        assert_eq!(got, payload);
    }

    #[test]
    fn oversize_frame_is_rejected() {
        // A 65KiB frame is rejected as a protocol violation
        let big = vec![0u8; 65 * 1024];
        let masked = mask_as_client(Op::Binary, &big);
        assert!(read_frame(&mut &masked[..]).is_err());
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Build a frame following client conventions (mask required), for tests.
    fn mask_as_client(op: Op, payload: &[u8]) -> Vec<u8> {
        let opcode: u8 = match op {
            Op::Text => 0x1,
            Op::Binary => 0x2,
            Op::Close => 0x8,
            Op::Ping => 0x9,
            Op::Pong => 0xA,
        };
        let mut out = vec![0x80 | opcode];
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        let len = payload.len();
        if len < 126 {
            out.push(0x80 | len as u8);
        } else if len <= 0xFFFF {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
        out
    }
}
