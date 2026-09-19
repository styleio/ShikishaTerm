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
}

impl Cast {
    /// Answer a viewer that asked for video, and return what to send back.
    ///
    /// The page offers and this answers, because the page is the one that
    /// knows what its browser can decode.
    pub fn answer(offer: &str, addrs: &[std::net::IpAddr]) -> Result<(Cast, String)> {
        let (viewer, answer) = crate::webrtc::answer(offer, addrs)?;
        Ok((
            Cast {
                viewer,
                pace: Pace::new(CEILING, WHOLE_EVERY),
                encoder: None,
                size: (0, 0),
                trouble: None,
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
