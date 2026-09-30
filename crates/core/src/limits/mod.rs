//! What an AI subscription has left.
//!
//! A window running several agents is a window where "can I start one more"
//! is the question, and the answer is a number the CLI already knows: how
//! much of each allowance is used, and when each resets. It is shown as one
//! pill while a tab of that AI is in view, and in full on the settings screen.
//!
//! Each AI is read where its own CLI keeps it, one file per AI:
//!
//! - **Claude** ([`claude`]) is asked of Claude's service the way Claude Code
//!   asks it, with the sign-in Claude Code keeps on this PC.
//! - **Codex** ([`codex`]) is read off the session records Codex writes on
//!   this PC. Nothing is sent anywhere.
//! - **Kimi**, **Grok**, **OpenCode Go**, **Cursor** and **Gemini** are asked
//!   of their services with the sign-in their own CLI (or, for Cursor, the
//!   editor) keeps on this PC.
//!
//! **Read, never written.** Every sign-in here belongs to another program,
//! which renews it when it runs. One that has run out is simply "no reading"
//! until that program has renewed it: writing a renewed one back could sign
//! out a copy of that program that is running this minute.
//!
//! **Nothing here may stop the program.** None of these places is
//! documented, so any of them will change one day without notice. Every step
//! answers `None` instead of failing: no sign-in, a sign-in past its time, a
//! service that does not answer, a body that is not the shape it was -- each
//! of those is "no pill", never an error on screen, and never a stalled
//! frame: the reading is done on a thread of its own, behind a mutex the
//! drawing only glances at.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod claude;
mod codex;
mod cursor;
mod gemini;
mod grok;
mod kimi;
mod opencode;

pub use claude::parse;

/// One allowance window, as the service reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// Whole percent used, 0-100
    pub pct: u32,
    /// When it resets, as seconds since the epoch, when the service says
    pub resets_at: Option<i64>,
}

impl Window {
    /// A percentage used, however precisely the service wrote it
    fn of_percent(pct: f64, resets_at: Option<i64>) -> Window {
        Window { pct: pct.round().clamp(0.0, 100.0) as u32, resets_at }
    }

    /// Whether the window has started again since it was read. Its number
    /// then belongs to the window before, and what the new one holds is not
    /// known until the next reading -- the AI may have run since, here or
    /// somewhere this program cannot see. The number is kept as read; saying
    /// what it is now is for the next reading, not a guess
    pub fn reset_by(&self, now: i64) -> bool {
        self.resets_at.is_some_and(|at| at <= now)
    }
}

/// How long a window runs before it starts again, when that is one of the
/// lengths a person already knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Hours5,
    Days7,
    /// A billing month: as long as the service's month is, reset on its date
    Month,
}

impl Span {
    /// The word the settings screen and the page look this up by
    pub fn key(self) -> &'static str {
        match self {
            Span::Hours5 => "five",
            Span::Days7 => "week",
            Span::Month => "month",
        }
    }

    fn of_key(k: &str) -> Option<Span> {
        [Span::Hours5, Span::Days7, Span::Month].into_iter().find(|s| s.key() == k)
    }

    /// By its length in minutes, when that is one of the named lengths. A
    /// window of any other length is left out rather than shown under a name
    /// that is not its own
    fn of_minutes(m: i64) -> Option<Span> {
        match m {
            300 => Some(Span::Hours5),
            10_080 => Some(Span::Days7),
            _ => None,
        }
    }
}

/// One allowance: a window, how long it runs, and -- for an allowance that
/// counts only some models -- which.
#[derive(Debug, Clone, PartialEq)]
pub struct Allowance {
    /// `None` when the service does not say how long the window is
    pub span: Option<Span>,
    /// The model this allowance is limited to ("Fable", "gemini-2.5-pro"),
    /// or `None` for the subscription as a whole
    pub only: Option<String>,
    pub window: Window,
}

impl Allowance {
    fn whole(span: Span, window: Window) -> Allowance {
        Allowance { span: Some(span), only: None, window }
    }
}

/// Every allowance read at once, and when.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    /// In the order a person reads them: the whole subscription, shortest
    /// window first, then the ones limited to a model
    pub wins: Vec<Allowance>,
    /// When the reading was taken, as seconds since the epoch, when that is
    /// not "just now". A Codex reading is as old as the last turn Codex took
    /// on this PC, and a reading that could be hours old says so
    pub as_of: Option<i64>,
    /// When the reading was taken, however new: every number shown says
    /// when it was read
    pub taken: Option<i64>,
    /// Read off a record the CLI wrote rather than asked of a service: its
    /// time is when the CLI last ran here, not when this program asked
    pub from_record: bool,
}

impl Limits {
    /// A reading of these allowances, put in reading order; `None` when there
    /// are none -- a body with nothing in it is no reading
    fn of(mut wins: Vec<Allowance>, as_of: Option<i64>) -> Option<Limits> {
        if wins.is_empty() {
            return None;
        }
        let rank = |a: &Allowance| {
            let span = match a.span {
                Some(Span::Hours5) => 0,
                Some(Span::Days7) => 1,
                Some(Span::Month) => 2,
                None => 3,
            };
            (a.only.is_some(), span)
        };
        wins.sort_by_key(rank);
        Some(Limits { wins, as_of, taken: as_of, from_record: false })
    }

    /// The whole subscription's window of this length
    pub fn whole(&self, span: Span) -> Option<&Window> {
        self.wins.iter().find(|a| a.only.is_none() && a.span == Some(span)).map(|a| &a.window)
    }
}

/// Whose allowance: the AI a tab runs, by the name the tabs know it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Claude,
    Codex,
    Kimi,
    Grok,
    OpenCode,
    Cursor,
    Gemini,
}

impl Source {
    pub const ALL: [Source; 7] =
        [Source::Claude, Source::Codex, Source::Gemini, Source::Kimi, Source::Grok, Source::OpenCode, Source::Cursor];

    /// The tab's AI kind this allowance belongs to (`Tab::ai_kind`)
    pub fn key(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
            Source::Kimi => "kimi",
            Source::Grok => "grok",
            Source::OpenCode => "opencode",
            Source::Cursor => "cursor-agent",
            Source::Gemini => "gemini",
        }
    }

    /// The AI's name, as a person reads it beside the reading
    pub fn name(self) -> &'static str {
        match self {
            Source::Claude => "Claude",
            Source::Codex => "Codex",
            Source::Kimi => "Kimi",
            Source::Grok => "Grok",
            Source::OpenCode => "OpenCode Go",
            Source::Cursor => "Cursor",
            Source::Gemini => "Gemini",
        }
    }

    /// How often to read while somebody could be looking.
    ///
    /// Codex's is a file on this disk, so it can be looked at often enough
    /// to follow the turns as they finish. Claude's is a request its own
    /// service turns away when it comes too often (see [`QUIET_FIRST`]).
    /// The others are requests to services this program has no measurement
    /// of: two minutes follows a 5-hour window closely enough to be worth
    /// reading, and is thirty asks an hour at most, well under what a CLI
    /// asking once per turn does on its own
    fn every(self) -> Duration {
        match self {
            Source::Codex => Duration::from_secs(15),
            Source::Claude => Duration::from_secs(60),
            _ => Duration::from_secs(120),
        }
    }

    /// The reading as it stands. Blocks for as long as the asking takes (up
    /// to [`ASK`] per request), so it is called off the drawing thread.
    ///
    /// What is asked of a service goes through its [`Book`]: a reading taken
    /// moments ago is handed out again rather than asked for twice, a service
    /// that has stopped answering is asked less and less often, and while it
    /// is not answering the last good reading is handed out with its time on it
    pub fn read(self) -> Option<Limits> {
        let now = now_ms() / 1000;
        match self {
            // A panic anywhere in the reading is one more "no answer"
            Source::Codex => codex::newest(
                asked(self, now, codex::ask, codex::account),
                std::panic::catch_unwind(|| codex::reading(now)).unwrap_or(None),
            ),
            Source::Claude => asked(self, now, claude::ask, claude::account),
            Source::Kimi => asked(self, now, kimi::ask, kimi::account),
            Source::Grok => asked(self, now, grok::ask, grok::account),
            Source::OpenCode => asked(self, now, opencode::ask, opencode::account),
            Source::Cursor => asked(self, now, cursor::ask, cursor::account),
            Source::Gemini => asked(self, now, gemini::ask, gemini::account),
        }
    }

    /// How old a reading may be and still stand on the status line. A
    /// service's numbers move while nobody can see them; Codex's are as new
    /// as the last turn here, and a window whose reset has passed is already
    /// shown as such
    fn stale_after(self) -> Option<i64> {
        match self {
            Source::Codex => None,
            _ => Some(KEEP.as_secs() as i64),
        }
    }

    /// Whether this PC has what the reading needs: the AI's sign-in (or, for
    /// Codex, its having been used here at all).
    ///
    /// Whether, never what: the settings screen says why the pill is or is
    /// not there, and nothing hands a token back out
    pub fn ready(self) -> bool {
        match self {
            Source::Claude => claude::signed_in(),
            Source::Codex => codex::used_here(),
            Source::Kimi => kimi::signed_in(),
            Source::Grok => grok::signed_in(),
            Source::OpenCode => opencode::signed_in(),
            Source::Cursor => cursor::signed_in(),
            Source::Gemini => gemini::signed_in(),
        }
    }
}

/// How long the last good answer is shown once the service stops answering.
/// "Resets in 3h" is still roughly true ten minutes on; after that it is a
/// number pretending to be current
const KEEP: Duration = Duration::from_secs(10 * 60);
/// How long one ask may take. The thread is the app's, not the service's
const ASK: Duration = Duration::from_secs(5);
/// How much life a sign-in must have left to be sent. One that runs out
/// while the ask is on its way is refused by the service; twice the longest
/// an ask may take leaves room for the ask and for a clock a little off
const SPARE: i64 = 2 * ASK.as_secs() as i64;

/// The reading, kept up on its own thread.
pub struct Meter {
    shared: Arc<Mutex<Shared>>,
}

struct Shared {
    /// Whether anybody could be looking: a tab of this AI exists and the
    /// setting is on. Nothing is read otherwise -- a machine that never runs
    /// an AI never talks to that AI's service
    want: bool,
    last: Option<(Limits, Instant)>,
    asked: Option<Instant>,
}

impl Meter {
    /// Starts the thread. It reads nothing until `want(true)`.
    pub fn start(source: Source) -> Self {
        let shared = Arc::new(Mutex::new(Shared { want: false, last: None, asked: None }));
        let mine = Arc::clone(&shared);
        std::thread::Builder::new()
            .name(format!("{}-limits", source.key()))
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let due = {
                    let Ok(s) = mine.lock() else { break };
                    s.want && s.asked.is_none_or(|t| t.elapsed() >= source.every())
                };
                if !due {
                    continue;
                }
                // A remembered reading stands on the status line only while
                // it is still roughly true (the settings screen shows it
                // longer, with its time on it)
                let got = source.read().filter(|l| match (source.stale_after(), l.as_of) {
                    (Some(max), Some(at)) => now_ms() / 1000 - at <= max,
                    _ => true,
                });
                let Ok(mut s) = mine.lock() else { break };
                s.asked = Some(Instant::now());
                match got {
                    Some(l) => s.last = Some((l, Instant::now())),
                    // Kept a while, then let go: see KEEP
                    None => {
                        if s.last.as_ref().is_some_and(|(_, at)| at.elapsed() > KEEP) {
                            s.last = None;
                        }
                    }
                }
            })
            .ok();
        Meter { shared }
    }

    /// Whether anybody could be looking. Turning it off also forgets the
    /// last reading, so turning it back on starts clean
    pub fn want(&self, on: bool) {
        if let Ok(mut s) = self.shared.lock()
            && s.want != on
        {
            s.want = on;
            s.asked = None;
            if !on {
                s.last = None;
            }
        }
    }

    /// The last reading, while it is still worth showing.
    pub fn current(&self) -> Option<Limits> {
        let s = self.shared.lock().ok()?;
        s.last.as_ref().filter(|(_, at)| at.elapsed() <= KEEP).map(|(l, _)| l.clone())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The home folder, as every CLI here finds its own under it
fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()).map(std::path::PathBuf::from)
}

/// A folder a CLI lets its own variable move, or where it keeps it otherwise
fn home_or(var: &str, under_home: &[&str]) -> Option<std::path::PathBuf> {
    match std::env::var_os(var) {
        Some(h) if !h.is_empty() => Some(h.into()),
        _ => Some(under_home.iter().fold(home()?, |p, part| p.join(part))),
    }
}

/// A JSON file another program keeps, read as it is (a byte-order mark put
/// there by an editor is not part of it)
fn read_json(path: &std::path::Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// A number the service may write as a number or as a string of one
fn number(v: Option<&serde_json::Value>) -> Option<f64> {
    match v? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// A reset time as a service writes it: an ISO date, or a count of seconds
/// or milliseconds since the epoch (told apart by size: seconds since the
/// epoch stay under ten digits until the year 2286)
fn instant_of(v: Option<&serde_json::Value>) -> Option<i64> {
    let secs_or_ms = |n: f64| if n >= 1e10 { (n / 1000.0) as i64 } else { n as i64 };
    match v? {
        serde_json::Value::String(s) => epoch_of(s).or_else(|| s.trim().parse::<f64>().ok().map(secs_or_ms)),
        serde_json::Value::Number(n) => n.as_f64().map(secs_or_ms),
        _ => None,
    }
}

/// What a sign-in token says about itself: the middle of a JWT is JSON,
/// read for who the sign-in is for and until when. Never checked for a
/// signature -- the service does that; this only decides whether to ask
fn token_claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let middle = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(middle.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// One HTTP client for one ask: the app's name, a bounded wait, and no
/// following a service that sends the ask elsewhere -- a sign-in page
/// answered in place of a reading is not a reading
fn client() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(ASK))
        .max_redirects(0)
        .user_agent(concat!("shikisha-term/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// A reading this new is handed out again instead of asking a second time:
/// the status line and the settings screen asking within the same minute
/// get one answer between them
const FRESH: i64 = 55;
/// A reading older than this is handed out with its time on it
const DATED: i64 = 120;
/// How long to leave a service alone after it did not answer: this long
/// the first time, twice as long each time after, up to [`QUIET_MOST`].
/// Claude's service answers a sign-in that asks too often with 429 and goes
/// on answering it so for minutes (measured 2026-09-23: every ask in two
/// minutes, 20 seconds apart, was turned away); asking every minute through
/// that only keeps it going. The other services are given the same patience
const QUIET_FIRST: i64 = 60;
const QUIET_MOST: i64 = 15 * 60;

/// Which account a sign-in is for, as a mark that says "same" or "not the
/// same" and nothing else: a hash of the account's id (or of the credential
/// itself where the file names no account). Never the id or the secret -- the
/// mark is kept on disk beside Claude's last reading
pub(super) fn account_mark(identity: &str) -> String {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(identity.trim().as_bytes());
    // Half of the hash: far more than enough to tell a person's few accounts
    // apart, and short enough to read in the file
    h.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// The account a JWT sign-in is for, from its own claims: the subject, or an
/// e-mail or user id where a service puts it instead. `None` for a token that
/// says nothing of the kind (an opaque one), which leaves "which account" as
/// unknown rather than guessing it from a string that changes every renewal
fn subject_in_token(token: &str) -> Option<String> {
    let c = token_claims(token)?;
    ["sub", "email", "user_id"]
        .iter()
        .find_map(|k| c.get(*k).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
}

/// What has been learnt of one service: the last good reading and when it
/// was taken, which account it was taken for, and how long to leave the
/// service alone.
///
/// One per service for the whole program, so the status line and the
/// settings screen share it
#[derive(Debug, Clone, PartialEq)]
struct Book {
    last: Option<(Limits, i64)>,
    /// The account the last reading is of (`account_mark`), when known
    who: Option<String>,
    quiet_until: i64,
    quiet: i64,
    loaded: bool,
}

impl Book {
    const EMPTY: Book = Book { last: None, who: None, quiet_until: 0, quiet: 0, loaded: false };

    /// The CLI is signed in as `who` now. A reading of another account is
    /// no reading of this one: it goes, and the service is asked afresh
    /// rather than left alone for the old account's refusals. An account that
    /// cannot be told (an opaque token, the file unreadable) changes nothing:
    /// a renewal must not look like somebody else signing in
    fn signed_in_as(&mut self, who: Option<&str>) {
        let Some(who) = who else { return };
        if self.who.as_deref().is_some_and(|was| was != who) {
            self.last = None;
            self.quiet = 0;
            self.quiet_until = 0;
        }
        self.who = Some(who.to_string());
    }

    /// Whether to ask the service now, rather than hand out what is known
    fn should_ask(&self, now: i64) -> bool {
        let fresh = self.last.as_ref().is_some_and(|(_, at)| now - at < FRESH);
        !fresh && now >= self.quiet_until
    }

    /// Writes down how an ask went
    fn record(&mut self, got: Option<Limits>, now: i64) {
        match got {
            Some(l) => {
                self.last = Some((l, now));
                self.quiet = 0;
                self.quiet_until = 0;
            }
            None => {
                self.quiet = if self.quiet == 0 { QUIET_FIRST } else { (self.quiet * 2).min(QUIET_MOST) };
                self.quiet_until = now + self.quiet;
            }
        }
    }

    /// The last good reading as it stands at `now`, dated when it is not
    /// from just now
    fn answer(&self, now: i64) -> Option<Limits> {
        self.last.as_ref().map(|(l, at)| Limits { as_of: (now - at >= DATED).then_some(*at), taken: Some(*at), ..l.clone() })
    }
}

static BOOKS: Mutex<Vec<(Source, Book)>> = Mutex::new(Vec::new());

/// A service's reading through its [`Book`]: `fetch` is called only when
/// the book says to ask, and a panic in it is one more "no answer". `account`
/// says which account the CLI is signed in as now, so a reading kept from
/// another account is never handed out as this one's
fn asked(source: Source, now: i64, fetch: fn() -> Option<Limits>, account: fn() -> Option<String>) -> Option<Limits> {
    let who = std::panic::catch_unwind(account).ok().flatten().map(|id| account_mark(&id));
    let with = |f: &mut dyn FnMut(&mut Book)| {
        let Ok(mut books) = BOOKS.lock() else { return };
        if !books.iter().any(|(s, _)| *s == source) {
            books.push((source, Book::EMPTY));
        }
        if let Some((_, b)) = books.iter_mut().find(|(s, _)| *s == source) {
            f(b);
        }
    };
    let mut ask = false;
    with(&mut |b| {
        if !b.loaded {
            b.loaded = true;
            // Claude's last reading is kept between starts, so a start while
            // its service is turning asks away still has something to show
            if source == Source::Claude
                && let Some((last, kept_for)) = claude::load()
            {
                b.last = Some(last);
                b.who = kept_for;
            }
        }
        b.signed_in_as(who.as_deref());
        ask = b.should_ask(now);
    });
    if ask {
        let got = std::panic::catch_unwind(fetch).unwrap_or(None);
        if source == Source::Claude
            && let Some(l) = &got
        {
            claude::save(l, now, who.as_deref());
        }
        with(&mut |b| b.record(got.clone(), now));
    }
    let mut out = None;
    with(&mut |b| out = b.answer(now));
    out
}

/// `2026-09-08T16:19:59.874255+00:00` as seconds since the epoch.
///
/// Only the shape the services write: date, time, optional fraction, and
/// `Z` or a `+HH:MM` offset. Anything else is "no reset time", which the
/// pill shows as the percentage alone
fn epoch_of(s: &str) -> Option<i64> {
    epoch_ms_of(s).map(|ms| ms.div_euclid(1000))
}

/// `2026-09-08T16:19:59.874255+00:00` as milliseconds since the epoch: the
/// shape the CLIs write their own times in (`timestamp` in a record). Date,
/// time, an optional fraction, and `Z` or a `+HH:MM` offset; anything else
/// is `None`
pub fn epoch_ms_of(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    // The offset starts at the last + or - after the time, or at Z
    let (time, offset) = match rest.rfind(['+', '-', 'Z']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "Z"),
    };
    let (time, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut t = time.split(':').map(|p| p.parse::<i64>());
    let (hh, mm, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next().unwrap_or(Ok(0)).ok()?);
    // The first three digits of the fraction are the milliseconds
    let digits: String = fraction.chars().take_while(char::is_ascii_digit).take(3).collect();
    let ms = match digits.len() {
        0 => 0,
        n => digits.parse::<i64>().ok()? * 10_i64.pow(3 - n as u32),
    };
    let shift = match offset {
        "Z" | "" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let mut p = o[1..].split(':').map(|x| x.parse::<i64>());
            let (oh, om) = (p.next()?.ok()?, p.next().unwrap_or(Ok(0)).ok()?);
            sign * (oh * 3600 + om * 60)
        }
    };
    Some((days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss - shift) * 1000 + ms)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reset_time_is_read_with_its_offset() {
        assert_eq!(epoch_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_of("1970-01-02T00:00:00+00:00"), Some(86_400));
        assert_eq!(epoch_of("1970-01-01T09:00:00+09:00"), Some(0), "the offset is not subtracted");
        assert_eq!(epoch_of("2026-09-08T16:19:59.874255+00:00"), Some(1_788_884_399));
        assert_eq!(epoch_of("2026-09-12T04:59:59.874275+00:00"), Some(1_789_189_199));
        assert_eq!(epoch_ms_of("2026-09-28T02:12:30.055Z"), Some(1_790_561_550_055));
        assert_eq!(epoch_ms_of("1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(epoch_of("soon"), None);
        assert_eq!(epoch_of(""), None);
    }

    /// A reset time is read whichever way a service writes it.
    #[test]
    fn a_reset_is_read_as_a_date_seconds_or_milliseconds() {
        let j = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(instant_of(Some(&j(r#""2026-09-08T16:19:59Z""#))), Some(1_788_884_399));
        assert_eq!(instant_of(Some(&j("1788884399"))), Some(1_788_884_399));
        assert_eq!(instant_of(Some(&j("1788884399000"))), Some(1_788_884_399), "milliseconds read as seconds");
        assert_eq!(instant_of(Some(&j(r#""1788884399000""#))), Some(1_788_884_399));
        assert_eq!(instant_of(Some(&j("null"))), None);
        assert_eq!(instant_of(None), None);
        assert_eq!(number(Some(&j(r#""12.5""#))), Some(12.5));
        assert_eq!(number(Some(&j(r#""lots""#))), None);
    }

    /// Nothing is asked until somebody could be looking, and a reading
    /// outlives a silence only for a while.
    #[test]
    fn the_meter_is_quiet_until_wanted_and_forgets_when_told_to() {
        let m = Meter { shared: Arc::new(Mutex::new(Shared { want: false, last: None, asked: None })) };
        assert_eq!(m.current(), None);
        let l = Limits::of(vec![Allowance::whole(Span::Hours5, Window { pct: 5, resets_at: None })], None).unwrap();
        m.shared.lock().unwrap().last = Some((l.clone(), Instant::now()));
        assert_eq!(m.current(), Some(l.clone()));
        // Stale readings are not shown
        m.shared.lock().unwrap().last = Some((l.clone(), Instant::now() - KEEP - Duration::from_secs(1)));
        assert_eq!(m.current(), None, "it showed an old value");
        // Wanting it off forgets what was read
        m.shared.lock().unwrap().last = Some((l, Instant::now()));
        m.want(true);
        m.want(false);
        assert_eq!(m.current(), None, "it remembers though it was cut");
        assert!(!m.shared.lock().unwrap().want);
    }

    /// Asked once a minute at most; left alone longer and longer while it
    /// does not answer; and what was last read is handed out meanwhile,
    /// with its time on it once it is not from just now.
    #[test]
    fn a_service_is_asked_sparingly_and_the_last_reading_stands_in() {
        let mut b = Book { loaded: true, ..Book::EMPTY };
        assert!(b.should_ask(1_000), "nothing known, and nothing asked");
        let l = Limits::of(vec![Allowance::whole(Span::Hours5, Window { pct: 6, resets_at: Some(9_000) })], None).unwrap();
        b.record(Some(l.clone()), 1_000);
        assert!(!b.should_ask(1_030), "asked again within the minute");
        let fresh = b.answer(1_030).unwrap();
        assert_eq!((fresh.as_of, fresh.taken), (None, Some(1_000)), "a reading from just now was dated, or lost its time");
        assert_eq!(fresh.wins, l.wins);
        assert!(b.should_ask(1_060));
        // Turned away: left alone 1, 2, 4 ... minutes, never more than 15
        b.record(None, 1_060);
        assert!(!b.should_ask(1_100) && b.should_ask(1_120), "not left alone for a minute");
        b.record(None, 1_120);
        assert!(!b.should_ask(1_230) && b.should_ask(1_240), "the wait did not grow");
        for _ in 0..10 {
            b.record(None, 2_000);
        }
        assert_eq!(b.quiet, QUIET_MOST, "the wait grew past its cap");
        // Meanwhile the last reading stands in, dated
        let seen = b.answer(1_240).unwrap();
        assert_eq!(seen.as_of, Some(1_000), "an old reading was handed out as new");
        assert_eq!(seen.whole(Span::Hours5).map(|w| w.pct), Some(6));
        // Past its reset, the number read is kept and said to have reset --
        // not replaced by a number nobody read
        let past = b.answer(9_000).unwrap();
        assert!(past.whole(Span::Hours5).unwrap().reset_by(9_000));
        assert_eq!(past.whole(Span::Hours5).map(|w| w.pct), Some(6));
        // An answer puts everything back
        b.record(Some(l), 3_000);
        assert_eq!((b.quiet, b.quiet_until), (0, 0));
    }

    /// A reading is of one account. Signed in as another, the old numbers go
    /// -- even while the service is turning asks away, when before they stood
    /// in as though they were the new account's -- and the service is asked
    /// at once. An account that cannot be told changes nothing, and the
    /// account itself is never kept, only its mark
    #[test]
    fn a_reading_of_another_account_is_not_handed_out() {
        let l = Limits::of(vec![Allowance::whole(Span::Hours5, Window { pct: 71, resets_at: None })], None).unwrap();
        let (alice, bob) = (account_mark("alice-uuid"), account_mark("bob-uuid"));
        assert_ne!(alice, bob);
        assert!(!alice.contains("alice"), "the mark carries the id itself");
        let mut b = Book { loaded: true, ..Book::EMPTY };
        b.signed_in_as(Some(&alice));
        b.record(Some(l), 1_000);
        b.record(None, 1_060); // turned away since
        b.signed_in_as(None); // a renewal in progress: unknown, not "someone else"
        assert!(b.answer(1_100).is_some(), "an unknown account dropped the reading");
        b.signed_in_as(Some(&alice));
        assert!(b.answer(1_100).is_some(), "the same account dropped the reading");
        b.signed_in_as(Some(&bob));
        assert!(b.answer(1_100).is_none(), "alice's numbers were handed out for bob");
        assert!(b.should_ask(1_100), "the new account waited out the old one's refusals");
        // A JWT's subject names the account; an opaque token names none
        let jwt = |claims: &str| {
            use base64::Engine as _;
            format!("h.{}.s", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims))
        };
        assert_eq!(subject_in_token(&jwt(r#"{"sub":"u-1"}"#)).as_deref(), Some("u-1"));
        assert_eq!(subject_in_token("opaque-token"), None);
    }

    /// The whole subscription first, shortest window first; a model's own
    /// allowance after; and a reading with nothing in it is no reading.
    #[test]
    fn allowances_are_put_in_reading_order() {
        let w = |pct| Window { pct, resets_at: None };
        let l = Limits::of(
            vec![
                Allowance { span: Some(Span::Days7), only: Some("Fable".into()), window: w(1) },
                Allowance::whole(Span::Month, w(2)),
                Allowance::whole(Span::Days7, w(3)),
                Allowance::whole(Span::Hours5, w(4)),
            ],
            None,
        )
        .unwrap();
        let order: Vec<u32> = l.wins.iter().map(|a| a.window.pct).collect();
        assert_eq!(order, vec![4, 3, 2, 1]);
        assert_eq!(Limits::of(Vec::new(), None), None);
        assert_eq!(Span::of_key("month"), Some(Span::Month));
        assert_eq!(Span::of_minutes(60), None, "an hour was named as a window it is not");
    }
}
