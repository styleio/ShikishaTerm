//! The sound of the page being watched, on its way to whoever is watching it.
//!
//! **Only that page's sound.** Windows can record what one process and its
//! children are playing (and a Mac, since macOS 14.2, what chosen processes
//! are), which is what this asks for: the music a relayed page
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
    cfg!(windows) || cfg!(target_os = "macos")
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
    #[cfg(target_os = "macos")]
    {
        mac_tap::open(pid).map(|e| Box::new(e) as Box<dyn Ears>)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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

/// Sound at one rate, made to fit [`RATE`]: each output sample read
/// between the two input samples it falls between. Plain, and for a page
/// already playing at 48kHz -- which is nearly all of them -- nothing is done
#[derive(Default)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct Fit {
    /// Where the next output sample falls, counted from the last input
    /// frame kept from the call before
    at: f64,
    /// The last frame of the call before, so a chunk joins the next
    last: Option<(f32, f32)>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl Fit {
    pub(crate) fn to_rate(&mut self, input: &[f32], rate: f64) -> Samples {
        if (rate - RATE as f64).abs() < 0.5 || rate <= 0.0 {
            return input.to_vec();
        }
        let step = rate / RATE as f64;
        let mut frames: Vec<(f32, f32)> = self.last.into_iter().collect();
        frames.extend(input.chunks_exact(2).map(|f| (f[0], f[1])));
        let mut out = Vec::new();
        while self.at + 1.0 < frames.len() as f64 {
            let i = self.at as usize;
            let t = (self.at - i as f64) as f32;
            let (a, b) = (frames[i], frames[i + 1]);
            out.push(a.0 + (b.0 - a.0) * t);
            out.push(a.1 + (b.1 - a.1) * t);
            self.at += step;
        }
        // Keep the last frame, and say where the next sample falls from it
        if let Some(&last) = frames.last() {
            self.at -= (frames.len() - 1) as f64;
            self.last = Some(last);
        }
        out
    }
}

#[cfg(target_os = "macos")]
mod mac_tap {
    //! A Mac records what chosen processes are playing through a *process
    //! tap* (macOS 14.2 and later): a tap is made over the processes, an
    //! aggregate device is made around the tap, and the device's input is the
    //! sound. Which processes: the ones this app started to play the page --
    //! Chromium's helpers, whose audio service plays every page the window
    //! shows -- and nothing else on the Mac.
    //!
    //! The calls that make a tap are not on a Mac older than 14.2, and a
    //! program that names them outright will not even start there; so they are
    //! looked up while running, and their absence is the answer "this Mac
    //! cannot". The person is asked by the system, once, whether this app may
    //! record what other programs play (Info.plist says why).
    use super::*;
    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use std::collections::VecDeque;
    use std::ffi::{CStr, c_void};
    use std::sync::{Arc, Mutex};

    type OSStatus = i32;
    type AudioObjectID = u32;
    type IOProcID = *mut c_void;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct StreamDescription {
        sample_rate: f64,
        format_id: u32,
        format_flags: u32,
        bytes_per_packet: u32,
        frames_per_packet: u32,
        bytes_per_frame: u32,
        channels_per_frame: u32,
        bits_per_channel: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct Buffer {
        channels: u32,
        size: u32,
        data: *mut c_void,
    }

    #[repr(C)]
    struct BufferList {
        count: u32,
        buffers: [Buffer; 1],
    }

    const fn code(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }
    const SYSTEM: AudioObjectID = 1;
    const GLOBAL: u32 = code(b"glob");
    const MAIN: u32 = 0;
    const PROCESS_LIST: u32 = code(b"prs#");
    const PROCESS_PID: u32 = code(b"ppid");
    const DEFAULT_OUTPUT: u32 = code(b"dOut");
    const DEVICE_UID: u32 = code(b"uid ");
    const TAP_FORMAT: u32 = code(b"tfmt");
    const LINEAR_PCM: u32 = code(b"lpcm");
    const IS_FLOAT: u32 = 1;
    const NOT_INTERLEAVED: u32 = 1 << 5;

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyDataSize(id: AudioObjectID, address: *const Address, qsize: u32, qdata: *const c_void, size: *mut u32) -> OSStatus;
        fn AudioObjectGetPropertyData(
            id: AudioObjectID,
            address: *const Address,
            qsize: u32,
            qdata: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> OSStatus;
        fn AudioHardwareCreateAggregateDevice(description: *const c_void, out: *mut AudioObjectID) -> OSStatus;
        fn AudioHardwareDestroyAggregateDevice(id: AudioObjectID) -> OSStatus;
        fn AudioDeviceCreateIOProcID(device: AudioObjectID, proc_: IoProc, client: *mut c_void, out: *mut IOProcID) -> OSStatus;
        fn AudioDeviceDestroyIOProcID(device: AudioObjectID, id: IOProcID) -> OSStatus;
        fn AudioDeviceStart(device: AudioObjectID, id: IOProcID) -> OSStatus;
        fn AudioDeviceStop(device: AudioObjectID, id: IOProcID) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    type IoProc = extern "C" fn(AudioObjectID, *const c_void, *const BufferList, *const c_void, *mut BufferList, *const c_void, *mut c_void) -> OSStatus;
    type CreateTap = unsafe extern "C" fn(*mut AnyObject, *mut AudioObjectID) -> OSStatus;
    type DestroyTap = unsafe extern "C" fn(AudioObjectID) -> OSStatus;

    /// A call of CoreAudio's that this Mac may not have
    fn look_up<T>(name: &CStr) -> Option<T> {
        let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
        (!found.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&found) })
    }

    fn property<T: Default>(id: AudioObjectID, selector: u32) -> Option<T> {
        let address = Address { selector, scope: GLOBAL, element: MAIN };
        let mut value = T::default();
        let mut size = std::mem::size_of::<T>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(id, &address, 0, std::ptr::null(), &mut size, (&mut value as *mut T).cast())
        };
        (status == 0).then_some(value)
    }

    /// The parent of a process, as the kernel knows it
    fn parent_of(pid: i32) -> Option<i32> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
        (got == size).then_some(info.pbi_ppid as i32)
    }

    /// The audio objects of `pid` and of the processes it started -- the ones
    /// that have played anything yet; a process that has not is not one the
    /// system has an object for
    fn players(pid: u32) -> Vec<AudioObjectID> {
        let address = Address { selector: PROCESS_LIST, scope: GLOBAL, element: MAIN };
        let mut size = 0u32;
        if unsafe { AudioObjectGetPropertyDataSize(SYSTEM, &address, 0, std::ptr::null(), &mut size) } != 0 {
            return Vec::new();
        }
        let mut list = vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
        if unsafe { AudioObjectGetPropertyData(SYSTEM, &address, 0, std::ptr::null(), &mut size, list.as_mut_ptr().cast()) } != 0 {
            return Vec::new();
        }
        let mut ours: Vec<AudioObjectID> = list
            .into_iter()
            .filter(|&object| {
                property::<i32>(object, PROCESS_PID)
                    .is_some_and(|p| p as u32 == pid || parent_of(p).is_some_and(|parent| parent as u32 == pid))
            })
            .collect();
        ours.sort_unstable();
        ours
    }

    /// What the tap's device hands over, waiting to be taken: at most two
    /// seconds of it, the oldest let go first
    type Heard = Arc<Mutex<VecDeque<f32>>>;

    struct Tap {
        tap: AudioObjectID,
        device: AudioObjectID,
        proc_id: IOProcID,
        destroy: DestroyTap,
        /// Holds the place the device writes to for as long as it runs
        _heard: Box<Heard>,
    }

    impl Drop for Tap {
        fn drop(&mut self) {
            unsafe {
                AudioDeviceStop(self.device, self.proc_id);
                AudioDeviceDestroyIOProcID(self.device, self.proc_id);
                AudioHardwareDestroyAggregateDevice(self.device);
                (self.destroy)(self.tap);
            }
        }
    }

    /// Each round of the device: its input, the tap, copied out as
    /// interleaved left-right floats
    extern "C" fn heard(
        _device: AudioObjectID,
        _now: *const c_void,
        input: *const BufferList,
        _input_time: *const c_void,
        _output: *mut BufferList,
        _output_time: *const c_void,
        client: *mut c_void,
    ) -> OSStatus {
        if input.is_null() || client.is_null() {
            return 0;
        }
        let heard = unsafe { &*(client as *const Heard) };
        let list = unsafe { &*input };
        let buffers = unsafe { std::slice::from_raw_parts(list.buffers.as_ptr(), list.count as usize) };
        let mut out = heard.lock().unwrap_or_else(|e| e.into_inner());
        match buffers {
            // One buffer of interleaved channels
            [one] if !one.data.is_null() => {
                let n = one.size as usize / 4;
                let samples = unsafe { std::slice::from_raw_parts(one.data as *const f32, n) };
                let ch = one.channels.max(1) as usize;
                for frame in samples.chunks_exact(ch) {
                    let (l, r) = (frame[0], *frame.get(1).unwrap_or(&frame[0]));
                    out.push_back(l);
                    out.push_back(r);
                }
            }
            // A buffer for each channel
            [left, right, ..] if !left.data.is_null() && !right.data.is_null() => {
                let n = (left.size.min(right.size) as usize) / 4;
                let (l, r) = unsafe {
                    (
                        std::slice::from_raw_parts(left.data as *const f32, n),
                        std::slice::from_raw_parts(right.data as *const f32, n),
                    )
                };
                for i in 0..n {
                    out.push_back(l[i]);
                    out.push_back(r[i]);
                }
            }
            _ => {}
        }
        let most = RATE as usize * CHANNELS * 2;
        while out.len() > most {
            out.pop_front();
        }
        0
    }

    /// An Objective-C string
    fn ns_string(s: &str) -> Option<Retained<AnyObject>> {
        let c = std::ffi::CString::new(s).ok()?;
        unsafe { msg_send![AnyClass::get(c"NSString")?, stringWithUTF8String: c.as_ptr()] }
    }

    fn ns_dictionary(entries: &[(&str, &AnyObject)]) -> Option<Retained<AnyObject>> {
        unsafe {
            let dict: Retained<AnyObject> = msg_send![AnyClass::get(c"NSMutableDictionary")?, new];
            for (key, value) in entries {
                let key = ns_string(key)?;
                let _: () = msg_send![&*dict, setObject: *value, forKey: &*key];
            }
            Some(dict)
        }
    }

    fn ns_array(items: &[&AnyObject]) -> Option<Retained<AnyObject>> {
        unsafe {
            let array: Retained<AnyObject> = msg_send![AnyClass::get(c"NSMutableArray")?, new];
            for item in items {
                let _: () = msg_send![&*array, addObject: *item];
            }
            Some(array)
        }
    }

    fn ns_bool(on: bool) -> Option<Retained<AnyObject>> {
        unsafe { msg_send![AnyClass::get(c"NSNumber")?, numberWithBool: Bool::new(on)] }
    }

    /// The UID of the device the Mac plays through: the aggregate device keeps
    /// time by it
    fn output_uid() -> Option<Retained<AnyObject>> {
        let device: AudioObjectID = property(SYSTEM, DEFAULT_OUTPUT)?;
        let uid: *const c_void = property::<usize>(device, DEVICE_UID)? as *const c_void;
        if uid.is_null() {
            return None;
        }
        // A CFString, the same object as an NSString; ours to release, and
        // retained by the one made from it
        let made = unsafe { Retained::retain(uid as *mut AnyObject) };
        unsafe { CFRelease(uid) };
        made
    }

    fn open_tap(players: &[AudioObjectID]) -> Result<(Tap, StreamDescription, Heard)> {
        let Some(create) = look_up::<CreateTap>(c"AudioHardwareCreateProcessTap") else {
            bail!("this Mac cannot record one program's sound: it needs macOS 14.2 or later")
        };
        let Some(destroy) = look_up::<DestroyTap>(c"AudioHardwareDestroyProcessTap") else {
            bail!("this Mac cannot record one program's sound: it needs macOS 14.2 or later")
        };
        autoreleasepool(|_| unsafe {
            let class = AnyClass::get(c"CATapDescription").ok_or_else(|| anyhow::anyhow!("this Mac has no process taps"))?;
            let numbers: Vec<Retained<AnyObject>> = players
                .iter()
                .filter_map(|&p| msg_send![AnyClass::get(c"NSNumber")?, numberWithUnsignedInt: p])
                .collect();
            let refs: Vec<&AnyObject> = numbers.iter().map(|n| &**n).collect();
            let processes = ns_array(&refs).ok_or_else(|| anyhow::anyhow!("no list of processes"))?;
            let alloc: Allocated<AnyObject> = msg_send![class, alloc];
            let desc: Option<Retained<AnyObject>> = msg_send![alloc, initStereoMixdownOfProcesses: &*processes];
            let desc = desc.ok_or_else(|| anyhow::anyhow!("the tap could not be described"))?;
            // Heard by this app alone, and still heard by the person at the Mac
            let _: () = msg_send![&*desc, setPrivate: Bool::YES];
            let _: () = msg_send![&*desc, setMuteBehavior: 0isize];
            let uuid: Retained<AnyObject> = msg_send![&*desc, UUID];
            let tap_uid: Retained<AnyObject> = msg_send![&*uuid, UUIDString];

            let mut tap: AudioObjectID = 0;
            let status = create(Retained::as_ptr(&desc).cast_mut(), &mut tap);
            if status != 0 || tap == 0 {
                bail!("the Mac would not record the page's sound ({status}); it may need to be allowed in System Settings > Privacy & Security");
            }
            let format: StreamDescription = property(tap, TAP_FORMAT).unwrap_or_default();

            // The device around the tap: private to this app, keeping time by
            // the output the person hears, with the tap started as it starts
            let device_uid = ns_string(&format!("shikisha-page-sound-{}-{tap}", std::process::id())).unwrap();
            let name = ns_string("SHIKISHA-TERM page sound").unwrap();
            let yes = ns_bool(true).unwrap();
            let no = ns_bool(false).unwrap();
            let sub_tap = ns_dictionary(&[("uid", &*tap_uid), ("drift", &*yes)]).unwrap();
            let taps = ns_array(&[&*sub_tap]).unwrap();
            let mut entries: Vec<(&str, &AnyObject)> = vec![
                ("uid", &*device_uid),
                ("name", &*name),
                ("private", &*yes),
                ("stacked", &*no),
                ("tapautostart", &*yes),
                ("taps", &*taps),
            ];
            let output = output_uid();
            let sub_device;
            let sub_devices;
            if let Some(output) = &output {
                sub_device = ns_dictionary(&[("uid", &**output)]).unwrap();
                sub_devices = ns_array(&[&*sub_device]).unwrap();
                entries.push(("master", &**output));
                entries.push(("subdevices", &*sub_devices));
            }
            let description = ns_dictionary(&entries).unwrap();
            let mut device: AudioObjectID = 0;
            let status = AudioHardwareCreateAggregateDevice(Retained::as_ptr(&description).cast(), &mut device);
            if status != 0 || device == 0 {
                destroy(tap);
                bail!("the device to record the page's sound through could not be made ({status})");
            }

            let sound: Heard = Arc::new(Mutex::new(VecDeque::new()));
            let place = Box::new(Arc::clone(&sound));
            let mut proc_id: IOProcID = std::ptr::null_mut();
            let status = AudioDeviceCreateIOProcID(device, heard as IoProc, (&*place as *const Heard).cast_mut().cast(), &mut proc_id);
            if status != 0 {
                AudioHardwareDestroyAggregateDevice(device);
                destroy(tap);
                bail!("the page's sound could not be listened to ({status})");
            }
            let status = AudioDeviceStart(device, proc_id);
            let made = Tap { tap, device, proc_id, destroy, _heard: place };
            if status != 0 {
                bail!("the page's sound would not start ({status})");
            }
            Ok((made, format, sound))
        })
    }

    /// The page's sound, as it plays
    pub struct Tapped {
        pid: u32,
        tap: Option<(Tap, StreamDescription, Heard)>,
        /// Which processes the tap is over, and when that was last looked at:
        /// a page that starts playing later does so from a process the tap was
        /// not made over, so the list is looked at again every second
        over: Vec<AudioObjectID>,
        looked: Option<std::time::Instant>,
        fit: super::Fit,
    }

    // Used from the one thread that owns it
    unsafe impl Send for Tapped {}

    pub fn open(pid: u32) -> Result<Tapped> {
        if look_up::<CreateTap>(c"AudioHardwareCreateProcessTap").is_none() {
            bail!("this Mac cannot record one program's sound: it needs macOS 14.2 or later");
        }
        Ok(Tapped { pid, tap: None, over: Vec::new(), looked: None, fit: super::Fit::default() })
    }

    impl super::Ears for Tapped {
        fn take(&mut self) -> Result<Samples> {
            if self.looked.is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(1)) {
                self.looked = Some(std::time::Instant::now());
                let now = players(self.pid);
                if now != self.over {
                    self.tap = None;
                    if !now.is_empty() {
                        self.tap = Some(open_tap(&now)?);
                    }
                    self.over = now;
                }
            }
            let Some((_, format, heard)) = &self.tap else {
                // Nothing of this app's is playing yet: silence, which is
                // what nothing playing sounds like
                return Ok(Vec::new());
            };
            let got: Samples = heard.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
            let rate = if format.format_id == LINEAR_PCM && format.format_flags & IS_FLOAT != 0 {
                format.sample_rate
            } else {
                RATE as f64
            };
            let _ = NOT_INTERLEAVED;
            Ok(self.fit.to_rate(&got, rate))
        }
    }
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

    /// Sound already at 48kHz goes through untouched, and sound at another
    /// rate comes out at 48kHz, in step across the chunks it arrives in
    #[test]
    fn sound_at_another_rate_is_made_to_fit() {
        let mut same = Fit::default();
        let tone: Samples = (0..960).flat_map(|i| [i as f32, -(i as f32)]).collect();
        assert_eq!(same.to_rate(&tone, RATE as f64), tone, "48kHz was changed");

        // A second of 44.1kHz, in ten pieces, comes to a second of 48kHz
        let mut fit = Fit::default();
        let mut out = Vec::new();
        for piece in 0..10 {
            let chunk: Samples = (0..4410).flat_map(|i| {
                let v = (piece * 4410 + i) as f32;
                [v, v]
            }).collect();
            out.extend(fit.to_rate(&chunk, 44_100.0));
        }
        let frames = out.len() / CHANNELS;
        assert!((47_990..=48_000).contains(&frames), "a second came out as {frames} frames");
        // A rising line stays a rising line: nothing jumps at the joins
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!(left.windows(2).all(|w| w[1] >= w[0]), "the sound went back on itself at a join");
        assert_eq!(out.len() % CHANNELS, 0, "a frame was split");
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
