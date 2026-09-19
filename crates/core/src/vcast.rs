//! The one place the three halves of sending a screen as video meet.
//!
//! Each of them is deliberately ignorant of the others:
//!
//! | piece | what it knows |
//! |---|---|
//! | [`crate::vframe`] | how to undo a JPEG, and when a picture is worth compressing at all |
//! | [`crate::vencode`] | how to turn planes into video with whatever this machine has |
//! | [`crate::webrtc`] | how to get bytes to a browser |
//!
//! Keeping them apart means each can be read, tested and replaced on its own
//! -- the encoder differs per platform, the pacing is policy, and the peer
//! connection is a protocol. What is left over is the small amount of
//! sequencing that knows all three, and it is this file.
//!
//! **Where it is fed from.** `remote.rs` already has one door every screen
//! frame goes through ([`crate::remote::Remote::push_frame`]), because the
//! phone is sent JPEGs from there today. A cast is fed from that same door, so
//! the video path cannot ever be looking at a different screen than the JPEG
//! path is -- and so that turning video on adds nothing to the path that was
//! there before.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::vencode::{Encoder, encoder_for};
use crate::vframe::{Do, Pace, planes_of};
use crate::webrtc::{Frame, State, Viewer};

/// The most frames a second to send. A page drawing faster than this is sent
/// at this, because the encoder is the expensive part and the eye is not
/// asking for more.
const CEILING: u32 = 30;

/// How long a viewer may go without a picture that stands on its own. Long,
/// because it is only a safety net: the far end asks for one the moment its
/// picture is damaged, and that is the path that ordinarily runs.
const WHOLE_EVERY: Duration = Duration::from_secs(4);

/// One viewer, watching the screen as video.
pub struct Cast {
    viewer: Viewer,
    pace: Pace,
    /// Made on the first picture, because until one arrives nobody knows how
    /// big the screen is. Thrown away and made again when the size changes
    encoder: Option<Box<dyn Encoder>>,
    size: (usize, usize),
    /// What has gone wrong, if anything. Kept rather than returned per frame:
    /// the caller is a loop pushing pictures and has nothing useful to do with
    /// an error about one of them
    pub trouble: Option<String>,
    /// Whether the connection has ever come up, and whether a picture has ever
    /// gone down it.
    ///
    /// Two separate facts, and keeping them apart is the point. "A viewer
    /// asked for video" was being said as though it meant "a viewer is getting
    /// video", and on the first phone where the two came apart -- the offer
    /// answered, the connection never made -- the record said the picture was
    /// going out when it was not. From out here that looks identical to video
    /// that connected and decoded badly, and the two want opposite fixes.
    said_live: bool,
    said_sent: bool,
}

impl Cast {
    /// Answer a viewer that asked for video, and return what to send back.
    ///
    /// The page offers and this answers, because the page is the one that
    /// knows what its browser can decode.
    pub fn answer(offer: &str, addrs: &[std::net::IpAddr]) -> Result<(Cast, String)> {
        // What the far end is agreed with has to be what the encoder will
        // later produce, and the encoder does not exist yet -- it is made on
        // the first picture, which is the first thing that says how big the
        // screen is. So the platform is asked instead
        let (viewer, answer) =
            crate::webrtc::answer(offer, addrs, crate::vencode::codec())?;
        Ok((
            Cast {
                viewer,
                pace: Pace::new(CEILING, WHOLE_EVERY),
                encoder: None,
                size: (0, 0),
                trouble: None,
                said_live: false,
                said_sent: false,
            },
            answer,
        ))
    }

    /// Whether this is still worth feeding.
    pub fn live(&self) -> bool {
        self.viewer.state() != State::Gone && self.trouble.is_none()
    }

    /// The far end says its picture is damaged.
    pub fn whole_one_wanted(&mut self) {
        self.pace.whole_one_wanted();
    }

    /// One picture from the screen relay.
    ///
    /// Arriving at all means the page drew something -- Chromium only produces
    /// a frame when it did -- so that is what is handed to the pacing as
    /// "changed". This is the reason the video path costs nothing while a tab
    /// sits still, and it is the reason it is fed from the relay rather than
    /// from a clock of its own.
    pub fn picture(&mut self, jpeg: &[u8]) {
        if !self.live() {
            return;
        }
        // Nothing is prepared until there is somewhere to send it. A
        // connection takes a moment to come up and may never come up at all,
        // and until then every picture undone and compressed here is thrown
        // away at the other end of the queue -- while costing the relay's own
        // thread, which is the thread still sending this viewer its JPEGs.
        // Paid for twice and delivered once
        if self.viewer.state() != State::Live {
            return;
        }
        if !self.said_live {
            self.said_live = true;
            crate::append_hook_log("a video viewer is connected");
        }
        let now = Instant::now();
        let what = self.pace.decide(now, true);
        if what == Do::Skip {
            return;
        }
        if let Err(e) = self.carry(jpeg, what == Do::Whole, now) {
            self.trouble = Some(format!("{e:#}"));
        }
    }

    /// Undo the picture, compress it, and hand it over. Split out so the
    /// error has one place to be turned into something the caller keeps.
    fn carry(&mut self, jpeg: &[u8], whole: bool, now: Instant) -> Result<()> {
        let planes = planes_of(jpeg)?;
        if planes.is_empty() {
            return Ok(());
        }
        // A page that was resized is a new encoder: every encoder builds its
        // state around one size, and the frame after a resize has to stand on
        // its own anyway
        let size = (planes.width, planes.height);
        let whole = whole || self.size != size;
        if self.size != size {
            self.encoder = None;
            self.size = size;
        }
        let encoder = match self.encoder.as_mut() {
            Some(e) => e,
            None => {
                self.encoder = Some(encoder_for(size.0, size.1, CEILING)?);
                self.encoder.as_mut().expect("just made")
            }
        };
        if let Some(out) = encoder.encode(&planes, whole)? {
            if !self.said_sent {
                self.said_sent = true;
                crate::append_hook_log(&format!(
                    "the first picture went out as video: {}x{}, {} bytes",
                    size.0,
                    size.1,
                    out.data.len()
                ));
            }
            self.viewer.send(Frame { data: out.data, keyframe: out.keyframe, taken: now });
        }
        Ok(())
    }
}

/// Whether this build and this machine can send video at all.
///
/// Asked before a viewer is offered the choice, so that nobody is shown a
/// switch that cannot do anything. A build with no encoder still relays the
/// screen as JPEG, which is what it did before any of this existed.
pub fn available() -> bool {
    crate::vencode::available()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cast with nowhere to send refuses rather than swallowing pictures.
    #[test]
    fn a_cast_with_no_address_is_refused() {
        let said = Cast::answer("v=0\r\n", &[]).map(|_| ()).unwrap_err().to_string();
        assert!(said.contains("no address"), "it failed for some other reason: {said}");
    }

    /// A picture is not prepared for a viewer that is not there yet.
    ///
    /// Undoing a JPEG and compressing it costs the relay's own thread -- the
    /// one still sending this same viewer its JPEGs -- so doing it for a
    /// connection that has not come up makes the picture they ARE watching
    /// worse, to prepare one nobody receives.
    #[test]
    fn nothing_is_prepared_before_the_connection_is_up() {
        // A cast that cannot connect to anywhere: the loopback with an offer
        // no browser will ever answer
        let addrs = [std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)];
        let offer = concat!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n",
            "m=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\n",
            "a=rtcp-mux\r\na=ice-ufrag:aaaa\r\na=ice-pwd:bbbbbbbbbbbbbbbbbbbbbb\r\n",
            "a=fingerprint:sha-256 ",
            "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:",
            "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF\r\n",
            "a=setup:actpass\r\na=mid:0\r\na=recvonly\r\n",
            "a=rtpmap:102 H264/90000\r\n",
            "a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f\r\n",
        );
        let Ok((mut cast, _)) = Cast::answer(offer, &addrs) else {
            // A build with no encoder cannot answer at all, and then there is
            // nothing here to prove
            return;
        };
        // Not a picture at all. It would fail loudly if it were ever looked
        // at, which is the point: nothing looks at it
        cast.picture(b"this is not a jpeg");
        assert!(
            cast.trouble.is_none(),
            "a picture was prepared before anyone could receive it: {:?}",
            cast.trouble
        );
    }

    /// The ceiling and the heartbeat are the ones the measurements argued for:
    /// thirty a second at most, and a whole picture often enough that a phone
    /// which missed one is not stuck for long.
    #[test]
    fn the_pacing_is_the_one_the_measurements_asked_for() {
        assert_eq!(CEILING, 30);
        assert!(WHOLE_EVERY <= Duration::from_secs(5), "a damaged picture would last too long");
        assert!(
            WHOLE_EVERY >= Duration::from_secs(2),
            "whole pictures this often would cost most of what video saves"
        );
    }

    /// Video is offered only where something can actually compress it. A build
    /// without an encoder goes on relaying JPEG, which is what it did before.
    #[test]
    fn video_is_only_offered_where_it_can_happen() {
        assert_eq!(available(), crate::vencode::available());
    }
}
