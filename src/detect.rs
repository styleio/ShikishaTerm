//! State-detection engine: overlays several independent signals (screen
//! pattern / bell / silence timer) into a state machine that decides tab
//! state. DESIGN.md ch. 4.2.

use crate::profile::Profile;

/// If there was output within this many ms, treat it as "active"
const ACTIVITY_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabState {
    /// Yellow: processing (output is flowing / matched a BUSY pattern)
    Busy,
    /// Green: response complete (silence or bell after activity)
    Done,
    /// Blue: waiting on a choice/confirmation (matched a QUESTION pattern)
    Question,
    /// Blue: idle (no activity)
    Wait,
    /// Red: child process exited (set by Tab, not by Detector)
    Exited,
}

impl TabState {
    /// Stable internal name (used by logging, automation, remote UI judgments)
    pub fn label(&self) -> &'static str {
        match self {
            TabState::Busy => "BUSY",
            TabState::Done => "DONE",
            TabState::Question => "QUESTION",
            TabState::Wait => "WAIT",
            TabState::Exited => "EXIT",
        }
    }

    /// Name shown on screen (translated)
    pub fn display(&self) -> String {
        crate::i18n::t(match self {
            TabState::Busy => "state.busy",
            TabState::Done => "state.done",
            TabState::Question => "state.question",
            TabState::Wait => "state.wait",
            TabState::Exited => "state.exit",
        })
    }
}

pub struct Detector {
    profile: Profile,
    state: TabState,
    /// Whether there was "activity" recently (Done only fires on the activity-to-silence transition)
    was_active: bool,
    /// Whether the "working" indicator was showing on screen at the last tick.
    ///
    /// This is distinct from whether the screen changed. Rendering a paste
    /// also changes the screen, but the AI hasn't done anything. This is
    /// the actual evidence that it started working
    working_shown: bool,
    /// The string that actually matched (to trace false positives)
    working_matched: Option<String>,
    last_bell: u64,
}

impl Detector {
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            state: TabState::Wait,
            was_active: false,
            working_shown: false,
            working_matched: None,
            last_bell: 0,
        }
    }

    pub fn profile_name(&self) -> &str {
        &self.profile.name
    }

    /// Number of bottom rows excluded from screen-change detection (works around status bars like byobu's)
    pub fn ignore_bottom_rows(&self) -> u16 {
        self.profile.ignore_bottom_rows
    }

    /// Whether the "working" indicator was showing at the last tick
    pub fn working_shown(&self) -> bool {
        self.working_shown
    }

    /// The string that triggered the "working" judgment (to trace false positives)
    pub fn working_matched(&self) -> Option<&str> {
        self.working_matched.as_deref()
    }

    /// Whether this AI shows a "working" indicator on screen (whether the profile specifies one)
    pub fn shows_working(&self) -> bool {
        !self.profile.busy.is_empty()
    }

    /// This AI's own confirmation delay, if specified
    pub fn done_confirm_ms(&self) -> Option<u64> {
        self.profile.done_confirm_ms
    }

    /// Runs periodically (roughly every 200ms).
    /// Priority: QUESTION > BUSY (pattern) > bell completion > activity timer > silence timer
    pub fn tick(&mut self, screen_text: &str, ms_since_output: u64, bell_count: u64) -> TabState {
        let bell_rang = bell_count > self.last_bell;
        self.last_bell = bell_count;

        if self.profile.question.iter().any(|r| r.is_match(screen_text)) {
            self.state = TabState::Question;
            return self.state;
        }
        // Keep not just what matched, but the whole line it's on.
        // Looking at just the word can't tell decoration from the real thing
        self.working_matched = self.profile.busy.iter().find_map(|r| {
            r.find(screen_text).map(|m| {
                let head = screen_text[..m.start()].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let tail = screen_text[m.end()..]
                    .find('\n')
                    .map(|i| m.end() + i)
                    .unwrap_or(screen_text.len());
                screen_text[head..tail].trim().to_string()
            })
        });
        self.working_shown = self.working_matched.is_some();
        if self.working_shown {
            self.was_active = true;
            self.state = TabState::Busy;
            return self.state;
        }
        if bell_rang && self.was_active {
            self.was_active = false;
            self.state = TabState::Done;
            return self.state;
        }
        if ms_since_output < ACTIVITY_MS {
            self.was_active = true;
            self.state = TabState::Busy;
        } else if ms_since_output >= self.profile.silence_ms {
            if self.was_active {
                self.was_active = false;
                self.state = TabState::Done;
            } else if self.state == TabState::Busy {
                self.state = TabState::Wait;
            }
            // Done / Wait are held until the next activity
        }
        // Between ACTIVITY_MS and silence_ms, hold the previous state (prevents flicker)
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Profile, ProfileFile};

    fn claude_like() -> Profile {
        Profile::compile(ProfileFile {
            name: "test".into(),
            command_match: vec![],
            busy_patterns: vec!["esc to interrupt".into()],
            question_patterns: vec!["Do you want".into(), "❯\\s*1\\.".into()],
            silence_ms: 2000,
            ignore_bottom_rows: 2,
            done_confirm_ms: None,
        })
        .unwrap()
    }

    /// Confirms the screen changing isn't confused with the AI starting to work.
    ///
    /// The screen changes even when a paste is only re-rendered into the
    /// `[Pasted Content …]` form. Counting that as the start of a response
    /// would pass the ball even though execution never actually happened
    #[test]
    fn a_redrawn_paste_is_not_the_ai_starting_work() {
        let mut d = Detector::new(claude_like());

        // Just a paste being re-rendered. No "working" indicator
        d.tick("> [Pasted Content 1917 chars]", 0, 0);
        assert!(
            !d.working_shown(),
            "貼り付けの描き変わりを「働き始めた」と数えている"
        );

        // The indicator shows once it actually starts working
        d.tick("Thinking… (12s · esc to interrupt)", 0, 0);
        assert!(d.working_shown(), "作業中の表示を見落としている");

        // Once the indicator disappears, it's not working again
        d.tick("> [Pasted Content 1917 chars]", 3000, 0);
        assert!(!d.working_shown(), "表示が消えたら働いていない");

        // Also confirms whether the profile has a "working" indicator at all
        assert!(d.shows_working(), "このAIは作業中を画面に出す");
    }

    #[test]
    fn busy_pattern_wins_over_silence() {
        let mut d = Detector::new(claude_like());
        assert_eq!(d.tick("... esc to interrupt ...", 10_000, 0), TabState::Busy);
    }

    #[test]
    fn question_has_top_priority() {
        let mut d = Detector::new(claude_like());
        let screen = "Do you want to proceed?\n❯ 1. Yes\n  2. No";
        assert_eq!(d.tick(screen, 100, 0), TabState::Question);
    }

    #[test]
    fn activity_then_silence_becomes_done() {
        let mut d = Detector::new(claude_like());
        assert_eq!(d.tick("output flowing", 100, 0), TabState::Busy);
        assert_eq!(d.tick("output stopped", 3000, 0), TabState::Done);
        // Holds Done unless activity happens in between
        assert_eq!(d.tick("output stopped", 10_000, 0), TabState::Done);
    }

    #[test]
    fn bell_after_activity_means_done() {
        let mut d = Detector::new(claude_like());
        d.tick("working", 100, 0);
        assert_eq!(d.tick("finished", 600, 1), TabState::Done);
    }

    #[test]
    fn grace_period_keeps_previous_state() {
        let mut d = Detector::new(claude_like());
        d.tick("working", 100, 0);
        // Between 500ms and 2000ms, hold the judgment and stay Busy (prevents flicker)
        assert_eq!(d.tick("quiet", 1000, 0), TabState::Busy);
    }

    #[test]
    fn generic_profile_uses_timers_only() {
        let mut d = Detector::new(Profile::generic());
        assert_eq!(d.tick("anything", 100, 0), TabState::Busy);
        assert_eq!(d.tick("anything", 3000, 0), TabState::Done);
    }
}

