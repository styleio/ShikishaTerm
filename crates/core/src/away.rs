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

/// When the person last did something on this board from a browser at a
/// desk -- another computer's, or this one's -- as against from a phone
static AT_A_DESK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// The person just did something on the board from a browser at a desk.
///
/// The machine the app runs on is not always the one the person sits at: a
/// server has no keyboard to be idle and no screen to be locked, and "away"
/// was all it could ever answer -- so the phone of somebody working the
/// board from their laptop, all day, was told everything, with "only while
/// away" ticked. Somebody working the board from a desk is not away from it
pub fn seen_at_a_desk() {
    *AT_A_DESK.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
}

/// Whether what a browser calls itself (its `User-Agent`) is a browser at a
/// desk: not a phone's or a tablet's
pub fn is_a_desk(calls_itself: &str) -> bool {
    !calls_itself.is_empty() && !["Mobi", "Android", "iPhone", "iPad", "iPod"].iter().any(|word| calls_itself.contains(word))
}

/// Whether the person is away from this PC now
pub fn person_away() -> bool {
    if AT_A_DESK.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|at| at.elapsed() < IDLE) {
        return false;
    }
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

    /// Somebody working the board from a browser at a desk is not away,
    /// whatever the machine the app runs on can say; a phone is not a desk
    #[test]
    fn somebody_at_a_desk_is_not_away() {
        assert!(super::is_a_desk("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36"));
        assert!(super::is_a_desk("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"));
        assert!(!super::is_a_desk("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"));
        assert!(!super::is_a_desk("Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36"));
        assert!(!super::is_a_desk(""), "a caller that names nothing was taken for a desk");
        super::seen_at_a_desk();
        assert!(!super::person_away(), "somebody at a desk was counted as away");
        *super::AT_A_DESK.lock().unwrap() = std::time::Instant::now().checked_sub(super::IDLE + std::time::Duration::from_secs(1));
        if super::idle().is_none() && super::locked().is_none() {
            assert!(super::person_away(), "somebody who left the desk a while ago is still counted there");
        }
        *super::AT_A_DESK.lock().unwrap() = None;
    }
}
