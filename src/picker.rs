//! The desktop's own file dialog.
//!
//! Only a process that owns a desktop can put a dialog on it, so this lives
//! with the window rather than with the settings server that asks for it. The
//! server states what it wants -- a folder, a file to read, a place to write --
//! and this turns that into the dialog the operating system draws.

use std::path::{Path, PathBuf};

#[cfg(windows)]
fn raise_own_dialog() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    // Spelled the way the operating system's own headers spell it, so the
    // signatures below can be read against the documentation
    #[allow(clippy::upper_case_acronyms)]
    type BOOL = i32;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
        SetForegroundWindow, SetWindowPos, SwitchToThisWindow, HWND_TOPMOST, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    struct Found {
        pid: u32,
        hwnd: HWND,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = unsafe { &mut *(lparam as *mut Found) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != found.pid || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        // Standard dialogs have the class name "#32770". Consoles are excluded
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if class == "#32770" {
            found.hwnd = hwnd;
            return 0;
        }
        1
    }

    let pid = std::process::id();
    std::thread::spawn(move || {
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut found = Found {
                pid,
                hwnd: std::ptr::null_mut(),
            };
            unsafe { EnumWindows(Some(cb), &mut found as *mut Found as LPARAM) };
            if !found.hwnd.is_null() {
                unsafe {
                    // Leave the topmost attribute set. It only lasts until the dialog closes,
                    // and removing it would let it hide behind the browser
                    SetWindowPos(
                        found.hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                    );
                    BringWindowToTop(found.hwnd);
                    // Foregrounding can be refused for a background process,
                    // so also use an Alt+Tab-equivalent switch
                    SwitchToThisWindow(found.hwnd, 1);
                    SetForegroundWindow(found.hwnd);
                }
                break;
            }
        }
    });
}

/// This desktop, seen as somewhere a person can point at a file.
pub struct DesktopPicker;

impl shikisha_shared::FilePicker for DesktopPicker {
    fn folder(&self, title: &str, start: Option<&Path>) -> Option<PathBuf> {
        raise_own_dialog();
        let mut d = rfd::FileDialog::new().set_title(title);
        if let Some(s) = start {
            d = d.set_directory(s);
        }
        d.pick_folder()
    }

    fn open(&self, title: &str, start: Option<&Path>, ext: (&str, &str)) -> Option<PathBuf> {
        raise_own_dialog();
        let mut d = rfd::FileDialog::new().set_title(title);
        if let Some(s) = start {
            d = d.set_directory(s);
        }
        if !ext.1.is_empty() {
            d = d.add_filter(ext.0, &[ext.1]);
        }
        d.pick_file()
    }

    fn save(&self, title: &str, start: Option<&Path>, name: &str, ext: (&str, &str)) -> Option<PathBuf> {
        raise_own_dialog();
        let mut d = rfd::FileDialog::new().set_title(title).set_file_name(name);
        if let Some(s) = start {
            d = d.set_directory(s);
        }
        if !ext.1.is_empty() {
            d = d.add_filter(ext.0, &[ext.1]);
        }
        d.save_file()
    }
}
