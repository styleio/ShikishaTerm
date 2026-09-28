//! Asking another tab, and waiting for what it says back.
//!
//! `send_to_tab` puts words in front of another AI and returns at once, which
//! is right for a script with hooks to catch the answer and wrong for an AI
//! that wants the answer as the result of the call it made. `ask_tab` is the
//! second shape: the call stays open until the tab it went to has finished,
//! and what comes back is that tab's reply.
//!
//! **Why it is answered here and not in Lua.** A call through the pipe is
//! carried out on the main loop, and a primitive that stops to wait stops the
//! loop with it -- every screen, every detector, including the one that would
//! notice the other tab had finished. So the call is taken apart: Lua checks
//! it may be made (the permission table, the tab existing), the loop keeps the
//! line's answer channel, and a tick at a time looks at the other tab until
//! there is something to say.
//!
//! **Where the reply comes from.** The CLI's own record of the conversation,
//! read the way the phone's reader reads it -- not the screen, which is a
//! picture of the reply cut to the window's width and wrapped in the tool's
//! own frame. A tab on another machine is read from the record kept there,
//! a look at a time on a thread of its own ([`FarRead`]). A tab whose profile
//! names no record, or whose machine has none by that name, falls back to the
//! screen.
//!
//! **What is never lost.** The caller may stop holding the line: its client
//! has a timeout of its own, or a person pressed Esc. The other tab goes on
//! working all the same, and when it finishes its reply is typed into the
//! caller's tab instead, once the caller is free to read it.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::detect::TabState;
use crate::reader::Who;
use crate::tab::Tab;

/// How long a caller is held when it names no time of its own. Shorter than
/// the hour this app writes into a CLI's settings for the call, so the answer
/// "still working" arrives before the client gives up on the line
pub const DEFAULT_WAIT: Duration = Duration::from_secs(50 * 60);

/// How long the pipe itself holds a line open for this one call. Longer than
/// anything a caller may ask for; the loop is what decides when to answer
pub const LINE_HOLD: Duration = Duration::from_secs(6 * 60 * 60);

/// A finished-looking tab has to stay finished this long before it counts. A
/// turn has quiet moments -- between a tool and the next thought -- that the
/// detector can read as the end
const SETTLE: Duration = Duration::from_millis(1500);

/// With no record to read, the screen is read instead once the tab has been
/// quiet this long
const SCREEN_SETTLE: Duration = Duration::from_secs(3);

/// How long a tab may sit with background work after its turn before the reply
/// it has written is taken as its answer. An AI that put a wait in the
/// background comes back and says more when it finishes; a tab that keeps a
/// server running never will, and the caller should not be held for an hour
/// on a reply that is already written
const BACKGROUND_GRACE: Duration = Duration::from_secs(180);

/// A tab that never looked busy after being sent something is taken to have
/// started (and finished) quickly once this has passed
const NEVER_SEEN_BUSY: Duration = Duration::from_secs(8);

/// How much of what was sent is looked for in the record, to find the turn
/// the reply belongs to. Enough to tell two questions apart, short enough to
/// survive a CLI trimming the end of a long paste
const MATCH_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Not sent yet: the other tab was busy, and words typed into a busy CLI
    /// are queued at best and lost at worst
    Queued,
    /// Sent; the caller is holding the line for the reply
    Waiting,
    /// The caller stopped holding the line. The reply goes into its tab
    Deliver,
    /// The reply is ready and waits for the caller's tab to be free
    Handing(String),
}

/// One ask in flight.
pub struct Ask {
    pub reply: Option<Sender<Result<Value, String>>>,
    pub caller: Option<String>,
    pub target: String,
    pub text: String,
    pub phase: Phase,
    pub asked_at: Instant,
    pub sent_at: Option<Instant>,
    pub seen_busy: bool,
    pub quiet_since: Option<Instant>,
    /// Since when the tab has sat in `Background` after the turn
    pub background_since: Option<Instant>,
    pub deadline: Instant,
    pub round: u32,
    pub max_rounds: u32,
    /// A command typed into a shell (`tab_run`) rather than words to an AI:
    /// what comes back is what the terminal printed, from where its session
    /// log stood when the command went in
    pub run: Option<RunFrom>,
    /// The reply being looked for on the machine the tab runs on
    pub far: FarRead,
}

/// Where a shell's output starts, for `tab_run`
#[derive(Debug, Clone, Default)]
pub struct RunFrom {
    /// The tab's session log, when it keeps one
    pub log: Option<std::path::PathBuf>,
    /// How far the log went before the command was typed
    pub from: u64,
}

impl RunFrom {
    /// Where the log of `t` stands now
    pub fn now(t: &Tab) -> Self {
        let log = t.log_path.clone();
        let from = log
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        RunFrom { log, from }
    }
}

/// Most of a command's output handed back: the end of it, where the result
/// and the error usually are
const RUN_KEEP: usize = 8000;

/// What a command printed since it was typed, from the session log, or the
/// bottom of the screen when the tab keeps no log
fn run_output(t: &Tab, from: &RunFrom) -> (String, &'static str) {
    if let Some(log) = from.log.as_ref() {
        let (text, _) = crate::session_log::read_from(log, from.from);
        if !text.trim().is_empty() {
            let text = text.trim_end();
            let cut = text
                .char_indices()
                .rev()
                .nth(RUN_KEEP)
                .map(|(i, _)| i)
                .unwrap_or(0);
            return (text[cut..].to_string(), "log");
        }
    }
    (tail(&t.last_screen, 60), "screen")
}

/// The reply looked for in a record on the machine the tab runs on.
///
/// Reading it there is a round trip or several, and this is asked on the main
/// loop, a tick at a time: so a look is sent off on a thread of its own and
/// its answer picked up on a later tick, the way the phone's reader answers
/// from a thread of its own.
#[derive(Default)]
pub struct FarRead {
    pending: Option<std::sync::mpsc::Receiver<Found>>,
    /// When the last look went out, so a record that has not caught up yet is
    /// looked at again every few seconds rather than every tick
    last: Option<Instant>,
    /// The machine keeps no record there by that name: the screen is what
    /// there is, as for a tab here with no record
    missing: bool,
}

/// What one look at a far record found
enum Found {
    Missing,
    NotYet,
    Reply(String),
}

/// How long after one look at a far record the next may go out
const FAR_AGAIN: Duration = Duration::from_secs(3);

impl FarRead {
    /// The reply to `sent` in `record`, once a look has come back with it.
    /// `None` while one is out or the record has not caught up
    fn reply(&mut self, record: &crate::reader::Record, sent: &str) -> Option<String> {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(Found::Reply(reply)) => {
                    self.pending = None;
                    return Some(reply);
                }
                Ok(Found::Missing) => {
                    self.pending = None;
                    self.missing = true;
                }
                Ok(Found::NotYet) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            return None;
        }
        if self.missing || self.last.is_some_and(|at| at.elapsed() < FAR_AGAIN) {
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (record, sent) = (record.clone(), sent.to_string());
        std::thread::spawn(move || {
            let found = match record.page(u64::MAX, REPLY_TURNS) {
                None => Found::Missing,
                Some(Ok(page)) => reply_of(&page, &sent).map_or(Found::NotYet, Found::Reply),
                Some(Err(_)) => Found::NotYet,
            };
            let _ = tx.send(found);
        });
        self.pending = Some(rx);
        self.last = Some(Instant::now());
        None
    }
}

/// Where the reply to an ask is read from: the record here, the record on the
/// machine the tab runs on, or -- with neither -- the screen
enum Source {
    Here(std::path::PathBuf),
    Far(crate::reader::Record),
    Screen,
}

impl Ask {
    fn source(&self, t: &Tab) -> Source {
        match t.record_at() {
            Some(record) if record.is_far() => {
                if self.far.missing { Source::Screen } else { Source::Far(record) }
            }
            _ => t.record().map_or(Source::Screen, Source::Here),
        }
    }

    /// The reply in the record, from wherever it is kept. `None` while it has
    /// not been found (yet)
    fn recorded(&mut self, t: &Tab) -> Option<String> {
        match self.source(t) {
            Source::Here(path) => reply_in(&path, &self.text),
            Source::Far(record) => {
                let text = self.text.clone();
                self.far.reply(&record, &text)
            }
            Source::Screen => None,
        }
    }
}

/// An answer that outlived the line it was asked on, waiting to be typed into
/// the caller's tab once that tab is free (a browser run, see `browser_do`)
pub fn handing(caller: String, target: String, text: String) -> Ask {
    let now = Instant::now();
    Ask {
        reply: None,
        caller: Some(caller),
        target,
        text: String::new(),
        phase: Phase::Handing(text),
        asked_at: now,
        sent_at: Some(now),
        seen_busy: true,
        quiet_since: None,
        background_since: None,
        deadline: now,
        round: 0,
        max_rounds: 0,
        run: None,
        far: FarRead::default(),
    }
}

/// What the loop should do for an ask this tick.
pub enum Step {
    Nothing,
    /// Send the words now
    Send,
    /// Answer the caller (or, with no line left, type it into the caller's tab)
    Answer(Value),
    /// Type the finished reply into the caller's tab now
    Hand(String),
    /// Forget it
    Drop,
}

/// `ask_tab(tab, text, {timeout_ms})`, taken apart. Only a tab's id names a
/// tab here: a position changes when tabs are moved, and a display name is
/// not unique
pub fn parse(params: &[Value]) -> Result<(String, String, Duration), String> {
    let target = params
        .first()
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("ask_tab needs the tab's id first")?
        .trim()
        // Handed over the way it appears in a message -- `<@codex>` -- or bare
        .trim_start_matches('<')
        .trim_start_matches('@')
        .trim_end_matches('>')
        .to_string();
    let text = params
        .get(1)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("ask_tab needs what to say second")?
        .to_string();
    let wait = params
        .get(2)
        .and_then(|o| o.get("timeout_ms"))
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_WAIT)
        .min(LINE_HOLD - Duration::from_secs(60));
    Ok((target, text, wait))
}

/// Words with their runs of space made single, for finding one text in another
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The reply to `sent` in the record at `path`: the last thing the AI said
/// after the turn in which it was given `sent`. `None` while that turn has not
/// been written, or the AI has not said anything since
pub fn reply_in(path: &Path, sent: &str) -> Option<String> {
    reply_of(&crate::reader::read_back(path, u64::MAX, REPLY_TURNS).ok()?, sent)
}

/// How many things said are read back to find the turn a reply belongs to
const REPLY_TURNS: usize = 16;

/// The reply to `sent` in a page of the record (see [`reply_in`])
fn reply_of(page: &crate::reader::Page, sent: &str) -> Option<String> {
    let want: String = flat(sent).chars().take(MATCH_CHARS).collect();
    let asked = page
        .turns
        .iter()
        .rposition(|t| t.who == Who::You && flat(&t.text).contains(&want))?;
    page.turns[asked + 1..]
        .iter()
        .rev()
        .find(|t| t.who == Who::Ai && !t.text.trim().is_empty())
        .map(|t| t.text.trim().to_string())
}

/// The turn is over and nothing it started is still running. `Background` is
/// not quiet: an AI that put a long command in the background has ended its
/// turn without the answer, and asking it now gets "still waiting". The app's
/// own MCP server in the tab's job is not counted as background work
/// ([`crate::job::is_this_program`]), so an AI with nothing running does read
/// as `Done`
pub fn quiet(state: TabState) -> bool {
    matches!(state, TabState::Done | TabState::Wait)
}

/// The tab's turn is over, whether or not something it started runs on: free
/// to be typed into
pub fn turn_over(state: TabState) -> bool {
    quiet(state) || state == TabState::Background
}

/// The answer sent back to the caller
fn answer(
    a: &Ask,
    state: &str,
    reply: Option<&str>,
    source: &str,
    same_folder: Option<bool>,
    note: Option<&str>,
) -> Value {
    let mut v = json!({
        "tab": a.target,
        "state": state,
        "reply": reply,
        "source": source,
        "round": a.round,
        "max_rounds": if a.max_rounds == 0 { Value::Null } else { json!(a.max_rounds) },
        "seconds": a.asked_at.elapsed().as_secs(),
    });
    if let Some(same) = same_folder {
        v["same_folder"] = json!(same);
    }
    if let Some(n) = note {
        v["note"] = json!(n);
    }
    v
}

/// One tick of one ask, against the tab it went to (`None`: gone) and whether
/// the caller's tab is free to be typed into.
pub fn step(
    a: &mut Ask,
    target: Option<&Tab>,
    caller_free: bool,
    same_folder: Option<bool>,
) -> Step {
    let now = Instant::now();
    if let Phase::Handing(text) = &a.phase {
        return if caller_free {
            Step::Hand(text.clone())
        } else {
            Step::Nothing
        };
    }
    let Some(t) = target else {
        return Step::Answer(answer(
            a,
            "GONE",
            None,
            "none",
            None,
            Some("the tab was closed"),
        ));
    };
    if a.phase == Phase::Queued {
        if matches!(t.state, TabState::Exited) {
            return Step::Answer(answer(
                a,
                "EXIT",
                None,
                "none",
                same_folder,
                Some("the tab's program has ended"),
            ));
        }
        if turn_over(t.state) {
            return Step::Send;
        }
        if now >= a.deadline {
            return Step::Answer(answer(
                a,
                "BUSY",
                None,
                "none",
                same_folder,
                Some("the tab stayed busy; nothing was sent"),
            ));
        }
        return Step::Nothing;
    }
    // Sent: watch it work
    match t.state {
        TabState::Busy => {
            a.seen_busy = true;
            a.quiet_since = None;
            a.background_since = None;
        }
        TabState::Background => {
            a.seen_busy = true;
            a.quiet_since = None;
            let since = *a.background_since.get_or_insert(now);
            if since.elapsed() >= BACKGROUND_GRACE {
                if let Some(reply) = a.recorded(t) {
                    return Step::Answer(answer(
                        a,
                        "DONE",
                        Some(&reply),
                        "record",
                        same_folder,
                        Some(
                            "the tab still has work running in the background; this is what it has said so far",
                        ),
                    ));
                }
            }
        }
        TabState::Question => {
            // A tab waiting on a person is not going to answer by itself, and
            // holding the line until the deadline would hide that for an hour
            let screen = tail(&t.last_screen, 12);
            return Step::Answer(answer(
                a,
                "QUESTION",
                Some(&screen),
                "screen",
                same_folder,
                Some("the tab is waiting for someone to approve or choose; its screen is in reply"),
            ));
        }
        TabState::Exited => {
            return Step::Answer(answer(
                a,
                "EXIT",
                None,
                "none",
                same_folder,
                Some("the tab's program has ended"),
            ));
        }
        TabState::Limit => {
            return Step::Answer(answer(
                a,
                "LIMIT",
                None,
                "none",
                same_folder,
                Some("the tab hit its usage limit"),
            ));
        }
        TabState::Failed => {
            let screen = tail(&t.last_screen, 12);
            return Step::Answer(answer(
                a,
                "FAILED",
                Some(&screen),
                "screen",
                same_folder,
                Some("the turn ended in an error"),
            ));
        }
        TabState::Done | TabState::Wait => {
            let sent = a.sent_at.unwrap_or(a.asked_at);
            if a.seen_busy || sent.elapsed() >= NEVER_SEEN_BUSY {
                a.quiet_since.get_or_insert(now);
            }
        }
    }
    if let Some(since) = a.quiet_since {
        // A command: done once the shell has been quiet a moment, and its
        // answer is what it printed
        if let Some(from) = a.run.as_ref() {
            if since.elapsed() >= SCREEN_SETTLE {
                let (out, source) = run_output(t, from);
                return Step::Answer(answer(a, "DONE", Some(&out), source, same_folder, None));
            }
            return if a.phase == Phase::Waiting && now >= a.deadline {
                Step::Answer(answer(
                    a,
                    "PENDING",
                    None,
                    "none",
                    same_folder,
                    Some("still running; its output will be typed into your tab when it finishes"),
                ))
            } else {
                Step::Nothing
            };
        }
        match a.source(t) {
            Source::Here(_) | Source::Far(_) => {
                if since.elapsed() >= SETTLE {
                    if let Some(reply) = a.recorded(t) {
                        return Step::Answer(answer(
                            a,
                            "DONE",
                            Some(&reply),
                            "record",
                            same_folder,
                            None,
                        ));
                    }
                }
            }
            Source::Screen => {
                if since.elapsed() >= SCREEN_SETTLE {
                    let reply = t
                        .last_response
                        .clone()
                        .unwrap_or_else(|| tail(&t.last_screen, 40));
                    return Step::Answer(answer(
                        a,
                        "DONE",
                        Some(&reply),
                        "screen",
                        same_folder,
                        None,
                    ));
                }
            }
        }
    }
    if a.phase == Phase::Waiting && now >= a.deadline {
        return Step::Answer(answer(
            a,
            "PENDING",
            None,
            "none",
            same_folder,
            Some("still working; its reply will be typed into your tab when it finishes"),
        ));
    }
    Step::Nothing
}

/// The last `n` lines of a screen with the blank ones at the bottom dropped
fn tail(screen: &str, n: usize) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    lines[end.saturating_sub(n)..end].join("\n")
}

/// What a named tab is, for deciding which verb reaches it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An AI in a terminal, or a model tab: asked in words (`ask_tab`)
    Ai,
    /// A terminal running anything else: given a command (`tab_run`)
    Shell,
    /// A page: driven toward a goal (`browser_do`), with its pane
    Browser(usize),
    /// Nothing on this desk answers to it
    Missing,
}

/// What the tab `id` is on this desk. A page is found among the screens,
/// where its key is its id; everything else among the terminal tabs
pub fn kind_of(id: &str, surfaces: &[crate::view::Surface], tabs: &[Tab]) -> Kind {
    if let Some(pane) = surfaces
        .iter()
        .position(|s| matches!(s, crate::view::Surface::Browser { key, .. } if key == id))
    {
        return Kind::Browser(pane + 1);
    }
    match tabs
        .iter()
        .find(|t| t.id.as_deref() == Some(id) || t.called() == id)
    {
        Some(t) if t.is_ai() => Kind::Ai,
        Some(_) => Kind::Shell,
        None => Kind::Missing,
    }
}

/// The ids a person named in what they sent (`<@finch>`). Only these tabs may
/// be driven -- a shell typed into, a page operated -- by the AI that was
/// sent it, until the person sends that AI something else
pub fn named_in(text: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut rest = text;
    while let Some(at) = rest.find("<@") {
        rest = &rest[at + 2..];
        let Some(end) = rest.find('>') else { break };
        let id = &rest[..end];
        if !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            out.insert(id.to_string());
        }
        rest = &rest[end + 1..];
    }
    out
}

/// Why a verb does not reach a tab of the other kind, with the one that does
pub fn wrong_verb(verb: &str, id: &str, kind: Kind) -> Option<String> {
    let right = match kind {
        Kind::Ai => "ask",
        Kind::Shell => "run",
        Kind::Browser(_) => "do",
        Kind::Missing => return None,
    };
    if right == verb {
        return None;
    }
    let what = match kind {
        Kind::Ai => format!("<@{id}> is an AI: use `shikisha ask {id} \"what you want it to do\"`"),
        Kind::Shell => {
            format!("<@{id}> is a terminal, not an AI: use `shikisha run {id} \"a command\"`")
        }
        Kind::Browser(_) => {
            format!("<@{id}> is a web page: use `shikisha do {id} \"what to get done on it\"`")
        }
        Kind::Missing => unreachable!(),
    };
    Some(what)
}

/// A page being driven toward a goal for another tab (`browser_do`), waited on
pub struct WordsCall {
    pub reply: Option<Sender<Result<Value, String>>>,
    pub caller: Option<String>,
    pub target: String,
    pub pane: usize,
    pub goal: String,
    pub asked_at: Instant,
    pub deadline: Instant,
}

/// What the caller of `browser_do` is handed: how the run ended, and what the
/// page showed about the goal
pub fn words_answer(w: &WordsCall, code: i64, why: &str, found: &str) -> Value {
    let state = match code {
        0 => "DONE",
        -1 => "STOPPED",
        _ => "STUCK",
    };
    let reply = if found.trim().is_empty() { why } else { found };
    json!({
        "tab": w.target,
        "state": state,
        "reply": reply,
        "note": why,
        "source": "page",
        "seconds": w.asked_at.elapsed().as_secs(),
    })
}

/// What is typed into the caller's tab when the reply outlived the line
pub fn handed(target: &str, reply: &str) -> String {
    format!("[shikisha] The reply of <@{target}> to what you asked earlier:\n{reply}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    struct Record(std::path::PathBuf);
    impl Record {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Record {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn record(lines: &[Value]) -> Record {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("asktab-{}-{n}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        Record(path)
    }

    fn said(role: &str, text: &str) -> Value {
        json!({"type": role, "message": {"role": role, "content": [{"type": "text", "text": text}]}})
    }

    #[test]
    fn the_reply_is_what_followed_the_question_and_nothing_before_it() {
        let f = record(&[
            said("user", "an older question"),
            said("assistant", "an older answer"),
            said(
                "user",
                "What is in secret.txt?  Reply with only its contents.",
            ),
            said("assistant", "Let me look."),
            said("assistant", "ALPHA-42"),
        ]);
        let got = reply_in(
            f.path(),
            "What is in secret.txt? Reply with only its contents.",
        );
        // One turn of the AI, read as the reader reads it: its paragraphs together
        assert_eq!(
            got.as_deref(),
            Some(
                "Let me look.

ALPHA-42"
            )
        );
    }

    #[test]
    fn no_reply_until_the_question_is_in_the_record() {
        let f = record(&[
            said("user", "an older question"),
            said("assistant", "an older answer"),
        ]);
        assert_eq!(reply_in(f.path(), "a new question"), None);
        let f = record(&[said("user", "a new question")]);
        assert_eq!(reply_in(f.path(), "a new question"), None);
    }

    #[test]
    fn only_the_ids_written_as_mentions_are_named() {
        let got = named_in("Run the tests in <@shell-2>, then ask <@finch> -- not <@ bad> or <@x");
        assert!(got.contains("shell-2") && got.contains("finch"));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_tab_is_named_by_its_id_as_a_mention_or_bare() {
        let (t, _, _) = parse(&[json!("<@codex>"), json!("hi")]).unwrap();
        assert_eq!(t, "codex");
        let (t, x, w) = parse(&[json!("@codex"), json!("hi")]).unwrap();
        assert_eq!((t.as_str(), x.as_str(), w), ("codex", "hi", DEFAULT_WAIT));
        let (_, _, w) = parse(&[json!("codex"), json!("hi"), json!({"timeout_ms": 5000})]).unwrap();
        assert_eq!(w, Duration::from_secs(5));
        assert!(parse(&[json!("codex")]).is_err());
    }
}
