//! Which pseudo-console this program is actually talking through.
//!
//! Windows has shipped two of these. The one in the box parses whatever a
//! program writes into a text buffer and re-renders a picture of that buffer
//! into our pipe; the newer one, which Microsoft distributes as a signed
//! package, copies the program's bytes through untouched. The difference is
//! not cosmetic: measured on this project's own stack, the in-box one drops
//! APC and DCS sequences entirely, rewrites OSC 8 hyperlinks, reorders OSC
//! against the text around it, and spends 286ms and 2.93MB where the newer one
//! spends 119ms and 2.17MB on the same ten thousand lines of Japanese.
//!
//! Nothing here chooses between them. portable-pty already prefers a
//! `conpty.dll` sitting next to the executable over the system's, so what
//! decides is whether the file is there -- which makes the interesting
//! question "which one did we end up with", and that is what this answers.
//!
//! It has to be answered out loud, because both ways of losing the newer one
//! are silent. A download that arrived without the file works exactly as
//! before, only slower. And `conpty.dll` without `OpenConsole.exe` beside it
//! is worse than useless: measured, it loads, reports success, and quietly
//! serves the in-box behaviour anyway. Neither shows up as an error, so this
//! is the only place either can be seen.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Why the newer ConPTY is not in use. Reported as a code rather than a
/// sentence: the settings screen puts it into the reader's own language, and
/// the log wants the short form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// No `conpty.dll` beside the executable. The ordinary shape of a build
    /// that never fetched it (tools/conpty.ps1)
    NotShipped,
    /// It is there but will not load -- the wrong architecture, or truncated
    Unloadable,
    /// It loads but does not export the three functions a pseudo console needs
    Incomplete,
    /// The DLL is there and `OpenConsole.exe` is not. The DLL starts that
    /// program to host the console; without it we get the in-box behaviour
    /// back with none of the noise that would tell us so
    NoOpenConsole,
}

impl Missing {
    /// The word the settings screen and the log both key off.
    pub fn id(self) -> &'static str {
        match self {
            Missing::NotShipped => "not_shipped",
            Missing::Unloadable => "unloadable",
            Missing::Incomplete => "incomplete",
            Missing::NoOpenConsole => "no_openconsole",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    /// True when the copy beside the executable is the one in use
    pub bundled: bool,
    /// Where we looked. Shown even when nothing was found, because "there is
    /// no file at this exact path" is the useful half of that answer
    pub path: PathBuf,
    /// The DLL's own version, when it could be read
    pub version: String,
    pub missing: Option<Missing>,
}

impl Report {
    /// One line for the hook log, in English like every other line there.
    pub fn line(&self) -> String {
        match self.missing {
            None => format!(
                "ConPTY: bundled{} ({})",
                match self.version.is_empty() {
                    true => String::new(),
                    false => format!(" {}", self.version),
                },
                self.path.display()
            ),
            Some(m) => format!(
                "ConPTY: in-box ({}) -- terminal output is slower and some sequences are dropped",
                m.id()
            ),
        }
    }
}

static REPORT: OnceLock<Report> = OnceLock::new();

/// Settled once, on first ask. The answer cannot change while the program
/// runs: portable-pty resolves the same question once per process too.
pub fn report() -> &'static Report {
    REPORT.get_or_init(look)
}

fn look() -> Report {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default();
    let path = dir.join("conpty.dll");
    let mut out = Report {
        bundled: false,
        path: path.clone(),
        version: String::new(),
        missing: Some(Missing::NotShipped),
    };
    if !path.is_file() {
        return out;
    }
    out.version = file_version(&path);
    // The same three the pseudo console is driven by. A DLL that answers for
    // all three is one portable-pty can use; asking here rather than assuming
    // means a truncated or mismatched file is reported as such instead of
    // failing later, once per tab, as "the terminal did not start".
    out.missing = match exports_pseudo_console(&path) {
        None => Some(Missing::Unloadable),
        Some(false) => Some(Missing::Incomplete),
        Some(true) => match dir.join("OpenConsole.exe").is_file() {
            false => Some(Missing::NoOpenConsole),
            true => None,
        },
    };
    out.bundled = out.missing.is_none();
    out
}

fn wide(s: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// `None` = it would not load at all; `Some(false)` = it loaded without the
/// functions we need.
///
/// The handle is deliberately never freed. portable-pty is about to load the
/// same file by name, and holding the reference costs one module in a process
/// that is going to hold it anyway -- while unloading it here would mean the
/// file could be swapped between our answer and its question.
fn exports_pseudo_console(path: &std::path::Path) -> Option<bool> {
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LoadLibraryExW,
    };
    let w = wide(path);
    // Search beside the DLL and in System32, and nowhere else: this is a full
    // path, and the flags keep whatever it depends on from being answered by
    // the current directory.
    let module = unsafe {
        LoadLibraryExW(
            w.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        return None;
    }
    let has = |name: &[u8]| unsafe { GetProcAddress(module, name.as_ptr()).is_some() };
    Some(
        has(b"CreatePseudoConsole\0")
            && has(b"ResizePseudoConsole\0")
            && has(b"ClosePseudoConsole\0"),
    )
}

/// The file's own version ("1.24.2607.10001"), or an empty string.
///
/// Worth reading rather than reciting the number we pinned at build time: the
/// point of showing it is to catch the case where the file beside the exe is
/// not the file we think we shipped.
fn file_version(path: &std::path::Path) -> String {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let w = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(w.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return String::new();
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, size, buf.as_mut_ptr().cast()) == 0 {
            return String::new();
        }
        let mut value: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len: u32 = 0;
        let root: Vec<u16> = "\\\0".encode_utf16().collect();
        if VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut value, &mut len) == 0
            || (len as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return String::new();
        }
        let info = &*(value as *const VS_FIXEDFILEINFO);
        format!(
            "{}.{}.{}.{}",
            info.dwFileVersionMS >> 16,
            info.dwFileVersionMS & 0xFFFF,
            info.dwFileVersionLS >> 16,
            info.dwFileVersionLS & 0xFFFF
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever this build has beside it, the answer has to be one of the two
    /// shapes -- with a reason whenever it is not the good one, and a path in
    /// both cases so a person can go and look.
    #[test]
    fn the_answer_is_always_sayable() {
        let r = report();
        assert_eq!(r.bundled, r.missing.is_none());
        assert!(r.path.ends_with("conpty.dll"), "どこを見たかは常に言う");
        let line = r.line();
        assert!(line.starts_with("ConPTY: "));
        if let Some(m) = r.missing {
            assert!(line.contains(m.id()), "理由が読める: {line}");
        }
    }

    /// A missing file is "not shipped", not "unloadable": the two are fixed in
    /// different places (fetch it, versus the wrong architecture arrived).
    #[test]
    fn nothing_there_is_not_the_same_as_something_broken() {
        let nowhere = std::path::Path::new("Z:\\no-such-folder\\conpty.dll");
        assert_eq!(exports_pseudo_console(nowhere), None);
        assert_eq!(file_version(nowhere), "");
    }

    /// kernel32 exports all three -- it is where the in-box ConPTY lives -- so
    /// it stands in for "a DLL that really does answer for these functions"
    /// without needing the redistributable to be present on the test machine.
    #[test]
    fn a_library_that_has_the_functions_is_recognised() {
        let k = std::path::Path::new("kernel32.dll");
        assert_eq!(exports_pseudo_console(k), Some(true));
    }
}
