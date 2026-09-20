//! The sound of the page being watched, on its way to whoever is watching it.
//!
//! **Only that page's sound.** Windows can record what one process and its
//! children are playing, which is what this asks for: the music a relayed page
//! is playing reaches the phone, and everything else this machine makes --
//! another window, a notification, a call in another program -- does not. The
//! ordinary way to record "what the speakers are playing" would send all of it,
//! and a screen somebody chose to share is not permission to listen to the
//! room.
//!
//! What comes out is what Opus wants: 48kHz, two channels, in frames of 20
//! milliseconds. What goes in is whatever the page is playing at, which is
//! usually already 48kHz stereo because that is what the audio engine mixes
//! at; anything else is made to fit.

use anyhow::{Result, bail};

/// What every part of this agrees on. Opus is defined at these rates and
/// WebRTC uses 48kHz stereo for everything, so there is nothing to choose.
pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// 20 milliseconds, the frame length WebRTC sends and every browser expects.
pub const FRAME: usize = (RATE as usize / 50) * CHANNELS;

/// Sound taken from a page, as it was playing it.
///
/// Interleaved, left then right, as floats between -1 and 1 -- the shape the
/// audio engine itself works in, so nothing is converted until it has to be.
pub type Samples = Vec<f32>;

/// Whether this build can listen to a page at all.
pub fn available() -> bool {
    cfg!(windows)
}

/// Start listening to what a process and its children are playing.
///
/// The process is the browser the page lives in, not this program: a page is
/// drawn and played by a browser of its own, and the sound comes out of that
/// one's tree.
pub fn listen(pid: u32) -> Result<Box<dyn Ears>> {
    #[cfg(windows)]
    {
        windows_loopback::open(pid).map(|e| Box::new(e) as Box<dyn Ears>)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        bail!("this build cannot listen to a page yet: the way to do it here is not written")
    }
}

/// Something that hands over sound as it is played.
pub trait Ears: Send {
    /// Whatever has been played since this was last asked, at [`RATE`] and
    /// [`CHANNELS`]. Empty is ordinary and means silence -- a page playing
    /// nothing produces nothing, and that is the whole reason this costs
    /// nothing while nobody is playing anything.
    fn take(&mut self) -> Result<Samples>;
}

#[cfg(windows)]
mod windows_loopback {
    //! Windows records one process's sound through the same interface it
    //! records a microphone with, activated in a different way: instead of
    //! naming a device, an activation asks for a process and says whether its
    //! children are included. It arrived in Windows 10 2004; before that there
    //! is no way to do this at all, and the failure says so.
    use super::*;
    use std::sync::Mutex;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
    use windows::core::{Interface, PCWSTR, implement};

    /// The device name that means "a process, not a device". It is a string
    /// rather than an id because that is how this activation is asked for.
    const PROCESS_LOOPBACK: &str = "VAD\\Process_Loopback";

    /// Told when the activation has finished, which is the only way this
    /// interface reports it: the call returns before the client exists.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct Done {
        signal: HANDLE,
        /// Used once and then empty. In a lock because the interface this
        /// implements hands out shared references and COM may call from
        /// anywhere, and taken out rather than borrowed because sending is
        /// the last thing it does
        came: Mutex<Option<std::sync::mpsc::Sender<Handed>>>,
    }

    /// What comes back from the activation, wrapped so it can cross the one
    /// thread boundary there is. COM objects are not ordinarily allowed to,
    /// and this one is: it is made by the callback, handed over, and never
    /// touched from there again.
    struct Handed(IAudioClient);
    unsafe impl Send for Handed {}

    impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
        fn ActivateCompleted(
            &self,
            op: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            unsafe {
                if let Some(op) = op.as_ref() {
                    let mut hr = windows::core::HRESULT(0);
                    let mut made: Option<windows::core::IUnknown> = None;
                    let _ = op.GetActivateResult(&mut hr, &mut made);
                    if hr.is_ok()
                        && let Some(made) = made
                        && let Ok(client) = made.cast::<IAudioClient>()
                        && let Some(came) =
                            self.came.lock().unwrap_or_else(|e| e.into_inner()).take()
                    {
                        let _ = came.send(Handed(client));
                    }
                }
                let _ = SetEvent(self.signal);
            }
            Ok(())
        }
    }

    pub struct Loopback {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        /// How many channels the page is actually playing in, which is not
        /// always two -- and what comes out of here always is
        channels: usize,
        rate: u32,
    }

    // Used from one thread at a time, which is the thread that owns it
    unsafe impl Send for Loopback {}

    pub fn open(pid: u32) -> Result<Loopback> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            // What to listen to: this process tree, playing anything
            let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        // The page is drawn by a browser that puts its sound
                        // in a child of its own, so the tree is the only
                        // answer that catches anything
                        ProcessLoopbackMode:
                            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            };
            // The parameters are handed over as a lump of bytes inside a
            // variant, which is how this call takes anything at all. Built
            // field by field because there is no shorthand for "a blob"
            //
            // Never dropped, and that is the whole of it: tidying one of
            // these away means handing what is inside it back to the
            // allocator, and what is inside this one is the stack. Dropped
            // once by accident and the program died of a corrupted heap with
            // nothing printed -- after the call had already worked
            let mut blob = std::mem::ManuallyDrop::new(PROPVARIANT::default());
            {
                let inner = &mut *blob.Anonymous.Anonymous;
                inner.vt = windows::Win32::System::Variant::VT_BLOB;
                inner.Anonymous.blob.cbSize =
                    std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
                inner.Anonymous.blob.pBlobData = std::ptr::addr_of_mut!(params) as *mut u8;
            }

            let signal = CreateEventW(None, true, false, PCWSTR::null())
                .map_err(|e| anyhow::anyhow!("the wait for the sound could not be made: {e}"))?;
            // Handed over exactly once, from whichever thread the callback
            // runs on to this one. A channel rather than a shared box: what
            // is inside is a COM object, and saying "one thread hands it to
            // another" is both what happens and what is allowed
            let (came, comes) = std::sync::mpsc::channel::<Handed>();
            let handler: IActivateAudioInterfaceCompletionHandler =
                Done { signal, came: Mutex::new(Some(came)) }.into();

            let mut wide: Vec<u16> = PROCESS_LOOPBACK.encode_utf16().collect();
            wide.push(0);
            let op = ActivateAudioInterfaceAsync(
                PCWSTR(wide.as_ptr()),
                &IAudioClient::IID,
                Some(&*blob as *const PROPVARIANT),
                &handler,
            )
            .map_err(|e| anyhow::anyhow!("this Windows cannot listen to one page: {e}"))?;
            // Held so the operation is not dropped while it runs
            let _op = op;

            if WaitForSingleObject(signal, 2000) != WAIT_OBJECT_0 {
                bail!("the sound did not start in time");
            }
            let client = comes
                .try_recv()
                .map(|h| h.0)
                .map_err(|_| anyhow::anyhow!("nothing came back to listen with"))?;

            // Ask for what everything downstream wants. A process loopback
            // has no format of its own to ask about -- there is no device --
            // so this says what it will take rather than asking
            let want = WAVEFORMATEX {
                // "the samples are floats". The name for it lives with the
                // multimedia constants, which are not worth a whole namespace
                // for one number that has been 3 since 1995
                wFormatTag: 3,
                nChannels: CHANNELS as u16,
                nSamplesPerSec: RATE,
                wBitsPerSample: 32,
                nBlockAlign: (CHANNELS * 4) as u16,
                nAvgBytesPerSec: RATE * (CHANNELS as u32) * 4,
                cbSize: 0,
            };
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    // Loopback of a process is always event-less polling, and
                    // AUTOCONVERTPCM lets Windows do the fitting if the page
                    // plays at another rate
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    // A fifth of a second of room, which is plenty: this is
                    // emptied every time a frame is asked for
                    2_000_000,
                    0,
                    &want,
                    None,
                )
                .map_err(|e| anyhow::anyhow!("the sound would not start: {e}"))?;

            let capture: IAudioCaptureClient = client
                .GetService()
                .map_err(|e| anyhow::anyhow!("there is nothing to read the sound from: {e}"))?;
            client.Start().map_err(|e| anyhow::anyhow!("the sound would not begin: {e}"))?;

            Ok(Loopback { client, capture, channels: CHANNELS, rate: RATE })
        }
    }

    impl Drop for Loopback {
        fn drop(&mut self) {
            unsafe {
                let _ = self.client.Stop();
            }
        }
    }

    impl Ears for Loopback {
        fn take(&mut self) -> Result<Samples> {
            let mut out: Samples = Vec::new();
            unsafe {
                loop {
                    let ready = self
                        .capture
                        .GetNextPacketSize()
                        .map_err(|e| anyhow::anyhow!("the sound stopped: {e}"))?;
                    if ready == 0 {
                        break;
                    }
                    let mut data: *mut u8 = std::ptr::null_mut();
                    let mut frames: u32 = 0;
                    let mut flags: u32 = 0;
                    self.capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .map_err(|e| anyhow::anyhow!("the sound could not be read: {e}"))?;
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    let n = frames as usize * self.channels;
                    if silent {
                        out.extend(std::iter::repeat_n(0.0f32, n));
                    } else if !data.is_null() {
                        out.extend_from_slice(std::slice::from_raw_parts(data as *const f32, n));
                    }
                    self.capture
                        .ReleaseBuffer(frames)
                        .map_err(|e| anyhow::anyhow!("the sound could not be let go: {e}"))?;
                }
            }
            let _ = self.rate;
            Ok(out)
        }
    }
}

/// Sound on its way out, compressed the way every browser expects it.
///
/// Opus, at 48kHz and two channels, in frames of twenty milliseconds. Fed
/// whatever the page has played since last time and handing back however many
/// whole frames that came to -- none of it, usually, because sound arrives in
/// smaller pieces than it is sent in.
pub struct Voice {
    encoder: rusty_opus::OpusEncoder,
    /// What is left over between calls. Sound does not arrive in twenties of
    /// a second and a frame that is short is a frame that clicks
    held: Samples,
    scratch: Vec<u8>,
}

impl Voice {
    pub fn new() -> Result<Voice> {
        // `Audio` rather than `Voip`: what a page plays is music and video as
        // often as it is speech, and the other setting spends its effort on
        // making a voice clear at the cost of everything else
        let encoder = rusty_opus::OpusEncoder::new(
            RATE as i32,
            CHANNELS,
            rusty_opus::Application::Audio,
        )
        .map_err(|e| anyhow::anyhow!("the sound could not be prepared: {e}"))?;
        Ok(Voice { encoder, held: Vec::new(), scratch: vec![0u8; 4000] })
    }

    /// Everything that is now a whole frame. Nothing is kept beyond one
    /// frame's worth, so a caller that stops asking stops costing anything.
    pub fn feed(&mut self, played: &[f32]) -> Result<Vec<Vec<u8>>> {
        self.held.extend_from_slice(played);
        let mut out = Vec::new();
        while self.held.len() >= FRAME {
            let frame: Samples = self.held.drain(..FRAME).collect();
            let n = self
                .encoder
                .encode(&frame, FRAME / CHANNELS, &mut self.scratch)
                .map_err(|e| anyhow::anyhow!("the sound would not compress: {e}"))?;
            if n > 0 {
                out.push(self.scratch[..n].to_vec());
            }
        }
        Ok(out)
    }
}

/// Gather the sound of one page and hand it to a viewer, for as long as they
/// are listening.
///
/// Its own thread, because sound does not arrive with the pictures: a page
/// plays continuously whether the screen is drawn or not, and twenty
/// milliseconds late is a click that everyone hears.
///
/// **Nothing is recorded until somebody asks.** `wanted` is turned on by the
/// phone tapping the speaker and off by tapping it again, and while it is off
/// this holds no microphone, no loopback and no buffer -- a screen somebody
/// is watching is not permission to listen to it.
pub fn pour(pid: u32, mouth: crate::webrtc::Mouth, wanted: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let mut ears: Option<Box<dyn Ears>> = None;
        let mut voice: Option<Voice> = None;
        loop {
            if !wanted.load(Ordering::Relaxed) {
                // Put it down. Not paused: a loopback that is held is a
                // recording that is running
                ears = None;
                voice = None;
                std::thread::sleep(std::time::Duration::from_millis(100));
                // The viewer may have gone while nobody was listening
                if !mouth.alive() {
                    return;
                }
                continue;
            }
            if ears.is_none() {
                match listen(pid) {
                    Ok(e) => {
                        ears = Some(e);
                        voice = Voice::new().ok();
                        crate::append_hook_log("the page's sound is going out too");
                    }
                    Err(e) => {
                        crate::append_hook_log(&format!("the page's sound cannot be taken: {e:#}"));
                        wanted.store(false, Ordering::Relaxed);
                        continue;
                    }
                }
            }
            let (Some(e), Some(v)) = (ears.as_mut(), voice.as_mut()) else { return };
            let played = std::time::Instant::now();
            let got = match e.take() {
                Ok(got) => got,
                Err(why) => {
                    crate::append_hook_log(&format!("the page's sound stopped: {why:#}"));
                    return;
                }
            };
            match v.feed(&got) {
                Ok(frames) => {
                    for data in frames {
                        if !mouth.say(crate::webrtc::Sound { data, played }) {
                            return;
                        }
                    }
                }
                Err(why) => {
                    crate::append_hook_log(&format!("the page's sound would not compress: {why:#}"));
                    return;
                }
            }
            // Half a frame: often enough that nothing piles up, seldom enough
            // that this is not a thread spinning
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame is twenty milliseconds of two channels at 48kHz, which is what
    /// the far end is expecting and not something to be worked out twice.
    #[test]
    fn a_frame_is_twenty_milliseconds() {
        assert_eq!(FRAME, 1920);
        assert_eq!(FRAME / CHANNELS, 960);
    }

    /// A tone goes in and Opus comes out, in frames of the length the far
    /// end is expecting.
    ///
    /// What cannot be checked here is whether a browser likes it; that is
    /// what a browser is for, and `tools/debug/relay-sound.win.mjs` asks one.
    /// What is checked here is that whole frames come out, one per twenty
    /// milliseconds in, and that they are not empty.
    #[test]
    fn a_tone_becomes_frames_of_the_right_length() {
        let mut voice = Voice::new().expect("the encoder would not start");
        // Half a second of 440Hz, which is a sound and not silence
        let mut tone: Samples = Vec::new();
        for i in 0..(RATE as usize / 2) {
            let at = (i as f32 / RATE as f32) * 440.0 * std::f32::consts::TAU;
            let v = at.sin() * 0.3;
            tone.push(v);
            tone.push(v);
        }
        let frames = voice.feed(&tone).expect("it would not compress");
        assert_eq!(frames.len(), 25, "half a second is twenty-five frames of twenty milliseconds");
        assert!(frames.iter().all(|f| f.len() > 2), "a frame came out empty: {:?}", frames.iter().map(Vec::len).collect::<Vec<_>>());

        // Silence costs almost nothing, which is what makes this cheap to
        // leave running: Opus says "nothing happened" in a byte or two
        let quiet: Samples = vec![0.0; FRAME * 5];
        let frames = voice.feed(&quiet).expect("it would not compress silence");
        assert_eq!(frames.len(), 5);
        // The first one after a sound still carries its tail, which is what
        // a codec is for; the ones after it are the cost of silence
        assert!(
            frames[1..].iter().all(|f| f.len() < 20),
            "silence was expensive: {:?}",
            frames.iter().map(Vec::len).collect::<Vec<_>>()
        );
    }

    /// Sound that is not a whole frame is kept, not sent short.
    #[test]
    fn a_part_of_a_frame_waits_for_the_rest() {
        let mut voice = Voice::new().expect("the encoder would not start");
        let bit: Samples = vec![0.0; FRAME / 2];
        assert!(voice.feed(&bit).unwrap().is_empty(), "half a frame was sent as a whole one");
        assert_eq!(voice.feed(&bit).unwrap().len(), 1, "the other half did not finish it");
    }

    /// A build that cannot listen says so rather than pretending to.
    #[test]
    fn a_build_that_cannot_listen_refuses_plainly() {
        if available() {
            return;
        }
        let said = listen(1).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(said.contains("cannot listen"), "it failed for some other reason: {said}");
    }
}
