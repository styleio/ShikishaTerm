//! The icon in the notification area, which is where the program lives while
//! its window is put away.
//!
//! Closing the window used to end the program, and with it every AI at work
//! in a tab and the phone's connection. Now the window is only put away, and
//! this icon is the one thing left on screen that says the program is still
//! there: a press brings the window back, a right press offers "Open" and
//! "Quit", the latter being the only way to end the program from here.
//!
//! Hand-rolled over `Shell_NotifyIcon` rather than a crate: it is one icon,
//! one message and a two-line menu.
//!
//! The shell reports a press by *sending* the callback message to the
//! window, straight into its window procedure -- not by posting it to the
//! queue. The first cut listened on the queue (tao's message hook), which a
//! test that posted the message itself passed, and which a real press never
//! reached. So the window procedure is taken over here (`attach`), and the
//! events are read there.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallWindowProcW, CreatePopupMenu, DestroyMenu, GWLP_WNDPROC, GetCursorPos,
    GetSystemMetrics, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, MF_STRING, PostMessageW,
    SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, SetWindowLongPtrW, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CONTEXTMENU, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NULL, WM_RBUTTONDOWN, WM_RBUTTONUP, WNDPROC,
};
use windows_sys::Win32::UI::Shell::{NIN_POPUPCLOSE, NIN_POPUPOPEN};

/// The message the shell sends the window about the icon. `WM_APP` up to
/// `0xBFFF` is the range a program may define for itself; the first cut of
/// this went past it (`WM_APP + 0x5348`, into the range Windows keeps for
/// registered messages)
const CALLBACK: u32 = WM_APP + 0x0348;
/// This program's one icon
const ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;
/// A press made with the keyboard (Enter or Space on the icon)
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
/// What the shell says about the icon when asked in the modern way
/// (`NOTIFYICON_VERSION_4`): the event in the low word of `lParam`, and a
/// press is `NIN_SELECT`, not a mouse message. The mouse messages are kept as
/// well, for a shell that still sends them
const OPEN_EVENTS: [u32; 4] = [NIN_SELECT, NIN_KEYSELECT, WM_LBUTTONUP, WM_LBUTTONDBLCLK];
const MENU_EVENTS: [u32; 2] = [WM_CONTEXTMENU, WM_RBUTTONUP];

/// What a press on the icon asked for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    Open,
    Quit,
    /// Ours, but nothing to do (the pointer passing over, a menu dismissed)
    Nothing,
}

/// The icon, once added. Copyable because the window thread needs it in two
/// places: inside the message loop, to show a notice, and after the loop, to
/// take the icon down
#[derive(Debug, Clone, Copy)]
pub struct Tray {
    hwnd: isize,
}

/// Where a press is reported, and the two words on the menu. One window,
/// one of these
struct Sink {
    on: Box<dyn Fn(Pressed) + Send>,
    open: String,
    quit: String,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);
/// The window procedure this one stands in front of
static PREVIOUS: AtomicIsize = AtomicIsize::new(0);

/// Writes `s` into a fixed UTF-16 field, cut to fit, always terminated
fn fill(field: &mut [u16], s: &str) {
    let room = field.len().saturating_sub(1);
    let mut n = 0;
    for u in s.encode_utf16().take(room) {
        field[n] = u;
        n += 1;
    }
    field[n] = 0;
}

fn data(hwnd: isize) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = hwnd as *mut c_void;
    d.uID = ID;
    d
}

impl Tray {
    /// Puts the icon up, wearing the program's own picture at the size the
    /// notification area wants, with `tip` as the text under the pointer, and
    /// stands in front of the window's procedure to read what the shell says
    /// about it. `on` is told of every press; `open` and `quit` are the menu
    pub fn add(hwnd: isize, tip: &str, on: impl Fn(Pressed) + Send + 'static, open: &str, quit: &str) -> Self {
        const OUR_ICON: *const u16 = std::ptr::dangling::<u16>(); // MAKEINTRESOURCE(1)
        *SINK.lock().unwrap() = Some(Sink { on: Box::new(on), open: open.into(), quit: quit.into() });
        unsafe {
            let previous = SetWindowLongPtrW(hwnd as *mut c_void, GWLP_WNDPROC, procedure as *const () as isize);
            PREVIOUS.store(previous, Ordering::Relaxed);
        }

        let mut d = data(hwnd);
        d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        d.uCallbackMessage = CALLBACK;
        unsafe {
            d.hIcon = LoadImageW(
                GetModuleHandleW(std::ptr::null()),
                OUR_ICON,
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_DEFAULTCOLOR,
            );
        }
        fill(&mut d.szTip, tip);
        unsafe {
            let added = Shell_NotifyIconW(NIM_ADD, &d);
            // Ask for the modern events (see OPEN_EVENTS). Has to be asked
            // after the icon exists
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let versioned = Shell_NotifyIconW(NIM_SETVERSION, &d);
            if added == 0 || versioned == 0 {
                shikisha_core::append_hook_log(&format!(
                    "tray: the icon could not be put up (add={added}, version={versioned})"
                ));
            }
        }
        Self { hwnd }
    }

    /// A notice from the icon, the way Windows shows one: a banner that
    /// names this program and fades on its own
    pub fn notice(&self, title: &str, text: &str) {
        let mut d = data(self.hwnd);
        d.uFlags = NIF_INFO;
        d.dwInfoFlags = NIIF_INFO;
        fill(&mut d.szInfoTitle, title);
        fill(&mut d.szInfo, text);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    /// Takes the icon down. Without this an icon outlives its program as a
    /// ghost that vanishes only when the pointer passes over it
    pub fn remove(&self) {
        let d = data(self.hwnd);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &d);
        }
    }
}

/// The window procedure, in front of the window's own. Reads the icon's
/// events and the "show yourself" request of a second copy; everything else
/// goes on to where it went before
unsafe extern "system" fn procedure(hwnd: *mut c_void, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == CALLBACK {
        let (open, quit) = {
            let sink = SINK.lock().unwrap();
            sink.as_ref().map(|s| (s.open.clone(), s.quit.clone())).unwrap_or_default()
        };
        // The menu runs its own message loop; the lock must not be held
        // across it, or a press made from inside would wait on itself
        let pressed = event(hwnd as isize, l, &open, &quit);
        if let Some(sink) = SINK.lock().unwrap().as_ref() {
            (sink.on)(pressed);
        }
        return 0;
    }
    if shikisha_core::instance::is_show_id(msg) {
        if let Some(sink) = SINK.lock().unwrap().as_ref() {
            (sink.on)(Pressed::Open);
        }
        return 0;
    }
    let previous: WNDPROC = unsafe { std::mem::transmute(PREVIOUS.load(Ordering::Relaxed)) };
    unsafe { CallWindowProcW(previous, hwnd, msg, w, l) }
}

/// What one callback asked for, the menu having been shown and answered here
/// when the press was a right one
fn event(hwnd: isize, lparam: LPARAM, open: &str, quit: &str) -> Pressed {
    // Version 4 puts the event in the low word (the icon's id is in the high
    // one); the older shape is the bare mouse message, which fits in the low
    // word as well
    let event = (lparam as u32) & 0xffff;
    if OPEN_EVENTS.contains(&event) {
        return Pressed::Open;
    }
    if MENU_EVENTS.contains(&event) {
        return menu(hwnd, open, quit);
    }
    // Anything unexpected is worth a line: this is how "a press does nothing"
    // gets its name next time. The pointer passing over, the halves of a
    // press that arrive before the press itself, and the tip opening and
    // closing are the expected traffic of every press, so those are not
    const EXPECTED: [u32; 5] = [WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_RBUTTONDOWN, NIN_POPUPOPEN, NIN_POPUPCLOSE];
    if !EXPECTED.contains(&event) {
        shikisha_core::append_hook_log(&format!("tray: event 0x{event:x} (nothing to do)"));
    }
    Pressed::Nothing
}

/// Shows the two-line menu under the pointer and waits for the answer.
///
/// The window has to be brought to the front first, and told a null message
/// afterwards, or the menu stays up after a click elsewhere -- the one thing
/// every notification-area menu has had to do since Windows 95
fn menu(hwnd: isize, open: &str, quit: &str) -> Pressed {
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let open = wide(open);
    let quit = wide(quit);
    let hwnd = hwnd as *mut c_void;
    let chosen = unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return Pressed::Nothing;
        }
        AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            at.x,
            at.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        chosen as usize
    };
    match chosen {
        MENU_OPEN => Pressed::Open,
        MENU_QUIT => Pressed::Quit,
        _ => Pressed::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field is always terminated, however long the text: a tip that ran
    /// to the end of the buffer would read on into whatever follows it
    #[test]
    fn a_field_is_cut_to_fit_and_terminated() {
        let mut f = [0xFFu16; 8];
        fill(&mut f, "0123456789");
        assert_eq!(&f[..7], &"0123456".encode_utf16().collect::<Vec<_>>()[..]);
        assert_eq!(f[7], 0);

        let mut g = [0xFFu16; 8];
        fill(&mut g, "ab");
        assert_eq!(g[2], 0);
    }

    /// A press is a press in both shapes the shell uses, and the pointer
    /// passing over is not one
    #[test]
    fn a_press_is_read_in_both_shapes_the_shell_uses() {
        assert_eq!(event(7, WM_LBUTTONUP as isize, "o", "q"), Pressed::Open);
        // The modern shape: the event low, the icon's id high
        assert_eq!(event(7, ((ID as isize) << 16) | NIN_SELECT as isize, "o", "q"), Pressed::Open);
        assert_eq!(event(7, ((ID as isize) << 16) | NIN_KEYSELECT as isize, "o", "q"), Pressed::Open);
        assert_eq!(event(7, ((ID as isize) << 16) | WM_MOUSEMOVE as isize, "o", "q"), Pressed::Nothing);
    }
}
