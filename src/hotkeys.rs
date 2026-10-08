//! Registering the keys that open the tools from any program (see
//! `shikisha_core::hotkeys` for which keys and why).
//!
//! On Windows, on a thread of its own with its own message queue: a key
//! registered with no window is delivered to the thread that registered it,
//! and that thread has to be waiting for messages -- the window's loop belongs
//! to the browser, and the conductor's loop reads no system messages at all.
//!
//! On a Mac the system takes them only from the program's first thread, the
//! one the window's loop runs on, so they are registered there
//! (`Cmd::RegisterKeys`); a press arrives on a channel of the library's,
//! read on a thread of its own as on Windows.

#[cfg(windows)]
use shikisha_core::hotkeys::{self, Row, Wanted};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY,
};

#[cfg(windows)]
/// Asks the thread to read the settings and register again
const RELOAD: u32 = WM_APP + 1;

#[cfg(windows)]
/// The keys' thread. Dropped with the process; the system lets go of a
/// thread's keys when the thread ends
pub struct Hotkeys {
    thread: u32,
}

#[cfg(windows)]
impl Hotkeys {
    /// Start registering, and call `fire` with the action whenever one of the
    /// keys is pressed. Windows takes the keys on any thread; the window's is
    /// not needed
    pub fn start(_window: crate::browser::KeysRegistrar, fire: impl Fn(&'static str) + Send + 'static) -> Option<Hotkeys> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || {
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                // A thread has no queue until it asks for a message; one has to
                // be there before anybody posts to it
                unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
                let _ = tx.send(unsafe { GetCurrentThreadId() });
                let mut held: Vec<&'static str> = register(&[]);
                loop {
                    let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
                    if got <= 0 {
                        break;
                    }
                    match msg.message {
                        WM_HOTKEY => {
                            if let Some(&action) = held.get(msg.wParam) {
                                hotkeys::pressed(action);
                                fire(action);
                            }
                        }
                        RELOAD => held = register(&held),
                        _ => {}
                    }
                }
            })
            .ok()?;
        rx.recv_timeout(std::time::Duration::from_secs(5)).ok().map(|thread| Hotkeys { thread })
    }

    /// The settings changed: register what they ask for now
    pub fn reload(&self) {
        unsafe { PostThreadMessageW(self.thread, RELOAD, 0, 0) };
    }
}

#[cfg(windows)]
/// Let go of the keys held (`held[i]` is registered under id `i`), register the
/// ones the settings ask for, and say how each went. Returns what is held now,
/// indexed the same way
fn register(held: &[&'static str]) -> Vec<&'static str> {
    for i in 0..held.len() {
        unsafe { UnregisterHotKey(std::ptr::null_mut(), i as i32) };
    }
    let written = shikisha_core::config::load().map(|c| c.hotkeys).unwrap_or_default();
    let mut now: Vec<&'static str> = Vec::new();
    let mut rows = Vec::new();
    for (action, want) in hotkeys::wanted(&written) {
        let row = |key: String, state| Row { action: action.to_string(), key, state, last: None };
        rows.push(match want {
            Wanted::Off => row(String::new(), "off"),
            Wanted::Unreadable(text) => row(text, "unreadable"),
            Wanted::Twice(c) => row(c.shown(), "twice"),
            // A Mac's ⌘, in settings carried over from one. Windows has no
            // such key, and the Windows key is not offered in its place
            Wanted::Key(c) if c.cmd => row(c.shown(), "elsewhere"),
            Wanted::Key(c) => {
                let id = now.len() as i32;
                let ok = unsafe { RegisterHotKey(std::ptr::null_mut(), id, c.mods() | MOD_NOREPEAT, c.vk()) } != 0;
                if ok {
                    now.push(action);
                    row(c.shown(), "on")
                } else {
                    // Another program has it. Said on the settings page, where
                    // the key is changed, rather than left not working
                    shikisha_core::append_hook_log(&format!("hotkeys: {} is taken by another program", c.shown()));
                    row(c.shown(), "taken")
                }
            }
        });
    }
    hotkeys::set_registered(rows);
    now
}

#[cfg(target_os = "macos")]
pub use mac::{Hotkeys, register_here};

#[cfg(target_os = "macos")]
mod mac {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use shikisha_core::hotkeys::{self, Combo, Row, Wanted};
    use std::cell::RefCell;

    /// Which registered key is which action, by the id a press comes back with
    static HELD: std::sync::Mutex<Vec<(u32, &'static str)>> = std::sync::Mutex::new(Vec::new());

    thread_local! {
        /// The system's side of the keys, and the keys it holds now. Made and
        /// kept on the window's thread, the only one the system takes them on
        static MANAGER: RefCell<Option<GlobalHotKeyManager>> = const { RefCell::new(None) };
        static REGISTERED: RefCell<Vec<HotKey>> = const { RefCell::new(Vec::new()) };
    }

    /// The thread the presses are heard on, and the way to the window's
    /// thread, where the keys are registered
    pub struct Hotkeys {
        window: crate::browser::KeysRegistrar,
    }

    impl Hotkeys {
        /// Start registering, and call `fire` with the action whenever one of
        /// the keys is pressed
        pub fn start(window: crate::browser::KeysRegistrar, fire: impl Fn(&'static str) + Send + 'static) -> Option<Hotkeys> {
            std::thread::Builder::new()
                .name("hotkeys".into())
                .spawn(move || {
                    let presses = GlobalHotKeyEvent::receiver();
                    while let Ok(event) = presses.recv() {
                        if event.state != HotKeyState::Pressed {
                            continue;
                        }
                        let action = HELD
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .iter()
                            .find(|(id, _)| *id == event.id)
                            .map(|(_, a)| *a);
                        if let Some(action) = action {
                            hotkeys::pressed(action);
                            fire(action);
                        }
                    }
                })
                .ok()?;
            window.register();
            Some(Hotkeys { window })
        }

        /// The settings changed: register what they ask for now
        pub fn reload(&self) {
            self.window.register();
        }
    }

    /// The combination as the system's: its held keys and the key's place on
    /// the keyboard
    fn hotkey_of(c: &Combo) -> Option<HotKey> {
        let mut mods = Modifiers::empty();
        for (on, m) in [(c.ctrl, Modifiers::CONTROL), (c.alt, Modifiers::ALT), (c.shift, Modifiers::SHIFT), (c.cmd, Modifiers::SUPER)] {
            if on {
                mods |= m;
            }
        }
        let place = match c.key.as_bytes() {
            [ch] if ch.is_ascii_uppercase() => format!("Key{}", *ch as char),
            [ch] if ch.is_ascii_digit() => format!("Digit{}", *ch as char),
            _ => c.key.clone(),
        };
        let code: Code = place.parse().ok()?;
        Some(HotKey::new(Some(mods), code))
    }

    /// Let go of the keys held, register the ones the settings ask for, and
    /// say how each went. On the window's thread (`Cmd::RegisterKeys`)
    pub fn register_here() {
        MANAGER.with(|m| {
            let mut m = m.borrow_mut();
            if m.is_none() {
                match GlobalHotKeyManager::new() {
                    Ok(made) => *m = Some(made),
                    Err(e) => {
                        shikisha_core::append_hook_log(&format!("hotkeys: the system's keys could not be reached: {e}"));
                        return;
                    }
                }
            }
            let Some(manager) = m.as_ref() else { return };
            REGISTERED.with(|r| {
                let mut registered = r.borrow_mut();
                if !registered.is_empty() {
                    let _ = manager.unregister_all(&registered);
                    registered.clear();
                }
                let written = shikisha_core::config::load().map(|c| c.hotkeys).unwrap_or_default();
                let mut held = Vec::new();
                let mut rows = Vec::new();
                for (action, want) in hotkeys::wanted(&written) {
                    let row = |key: String, state| Row { action: action.to_string(), key, state, last: None };
                    rows.push(match want {
                        Wanted::Off => row(String::new(), "off"),
                        Wanted::Unreadable(text) => row(text, "unreadable"),
                        Wanted::Twice(c) => row(c.shown(), "twice"),
                        Wanted::Key(c) => match hotkey_of(&c) {
                            None => row(c.shown(), "unreadable"),
                            Some(key) => match manager.register(key) {
                                Ok(()) => {
                                    held.push((key.id(), action));
                                    registered.push(key);
                                    row(c.shown(), "on")
                                }
                                Err(e) => {
                                    // Another program has it. Said on the settings
                                    // page, where the key is changed
                                    shikisha_core::append_hook_log(&format!("hotkeys: {} could not be had: {e}", c.shown()));
                                    row(c.shown(), "taken")
                                }
                            },
                        },
                    });
                }
                *HELD.lock().unwrap_or_else(|e| e.into_inner()) = held;
                hotkeys::set_registered(rows);
            });
        });
    }
}

/// Where there is no window there are no keys of the window's: a server
/// registers none, and says nothing is held
#[cfg(not(any(windows, target_os = "macos")))]
pub struct Hotkeys;

#[cfg(not(any(windows, target_os = "macos")))]
impl Hotkeys {
    /// Nothing is registered, so nothing is held
    pub fn start(_window: crate::browser::KeysRegistrar, _fire: impl Fn(&'static str) + Send + 'static) -> Option<Hotkeys> {
        None
    }

    /// Nothing to register again
    pub fn reload(&self) {}
}
