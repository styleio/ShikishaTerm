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
//! **The line for the chat.** Every ask carries a short line said beside it
//! (`crate::convo::confer`), and so does every answer. The answer's is asked
//! for when the tab ends its turn: a stop hook of this app's holds the end
//! back once, with the reply the tab just gave kept as the answer, and the
//! tab replies with the line alone ([`Ask::held`]). Taking the line from a
//! second message rather than from a command the tab runs keeps the answer
//! whole: an AI that ran a command after answering would say a word more
//! about it, and that word would be what the record calls its last.
//!
//! **What is never lost.** The caller may stop holding the line: its client
//! has a timeout of its own, or a person pressed Esc. The other tab goes on
//! working all the same, and when it finishes its reply goes into the
//! caller's inbox, and the caller is told so once it is free to read it.

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

/// The commands answered once another tab has finished, which can be an hour
/// of work: the pipe holds their line for [`LINE_HOLD`], and the `shikisha`
/// command gives them a wait shorter than an AI's shell will sit through
pub const HELD: [&str; 3] = ["ask_tab", "tab_run", "browser_do"];

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

/// How often the record is looked at for the turn's end while the tab still
/// reads as busy. The record is the CLI's own word and costs a file read, so
/// not every tick; a few seconds late on an answer the screen held back for
/// hours is nothing
const RECORD_AGAIN: Duration = Duration::from_secs(5);

/// How long the screen must have read as busy, without a break, before the
/// record is asked whether the turn is over. What this is for is a screen
/// wrong for minutes (five, for a Codex whose connection died) or a night; a
/// screen that turned busy a moment ago is more likely a NEW turn whose first
/// line the record has not been given yet -- measured 2026-09-30: an AI woken
/// by its own background job was handed the answer of the turn before
const RECORD_TRUSTED_AFTER: Duration = Duration::from_secs(30);

/// How long a tab held back at the end of its turn may take to say its line
/// before its answer is given its first sentence instead. Saying a line is
/// one short message; this is for a tab that never does
pub const LINE_WAIT: Duration = Duration::from_secs(90);

/// How long a tab held back for its line may sit idle before it is taken to
/// be saying nothing more. A tab told to go on starts working within seconds;
/// one that does not has let the end of its turn stand (a CLI can drop the
/// "go on", measured 2026-10-01 with Codex 0.155: 2 of the first 13 asks), and
/// a line it is not going to say is not waited for to the end of [`LINE_WAIT`]
const HELD_IDLE: Duration = Duration::from_secs(20);

/// How long a finished-looking tab expected to call the stop hook is given
/// for it, before its answer is read the old way. The hook runs as the turn
/// ends and reaches the app in a fraction of a second; the screen can read as
/// finished a moment before it does
const HOOK_GRACE: Duration = Duration::from_secs(5);

/// How many times a tab is asked for its line. Once, and once more with the
/// reason when what it said could not be taken; after that its first
/// sentence is used
pub const LINE_TRIES: u8 = 2;

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
    /// Who the caller is (`Tab::uid`), fixed as the ask was taken: the tab
    /// the answer goes back to. A caller closed since, and a tab given its
    /// name since, is not it. `None` when no tab was calling
    pub caller_uid: Option<String>,
    pub target: String,
    /// Who the tab asked is, fixed as the ask was taken: the tab followed,
    /// sent to and answered from. A tab closed since, and a tab given its
    /// name since, is not it. `None` when the name found no tab then, and the
    /// ask is followed by the name
    pub target_uid: Option<String>,
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
    /// When the record was last looked at for the turn's end while the
    /// screen still read as busy (see [`reply_when_over`])
    pub record_look: Option<Instant>,
    /// When the log was last told why the tab still reads as busy
    /// (see [`WHY_AGAIN`])
    pub why_said: Option<Instant>,
    /// How long the tab's record was when the words were sent: the question
    /// is looked for only after that (see [`asked_in`]). `None` until sent,
    /// for a tab not yet known to keep one then (a CLI writes its record at
    /// the first thing said: all of it is after), and for a record on
    /// another machine whose length could not be found out ([`Self::record_unsure`])
    pub record_from: Option<u64>,
    /// Since when the tab has read as busy without a break (see
    /// [`RECORD_TRUSTED_AFTER`])
    pub busy_since: Option<Instant>,
    /// How long a record on another machine is, being asked before the
    /// words go in (see [`far_len`]), and when it was asked
    pub far_len: Option<(std::sync::mpsc::Receiver<Result<u64, String>>, Instant)>,
    /// How long a record on another machine was as the words went in could
    /// not be found out. Its record is then not read for this answer: looked
    /// for anywhere in it, the question could be taken for an earlier one
    /// with the same opening words, and that one's answer handed back. The
    /// screen answers instead
    pub record_unsure: bool,
    /// The desk it was asked on, by uid: its tabs are the ones named, whichever
    /// desk is in front now. `None` is the desk in front
    pub desk: Option<String>,
    /// The asker's line for the chat (`crate::convo::confer`). Empty for a
    /// command typed into a terminal
    pub line: String,
    /// The ask as the conference keeps it, once sent
    pub ask_id: Option<i64>,
    /// The tab asked calls this app's stop hook as its turn ends (see
    /// `agenthook::LINE_ARG`): its answer waits a moment for it
    pub hook_expected: bool,
    /// The stop hook has been heard from for this ask
    pub hook_seen: bool,
    /// The reply the tab gave as its turn first ended, kept while it is asked
    /// for its line: the answer, whatever it says next
    pub held: Option<String>,
    /// When the reply was kept
    pub held_at: Option<Instant>,
    /// How many times the tab has been asked for its line
    pub line_asks: u8,
    /// The answer has its line, or will not get one of its own
    pub lined: bool,
    /// Since when a tab held back for its line has sat idle (see [`HELD_IDLE`])
    pub held_idle: Option<Instant>,
    /// The tab ended its turn again without the hook being told what it said
    /// last: its line is looked for in its record instead
    pub line_unheard: bool,
    /// The answer's line, found in the record rather than heard from the
    /// hook, for the loop to write down once the line is settled
    pub late_line: Option<String>,
    /// The caller has the answer kept in [`Self::held`]; the ask stays only
    /// for the answer's line (see [`Step::Lined`])
    pub answered: bool,
}

/// How long `t`'s record is now, for [`Ask::record_from`]. A record that
/// does not exist yet is empty: all of it will come after
pub fn record_len(t: &Tab) -> Option<u64> {
    t.record_at().filter(|r| !r.is_far())?;
    Some(t.record().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len()))
}

/// How often a tab that has held an answer back for a long time has the
/// reason it reads as busy written to the log. Not sooner than an ask's
/// ordinary wait: a turn of a few minutes is not a mystery
pub const WHY_AGAIN: Duration = Duration::from_secs(10 * 60);

impl Ask {
    /// Whether it is time to write down why the tab it waits on still reads
    /// as busy, and if so, notes that it was written
    pub fn time_to_say_why(&mut self, state: TabState) -> bool {
        let waited = self.sent_at.unwrap_or(self.asked_at).elapsed();
        if state != TabState::Busy || waited < WHY_AGAIN || self.why_said.is_some_and(|at| at.elapsed() < WHY_AGAIN) {
            return false;
        }
        self.why_said = Some(Instant::now());
        true
    }
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
    /// A look out on its thread, and whether it asked for the turn to be over
    /// too (see [`FarRead::reply`]). An answer to the other kind of look is
    /// not this one's
    pending: Option<(std::sync::mpsc::Receiver<Found>, bool)>,
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
    /// `None` while one is out or the record has not caught up.
    ///
    /// `from` is how long the record was when `sent` was sent, as for
    /// [`reply_in`]. `over` is the record-says-so road taken while the screen
    /// still reads as busy (see [`reply_when_over`]): the answer only counts
    /// once the record's last mark ends a turn, written after the question
    fn reply(&mut self, record: &crate::reader::Record, sent: &str, from: u64, over: bool) -> Option<String> {
        if let Some((rx, kind)) = &self.pending {
            let kind = *kind;
            match rx.try_recv() {
                Ok(Found::Reply(reply)) => {
                    self.pending = None;
                    return (kind == over).then_some(reply);
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
                Some(Ok(page)) => match asked_in(&page, &sent, from) {
                    None => Found::NotYet,
                    // The turn's end is known here only by when it was
                    // written: after the question, and nothing begun since
                    Some(asked) if over => {
                        let ended_after = |asked_when: i64| {
                            record.standing().and_then(|s| s.mark).is_some_and(|(mark, when)| {
                                mark == crate::reader::TurnMark::Over && when.is_some_and(|w| w >= asked_when)
                            })
                        };
                        match page.turns[asked].when {
                            Some(asked_when) if ended_after(asked_when) => {
                                reply_after(&page, asked).map_or(Found::NotYet, Found::Reply)
                            }
                            _ => Found::NotYet,
                        }
                    }
                    Some(asked) => reply_after(&page, asked).map_or(Found::NotYet, Found::Reply),
                },
                Some(Err(_)) => Found::NotYet,
            };
            let _ = tx.send(found);
        });
        self.pending = Some((rx, over));
        self.last = Some(Instant::now());
        None
    }
}

/// How long `record` is, looked up on a thread: for a record on another
/// machine, [`Ask::record_from`] has to be read over the network before the
/// words go out (see `runtime::tend_asks`)
pub fn far_len(record: crate::reader::Record) -> std::sync::mpsc::Receiver<Result<u64, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(record.length());
    });
    rx
}

/// How long the words may wait for [`far_len`] before they go without it --
/// and the answer is then read from the screen (see [`Ask::record_unsure`])
pub const FAR_LEN_WAIT: Duration = Duration::from_secs(10);

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
                if self.far.missing || self.record_unsure { Source::Screen } else { Source::Far(record) }
            }
            _ => t.record().map_or(Source::Screen, Source::Here),
        }
    }

    /// The reply in the record, from wherever it is kept. `None` while it has
    /// not been found (yet)
    fn recorded(&mut self, t: &Tab) -> Option<String> {
        match self.source(t) {
            Source::Here(path) => reply_in(&path, &self.text, self.record_from.unwrap_or(0)),
            Source::Far(record) => {
                let text = self.text.clone();
                self.far.reply(&record, &text, self.record_from.unwrap_or(0), false)
            }
            Source::Screen => None,
        }
    }
}

/// An answer that outlived the line it was asked on, waiting to be typed into
/// the caller's tab once that tab is free (a browser run, see `browser_do`)
pub fn handing(caller: String, caller_uid: Option<String>, target: String, text: String) -> Ask {
    let now = Instant::now();
    Ask {
        reply: None,
        caller: Some(caller),
        caller_uid,
        target,
        target_uid: None,
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
        record_look: None,
        why_said: None,
        record_from: None,
        busy_since: None,
        far_len: None,
        record_unsure: false,
        desk: None,
        line: String::new(),
        ask_id: None,
        hook_expected: false,
        hook_seen: false,
        held: None,
        held_at: None,
        line_asks: 0,
        lined: false,
        held_idle: None,
        line_unheard: false,
        late_line: None,
        answered: false,
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
    /// The answer went to the caller earlier, and its line is now said or
    /// given up on: write it down (the one found in the record, or the
    /// answer's first sentence) and forget the ask
    Lined,
    /// Forget it
    Drop,
}

/// Remove a wrapper only when it encloses the whole line. A quote around a
/// title or a code name inside the sentence belongs to what was said.
fn bare_line(said: &str) -> &str {
    let mut line = said.trim();
    loop {
        let inner = [('"', '"'), ('\'', '\''), ('`', '`'), ('「', '」'), ('“', '”')]
            .into_iter()
            .find_map(|(open, close)| {
                line.strip_prefix(open)?.strip_suffix(close).filter(|s| !s.contains([open, close]))
            });
        match inner {
            Some(s) => line = s.trim(),
            None => return line,
        }
    }
}

/// `tab_run(tab, command, {timeout_ms})` and `browser_do(tab, goal, ...)`,
/// taken apart. Only a tab's id names a tab here: a position changes when
/// tabs are moved, and a display name is not unique
pub fn parse(params: &[Value]) -> Result<(String, String, Duration), String> {
    let target = target_of(params, "the command")?;
    let text = params
        .get(1)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("this needs what to do second")?
        .to_string();
    Ok((target, text, wait_of(params.get(2))))
}

/// How an ask is written, for every refusal that has to say it
pub const ASK_FORM: &str = "ask_tab ID \"one short line for the chat\" \"everything you want it to do\"";

/// `ask_tab(tab, line, text, {timeout_ms})`, taken apart: the tab, the line
/// said beside the ask for the chat (checked against `line_max`, see
/// `crate::convo::confer::check_line`), everything the tab is sent, and how
/// long the caller waits.
///
/// The line comes second, before the text, the way a subject comes before a
/// letter. An ask written the old way, with the text second and nothing or
/// the options third, is refused with the new form rather than read in a way
/// nobody meant
pub fn parse_ask(params: &[Value], line_max: u32) -> Result<(String, String, String, Duration), String> {
    let target = target_of(params, "ask_tab")?;
    let text = match params.get(2) {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(Value::String(_)) => return Err(format!("ask_tab needs what to ask third: {ASK_FORM}")),
        _ => {
            return Err(format!(
                "ask_tab needs a short line for the chat second and what to ask third: {ASK_FORM}"
            ));
        }
    };
    let line = params.get(1).and_then(Value::as_str).unwrap_or_default();
    let line = crate::convo::confer::check_line(line, line_max, "the line (second)")?;
    Ok((target, line, text, wait_of(params.get(3))))
}

/// The tab named first, handed over the way it appears in a message --
/// `<@codex>` -- or bare
fn target_of(params: &[Value], what: &str) -> Result<String, String> {
    Ok(params
        .first()
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{what} needs the tab's id first"))?
        .trim()
        .trim_start_matches('<')
        .trim_start_matches('@')
        .trim_end_matches('>')
        .to_string())
}

/// How long a caller is held: `{timeout_ms}` if it says, and never past the line
fn wait_of(options: Option<&Value>) -> Duration {
    options
        .and_then(|o| o.get("timeout_ms"))
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_WAIT)
        .min(LINE_HOLD - Duration::from_secs(60))
}

/// What the stop hook of a tab ending its turn is answered, for the ask it
/// was answering (see [`Ask::held`]). `first` is what the tab said last;
/// `line_max` the settings' limit. `Some(reason)`: hold the end back, and
/// the reason is what the tab is told. `Hear::Line` is a line the tab said
#[derive(Debug, PartialEq, Eq)]
pub enum Hear {
    /// Let the turn end
    Go,
    /// Hold it: the tab is told this
    Hold(String),
    /// Let it end: this is the answer's line
    Line(String),
}

impl Ask {
    /// The tab gave its answer and has not yet said its line (nor been given
    /// up on): nothing more is sent to it, and the ask is kept for the line
    pub fn owes_line(&self) -> bool {
        self.held.is_some() && !self.lined
    }

    /// The tab asked ended its turn, saying `said` last
    pub fn hear_stop(&mut self, said: &str, line_max: u32) -> Hear {
        self.hook_seen = true;
        if self.lined || self.run.is_some() {
            return Hear::Go;
        }
        let said = said.trim();
        // The end of the turn it was told to go on in, with nothing said
        // in it as far as the hook knows. A CLI does not always hand the last
        // message to the hook (seen with Codex 0.155, 2026-10-01: its record
        // held the line, the hook got none) -- and asking again would be
        // refused, a turn goes on once. What it said is in its record
        if said.is_empty() && self.held.is_some() {
            self.line_unheard = true;
            return Hear::Go;
        }
        let Some(_) = self.held else {
            if said.is_empty() {
                // Nothing to pass on; the answer is read the ordinary way
                return Hear::Go;
            }
            self.held = Some(said.to_string());
            self.held_at = Some(Instant::now());
            self.line_asks = 1;
            return Hear::Hold(crate::convo::confer::stop_reason(line_max));
        };
        match crate::convo::confer::check_line(bare_line(said), line_max, "That line") {
            Ok(line) => {
                self.lined = true;
                Hear::Line(line)
            }
            Err(why) if self.line_asks < LINE_TRIES => {
                self.line_asks += 1;
                Hear::Hold(format!("{why}. Reply again with only the line."))
            }
            Err(_) => {
                self.lined = true;
                Hear::Go
            }
        }
    }
}

/// Words with their runs of space made single, for finding one text in another
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The reply to `sent` in the record at `path`: the last thing the AI said
/// after the turn in which it was given `sent`. `None` while that turn has not
/// been written, or the AI has not said anything since.
///
/// `from` is how long the record was when `sent` was sent (see
/// [`Ask::record_from`]): the question is looked for only after it
pub fn reply_in(path: &Path, sent: &str, from: u64) -> Option<String> {
    let page = crate::reader::read_back(path, u64::MAX, REPLY_TURNS).ok()?;
    reply_after(&page, asked_in(&page, sent, from)?)
}

/// How many things said are read back to find the turn a reply belongs to
const REPLY_TURNS: usize = 16;

/// The reply to `sent` in the record at `path`, but only once the record says
/// the turn it was given in has ended ([`crate::reader::turn_over_after`]).
///
/// This is the answer for a tab whose screen still reads as busy: the screen
/// is a guess at what the CLI is doing, the record is the CLI saying so. A
/// record with no marks of its own (Gemini) never answers here, and is left to
/// the screen as before. `from` as for [`reply_in`]
pub fn reply_when_over(path: &Path, sent: &str, from: u64) -> Option<String> {
    let page = crate::reader::read_back(path, u64::MAX, REPLY_TURNS).ok()?;
    let asked = asked_in(&page, sent, from)?;
    let begun = page.turns[asked].at?;
    if !crate::reader::turn_over_after(path, begun) {
        return None;
    }
    // And no turn has begun since. An AI that answered, left work running in
    // the background and was woken by it is still on the job: what it said
    // before it was woken is not the answer (measured 2026-09-30). The
    // ordinary road -- the screen going quiet -- waits for the answer then
    if crate::reader::last_turn_mark(path).map(|(mark, _)| mark) != Some(crate::reader::TurnMark::Over) {
        return None;
    }
    reply_after(&page, asked)
}

/// Where in a page of the record `sent` was given: the last turn of the
/// person's that holds its opening words, written at or after byte `from`.
///
/// The opening words alone are not enough. Asked twice in a row with the
/// same opening, the record looked at the moment after the second ask still
/// holds only the first -- and handed back the first ask's answer, five
/// seconds in, for a question that takes ninety (measured 2026-09-30). A turn
/// that says nothing of where it starts is not taken once `from` means
/// anything
fn asked_in(page: &crate::reader::Page, sent: &str, from: u64) -> Option<usize> {
    let want: String = flat(sent).chars().take(MATCH_CHARS).collect();
    page.turns.iter().rposition(|t| {
        t.who == Who::You
            && flat(&t.text).contains(&want)
            && (from == 0 || t.at.is_some_and(|at| at >= from))
    })
}

/// The last thing the AI said after the turn at `asked`
fn reply_after(page: &crate::reader::Page, asked: usize) -> Option<String> {
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
///
/// `shown` is whether the tab's desk is the one in front: words for a tab on
/// a desk not in front wait for it (they are typed through the desk in
/// front), and the wait for them is the same wait, with the same end
pub fn step(
    a: &mut Ask,
    target: Option<&Tab>,
    caller_free: bool,
    same_folder: Option<bool>,
    shown: bool,
) -> Step {
    let now = Instant::now();
    if let Phase::Handing(text) = &a.phase {
        return if caller_free {
            Step::Hand(text.clone())
        } else {
            Step::Nothing
        };
    }
    if a.answered {
        return line_step(a, target);
    }
    // The tab gave its answer and is saying its line. The answer is the reply
    // kept as its turn first ended, whatever it says next -- its line said
    // already, or the tab closed since -- and it goes to the caller at once:
    // the line is for the chat, and nobody waits on it. The ask stays for the
    // line (`line_step`)
    if a.held.is_some() && !a.answered {
        a.answered = true;
        return Step::Answer(answer(a, "DONE", a.held.as_deref(), "record", same_folder, None));
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
        if turn_over(t.state) && shown {
            return Step::Send;
        }
        if now >= a.deadline && !shown {
            return Step::Answer(answer(
                a,
                "BUSY",
                None,
                "none",
                same_folder,
                Some("the tab's desk was not shown before the wait ran out; nothing was sent"),
            ));
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
    if t.state != TabState::Busy {
        a.busy_since = None;
    }
    match t.state {
        TabState::Busy => {
            a.seen_busy = true;
            a.quiet_since = None;
            a.background_since = None;
            let busy_since = *a.busy_since.get_or_insert(now);
            // The screen says busy; the record may know better. A record here
            // is read on the spot; one on another machine through `FarRead`,
            // on a thread, a look every few seconds
            if a.run.is_none() && busy_since.elapsed() >= RECORD_TRUSTED_AFTER {
                let over = match a.source(t) {
                    Source::Here(path) if a.record_look.is_none_or(|at| at.elapsed() >= RECORD_AGAIN) => {
                        a.record_look = Some(now);
                        reply_when_over(&path, &a.text, a.record_from.unwrap_or(0))
                    }
                    Source::Far(record) => {
                        let text = a.text.clone();
                        a.far.reply(&record, &text, a.record_from.unwrap_or(0), true)
                    }
                    _ => None,
                };
                if let Some(reply) = over {
                    return Step::Answer(answer(
                        a,
                        "DONE",
                        Some(&reply),
                        "record",
                        same_folder,
                        Some("the tab's own record says the turn ended, though its screen still looked busy"),
                    ));
                }
            }
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
                    Some("still running; when it finishes you will be told in your tab, and shikisha inbox has its output"),
                ))
            } else {
                Step::Nothing
            };
        }
        // A tab that calls the stop hook is given a moment to: the screen can
        // read as finished just before the turn's end reaches the hook
        if a.hook_expected && !a.hook_seen && since.elapsed() < HOOK_GRACE {
            return Step::Nothing;
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
            Some("still working; when it finishes you will be told in your tab, and shikisha inbox has its reply"),
        ));
    }
    Step::Nothing
}

/// One tick of an ask whose answer went to the caller and whose line is still
/// to come. It waits for the line only so long: not past the tab stopping in
/// another way (a question, a limit, an error, the end of its program, the
/// tab closed), nor past it sitting idle, nor past [`LINE_WAIT`]. The caller's
/// own wait has no say in it -- the caller has its answer, and a caller that
/// let go of the line long ago (the `shikisha` command hands its wait over
/// after 100 s) would end every longer answer's line before it was said
fn line_step(a: &mut Ask, target: Option<&Tab>) -> Step {
    let (Some(t), Some(reply)) = (target, a.held.clone()) else {
        a.lined = true;
        return Step::Lined;
    };
    let now = Instant::now();
    let stopped = matches!(t.state, TabState::Question | TabState::Exited | TabState::Limit | TabState::Failed);
    if quiet(t.state) {
        a.held_idle.get_or_insert(now);
    } else {
        a.held_idle = None;
    }
    let idle = a.held_idle.is_some_and(|at| at.elapsed() >= HELD_IDLE);
    // Its line, from its record, when the hook did not bring it: the last
    // thing it said, if that is not the answer itself and can be a line
    if !a.lined
        && (a.line_unheard || idle)
        && let Some(last) = a.recorded(t)
        && last.trim() != reply.trim()
        && let Ok(line) = crate::convo::confer::check_line(bare_line(&last), crate::config::confer().line_max, "the line")
    {
        a.late_line = Some(line);
        a.lined = true;
    }
    if a.lined || stopped || idle || a.held_at.is_some_and(|at| at.elapsed() >= LINE_WAIT) {
        a.lined = true;
        return Step::Lined;
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

/// What a named tab is, for deciding which command reaches it
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

/// Why a command does not reach a tab of the other kind, with the one that does
pub fn wrong_command(command: &str, id: &str, kind: Kind) -> Option<String> {
    let right = match kind {
        Kind::Ai => "ask_tab",
        Kind::Shell => "tab_run",
        Kind::Browser(_) => "browser_do",
        Kind::Missing => return None,
    };
    if right == command {
        return None;
    }
    let what = match kind {
        Kind::Ai => format!("<@{id}> is an AI: use ask_tab (`shikisha ask_tab {id} \"a line for the chat\" \"what you want it to do\"`)"),
        Kind::Shell => {
            format!("<@{id}> is a terminal, not an AI: use tab_run (`shikisha tab_run {id} \"a command\"`)")
        }
        Kind::Browser(_) => {
            format!("<@{id}> is a web page: use browser_do (`shikisha browser_do {id} \"what to get done on it\"`)")
        }
        Kind::Missing => unreachable!(),
    };
    Some(what)
}

/// A page being driven toward a goal for another tab (`browser_do`), waited on
pub struct WordsCall {
    pub reply: Option<Sender<Result<Value, String>>>,
    pub caller: Option<String>,
    /// Who the caller is, fixed as the run was asked for: the tab a late
    /// answer is typed into ([`handing`])
    pub caller_uid: Option<String>,
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

/// What a reply that outlived the line it was asked on is filed under in the
/// caller's inbox (the caller is told it is there once it is free)
pub fn handed_subject(target: &str) -> String {
    format!("The reply of <@{target}> to what you asked earlier")
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
    fn an_ask_carries_a_line_for_the_chat_and_the_old_form_is_refused_with_the_new() {
        let (t, line, text, _) = parse_ask(&[json!("<@otter>"), json!(" Review this? "), json!("the diff in src/p.rs")], 80).unwrap();
        assert_eq!((t.as_str(), line.as_str(), text.as_str()), ("otter", "Review this?", "the diff in src/p.rs"));
        let old = parse_ask(&[json!("otter"), json!("review the diff in src/p.rs")], 80).unwrap_err();
        assert!(old.contains(ASK_FORM), "{old}");
        let old = parse_ask(&[json!("otter"), json!("review"), json!({"timeout_ms": 5})], 80).unwrap_err();
        assert!(old.contains(ASK_FORM), "the options where the text now goes: {old}");
        let long = parse_ask(&[json!("otter"), json!("x".repeat(81)), json!("y")], 80).unwrap_err();
        assert!(long.contains("the most is 80"), "{long}");
        let (_, _, _, wait) = parse_ask(&[json!("otter"), json!("a"), json!("b"), json!({"timeout_ms": 5000})], 80).unwrap();
        assert_eq!(wait, Duration::from_millis(5000));
        // A command keeps its old form
        assert_eq!(parse(&[json!("sh"), json!("make")]).unwrap().1, "make");
    }

    fn sent_ask() -> Ask {
        let mut a = handing("caller".into(), None, "otter".into(), String::new());
        a.phase = Phase::Waiting;
        a.text = "review it".into();
        a.deadline = Instant::now() + Duration::from_secs(600);
        a.hook_expected = true;
        a
    }

    #[test]
    fn a_line_keeps_quotes_that_belong_to_the_sentence() {
        for line in ["「白鳥」、弾けたら素敵だね！", "`readDocument` を直しました。", "\"read\" calls \"check\"", "「認証」と「認可」"] {
            assert_eq!(bare_line(line), line);
        }
        assert_eq!(bare_line("  \"Ship it.\"  "), "Ship it.");
        assert_eq!(bare_line("「修正を確認しました。」"), "修正を確認しました。");
        assert_eq!(bare_line("`\"Ready.\"`"), "Ready.");
    }

    #[test]
    fn the_end_of_a_turn_keeps_the_reply_and_asks_once_for_the_line() {
        let mut a = sent_ask();
        let Hear::Hold(why) = a.hear_stop("Two findings: the parser drops the tail; the test is missing.", 80) else {
            panic!("the first end is held back")
        };
        assert!(why.contains("only one short line") && why.contains("at most 80 characters"), "{why}");
        assert_eq!(a.held.as_deref(), Some("Two findings: the parser drops the tail; the test is missing."));
        // Said too long: asked again with the reason, the reply kept as it was
        let Hear::Hold(again) = a.hear_stop(&"x".repeat(200), 80) else { panic!("asked again") };
        assert!(again.contains("the most is 80"), "{again}");
        assert_eq!(a.hear_stop("\"Two small things, both fixable.\"", 80), Hear::Line("Two small things, both fixable.".into()));
        assert!(a.lined);
        assert!(a.held.as_deref().unwrap().starts_with("Two findings"), "the answer is the reply, not the line");
        assert_eq!(a.hear_stop("anything", 80), Hear::Go, "once it has its line, the turn ends");
    }

    #[test]
    fn a_reply_kept_is_the_answer_and_goes_to_the_caller_at_once() {
        let mut t = Tab::spawn("otter".into(), &[crate::test_shell()], None, 10, 40, Default::default()).unwrap();
        // Busy saying its line: the caller is not kept waiting for it
        t.state = TabState::Busy;
        let mut a = sent_ask();
        assert!(matches!(a.hear_stop("The memo says ABC.", 80), Hear::Hold(_)));
        let Step::Answer(v) = step(&mut a, Some(&t), false, None, true) else { panic!("the caller was kept waiting for a line") };
        assert_eq!((v["state"].as_str(), v["reply"].as_str()), (Some("DONE"), Some("The memo says ABC.")));
        assert!(a.answered && a.owes_line(), "the ask is kept for the line");
        assert!(matches!(step(&mut a, Some(&t), false, None, true), Step::Nothing), "answered once, then the line is waited for");
        // Its line said: written down, and the ask forgotten
        assert_eq!(a.hear_stop("Read it: ABC.", 80), Hear::Line("Read it: ABC.".into()));
        assert!(!a.owes_line());
        assert!(matches!(step(&mut a, Some(&t), false, None, true), Step::Lined));
    }

    #[test]
    fn the_answer_kept_is_passed_on_even_when_the_line_or_the_closing_came_first() {
        let mut t = Tab::spawn("otter".into(), &[crate::test_shell()], None, 10, 40, Default::default()).unwrap();
        t.state = TabState::Busy;
        // The stop hook's calls are served before the asks are stepped: the
        // line can be heard before the answer has gone anywhere
        let mut a = sent_ask();
        assert!(matches!(a.hear_stop("The memo says ABC.", 80), Hear::Hold(_)));
        assert_eq!(a.hear_stop("Read it: ABC.", 80), Hear::Line("Read it: ABC.".into()));
        let Step::Answer(v) = step(&mut a, Some(&t), false, None, true) else { panic!("the answer was not passed on") };
        assert_eq!(v["reply"].as_str(), Some("The memo says ABC."), "the line was taken for the answer");
        assert!(!a.owes_line(), "nothing left to wait for");
        // The tab closed between giving its answer and the next look: the
        // answer it gave still goes, not "the tab was closed"
        let mut a = sent_ask();
        assert!(matches!(a.hear_stop("The memo says ABC.", 80), Hear::Hold(_)));
        let Step::Answer(v) = step(&mut a, None, false, None, true) else { panic!("the answer was not passed on") };
        assert_eq!((v["state"].as_str(), v["reply"].as_str()), (Some("DONE"), Some("The memo says ABC.")));
        assert!(matches!(step(&mut a, None, false, None, true), Step::Lined), "the closed tab says no line");
    }

    #[test]
    fn a_line_is_waited_for_past_the_callers_wait_but_not_past_the_tab_stopping() {
        let mut t = Tab::spawn("otter".into(), &[crate::test_shell()], None, 10, 40, Default::default()).unwrap();
        let answered = |t: &Tab| {
            let mut a = sent_ask();
            assert!(matches!(a.hear_stop("The memo says ABC.", 80), Hear::Hold(_)));
            assert!(matches!(step(&mut a, Some(t), false, None, true), Step::Answer(_)));
            a
        };
        // The caller let go long ago (the `shikisha` command hands its wait
        // over after 100 s, and every longer answer ended here): the line is
        // still waited for. Measured 2026-10-02: 3 of 3 such answers lost it
        t.state = TabState::Busy;
        let mut a = answered(&t);
        a.phase = Phase::Deliver;
        a.deadline = Instant::now() - Duration::from_secs(600);
        assert!(matches!(step(&mut a, Some(&t), false, None, true), Step::Nothing), "the caller's wait ended the line");
        // ...but not past the line's own wait
        a.held_at = Some(Instant::now() - LINE_WAIT);
        assert!(matches!(step(&mut a, Some(&t), false, None, true), Step::Lined));
        // Nor past the tab stopping in another way, or closing
        for state in [TabState::Question, TabState::Limit, TabState::Failed, TabState::Exited] {
            t.state = TabState::Busy;
            let mut a = answered(&t);
            t.state = state;
            assert!(matches!(step(&mut a, Some(&t), false, None, true), Step::Lined), "{state:?}");
        }
        t.state = TabState::Busy;
        let mut a = answered(&t);
        assert!(matches!(step(&mut a, None, false, None, true), Step::Lined), "the tab closed");
        // Nor while it sits idle, told to go on and not going
        t.state = TabState::Done;
        let mut a = answered(&t);
        assert!(matches!(step(&mut a, Some(&t), false, None, true), Step::Nothing), "idle a moment: still waiting");
        a.held_idle = Some(Instant::now() - HELD_IDLE);
        assert!(matches!(step(&mut a, Some(&t), false, None, true), Step::Lined), "idle too long");
    }

    #[test]
    fn a_line_the_hook_did_not_hear_is_read_from_the_record() {
        let rec = record(&[
            said("user", "review it"),
            said("assistant", "The memo says ABC."),
            said("user", "Your answer has been passed on in full ..."),
            said("assistant", "I sent the memo's exact text."),
        ]);
        let mut a = sent_ask();
        assert!(matches!(a.hear_stop("The memo says ABC.", 80), Hear::Hold(_)));
        assert_eq!(a.hear_stop("", 80), Hear::Go, "nothing heard: the turn ends, not held again");
        assert!(a.line_unheard && !a.lined);
        // The record is where the line is (a tab with no record of its own
        // leaves it to the first sentence)
        let last = reply_in(rec.path(), "review it", 0).unwrap();
        assert_eq!(last, "I sent the memo's exact text.");
        assert_eq!(bare_line("\u{201c}Done.\u{201d}"), "Done.");
    }

    #[test]
    fn a_line_that_cannot_be_taken_twice_lets_the_turn_end() {
        let mut a = sent_ask();
        assert!(matches!(a.hear_stop("The answer.", 80), Hear::Hold(_)));
        assert!(matches!(a.hear_stop("one\ntwo", 80), Hear::Hold(_)));
        assert_eq!(a.hear_stop("one\ntwo", 80), Hear::Go);
        assert!(a.lined, "no line of its own: its first sentence is taken");
        let mut quiet = sent_ask();
        assert_eq!(quiet.hear_stop("   ", 80), Hear::Go, "nothing said, nothing to keep");
        let mut run = sent_ask();
        run.run = Some(RunFrom::default());
        assert_eq!(run.hear_stop("done", 80), Hear::Go, "a terminal's command owes no line");
    }

    /// The night of 2026-09-30: a Codex tab finished at 00:58 and its screen
    /// went on reading as busy until 09:26, so the answer sat in its record
    /// all night. The record said the turn was over; that is enough
    #[test]
    fn a_turn_the_record_says_is_over_is_answered_whatever_the_screen_says() {
        let q = "Review far-keep-plan.ja.md adversarially and reply with the findings only.";
        let codex_said = |role: &str, kind: &str, text: &str| {
            json!({"type": "response_item", "payload": {"type": "message", "role": role, "content": [{"type": kind, "text": text}]}})
        };
        let event = |kind: &str| json!({"type": "event_msg", "payload": {"type": kind, "turn_id": "t4"}});
        let mut lines = vec![
            codex_said("user", "input_text", "an earlier review"),
            codex_said("assistant", "output_text", "the earlier findings"),
            event("task_complete"),
            event("task_started"),
            codex_said("user", "input_text", q),
            codex_said("assistant", "output_text", "Reading the plan first."),
        ];
        let working = record(&lines);
        assert_eq!(
            reply_when_over(working.path(), q, 0),
            None,
            "a turn still going on is not answered from the record, however much it has said"
        );
        lines.push(codex_said("assistant", "output_text", "Section 5 is not enough: ..."));
        lines.push(event("task_complete"));
        let over = record(&lines);
        // What the AI said in one breath is read as one answer, as `reply_in` reads it
        let reply = reply_when_over(over.path(), q, 0).expect("the turn is over and its answer is in the record");
        assert!(reply.ends_with("Section 5 is not enough: ..."), "{reply}");
        assert_eq!(Some(reply), reply_in(over.path(), q, 0), "the same answer the quiet screen would have fetched");
        // The end of an EARLIER turn is not the end of this one
        let early = record(&lines[..5]);
        assert_eq!(reply_when_over(early.path(), q, 0), None);
    }

    /// Claude Code: an answer that stops for a tool is the middle of the turn
    #[test]
    fn a_claude_turn_is_over_at_the_answer_that_says_it_ended() {
        let q = "What is in secret.txt? Reply with only its contents.";
        let stop = |text: &str, why: &str| {
            json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": why, "content": [{"type": "text", "text": text}]}})
        };
        let mid = record(&[said("user", q), stop("Let me look.", "tool_use")]);
        assert_eq!(reply_when_over(mid.path(), q, 0), None);
        // Measured 2026-09-30: the model's thinking is filed first, stamped
        // with the answer's `end_turn`, minutes before the words arrive. An
        // aside said on the way is not the answer
        let thinking = json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": ""}]}});
        let early = record(&[said("user", q), thinking.clone(), stop("Let me look.", "tool_use")]);
        assert_eq!(reply_when_over(early.path(), q, 0), None, "a line of thinking is not the end of the turn");
        // The line Claude Code files once, last, at the end of every turn
        let ended = record(&[
            said("user", q),
            thinking,
            stop("Let me look.", "tool_use"),
            json!({"type": "system", "subtype": "turn_duration", "durationMs": 9000}),
        ]);
        assert!(reply_when_over(ended.path(), q, 0).is_some_and(|r| r.ends_with("Let me look.")));
        let done = record(&[said("user", q), stop("Let me look.", "tool_use"), stop("ALPHA-42", "end_turn")]);
        let reply = reply_when_over(done.path(), q, 0).expect("the turn ended");
        assert!(reply.ends_with("ALPHA-42"), "{reply}");
        assert_eq!(Some(reply), reply_in(done.path(), q, 0));
    }

    /// Measured 2026-09-30: an AI answered "still waiting", left the wait in
    /// the background and ended its turn; woken by that job, it began a new
    /// one, and its screen turned busy. The turn asked in is over -- but it is
    /// still on the job, and what it said before is not the answer
    #[test]
    fn a_turn_begun_since_holds_the_answer_back() {
        let q = "Wait 90 seconds, then tell me what is in memo-3.txt.";
        let turn_end = json!({"type": "system", "subtype": "turn_duration", "durationMs": 15000});
        let mut lines = vec![
            said("user", q),
            json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": "end_turn", "content": [{"type": "text", "text": "Waiting in the background; I will read it after."}]}}),
            turn_end.clone(),
        ];
        let answered = record(&lines);
        assert!(reply_when_over(answered.path(), q, 0).is_some(), "nothing began since: that is the answer");
        lines.push(json!({"type": "user", "message": {"role": "user", "content": "<task-notification>the wait is over</task-notification>"}}));
        let woken = record(&lines);
        assert_eq!(reply_when_over(woken.path(), q, 0), None, "a new turn is going on");
        lines.push(json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": "end_turn", "content": [{"type": "text", "text": "memo-3 says ZETA-9"}]}}));
        lines.push(turn_end);
        let done = record(&lines);
        assert!(reply_when_over(done.path(), q, 0).is_some_and(|r| r.ends_with("memo-3 says ZETA-9")));
    }

    /// A record with no marks (Gemini) is never answered from here: the
    /// screen goes on deciding, as before
    #[test]
    fn a_record_without_marks_leaves_it_to_the_screen() {
        let q = "Summarise the log.";
        let f = record(&[
            json!({"type": "user", "content": q}),
            json!({"type": "gemini", "content": "The log shows three errors."}),
        ]);
        assert_eq!(reply_when_over(f.path(), q, 0), None);
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
            0,
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
        assert_eq!(reply_in(f.path(), "a new question", 0), None);
        let f = record(&[said("user", "a new question")]);
        assert_eq!(reply_in(f.path(), "a new question", 0), None);
    }

    /// Measured 2026-09-30: the same question asked twice in a row, looked for
    /// five seconds after the second ask, found the FIRST ask in the record and
    /// handed back its answer. Only what was written after the ask counts
    #[test]
    fn a_question_asked_again_is_not_answered_with_the_last_answer() {
        let q = "Run sleep 90, then tell me what is in secret-7.txt.";
        let turn_end = json!({"type": "system", "subtype": "turn_duration", "durationMs": 91000});
        let first = [
            said("user", q),
            json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": "end_turn", "content": [{"type": "text", "text": "OLD-ANSWER"}]}}),
            turn_end.clone(),
        ];
        let before = record(&first);
        let asked_at = std::fs::metadata(before.path()).unwrap().len();
        // Right after the second ask: the record does not hold it yet
        assert_eq!(reply_when_over(before.path(), q, asked_at), None, "the last ask's answer is not this one's");
        assert_eq!(reply_in(before.path(), q, asked_at), None);
        // Without knowing where the record stood, the old answer is what is found -- the bug
        assert_eq!(reply_in(before.path(), q, 0).as_deref(), Some("OLD-ANSWER"));
        // Once the second ask and its answer are written, that is what comes back
        let mut both = first.to_vec();
        both.push(said("user", q));
        both.push(json!({"type": "assistant", "message": {"role": "assistant", "stop_reason": "end_turn", "content": [{"type": "text", "text": "NEW-ANSWER"}]}}));
        both.push(turn_end);
        let after = record(&both);
        assert_eq!(reply_when_over(after.path(), q, asked_at).as_deref(), Some("NEW-ANSWER"));
        assert_eq!(reply_in(after.path(), q, asked_at).as_deref(), Some("NEW-ANSWER"));
    }

    #[test]
    fn only_the_ids_written_as_mentions_are_named() {
        let got = named_in("Run the tests in <@shell-2>, then ask <@finch> -- not <@ bad> or <@x");
        assert!(got.contains("shell-2") && got.contains("finch"));
        assert_eq!(got.len(), 2);
        // Japanese puts no spaces around it, and the input bar adds none
        let got = named_in("テストを直して、<@codex>にレビューしてもらって");
        assert!(got.contains("codex") && got.len() == 1, "{got:?}");
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
