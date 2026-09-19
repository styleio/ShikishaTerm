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

/// The addresses of this machine that are worth offering a viewer.
///
/// Every address it actually has, ordered so the ones most likely to work come
/// first: the private ones -- a home network, a VPN -- because a phone reaches
/// those directly and that is the whole of how this app is used, then anything
/// public, then the loopback.
///
/// **The loopback is offered too, last.** It was left out at first, on the
/// reasoning that the far end is a different device and "here" is not a path
/// to it. That is true of a phone and false of the other viewer this relay
/// has: a browser on this very machine, which is how the screen is looked at
/// without picking up a phone at all. Offering it costs nothing -- a phone
/// simply never succeeds on it and uses one of the others -- and leaving it
/// out cost that viewer the whole feature.
///
/// **Nothing is asked of anybody to build this list.** The addresses come from
/// the machine itself. A STUN server would add the address the internet sees
/// us at, which is the one thing here nobody asked to publish.
pub fn addresses_here(all: &[IpAddr]) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = all.iter().filter(|a| !a.is_unspecified()).copied().collect();
    if !out.iter().any(|a| a.is_loopback()) {
        out.push(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    }
    out.sort_by_key(|a| (a.is_loopback(), !private(a), a.is_ipv6(), a.to_string()));
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
///
/// `codec` is what will be fed in later -- `"H264"` or `"VP8"`. Said now,
/// before any of it exists, because it is what the far end is agreeing to.
pub fn answer(offer: &str, addrs: &[IpAddr], codec: &str) -> Result<(Viewer, String)> {
    use str0m::change::SdpOffer;
    use str0m::{Candidate, Rtc};

    if addrs.is_empty() {
        bail!("this machine has no address a phone could reach");
    }
    // **One socket per address, each bound to that address alone.**
    //
    // The obvious thing is one socket on 0.0.0.0 and several candidates
    // naming it. It does not work, and it fails silently: every packet has to
    // be handed to the library with the address it arrived AT, and a socket
    // bound to 0.0.0.0 cannot say which of this machine's addresses that was.
    // Handing over 0.0.0.0 matches none of the candidates offered, so every
    // connectivity check is quietly ignored and the connection sits at
    // "connecting" until it gives up. Nothing logs anything. An end-to-end
    // test against a real browser is the only thing that found it.
    let mut sockets: Vec<Arc<UdpSocket>> = Vec::new();
    for ip in addrs {
        match UdpSocket::bind(SocketAddr::new(*ip, 0)) {
            Ok(s) => sockets.push(Arc::new(s)),
            // An address the machine claims but will not bind is not a path.
            // Ordinary enough -- an interface going down between one call and
            // the next -- and the others still stand
            Err(_) => continue,
        }
    }
    if sockets.is_empty() {
        bail!("none of this machine's addresses could be listened on");
    }

    // Said per connection rather than installed for the whole process, which
    // the library asks libraries not to do. Windows has the cipher suite in
    // the operating system; everywhere else it is OpenSSL, built into the
    // binary. Left unsaid, the library reaches for OpenSSL on every platform
    // and dies at the first handshake on the one where it is not there
    // Only the encoding this machine can actually produce is agreed to. The
    // library offers VP8 and nothing else unless told otherwise, so a Windows
    // build -- which makes H.264 -- would agree to send VP8 and then send
    // H.264 down it. The far end shows a black rectangle and nothing anywhere
    // says why, which is the worst kind of wrong
    let mut rtc = Rtc::builder()
        .set_crypto_provider(crypto())
        .clear_codecs()
        .enable_h264(codec == "H264")
        .enable_vp8(codec == "VP8")
        .build(Instant::now());
    let mut offered = Vec::new();
    for s in &sockets {
        let at = s.local_addr()?;
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
        if let Err(e) = run(rtc, sockets, rx, &mine) {
            crate::append_hook_log(&format!("the video relay stopped: {e:#}"));
        }
        mine.store(State::Gone.as_u8(), Ordering::Relaxed);
    });

    Ok((Viewer { frames: tx, state, offered }, answer_sdp))
}

/// One datagram, as it arrived: what it says, who sent it, and -- the part
/// that matters -- which of this machine's addresses it arrived at.
struct Arrived {
    at: Instant,
    from: SocketAddr,
    here: SocketAddr,
    data: Vec<u8>,
}

/// The connection's own thread: everything the library wants to send goes out
/// of the socket it came from, everything that arrives goes in, and the
/// pictures queued by [`Viewer::send`] are written between the two.
///
/// Written as a loop around a channel rather than around an async runtime,
/// because that is how the rest of this program talks to the network, and one
/// style of concurrency is enough for any program. Each socket has a small
/// thread that does nothing but read it and post what it got -- which is also
/// what keeps the address a packet arrived at, the thing a socket bound to
/// every interface cannot tell you.
fn run(
    mut rtc: str0m::Rtc,
    sockets: Vec<Arc<UdpSocket>>,
    frames: mpsc::Receiver<Frame>,
    state: &Arc<AtomicU8>,
) -> Result<()> {
    use str0m::net::{Protocol, Receive};
    use str0m::{Event, Input, Output};

    let began = Instant::now();
    let (heard_tx, heard) = mpsc::channel::<Arrived>();
    for s in &sockets {
        let s = Arc::clone(s);
        let tx = heard_tx.clone();
        let here = s.local_addr()?;
        std::thread::spawn(move || {
            let mut buf = vec![0u8; DATAGRAM];
            loop {
                match s.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        let one =
                            Arrived { at: Instant::now(), from, here, data: buf[..n].to_vec() };
                        if tx.send(one).is_err() {
                            return;
                        }
                    }
                    // The loop below has finished and dropped the socket, or
                    // the interface went away. Either way there is nothing
                    // left to read
                    Err(_) => return,
                }
            }
        });
    }
    // Held only by the reading threads from here, so that when they are gone
    // the loop below is told rather than waiting on a channel nobody will
    // ever write to
    drop(heard_tx);
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
                    // Out of the socket the library says it is coming from.
                    // Sending from any other one would arrive with the wrong
                    // address on it and be ignored at the far end, which is
                    // the same mistake as the one this file's sockets exist
                    // to avoid, in the other direction
                    if let Some(s) = sockets.iter().find(|s| {
                        s.local_addr().is_ok_and(|a| a == t.source)
                    }) {
                        let _ = s.send_to(&t.contents, t.destination);
                    }
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
        let until = (wait.max(now) - now).max(Duration::from_millis(1));
        match heard.recv_timeout(until) {
            Ok(one) => {
                if let Ok(r) = Receive::new(Protocol::Udp, one.from, one.here, &one.data) {
                    rtc.handle_input(Input::Receive(one.at, r)).map_err(|e| anyhow!("{e}"))?;
                }
            }
            // Nothing arrived in time, which is the ordinary case: the
            // library is owed the moment it asked for
            Err(mpsc::RecvTimeoutError::Timeout) => {
                rtc.handle_input(Input::Timeout(Instant::now())).map_err(|e| anyhow!("{e}"))?;
            }
            // Every socket has stopped being read, so nothing can arrive again
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
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

/// The cipher suite this build carries, made once and shared.
///
/// Asked of the library rather than named here: which one is built in is a
/// question for the manifest, and saying it twice is how the two come to
/// disagree.
fn crypto() -> std::sync::Arc<str0m::crypto::CryptoProvider> {
    use std::sync::OnceLock;
    static ONE: OnceLock<std::sync::Arc<str0m::crypto::CryptoProvider>> = OnceLock::new();
    ONE.get_or_init(|| std::sync::Arc::new(str0m::crypto::from_feature_flags())).clone()
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
        // The two a phone reaches directly come first, then the one that needs
        // the internet to cooperate, and the loopback last of all -- it works
        // for exactly one viewer, the browser on this machine, and that viewer
        // has nothing else
        let at = |s: &str| out.iter().position(|a| a.to_string() == s);
        let public_at = at("203.0.113.7").expect("a public address went missing");
        for private in ["192.168.0.99", "100.115.38.97"] {
            let p = at(private).unwrap_or_else(|| panic!("{private} went missing: {out:?}"));
            assert!(p < public_at, "{private} was offered after a public address: {out:?}");
        }
        let loop_at = out
            .iter()
            .position(|a| a.is_loopback())
            .unwrap_or_else(|| panic!("the browser on this machine was left with nothing: {out:?}"));
        assert!(loop_at > public_at, "the loopback was preferred to a real path: {out:?}");
    }

    /// A machine with nothing but loopback -- no network at all -- can still
    /// show its own screen to its own browser.
    #[test]
    fn a_machine_with_no_network_can_still_show_itself() {
        let out = addresses_here(&[]);
        assert_eq!(out.len(), 1, "something other than the loopback appeared: {out:?}");
        assert!(out[0].is_loopback(), "{out:?}");
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
        let said = answer("v=0\r\n", &[], "VP8").map(|_| ()).unwrap_err().to_string();
        assert!(said.contains("no address"), "it failed for some other reason: {said}");
    }

    /// Nonsense is refused as nonsense, and the message says which side is at
    /// fault -- the offer came from a page, and a page can be out of date.
    #[test]
    fn an_offer_that_is_not_one_is_refused() {
        let addrs = [IpAddr::V4(Ipv4Addr::new(192, 168, 0, 99))];
        let said = answer("not an offer at all", &addrs, "VP8").map(|_| ()).unwrap_err().to_string();
        assert!(said.contains("the offer"), "the reason names the wrong side: {said}");
    }
}
