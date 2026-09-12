//! A way out to the network, for a browser that is somewhere else.
//!
//! The reason is `localhost`. An agent on this machine starts something on
//! port 3000 and the person wants to look at it. If their browser reaches the
//! network from their own desk, `localhost:3000` is *their* machine -- the
//! wrong machine, and usually nothing at all. The same goes for a private
//! network this machine can see and their laptop cannot, and for what a site
//! thinks the visitor's address is.
//!
//! So the page can be drawn over there while everything it fetches comes
//! through here. Names are resolved here too, which falls out of the design
//! rather than being arranged: the far side asks for `whatever:443` by name,
//! and this is what looks it up.
//!
//! It carries no more power than the caller already holds. A paired client can
//! type into any terminal on this machine, and a terminal can open any socket;
//! this is narrower than what one line of shell would do.
//!
//! What it does carry is *everything* the browser over there does, not only
//! what a page asked for -- a browser reaches for services of its own accord,
//! and those come through here too (measured: WebView2 asking Microsoft about
//! itself). The machine at this end sees that traffic, which is the price of
//! the page seeing this machine's network.
//!
//! ## The shape of it
//!
//! Every connection a browser makes shares one pipe, because a socket per
//! connection is a pair of threads per connection and a page can open twenty.
//! A frame is `[id: u32][kind: u8][payload]`, and the frame boundary is the
//! transport's -- one WebSocket binary message, which already carries a
//! length.
//!
//! Nothing here knows about WebSockets. Frames arrive through [`Pipe::accept`]
//! and leave through the sink it was built with, which is what lets the whole
//! thing be measured without a socket in sight.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a frame is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Open a connection to the address in the payload (`host:port`, UTF-8)
    Open = 1,
    /// Bytes, in whichever direction the frame is travelling
    Data = 2,
    /// This connection is finished. Sent from either side, and by this side
    /// when an open fails -- a refusal and a hang-up are the same news to a
    /// browser, which shows its own "cannot reach" page for both
    Close = 3,
}

impl Kind {
    fn of(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Kind::Open),
            2 => Some(Kind::Data),
            3 => Some(Kind::Close),
            _ => None,
        }
    }
}

/// One frame, ready to hand to whatever is carrying it.
pub fn frame(id: u32, kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.push(kind as u8);
    out.extend_from_slice(payload);
    out
}

/// Read one back. `None` for anything that is not a frame this speaks.
pub fn unframe(bytes: &[u8]) -> Option<(u32, Kind, &[u8])> {
    if bytes.len() < 5 {
        return None;
    }
    let id = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    Some((id, Kind::of(bytes[4])?, &bytes[5..]))
}

/// How long to spend trying to reach somewhere before saying it cannot be
/// reached. Short: the far side is usually this machine, or something beside
/// it on a private network
const REACH: std::time::Duration = std::time::Duration::from_secs(10);

/// Bytes read from the far side in one go. A browser's download is the thing
/// that fills this, and a bigger buffer is fewer frames
const CHUNK: usize = 32 * 1024;

type Sink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

/// How far along one connection is.
///
/// Reaching somewhere takes a moment, and a client does not wait for it: it
/// asks for a connection and sends its request in the same breath, which is
/// what a browser does. Those bytes wait here rather than being dropped on the
/// floor -- dropped, they would be the first line of an HTTP request, and the
/// far side would sit waiting for a request that never came
enum Conn {
    /// On its way, with what has been said in the meantime
    Reaching(Vec<u8>),
    Open(TcpStream),
}

/// How much may wait for a connection that has not been made yet. A request's
/// headers are a few kilobytes; anything on this scale is a client that is not
/// waiting for an answer it should be waiting for
const WAITING_ROOM: usize = 1024 * 1024;

/// Every connection one client has open through this machine.
pub struct Pipe {
    /// Every connection, on its way or made
    open: Mutex<HashMap<u32, Conn>>,
    /// Where frames bound for the client go
    out: Sink,
    /// Whether this pipe is still to be used. A pipe is shut when the client
    /// goes away or its session is taken from it, and every connection it
    /// holds goes with it -- a page must not keep fetching through a client
    /// that has been cut off
    live: Arc<AtomicBool>,
}

impl Pipe {
    /// A pipe whose frames go to `out`.
    pub fn new(out: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(HashMap::new()),
            out: Arc::new(out),
            live: Arc::new(AtomicBool::new(true)),
        })
    }

    /// One frame from the client.
    ///
    /// Anything malformed is dropped rather than answered. The far end of this
    /// is a proxy this program wrote, so a frame it cannot read is a bug and
    /// not a negotiation.
    pub fn accept(self: &Arc<Self>, bytes: &[u8]) {
        if !self.live.load(Ordering::Relaxed) {
            return;
        }
        let Some((id, kind, payload)) = unframe(bytes) else { return };
        match kind {
            Kind::Open => {
                let Ok(addr) = std::str::from_utf8(payload) else { return };
                // Written down before anything is reached, so what the client
                // says next has somewhere to wait
                self.open
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(id, Conn::Reaching(Vec::new()));
                self.dial(id, addr.to_string());
            }
            Kind::Data => {
                // Written from this thread on purpose: the frames of one
                // connection arrive in order and have to leave in order, and
                // handing them to a thread each would be how they stop being
                // in order
                let mut held = self.open.lock().unwrap_or_else(|e| e.into_inner());
                let gone = match held.get_mut(&id) {
                    Some(Conn::Open(sock)) => {
                        sock.write_all(payload).and_then(|()| sock.flush()).is_err()
                    }
                    Some(Conn::Reaching(waiting)) => {
                        waiting.extend_from_slice(payload);
                        waiting.len() > WAITING_ROOM
                    }
                    // Bytes for a connection that is already over. The client
                    // has not heard the hang-up yet; it will
                    None => return,
                };
                if gone {
                    held.remove(&id);
                    drop(held);
                    self.tell(id, Kind::Close, &[]);
                }
            }
            Kind::Close => self.hang_up(id),
        }
    }

    /// Reach somewhere, and start carrying what comes back.
    ///
    /// On its own thread because reaching somewhere can take as long as the
    /// name takes to look up, and every other connection is waiting on the one
    /// line this arrived on.
    fn dial(self: &Arc<Self>, id: u32, addr: String) {
        let pipe = Arc::clone(self);
        let started = std::thread::Builder::new()
            .name("shikisha-tunnel".into())
            .spawn(move || {
                let Some(sock) = reach(&addr) else {
                    crate::append_hook_log(&format!("tunnel: cannot reach {addr}"));
                    pipe.tell(id, Kind::Close, &[]);
                    return;
                };
                let Ok(reading) = sock.try_clone() else {
                    pipe.tell(id, Kind::Close, &[]);
                    return;
                };
                // Whatever was said while this was on its way goes first, in
                // the order it was said
                let waited = {
                    let mut held = pipe.open.lock().unwrap_or_else(|e| e.into_inner());
                    match held.insert(id, Conn::Open(sock)) {
                        Some(Conn::Reaching(waiting)) => waiting,
                        // Closed while it was on its way. Nothing to carry
                        _ => {
                            held.remove(&id);
                            return;
                        }
                    }
                };
                if !waited.is_empty() {
                    let mut held = pipe.open.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(Conn::Open(sock)) = held.get_mut(&id)
                        && sock.write_all(&waited).and_then(|()| sock.flush()).is_err()
                    {
                        held.remove(&id);
                        drop(held);
                        pipe.tell(id, Kind::Close, &[]);
                        return;
                    }
                }
                pipe.carry(id, reading);
            });
        if started.is_err() {
            self.tell(id, Kind::Close, &[]);
        }
    }

    /// Everything the far side says, until it stops saying it.
    fn carry(self: &Arc<Self>, id: u32, mut sock: TcpStream) {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if !self.live.load(Ordering::Relaxed) {
                        break;
                    }
                    self.tell(id, Kind::Data, &buf[..n]);
                }
            }
        }
        self.open.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        self.tell(id, Kind::Close, &[]);
    }

    /// Let go of one connection.
    fn hang_up(&self, id: u32) {
        // The reading thread is inside `read`; shutting the socket is what
        // wakes it up to find out that it is over
        if let Some(Conn::Open(sock)) = self.open.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
    }

    fn tell(&self, id: u32, kind: Kind, payload: &[u8]) {
        (self.out)(frame(id, kind, payload));
    }

    /// How many connections are open. For a person asking what is going on
    pub fn count(&self) -> usize {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Shut everything, and refuse anything further.
    ///
    /// Called when the client goes away or is cut off. Without it, a page left
    /// open on a disconnected laptop would go on fetching through this machine
    pub fn shut(&self) {
        self.live.store(false, Ordering::Relaxed);
        let held: Vec<TcpStream> = self
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .filter_map(|(_, conn)| match conn {
                Conn::Open(sock) => Some(sock),
                Conn::Reaching(_) => None,
            })
            .collect();
        for sock in held {
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.shut();
    }
}

/// Reach an address, by name, from here.
///
/// Every address it is given is looked up here, which is the point: a name
/// that means one thing on this machine and another somewhere else -- a
/// private network's name, or `localhost` -- has to mean what it means here.
fn reach(addr: &str) -> Option<TcpStream> {
    use std::net::ToSocketAddrs as _;
    // A port is required. Without one this would guess, and a guess about
    // where to connect is how a request for :443 goes to :80 in the clear
    if !addr.rsplit(':').next().is_some_and(|p| p.parse::<u16>().is_ok()) {
        return None;
    }
    for one in addr.to_socket_addrs().ok()? {
        if let Ok(sock) = TcpStream::connect_timeout(&one, REACH) {
            let _ = sock.set_nodelay(true);
            return Some(sock);
        }
    }
    None
}

// ── the other end ─────────────────────────────────────────────────────────
//
// A browser cannot be told "send everything through that program". It can be
// told "send everything to that proxy", which every browser has understood
// for thirty years, and which needs nothing of the browser at all. So this is
// a proxy: a door on the loopback that a browser is pointed at, and which
// carries what it hears to the machine at the other end of the line.

/// A way on to the network of the machine at the other end of the line.
///
/// Pointed at by a browser (`--proxy-server=http://127.0.0.1:<port>`), and
/// good for as long as it is held. Dropping it takes down the line and
/// everything on it.
pub struct Proxy {
    port: u16,
    stop: Arc<AtomicBool>,
    /// The line out. Held so the frames of one connection cannot interleave
    /// with another's halfway through
    up: Arc<Mutex<Wire>>,
    /// The browser's connections, by the number they were given
    here: Arc<Mutex<HashMap<u32, std::net::TcpStream>>>,
    next: Arc<std::sync::atomic::AtomicU32>,
}

impl Proxy {
    /// Open the line and start listening for a browser.
    ///
    /// `base` is where the board is served from, and the key and cookie are
    /// the ones this client was let in with: the way out is a door in the same
    /// house, behind the same lock.
    pub fn start(base: &str, token: &str, cookie: &str) -> anyhow::Result<Self> {
        let name = crate::random_hex(10);
        // This one first: the line carrying what that machine says is the one
        // the other half looks for
        let down = handshake(base, &format!("/ws-tunnel?p={name}&t={token}"), cookie)?;
        let up = handshake(base, &format!("/ws-tunnel-in?p={name}&t={token}"), cookie)?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();

        let proxy = Self {
            port,
            stop: Arc::new(AtomicBool::new(false)),
            up: Arc::new(Mutex::new(up)),
            here: Arc::new(Mutex::new(HashMap::new())),
            next: Arc::new(std::sync::atomic::AtomicU32::new(1)),
        };
        proxy.hear(down);
        proxy.answer(listener);
        Ok(proxy)
    }

    /// Where to tell a browser to send everything.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// What the far machine says, given back to whichever connection asked.
    fn hear(&self, mut down: Wire) {
        let here = Arc::clone(&self.here);
        let stop = Arc::clone(&self.stop);
        std::thread::Builder::new()
            .name("shikisha-tunnel-down".into())
            .spawn(move || {
                let mut whole = Vec::new();
                loop {
                    let Some((fin, opcode, payload)) = crate::ws::read_server_frame(&mut down)
                    else {
                        break;
                    };
                    match opcode {
                        // continuation, text, binary
                        0x0 | 0x1 | 0x2 => {
                            whole.extend_from_slice(&payload);
                            if !fin {
                                continue;
                            }
                            let message = std::mem::take(&mut whole);
                            let Some((id, kind, bytes)) = unframe(&message) else { continue };
                            let mut held = here.lock().unwrap_or_else(|e| e.into_inner());
                            match kind {
                                Kind::Data => {
                                    let gone = held.get_mut(&id).is_some_and(|sock| {
                                        sock.write_all(bytes).and_then(|()| sock.flush()).is_err()
                                    });
                                    if gone {
                                        held.remove(&id);
                                    }
                                }
                                // The far side hung up, or could not reach
                                // anywhere. Both are the same news here
                                Kind::Close | Kind::Open => {
                                    if let Some(sock) = held.remove(&id) {
                                        let _ = sock.shutdown(std::net::Shutdown::Both);
                                    }
                                }
                            }
                        }
                        0x8 => break, // close
                        _ => {}       // ping/pong
                    }
                }
                stop.store(true, Ordering::Relaxed);
                for (_, sock) in here.lock().unwrap_or_else(|e| e.into_inner()).drain() {
                    let _ = sock.shutdown(std::net::Shutdown::Both);
                }
            })
            .ok();
    }

    /// Take what the browser asks for, one connection at a time.
    fn answer(&self, listener: std::net::TcpListener) {
        let stop = Arc::clone(&self.stop);
        let here = Arc::clone(&self.here);
        let up = Arc::clone(&self.up);
        let next = Arc::clone(&self.next);
        std::thread::Builder::new()
            .name("shikisha-proxy".into())
            .spawn(move || {
                for sock in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(sock) = sock else { continue };
                    let (stop, here, up, next) = (
                        Arc::clone(&stop),
                        Arc::clone(&here),
                        Arc::clone(&up),
                        Arc::clone(&next),
                    );
                    std::thread::spawn(move || {
                        carry_one(&sock, &stop, &here, &up, &next);
                    });
                }
            })
            .ok();
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Waking the listener: it is inside `accept`, and the only thing that
        // brings it back is somebody knocking
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        let _ = self.up.lock().map(|s| s.close());
        for (_, sock) in self.here.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// One connection a browser made, from its first line to its last byte.
fn carry_one(
    sock: &std::net::TcpStream,
    stop: &Arc<AtomicBool>,
    here: &Arc<Mutex<HashMap<u32, std::net::TcpStream>>>,
    up: &Arc<Mutex<Wire>>,
    next: &Arc<std::sync::atomic::AtomicU32>,
) {
    let Ok(mut reading) = sock.try_clone() else { return };
    let Some(head) = read_head(&mut reading) else { return };
    let Some(asked) = Asked::read(&head) else {
        let _ = (&*sock).write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        return;
    };

    let id = next.fetch_add(1, Ordering::Relaxed);
    let Ok(mine) = sock.try_clone() else { return };
    here.lock().unwrap_or_else(|e| e.into_inner()).insert(id, mine);

    let say = |kind: Kind, payload: &[u8]| -> bool {
        let framed = crate::ws::client_encode(crate::ws::Op::Binary, &frame(id, kind, payload));
        let mut line = up.lock().unwrap_or_else(|e| e.into_inner());
        line.write_all(&framed).and_then(|()| line.flush()).is_ok()
    };

    if !say(Kind::Open, asked.to.as_bytes()) {
        return;
    }
    match asked.tunnelled {
        // A browser waits to be told the way is open before it starts
        true => {
            if (&*sock)
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .is_err()
            {
                return;
            }
        }
        // ...and for an ordinary http request, the request itself is the
        // first thing to send, with the address put back the way a server
        // expects to see it
        false => {
            if !say(Kind::Data, asked.head.as_bytes()) {
                return;
            }
        }
    }
    // And from here it is bytes, until there are none
    let mut buf = vec![0u8; CHUNK];
    loop {
        match reading.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stop.load(Ordering::Relaxed) || !say(Kind::Data, &buf[..n]) {
                    break;
                }
            }
        }
    }
    here.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    say(Kind::Close, &[]);
}

/// What a browser asked this proxy for.
struct Asked {
    /// Where it wants to reach, as `host:port`
    to: String,
    /// Whether it asked for a way through rather than for a page. `CONNECT` is
    /// what a browser sends for anything encrypted, which is nearly everything
    tunnelled: bool,
    /// The request to pass on, with the address written the way a server
    /// expects it. Empty for a tunnel, which carries no request of its own
    head: String,
}

impl Asked {
    fn read(head: &str) -> Option<Self> {
        let line = head.lines().next()?;
        let mut parts = line.split(' ');
        let method = parts.next()?;
        let target = parts.next()?;
        if method.eq_ignore_ascii_case("CONNECT") {
            return with_port(target, 443).map(|to| Self { to, tunnelled: true, head: String::new() });
        }
        // Absolute form -- `GET http://host/path HTTP/1.1` -- which is what a
        // browser sends to a proxy for anything not encrypted. A server will
        // not answer that, so the address goes back to being a path
        let rest = target.strip_prefix("http://")?;
        let (host, path) = rest.split_once('/').map_or((rest, String::new()), |(h, p)| (h, format!("/{p}")));
        let to = with_port(host, 80)?;
        let path = if path.is_empty() { "/".to_string() } else { path };
        let version = parts.next().unwrap_or("HTTP/1.1");
        let after = head.split_once("\r\n").map_or("", |(_, r)| r);
        Some(Self {
            to,
            tunnelled: false,
            head: format!("{method} {path} {version}\r\n{after}"),
        })
    }
}

/// `host:port`, with the port filled in when the address left it out.
///
/// A bare name would leave the far side guessing, and a guess about which port
/// is how a request for an encrypted page goes out in the clear.
fn with_port(host: &str, fallback: u16) -> Option<String> {
    if host.is_empty() {
        return None;
    }
    // A bracketed address is IPv6, whose colons are its own
    let has_port = match host.rfind(']') {
        Some(at) => host[at..].contains(':'),
        None => host.matches(':').count() == 1,
    };
    match has_port {
        true => Some(host.to_string()),
        false => Some(format!("{host}:{fallback}")),
    }
}

/// Everything up to the blank line.
///
/// A byte at a time, so nothing past the blank line is swallowed: what
/// follows it belongs to whoever asked, and on a proxy's line that is the
/// first thing a browser sends.
fn read_head(from: &mut impl std::io::Read) -> Option<String> {
    let mut seen = Vec::new();
    let mut one = [0u8; 1];
    while seen.len() < 64 * 1024 {
        match from.read(&mut one) {
            Ok(0) | Err(_) => return None,
            Ok(_) => seen.push(one[0]),
        }
        if seen.ends_with(b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&seen).to_string());
        }
    }
    None
}

// ── reaching a board ──────────────────────────────────────────────────────

/// A line to a board: plain bytes, or the same bytes inside TLS.
///
/// Which one it is depends on how the board is addressed, and nothing above
/// this has to know: a WebSocket frame is a WebSocket frame either way. The
/// encrypted kind cannot be cloned the way a socket can -- the session has
/// state, and two halves of it would be two different conversations -- so the
/// socket underneath is kept alongside, purely to be able to shut the whole
/// thing at once.
pub struct Wire {
    line: Box<dyn ReadWrite + Send>,
    raw: TcpStream,
}

pub trait ReadWrite: std::io::Read + std::io::Write {}
impl<T: std::io::Read + std::io::Write> ReadWrite for T {}

impl std::io::Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.line.read(buf)
    }
}

impl std::io::Write for Wire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.line.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.line.flush()
    }
}

impl Wire {
    /// End it, from wherever. What is blocked reading finds out by the read
    /// ending, which is the only thing that wakes a blocked read
    pub fn close(&self) {
        let _ = self.raw.shutdown(std::net::Shutdown::Both);
    }
}

/// Open a line to a board, encrypted if its address says so.
///
/// The roots are the ones this program carries rather than the machine's. On
/// a server that is often the only set there is, and on a desktop it is the
/// same set the browser would use -- a board behind `tailscale serve` has an
/// ordinary certificate from an ordinary authority, and this is what checks it
pub(crate) fn dial(base: &str) -> anyhow::Result<Wire> {
    let (encrypted, rest) = match base.trim_end_matches('/') {
        b if b.starts_with("https://") => (true, &b[8..]),
        b if b.starts_with("http://") => (false, &b[7..]),
        _ => anyhow::bail!(crate::i18n::tp("err.tunnel.bad_address", &[("base", base)])),
    };
    let host = rest.split('/').next().unwrap_or(rest);
    let (name, _) = host.rsplit_once(':').unwrap_or((host, ""));
    let with_port = match host.contains(':') {
        true => host.to_string(),
        false => format!("{host}:{}", if encrypted { 443 } else { 80 }),
    };
    let raw = TcpStream::connect(&with_port)?;
    raw.set_nodelay(true)?;
    if !encrypted {
        let line = Box::new(raw.try_clone()?);
        return Ok(Wire { line, raw });
    }
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let server = rustls::pki_types::ServerName::try_from(name.to_string())
        .map_err(|_| anyhow::anyhow!(crate::i18n::tp("err.tunnel.bad_address", &[("base", base)])))?;
    let session = rustls::ClientConnection::new(std::sync::Arc::new(config), server)?;
    let line = Box::new(rustls::StreamOwned::new(session, raw.try_clone()?));
    Ok(Wire { line, raw })
}

/// Open a WebSocket to a board, by hand.
///
/// The answer's key is not checked. What that check proves is that the far
/// side speaks WebSocket rather than being a cache that echoed the request,
/// and the far side here has already been let in through a lock
pub(crate) fn handshake(base: &str, path: &str, cookie: &str) -> anyhow::Result<Wire> {
    let mut wire = dial(base)?;
    let host = base
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let key = {
        use base64::Engine as _;
        let bytes = crate::random_bytes(16).unwrap_or_else(|| vec![0; 16]);
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    let carried = match cookie.is_empty() {
        true => String::new(),
        false => format!("Cookie: {cookie}\r\n"),
    };
    write!(
        wire,
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         {carried}\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    )?;
    wire.flush()?;

    let said = read_head(&mut wire).ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.tunnel.refused")))?;
    if !said.starts_with("HTTP/1.1 101") {
        anyhow::bail!(crate::i18n::tp(
            "err.tunnel.refused_with",
            &[("said", said.lines().next().unwrap_or("?"))]
        ));
    }
    Ok(wire)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    /// A pipe whose frames land in a queue a test can read.
    fn pipe() -> (Arc<Pipe>, Receiver<Vec<u8>>) {
        let (tx, rx) = channel();
        let pipe = Pipe::new(move |f| {
            let _ = tx.send(f);
        });
        (pipe, rx)
    }

    /// Wait for the next frame of one connection.
    ///
    /// Frames for the others are kept aside rather than thrown away -- two
    /// connections share this line, and discarding one while waiting for the
    /// other would be the test losing it, not the pipe
    fn next_of(rx: &Receiver<Vec<u8>>, spare: &mut Vec<Vec<u8>>, id: u32) -> Option<(Kind, Vec<u8>)> {
        if let Some(at) = spare.iter().position(|f| unframe(f).is_some_and(|(got, ..)| got == id)) {
            let (_, kind, payload) = unframe(&spare.remove(at)).map(|(i, k, p)| (i, k, p.to_vec()))?;
            return Some((kind, payload));
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < until {
            let Ok(f) = rx.recv_timeout(std::time::Duration::from_millis(500)) else { continue };
            match unframe(&f) {
                Some((got, kind, payload)) if got == id => return Some((kind, payload.to_vec())),
                Some(_) => spare.push(f),
                None => {}
            }
        }
        None
    }

    /// Something to reach: it answers with whatever it was told, upper-cased,
    /// so the two directions cannot be confused for one another
    fn shouter() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for sock in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut sock = sock;
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = sock.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        let said = String::from_utf8_lossy(&buf[..n]).to_uppercase();
                        if sock.write_all(said.as_bytes()).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    /// An address says which kind of line to open, and a bad one says so.
    #[test]
    fn an_address_says_whether_the_line_is_encrypted() {
        // Nothing is reached here -- only the reading of the address, which is
        // what decides between a plain socket and a TLS session
        for bad in ["", "ws://board", "board:8787", "ftp://board/"] {
            let said = dial(bad).map(|_| String::new()).unwrap_or_else(|e| e.to_string());
            assert!(
                said.contains(bad) || said.is_empty() == false,
                "住所として断っていない: {bad}"
            );
            assert!(dial(bad).is_err(), "話せない住所を受けている: {bad}");
        }
    }

    /// The encrypted line, against a server that really is one.
    ///
    /// Ignored by default because it needs the internet, and a test that fails
    /// on a train is a test people learn to ignore. What it proves is the part
    /// no local test can: a real certificate, checked against the roots this
    /// program carries, with bytes going both ways afterwards.
    ///
    ///     cargo test -p shikisha-core --lib really_encrypted -- --ignored --nocapture
    #[test]
    #[ignore = "needs the internet"]
    fn an_https_board_is_reached_and_really_encrypted() {
        use std::io::Write as _;
        let site = "https://shikisha-term.com";
        let mut wire = dial(site).expect("繋がらない");
        write!(
            wire,
            "GET / HTTP/1.1\r\nHost: shikisha-term.com\r\nConnection: close\r\n\r\n"
        )
        .expect("書けない");
        wire.flush().unwrap();
        let said = read_head(&mut wire).expect("何も返ってこない");
        let first = said.lines().next().unwrap_or_default().to_string();
        assert!(first.starts_with("HTTP/"), "HTTP が返っていない: {first}");
        println!("{site} -> {first}");

        // And the same address spelled without its scheme is refused rather
        // than guessed at: a guess would be plain text to a port expecting TLS
        assert!(dial("shikisha-term.com").is_err());
    }

    /// A frame survives being written and read
    #[test]
    fn a_frame_says_which_connection_and_what_for() {
        let f = frame(7, Kind::Data, b"hello");
        let (id, kind, payload) = unframe(&f).expect("読めない");
        assert_eq!((id, kind, payload), (7, Kind::Data, &b"hello"[..]));
        // Empty payloads are ordinary (a close carries none)
        assert_eq!(unframe(&frame(1, Kind::Close, b"")).unwrap().2.len(), 0);
        // Anything shorter than a header, or a kind this does not speak
        assert!(unframe(b"abc").is_none());
        assert!(unframe(&[0, 0, 0, 1, 99]).is_none());
    }

    /// The whole thing: open somewhere, say something, hear it back, hang up
    #[test]
    fn bytes_go_out_and_come_back() {
        let far = shouter();
        let (pipe, rx) = pipe();
        let spare = &mut Vec::new();
        // Asked for and spoken to in the same breath, which is what a browser
        // does: the request cannot wait for the connection to be made
        pipe.accept(&frame(1, Kind::Open, far.as_bytes()));
        pipe.accept(&frame(1, Kind::Data, "こんにちは hello".as_bytes()));
        let (kind, said) = next_of(&rx, spare, 1).expect("返事が来ない");
        assert_eq!(kind, Kind::Data);
        assert_eq!(String::from_utf8_lossy(&said), "こんにちは HELLO");

        // Two connections at once do not become one
        pipe.accept(&frame(2, Kind::Open, far.as_bytes()));
        pipe.accept(&frame(2, Kind::Data, b"two"));
        pipe.accept(&frame(1, Kind::Data, b"one"));
        assert_eq!(next_of(&rx, spare, 2).unwrap().1, b"TWO");
        assert_eq!(next_of(&rx, spare, 1).unwrap().1, b"ONE");
        assert_eq!(pipe.count(), 2);

        pipe.accept(&frame(1, Kind::Close, b""));
        pipe.accept(&frame(2, Kind::Close, b""));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while pipe.count() > 0 && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(pipe.count(), 0, "切ったのに握ったままになっている");
    }

    /// Somewhere that cannot be reached is news, not silence: a browser
    /// waiting for an answer that never comes shows nothing at all
    #[test]
    fn nowhere_answers_as_a_hang_up() {
        let (pipe, rx) = pipe();
        // Port 1 on this machine, which nothing listens on
        let spare = &mut Vec::new();
        pipe.accept(&frame(9, Kind::Open, b"127.0.0.1:1"));
        assert_eq!(next_of(&rx, spare, 9).expect("何も答えない").0, Kind::Close);
        // An address with no port is refused the same way rather than guessed
        pipe.accept(&frame(10, Kind::Open, b"example.com"));
        assert_eq!(next_of(&rx, spare, 10).expect("何も答えない").0, Kind::Close);
    }

    /// A cut-off client's pages stop fetching through this machine
    #[test]
    fn shutting_the_pipe_shuts_what_it_was_holding() {
        let far = shouter();
        let (pipe, rx) = pipe();
        pipe.accept(&frame(1, Kind::Open, far.as_bytes()));
        pipe.accept(&frame(1, Kind::Data, b"x"));
        assert_eq!(next_of(&rx, &mut Vec::new(), 1).unwrap().1, b"X");
        pipe.shut();
        assert_eq!(pipe.count(), 0, "握ったままになっている");
        // And nothing further is opened
        pipe.accept(&frame(2, Kind::Open, far.as_bytes()));
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(pipe.count(), 0, "閉じた後に繋いでいる");
    }
}
