//! Sending a page's picture to a phone as video, rather than as a stream of
//! JPEGs.
//!
//! What the phone sees today is one JPEG per change, sent over the relay's
//! WebSocket (`remote.rs` `/ws`). Every frame is the whole picture compressed
//! again from nothing, so a page that is moving costs the machine and the line
//! at the same time, and there is nowhere for sound to go. Measured on a
//! rented Linux machine, the same picture as video cost a third of the CPU and
//! a ninth of the bytes.
//!
//! This module is the far end of that: one peer connection per viewer, fed
//! frames that are already compressed. What it is **not** is the encoder, the
//! screen, or the signalling — each of those belongs to somebody who already
//! owns it, and holding them here would mean this could only ever be used the
//! one way.
//!
//! **Nothing here reaches the network on its own.** `str0m` holds no sockets;
//! every packet that leaves is one this file handed to a socket it made. That
//! is a property worth stating out loud: a screen relay that quietly asked a
//! public STUN server where this machine lives would be publishing the
//! person's home address as a side effect of showing them their own screen.
//! Nothing here can, because there is nothing here that could.
//!
//! The pieces it does not own:
//!
//! | what | who has it |
//! |---|---|
//! | the picture | `cdp.rs` / `browser.rs`, as JPEG frames |
//! | compressing it | the encoder (Media Foundation on Windows, libvpx elsewhere) |
//! | carrying the offer and answer | the relay's WebSocket, which is already open |
//! | deciding a viewer wants video | `remote.rs` |

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

/// How long a connection is given to be established before it is called dead.
///
/// A phone that walked out of range never says so; the only sign is that the
/// checks stop being answered. Long enough not to give up on a slow network,
/// short enough that a viewer who is gone stops being sent pictures.
const SETTLE: Duration = Duration::from_secs(20);

/// The longest a single UDP datagram this reads can be. Ordinary media
/// datagrams are far under 1500; this is the room the library asks for.
const DATAGRAM: usize = 2000;

/// One compressed picture, on its way out.
pub struct Frame {
    /// The compressed bytes, as the encoder produced them
    pub data: Vec<u8>,
    /// Whether this frame stands on its own. A viewer that joins late, or one
    /// whose picture has been damaged, can only start again from one of these
    pub keyframe: bool,
    /// When the picture was taken. Not when it was compressed and not when it
    /// was sent: the receiver lines frames up by this, and measuring it later
    /// puts the jitter of our own machine into the picture
    pub taken: Instant,
}

/// How a connection is getting on, in the three states anything outside it
/// needs to tell apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Still finding a path to the far end
    Connecting,
    /// Pictures are flowing
    Live,
    /// Over. Either side may have ended it, and from out here it makes no
    /// difference: what is left to do is the same
    Gone,
}

impl State {
    fn of(n: u8) -> State {
        match n {
            1 => State::Live,
            2 => State::Gone,
            _ => State::Connecting,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            State::Connecting => 0,
            State::Live => 1,
            State::Gone => 2,
        }
    }
}

/// One viewer, watching over WebRTC.
///
/// Dropping it ends the connection: the thread sees the channel close and
/// stops. There is no `close()` to forget to call.
pub struct Viewer {
    frames: mpsc::Sender<Frame>,
    state: Arc<AtomicU8>,
    /// The candidates this side offered, kept for the tests and for saying
    /// what happened when a connection does not come up
    pub offered: Vec<String>,
}

impl Viewer {
    /// How it is getting on.
    pub fn state(&self) -> State {
        State::of(self.state.load(Ordering::Relaxed))
    }

    /// Hand over a compressed picture. Cheap: it goes onto a queue and the
    /// connection's own thread sends it.
    ///
    /// A frame handed over after the far end has gone is dropped, and saying
    /// so is not an error -- the picture was produced before anyone here knew,
    /// and there is nothing for the caller to do differently.
    pub fn send(&self, frame: Frame) {
        if self.frames.send(frame).is_err() {
            self.state.store(State::Gone.as_u8(), Ordering::Relaxed);
        }
    }
}

/// The addresses of this machine that are worth offering a phone.
///
/// Every address it actually has, minus the loopback -- the far end is a
/// different device, so an address that only means "here" is a path that
/// cannot work. Ordered with the private ones first, because those are the
/// ones that succeed: a phone on the same network or on the same VPN reaches
/// them directly, which is the whole of how this app is used.
///
/// **Nothing is asked of anybody to build this list.** The addresses come from
/// the machine itself. A STUN server would add the address the internet sees
/// us at, which is the one thing here nobody asked to publish.
pub fn addresses_here(all: &[IpAddr]) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = all
        .iter()
        .filter(|a| !a.is_loopback() && !a.is_unspecified())
        .copied()
        .collect();
    out.sort_by_key(|a| (!private(a), a.is_ipv6(), a.to_string()));
    out.dedup();
    out
}

/// Whether an address is one of the private ranges -- a home network, or a
/// VPN's own range. These are the ones a phone can actually reach.
fn private(a: &IpAddr) -> bool {
    match a {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_link_local()
                // 100.64.0.0/10, which is what Tailscale hands out
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        // A link-local or unique-local v6 address. `is_unique_local` is not
        // stable, so the range is spelled out
        IpAddr::V6(v6) => {
            let s = v6.segments();
            (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xfe00) == 0xfc00
        }
    }
}

/// Answer a viewer that has asked for video.
///
/// The page offers, because the page is the one that knows when it wants a
/// picture and what its browser can decode; this side answers. Returns the
/// answer to hand back, and the viewer to feed.
///
/// `addrs` is what this machine calls itself ([`addresses_here`]). Handing
/// them in rather than looking them up means a test can say what the machine
/// looks like, and means this module never has to ask the operating system
/// anything.
pub fn answer(offer: &str, addrs: &[IpAddr]) -> Result<(Viewer, String)> {
    use str0m::change::SdpOffer;
    use str0m::{Candidate, Rtc};

    if addrs.is_empty() {
        bail!("this machine has no address a phone could reach");
    }
    // One socket for the connection, on a port the system picks. Bound to
    // every interface because the candidates below name them individually
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let port = socket.local_addr()?.port();

    // Said per connection rather than installed for the whole process, which
    // the library asks libraries not to do. Windows has the cipher suite in
    // the operating system; everywhere else it is OpenSSL, built into the
    // binary. Left unsaid, the library reaches for OpenSSL on every platform
    // and dies at the first handshake on the one where it is not there
    let mut rtc = Rtc::builder().set_crypto_provider(crypto()).build();
    let mut offered = Vec::new();
    for ip in addrs {
        let at = SocketAddr::new(*ip, port);
        // A "host" candidate: an address this machine has, offered as it is.
        // The other two kinds -- what a STUN server says we look like, and a
        // relay to borrow -- are the ones that involve somebody else
        let Ok(c) = Candidate::host(at, "udp") else { continue };
        offered.push(c.to_string());
        rtc.add_local_candidate(c);
    }
    if offered.is_empty() {
        bail!("none of this machine's addresses could be offered");
    }

    let offer = SdpOffer::from_sdp_string(offer).map_err(|e| anyhow!("the offer is not one: {e}"))?;
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| anyhow!("the offer could not be answered: {e}"))?;
    let answer_sdp = answer.to_sdp_string();

    let (tx, rx) = mpsc::channel::<Frame>();
    let state = Arc::new(AtomicU8::new(State::Connecting.as_u8()));
    let mine = state.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(rtc, socket, rx, &mine) {
            crate::append_hook_log(&format!("the video relay stopped: {e:#}"));
        }
        mine.store(State::Gone.as_u8(), Ordering::Relaxed);
    });

    Ok((Viewer { frames: tx, state, offered }, answer_sdp))
}

/// The connection's own thread: everything the library wants to send goes out
/// of this socket, everything that arrives goes in, and the pictures queued by
/// [`Viewer::send`] are written between the two.
///
/// Written as a loop around a socket with a read deadline rather than around
/// an async runtime, because that is how the rest of this program talks to the
/// network, and one style of concurrency is enough for any program.
fn run(
    mut rtc: str0m::Rtc,
    socket: UdpSocket,
    frames: mpsc::Receiver<Frame>,
    state: &Arc<AtomicU8>,
) -> Result<()> {
    use str0m::net::{Protocol, Receive};
    use str0m::{Event, Input, Output};

    let began = Instant::now();
    let mut buf = vec![0u8; DATAGRAM];
    let here = socket.local_addr()?;
    // Which media line carries the picture, and which of the encodings both
    // sides kept. Not known yet: the library says so once the answer has been
    // negotiated, through an event below. Until then frames are dropped, which
    // is right -- there is nowhere to put them and a picture from before the
    // connection existed is not worth showing
    let mut carrying: Option<(str0m::media::Mid, str0m::media::Pt)> = None;

    loop {
        // Everything the library has to say, until it says "now wait"
        let wait = loop {
            match rtc.poll_output().map_err(|e| anyhow!("{e}"))? {
                Output::Timeout(at) => break at,
                Output::Transmit(t) => {
                    let _ = socket.send_to(&t.contents, t.destination);
                }
                Output::Event(e) => match e {
                    Event::IceConnectionStateChange(str0m::IceConnectionState::Connected) => {
                        state.store(State::Live.as_u8(), Ordering::Relaxed);
                    }
                    Event::IceConnectionStateChange(str0m::IceConnectionState::Disconnected) => {
                        return Ok(());
                    }
                    // The answer is settled: this is the line the picture goes
                    // out on, and the first payload type on it is the encoding
                    // both sides kept
                    Event::MediaAdded(added)
                        if added.kind == str0m::media::MediaKind::Video && carrying.is_none() =>
                    {
                        carrying = rtc
                            .writer(added.mid)
                            .and_then(|w| w.payload_params().next().map(|p| p.pt()))
                            .map(|pt| (added.mid, pt));
                        if carrying.is_none() {
                            bail!("the offer asked for no encoding this side can send");
                        }
                    }
                    _ => {}
                },
            }
        };

        // A connection that never comes up must not hold a thread for ever
        if State::of(state.load(Ordering::Relaxed)) == State::Connecting && began.elapsed() > SETTLE
        {
            return Ok(());
        }

        let now = Instant::now();
        let until = wait.max(now) - now;
        socket.set_read_timeout(Some(until.max(Duration::from_millis(1))))?;
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let at = Instant::now();
                if let Ok(r) = Receive::new(Protocol::Udp, from, here, &buf[..n]) {
                    rtc.handle_input(Input::Receive(at, r)).map_err(|e| anyhow!("{e}"))?;
                }
            }
            // The deadline passed, which is the ordinary case: nothing
            // arrived, and the library is owed the moment it asked for
            Err(e) if would_block(&e) => {
                rtc.handle_input(Input::Timeout(Instant::now())).map_err(|e| anyhow!("{e}"))?;
            }
            Err(e) => return Err(e.into()),
        }

        // Anything queued for the far end. Taken without waiting: the socket
        // above is what this loop waits on
        loop {
            match frames.try_recv() {
                Ok(frame) => {
                    if let Some((mid, pt)) = carrying {
                        send_frame(&mut rtc, mid, pt, began, frame);
                    }
                }
                // The viewer was dropped, which is how a caller says stop
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if !rtc.is_alive() {
            return Ok(());
        }
    }
}

/// Put one compressed picture on the wire.
///
/// The clock media is timed against runs at 90kHz, which is what every video
/// payload in WebRTC uses. Counted from when this connection began rather than
/// from when the frame was compressed, so that a slow encoder shows up as a
/// late frame instead of as a frame that claims to be from the future.
fn send_frame(rtc: &mut str0m::Rtc, mid: str0m::media::Mid, pt: str0m::media::Pt, began: Instant, frame: Frame) {
    use str0m::media::MediaTime;

    let Some(writer) = rtc.writer(mid) else { return };
    let ticks = frame.taken.saturating_duration_since(began).as_secs_f64() * 90_000.0;
    let at = MediaTime::from_90khz(ticks as u64);
    // The wallclock is when the picture was taken, which is what lines it up
    // against sound if sound is ever added beside it
    if let Err(e) = writer.write(pt, frame.taken, at, frame.data) {
        crate::append_hook_log(&format!("a frame could not be sent: {e}"));
    }
}

/// Whether a socket error is "nothing arrived in time", which is not a fault.
/// Windows and Unix name it differently.
fn would_block(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// Which cipher suite this build carries.
fn crypto() -> str0m::config::CryptoProvider {
    #[cfg(windows)]
    {
        str0m::config::CryptoProvider::WinCrypto
    }
    #[cfg(not(windows))]
    {
        str0m::config::CryptoProvider::OpenSsl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// The addresses offered to a phone are the ones a phone can reach, and
    /// the ones it is most likely to reach come first. Loopback is not one of
    /// them: the far end is a different device, so "here" is not a path.
    #[test]
    fn only_addresses_a_phone_could_reach_are_offered() {
        let all = vec![
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)), // a public one
            IpAddr::V4(Ipv4Addr::new(192, 168, 0, 99)), // the home network
            IpAddr::V4(Ipv4Addr::new(100, 115, 38, 97)), // what Tailscale hands out
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ];
        let out = addresses_here(&all);
        assert!(
            !out.contains(&IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
            "loopback was offered as a path to another device: {out:?}"
        );
        assert!(!out.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST)), "and its v6 twin: {out:?}");
        assert_eq!(out.len(), 3, "an address went missing: {out:?}");
        // The two that actually work come before the one that needs the
        // internet to cooperate
        let public_at = out.iter().position(|a| a.to_string() == "203.0.113.7").unwrap();
        for private in ["192.168.0.99", "100.115.38.97"] {
            let at = out.iter().position(|a| a.to_string() == private).unwrap();
            assert!(at < public_at, "{private} was offered after a public address: {out:?}");
        }
    }

    /// Tailscale's range is not one of the ordinary private ranges, and this
    /// app is used over Tailscale more than anything else. Getting it wrong
    /// would put the address that always works at the back of the queue.
    #[test]
    fn the_vpn_range_counts_as_one_a_phone_can_reach() {
        assert!(private(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))), "the bottom of the range");
        assert!(private(&IpAddr::V4(Ipv4Addr::new(100, 127, 255, 254))), "the top of it");
        assert!(!private(&IpAddr::V4(Ipv4Addr::new(100, 63, 0, 1))), "just below it");
        assert!(!private(&IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))), "just above it");
        // And the ordinary ones
        assert!(private(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(private(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(private(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!private(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    /// A machine with nothing to offer says so rather than opening a
    /// connection that can never come up.
    #[test]
    fn a_machine_with_no_address_refuses() {
        let said = answer("v=0\r\n", &[]).map(|_| ()).unwrap_err().to_string();
        assert!(said.contains("no address"), "it failed for some other reason: {said}");
    }

    /// Nonsense is refused as nonsense, and the message says which side is at
    /// fault -- the offer came from a page, and a page can be out of date.
    #[test]
    fn an_offer_that_is_not_one_is_refused() {
        let addrs = [IpAddr::V4(Ipv4Addr::new(192, 168, 0, 99))];
        let said = answer("not an offer at all", &addrs).map(|_| ()).unwrap_err().to_string();
        assert!(said.contains("the offer"), "the reason names the wrong side: {said}");
    }
}
