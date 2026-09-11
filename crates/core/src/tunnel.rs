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
                    if let Some(Conn::Open(sock)) = held.get_mut(&id) {
                        if sock.write_all(&waited).and_then(|()| sock.flush()).is_err() {
                            held.remove(&id);
                            drop(held);
                            pipe.tell(id, Kind::Close, &[]);
                            return;
                        }
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
