//! Whether the person is at this PC or away from it.
//!
//! A phone that buzzes with what an AI said, while its owner sits in front of
//! the window the AI is in, says it twice -- and a phone that buzzes all day
//! is a phone that gets its notifications turned off. So a phone can be told
//! only while the person is away: the screen locked, or nothing typed or
//! moved on this PC for a while.
//!
//! Asked of the system each time, never remembered: it changes every time the
//! person gets up. Where the system has no way to say, the answer is "away",
//! so the phone is told as it was before anything asked.

use std::time::Duration;

/// How long without a key or the pointer before the person counts as away.
/// Long enough for reading a page without touching anything, short enough
/// that somebody who went to make tea hears about it
pub const IDLE: Duration = Duration::from_secs(180);

/// Whether the person is away from this PC now
pub fn person_away() -> bool {
    match (locked(), idle()) {
        (Some(true), _) => true,
        (_, Some(idle)) => idle >= IDLE,
        // Nothing to go by: told as before
        (_, None) => true,
    }
}

/// How long since the last key or pointer movement on this PC, when the
/// system says
#[cfg(windows)]
pub fn idle() -> Option<Duration> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: the struct is ours, sized as the call asks
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // Both counts wrap together every 49.7 days; the difference does not
    let now = unsafe { GetTickCount() };
    Some(Duration::from_millis(now.wrapping_sub(info.dwTime) as u64))
}

/// The same on a Mac, from the HID system's idle time (nanoseconds)
#[cfg(target_os = "macos")]
pub fn idle() -> Option<Duration> {
    let out = std::process::Command::new("ioreg").args(["-c", "IOHIDSystem", "-d", "4"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains("\"HIDIdleTime\""))?;
    let ns: u64 = line.rsplit('=').next()?.trim().parse().ok()?;
    Some(Duration::from_nanos(ns))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn idle() -> Option<Duration> {
    None
}

/// Whether this session's screen is locked, when the system says
#[cfg(windows)]
pub fn locked() -> Option<bool> {
    use windows_sys::Win32::System::RemoteDesktop::{
        WTS_CURRENT_SESSION, WTSFreeMemory, WTSINFOEXW, WTSQuerySessionInformationW, WTSSessionInfoEx,
    };
    let mut buf: windows_sys::core::PWSTR = std::ptr::null_mut();
    let mut len: u32 = 0;
    // SAFETY: the system fills the buffer it hands back, freed below
    let ok = unsafe { WTSQuerySessionInformationW(std::ptr::null_mut(), WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buf, &mut len) };
    if ok == 0 || buf.is_null() || (len as usize) < std::mem::size_of::<WTSINFOEXW>() {
        return None;
    }
    // SAFETY: a WTSINFOEXW, as asked for, at least as long as one
    let flags = unsafe {
        let info = &*(buf as *const WTSINFOEXW);
        let flags = if info.Level == 1 { Some(info.Data.WTSInfoExLevel1.SessionFlags) } else { None };
        WTSFreeMemory(buf as *mut _);
        flags
    };
    // WTS_SESSIONSTATE_LOCK is 0, WTS_SESSIONSTATE_UNLOCK 1; anything else is unknown
    match flags? {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn locked() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    /// The system is asked and answers something sensible: an idle time that
    /// is not in the future, and a session that is or is not locked
    #[test]
    fn the_system_can_be_asked() {
        if let Some(idle) = super::idle() {
            assert!(idle < std::time::Duration::from_secs(60 * 60 * 24 * 50), "an idle time past the counter's wrap: {idle:?}");
        }
        #[cfg(windows)]
        assert!(super::idle().is_some(), "Windows did not say how long since the last input");
        let _ = super::locked();
    }
}
