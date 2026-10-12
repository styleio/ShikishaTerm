//! Scheduled prompts: a prompt handed to an AI at a time, filled in on a
//! form in the settings.
//!
//! The app's clock already runs a script named after a time
//! ([`crate::schedule`]). That is for whoever writes Lua. What most people
//! want from a clock is plainer -- "at nine on weekdays, have Claude look at
//! last night's failures" -- and that is a prompt, a folder, an AI and a time,
//! which a form can say without a line of code. So this is the front door,
//! and the scripts stay as the door for what a form cannot say.
//!
//! A prompt goes one of two ways:
//!
//! - **into a tab of its own, opened for the run** in the folder it names,
//!   with the AI it names -- the way a quick command opens one. Each run is a
//!   conversation that starts clean, and with `tidy` on, the tab is closed
//!   when the answer is in and nobody has touched it: what was said is in the
//!   run's history and in the record of conversations, and the closed tab can
//!   be opened again with its conversation;
//! - **into one tab that is already open**, by its uid: the same conversation
//!   carried on, run after run. A tab that is busy is not typed into -- the
//!   run is skipped and the reason written down, because words pasted into
//!   the middle of an AI's work are a request nobody meant.
//!
//! **One run at a time.** A run whose answer has not come is still going, and
//! the next time that comes round while it is going is skipped and said so:
//! two copies of the same work, one on top of the other, is never what was
//! wanted.
//!
//! **What happened is written down** ([`Book`], `data/routine-runs.json`):
//! every run, with when it was due, how it went and what the AI answered, so
//! a person who was away can read the morning's answer without hunting for
//! the tab. The same reason given time after time (the tab was busy, every
//! half hour all night) is one row with a count, not a page of them.
//!
//! Times come round by the same clock and the same rules as the scripts'
//! (this PC's clock; nothing while the app is closed or the PC sleeps; a time
//! that went by is run once if it is not later than the prompt allows). And
//! the brake on automation holds a scheduled prompt too; "Run now" is a
//! person's own press, and runs whatever the brake says.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// How long a name and a prompt may be. The same as a quick command's: a
/// prompt is the same kind of thing, written in the same kind of box
pub const NAME_MAX: usize = crate::quick::LABEL_MAX;
pub const PROMPT_MAX: usize = crate::quick::BODY_MAX;

/// How late, in minutes, a time that went by may still be run, when the form
/// says nothing. The scripts' hour ([`crate::schedule::GRACE`]): late enough
/// for a PC woken a little after nine, not so late that the morning's report
/// lands in the afternoon
pub const LATE_DEFAULT: u32 = 60;
/// The least it may be. The clock is looked at four times a minute and a busy
/// loop can be later still; a time allowed less than this would be missed for
/// being looked at a few seconds after it, not for anything a person did
pub const LATE_LEAST: u32 = 2;
/// The most: two days. A prompt run later than that is about a day that has
/// gone, whatever it says
pub const LATE_MOST: u32 = 2 * 24 * 60;

/// How many runs of each prompt are kept. Enough for a month of a daily one
/// and for a night of a half-hourly one; the record of conversations keeps
/// what was said for longer
pub const RUNS_KEPT: usize = 60;
/// How much of an answer is kept with a run. An answer is read in the
/// settings, not pasted from; a longer one is in the conversation
pub const REPLY_MAX: usize = 32 * 1024;

fn yes() -> bool {
    true
}

fn late_default() -> u32 {
    LATE_DEFAULT
}

/// When a prompt comes round, as the form fills it in. Every field has a
/// default, so a file written by hand -- or one a later version wrote -- is
/// read rather than refused; [`Times::when`] says whether it means anything
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Times {
    /// `days`: at `time` on the `days` ticked; `hourly`: every hour at
    /// `minute` past; `every`: every `minutes`, from when it last ran
    #[serde(default)]
    pub repeat: String,
    /// The days of the week, 0 for Sunday
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<u8>,
    /// The time of day, `HH:MM` by this PC's clock
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub time: String,
    #[serde(default)]
    pub minute: u8,
    #[serde(default)]
    pub minutes: u32,
}

impl Times {
    /// What the clock makes of it; `None` for a form not filled in right (no
    /// day ticked, a time that is not one, a repeat shorter than the clock
    /// allows)
    pub fn when(&self) -> Option<crate::schedule::When> {
        use crate::schedule::{Days, When};
        match self.repeat.as_str() {
            "days" => {
                let set = self.days.iter().filter(|d| **d < 7).fold(0u8, |set, d| set | 1 << d);
                let (h, m) = self.time.split_once(':')?;
                let (hour, minute): (u8, u8) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
                (set != 0 && hour < 24 && minute < 60).then_some(When::At { days: Days::Set(set), hour, minute })
            }
            "hourly" => (self.minute < 60).then_some(When::Hourly(self.minute)),
            "every" => {
                let secs = u64::from(self.minutes) * 60;
                (secs >= crate::schedule::SHORTEST).then_some(When::Every(secs))
            }
            _ => None,
        }
    }
}

/// One scheduled prompt, as the settings file holds it (`routines`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Routine {
    /// What it is known by: made by the settings page when it is
    #[serde(default)]
    pub id: String,
    /// What the history and the tab it opens are called
    #[serde(default)]
    pub name: String,
    /// Off keeps it, and its history, without running it
    #[serde(default = "yes")]
    pub on: bool,
    #[serde(default)]
    pub prompt: String,
    /// Which AI a tab opened for it starts, by the command that starts it
    /// (`claude`, `codex`...). Empty is the first one this PC can start, as
    /// for a quick command
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ai: String,
    /// The desk it runs in, by its uid
    #[serde(default)]
    pub desk: String,
    /// The folder a tab is opened in, as a place key
    /// ([`crate::uistate::place_key`])
    #[serde(default)]
    pub folder: String,
    /// The tab it is handed to, by its uid. Empty is a tab of its own, opened
    /// for each run
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tab: String,
    #[serde(default)]
    pub when: Times,
    /// How late, in minutes, a time that went by may still be run
    #[serde(default = "late_default")]
    pub late: u32,
    /// Close the tab opened for a run once its answer is in, if nobody has
    /// touched it. Means nothing for a prompt handed to a tab already open:
    /// that tab is the person's
    #[serde(default = "yes")]
    pub tidy: bool,
}

impl Routine {
    /// How late a time may be run, in seconds, kept between the least and
    /// the most there are
    pub fn grace(&self) -> u64 {
        u64::from(self.late.clamp(LATE_LEAST, LATE_MOST)) * 60
    }

    /// The key its last time is kept under in [`crate::schedule::Kept`],
    /// beside the scripts'. A script's key always holds a `#`, an id never
    /// does, so the two cannot meet
    pub fn kept_key(&self) -> String {
        format!("routine:{}", self.id)
    }

    /// What is wrong with it, as a dictionary key; `None` for one that can run
    pub fn fault(&self) -> Option<&'static str> {
        if self.id.trim().is_empty() || self.id.contains('#') {
            return Some("routine.fault.id");
        }
        if self.prompt.trim().is_empty() {
            return Some("routine.fault.prompt");
        }
        if self.desk.is_empty() || (self.folder.is_empty() && self.tab.is_empty()) {
            return Some("routine.fault.place");
        }
        if self.when.when().is_none() {
            return Some("routine.fault.when");
        }
        None
    }

    /// The name it is shown by: its own, or the start of its prompt
    pub fn shown_name(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.chars().take(NAME_MAX).collect();
        }
        let first = self.prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        match first.chars().count() > 40 {
            true => format!("{}…", first.chars().take(40).collect::<String>()),
            false => first.to_string(),
        }
    }
}

/// The list as the settings file holds it. An entry that is not one at all --
/// a string, a list, a time that is not a number -- is left out, rather than
/// the whole settings file being refused over it and every desk with it
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Routine>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let serde_json::Value::Array(items) = value else { return Ok(Vec::new()) };
    Ok(items.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

/// How a run went. The words are this app's own; the page says each in the
/// person's language (`routine.how.*`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// Handed over, or on its way; the answer has not come
    Going,
    /// The AI answered
    Answered,
    /// Its time went by while the app could not run it, too long ago to run
    Missed,
    /// Not run, for a reason that is nobody's fault: the last run still
    /// going, the tab busy
    Skipped,
    /// Could not be run: the folder or the tab is gone, no AI to start
    Failed,
    /// The app was closed, or the tab went away, before the answer came
    Cut,
}

/// One run of a prompt, as the history keeps it
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// Counted up for each prompt, and never reused when old runs are let go
    pub n: u64,
    /// The time it came round for, in seconds since 1970. `None` for a run a
    /// person asked for with "Run now"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<u64>,
    /// When it was started, or written down
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
    pub how: How,
    /// The tab it went to, by uid and by the name it had
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// Why it was skipped, failed or cut, as a dictionary key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// How many times in a row it was skipped or failed for this same
    /// reason; the row's `at` is the first, `end` the last
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub times: u32,
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// Every prompt's runs, kept from one run of the app to the next.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub runs: BTreeMap<String, Vec<Run>>,
    /// The number the next run of each prompt gets
    #[serde(default)]
    pub next: BTreeMap<String, u64>,
}

fn book_path() -> std::path::PathBuf {
    crate::config::state_path("routine-runs.json")
}

impl Book {
    /// The book as it was left. A run that was still going when the app last
    /// stopped can never be answered now: it is written down as cut short
    pub fn load() -> Book {
        let mut book: Book = std::fs::read_to_string(book_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        book.cut_short();
        book
    }

    /// Every run still going, written down as cut short by the app closing
    fn cut_short(&mut self) {
        for run in self.runs.values_mut().flatten() {
            if run.how == How::Going {
                run.how = How::Cut;
                run.why = Some("routine.why.app_closed".into());
            }
        }
    }

    /// Read as the settings page reads it, without touching a run
    pub fn read() -> Book {
        std::fs::read_to_string(book_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = crate::crypto::write_atomic(&book_path(), &text);
        }
    }

    /// A new run of `id`, written down; its number back
    pub fn begin(&mut self, id: &str, due: Option<u64>, now: u64) -> u64 {
        let n = self.next.get(id).copied().unwrap_or(1);
        self.next.insert(id.to_string(), n + 1);
        self.push(id, Run { n, due, at: now, end: None, how: How::Going, tab: None, title: None, reply: None, why: None, times: 1 });
        n
    }

    /// A run that never started -- skipped, failed or missed -- written down.
    /// The same thing said again straight after the last is that row once
    /// more, with its count up, rather than a new row
    pub fn note(&mut self, id: &str, due: Option<u64>, now: u64, how: How, why: &str) {
        let runs = self.runs.entry(id.to_string()).or_default();
        if let Some(last) = runs.last_mut()
            && last.how == how
            && how != How::Missed
            && last.why.as_deref() == Some(why)
        {
            last.times += 1;
            last.end = Some(now);
            return;
        }
        let n = self.next.get(id).copied().unwrap_or(1);
        self.next.insert(id.to_string(), n + 1);
        self.push(id, Run { n, due, at: now, end: None, how, tab: None, title: None, reply: None, why: Some(why.into()), times: 1 });
    }

    fn push(&mut self, id: &str, run: Run) {
        let runs = self.runs.entry(id.to_string()).or_default();
        runs.push(run);
        // The oldest go first; a run still going is never let go
        while runs.len() > RUNS_KEPT {
            match runs.iter().position(|r| r.how != How::Going) {
                Some(i) => {
                    runs.remove(i);
                }
                None => break,
            }
        }
    }

    /// Run `n` of `id`, to be filled in as it goes
    pub fn run_mut(&mut self, id: &str, n: u64) -> Option<&mut Run> {
        self.runs.get_mut(id)?.iter_mut().find(|r| r.n == n)
    }

    /// The history of prompts that are no longer in the settings is let go
    pub fn keep_only(&mut self, ids: &[&str]) -> bool {
        let before = self.runs.len() + self.next.len();
        self.runs.retain(|id, _| ids.contains(&id.as_str()));
        self.next.retain(|id, _| ids.contains(&id.as_str()));
        before != self.runs.len() + self.next.len()
    }
}

/// An answer cut to what a run keeps, at a character boundary
pub fn kept_reply(reply: &str) -> String {
    let reply = reply.trim();
    if reply.len() <= REPLY_MAX {
        return reply.to_string();
    }
    let mut end = REPLY_MAX;
    while !reply.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &reply[..end])
}

/// The prompts a person asked to run now, from the settings, waiting for the
/// board's loop
static RUN_NOW: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Ask for `id` to be run now
pub fn want_run(id: &str) {
    if let Ok(mut w) = RUN_NOW.lock()
        && !w.iter().any(|x| x == id)
    {
        w.push(id.to_string());
    }
}

/// The prompts asked to run now, taken
pub fn take_run_now() -> Vec<String> {
    RUN_NOW.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default()
}

/// When each prompt comes round next, for the settings page: by this PC's
/// clock, from the times kept. A prompt off, or not filled in right, has none
pub fn next_times(routines: &[Routine], kept: &crate::schedule::Kept, now: u64) -> BTreeMap<String, u64> {
    let local = |at: u64| crate::hooks::local_at(at);
    routines
        .iter()
        .filter(|r| r.on && r.fault().is_none())
        .filter_map(|r| {
            let when = r.when.when()?;
            let last = kept.last.get(&r.kept_key()).copied();
            Some((r.id.clone(), crate::schedule::next(when, last, now, &local)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weekday_nine() -> Routine {
        Routine {
            id: "r1".into(),
            prompt: "Look at last night's failures".into(),
            desk: "d1".into(),
            folder: "C:\\work\\app".into(),
            when: Times { repeat: "days".into(), days: vec![1, 2, 3, 4, 5], time: "09:00".into(), ..Default::default() },
            late: LATE_DEFAULT,
            on: true,
            tidy: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_form_filled_in_says_a_time() {
        use crate::schedule::{Days, When};
        assert_eq!(weekday_nine().when.when(), Some(When::At { days: Days::Set(0b011_1110), hour: 9, minute: 0 }));
        let hourly = Times { repeat: "hourly".into(), minute: 15, ..Default::default() };
        assert_eq!(hourly.when(), Some(When::Hourly(15)));
        let every = Times { repeat: "every".into(), minutes: 30, ..Default::default() };
        assert_eq!(every.when(), Some(When::Every(1800)));
        for wrong in [
            Times { repeat: "days".into(), days: vec![], time: "09:00".into(), ..Default::default() },
            Times { repeat: "days".into(), days: vec![1], time: "25:00".into(), ..Default::default() },
            Times { repeat: "days".into(), days: vec![1], time: "nine".into(), ..Default::default() },
            Times { repeat: "hourly".into(), minute: 60, ..Default::default() },
            Times { repeat: "every".into(), minutes: 0, ..Default::default() },
            Times { repeat: "fortnightly".into(), ..Default::default() },
        ] {
            assert_eq!(wrong.when(), None, "{wrong:?} was taken for a time");
        }
    }

    #[test]
    fn a_prompt_that_cannot_run_says_why() {
        assert_eq!(weekday_nine().fault(), None);
        let mut r = weekday_nine();
        r.prompt = "  ".into();
        assert_eq!(r.fault(), Some("routine.fault.prompt"));
        let mut r = weekday_nine();
        r.folder.clear();
        assert_eq!(r.fault(), Some("routine.fault.place"));
        // ...unless it goes to a tab
        r.tab = "t1".into();
        assert_eq!(r.fault(), None);
        let mut r = weekday_nine();
        r.when.days.clear();
        assert_eq!(r.fault(), Some("routine.fault.when"));
    }

    #[test]
    fn a_file_written_by_hand_is_read_not_refused() {
        let r: Routine = serde_json::from_str(r#"{"id":"x","prompt":"hi","when":{"repeat":"weekly-ish"},"extra":1}"#).unwrap();
        assert!(r.on && r.tidy, "a prompt read with nothing said is on and tidies, as the form makes one");
        assert_eq!(r.late, LATE_DEFAULT);
        assert_eq!(r.fault(), Some("routine.fault.place"));
    }

    #[test]
    fn one_entry_that_is_not_one_does_not_take_the_rest_with_it() {
        #[derive(serde::Deserialize)]
        struct Holder {
            #[serde(default, deserialize_with = "lenient")]
            routines: Vec<Routine>,
        }
        let h: Holder = serde_json::from_str(r#"{"routines":[{"id":"a","prompt":"p"},"nonsense",{"id":"b","late":"soon"},{"id":"c"}]}"#).unwrap();
        assert_eq!(h.routines.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        let h: Holder = serde_json::from_str(r#"{"routines":{"id":"a"}}"#).unwrap();
        assert!(h.routines.is_empty());
    }

    #[test]
    fn how_late_is_kept_within_bounds() {
        let mut r = weekday_nine();
        r.late = 0;
        assert_eq!(r.grace(), u64::from(LATE_LEAST) * 60);
        r.late = 100_000;
        assert_eq!(r.grace(), u64::from(LATE_MOST) * 60);
    }

    #[test]
    fn the_same_reason_again_is_one_row_with_a_count() {
        let mut book = Book::default();
        book.note("r1", Some(100), 100, How::Skipped, "routine.why.busy");
        book.note("r1", Some(200), 200, How::Skipped, "routine.why.busy");
        book.note("r1", Some(300), 300, How::Skipped, "routine.why.last_going");
        let runs = &book.runs["r1"];
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].times, runs[0].at, runs[0].end), (2, 100, Some(200)));
        assert_eq!(runs[1].n, 2, "a folded row took no number");
        // A missed time is its own row each time: each is a morning that went by
        book.note("r1", Some(400), 400, How::Missed, "routine.why.late");
        book.note("r1", Some(500), 500, How::Missed, "routine.why.late");
        assert_eq!(book.runs["r1"].len(), 4);
    }

    #[test]
    fn old_runs_go_but_a_run_still_going_stays() {
        let mut book = Book::default();
        let first = book.begin("r1", None, 1);
        for i in 0..RUNS_KEPT as u64 + 5 {
            book.note("r1", None, 10 + i, How::Missed, "routine.why.late");
        }
        let runs = &book.runs["r1"];
        assert_eq!(runs.len(), RUNS_KEPT);
        assert_eq!(runs[0].n, first, "the run still going was let go");
        assert!(runs.windows(2).all(|w| w[0].n < w[1].n));
    }

    #[test]
    fn a_run_going_when_the_app_stopped_is_cut() {
        let mut book = Book::default();
        let n = book.begin("r1", Some(5), 5);
        book.note("r1", None, 6, How::Skipped, "routine.why.last_going");
        let mut read: Book = serde_json::from_str(&serde_json::to_string(&book).unwrap()).unwrap();
        read.cut_short();
        let run = read.run_mut("r1", n).unwrap();
        assert_eq!((run.how, run.why.as_deref()), (How::Cut, Some("routine.why.app_closed")));
        assert_eq!(read.runs["r1"][1].how, How::Skipped);
    }

    #[test]
    fn a_long_answer_is_cut_at_a_character() {
        let long = "あ".repeat(REPLY_MAX);
        let kept = kept_reply(&long);
        assert!(kept.len() <= REPLY_MAX + "…".len());
        assert!(kept.ends_with('…'));
        assert_eq!(kept_reply("  short  "), "short");
    }

    #[test]
    fn the_history_of_prompts_gone_is_let_go() {
        let mut book = Book::default();
        book.begin("keep", None, 1);
        book.begin("gone", None, 1);
        assert!(book.keep_only(&["keep"]));
        assert!(book.runs.contains_key("keep") && !book.runs.contains_key("gone"));
        assert!(!book.keep_only(&["keep"]));
    }
}
