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

/// What a WebSocket's opening request asked for, read off a raw socket: the
/// path, and the three headers a door decides by
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Opening {
    pub path: String,
    pub origin: String,
    pub host: String,
    /// The `Sec-WebSocket-Key`; empty when the request was not an opening
    pub key: String,
}

/// The longest line of an opening request, and the most lines it may have.
/// A browser's are a few hundred bytes and a dozen lines; whoever sends more
/// is not opening a WebSocket
const OPENING_LINE: u64 = 8 * 1024;
const OPENING_LINES: usize = 64;

/// How long an opening request may take to arrive. Set on the socket by the
/// door before it reads one (see [`read_opening`]), and taken off after
pub const OPENING_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

/// Read an opening request to the end of its headers. `None` when the
/// socket ends first, or the request is longer than any opening is.
///
/// Read before the caller is known to be anybody: nothing here keeps more
/// than a line of what it is sent, and a line that does not end is given up
/// on. The door sets [`OPENING_WITHIN`] as the socket's read timeout first,
/// so one that says nothing is not waited on either
pub fn read_opening<R: std::io::BufRead>(r: &mut R) -> Option<Opening> {
    // One line, or `None` for one that is too long or cut short
    fn line<R: std::io::BufRead>(r: &mut R) -> Option<String> {
        use std::io::BufRead as _;
        let mut bytes = Vec::new();
        let n = std::io::Read::take(&mut *r, OPENING_LINE).read_until(b'\n', &mut bytes).ok()?;
        (n > 0 && bytes.last() == Some(&b'\n')).then(|| String::from_utf8_lossy(&bytes).into_owned())
    }
    let first = line(r)?;
    let mut asked = Opening { path: first.split_whitespace().nth(1).unwrap_or_default().to_string(), ..Default::default() };
    for _ in 0..OPENING_LINES {
        let line = line(r)?;
        if line.trim().is_empty() {
            return Some(asked);
        }
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().to_string();
            match k.trim().to_ascii_lowercase().as_str() {
                "origin" => asked.origin = v,
                "host" => asked.host = v,
                "sec-websocket-key" => asked.key = v,
                _ => {}
            }
        }
    }
    None
}

/// The answer that lets an opening in
pub fn welcome(key: &str) -> String {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    )
}

/// The answer to an opening that is not let in. Nothing about why: a caller
/// with no business at the door learns nothing from it
pub const TURNED_AWAY: &[u8] = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Complete a handshake after the route has checked its key and admission.
pub(crate) fn upgrade(req: tiny_http::Request, key: &str) -> Box<dyn tiny_http::ReadWrite + Send> {
    let accept = accept_key(key);
    let response = tiny_http::Response::empty(101).with_header(
        tiny_http::Header::from_bytes(b"Sec-WebSocket-Accept", accept.as_bytes()).unwrap(),
    );
    req.upgrade("websocket", response)
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

/// The most a phone may say in one frame.
///
/// Everything it sends is an intent -- a tap, a line of text, a request to
/// look at something -- and none of those is large. A frame past this is not
/// a phone.
pub const ONE_INTENT: usize = 64 * 1024;

/// The most a device drawing pages may say in one frame.
///
/// These are answers rather than intents, and an answer can be a whole page's
/// HTML or a picture of one. Still bounded: a number this size is a mistake
/// somewhere, not a page
pub const ONE_ANSWER: usize = 32 * 1024 * 1024;

/// Read one frame from the client (a client frame is always masked).
/// An oversized frame or a missing mask is treated as a protocol violation
/// and closed with an error.
pub fn read_frame<R: Read>(r: &mut R) -> std::io::Result<(Op, Vec<u8>)> {
    read_frame_upto(r, ONE_INTENT)
}

/// The same, for a line whose answers are bigger than an intent.
pub fn read_frame_upto<R: Read>(r: &mut R, most: usize) -> std::io::Result<(Op, Vec<u8>)> {
    let (_, opcode, payload) = read_piece(r, most)?;
    Ok((op_of(opcode)?, payload))
}

fn op_of(opcode: u8) -> std::io::Result<Op> {
    Ok(match opcode {
        0x1 => Op::Text,
        0x2 => Op::Binary,
        0x8 => Op::Close,
        0x9 => Op::Ping,
        0xA => Op::Pong,
        other => return Err(err(&format!("unknown opcode {other}"))),
    })
}

/// A message being put together from the pieces it was sent in (see
/// [`read_message_upto`]): what it is, and what of it has arrived
#[derive(Default)]
pub struct Partial(Option<(Op, Vec<u8>)>);

/// Read one whole message, however many frames it was sent in.
///
/// A browser sends a long message in pieces -- the first says what it is,
/// the rest are continuations, the last says it is the last -- and a reader
/// that takes each frame for a message drops the first piece as nonsense
/// and hangs up on the second. `most` is the most the whole may come to.
/// A ping or a close between the pieces is handed back as it comes;
/// `partial` keeps what has arrived so far across such a return
pub fn read_message_upto<R: Read>(r: &mut R, most: usize, partial: &mut Partial) -> std::io::Result<(Op, Vec<u8>)> {
    loop {
        let room = most - partial.0.as_ref().map_or(0, |(_, whole)| whole.len());
        let (last, opcode, piece) = read_piece(r, room)?;
        match (opcode, partial.0.as_mut()) {
            // A piece after the first
            (0x0, Some((_, whole))) => {
                whole.extend_from_slice(&piece);
                if last {
                    return Ok(partial.0.take().expect("there, a line ago"));
                }
            }
            (0x0, None) => return Err(err("a continuation with nothing to continue")),
            // Something of its own in between: handed over, the pieces kept
            (0x8..=0xA, _) => return Ok((op_of(opcode)?, piece)),
            (_, Some(_)) => return Err(err("a new message inside another")),
            (_, None) if last => return Ok((op_of(opcode)?, piece)),
            (_, None) => partial.0 = Some((op_of(opcode)?, piece)),
        }
    }
}

/// One frame as it is on the wire: whether it is the last of its message,
/// its opcode, and what it carries
fn read_piece<R: Read>(r: &mut R, most: usize) -> std::io::Result<(bool, u8, Vec<u8>)> {
    let mut hdr = [0u8; 2];
    r.read_exact(&mut hdr)?;
    let last = hdr[0] & 0x80 != 0;
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
    if len > most {
        return Err(err(&format!("frame too large ({len} bytes)")));
    }
    let mut mask = [0u8; 4];
    r.read_exact(&mut mask)?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i & 3];
    }
    Ok((last, opcode, payload))
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

    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
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

/// A way out to a socket for a thread that must never wait on it.
///
/// What is sent is queued and written by a thread of the lane's own, so the
/// sender -- the thread every page's talk arrives on, a window's loop -- is
/// never held up by a reader that is slow. And what is queued is counted: a
/// reader that has stopped reading (a tab asleep, a line gone quiet) while
/// the sender has a great deal to say would otherwise have all of it kept
/// here, without end. Past `most` bytes waiting the lane closes: nothing
/// more is taken, what waits is thrown away, and the socket is cut loose
#[derive(Clone)]
pub struct Lane {
    out: std::sync::mpsc::Sender<Vec<u8>>,
    behind: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    shut: std::sync::Arc<std::sync::atomic::AtomicBool>,
    most: usize,
    cut: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lane").field("closed", &self.closed()).finish_non_exhaustive()
    }
}

impl Lane {
    /// Start writing to `w`. `cut` ends the socket under it from any
    /// thread (for a TCP one, a shutdown of a clone of it), and is called
    /// once, when the lane is over: closed for being too far behind, a
    /// write failed, or every sender gone.
    ///
    /// Cut from outside, and not left to the writer to stop: a write in
    /// progress to a reader taking a byte now and then goes on for as long
    /// as that reader likes, and a lane said to be closed would keep its
    /// socket and its thread until then
    pub fn open<W: Write + Send + 'static>(mut w: W, most: usize, cut: impl Fn() + Send + Sync + 'static) -> Lane {
        use std::sync::atomic::Ordering::Relaxed;
        let (out, waiting) = std::sync::mpsc::channel::<Vec<u8>>();
        let lane = Lane { out, behind: Default::default(), shut: Default::default(), most, cut: std::sync::Arc::new(cut) };
        let (behind, mine) = (lane.behind.clone(), lane.loose());
        std::thread::spawn(move || {
            for frame in waiting {
                if mine.closed() {
                    break;
                }
                let sent = w.write_all(&frame).and_then(|()| w.flush());
                behind.fetch_sub(frame.len(), Relaxed);
                if sent.is_err() {
                    break;
                }
            }
            mine.close();
        });
        lane
    }

    /// This lane without its way in: what the writer holds, so that the
    /// writer does not keep its own queue open
    fn loose(&self) -> Lane {
        let (out, _) = std::sync::mpsc::channel();
        Lane { out, ..self.clone() }
    }

    /// Close the lane, cutting its socket loose the first time
    fn close(&self) {
        if !self.shut.swap(true, std::sync::atomic::Ordering::SeqCst) {
            (self.cut)();
        }
    }

    /// Queue one frame. `false` when the lane is closed, this frame being
    /// the one that closed it included
    pub fn send(&self, frame: Vec<u8>) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        if self.closed() {
            return false;
        }
        let len = frame.len();
        if self.behind.fetch_add(len, Relaxed) + len > self.most {
            self.behind.fetch_sub(len, Relaxed);
            self.close();
            // Wakes the writer, when it is waiting for something to write
            let _ = self.out.send(Vec::new());
            return false;
        }
        self.out.send(frame).is_ok()
    }

    /// Queue a text message
    pub fn send_text(&self, text: &str) -> bool {
        self.send(encode(Op::Text, text.as_bytes()))
    }

    /// Whether the lane has closed
    pub fn closed(&self) -> bool {
        self.shut.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ── the client half ─────────────────────────────────────────────────────────
//
// This program is a client as well as a server: it speaks the DevTools
// protocol to a browser, and carries a tunnel out to one. The difference is
// one rule turned around -- a client's frames must carry a mask, a server's
// must not -- so it is the same code, not a second copy of it.

/// Build a frame to send *as a client*: masked, as the protocol requires.
///
/// A fresh mask each time, which is what makes it worth having: not secrecy,
/// which it does not provide, but bytes that cannot be predicted by anything
/// in between which might otherwise mistake them for something to act on.
pub fn client_encode(op: Op, payload: &[u8]) -> Vec<u8> {
    let opcode: u8 = match op {
        Op::Text => 0x1,
        Op::Binary => 0x2,
        Op::Close => 0x8,
        Op::Ping => 0x9,
        Op::Pong => 0xA,
    };
    let mut out = vec![0x80 | opcode]; // FIN + the kind
    let mask: [u8; 4] = crate::random_bytes(4)
        .and_then(|b| b.try_into().ok())
        .unwrap_or([0xA1, 0xB2, 0xC3, 0xD4]);
    let len = payload.len();
    match len {
        0..=125 => out.push(0x80 | len as u8),
        126..=65535 => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
    out
}

/// Read one frame from a server: whether it ends the message, what kind it is,
/// and its bytes. `None` once there is nothing more to read.
///
/// Unlike [`read_frame`], this accepts an unmasked frame -- which is what a
/// server sends -- and hands back the pieces of a message that arrived in
/// several frames rather than refusing them.
pub fn read_server_frame<R: Read>(s: &mut R) -> Option<(bool, u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    s.read_exact(&mut head).ok()?;
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let len = match head[1] & 0x7f {
        126 => {
            let mut n = [0u8; 2];
            s.read_exact(&mut n).ok()?;
            u16::from_be_bytes(n) as usize
        }
        127 => {
            let mut n = [0u8; 8];
            s.read_exact(&mut n).ok()?;
            u64::from_be_bytes(n) as usize
        }
        n => n as usize,
    };
    // A browser answering a screenshot sends megabytes; anything past this is
    // not an answer anybody asked for
    if len > 64 * 1024 * 1024 {
        return None;
    }
    let mask = masked
        .then(|| {
            let mut m = [0u8; 4];
            s.read_exact(&mut m).ok().map(|()| m)
        })
        .flatten();
    let mut payload = vec![0u8; len];
    s.read_exact(&mut payload).ok()?;
    if let Some(m) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= m[i & 3];
        }
    }
    Some((fin, opcode, payload))
}

#[cfg(test)]
mod tests {
    /// A message sent in pieces is put together again, a ping between the
    /// pieces is handed over as it comes, and the whole may only be so long
    #[test]
    fn a_message_in_pieces_is_read_whole() {
        use super::{Op, Partial, client_encode, read_message_upto};
        // One piece as a client writes it: the first says what it is, the
        // rest are continuations, the last says it is the last
        let piece = |first: bool, last: bool, body: &[u8]| {
            let mut f = client_encode(Op::Text, body);
            f[0] = (if last { 0x80 } else { 0 }) | (if first { 0x01 } else { 0 });
            f
        };
        let mut wire = Vec::new();
        wire.extend(piece(true, false, b"{\"a\":"));
        wire.extend(client_encode(Op::Ping, b"still there?"));
        wire.extend(piece(false, false, b"[1,2,"));
        wire.extend(piece(false, true, b"3]}"));
        wire.extend(client_encode(Op::Text, b"whole"));
        let mut r = wire.as_slice();
        let mut partial = Partial::default();
        assert_eq!(read_message_upto(&mut r, 1024, &mut partial).unwrap(), (Op::Ping, b"still there?".to_vec()));
        assert_eq!(read_message_upto(&mut r, 1024, &mut partial).unwrap(), (Op::Text, b"{\"a\":[1,2,3]}".to_vec()));
        assert_eq!(read_message_upto(&mut r, 1024, &mut partial).unwrap(), (Op::Text, b"whole".to_vec()));

        // Longer in all than it may be, however small the pieces
        let mut wire = Vec::new();
        wire.extend(piece(true, false, &[b'x'; 600]));
        wire.extend(piece(false, true, &[b'x'; 600]));
        assert!(read_message_upto(&mut wire.as_slice(), 1000, &mut Partial::default()).is_err(), "pieces add up past the most");
        // A continuation of nothing, and a new message inside another
        assert!(read_message_upto(&mut piece(false, true, b"x").as_slice(), 1000, &mut Partial::default()).is_err());
        let mut wire = piece(true, false, b"a");
        wire.extend(client_encode(Op::Text, b"b"));
        assert!(read_message_upto(&mut wire.as_slice(), 1000, &mut Partial::default()).is_err());
    }

    /// An opening is read to the end of its headers, and one that is longer
    /// than any opening is given up on without being kept
    #[test]
    fn an_opening_is_read_and_a_flood_is_not() {
        use super::read_opening;
        let asked = read_opening(&mut "GET /devtools/k/T HTTP/1.1\r\nHost: 127.0.0.1:9\r\nOrigin: devtools://devtools\r\nSec-WebSocket-Key: abc==\r\nX-Other: 1\r\n\r\nrest".as_bytes()).unwrap();
        assert_eq!((asked.path.as_str(), asked.host.as_str(), asked.origin.as_str(), asked.key.as_str()), ("/devtools/k/T", "127.0.0.1:9", "devtools://devtools", "abc=="));
        // Cut short
        assert!(read_opening(&mut "GET / HTTP/1.1\r\nHost: x\r\n".as_bytes()).is_none());
        assert!(read_opening(&mut "".as_bytes()).is_none());
        // A line that never ends, and lines without end
        let long = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(100_000));
        assert!(read_opening(&mut long.as_bytes()).is_none(), "a line of any length was read");
        let many = format!("GET / HTTP/1.1\r\n{}\r\n", "X: 1\r\n".repeat(10_000));
        assert!(read_opening(&mut many.as_bytes()).is_none(), "any number of lines was read");
    }

    /// A lane hands on what it is given, closes when too much waits on it,
    /// and cuts its socket loose once -- though a write is still under way
    #[test]
    fn a_lane_that_falls_too_far_behind_is_cut_loose() {
        use super::Lane;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex, mpsc};
        // A reader that takes what is written only when told to
        struct Slow(mpsc::Receiver<()>, Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Slow {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.recv().map_err(|_| std::io::Error::other("cut"))?;
                self.1.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (take, taken) = mpsc::channel();
        let written: Arc<Mutex<Vec<u8>>> = Default::default();
        let cuts = Arc::new(AtomicUsize::new(0));
        let lane = Lane::open(Slow(taken, Arc::clone(&written)), 10, {
            let cuts = Arc::clone(&cuts);
            move || {
                cuts.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert!(lane.send(b"12345".to_vec()));
        take.send(()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while written.lock().unwrap().len() < 5 {
            assert!(std::time::Instant::now() < until, "nothing was written");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(*written.lock().unwrap(), b"12345");
        // The reader stops taking: a write waits on it, and more piles up
        assert!(lane.send(b"abcde".to_vec()));
        assert!(lane.send(b"fghij".to_vec()));
        assert!(!lane.closed());
        assert!(!lane.send(b"k".to_vec()), "more than it may hold was taken");
        assert!(lane.closed());
        assert_eq!(cuts.load(Ordering::SeqCst), 1, "not cut loose while a write was under way");
        assert!(!lane.send(b"l".to_vec()), "a closed lane took something");
        // Cut, the write under way fails, and the writer ends: cut once
        drop(take);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(cuts.load(Ordering::SeqCst), 1, "cut more than once");
    }

    /// A frame this writes must be one the other side can read back. The two
    /// halves are written here, so a mistake in the length forms would agree
    /// with itself and prove nothing -- the lengths below are chosen to cross
    /// both boundaries where the form changes
    #[test]
    fn a_frame_survives_being_written_and_read() {
        // Lengths in bytes, so the body is made of one-byte characters. The
        // three forms change at 125/126 and 65535/65536, and each is crossed
        let mut bodies: Vec<String> = [0usize, 1, 125, 126, 127, 65535, 65536]
            .into_iter()
            .map(|n| "a".repeat(n))
            .collect();
        // ...and one that is not ASCII at all, because the length is counted
        // in bytes and a character is not a byte
        bodies.push("あいうえお かきくけこ".to_string());
        for body in &bodies {
            let framed = client_encode(Op::Text, body.as_bytes());
            let mut cursor = std::io::Cursor::new(framed);
            let (fin, opcode, payload) =
                read_server_frame(&mut cursor).expect("a frame it wrote cannot be read");
            assert!(fin, "it does not end in one frame");
            assert_eq!(opcode, 0x1, "not a text frame");
            assert_eq!(&String::from_utf8(payload).unwrap(), body, "the contents changed");
        }
    }

    /// The mask is what makes a client's frame legal, and a fresh one each
    /// time is what makes it worth having
    #[test]
    fn every_frame_is_masked_differently() {
        let a = client_encode(Op::Text, b"hello");
        let b = client_encode(Op::Text, b"hello");
        assert_eq!(a[1] & 0x80, 0x80, "it sends frames with no mask");
        assert_ne!(a, b, "it uses the same mask every time");
    }

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
