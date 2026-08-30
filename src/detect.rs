//! State-detection engine: overlays several independent signals (screen
//! pattern / bell / silence timer / the program's own word) into a state
//! machine that decides tab state. DESIGN.md ch. 4.2.

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

    /// A state named from outside this process, read back.
    ///
    /// `EXIT` is deliberately not here. A program is dead when its process is
    /// dead, and nothing else -- an announcement to the contrary, from a hook
    /// or a script, would paint a tab red while the thing in it is still
    /// running
    pub fn from_label(name: &str) -> Option<TabState> {
        Some(match name.trim().to_ascii_uppercase().as_str() {
            "BUSY" => TabState::Busy,
            "DONE" => TabState::Done,
            "QUESTION" => TabState::Question,
            "WAIT" => TabState::Wait,
            _ => return None,
        })
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

/// The end of a turn can go missing: pressing Ctrl+C or Esc is something a
/// person does to a CLI, so most of them have no event for it, and the app can
/// also be started in the middle of a turn it never saw begin. A dot stuck on
/// "working" for the rest of the session is worse than a dot that goes back to
/// guessing, so a word that says "working" has two ways to stop standing.
///
/// This is the one for a CLI that shows nothing while it works: a full minute
/// without a single character on screen.
const BUSY_STALL_MS: u64 = 60_000;

/// What the program in the tab said about itself, in its own words.
#[derive(Debug, Clone, Copy)]
struct Word {
    state: TabState,
    /// The sender's clock. Hooks are separate processes, told not to block, so
    /// the order they were SENT is the only order that means anything -- two
    /// of them can arrive the other way round
    sent_ms: u64,
    /// Whether the screen has been still once since it was said.
    ///
    /// A dialog is being painted at the very moment its hook fires, so
    /// "the screen moved" can only mean "the person answered" after the
    /// screen has stopped moving at least once
    settled: bool,
}

pub struct Detector {
    profile: Profile,
    /// The screen's own judgment. Kept apart from what `tick` returns so that
    /// a hook's word can outrank it without rewriting the history the screen
    /// logic reasons from
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
    /// The last thing the program said about itself, while it still stands
    word: Option<Word>,
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
            word: None,
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

    /// The program's own word about what it is doing, through its hook.
    ///
    /// `sent_ms` is the sender's clock at the moment it was said. Returns
    /// whether it was taken: a report older than the one already in hand is
    /// dropped rather than applied, because these arrive by way of separate
    /// processes that were told not to wait for each other -- "started
    /// working" landing after "finished" would otherwise leave a tab busy
    /// forever, and the two are milliseconds apart when a turn is short
    pub fn hook_says(&mut self, state: TabState, sent_ms: u64) -> bool {
        if self.word.is_some_and(|w| sent_ms < w.sent_ms) {
            return false;
        }
        self.word = Some(Word { state, sent_ms, settled: false });
        true
    }

    /// What the program last said about itself, while it still stands
    pub fn hook_word(&self) -> Option<TabState> {
        self.word.map(|w| w.state)
    }

    /// The word, if it still stands -- and this is also where it stops
    /// standing.
    ///
    /// Every CLI worth hooking announces the beginning of a turn, the opening
    /// of a permission dialog and the end of a turn, and **none of them
    /// announce the answer to the dialog**: approving, refusing and pressing
    /// Ctrl+C are things a person does to the CLI, not things the CLI does.
    /// So the rising edge comes from the hook and the falling edge comes from
    /// the screen: output moving again IS the answer. A tab left saying
    /// "waiting for you" after you already answered is worse than no dot at
    /// all, so nothing here is allowed to depend on an event that never comes.
    fn word_now(&mut self, ms_since_output: u64) -> Option<TabState> {
        // Read before the word is borrowed: this is the screen's evidence that
        // a turn is NOT running, and it is only evidence for the CLIs that
        // show something while one is. Those redraw a counting clock every
        // second, so a screen frozen this long with no indicator on it is not
        // a CLI at work -- it is the end of the turn having gone unreported
        let indicator_gone = !self.profile.busy.is_empty() && !self.working_shown;
        let long_enough = self
            .profile
            .done_confirm_ms
            .unwrap_or(crate::profile::DEFAULT_DONE_CONFIRM_MS);
        let w = self.word.as_mut()?;
        let still = ms_since_output >= ACTIVITY_MS;
        w.settled |= still;
        let spent = match w.state {
            // Answered: it had gone quiet waiting for a person, and now it is
            // moving again
            TabState::Question => w.settled && !still,
            // Working, with the end of the turn never delivered
            TabState::Busy => {
                (indicator_gone && ms_since_output >= long_enough)
                    || ms_since_output > BUSY_STALL_MS
            }
            // Resting states. They stand until the program says otherwise --
            // which it will, at the next prompt
            _ => false,
        };
        if spent {
            self.word = None;
            return None;
        }
        Some(w.state)
    }

    /// Runs periodically (roughly every 200ms).
    ///
    /// Priority: a question on screen > the program's own word > the screen's
    /// own reading of it. The word outranks the screen because it is not a
    /// reading at all -- the CLI fired it the instant the turn began or ended,
    /// in whatever language, under whatever theme, whether or not it happened
    /// to draw a word this app knows. A question on screen outranks even that,
    /// because not every question a CLI asks is one it reports (a menu, a
    /// login, a trust prompt), and a tab that says "working" while it is in
    /// fact waiting for a person is the one mistake with no way back: nobody
    /// goes to look at a tab that claims to be busy.
    pub fn tick(&mut self, screen_text: &str, ms_since_output: u64, bell_count: u64) -> TabState {
        let screen = self.screen_tick(screen_text, ms_since_output, bell_count);
        // Bookkeeping runs every tick, whichever signal ends up winning, so
        // that whoever takes over is never handed a stale reading
        let word = self.word_now(ms_since_output);
        if screen == TabState::Question {
            return screen;
        }
        word.unwrap_or(screen)
    }

    /// The screen's own reading, on its own. Priority within it:
    /// QUESTION > BUSY (pattern) > bell completion > activity timer > silence timer
    fn screen_tick(&mut self, screen_text: &str, ms_since_output: u64, bell_count: u64) -> TabState {
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
            resume: None,
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

    /// The real profile, read from `profiles/codex.json`, against the three
    /// screens that mattered.
    ///
    /// Codex stops on first launch in a folder and asks whether it is trusted.
    /// That prompt reads "…**Working** with untrusted contents comes with
    /// higher risk of prompt injection", so a busy pattern of bare `Working`
    /// showed a spinner on a tab that was in fact waiting for a person — and
    /// the same pattern hit any line of Codex's own output with the word in it,
    /// which does not go away, so the tab stayed "busy" for good.
    #[test]
    fn codexs_trust_prompt_is_a_question_and_not_work() {
        let profile = crate::profile::load_for_command("codex");
        let mut d = Detector::new(profile);

        let trust = "  Do you trust the contents of this directory? Working with untrusted \
                     contents comes with higher risk of prompt injection.\n\n› 1. Yes\n  2. No";
        assert_eq!(d.tick(trust, 100, 0), TabState::Question, "人を待っている");

        let diff = "28 +    Write-Host \"Working tree is not clean. Commit or discard changes.\"";
        d.tick(diff, 5_000, 0);
        assert!(!d.working_shown(), "出力に混ざった語で作業中にしている");

        let spinner = "• Working (0s • esc to interrupt)";
        d.tick(spinner, 100, 0);
        assert!(d.working_shown(), "本物の作業中表示を見落としている");
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

    /// The whole point of asking the CLI: it says it is working, and it stays
    /// working through a silence that the screen alone reads as "finished".
    ///
    /// This is the everyday case, not an edge one — the silence timer is two
    /// seconds, and any pause longer than that used to end the turn, hand the
    /// ball on, and fire on_done on a response that had not been written yet
    #[test]
    fn a_program_that_says_it_is_working_is_not_finished_by_silence() {
        let mut d = Detector::new(claude_like());
        d.tick("thinking", 100, 0);
        assert!(d.hook_says(TabState::Busy, 1_000));
        // The screen alone calls this finished. It is not
        assert_eq!(d.tick("nothing moving", 3_000, 0), TabState::Busy);
        assert_eq!(d.hook_word(), Some(TabState::Busy));
    }

    /// ...and the other half: the end of a turn is the CLI saying so, not the
    /// screen going quiet for two seconds
    #[test]
    fn the_end_of_a_turn_is_taken_from_the_program_at_once() {
        let mut d = Detector::new(claude_like());
        assert!(d.hook_says(TabState::Busy, 1_000));
        assert_eq!(d.tick("... esc to interrupt ...", 100, 0), TabState::Busy);
        assert!(d.hook_says(TabState::Done, 2_000));
        // The screen still holds the last frame of the spinner, and it no
        // longer matters
        assert_eq!(d.tick("... esc to interrupt ...", 100, 0), TabState::Done);
    }

    /// Hooks are separate processes told not to block each other, so a short
    /// turn can deliver "finished" and "started" in that order. Applying them
    /// as they land would leave the tab working with nothing running
    #[test]
    fn a_report_that_arrives_late_does_not_undo_a_newer_one() {
        let mut d = Detector::new(claude_like());
        assert!(d.hook_says(TabState::Done, 5_000));
        assert!(!d.hook_says(TabState::Busy, 4_900), "古い報告は適用しない");
        assert_eq!(d.tick("quiet", 3_000, 0), TabState::Done);
        // A genuinely newer one still gets through
        assert!(d.hook_says(TabState::Busy, 5_001));
        assert_eq!(d.tick("quiet", 3_000, 0), TabState::Busy);
    }

    /// Nothing is sent when a person approves, refuses or interrupts — the
    /// only event a CLI has to offer is the one that opened the dialog. So the
    /// answer has to be read off the screen, or the tab asks forever
    #[test]
    fn a_dialog_stops_waiting_when_the_screen_moves_again() {
        let mut d = Detector::new(claude_like());
        assert!(d.hook_says(TabState::Question, 1_000));
        // The dialog is still being painted as its hook fires
        assert_eq!(d.tick("Allow Bash?", 0, 0), TabState::Question);
        // Nobody has answered yet
        assert_eq!(d.tick("Allow Bash?", 8_000, 0), TabState::Question);
        // Answered: output moves again
        assert_eq!(d.tick("running npm test", 100, 0), TabState::Busy);
        assert_eq!(d.hook_word(), None, "答えたら待ちは消える");
    }

    /// A question on screen outranks even the program's own word: not every
    /// question a CLI asks is one it reports, and "busy" on a tab that is in
    /// fact waiting for a person is the mistake nobody goes back to check
    #[test]
    fn a_question_on_screen_wins_over_a_word_that_says_working() {
        let mut d = Detector::new(claude_like());
        assert!(d.hook_says(TabState::Busy, 1_000));
        assert_eq!(d.tick("Do you want to proceed?\n❯ 1. Yes", 100, 0), TabState::Question);
        // The word is not thrown away by it: the menu belongs to a turn that
        // is still running, so the turn is still running once it is answered
        assert_eq!(d.hook_word(), Some(TabState::Busy), "画面の問いは言葉を捨てない");
        assert_eq!(d.tick("carrying on", 100, 0), TabState::Busy);
    }

    /// Pressing Esc is something a person does to a CLI, and most of them have
    /// no event for it: the turn ends and the last thing anyone said about it
    /// was "working". A CLI that shows a spinner is telling us the truth by
    /// not showing one, and after long enough that outweighs a word nobody
    /// came back to correct.
    #[test]
    fn an_interrupted_turn_is_noticed_by_the_missing_spinner() {
        let mut d = Detector::new(claude_like());
        d.tick("Thinking… (3s · esc to interrupt)", 100, 0);
        assert!(d.hook_says(TabState::Busy, 1_000));
        // Interrupted: the spinner is gone and the screen has stopped moving
        assert_eq!(d.tick("[Request interrupted by user]", 4_000, 0), TabState::Busy);
        assert_eq!(
            d.tick("[Request interrupted by user]", 11_000, 0),
            TabState::Done,
            "画面の判断に戻る"
        );
        assert_eq!(d.hook_word(), None);
    }

    /// A CLI that shows nothing while it works gives the screen no evidence to
    /// weigh, so the only thing left is a stall long enough that no turn could
    /// still be running behind it
    #[test]
    fn with_nothing_to_show_a_word_stands_until_the_screen_is_long_dead() {
        let mut d = Detector::new(Profile::generic());
        d.tick("working", 100, 0);
        assert!(d.hook_says(TabState::Busy, 1_000));
        assert_eq!(d.tick("nothing", 30_000, 0), TabState::Busy, "無表示のCLIは黙って働く");
        assert_eq!(d.tick("nothing", 61_000, 0), TabState::Done, "画面の判断に戻る");
        assert_eq!(d.hook_word(), None);
    }

    #[test]
    fn only_states_this_app_has_a_name_for_are_accepted() {
        assert_eq!(TabState::from_label("busy"), Some(TabState::Busy));
        assert_eq!(TabState::from_label(" DONE "), Some(TabState::Done));
        assert_eq!(TabState::from_label("QUESTION"), Some(TabState::Question));
        assert_eq!(TabState::from_label("WAIT"), Some(TabState::Wait));
        // A program is dead when its process is dead, and not because
        // something said so
        assert_eq!(TabState::from_label("EXIT"), None);
        assert_eq!(TabState::from_label("working"), None);
    }

    #[test]
    fn generic_profile_uses_timers_only() {
        let mut d = Detector::new(Profile::generic());
        assert_eq!(d.tick("anything", 100, 0), TabState::Busy);
        assert_eq!(d.tick("anything", 3000, 0), TabState::Done);
    }
}

