//! Taking a picture of the screen, for the tools that start from one.
//!
//! The screen the pointer is on, at the moment it is asked for, in its own
//! pixels. Taken with the desktop's own drawing, which is what the system's
//! snipping tool falls back to as well: it is one call, it needs no permission
//! beyond the one this program already runs with, and a still picture is all the
//! tools need.
//!
//! The picture is held only while a tool is open over it. It is somebody's
//! screen -- their mail, their passwords, whatever was up -- so it is dropped
//! the moment the tool is closed rather than kept for the next time.

use std::sync::{Arc, Mutex};

/// A screen, where it is on the desktop and how large, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The picture a tool is open over, by the number it was handed out under.
///
/// Asked for by number so that a page still loading from an earlier press can
/// never be handed the picture from a later one
static FRAME: Mutex<Option<(u64, Arc<Vec<u8>>)>> = Mutex::new(None);

/// The picture under this number, if it is still the one a tool is open over.
pub fn frame(n: u64) -> Option<Arc<Vec<u8>>> {
    let held = FRAME.lock().ok()?;
    held.as_ref().filter(|(at, _)| *at == n).map(|(_, bytes)| Arc::clone(bytes))
}

/// Keep a picture for a tool, under a new number.
pub fn hold(bmp: Vec<u8>) -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut held) = FRAME.lock() {
        *held = Some((n, Arc::new(bmp)));
    }
    n
}

/// Let go of the picture. Called as the tool closes
pub fn drop_frame() {
    if let Ok(mut held) = FRAME.lock() {
        *held = None;
    }
}

/// The screen the pointer is on.
#[cfg(windows)]
pub fn screen_at_pointer() -> Option<Screen> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut at = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut at) == 0 {
            return None;
        }
        let mon = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut info) == 0 {
            return None;
        }
        let r = info.rcMonitor;
        Some(Screen { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
    }
}

/// A picture of one screen, as a BMP the page can show as it is.
///
/// BMP rather than PNG: it is on its way to a page on this same machine, and
/// compressing a screenful only to have the page decompress it again is the
/// slowness this tool exists to not have
#[cfg(windows)]
pub fn take(s: Screen) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
    };
    if s.w <= 0 || s.h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return None;
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, s.w, s.h);
        let old = SelectObject(mem, bmp);
        // CAPTUREBLT takes the windows drawn on top of the desktop as layers
        // too -- menus, tooltips -- which are exactly the things somebody waits
        // for before a picture
        let ok = BitBlt(mem, 0, 0, s.w, s.h, screen, s.x, s.y, SRCCOPY | CAPTUREBLT) != 0;
        let mut out = None;
        if ok {
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: s.w,
                // Negative: rows from the top, the order a page reads them in
                biHeight: -s.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut px = vec![0u8; (s.w as usize) * (s.h as usize) * 4];
            let rows = GetDIBits(mem, bmp, 0, s.h as u32, px.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
            if rows == s.h {
                out = Some(bmp_file(s.w, s.h, px));
            }
        }
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        out
    }
}

/// BGRA rows, top first, as a BMP file.
///
/// The fourth byte of each pixel is set to opaque: the desktop leaves it at
/// zero, and a page that honours it would draw the whole picture see-through
pub fn bmp_file(w: i32, h: i32, mut bgra: Vec<u8>) -> Vec<u8> {
    for px in bgra.chunks_exact_mut(4) {
        px[3] = 255;
    }
    let header = 14 + 40;
    let size = header + bgra.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(header as u32).to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(-h).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(bgra.len() as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 dpi, which nothing reads
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&bgra);
    out
}

/// Keep a window out of pictures of the screen, or let it back in.
///
/// For the card that counts down before a picture: it is on screen so the
/// person knows when, and it must not be in what it is counting down to
#[cfg(windows)]
pub fn keep_out_of_pictures(hwnd: isize, out: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};
    unsafe {
        SetWindowDisplayAffinity(hwnd as _, if out { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
    }
}

/// Text to this machine's clipboard.
pub fn copy_text(text: &str) -> bool {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())).is_ok()
}

/// Text to a file the person chooses. Asked on a thread of its own: the dialog
/// waits for the person, and the window's message loop must not wait with it
pub fn save_text(text: String, name: String) {
    use shikisha_shared::FilePicker as _;
    std::thread::spawn(move || {
        let name = sanitize(&name);
        // The same dialog every other "save" in the app opens, brought in front
        // the same way -- the tool's own window has just gone, and a dialog
        // left behind the board is a save that looks like it did nothing
        let title = shikisha_core::i18n::t("snip.save.title");
        if let Some(path) = crate::picker::DesktopPicker.save(&title, None, &name, ("Text", "txt"))
            && let Err(e) = std::fs::write(&path, text)
        {
            shikisha_core::append_hook_log(&format!("snip: could not write {}: {e}", path.display()));
        }
    });
}

/// A file name a page suggested, with nothing in it that could name a folder.
fn sanitize(name: &str) -> String {
    let leaf: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .collect();
    let leaf = leaf.trim().trim_matches('.').to_string();
    if leaf.is_empty() { "snip.txt".into() } else { leaf }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture is a BMP a page can read: the right header, the size it says,
    /// rows from the top, and every pixel opaque
    #[test]
    fn a_picture_is_a_bmp_the_page_can_read() {
        let px = vec![10, 20, 30, 0, 40, 50, 60, 0];
        let bmp = bmp_file(2, 1, px);
        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[2..6].try_into().unwrap()) as usize, bmp.len());
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54, "画素の開始位置");
        assert_eq!(i32::from_le_bytes(bmp[18..22].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(bmp[22..26].try_into().unwrap()), -1, "上から並んでいない");
        assert_eq!(u16::from_le_bytes(bmp[28..30].try_into().unwrap()), 32);
        assert_eq!(&bmp[54..], &[10, 20, 30, 255, 40, 50, 60, 255], "透明のまま");
    }

    /// A picture is handed out only under the number it was kept under, and
    /// is gone once let go
    #[test]
    fn a_picture_is_only_the_one_that_was_asked_for() {
        let n = hold(vec![1, 2, 3]);
        assert_eq!(frame(n).as_deref().map(Vec::as_slice), Some(&[1u8, 2, 3][..]));
        assert!(frame(n + 1).is_none(), "別の番号で渡っている");
        drop_frame();
        assert!(frame(n).is_none(), "閉じた後も残っている");
    }

    /// A name a page suggests cannot walk out of the folder it is saved in
    #[test]
    fn a_suggested_name_is_only_a_name() {
        assert_eq!(sanitize("colors.txt"), "colors.txt");
        assert_eq!(sanitize("..\\..\\evil.txt"), "evil.txt");
        assert_eq!(sanitize("a/b:c.txt"), "abc.txt");
        assert_eq!(sanitize("..."), "snip.txt");
    }
}
