//! Compressing a screen into video.
//!
//! The picture arrives as planes ([`crate::vframe::Planes`]) and leaves as a
//! few kilobytes that only say what changed since the last one. That
//! difference is the whole point: the present way of doing things compresses
//! every frame from nothing, which on a rented Linux machine cost three times
//! the CPU and nine times the bytes of the same picture as video.
//!
//! **Which encoder, and why it is not one choice.** WebRTC settles on an
//! encoding per connection, so there is no need to pick one for the whole
//! program -- and picking one would mean taking on somebody's terms for no
//! gain:
//!
//! | where | what | why |
//! |---|---|---|
//! | Windows | H.264, through Media Foundation | the operating system has the encoder and the licence for it. Nothing extra is shipped |
//! | elsewhere | VP8 | BSD, carries no conditions, and can simply be built in. Measured at about twice the CPU of H.264 on a small machine, which is 0.56 of a core against 0.29 -- both far under one |
//!
//! The Cisco-provided H.264 binary is a third road, for somebody on Linux who
//! would rather spend the conditions than the CPU. Its terms are in
//! `.private/doc/webrtc-relay-plan.ja.md` §8 and they are not free: the binary
//! must be fetched at the moment it is turned on and never shipped, the person
//! must be able to turn it off again, and a line naming Cisco must stand where
//! that switch is. Off by default, so nobody who never asks is ever subject to
//! any of it.

use anyhow::Result;

use crate::vframe::Planes;

/// A compressed picture, ready to be sent.
#[derive(Debug)]
pub struct Encoded {
    pub data: Vec<u8>,
    /// Whether it stands on its own. A viewer joining late can only start here
    pub keyframe: bool,
}

/// Something that turns planes into video.
///
/// One per connection, because an encoder carries the picture before as its
/// state: two viewers sharing one would each be sent differences from frames
/// the other had seen.
pub trait Encoder: Send {
    /// Compress one picture. `whole` asks for a frame that stands on its own,
    /// which is what [`crate::vframe::Do::Whole`] means.
    ///
    /// `None` is not a failure: some encoders hold a frame back and answer on
    /// the next one.
    fn encode(&mut self, planes: &Planes, whole: bool) -> Result<Option<Encoded>>;

    /// What the far end has to be told this is, in the words an SDP uses.
    fn codec(&self) -> &'static str;
}

/// An encoder for a picture of this size, or why there is none.
///
/// The size cannot change afterwards: every encoder builds its state around
/// it. A page that is resized gets a new encoder, which is also when the next
/// frame has to stand on its own anyway.
pub fn encoder_for(width: usize, height: usize, fps: u32) -> Result<Box<dyn Encoder>> {
    #[cfg(windows)]
    {
        windows_h264::open(width, height, fps).map(|e| Box::new(e) as Box<dyn Encoder>)
    }
    #[cfg(all(not(windows), feature = "vp8"))]
    {
        vp8::open(width, height, fps).map(|e| Box::new(e) as Box<dyn Encoder>)
    }
    #[cfg(all(not(windows), not(feature = "vp8")))]
    {
        let _ = (width, height, fps);
        anyhow::bail!(
            "this build cannot compress video: it was built without the VP8 encoder"
        )
    }
}

/// Whether this build can compress video at all. For asking before offering
/// somebody something that cannot happen.
pub fn available() -> bool {
    cfg!(windows) || cfg!(feature = "vp8")
}

/// What this machine will produce, in the word an SDP uses.
///
/// Asked before an encoder exists, because the far end has to be told what to
/// expect while the connection is being agreed -- and the encoder is not made
/// until the first picture arrives and says how big the screen is. A
/// connection agreed on one encoding and fed another is a black rectangle
/// with no error anywhere.
pub fn codec() -> &'static str {
    match cfg!(windows) {
        true => "H264",
        false => "VP8",
    }
}

/// VP8, through the library that defines it.
///
/// Not the operating system's, because no operating system has one: Windows
/// carries an H.264 encoder and its licence, and everywhere else the answer
/// is a library. libvpx is BSD and takes on no conditions at all -- nothing
/// to display, nothing to fetch at runtime, nothing that stops working
/// offline -- which is why it is the default here rather than H.264 through
/// Cisco's binary. The cost is about twice the processor time, measured, and
/// still well under one core for a screen at thirty frames a second.
///
/// The binding is [`crate::vpx`], generated from the headers once and kept in
/// the repository so that building this needs nothing but a linker and the
/// library itself.
#[cfg(all(not(windows), feature = "vp8"))]
mod vp8 {
    use super::*;
    use crate::vpx;
    use anyhow::bail;

    /// What to aim at, in bits a second. The same figure the Windows side
    /// uses, so a comparison between them is about the encoders and not about
    /// what they were asked for.
    const BITS: u32 = 2_000;

    pub struct Vp8 {
        ctx: Box<vpx::vpx_codec_ctx_t>,
        img: Box<vpx::vpx_image_t>,
        /// One buffer holding Y, then U, then V, because that is what an
        /// image is handed over as. Kept between frames so a picture does not
        /// mean an allocation
        planes: Vec<u8>,
        width: usize,
        height: usize,
        /// Counted in frames rather than taken from a clock: the library
        /// wants a number that only goes up, and the pacing above already
        /// decided which pictures are worth sending
        at: i64,
    }

    // The encoder is used from the one thread that owns it
    unsafe impl Send for Vp8 {}

    pub fn open(width: usize, height: usize, fps: u32) -> Result<Vp8> {
        if width == 0 || height == 0 {
            bail!("a picture with no size cannot be compressed");
        }
        unsafe {
            let iface = vpx::vpx_codec_vp8_cx();
            if iface.is_null() {
                bail!("this system's libvpx has no VP8 encoder in it");
            }
            let mut cfg: vpx::vpx_codec_enc_cfg_t = std::mem::zeroed();
            let how = vpx::vpx_codec_enc_config_default(iface, &mut cfg, 0);
            if how != vpx::vpx_codec_err_t_VPX_CODEC_OK {
                bail!("the encoder would not say how it works: {}", said(how));
            }

            cfg.g_w = width as u32;
            cfg.g_h = height as u32;
            cfg.g_timebase = vpx::vpx_rational { num: 1, den: fps.max(1) as i32 };
            cfg.rc_target_bitrate = BITS;
            // A screen, not a film: the picture has to go out as it is made.
            // Constant bitrate and no lag, or the encoder holds frames back
            // to look at what follows them -- the same mistake the Windows
            // side made, where it cost seventeen seconds of nothing
            cfg.rc_end_usage = vpx::vpx_rc_mode_VPX_CBR;
            cfg.g_lag_in_frames = 0;
            cfg.g_error_resilient = 0;
            // Whole pictures come from the pacing above, which knows when one
            // is worth sending; the library is told not to decide on its own
            cfg.kf_mode = vpx::vpx_kf_mode_VPX_KF_DISABLED;
            cfg.g_threads = 2;
            // Dropping frames is the pacing's business too: a frame this side
            // decided to send is one somebody is waiting for
            cfg.rc_dropframe_thresh = 0;

            let mut ctx: Box<vpx::vpx_codec_ctx_t> = Box::new(std::mem::zeroed());
            let how = vpx::vpx_codec_enc_init_ver(
                &mut *ctx,
                iface,
                &cfg,
                0,
                vpx::VPX_ENCODER_ABI_VERSION as i32,
            );
            if how != vpx::vpx_codec_err_t_VPX_CODEC_OK {
                // The one failure worth naming: this system's libvpx is not
                // the one the binding was made from, and it says so rather
                // than reading the wrong bytes
                if how == vpx::vpx_codec_err_t_VPX_CODEC_ABI_MISMATCH {
                    bail!(
                        "this system's libvpx is a different version than this build was made for"
                    );
                }
                bail!("the encoder would not start: {}", said(how));
            }

            // Speed over quality, which for a screen is not a compromise: a
            // page of text at a lower setting is still a page of text, and a
            // page of text that arrives late is nothing
            let _ = vpx::vpx_codec_control_(&mut *ctx, VP8E_SET_CPUUSED, 12i32);

            let size = width * height * 3 / 2;
            let mut me = Vp8 {
                ctx,
                img: Box::new(std::mem::zeroed()),
                planes: vec![0u8; size],
                width,
                height,
                at: 0,
            };
            let wrapped = vpx::vpx_img_wrap(
                &mut *me.img,
                vpx::vpx_img_fmt_VPX_IMG_FMT_I420,
                width as u32,
                height as u32,
                1,
                me.planes.as_mut_ptr(),
            );
            if wrapped.is_null() {
                bail!("the picture could not be described to the encoder");
            }
            Ok(me)
        }
    }

    /// What the library calls a failure, in its own words.
    fn said(how: vpx::vpx_codec_err_t) -> String {
        unsafe {
            let p = vpx::vpx_codec_err_to_string(how);
            if p.is_null() {
                return format!("error {how}");
            }
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }

    /// Speed against quality, 0 (best) to 16 (fastest) for VP8. Named here
    /// rather than taken from the binding because it is a control id, and the
    /// control ids are macros the generator does not carry over.
    const VP8E_SET_CPUUSED: i32 = 13;

    impl Drop for Vp8 {
        fn drop(&mut self) {
            unsafe {
                vpx::vpx_codec_destroy(&mut *self.ctx);
            }
        }
    }

    impl Encoder for Vp8 {
        fn codec(&self) -> &'static str {
            "VP8"
        }

        fn encode(&mut self, planes: &Planes, whole: bool) -> Result<Option<Encoded>> {
            if planes.width != self.width || planes.height != self.height {
                bail!("the picture changed size under the encoder");
            }
            // Into one buffer, in the order an image is read in
            let y = self.width * self.height;
            let c = (self.width / 2) * (self.height / 2);
            if planes.y.len() < y || planes.u.len() < c || planes.v.len() < c {
                bail!("the picture is smaller than it says it is");
            }
            self.planes[..y].copy_from_slice(&planes.y[..y]);
            self.planes[y..y + c].copy_from_slice(&planes.u[..c]);
            self.planes[y + c..y + 2 * c].copy_from_slice(&planes.v[..c]);

            unsafe {
                let flags: i64 = if whole { vpx::VPX_EFLAG_FORCE_KF as i64 } else { 0 };
                let how = vpx::vpx_codec_encode(
                    &mut *self.ctx,
                    &*self.img,
                    self.at,
                    1,
                    flags,
                    vpx::VPX_DL_REALTIME as u64,
                );
                self.at += 1;
                if how != vpx::vpx_codec_err_t_VPX_CODEC_OK {
                    bail!("the picture would not compress: {}", said(how));
                }
                let mut iter: vpx::vpx_codec_iter_t = std::ptr::null();
                loop {
                    let pkt = vpx::vpx_codec_get_cx_data(&mut *self.ctx, &mut iter);
                    if pkt.is_null() {
                        return Ok(None);
                    }
                    if (*pkt).kind != vpx::vpx_codec_cx_pkt_kind_VPX_CODEC_CX_FRAME_PKT {
                        continue;
                    }
                    let frame = (*pkt).data.frame;
                    if frame.buf.is_null() || frame.sz == 0 {
                        continue;
                    }
                    let data =
                        std::slice::from_raw_parts(frame.buf as *const u8, frame.sz).to_vec();
                    let keyframe = frame.flags & vpx::VPX_FRAME_IS_KEY != 0;
                    return Ok(Some(Encoded { data, keyframe }));
                }
            }
        }
    }
}

/// H.264 through the encoder Windows already has.
///
/// The operating system carries both the encoder and the licence that covers
/// it, so using it adds nothing to what is shipped and takes on no terms of
/// our own. The transform asked for here is the software one: it is
/// synchronous, which is a great deal simpler to drive correctly than the
/// hardware transforms, and it was measured at a third of a core for 720p at
/// 30 -- room enough that the graphics card can wait for a later day.
#[cfg(windows)]
mod windows_h264 {
    use super::{Encoded, Encoder};
    use crate::vframe::Planes;
    use anyhow::{Result, anyhow, bail};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::*;
    use windows::core::Interface;

    /// How many bits a second to aim at. 2 Mbit is generous for a page of
    /// text at 720p and still a ninth of what the JPEGs were measured at.
    const BITS: u32 = 2_000_000;

    /// Media Foundation counts time in ten-millionths of a second.
    const TICKS: i64 = 10_000_000;

    pub struct H264 {
        mft: IMFTransform,
        width: usize,
        height: usize,
        /// Ten-millionths of a second per frame, for the timestamps the
        /// encoder wants on the way in
        step: i64,
        at: i64,
        /// Scratch, so a frame does not mean an allocation
        nv12: Vec<u8>,
    }

    // The transform is used from the one thread that owns this, and Media
    // Foundation's software transforms are free-threaded
    unsafe impl Send for H264 {}

    pub fn open(width: usize, height: usize, fps: u32) -> Result<H264> {
        if width == 0 || height == 0 {
            bail!("a picture with no size cannot be compressed");
        }
        unsafe {
            // Both are safe to call more than once; each keeps its own count
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
                .map_err(|e| anyhow!("the video system would not start: {e}"))?;

            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| anyhow!("this Windows has no H.264 encoder: {e}"))?;

            // Before the media types, and this is the difference between a
            // relay and a recording.
            //
            // Left alone this encoder works the way one compressing a film
            // does: it holds pictures back, looks at what follows them, and
            // lets the first out only when it has enough -- seventeen of them
            // on this machine. On a page drawing sixty times a second that is
            // a moment; on a page that changes once a second, which is most
            // pages, it is seventeen seconds of nothing, and the far end has
            // long since given up and gone back to JPEG. It is also exactly
            // what "choppy at first and then suddenly smooth" was.
            //
            // Said before the types are set because that is when an encoder
            // decides how it is going to work. Refusals are not failures: an
            // encoder that will not take these still produces video, later
            low_latency(&mft);

            // The output type goes first. An encoder will not say what it can
            // take until it has been told what it is to produce
            let out = MFCreateMediaType().map_err(|e| anyhow!("{e}"))?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, BITS)?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            // Constrained baseline: the profile every phone decodes in
            // hardware. A better profile saves bytes on a machine that can
            // use it and costs the whole picture on one that cannot
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)?;
            set_size(&out, &MF_MT_FRAME_SIZE, width as u32, height as u32)?;
            set_size(&out, &MF_MT_FRAME_RATE, fps.max(1), 1)?;
            set_size(&out, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            mft.SetOutputType(0, &out, 0)
                .map_err(|e| anyhow!("the encoder refused to produce H.264: {e}"))?;

            // And then what it will be fed. NV12 rather than the three planes
            // the rest of this program passes around: it is what this encoder
            // takes, and the interleaving is two lines of work
            let inp = MFCreateMediaType().map_err(|e| anyhow!("{e}"))?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            set_size(&inp, &MF_MT_FRAME_SIZE, width as u32, height as u32)?;
            set_size(&inp, &MF_MT_FRAME_RATE, fps.max(1), 1)?;
            set_size(&inp, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
            mft.SetInputType(0, &inp, 0)
                .map_err(|e| anyhow!("the encoder refused the picture's shape: {e}"))?;

            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            Ok(H264 {
                mft,
                width,
                height,
                step: TICKS / fps.max(1) as i64,
                at: 0,
                nv12: Vec::with_capacity(width * height * 3 / 2),
            })
        }
    }

    /// Two numbers packed into one attribute, which is how Media Foundation
    /// carries a size and a rate.
    unsafe fn set_size(
        t: &IMFMediaType,
        key: &windows::core::GUID,
        hi: u32,
        lo: u32,
    ) -> Result<()> {
        unsafe { t.SetUINT64(key, ((hi as u64) << 32) | lo as u64)? };
        Ok(())
    }

    impl Encoder for H264 {
        fn codec(&self) -> &'static str {
            "H264"
        }

        fn encode(&mut self, planes: &Planes, whole: bool) -> Result<Option<Encoded>> {
            if planes.width != self.width || planes.height != self.height {
                bail!(
                    "the picture changed size under the encoder: {}x{} became {}x{}",
                    self.width,
                    self.height,
                    planes.width,
                    planes.height
                );
            }
            nv12_into(planes, &mut self.nv12);
            unsafe {
                if whole {
                    force_keyframe(&self.mft);
                }
                let buf = MFCreateMemoryBuffer(self.nv12.len() as u32)?;
                {
                    let mut at: *mut u8 = std::ptr::null_mut();
                    buf.Lock(&mut at, None, None)?;
                    std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), at, self.nv12.len());
                    buf.Unlock()?;
                }
                buf.SetCurrentLength(self.nv12.len() as u32)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buf)?;
                sample.SetSampleTime(self.at)?;
                sample.SetSampleDuration(self.step)?;
                self.at += self.step;

                self.mft.ProcessInput(0, &sample, 0)?;
                take_output(&self.mft)
            }
        }
    }

    /// Hand every picture over as soon as it is compressed.
    ///
    /// Two knobs, because encoders disagree about which they honour: low
    /// latency mode, and no B-frames. A B-frame is compressed against the
    /// picture AFTER it, so the encoder cannot emit either until both exist --
    /// which on a screen that changes slowly means waiting for something to
    /// happen before the last thing that happened can be sent.
    unsafe fn low_latency(mft: &IMFTransform) {
        use windows::Win32::System::Variant::{
            VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4,
        };
        unsafe {
            // The transform's own attribute. Some encoders read only this one
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let Ok(api) = mft.cast::<ICodecAPI>() else { return };
            let yes = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_BOOL,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        // -1 is true in this shape, the way it has been since
                        // long before any of this
                        Anonymous: VARIANT_0_0_0 { boolVal: windows::Win32::Foundation::VARIANT_TRUE },
                    }),
                },
            };
            let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &yes);
            let none = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_UI4,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: VARIANT_0_0_0 { ulVal: 0 },
                    }),
                },
            };
            let _ = api.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &none);
        }
    }

    /// Ask for the next frame to stand on its own.
    ///
    /// Said through the codec's own knobs rather than on the sample, because
    /// that is the only place H.264 encoders listen for it. Refused by some
    /// encoders, and a refusal is not worth failing over: what comes out is
    /// still a frame, and the heartbeat in `vframe::Pace` will ask again
    /// before long.
    unsafe fn force_keyframe(mft: &IMFTransform) {
        use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_UI4};
        unsafe {
            let Ok(api) = mft.cast::<ICodecAPI>() else { return };
            // Built by hand: the crate has no shorthand for a plain unsigned
            // one, and this is the shape the interface reads
            let one = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_UI4,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: VARIANT_0_0_0 { ulVal: 1 },
                    }),
                },
            };
            let _ = api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &one);
        }
    }

    /// Whatever the encoder has ready. `None` when it is still holding on to
    /// what it has been given, which is ordinary for the first frames.
    unsafe fn take_output(mft: &IMFTransform) -> Result<Option<Encoded>> {
        unsafe {
            let info = mft.GetOutputStreamInfo(0)?;
            // Some transforms hand over their own buffer; this one wants ours
            let provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32
                != 0;
            let sample = match provides {
                true => None,
                false => {
                    let buf = MFCreateMemoryBuffer(info.cbSize.max(1))?;
                    let s = MFCreateSample()?;
                    s.AddBuffer(&buf)?;
                    Some(s)
                }
            };
            let mut out = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            match mft.ProcessOutput(0, &mut out, &mut status) {
                Ok(()) => {}
                // It wants another picture before it will say anything
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) => return Err(anyhow!("the encoder stopped: {e}")),
            }
            let Some(sample) = out[0].pSample.take() else { return Ok(None) };
            let buf = sample.ConvertToContiguousBuffer()?;
            let mut at: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut at, None, Some(&mut len))?;
            let data = std::slice::from_raw_parts(at, len as usize).to_vec();
            buf.Unlock()?;
            // A frame that stands on its own says so in its own attributes
            let keyframe = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) == 1;
            Ok(Some(Encoded { data, keyframe }))
        }
    }

    /// The three planes, laid out the way this encoder reads them: brightness
    /// as it is, then the two colour planes woven together a byte at a time.
    pub(super) fn nv12_into(p: &Planes, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(p.y.len() + p.u.len() + p.v.len());
        out.extend_from_slice(&p.y);
        for (u, v) in p.u.iter().zip(p.v.iter()) {
            out.push(*u);
            out.push(*v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vframe::from_rgb;

    /// The first picture comes out of the encoder at once, not once the next
    /// several have been fed in.
    ///
    /// Left to itself this encoder works the way one compressing a film does:
    /// it holds pictures back, looks at what follows them, and lets the first
    /// out only when it has enough. On a page drawing sixty times a second
    /// that is a moment. On a page that changes once a second -- a terminal, a
    /// form, almost anything being worked in -- it was twenty seconds before
    /// the first picture went out, by which time the far end had given up and
    /// gone back to JPEG.
    ///
    /// Counted in pictures, not seconds, because pictures are what it waits
    /// for: the seconds depend on how fast the page changes.
    #[test]
    fn the_first_picture_comes_out_without_waiting_for_the_next_ones() {
        let (w, h) = (320usize, 240usize);
        let Ok(mut enc) = encoder_for(w, h, 30) else {
            // A machine with no encoder has nothing to say here
            return;
        };
        let mut held = None;
        for i in 0..40usize {
            if enc.encode(&moving(i, w, h), i == 0).ok().flatten().is_some() {
                held = Some(i);
                break;
            }
        }
        let at = held.expect("the encoder never produced a picture at all");
        assert!(
            at <= 2,
            "the encoder held {} pictures back before letting the first one out; \
             on a screen that changes once a second that is {} seconds of nothing",
            at + 1,
            at + 1
        );
    }

    /// A picture the size of a real one, with something in it that compresses
    /// differently from frame to frame.
    fn moving(n: usize, w: usize, h: usize) -> crate::vframe::Planes {
        let mut rgb = vec![20u8; w * h * 3];
        // A bar that moves, so each frame differs from the one before
        let x0 = (n * 17) % (w - 40);
        for row in 0..h {
            for col in x0..x0 + 40 {
                let i = (row * w + col) * 3;
                rgb[i] = 220;
                rgb[i + 1] = 40;
                rgb[i + 2] = 40;
            }
        }
        from_rgb(&rgb, w, h)
    }

    /// The colour planes are woven together the way the encoder reads them,
    /// and nothing is lost on the way.
    #[cfg(windows)]
    #[test]
    fn the_colour_planes_are_woven_for_the_encoder() {
        let p = from_rgb(&[255, 0, 0, 0, 0, 255, 0, 255, 0, 255, 255, 255], 2, 2);
        let mut out = Vec::new();
        super::windows_h264::nv12_into(&p, &mut out);
        assert_eq!(out.len(), p.y.len() + p.u.len() + p.v.len(), "the picture changed size");
        assert_eq!(&out[..p.y.len()], &p.y[..], "the brightness was disturbed");
        assert_eq!(out[p.y.len()], p.u[0], "blue-difference is not first");
        assert_eq!(out[p.y.len() + 1], p.v[0], "red-difference does not follow it");
    }

    /// The whole way through, against the encoder this machine actually has:
    /// planes in, H.264 out, and the first thing out stands on its own.
    ///
    /// Run against the real Media Foundation rather than a stand-in, because
    /// what is being asked is whether *this* encoder accepts what we build --
    /// a stand-in would only prove we agree with ourselves.
    #[cfg(windows)]
    #[test]
    fn a_moving_picture_comes_out_as_h264() {
        let (w, h) = (320, 240);
        let mut enc = match encoder_for(w, h, 30) {
            Ok(e) => e,
            Err(e) => panic!("no encoder on a Windows machine: {e:#}"),
        };
        assert_eq!(enc.codec(), "H264");
        let mut frames = 0;
        let mut first: Option<Encoded> = None;
        // Encoders hold the first pictures back before they say anything
        for n in 0..30 {
            if let Some(out) = enc.encode(&moving(n, w, h), n == 0).unwrap() {
                assert!(!out.data.is_empty(), "an empty frame came out");
                if first.is_none() {
                    first = Some(out);
                } else {
                    frames += 1;
                }
            }
        }
        let first = first.expect("nothing came out of the encoder in 30 pictures");
        assert!(frames > 0, "only one frame came out of 30 pictures");
        // H.264 arrives as units, each behind a start code. The first thing
        // out has to carry the ones a decoder cannot start without
        assert!(
            first.data.starts_with(&[0, 0, 0, 1]) || first.data.starts_with(&[0, 0, 1]),
            "what came out is not H.264: {:02x?}",
            &first.data[..8.min(first.data.len())]
        );
    }

    /// A picture that changes size under an encoder is refused rather than
    /// quietly producing a mangled frame.
    #[cfg(any(windows, feature = "vp8"))]
    #[test]
    fn a_picture_that_changes_size_is_refused() {
        let mut enc = encoder_for(320, 240, 30).unwrap();
        let said = enc.encode(&moving(0, 160, 120), true).unwrap_err().to_string();
        assert!(said.contains("changed size"), "it failed for some other reason: {said}");
    }

    /// Pictures go in and VP8 comes out, with the ones asked to stand on
    /// their own doing so.
    ///
    /// A keyframe is recognisable from the outside: VP8 puts the frame type
    /// in the low bit of the first byte (0 for a keyframe) and follows a
    /// keyframe with the three bytes 9d 01 2a. Checked because "it produced
    /// some bytes" is not the same as "it produced what was asked for", and
    /// the far end shows a black rectangle for the difference.
    #[cfg(all(not(windows), feature = "vp8"))]
    #[test]
    fn what_comes_out_is_vp8_and_the_whole_ones_are_whole() {
        let (w, h) = (320usize, 240usize);
        let mut enc = encoder_for(w, h, 30).expect("no encoder to test");

        let first = enc
            .encode(&moving(0, w, h), true)
            .expect("the first picture would not compress")
            .expect("the first picture came back as nothing");
        assert!(first.keyframe, "the first picture was asked to stand on its own and did not");
        assert!(first.data.len() > 20, "the first picture is too small to be one");
        assert_eq!(first.data[0] & 1, 0, "the first byte does not say keyframe");
        assert_eq!(
            &first.data[3..6],
            &[0x9d, 0x01, 0x2a],
            "the mark every VP8 keyframe carries is not there"
        );

        // And the ones after it are differences: smaller, and not keyframes
        let mut after = Vec::new();
        for i in 1..6 {
            if let Some(out) = enc.encode(&moving(i, w, h), false).expect("it would not compress") {
                after.push(out);
            }
        }
        assert!(!after.is_empty(), "nothing came out after the first picture");
        assert!(
            after.iter().all(|f| !f.keyframe),
            "a picture stood on its own without being asked to"
        );
        assert!(
            after.iter().all(|f| f.data.len() > 4),
            "a picture came out with nothing in it: {:?}",
            after.iter().map(|f| f.data.len()).collect::<Vec<_>>()
        );
        // Not "smaller than the whole one", which is only true of pictures
        // that hold still: the test picture is nearly flat, so its keyframe
        // is tiny and a moving bar across it costs more than the page did
    }

    /// Somewhere to ask before offering a viewer something that cannot happen.
    ///
    /// Two ways of being able to: Windows carries an encoder, and everywhere
    /// else it is a library this was built against or it is nothing.
    #[test]
    fn the_build_says_whether_it_can_compress() {
        assert_eq!(available(), cfg!(windows) || cfg!(feature = "vp8"));
        // And whichever it is, there is an encoder to be had
        if available() {
            assert!(encoder_for(160, 120, 30).is_ok(), "it says it can and then cannot");
        }
    }

    /// What the connection is agreed on and what the encoder produces have to
    /// be the same word. They are decided in different places and at
    /// different times -- the agreement before the first picture, the encoder
    /// after it -- and if they ever part, the far end shows a black rectangle
    /// and nothing anywhere says why.
    #[cfg(any(windows, feature = "vp8"))]
    #[test]
    fn what_is_promised_is_what_is_produced() {
        let enc = encoder_for(160, 120, 30).unwrap();
        assert_eq!(enc.codec(), codec(), "the encoder makes something else than was promised");
    }
}
