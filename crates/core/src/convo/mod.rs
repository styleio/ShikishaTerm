//! Conversations: what was said in an AI tab, read back from the CLI's own
//! record, with what only this app saw put beside it.
//!
//! A CLI writes down every word of a conversation, and nothing about how the
//! words got there. What a person typed at the window, what they sent from the
//! phone, what another tab asked and what a job briefed all read as "the
//! user" in its record. It cannot say who answered a question it asked, who
//! stopped it, or that it moved to a new conversation after a `/clear`. This
//! app can, because every one of those passed through it.
//!
//! * [`db`] -- the record of those things, kept on disk (`conversations.db`).
//!   Written only here, by [`Log`], from the app's main loop.
//! * [`read`] -- a page of a conversation for the panel beside the terminal:
//!   the words from the CLI's record, each person's line matched to whoever
//!   sent it, with the waits and stops in between. Read on a thread.
//! * [`marks`] -- the pins and notes a person puts on what was said. Theirs,
//!   so kept with their settings (`config/conversation-marks.json`), not here.
//! * [`confer`] -- AIs conferring: the short lines said beside each ask, and
//!   the rules they are kept to.

pub mod confer;
pub mod db;
pub mod marks;
pub mod read;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub use db::{By, Device, Origin, Stop};

use crate::detect::TabState;

/// Where the record lives
pub fn path() -> PathBuf {
    crate::config::state_path("conversations.db")
}

/// Something that happened to a tab, written down where it happened and put
/// into the record by the main loop at its next look ([`Log::take_notes`]).
/// What types a job's brief or a script's line into a tab is a long way from
/// the loop that holds the record; a note is how it says so without the
/// record being handed down to it.
///
/// `tab` is the tab's uid. A name is taken too, from what knows a tab only by
/// the name it was given (a job's record): it means the tab called that now
#[derive(Clone, Debug)]
enum Note {
    Sent { tab: String, texts: Vec<String>, from: Origin, at: i64 },
    Touched { tab: String, from: Origin, at: i64 },
    Stopped { tab: String, stop: Stop, at: i64 },
}

static NOTES: Mutex<Vec<Note>> = Mutex::new(Vec::new());

fn note(n: Note) {
    NOTES.lock().unwrap_or_else(|e| e.into_inner()).push(n);
}

/// Text went into `tab` (by uid) from `from`. `texts` are the ways it may land
/// in the CLI's record: what was typed whole first, then any part of it the
/// CLI may keep on its own
pub fn note_sent(tab: &str, texts: &[&str], from: Origin) {
    note(Note::Sent { tab: tab.to_string(), texts: texts.iter().map(|t| t.to_string()).collect(), from, at: db::now_ms() });
}

/// Input from `from` reached `tab`: the answer, when it was waiting on one
pub fn note_touched(tab: &str, from: Origin) {
    note(Note::Touched { tab: tab.to_string(), from, at: db::now_ms() });
}

/// `tab` was stopped
pub fn note_stopped(tab: &str, stop: Stop) {
    note(Note::Stopped { tab: tab.to_string(), stop, at: db::now_ms() });
}

/// A decision made in a job, waiting for the main loop to put it in the
/// conference of the desk its lead is on: the lead's id and what was decided
static AGREED: Mutex<Vec<(String, String, i64)>> = Mutex::new(Vec::new());

/// A job's decision was made (see `orch::Orchestra::make_decision`)
pub fn note_agreed(lead: &str, text: &str) {
    AGREED.lock().unwrap_or_else(|e| e.into_inner()).push((lead.to_string(), text.to_string(), db::now_ms()));
}

/// Every decision noted since the last look, oldest first
pub fn take_agreed() -> Vec<(String, String, i64)> {
    std::mem::take(&mut *AGREED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// What was last written about a tab, so the loop that looks at every tab
/// five times a second writes only when something changed
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sighting {
    record_id: String,
    cli: String,
    is_yolo: bool,
}

/// The writer of the record, held by the app's main loop. A record that
/// cannot be opened costs the app nothing but this record: every call becomes
/// a no-op, and why is said once in the hooks log.
///
/// A tab is named to it by its uid (`Tab::uid`), and the conference's calls
/// by the desk and the name a conversation knows it by -- which the record
/// turns into the uid of the tab called that ([`db::Store::uid_named`]), as
/// last told by [`Log::roster`]
pub struct Log {
    store: Option<db::Store>,
    /// Whether a failure has been said already. One line, not one a tick
    said_failure: bool,
    /// The conversation each tab was last seen on, by uid
    seen: HashMap<String, Sighting>,
    /// What each tab is called and where, by uid, as last written down
    names: HashMap<String, (String, String)>,
    /// The tab each name is answered by now, on any desk: what a note that
    /// names a tab is taken to mean
    by_name: HashMap<String, String>,
    /// Goes up every time the conference changes, so a panel showing it
    /// knows to read it again
    pub confer_rev: u64,
}

impl Default for Log {
    fn default() -> Self {
        Self::open(&path())
    }
}

impl Log {
    pub fn open(path: &std::path::Path) -> Self {
        let (store, said_failure) = match db::Store::open(path) {
            Ok(store) => (Some(store), false),
            Err(e) => {
                crate::append_hook_log(&format!("conversations: the record could not be opened ({e:#}); nothing is recorded"));
                (None, true)
            }
        };
        Log { store, said_failure, seen: HashMap::new(), names: HashMap::new(), by_name: HashMap::new(), confer_rev: 0 }
    }

    /// A record that lives only as long as this value (tests)
    pub fn in_memory() -> Self {
        Log {
            store: db::Store::in_memory().ok(),
            said_failure: false,
            seen: HashMap::new(),
            names: HashMap::new(),
            by_name: HashMap::new(),
            confer_rev: 0,
        }
    }

    /// Rewrite what an older version wrote under desks' ids and tabs' names,
    /// once, under their uids: `desks` is every desk of the settings -- its id
    /// and its uid ([`db::Store::adopt_desk_uids`]) -- and `tabs` every tab --
    /// its desk's uid, its name, its uid ([`db::Store::adopt_uids`]). Called
    /// before anything else is written, so nothing new is mistaken for
    /// something old
    pub fn adopt(&mut self, desks: &[(String, String)], tabs: &[(String, String, String)]) {
        // The desks first: what the tabs are matched by is the desk they are on
        let mut desks_done = false;
        self.write("the desks' uids", |s| {
            desks_done = s.adopt_desk_uids(desks)?;
            Ok(())
        });
        if desks_done {
            crate::append_hook_log(&format!("conversations: the record now knows desks by their uids ({} desks)", desks.len()));
        }
        let mut did = false;
        self.write("the tabs' uids", |s| {
            did = s.adopt_uids(tabs)?;
            Ok(())
        });
        if did {
            crate::append_hook_log(&format!("conversations: the record now knows tabs by their uids ({} tabs in the settings)", tabs.len()));
        }
    }

    /// The tabs there are now: each one's desk, name and uid. What changed
    /// since the last look is written down, so a name said in a conversation
    /// is taken to mean the tab called that now, and what a closed tab took
    /// part in still reads with the name it had
    pub fn roster<'a>(&mut self, tabs: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>) {
        let at = db::now_ms();
        let mut current: HashMap<String, Option<String>> = HashMap::new();
        for (desk, name, uid) in tabs {
            if name.is_empty() || uid.is_empty() {
                continue;
            }
            current.entry(name.to_string())
                .and_modify(|known| { if known.as_deref() != Some(uid) { *known = None; } })
                .or_insert_with(|| Some(uid.to_string()));
            let now = (desk.to_string(), name.to_string());
            if self.names.get(uid) == Some(&now) {
                continue;
            }
            self.names.insert(uid.to_string(), now);
            self.write("a tab's name", |s| s.named(uid, desk, name, at));
        }
        self.by_name = current.into_iter().filter_map(|(name, uid)| uid.map(|uid| (name, uid))).collect();
    }

    /// The uid a note's tab stands for: itself when it is one, else the tab
    /// called that now, else the uid of a name nobody answers to
    fn uid_of(&self, tab: &str) -> String {
        if crate::config::is_tab_uid(tab) {
            return tab.to_string();
        }
        self.by_name.get(tab).cloned().unwrap_or_else(|| db::gone_uid("", tab))
    }

    fn write(&mut self, what: &str, f: impl FnOnce(&mut db::Store) -> anyhow::Result<()>) {
        let Some(store) = self.store.as_mut() else { return };
        if let Err(e) = f(store)
            && !self.said_failure
        {
            self.said_failure = true;
            crate::append_hook_log(&format!("conversations: could not record {what} ({e:#}); later failures are not repeated"));
        }
    }

    /// Where a tab is: which conversation its CLI is on, which CLI, and
    /// whether it runs without asking first. Called for every AI tab on every
    /// look; writes only when the tab moved to another conversation
    pub fn follow(&mut self, tab: &str, cli: &str, record_id: Option<&str>, is_yolo: bool) {
        let Some(record_id) = record_id.filter(|r| !r.is_empty()) else { return };
        let now = Sighting { record_id: record_id.to_string(), cli: cli.to_string(), is_yolo };
        if self.seen.get(tab) == Some(&now) {
            return;
        }
        self.seen.insert(tab.to_string(), now);
        let at = db::now_ms();
        self.write("a conversation", |s| s.seen(tab, cli, record_id, is_yolo, at));
    }

    /// The conversation a tab was last seen on
    pub fn record_of(&self, tab: &str) -> Option<&str> {
        self.seen.get(tab).map(|s| s.record_id.as_str())
    }

    /// Everything noted since the last look (`note_sent` and the others), in
    /// the order it happened. Called by the main loop on every turn, and
    /// before a change of state is written, so an answer lands on the wait it
    /// answered
    pub fn take_notes(&mut self) {
        let notes = std::mem::take(&mut *NOTES.lock().unwrap_or_else(|e| e.into_inner()));
        for n in notes {
            match n {
                Note::Sent { tab, texts, from, at } => {
                    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
                    let tab = self.uid_of(&tab);
                    self.sent_at(&tab, &texts, &from, at);
                }
                Note::Touched { tab, from, at } => {
                    let tab = self.uid_of(&tab);
                    self.touched_at(&tab, &from, at)
                }
                Note::Stopped { tab, stop, at } => {
                    let tab = self.uid_of(&tab);
                    self.write("a stop", |s| s.stopped(&tab, &stop, at))
                }
            }
        }
    }

    /// Text went into `tab` (see [`note_sent`])
    pub fn sent(&mut self, tab: &str, texts: &[&str], from: &Origin) {
        self.sent_at(tab, texts, from, db::now_ms());
    }

    fn sent_at(&mut self, tab: &str, texts: &[&str], from: &Origin, at: i64) {
        let record = self.record_of(tab).map(str::to_string);
        // What a CLI may keep of the text: the text, and the text with the
        // app's own envelopes taken out, as the reader reads it back
        let mut ways: Vec<String> = Vec::new();
        for t in texts {
            ways.push(t.to_string());
            ways.push(crate::reader::human_part(t));
        }
        let ways: Vec<&str> = ways.iter().map(String::as_str).collect();
        self.write("what was sent", |s| s.sent(tab, record.as_deref(), &ways, from, at));
        self.touched_at(tab, from, at);
    }

    /// A tab changed state. A wait for an answer begins or ends here; a usage
    /// limit reached is a stop, with the CLI's own line about it
    pub fn state(&mut self, tab: &str, new: TabState, limit_line: Option<&str>) {
        self.take_notes();
        let at = db::now_ms();
        self.write("a change of state", |s| s.state(tab, new.label(), at));
        if new == TabState::Limit {
            let stop = Stop { by: By::Limit, device: None, how: "limit", job: None, why: limit_line.map(str::to_string) };
            self.write("a stop", |s| s.stopped(tab, &stop, at));
        }
        // The conversation was still going on now
        if let Some(seen) = self.seen.get(tab).cloned() {
            self.write("a conversation", |s| s.seen(tab, &seen.cli, &seen.record_id, seen.is_yolo, at));
        }
    }

    /// Input reached `tab` from `from`: the answer, when it was waiting on one
    pub fn touched(&mut self, tab: &str, from: &Origin) {
        self.touched_at(tab, from, db::now_ms());
    }

    fn touched_at(&mut self, tab: &str, from: &Origin, at: i64) {
        self.write("an answer", |s| s.touched(tab, from, at).map(|_| ()));
    }

    /// `tab` was stopped
    pub fn stopped(&mut self, tab: &str, stop: &Stop) {
        let at = db::now_ms();
        self.write("a stop", |s| s.stopped(tab, stop, at));
    }

    /// A conversation whose record is gone (see [`db::Store::forget_conversation`])
    pub fn forget(&mut self, tab: &str, record_id: &str) {
        self.write("a forgotten conversation", |s| s.forget_conversation(tab, record_id));
    }

    /// A value written to the record, or `None` when it could not be (said
    /// once in the log, as for every other write)
    fn written<T>(&mut self, what: &str, f: impl FnOnce(&mut db::Store) -> anyhow::Result<T>) -> Option<T> {
        let mut out = None;
        self.write(what, |s| {
            out = Some(f(s)?);
            Ok(())
        });
        if out.is_some() {
            self.confer_rev += 1;
        }
        out
    }

    /// The conversation grown from `origin` (see [`db::Store::thread_for`]).
    /// `.1`: it was begun now
    pub fn thread_for(&mut self, desk: &str, origin: &str) -> Option<(i64, bool)> {
        let at = db::now_ms();
        self.written("a conversation of AIs", |s| s.thread_for(desk, origin, at))
    }

    /// `from` belongs to `into` (see [`db::Store::merge_thread`])
    pub fn merge_thread(&mut self, from: i64, into: i64) {
        self.written("a conversation merged into another", |s| s.merge_thread(from, into));
    }

    /// An ask was sent in `thread` (see [`db::Store::ask_opened`]), with the
    /// asker's line
    #[allow(clippy::too_many_arguments)]
    pub fn ask_opened(
        &mut self,
        desk: &str,
        thread: i64,
        caller: Option<&str>,
        target: &str,
        line: &str,
        text: &str,
        round: u32,
    ) -> Option<i64> {
        let at = db::now_ms();
        let id = self.written("an ask", |s| s.ask_opened(desk, thread, caller, target, text, round, at))?;
        self.written("an ask's line", |s| s.line(desk, thread, caller, line, Some(id), "ask", at));
        Some(id)
    }

    /// How an ask ended. The answer's line is a step of its own
    /// ([`Self::answer_lined`]): a tab still saying it has passed its answer
    /// on already, and its line comes after
    pub fn ask_answered(&mut self, ask: i64, state: &str, reply: Option<&str>) {
        let at = db::now_ms();
        self.written("an answer to an ask", |s| s.ask_answered(ask, state, reply, at));
    }

    /// The answer's line is said, or given up on: a reply with no line of its
    /// own is given its first sentence, marked as taken for it
    pub fn answer_lined(&mut self, desk: &str, ask: i64, target: &str, state: &str, reply: Option<&str>, line_max: u32) {
        let at = db::now_ms();
        let Some(reply) = reply.filter(|_| state == "DONE") else { return };
        let has = self.store.as_ref().is_some_and(|s| s.ask_has_answer_line(ask).unwrap_or(true));
        let line = confer::first_sentence(reply, line_max);
        if !has
            && !line.is_empty()
            && let Some(thread) = self.thread_of_ask(ask)
        {
            self.written("an answer's line", |s| s.line(desk, thread, Some(target), &line, Some(ask), "auto", at));
        }
    }

    /// The conversation an ask was made in
    pub fn thread_of_ask(&self, ask: i64) -> Option<i64> {
        self.store.as_ref()?.thread_of_ask(ask).ok()?
    }

    /// A line said in `thread`: `how` as in the `lines` table
    pub fn line(&mut self, desk: &str, thread: i64, tab: Option<&str>, text: &str, ask: Option<i64>, how: &str) -> Option<i64> {
        let at = db::now_ms();
        self.written("a line", |s| s.line(desk, thread, tab, text, ask, how, at))
    }

    /// The last line `tab` said on `desk`, in `thread` when one is named
    pub fn last_line_of(&self, desk: &str, thread: Option<i64>, tab: Option<&str>) -> Option<i64> {
        self.store.as_ref()?.last_line_of(desk, thread, tab).ok()?
    }

    /// Whether `text` reached `tab` from this app a moment ago -- typed by
    /// the composer, another tab, a job -- rather than by somebody at the
    /// tab's own prompt. What a CLI reports a person typed is checked against
    /// it, so a line the app put in is not taken for one typed there
    pub fn typed_by_app(&mut self, tab: &str, text: &str, within_ms: i64) -> bool {
        self.take_notes();
        let Some(store) = self.store.as_ref() else { return false };
        let now = db::now_ms();
        let want = db::heads(&[text, &crate::reader::human_part(text)]);
        store
            .sends(tab, now - within_ms, now + 1000)
            .is_ok_and(|sent| sent.iter().any(|s| s.heads.iter().any(|h| want.contains(h))))
    }

    /// The desk a line was said on
    pub fn desk_of_line(&self, line: i64) -> Option<String> {
        self.store.as_ref()?.desk_of_line(line).ok()?
    }

    /// A mark put on a line or taken off it. `Some(true)`: it is on now
    pub fn toggle_mark(&mut self, line: i64, by: &str, mark: &str) -> Option<bool> {
        let at = db::now_ms();
        self.written("a mark", |s| s.toggle_mark(line, by, mark, at))
    }

    /// A card shared in `thread`
    #[allow(clippy::too_many_arguments)]
    pub fn shared(
        &mut self,
        desk: &str,
        thread: i64,
        tab: &str,
        kind: &str,
        target: &str,
        title: &str,
        detail: &serde_json::Value,
    ) -> Option<i64> {
        let at = db::now_ms();
        self.written("a card", |s| s.shared(desk, thread, tab, kind, target, title, detail, at))
    }

    /// The app is closing: every stretch of time still open ends now
    pub fn close(&mut self) {
        self.take_notes();
        let at = db::now_ms();
        self.write("the end of every stretch", |s| s.close_spans(at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_roster_does_not_keep_closed_or_ambiguous_names() {
        let mut log = Log::in_memory();
        log.roster([("one", "coder", "uid-one")]);
        assert_eq!(log.uid_of("coder"), "uid-one");
        log.roster([("one", "coder", "uid-one"), ("two", "coder", "uid-two")]);
        assert!(!["uid-one", "uid-two"].contains(&log.uid_of("coder").as_str()));
        log.roster([]);
        assert_ne!(log.uid_of("coder"), "uid-one");
        log.roster([("two", "coder", "uid-two")]);
        assert_eq!(log.uid_of("coder"), "uid-two");
    }

    #[test]
    fn a_tab_is_written_down_only_when_it_moves() {
        let mut log = Log::in_memory();
        for _ in 0..5 {
            log.follow("t", "claude", Some("a"), false);
        }
        log.follow("t", "claude", None, false);
        log.follow("t", "claude", Some("b"), true);
        let store = log.store.as_ref().unwrap();
        let c = store.conversations("t").unwrap();
        assert_eq!(c.iter().map(|c| c.record_id.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
        assert!(c[0].is_yolo);
        assert_eq!(log.record_of("t"), Some("b"));
    }

    #[test]
    fn what_was_sent_is_kept_against_the_conversation_and_answers_a_question() {
        let mut log = Log::in_memory();
        log.follow("t", "claude", Some("a"), false);
        log.state("t", TabState::Question, None);
        log.sent("t", &["<system-reminder>x</system-reminder>yes, go on"], &Origin::person(Device::Phone, "composer"));
        let store = log.store.as_ref().unwrap();
        let sent = store.sends("t", 0, i64::MAX).unwrap();
        assert_eq!(sent[0].record_id.as_deref(), Some("a"));
        assert!(sent[0].heads.contains(&db::head("yes, go on").unwrap()), "the words as the reader reads them back");
        let w = store.waits("t", i64::MIN, i64::MAX).unwrap();
        assert_eq!((w[0].by.as_deref(), w[0].via.as_deref()), (Some("person"), Some("composer")));
    }

    #[test]
    fn a_limit_reached_is_a_stop_with_the_cli_s_own_line() {
        let mut log = Log::in_memory();
        log.state("t", TabState::Busy, None);
        log.state("t", TabState::Limit, Some("5-hour limit reached ∙ resets 3pm"));
        let s = log.store.as_ref().unwrap().stops("t", i64::MIN, i64::MAX).unwrap();
        assert_eq!((s[0].by.as_str(), s[0].how.as_str(), s[0].why.as_deref()), ("limit", "limit", Some("5-hour limit reached ∙ resets 3pm")));
    }

    #[test]
    fn a_program_run_without_asking_is_told_by_its_command_line() {
        use crate::tab::runs_without_asking;
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(runs_without_asking(&argv(&["claude", "--dangerously-skip-permissions"])));
        assert!(runs_without_asking(&argv(&["C:\\bin\\codex.exe", "--dangerously-bypass-approvals-and-sandbox"])));
        assert!(!runs_without_asking(&argv(&["claude"])));
        assert!(!runs_without_asking(&argv(&["bash", "--yolo"])), "a word means something only to the program it belongs to");
    }
}
