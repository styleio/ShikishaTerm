//! One running program per layout.
//!
//! Now that closing the window leaves the program running, starting it again
//! from the Start menu or a shortcut is a thing that happens by accident,
//! and a second copy over the same config, data and logs is not a second
//! window: it is two programs writing the same session file, fighting over
//! the phone's port and the API pipe, each unaware of the other. So the
//! second one asks the first to show its window, and leaves.
//!
//! "The same layout" is the deciding line, not "this program". The portable
//! copy beside the exe, the one under LOCALAPPDATA, a demo folder: each has
//! its own root, its own files, its own port, and each may run beside the
//! others -- that has always been a way of working here.
//!
//! The claim is a named mutex, the oldest way Windows offers to say this and
//! the one that needs no file: the system drops it the moment the process is
//! gone, so a crash leaves nothing stale behind to unlock. The request to
//! show is a message registered under the same name and posted to every
//! top-level window; only the window of the copy on this layout knows it.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, PostMessageW, RegisterWindowMessageW};

/// Held for the life of the process. Dropping it (or dying) lets the next
/// start be the first again
pub struct Claim {
    handle: HANDLE,
}

impl Drop for Claim {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub enum Standing {
    /// This is the copy that runs. Keep the claim until the end
    First(Claim),
    /// Another copy already runs on this layout
    Second,
}

/// What this layout is called in the system's namespace: the root folder,
/// folded so that `D:\A` and `d:\a\` are the same place
fn key() -> String {
    let root = crate::config::root_dir()
        .display()
        .to_string()
        .to_lowercase()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\");
    // FNV-1a over the folded path. Short enough for a kernel object name,
    // and two different folders do not collide in practice
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Says this copy runs on its layout, or learns that one already does
pub fn claim() -> Standing {
    let name = wide(&format!("Local\\SHIKISHA-TERM:{}", key()));
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            // The system would not say either way. Running is the safer of
            // the two mistakes: a second window beats a program that will not start
            return Standing::First(Claim { handle: std::ptr::null_mut() });
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return Standing::Second;
        }
        Standing::First(Claim { handle })
    }
}

/// The message that means "show your window", private to this layout
fn show_message() -> u32 {
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| {
        let name = wide(&format!("SHIKISHA-TERM.show:{}", key()));
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

/// Asks the copy that runs on this layout to show its window. Posted to
/// every top-level window, since the other copy's window is not known here;
/// only that window answers to the name
pub fn ask_to_show() {
    unsafe {
        PostMessageW(HWND_BROADCAST, show_message(), 0, 0);
    }
}

/// Whether a message reaching the window's procedure is that request
pub fn is_show_id(message: u32) -> bool {
    message == show_message()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second claim on the same layout is told so, and the first holds
    /// until it is dropped. Both claims are made from this one process, which
    /// the system treats no differently from two
    #[test]
    fn the_second_copy_on_a_layout_is_told_it_is_second() {
        let first = match claim() {
            Standing::First(c) => c,
            Standing::Second => panic!("最初の起動なのに二重扱い"),
        };
        assert!(matches!(claim(), Standing::Second), "二つ目が一つ目として通った");
        drop(first);
        assert!(matches!(claim(), Standing::First(_)), "手放した後も塞がったまま");
    }

    /// Only the registered message is the request, and nothing else is
    #[test]
    fn only_the_registered_message_is_the_request() {
        assert!(!is_show_id(show_message() + 1));
        assert!(is_show_id(show_message()));
    }
}
