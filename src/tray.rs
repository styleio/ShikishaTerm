//! The icon in the notification area, which is where the program lives while
//! its window is put away.
//!
//! Closing the window used to end the program, and with it every AI at work
//! in a tab and the phone's connection. Now the window is only put away, and
//! this icon is the one thing left on screen that says the program is still
//! there: a left press brings the window back, a right press offers "Open"
//! and "Quit", the latter being the only way to end the program from here.
//!
//! Hand-rolled over `Shell_NotifyIcon` rather than a crate: it is one icon,
//! one message and a two-line menu, and the message loop it has to fit into
//! is `tao`'s, which offers a hook for exactly this.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, IMAGE_ICON,
    LR_DEFAULTCOLOR, LoadImageW, MF_STRING, MSG, PostMessageW, SM_CXSMICON, SM_CYSMICON,
    SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
    WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
};

/// The message the shell sends the window about the icon. Anything above
/// `WM_APP` is ours to define; the offset only has to be one nothing else in
/// this program uses
const CALLBACK: u32 = WM_APP + 0x5348;
/// This program's one icon
const ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

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
    /// notification area wants, with `tip` as the text under the pointer
    pub fn add(hwnd: isize, tip: &str) -> Self {
        const OUR_ICON: *const u16 = 1 as *const u16; // MAKEINTRESOURCE(1)
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
            Shell_NotifyIconW(NIM_ADD, &d);
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

/// Reads one message off the loop. `None` when it is not about the icon;
/// otherwise what the press asked for, the menu having been shown and
/// answered here when the press was a right one.
///
/// `msg` is what tao's message hook hands over: a pointer to the `MSG`
pub fn pressed(msg: *const c_void, hwnd: isize, open: &str, quit: &str) -> Option<Pressed> {
    if msg.is_null() || hwnd == 0 {
        return None;
    }
    let m = unsafe { &*(msg as *const MSG) };
    if m.hwnd as isize != hwnd || m.message != CALLBACK {
        return None;
    }
    match m.lParam as u32 {
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => Some(Pressed::Open),
        WM_RBUTTONUP | WM_CONTEXTMENU => Some(menu(hwnd, open, quit)),
        _ => Some(Pressed::Nothing),
    }
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

    /// A message for some other window, or about something else, is not ours
    #[test]
    fn a_message_that_is_not_about_the_icon_is_left_alone() {
        assert_eq!(pressed(std::ptr::null(), 1, "o", "q"), None);
        let mut m: MSG = unsafe { std::mem::zeroed() };
        m.hwnd = 7 as *mut c_void;
        m.message = CALLBACK;
        m.lParam = WM_LBUTTONUP as isize;
        assert_eq!(pressed(&m as *const MSG as *const c_void, 8, "o", "q"), None);
        m.message = CALLBACK + 1;
        assert_eq!(pressed(&m as *const MSG as *const c_void, 7, "o", "q"), None);
        m.message = CALLBACK;
        assert_eq!(pressed(&m as *const MSG as *const c_void, 7, "o", "q"), Some(Pressed::Open));
    }
}
