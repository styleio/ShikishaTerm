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

/// The ask to the system, held or not. One per thread that asks; dropping it
/// lets go.
#[derive(Debug, Default)]
pub struct Awake {
    held: bool,
    /// The last ask to hold was refused, or this system has no way to be
    /// asked: the PC may sleep whatever the setting says, and the row says so
    /// rather than claiming it is being kept up
    cannot: bool,
    /// What holds a Linux machine up: a child that keeps an inhibitor lock
    /// for as long as it lives
    #[cfg(target_os = "linux")]
    lock: Option<std::process::Child>,
}

impl Awake {
    /// Whether the PC is being kept up right now
    pub fn held(&self) -> bool {
        self.held
    }

    /// Whether the system turned the last ask down, or cannot be asked at all
    pub fn cannot(&self) -> bool {
        self.cannot
    }

    /// Holds or lets go. Says whether anything changed, so the caller can
    /// write the change down once instead of every frame. An ask to hold that
    /// the system refuses leaves it not held -- never said to be held
    pub fn hold(&mut self, on: bool) -> bool {
        self.hold_by(on, Self::ask)
    }

    /// `hold`, with the ask to the system handed in: the one piece that
    /// differs per system, and the one a test cannot make on every machine
    fn hold_by(&mut self, on: bool, ask: impl FnOnce(&mut Self, bool) -> bool) -> bool {
        if on == self.held {
            // Wanting nothing is never refused
            if !on && self.cannot {
                self.cannot = false;
                return true;
            }
            return false;
        }
        if ask(self, on) {
            self.held = on;
            self.cannot = false;
            true
        } else {
            let changed = on && !self.cannot;
            self.cannot = on;
            changed
        }
    }

    /// Tells Windows what this thread needs. The screen is kept on as well as
    /// the machine: a PC whose screen goes dark is, on many laptops, a PC that
    /// then goes to standby whatever the thread asked, and a person coming
    /// back to check on the AI wants to see it
    #[cfg(windows)]
    fn ask(&mut self, on: bool) -> bool {
        use windows_sys::Win32::System::Power::{
            ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
        };
        let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        // SAFETY: takes flags and returns the previous state; 0 is failure
        unsafe { SetThreadExecutionState(flags) != 0 }
    }

    /// A Linux machine is held up by systemd's inhibitor lock, taken by a
    /// child that keeps it until its input closes: letting go is closing it,
    /// and a program that dies lets go by dying, with nothing left behind.
    /// A machine without systemd-logind cannot be asked, and says so
    #[cfg(target_os = "linux")]
    fn ask(&mut self, on: bool) -> bool {
        if !on {
            if let Some(mut child) = self.lock.take() {
                // Closing its input ends it; waited for so no zombie is left
                drop(child.stdin.take());
                let _ = child.wait();
            }
            return true;
        }
        let (program, args) = inhibit_command();
        match std::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // One that could not take the lock ends at once: looked at a
                // moment later so that is seen, not taken for holding
                std::thread::sleep(std::time::Duration::from_millis(150));
                match child.try_wait() {
                    Ok(None) => {
                        self.lock = Some(child);
                        true
                    }
                    _ => false,
                }
            }
            Err(_) => false,
        }
    }

    /// Anywhere else there is no ask this program knows how to make
    #[cfg(not(any(windows, target_os = "linux")))]
    fn ask(&mut self, on: bool) -> bool {
        !on
    }
}

/// What takes the lock on Linux: the sleep and the idle action both, for as
/// long as `cat` has its input open. The lid is left to the machine's own
/// settings, as it is on Windows
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn inhibit_command() -> (&'static str, Vec<String>) {
    (
        "systemd-inhibit",
        vec![
            "--what=idle:sleep".into(),
            "--who=SHIKISHA-TERM".into(),
            format!("--why={}", crate::i18n::t("tui.awake.now.held")),
            "--mode=block".into(),
            "cat".into(),
        ],
    )
}

impl Drop for Awake {
    fn drop(&mut self) {
        if self.held {
            self.ask(false);
        }
    }
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
        let yes = |_: &mut Awake, _: bool| true;
        let mut a = Awake::default();
        assert!(a.hold_by(true, yes));
        assert!(!a.hold_by(true, yes), "holding again counted as a change");
        assert!(a.held());
        assert!(a.hold_by(false, yes));
        assert!(!a.held());
    }

    /// A system that turns the ask down -- or one this program cannot ask --
    /// leaves the PC not held, and says it cannot, instead of claiming it is
    /// being kept up
    #[test]
    fn a_refused_ask_is_never_said_to_be_held() {
        let no = |_: &mut Awake, on: bool| !on;
        let mut a = Awake::default();
        assert!(a.hold_by(true, no), "the refusal was not a change to say");
        assert!(!a.held(), "a refused ask was taken for holding");
        assert!(a.cannot());
        assert!(!a.hold_by(true, no), "the same refusal was said again every pass");
        assert!(a.hold_by(false, no), "wanting nothing did not clear the refusal");
        assert!(!a.cannot());
    }

    /// The command that holds a Linux machine up: it blocks sleep and the idle
    /// action, and lives while its input is open
    #[test]
    fn linux_is_held_by_an_inhibitor_that_lives_on_its_input() {
        let (program, args) = inhibit_command();
        assert_eq!(program, "systemd-inhibit");
        assert!(args.iter().any(|a| a == "--what=idle:sleep"));
        assert!(args.iter().any(|a| a == "--mode=block"));
        assert_eq!(args.last().map(String::as_str), Some("cat"), "nothing keeps the lock while the input is open");
    }

    /// On a system that can be asked, holding and letting go really go through
    #[cfg(windows)]
    #[test]
    fn windows_is_asked_and_let_go() {
        let mut a = Awake::default();
        assert!(a.hold(true));
        assert!(a.held());
        assert!(a.hold(false));
        assert!(!a.held());
    }
}
