//! Registering the keys that open the tools from any program (see
//! `shikisha_core::hotkeys` for which keys and why).
//!
//! On a thread of its own with its own message queue: a key registered with no
//! window is delivered to the thread that registered it, and that thread has to
//! be waiting for messages -- the window's loop belongs to the browser, and
//! the conductor's loop reads no system messages at all.

use shikisha_core::hotkeys::{self, Row, Wanted};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY,
};

/// Asks the thread to read the settings and register again
const RELOAD: u32 = WM_APP + 1;

/// The keys' thread. Dropped with the process; the system lets go of a
/// thread's keys when the thread ends
pub struct Hotkeys {
    thread: u32,
}

impl Hotkeys {
    /// Start registering, and call `fire` with the action whenever one of the
    /// keys is pressed
    pub fn start(fire: impl Fn(&'static str) + Send + 'static) -> Option<Hotkeys> {
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
