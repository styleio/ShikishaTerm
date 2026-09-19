//! A picture, on its way from the screen relay to a video encoder.
//!
//! The relay hands over JPEG, one per change (`cdp.rs` `CAST_PARAMS`). Every
//! video encoder wants the opposite: raw samples, brightness kept apart from
//! colour, colour at half the detail. So something has to undo the JPEG and
//! lay the samples out again, and this is it.
//!
//! It also decides **when that work is worth doing at all**, which turned out
//! to matter more than the conversion. Measured on a rented Linux machine, a
//! page that is not moving costs the present way of doing things almost
//! nothing -- one frame in six seconds, because Chromium only produces a frame
//! when something changes -- while taking the screen on a clock thirty times a
//! second cost most of a core and 40 KB/s to send a picture that never
//! changed. A video path driven by a clock would be a plain regression for the
//! way a browser tab is used nearly all of the time, which is: open, and still.
//!
//! So the rule here is that **a frame is only prepared when the picture
//! actually changed**, and the change is the relay's own signal -- the same one
//! that produces a JPEG today. Nothing is polled and nothing is timed.

/// A picture with its colour split the way encoders want it: one plane of
/// brightness at full detail, two of colour at half in each direction.
///
/// Named I420 everywhere in video, and it is what both H.264 and VP8 take.
pub struct Planes {
    pub width: usize,
    pub height: usize,
    /// Brightness, `width * height` of it
    pub y: Vec<u8>,
    /// Blue-difference, a quarter as many
    pub u: Vec<u8>,
    /// Red-difference, a quarter as many
    pub v: Vec<u8>,
}

impl Planes {
    /// How many bytes the three planes come to. What an encoder asks for when
    /// it wants one buffer rather than three.
    pub fn len(&self) -> usize {
        self.y.len() + self.u.len() + self.v.len()
    }

    /// Whether there is no picture here at all.
    pub fn is_empty(&self) -> bool {
        self.y.is_empty()
    }

    /// The three planes end to end, which is how encoders that take a single
    /// buffer expect to find them.
    pub fn joined(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        out.extend_from_slice(&self.y);
        out.extend_from_slice(&self.u);
        out.extend_from_slice(&self.v);
        out
    }
}

/// Undo a JPEG and lay its samples out for an encoder.
///
/// Sizes are rounded **down** to even numbers. Colour is carried at half the
/// detail in each direction, so an odd width has half a column of colour with
/// nowhere to go; every encoder refuses such a picture, and quietly losing the
/// last row is better than handing one over and being refused per frame.
pub fn planes_of(jpeg: &[u8]) -> anyhow::Result<Planes> {
    let options = zune_core::options::DecoderOptions::default()
        .jpeg_set_out_colorspace(zune_core::colorspace::ColorSpace::RGB);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(
        zune_core::bytestream::ZCursor::new(jpeg),
        options,
    );
    let rgb = d.decode().map_err(|e| anyhow::anyhow!("the picture could not be read: {e:?}"))?;
    let (w, h) = d.dimensions().ok_or_else(|| anyhow::anyhow!("the picture has no size"))?;
    Ok(from_rgb(&rgb, w, h))
}

/// The same, from samples already undone. Kept apart from the JPEG so the
/// conversion can be tested without a picture file, and so a source that hands
/// over raw samples can use it directly.
pub fn from_rgb(rgb: &[u8], width: usize, height: usize) -> Planes {
    // Down to even in both directions: see planes_of
    let w = width & !1;
    let h = height & !1;
    let (cw, ch) = (w / 2, h / 2);
    let mut y = vec![0u8; w * h];
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    if w == 0 || h == 0 {
        return Planes { width: w, height: h, y, u, v };
    }

    // BT.601, studio swing -- brightness 16..235, colour 16..240. The range a
    // decoder assumes when nothing says otherwise, and nothing here says
    // otherwise: the extra lines that would claim the full range are one more
    // thing for a phone's decoder to disagree with us about
    let at = |x: usize, yy: usize| {
        let i = (yy * width + x) * 3;
        (rgb[i] as i32, rgb[i + 1] as i32, rgb[i + 2] as i32)
    };
    for row in 0..h {
        for col in 0..w {
            let (r, g, b) = at(col, row);
            y[row * w + col] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
        }
    }
    // Colour is averaged over each 2x2 square rather than sampled from one
    // corner. Sampling picks up whichever pixel happens to be there, which on
    // text -- which is what a browser tab mostly is -- means the colour of a
    // single stroke standing for four pixels
    for row in 0..ch {
        for col in 0..cw {
            let mut rs = 0;
            let mut gs = 0;
            let mut bs = 0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (r, g, b) = at(col * 2 + dx, row * 2 + dy);
                    rs += r;
                    gs += g;
                    bs += b;
                }
            }
            let (r, g, b) = (rs / 4, gs / 4, bs / 4);
            u[row * cw + col] =
                (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            v[row * cw + col] =
                (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
    Planes { width: w, height: h, y, u, v }
}

/// When it is worth compressing a picture, and when it is not.
///
/// Three rules, in the order they are asked:
///
/// 1. **Nothing changed, nothing is sent.** The relay only produces a frame
///    when the page drew, so this is the ordinary state of a browser tab and
///    it must cost nothing. This is the rule the measurements made
///    non-negotiable
/// 2. **Not faster than the ceiling.** A page animating at 60 does not need to
///    be sent at 60: the eye and the line are both happier with 30, and the
///    encoder is the expensive part
/// 3. **A whole picture now and then.** A viewer that joins late, or one whose
///    picture was damaged, cannot start from a frame that only says what
///    changed. Asked for by the far end when it needs one, and otherwise sent
///    on a slow heartbeat so that a phone that missed one is never stuck for
///    long
pub struct Pace {
    /// The shortest gap between frames, from the ceiling
    least: std::time::Duration,
    /// How long a picture may go without a frame that stands on its own
    whole_every: std::time::Duration,
    sent_at: Option<std::time::Instant>,
    whole_at: Option<std::time::Instant>,
    /// Set when the far end says its picture is damaged
    asked: bool,
}

/// What to do with a picture the relay just produced.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Do {
    /// Compress it, and let it stand on its own
    Whole,
    /// Compress it as a difference from the one before
    Difference,
    /// Let it go. Either nothing changed or it came too soon after the last
    Skip,
}

impl Pace {
    /// `most` frames a second at the most; a whole picture at least every
    /// `whole_every`.
    pub fn new(most: u32, whole_every: std::time::Duration) -> Pace {
        Pace {
            least: std::time::Duration::from_secs_f64(1.0 / most.max(1) as f64),
            whole_every,
            sent_at: None,
            whole_at: None,
            asked: false,
        }
    }

    /// The far end says its picture is damaged and it needs one that stands on
    /// its own. Answered at the next frame rather than by making one now: the
    /// next frame is however long the page takes to draw, and a picture made
    /// out of turn would be of a screen nobody asked about.
    pub fn whole_one_wanted(&mut self) {
        self.asked = true;
    }

    /// What to do with the picture that has just arrived.
    ///
    /// `changed` is the relay's own answer -- a frame arriving at all means
    /// the page drew something. Passed in rather than worked out here by
    /// comparing pictures, which would cost more than the compressing does.
    pub fn decide(&mut self, now: std::time::Instant, changed: bool) -> Do {
        let overdue = self.whole_at.is_none_or(|at| now.duration_since(at) >= self.whole_every);
        // A damaged picture at the far end is worth answering even if nothing
        // has changed here -- what is on their screen is wrong either way
        if (self.asked || overdue) && (changed || self.asked) {
            self.asked = false;
            self.sent_at = Some(now);
            self.whole_at = Some(now);
            return Do::Whole;
        }
        if !changed {
            return Do::Skip;
        }
        // Not `>= least` exactly. A page drawing at 60 arrives every 16.667ms,
        // so every second frame is 33.333ms behind the last -- and a ceiling
        // of 30 asks for 33.333ms. On the wrong side of the last decimal that
        // frame is refused, the one after is 50ms late, and a ceiling of 30
        // quietly delivers 20. The slack is a tenth of the gap, which lets the
        // frame that is a hair early through and can overshoot the ceiling by
        // that tenth and no more
        let soonest = self.least.mul_f64(0.9);
        if self.sent_at.is_some_and(|at| now.duration_since(at) < soonest) {
            return Do::Skip;
        }
        self.sent_at = Some(now);
        Do::Difference
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// White, black and a colour, each turned into the numbers a decoder will
    /// turn back into the same thing. The middle of the range is where a sign
    /// error hides, so grey is checked too.
    #[test]
    fn the_colours_survive_the_journey() {
        let px = |r: u8, g: u8, b: u8| {
            let rgb: Vec<u8> = std::iter::repeat_n([r, g, b], 4).flatten().collect();
            from_rgb(&rgb, 2, 2)
        };
        // Studio swing: white is 235, black is 16, and neither is 0 or 255
        let white = px(255, 255, 255);
        assert_eq!(white.y[0], 235, "white came out at the wrong brightness");
        let black = px(0, 0, 0);
        assert_eq!(black.y[0], 16, "black came out at the wrong brightness");
        // Grey has no colour in it: both colour planes sit at the middle
        let grey = px(128, 128, 128);
        assert!((grey.u[0] as i32 - 128).abs() <= 1, "grey has blue in it: {}", grey.u[0]);
        assert!((grey.v[0] as i32 - 128).abs() <= 1, "grey has red in it: {}", grey.v[0]);
        // Blue is blue: the blue-difference plane goes far up, and the
        // red-difference one down. Not symmetrically -- blue weighs much more
        // in the blue-difference than it does in the red-difference, so pure
        // blue sits at about 239 and 110, not at 239 and 16
        let blue = px(0, 0, 255);
        assert!(blue.u[0] > 230, "blue is not blue: {}", blue.u[0]);
        assert!(blue.v[0] < 120, "blue is not short of red: {}", blue.v[0]);
        let red = px(255, 0, 0);
        assert!(red.v[0] > 230, "red is not red: {}", red.v[0]);
        assert!(red.u[0] < 100, "red is not short of blue: {}", red.u[0]);
    }

    /// The planes are the sizes an encoder demands, and an odd picture is cut
    /// rather than handed over to be refused.
    #[test]
    fn the_planes_are_the_sizes_an_encoder_takes() {
        let rgb = vec![128u8; 7 * 5 * 3];
        let p = from_rgb(&rgb, 7, 5);
        assert_eq!((p.width, p.height), (6, 4), "an odd size was not cut back");
        assert_eq!(p.y.len(), 6 * 4);
        assert_eq!(p.u.len(), 3 * 2, "colour is not at half the detail");
        assert_eq!(p.v.len(), 3 * 2);
        assert_eq!(p.joined().len(), p.len());
        // Nothing at all is not a crash
        assert!(from_rgb(&[], 0, 0).is_empty());
        assert!(from_rgb(&[1, 2, 3], 1, 1).is_empty(), "a single pixel has no even size");
    }

    /// Colour is averaged over each square rather than picked from a corner.
    /// Sampling a corner puts the colour of one stroke of text on four pixels.
    #[test]
    fn colour_is_averaged_not_sampled() {
        // Three white pixels and one red, in one square
        let rgb = vec![
            255, 255, 255, 255, 0, 0, //
            255, 255, 255, 255, 255, 255,
        ];
        let p = from_rgb(&rgb, 2, 2);
        let averaged = from_rgb(&[191, 191, 191, 191, 191, 191, 191, 191, 191, 191, 191, 191], 2, 2);
        assert!(
            (p.v[0] as i32 - 128).abs() > 5,
            "the red pixel left no trace: {}",
            p.v[0]
        );
        assert!(
            p.v[0] < 160,
            "the red pixel was taken for the whole square: {} (all-grey is {})",
            p.v[0],
            averaged.v[0]
        );
    }

    /// The rule the measurements made non-negotiable: a page that is not
    /// changing costs nothing at all.
    #[test]
    fn a_still_page_is_never_compressed() {
        let mut p = Pace::new(30, Duration::from_secs(2));
        let t = Instant::now();
        // The first change is a whole picture -- nothing has been sent yet
        assert_eq!(p.decide(t, true), Do::Whole);
        // And then nothing happens for a long time
        for ms in [10, 100, 500, 1500] {
            assert_eq!(
                p.decide(t + Duration::from_millis(ms), false),
                Do::Skip,
                "a still page was compressed at {ms}ms"
            );
        }
    }

    /// A page animating faster than the ceiling is sent at the ceiling.
    #[test]
    fn nothing_is_sent_faster_than_the_ceiling() {
        let mut p = Pace::new(30, Duration::from_secs(10));
        let t = Instant::now();
        assert_eq!(p.decide(t, true), Do::Whole);
        // 60 a second arriving; 30 a second is the most that may go out
        let mut sent = 0;
        for i in 1..=60 {
            if p.decide(t + Duration::from_millis(i * 1000 / 60), true) != Do::Skip {
                sent += 1;
            }
        }
        assert!((28..=31).contains(&sent), "{sent} frames went out in a second, not about 30");
    }

    /// A viewer whose picture is damaged gets a whole one at the next frame,
    /// and gets it even if the page has stopped changing -- what is on their
    /// screen is wrong either way.
    #[test]
    fn a_damaged_picture_is_answered_even_on_a_still_page() {
        let mut p = Pace::new(30, Duration::from_secs(60));
        let t = Instant::now();
        assert_eq!(p.decide(t, true), Do::Whole);
        assert_eq!(p.decide(t + Duration::from_millis(50), true), Do::Difference);
        p.whole_one_wanted();
        assert_eq!(
            p.decide(t + Duration::from_millis(60), false),
            Do::Whole,
            "a damaged picture went unanswered because the page was still"
        );
        // And only once
        assert_eq!(p.decide(t + Duration::from_millis(200), true), Do::Difference);
    }

    /// A whole picture now and then, so a phone that missed one is not stuck
    /// with a broken screen until something else goes wrong.
    #[test]
    fn a_whole_picture_comes_round_on_its_own() {
        let mut p = Pace::new(30, Duration::from_secs(2));
        let t = Instant::now();
        assert_eq!(p.decide(t, true), Do::Whole);
        assert_eq!(p.decide(t + Duration::from_millis(100), true), Do::Difference);
        assert_eq!(
            p.decide(t + Duration::from_millis(2100), true),
            Do::Whole,
            "the heartbeat never came round"
        );
    }
}
