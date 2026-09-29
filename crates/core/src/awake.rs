//! Keeping this PC from going to sleep while an AI is at work.
//!
//! A turn that takes forty minutes is a turn somebody walks away from, and a
//! PC that sleeps under it stops the AI half way: the terminal is frozen, the
//! network drops, and whatever it was waiting on times out. The setting
//! chooses when to ask Windows to stay up -- never, while an AI tab is at
//! work, or for as long as this program runs.
//!
//! The ask is `SetThreadExecutionState`, which belongs to the thread that
//! made it and ends with that thread. It is made from the loop that owns the
//! terminals ([`crate::runtime::run`]), so a crash or a kill hands the PC back
//! to its own power settings without anybody having to remember to.
//!
//! What a closed lid does is left to Windows: keeping a PC up with the lid
//! down means changing the power plan the whole machine runs on, and that is
//! not a setting for this program to change behind somebody's back.

/// When to keep the PC up (`stay_awake` in the settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stay {
    /// Leave it to Windows
    #[default]
    Off,
    /// While an AI tab is working on a turn
    WhileAi,
    /// For as long as this program runs
    Always,
}

impl Stay {
    /// The value as the settings file spells it. Anything unknown reads as
    /// `Off`: a word this build does not know must not keep a laptop awake
    pub fn parse(s: Option<&str>) -> Stay {
        match s.map(str::trim) {
            Some("ai") => Stay::WhileAi,
            Some("always") => Stay::Always,
            _ => Stay::Off,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Stay::Off => "off",
            Stay::WhileAi => "ai",
            Stay::Always => "always",
        }
    }
}

/// Whether to keep the PC up now, with `working` AI tabs working on a turn
/// on any desk (every desk's state is kept current: `Tab::tick_away`)
pub fn wanted(stay: Stay, working: usize) -> bool {
    match stay {
        Stay::Off => false,
        Stay::Always => true,
        Stay::WhileAi => working > 0,
    }
}

/// The ask to Windows, held or not. One per thread that asks; dropping it
/// lets go.
#[derive(Debug, Default)]
pub struct Awake {
    held: bool,
}

impl Awake {
    /// Whether the PC is being kept up right now
    pub fn held(&self) -> bool {
        self.held
    }

    /// Holds or lets go. Says whether anything changed, so the caller can
    /// write the change down once instead of every frame
    pub fn hold(&mut self, on: bool) -> bool {
        if on == self.held {
            return false;
        }
        if ask(on) {
            self.held = on;
            true
        } else {
            false
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        if self.held {
            ask(false);
        }
    }
}

/// Tells Windows what this thread needs. The screen is kept on as well as
/// the machine: a PC whose screen goes dark is, on many laptops, a PC that
/// then goes to standby whatever the thread asked, and a person coming back
/// to check on the AI wants to see it
#[cfg(windows)]
fn ask(on: bool) -> bool {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
    // SAFETY: takes flags and returns the previous state; 0 is failure
    unsafe { SetThreadExecutionState(flags) != 0 }
}

#[cfg(not(windows))]
fn ask(_on: bool) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_reads_back_and_an_unknown_word_is_off() {
        for s in [Stay::Off, Stay::WhileAi, Stay::Always] {
            assert_eq!(Stay::parse(Some(s.key())), s);
        }
        assert_eq!(Stay::parse(None), Stay::Off);
        assert_eq!(Stay::parse(Some("forever")), Stay::Off, "an unknown word kept the PC up");
    }

    #[test]
    fn the_pc_is_kept_up_only_when_the_setting_and_the_tabs_say_so() {
        assert!(!wanted(Stay::Off, 3), "off kept the PC up");
        assert!(wanted(Stay::Always, 0), "always let it sleep");
        assert!(wanted(Stay::WhileAi, 1));
        assert!(!wanted(Stay::WhileAi, 0), "nothing at work, and it stayed up");
    }

    #[test]
    fn holding_twice_is_one_change_and_letting_go_is_another() {
        let mut a = Awake::default();
        assert!(a.hold(true));
        assert!(!a.hold(true), "holding again counted as a change");
        assert!(a.held());
        assert!(a.hold(false));
        assert!(!a.held());
    }
}
