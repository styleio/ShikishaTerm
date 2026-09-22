//! A place for a runtime with no window of its own to be seen and ended from.
//!
//! Split in two, the runtime keeps the tabs and the work and draws nothing;
//! the window is another program entirely, and it comes and goes. Between one
//! window and the next there has to be *something* on screen saying the
//! program is still there, or a person who closed the window is looking at a
//! machine that appears to have nothing running on it and a notification area
//! that agrees.
//!
//! So the icon moves to the runtime. That is where it belonged all along:
//! the icon's whole promise is "the work is still going", and the work is
//! here.
//!
//! Windows will not put an icon up for a program with no window -- every
//! notification icon is addressed to one, and the menu it opens has to belong
//! to a window that can be brought to the front. So one is made and never
//! shown. It draws nothing, occupies no space on any screen, and exists to
//! own the icon and the little message loop that reads it.
//!
//! The thread that makes a window is the only thread that may read its
//! messages, so the window is made on a thread of its own and that thread
//! does nothing else. [`Standing`] is the handle the rest of the program
//! holds, and every method on it is a message posted to that thread.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_OVERLAPPED,
};

pub use crate::tray::Pressed;

/// The class this program's invisible window is made from. Named rather than
/// anonymous because Windows keeps registered classes by name and this one is
/// registered once per program
const CLASS: &str = "ShikishaResident";

/// The window that is standing by, and the icon on it
pub struct Standing {
    hwnd: AtomicIsize,
    tray: crate::tray::Tray,
}

impl Standing {
    /// A banner from the icon: what Windows shows when a program has
    /// something to say and no window to say it in
    pub fn notice(&self, title: &str, text: &str) {
        self.tray.notice(title, text);
    }

    /// Take the icon down and let the window go. The thread reading its
    /// messages ends with it.
    ///
    /// Without this the icon outlives the program as a ghost that disappears
    /// only when somebody happens to move the pointer across it
    pub fn stop(&self) {
        self.tray.remove();
        let hwnd = self.hwnd.swap(0, Ordering::Relaxed);
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) };
        }
    }
}

impl Drop for Standing {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Put the icon up and keep a window behind it until [`Standing::stop`].
///
/// `on` is called on the message thread for every press, so it does its
/// talking through a channel and nothing else: work done in there is work the
/// icon is not answering during.
pub fn start(
    tip: &str,
    open: &str,
    quit: &str,
    on: impl Fn(Pressed) + Send + 'static,
) -> anyhow::Result<Standing> {
    let (told, wait) = mpsc::channel::<Option<(isize, crate::tray::Tray)>>();
    let (tip, open, quit) = (tip.to_string(), open.to_string(), quit.to_string());
    std::thread::Builder::new().name("resident".into()).spawn(move || {
        let hwnd = match window() {
            Ok(hwnd) => hwnd,
            Err(e) => {
                crate::append_hook_log(&format!("resident: no window to hang the icon on: {e}"));
                let _ = told.send(None);
                return;
            }
        };
        // Put up before the handle is handed back, so that whoever is given
        // one is given one with an icon already on it
        let tray = crate::tray::Tray::add(hwnd, &tip, on, &open, &quit);
        if told.send(Some((hwnd, tray))).is_err() {
            // Nobody left to hand it to
            tray.remove();
            return;
        }
        pump();
        crate::append_hook_log("resident: the icon's window is gone");
    })?;

    let Some((hwnd, tray)) = wait.recv()? else {
        anyhow::bail!("the notification area could not be reached");
    };
    Ok(Standing { hwnd: AtomicIsize::new(hwnd), tray })
}

/// A window that is never shown. `WS_OVERLAPPED` without `WS_VISIBLE`: a real
/// top-level window, which is what the notification area's menu needs to
/// belong to, and one nobody ever sees because it is not shown and has no
/// size worth giving it
fn window() -> anyhow::Result<isize> {
    let class: Vec<u16> = CLASS.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let mut c: WNDCLASSW = std::mem::zeroed();
        c.lpfnWndProc = Some(procedure);
        c.hInstance = module;
        c.lpszClassName = class.as_ptr();
        // Registering the same class twice answers zero and says "already
        // registered", which is not a failure -- a second call here means a
        // window was made and let go earlier in this same program
        RegisterClassW(&c);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            anyhow::bail!("the window could not be made");
        }
        Ok(hwnd as isize)
    }
}

/// Read this thread's messages until the window is gone. The icon's presses
/// arrive here, through the procedure the tray puts in front of this one
fn pump() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

/// Everything this window does on its own. It has no drawing, no input and no
/// size to keep; the only message that means anything is the one saying it is
/// going, which is how the loop above is let out
unsafe extern "system" fn procedure(hwnd: *mut c_void, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// A yes-or-no question, put the way Windows puts one when the program
/// asking has no window to put it in.
///
/// Its own window, on top of whatever is there. A runtime split from its
/// screen still has a desktop and somebody at it, and the one question worth
/// asking -- whether to stop with an AI mid-turn -- is worth asking there
pub fn ask_yes_no(title: &str, body: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
    };
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (body, title) = (wide(body), wide(title));
    let answered = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    answered == IDYES
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window with nothing in it is still a window, and the same class
    /// asked for twice is not a failure -- which it would look like, because
    /// registering it the second time answers zero
    #[test]
    fn a_window_nobody_sees_can_be_made_more_than_once() {
        let a = window().expect("no window");
        assert_ne!(a, 0);
        let b = window().expect("the class was registered once and never again");
        assert_ne!(b, 0);
        assert_ne!(a, b, "the same window came back twice");
    }

    /// The whole of it, on a real notification area: the icon goes up, a
    /// banner is shown, and it comes down again leaving nothing behind.
    ///
    /// Not run by default because it puts a picture on the machine's own
    /// notification area for as long as it takes. Run it with
    /// `cargo test -p shikisha-core resident -- --ignored --nocapture`
    /// and watch the corner of the screen
    #[test]
    #[ignore = "puts a real icon in this machine's notification area"]
    fn the_icon_goes_up_and_comes_down() {
        let standing = start("SHIKISHA-TERM", "Open", "Quit", |p| {
            println!("pressed: {p:?}");
        })
        .expect("the icon did not go up");
        standing.notice("SHIKISHA-TERM", "still working");
        std::thread::sleep(std::time::Duration::from_secs(5));
        standing.stop();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
