//! What an AI subscription has left: Claude's and Codex's.
//!
//! A window running several agents is a window where "can I start one more"
//! is the question, and the answer is a number the CLI already knows: how
//! much of the 5-hour and the 7-day allowance is used, and when each resets.
//! It is shown as one pill while a tab of that AI is in view.
//!
//! The two are read in different places, each where its own CLI keeps it:
//!
//! - **Claude** is asked of Claude's service the way Claude Code asks it --
//!   with the sign-in Claude Code keeps on this PC, at `/api/oauth/usage`.
//! - **Codex** is read off the session records Codex writes on this PC. It
//!   puts the reading into every turn's record, so nothing is sent anywhere,
//!   and the reading is as new as the last turn Codex took here.
//!
//! **Nothing here may stop the program.** Neither place is documented, so
//! either will change one day without notice. Every step answers `None`
//! instead of failing: no sign-in file, a sign-in that has expired, a service
//! that does not answer, a record that is not the shape it was -- each of
//! those is "no pill", never an error on screen, and never a stalled frame:
//! the reading is done on a thread of its own, behind a mutex the drawing
//! only glances at.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One allowance window, as the service reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// Whole percent used, 0-100
    pub pct: u32,
    /// When it resets, as seconds since the epoch, when the service says
    pub resets_at: Option<i64>,
}

impl Window {
    /// The window as it stands at `now`. One whose reset has passed since
    /// it was read has started again, so it is unused and its next reset is
    /// unknown -- unless the AI ran somewhere this program cannot see, and
    /// then there is no way to know
    fn at(self, now: i64) -> Window {
        match self.resets_at {
            Some(at) if at <= now => Window { pct: 0, resets_at: None },
            _ => self,
        }
    }
}

/// Both windows, and when they were read.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    /// When the reading was taken, as seconds since the epoch, when that is
    /// not "just now". A Codex reading is as old as the last turn Codex took
    /// on this PC, and a reading that could be hours old says so
    pub as_of: Option<i64>,
}

/// Whose allowance: the AI a tab runs, by the name the tabs know it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Claude, Source::Codex];

    /// The tab's AI kind this allowance belongs to
    pub fn key(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }

    /// The AI's name, as a person reads it beside the reading
    pub fn name(self) -> &'static str {
        match self {
            Source::Claude => "Claude",
            Source::Codex => "Codex",
        }
    }

    /// How often to read while somebody could be looking. Claude's is a
    /// request to a service; Codex's is a file on this disk, so it can be
    /// looked at often enough to follow the turns as they finish
    fn every(self) -> Duration {
        match self {
            Source::Claude => Duration::from_secs(60),
            Source::Codex => Duration::from_secs(15),
        }
    }

    /// The reading as it stands. Blocks for as long as the asking takes (up
    /// to [`ASK`] for Claude), so it is called off the drawing thread.
    ///
    /// Claude's goes through [`CLAUDE`]: a reading taken within the last
    /// minute is handed out again rather than asked for twice, a service that
    /// has stopped answering is asked less and less often, and while it is
    /// not answering the last good reading is handed out with its time on it
    pub fn read(self) -> Option<Limits> {
        let now = now_ms() / 1000;
        match self {
            Source::Claude => claude_read(now),
            // A panic anywhere in the reading is one more "no answer"
            Source::Codex => std::panic::catch_unwind(|| codex_reading(now)).unwrap_or(None),
        }
    }

    /// How old a reading may be and still stand on the status line. Claude's
    /// numbers move while nobody can see them; Codex's are as new as the last
    /// turn here, and a window whose reset has passed is already shown as such
    fn stale_after(self) -> Option<i64> {
        match self {
            Source::Claude => Some(KEEP.as_secs() as i64),
            Source::Codex => None,
        }
    }

    /// Whether this PC has what the reading needs: Claude Code's sign-in, or
    /// Codex having been used here at all.
    ///
    /// Whether, never what: the settings screen says why the pill is or is
    /// not there, and nothing hands a token back out
    pub fn ready(self) -> bool {
        match self {
            Source::Claude => token().is_some(),
            Source::Codex => codex_home().is_some_and(|h| h.join("sessions").is_dir()),
        }
    }
}

/// How long the last good answer is shown once the service stops answering.
/// "Resets in 3h" is still roughly true ten minutes on; after that it is a
/// number pretending to be current
const KEEP: Duration = Duration::from_secs(10 * 60);
/// How long one ask may take. The thread is the app's, not the service's
const ASK: Duration = Duration::from_secs(5);

/// The reading, kept up on its own thread.
pub struct Meter {
    shared: Arc<Mutex<Shared>>,
}

struct Shared {
    /// Whether anybody could be looking: a tab of this AI exists and the
    /// setting is on. Nothing is read otherwise -- a machine that never runs
    /// Claude never talks to Claude's service
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
            && s.want != on {
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
        s.last
            .as_ref()
            .filter(|(_, at)| at.elapsed() <= KEEP)
            .map(|(l, _)| l.clone())
    }
}

/// The sign-in Claude Code keeps on this PC, while it is still good.
///
/// Read, never written: renewing it is Claude Code's business, and a token
/// past its time is simply "no pill" until Claude Code has renewed it
fn token() -> Option<String> {
    let home = std::env::var_os("USERPROFILE")?;
    let path = std::path::PathBuf::from(home).join(".claude").join(".credentials.json");
    let text = std::fs::read_to_string(path).ok()?;
    token_in(&text, now_ms())
}

fn token_in(text: &str, now_ms: i64) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let o = v.get("claudeAiOauth")?;
    let expires = o.get("expiresAt").and_then(|e| e.as_i64()).unwrap_or(i64::MAX);
    if expires <= now_ms {
        return None;
    }
    let t = o.get("accessToken")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A reading this new is handed out again instead of asking a second time:
/// the status line and the settings screen asking within the same minute
/// get one answer between them
const FRESH: i64 = 55;
/// A reading older than this is handed out with its time on it
const DATED: i64 = 120;
/// How long to leave the service alone after it did not answer: this long
/// the first time, twice as long each time after, up to [`QUIET_MOST`].
/// The service answers a sign-in that asks too often with 429 and goes on
/// answering it so for minutes (measured 2026-09-23: every ask in two
/// minutes, 20 seconds apart, was turned away); asking every minute through
/// that only keeps it going
const QUIET_FIRST: i64 = 60;
const QUIET_MOST: i64 = 15 * 60;

/// What has been learnt of Claude's service: the last good reading and when
/// it was taken, and how long to leave the service alone.
///
/// One for the whole program, so the status line and the settings screen
/// share it, and kept on disk, so a start while the service is turning
/// asks away still has something to show, with its time on it
#[derive(Debug, Clone, PartialEq)]
struct Book {
    last: Option<(Limits, i64)>,
    quiet_until: i64,
    quiet: i64,
    loaded: bool,
}

static CLAUDE: Mutex<Book> = Mutex::new(Book { last: None, quiet_until: 0, quiet: 0, loaded: false });

impl Book {
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
        self.last.as_ref().map(|(l, at)| Limits {
            five_hour: l.five_hour.clone().map(|w| w.at(now)),
            seven_day: l.seven_day.clone().map(|w| w.at(now)),
            as_of: (now - at >= DATED).then_some(*at),
        })
    }
}

fn claude_read(now: i64) -> Option<Limits> {
    let ask = {
        let Ok(mut b) = CLAUDE.lock() else { return None };
        if !b.loaded {
            b.loaded = true;
            b.last = load_claude();
        }
        b.should_ask(now)
    };
    if ask {
        // A panic anywhere in the asking is one more "no answer"
        let got = std::panic::catch_unwind(|| token().and_then(|t| fetch(&t))).unwrap_or(None);
        let Ok(mut b) = CLAUDE.lock() else { return None };
        b.record(got.clone(), now);
        if let Some(l) = &got {
            save_claude(l, now);
        }
    }
    CLAUDE.lock().ok()?.answer(now)
}

/// Where the last good reading of Claude's is kept between starts
fn claude_file() -> std::path::PathBuf {
    crate::config::state_path("claude-usage.json")
}

fn save_claude(l: &Limits, at: i64) {
    let _ = crate::crypto::write_atomic(&claude_file(), &book_json(l, at).to_string());
}

fn load_claude() -> Option<(Limits, i64)> {
    let text = std::fs::read_to_string(claude_file()).ok()?;
    book_of(&serde_json::from_str(&text).ok()?)
}

fn book_json(l: &Limits, at: i64) -> serde_json::Value {
    let w = |w: &Option<Window>| w.as_ref().map(|w| serde_json::json!({"pct": w.pct, "resets_at": w.resets_at}));
    serde_json::json!({"at": at, "five": w(&l.five_hour), "week": w(&l.seven_day)})
}

fn book_of(v: &serde_json::Value) -> Option<(Limits, i64)> {
    let w = |k: &str| -> Option<Window> {
        let w = v.get(k).filter(|w| w.is_object())?;
        Some(Window {
            pct: w.get("pct")?.as_u64()?.min(100) as u32,
            resets_at: w.get("resets_at").and_then(|r| r.as_i64()),
        })
    };
    let (five_hour, seven_day) = (w("five"), w("week"));
    let at = v.get("at")?.as_i64()?;
    (five_hour.is_some() || seven_day.is_some()).then_some((Limits { five_hour, seven_day, as_of: None }, at))
}

/// One ask. The endpoint and the header are the ones Claude Code itself uses
/// (read off its binary, 2026-09-06 build), not a guess
fn fetch(token: &str) -> Option<Limits> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(ASK))
        .build()
        .new_agent();
    let mut resp = agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Content-Type", "application/json")
        .header("User-Agent", concat!("shikisha-term/", env!("CARGO_PKG_VERSION")))
        .call()
        .ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    parse(&v)
}

/// The two windows out of the body, however much else is in it.
pub fn parse(v: &serde_json::Value) -> Option<Limits> {
    let window = |name: &str| -> Option<Window> {
        let w = v.get(name)?;
        if w.is_null() {
            return None;
        }
        let pct = w.get("utilization")?.as_f64()?;
        Some(Window {
            pct: pct.round().clamp(0.0, 100.0) as u32,
            resets_at: w.get("resets_at").and_then(|r| match r {
                serde_json::Value::String(s) => epoch_of(s),
                serde_json::Value::Number(n) => n.as_i64(),
                _ => None,
            }),
        })
    };
    let five_hour = window("five_hour");
    let seven_day = window("seven_day");
    (five_hour.is_some() || seven_day.is_some()).then_some(Limits { five_hour, seven_day, as_of: None })
}

/// Where Codex keeps its things: `CODEX_HOME` when it is set, as Codex
/// itself reads it, and `.codex` in the home folder otherwise.
fn codex_home() -> Option<std::path::PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(h) if !h.is_empty() => Some(h.into()),
        _ => Some(std::path::PathBuf::from(std::env::var_os("USERPROFILE")?).join(".codex")),
    }
}

/// How many of the newest session records are looked into for a reading.
/// A session opened and left without a turn has none, so the newest alone
/// is not enough; a handful is, and keeps the reading to a few file reads
const RECORDS: usize = 6;
/// How many day folders are looked through for the newest records. A
/// session resumed after days goes on writing into the folder of the day
/// it began, so "today's folder" is not where the newest record has to be
const DAYS: usize = 31;
/// How much of a record's end is read. The reading is in every turn's
/// record, so it is near the end; a record grows to megabytes, and reading
/// the whole of it every few seconds would be paying for its history
const TAIL: u64 = 1 << 20;

/// The newest reading Codex wrote on this PC, as it stands at `now`.
fn codex_reading(now: i64) -> Option<Limits> {
    let sessions = codex_home()?.join("sessions");
    newest_records(&sessions).iter().find_map(|f| last_reading_in(f, now))
}

/// The session records Codex wrote most lately, newest first.
///
/// They sit in `sessions/<year>/<month>/<day>/`. The folders are walked from
/// the newest date back, and the files are ordered by when they were last
/// written, not by their names -- a resumed session is an old name
fn newest_records(sessions: &std::path::Path) -> Vec<std::path::PathBuf> {
    // A folder's subfolders with number names, the largest number first
    let numbered = |dir: &std::path::Path| -> Vec<std::path::PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut v: Vec<(u32, std::path::PathBuf)> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| Some((e.file_name().to_str()?.parse::<u32>().ok()?, e.path())))
            .collect();
        v.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        v.into_iter().map(|(_, p)| p).collect()
    };
    let days = numbered(sessions)
        .iter()
        .flat_map(|y| numbered(y))
        .flat_map(|m| numbered(&m))
        .take(DAYS)
        .collect::<Vec<_>>();
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = days
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    files.into_iter().take(RECORDS).map(|(_, p)| p).collect()
}

/// The last reading in a record, read from its end.
fn last_reading_in(path: &std::path::Path, now: i64) -> Option<Limits> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    // A cut through the middle of a character or a line is only ever at the
    // front, and a line that does not read -- that one, or a reading of some
    // other allowance -- gives way to the one before it
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .rev()
        .filter(|l| l.contains("\"rate_limits\":{"))
        .find_map(|l| parse_codex(l, now))
}

/// One record line as a reading, at `now` (seconds since the epoch).
///
/// The line is a turn's `token_count` event, as Codex wrote it on
/// 2026-09-18: `payload.rate_limits` holds a `primary` and a `secondary`
/// window, each with `used_percent`, `window_minutes` and `resets_at`
/// (seconds since the epoch). Earlier builds wrote `resets_in_seconds`,
/// counted from the line's own `timestamp`, and that is read too.
///
/// A window is placed by its length, not by being first or second: 300
/// minutes is the 5-hour window and 10080 the 7-day one. One of another
/// length is left out rather than shown under a name that is not its own.
/// A window whose reset has already passed has started again since the
/// line was written, so it is shown as unused -- which it is, unless Codex
/// ran on another PC, and then this PC has no way to know
fn parse_codex(line: &str, now: i64) -> Option<Limits> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let rl = v.pointer("/payload/rate_limits")?;
    // A reading for some other allowance than Codex's own
    if rl.get("limit_id").and_then(|i| i.as_str()).is_some_and(|i| i != "codex") {
        return None;
    }
    let written = v.get("timestamp").and_then(|t| t.as_str()).and_then(epoch_of);
    let mut five_hour = None;
    let mut seven_day = None;
    for key in ["primary", "secondary"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("used_percent").and_then(|p| p.as_f64()) else { continue };
        let resets_at = w.get("resets_at").and_then(|r| r.as_i64()).or_else(|| {
            Some(written? + w.get("resets_in_seconds")?.as_i64()?)
        });
        let window = Window { pct: pct.round().clamp(0.0, 100.0) as u32, resets_at }.at(now);
        match w.get("window_minutes").and_then(|m| m.as_u64()) {
            Some(300) => five_hour = Some(window),
            Some(10_080) => seven_day = Some(window),
            _ => {}
        }
    }
    (five_hour.is_some() || seven_day.is_some()).then_some(Limits { five_hour, seven_day, as_of: written })
}

/// `2026-09-08T16:19:59.874255+00:00` as seconds since the epoch.
///
/// Only the shape the service writes: date, time, optional fraction, and
/// `Z` or a `+HH:MM` offset. Anything else is "no reset time", which the
/// pill shows as the percentage alone
fn epoch_of(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    // The offset starts at the last + or - after the time, or at Z
    let (time, offset) = match rest.rfind(['+', '-', 'Z']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "Z"),
    };
    let time = time.split('.').next()?;
    let mut t = time.split(':').map(|p| p.parse::<i64>());
    let (hh, mm, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next().unwrap_or(Ok(0)).ok()?);
    let shift = match offset {
        "Z" | "" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let mut p = o[1..].split(':').map(|x| x.parse::<i64>());
            let (oh, om) = (p.next()?.ok()?, p.next().unwrap_or(Ok(0)).ok()?);
            sign * (oh * 3600 + om * 60)
        }
    };
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss - shift)
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

    /// The body as the service wrote it on 2026-09-08, with the fields this
    /// app does not read left in, since they are in the real thing.
    #[test]
    fn the_two_windows_are_read_out_of_the_body() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":19.0,"resets_at":"2026-09-08T16:19:59.874255+00:00","limit_dollars":null},
                "seven_day":{"utilization":23.4,"resets_at":"2026-09-12T04:59:59.874275+00:00"},
                "seven_day_opus":null,"extra_usage":{"is_enabled":false}}"#,
        )
        .unwrap();
        let l = parse(&v).unwrap();
        assert_eq!(l.five_hour, Some(Window { pct: 19, resets_at: Some(1_788_884_399) }));
        assert_eq!(l.seven_day.as_ref().map(|w| w.pct), Some(23));
        // A window the service withholds is simply absent; a body with
        // neither is no reading at all
        let v: serde_json::Value = serde_json::from_str(r#"{"five_hour":null,"seven_day":{"utilization":1}}"#).unwrap();
        let l = parse(&v).unwrap();
        assert_eq!(l.five_hour, None);
        assert_eq!(l.seven_day, Some(Window { pct: 1, resets_at: None }));
        assert_eq!(parse(&serde_json::json!({"rate_limits": null})), None, "if the shape changes, quietly nothing");
        assert_eq!(parse(&serde_json::json!("nonsense")), None);
    }

    #[test]
    fn the_reset_time_is_read_with_its_offset() {
        assert_eq!(epoch_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_of("1970-01-02T00:00:00+00:00"), Some(86_400));
        assert_eq!(epoch_of("1970-01-01T09:00:00+09:00"), Some(0), "the offset is not subtracted");
        assert_eq!(epoch_of("2026-09-08T16:19:59.874255+00:00"), Some(1_788_884_399));
        assert_eq!(epoch_of("2026-09-12T04:59:59.874275+00:00"), Some(1_789_189_199));
        assert_eq!(epoch_of("soon"), None);
        assert_eq!(epoch_of(""), None);
    }

    /// The sign-in is read, never trusted past its time, and never written.
    #[test]
    fn the_sign_in_is_used_only_while_it_is_good() {
        let text = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-abc","refreshToken":"r","expiresAt":2000,"scopes":["user:inference"]}}"#;
        assert_eq!(token_in(text, 1000).as_deref(), Some("sk-ant-oat01-abc"));
        assert_eq!(token_in(text, 2000), None, "an expired sign-in was used");
        assert_eq!(token_in(r#"{"claudeAiOauth":{"accessToken":""}}"#, 0), None);
        assert_eq!(token_in(r#"{"apiKey":"x"}"#, 0), None, "something that is not OAuth was used");
        assert_eq!(token_in("not json", 0), None);
        // A file with no expiry is taken as good -- the service will say otherwise
        assert_eq!(token_in(r#"{"claudeAiOauth":{"accessToken":"t"}}"#, 0).as_deref(), Some("t"));
    }

    /// Nothing is asked until somebody could be looking, and a reading
    /// outlives a silence only for a while.
    #[test]
    fn the_meter_is_quiet_until_wanted_and_forgets_when_told_to() {
        let m = Meter { shared: Arc::new(Mutex::new(Shared { want: false, last: None, asked: None })) };
        assert_eq!(m.current(), None);
        let l = Limits { five_hour: Some(Window { pct: 5, resets_at: None }), seven_day: None, as_of: None };
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
    fn claude_is_asked_sparingly_and_the_last_reading_stands_in() {
        let mut b = Book { last: None, quiet_until: 0, quiet: 0, loaded: true };
        assert!(b.should_ask(1_000), "nothing known, and nothing asked");
        let l = Limits { five_hour: Some(Window { pct: 6, resets_at: Some(9_000) }), seven_day: None, as_of: None };
        b.record(Some(l.clone()), 1_000);
        assert!(!b.should_ask(1_030), "asked again within the minute");
        assert_eq!(b.answer(1_030), Some(l.clone()), "a reading from just now was dated");
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
        assert_eq!(seen.five_hour.as_ref().map(|w| w.pct), Some(6));
        // Past its reset, the window has started again
        assert_eq!(b.answer(9_000).unwrap().five_hour, Some(Window { pct: 0, resets_at: None }));
        // An answer puts everything back
        b.record(Some(l), 3_000);
        assert_eq!((b.quiet, b.quiet_until), (0, 0));
    }

    /// What is kept between starts reads back as it was written.
    #[test]
    fn the_kept_claude_reading_reads_back() {
        let l = Limits {
            five_hour: Some(Window { pct: 6, resets_at: Some(1_789_000_000) }),
            seven_day: Some(Window { pct: 35, resets_at: None }),
            as_of: None,
        };
        assert_eq!(book_of(&book_json(&l, 42)), Some((l, 42)));
        assert_eq!(book_of(&serde_json::json!({"at": 1, "five": null, "week": null})), None);
        assert_eq!(book_of(&serde_json::json!("nonsense")), None);
    }

    /// A turn's record line as Codex wrote it on 2026-09-18, the token
    /// counts cut down to what the reading does not need.
    fn codex_line(limit_id: &str, primary: &str, secondary: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-09-18T13:11:43.155Z","type":"event_msg","payload":{{"type":"token_count","info":{{"model_context_window":258400}},"rate_limits":{{"limit_id":"{limit_id}","limit_name":null,"primary":{primary},"secondary":{secondary},"credits":{{"has_credits":false,"unlimited":false,"balance":"0"}},"plan_type":"plus"}}}}}}"#
        )
    }
    /// 2026-09-18T13:11:43Z, the line's own timestamp
    const WRITTEN: i64 = 1_789_737_103;

    #[test]
    fn the_codex_windows_are_read_out_of_a_turn_record() {
        let line = codex_line(
            "codex",
            r#"{"used_percent":15.0,"window_minutes":300,"resets_at":1789751640}"#,
            r#"{"used_percent":18.4,"window_minutes":10080,"resets_at":1789805685}"#,
        );
        let l = parse_codex(&line, WRITTEN).unwrap();
        assert_eq!(l.five_hour, Some(Window { pct: 15, resets_at: Some(1_789_751_640) }));
        assert_eq!(l.seven_day, Some(Window { pct: 18, resets_at: Some(1_789_805_685) }));
        assert_eq!(l.as_of, Some(WRITTEN), "the reading does not say how old it is");
    }

    /// Placed by length, not by order; a length with no name of its own is
    /// left out; a reset already past is a window started again.
    #[test]
    fn a_codex_window_is_named_by_its_length_and_reset_by_the_clock() {
        let line = codex_line(
            "codex",
            r#"{"used_percent":40.0,"window_minutes":10080,"resets_at":1789805685}"#,
            r#"{"used_percent":9.0,"window_minutes":60,"resets_at":1789751640}"#,
        );
        let l = parse_codex(&line, WRITTEN).unwrap();
        assert_eq!(l.five_hour, None, "a 1-hour window was shown as the 5-hour one");
        assert_eq!(l.seven_day.map(|w| w.pct), Some(40), "the 7-day window was read by its position");
        // Past the 5-hour reset but not the 7-day one
        let line = codex_line(
            "codex",
            r#"{"used_percent":90.0,"window_minutes":300,"resets_at":1789751640}"#,
            r#"{"used_percent":18.0,"window_minutes":10080,"resets_at":1789805685}"#,
        );
        let l = parse_codex(&line, 1_789_751_640).unwrap();
        assert_eq!(l.five_hour, Some(Window { pct: 0, resets_at: None }), "a window already reset still shows as used");
        assert_eq!(l.seven_day.map(|w| w.pct), Some(18));
    }

    /// Earlier builds wrote the time left rather than the time of the reset.
    #[test]
    fn an_older_codex_record_is_read_by_the_time_it_was_written() {
        let line = codex_line(
            "codex",
            r#"{"used_percent":3.0,"window_minutes":300,"resets_in_seconds":600}"#,
            "null",
        );
        let l = parse_codex(&line, WRITTEN).unwrap();
        assert_eq!(l.five_hour, Some(Window { pct: 3, resets_at: Some(WRITTEN + 600) }));
        assert_eq!(l.seven_day, None);
    }

    #[test]
    fn what_is_not_codex_own_reading_is_nothing() {
        let w = r#"{"used_percent":3.0,"window_minutes":300,"resets_at":1789751640}"#;
        assert_eq!(parse_codex(&codex_line("other_model", w, "null"), WRITTEN), None, "another allowance was shown as Codex's");
        assert_eq!(parse_codex(r#"{"payload":{"rate_limits":null}}"#, WRITTEN), None);
        assert_eq!(parse_codex("not json", WRITTEN), None);
    }

    /// The newest reading is found across the day folders, by when each
    /// record was written, past a newer record that has no reading yet.
    #[test]
    fn the_newest_codex_reading_is_found_on_disk() {
        let root = std::env::temp_dir().join(format!("shikisha-codex-{}", crate::random_hex(6)));
        let old_day = root.join("2026").join("09").join("17");
        let new_day = root.join("2026").join("09").join("18");
        std::fs::create_dir_all(&old_day).unwrap();
        std::fs::create_dir_all(&new_day).unwrap();
        let reading = |pct: u32| {
            codex_line(
                "codex",
                &format!(r#"{{"used_percent":{pct}.0,"window_minutes":300,"resets_at":1789751640}}"#),
                "null",
            )
        };
        // Written in this order: the resumed session in yesterday's folder
        // is the newest reading, and today's session has not had a turn
        let resumed = old_day.join("rollout-a.jsonl");
        std::fs::write(new_day.join("rollout-b.jsonl"), format!("{}\n", reading(1))).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&resumed, format!("{}\n{}\n", reading(5), reading(7))).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(new_day.join("rollout-c.jsonl"), "{\"type\":\"session_meta\"}\n").unwrap();
        let files = newest_records(&root);
        assert_eq!(files.first().and_then(|f| f.file_name()), Some(std::ffi::OsStr::new("rollout-c.jsonl")));
        let l = files.iter().find_map(|f| last_reading_in(f, WRITTEN)).unwrap();
        assert_eq!(l.five_hour.map(|w| w.pct), Some(7), "not the last reading of the newest record");
        let _ = std::fs::remove_dir_all(&root);
    }
}
