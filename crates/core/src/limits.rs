//! What Claude's subscription has left, asked of Claude's own service.
//!
//! A window running several agents is a window where "can I start one more"
//! is the question, and the answer is a number Claude Code already knows:
//! how much of the 5-hour and the 7-day allowance is used, and when each
//! resets. It is asked the way Claude Code asks it -- with the sign-in Claude
//! Code keeps on this PC, at `/api/oauth/usage` -- and shown as one pill while
//! a Claude tab is in view.
//!
//! **Nothing here may stop the program.** The path is not documented, so it
//! will change one day without notice. Every step answers `None` instead of
//! failing: no sign-in file, a sign-in that has expired, a service that does
//! not answer, a body that is not the shape it was -- each of those is "no
//! pill", never an error on screen, and never a stalled frame: the asking is
//! done on a thread of its own, behind a mutex the drawing only glances at.

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

/// Both windows, and when they were read.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

/// How often the service is asked while somebody could be looking.
const EVERY: Duration = Duration::from_secs(60);
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
    /// Whether anybody could be looking: a Claude tab exists and the setting
    /// is on. Nothing is asked otherwise -- a machine that never runs Claude
    /// never talks to Claude's service
    want: bool,
    last: Option<(Limits, Instant)>,
    asked: Option<Instant>,
}

impl Meter {
    /// Starts the thread. It asks nothing until `want(true)`.
    pub fn start() -> Self {
        let shared = Arc::new(Mutex::new(Shared { want: false, last: None, asked: None }));
        let mine = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("claude-limits".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let due = {
                    let Ok(s) = mine.lock() else { break };
                    s.want && s.asked.is_none_or(|t| t.elapsed() >= EVERY)
                };
                if !due {
                    continue;
                }
                // A panic anywhere in the asking must not take the thread,
                // let alone the program. It is treated as one more "no answer"
                let got = std::panic::catch_unwind(|| token().and_then(|t| fetch(&t))).unwrap_or(None);
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
        if let Ok(mut s) = self.shared.lock() {
            if s.want != on {
                s.want = on;
                s.asked = None;
                if !on {
                    s.last = None;
                }
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

/// Whether Claude Code has a sign-in on this PC that is still good.
///
/// Whether, never what: the settings screen says why the pill is or is not
/// there, and nothing hands the token back out
pub fn signed_in() -> bool {
    token().is_some()
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
    (five_hour.is_some() || seven_day.is_some()).then_some(Limits { five_hour, seven_day })
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
        assert_eq!(parse(&serde_json::json!({"rate_limits": null})), None, "形が変わったら黙って無し");
        assert_eq!(parse(&serde_json::json!("nonsense")), None);
    }

    #[test]
    fn the_reset_time_is_read_with_its_offset() {
        assert_eq!(epoch_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_of("1970-01-02T00:00:00+00:00"), Some(86_400));
        assert_eq!(epoch_of("1970-01-01T09:00:00+09:00"), Some(0), "オフセットが引かれていない");
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
        assert_eq!(token_in(text, 2000), None, "切れたサインインを使った");
        assert_eq!(token_in(r#"{"claudeAiOauth":{"accessToken":""}}"#, 0), None);
        assert_eq!(token_in(r#"{"apiKey":"x"}"#, 0), None, "OAuth でないものを使った");
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
        let l = Limits { five_hour: Some(Window { pct: 5, resets_at: None }), seven_day: None };
        m.shared.lock().unwrap().last = Some((l.clone(), Instant::now()));
        assert_eq!(m.current(), Some(l.clone()));
        // Stale readings are not shown
        m.shared.lock().unwrap().last = Some((l.clone(), Instant::now() - KEEP - Duration::from_secs(1)));
        assert_eq!(m.current(), None, "古い値を出した");
        // Wanting it off forgets what was read
        m.shared.lock().unwrap().last = Some((l, Instant::now()));
        m.want(true);
        m.want(false);
        assert_eq!(m.current(), None, "切ったのに覚えている");
        assert!(!m.shared.lock().unwrap().want);
    }
}
