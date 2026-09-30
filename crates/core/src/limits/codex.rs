//! Codex's allowance: asked of the service the Codex CLI signs in to, with
//! the sign-in Codex keeps on this PC, and -- when that cannot be had --
//! read off the session records Codex writes on this PC, which carry a
//! reading in every turn and are as new as the last turn Codex took here.

use super::{Allowance, Limits, Span, Window};

/// The sign-in Codex keeps on this PC: the token, and the account it picks
/// when the sign-in has more than one. Read, never written -- Codex renews it
fn sign_in(now: i64) -> Option<(String, Option<String>)> {
    let v = super::read_json(&codex_home()?.join("auth.json"))?;
    sign_in_of(&v, now)
}

fn sign_in_of(v: &serde_json::Value, now: i64) -> Option<(String, Option<String>)> {
    let t = v.get("tokens")?;
    let token = t.get("access_token")?.as_str()?.trim();
    if token.is_empty() {
        return None;
    }
    let expires = super::token_claims(token).and_then(|c| c.get("exp").and_then(|e| e.as_i64()));
    if expires.is_some_and(|e| e - now <= super::SPARE) {
        return None;
    }
    Some((token.to_string(), t.get("account_id").and_then(|a| a.as_str()).map(str::to_string)))
}

/// The account Codex is signed in as: the id its sign-in names, or the
/// token's own subject. Read whether or not the token has run out -- the
/// account does not change when a token does
pub(super) fn account() -> Option<String> {
    let t = super::read_json(&codex_home()?.join("auth.json"))?.get("tokens")?.clone();
    t.get("account_id")
        .and_then(|a| a.as_str())
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .or_else(|| super::subject_in_token(t.get("access_token")?.as_str()?))
}

/// One ask of the service, the question the CLI's own status asks
pub(super) fn ask() -> Option<Limits> {
    let (token, account) = sign_in(super::now_ms() / 1000)?;
    let mut req = super::client()
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json");
    if let Some(a) = &account {
        req = req.header("ChatGPT-Account-Id", a);
    }
    let mut resp = req.call().ok()?;
    parse_live(&resp.body_mut().read_json().ok()?)
}

/// The service's answer: the subscription's windows under `rate_limit`, and
/// allowances of their own for some models under `additional_rate_limits`,
/// each named. A window is placed by its length in seconds
fn parse_live(v: &serde_json::Value) -> Option<Limits> {
    // The account's plan is what says this is a reading at all
    v.get("plan_type")?.as_str()?;
    let windows = |rl: &serde_json::Value, only: Option<&str>, wins: &mut Vec<Allowance>| {
        for key in ["primary_window", "secondary_window"] {
            let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
            let Some(pct) = super::number(w.get("used_percent")) else { continue };
            let span = super::number(w.get("limit_window_seconds")).and_then(|s| Span::of_minutes((s / 60.0).round() as i64));
            let Some(span) = span else { continue };
            let window = Window::of_percent(pct, super::instant_of(w.get("reset_at")));
            wins.push(Allowance { span: Some(span), only: only.map(str::to_string), window });
        }
    };
    let mut wins = Vec::new();
    if let Some(rl) = v.get("rate_limit") {
        windows(rl, None, &mut wins);
    }
    for extra in v.get("additional_rate_limits").and_then(|a| a.as_array()).into_iter().flatten() {
        let (Some(name), Some(rl)) = (extra.get("limit_name").and_then(|n| n.as_str()), extra.get("rate_limit")) else { continue };
        windows(rl, Some(name), &mut wins);
    }
    Limits::of(wins, None)
}

/// The newer of the service's reading and the records' -- the service's
/// when it answered just now, the records' when they were written after the
/// service last answered
pub(super) fn newest(live: Option<Limits>, record: Option<Limits>) -> Option<Limits> {
    match (live, record) {
        (Some(l), Some(r)) => {
            let at = |x: &Limits| x.taken.unwrap_or(i64::MAX);
            Some(if at(&r) > at(&l) { r } else { l })
        }
        (l, r) => l.or(r),
    }
}

/// Where Codex keeps its things: `CODEX_HOME` when it is set, as Codex
/// itself reads it, and `.codex` in the home folder otherwise.
fn codex_home() -> Option<std::path::PathBuf> {
    super::home_or("CODEX_HOME", &[".codex"])
}

pub(super) fn used_here() -> bool {
    codex_home().is_some_and(|h| h.join("sessions").is_dir() || h.join("auth.json").is_file())
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
pub(super) fn reading(now: i64) -> Option<Limits> {
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
        .find_map(|l| parse_line(l, now))
}

/// One record line as a reading, at `now` (seconds since the epoch).
///
/// The line is a turn's `token_count` event, as Codex wrote it on
/// 2026-09-18: `payload.rate_limits` holds a `primary` and a `secondary`
/// window, each with `used_percent`, `window_minutes` and `resets_at`
/// (seconds since the epoch). Earlier builds wrote `resets_in_seconds`,
/// counted from the line's own `timestamp`, and that is read too.
///
/// A window is placed by its length, not by being first or second (see
/// [`Span::of_minutes`]). A window whose reset has passed since the line was
/// written keeps the number it had then, and says so ([`Window::reset_by`])
fn parse_line(line: &str, _now: i64) -> Option<Limits> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let rl = v.pointer("/payload/rate_limits")?;
    // A reading for some other allowance than Codex's own
    if rl.get("limit_id").and_then(|i| i.as_str()).is_some_and(|i| i != "codex") {
        return None;
    }
    let written = v.get("timestamp").and_then(|t| t.as_str()).and_then(super::epoch_of);
    let mut wins = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("used_percent").and_then(|p| p.as_f64()) else { continue };
        let resets_at = w.get("resets_at").and_then(|r| r.as_i64()).or_else(|| {
            Some(written? + w.get("resets_in_seconds")?.as_i64()?)
        });
        let window = Window::of_percent(pct, resets_at);
        if let Some(span) = w.get("window_minutes").and_then(|m| m.as_i64()).and_then(Span::of_minutes) {
            wins.push(Allowance::whole(span, window));
        }
    }
    Limits::of(wins, written).map(|l| Limits { from_record: true, ..l })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let l = parse_line(&line, WRITTEN).unwrap();
        assert_eq!(l.whole(Span::Hours5), Some(&Window { pct: 15, resets_at: Some(1_789_751_640) }));
        assert_eq!(l.whole(Span::Days7), Some(&Window { pct: 18, resets_at: Some(1_789_805_685) }));
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
        let l = parse_line(&line, WRITTEN).unwrap();
        assert_eq!(l.whole(Span::Hours5), None, "a 1-hour window was shown as the 5-hour one");
        assert_eq!(l.whole(Span::Days7).map(|w| w.pct), Some(40), "the 7-day window was read by its position");
        // Past the 5-hour reset but not the 7-day one
        let line = codex_line(
            "codex",
            r#"{"used_percent":90.0,"window_minutes":300,"resets_at":1789751640}"#,
            r#"{"used_percent":18.0,"window_minutes":10080,"resets_at":1789805685}"#,
        );
        let l = parse_line(&line, 1_789_751_640).unwrap();
        let five = l.whole(Span::Hours5).unwrap();
        assert!(five.reset_by(1_789_751_640), "a window whose reset has passed was not said to have reset");
        assert_eq!(five.pct, 90, "a number nobody read was made up for the new window");
        assert!(!l.whole(Span::Days7).unwrap().reset_by(1_789_751_640));
    }

    /// Earlier builds wrote the time left rather than the time of the reset.
    #[test]
    fn an_older_codex_record_is_read_by_the_time_it_was_written() {
        let line = codex_line("codex", r#"{"used_percent":3.0,"window_minutes":300,"resets_in_seconds":600}"#, "null");
        let l = parse_line(&line, WRITTEN).unwrap();
        assert_eq!(l.whole(Span::Hours5), Some(&Window { pct: 3, resets_at: Some(WRITTEN + 600) }));
        assert_eq!(l.whole(Span::Days7), None);
    }

    /// The service's answer, shaped as it answered on 2026-09-30 (a plan
    /// with a 7-day window only, and one model with an allowance of its own).
    #[test]
    fn the_live_reading_is_read_by_window_length() {
        let v = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {"allowed": true,
                "primary_window": {"used_percent": 8, "limit_window_seconds": 604_800, "reset_at": 1_791_087_165},
                "secondary_window": null},
            "additional_rate_limits": [{"limit_name": "reserve-model", "rate_limit": {
                "primary_window": {"used_percent": 0, "limit_window_seconds": 604_800, "reset_at": 1_791_298_871}}}]
        });
        let l = parse_live(&v).unwrap();
        assert_eq!(l.whole(Span::Days7), Some(&Window { pct: 8, resets_at: Some(1_791_087_165) }));
        assert_eq!(l.whole(Span::Hours5), None);
        assert_eq!(l.wins.last().and_then(|a| a.only.as_deref()), Some("reserve-model"));
        assert_eq!(parse_live(&serde_json::json!({"rate_limit": {}})), None, "a body with no plan was taken for a reading");
        // Five hours and a week, whichever order they come in
        let both = serde_json::json!({"plan_type": "pro", "rate_limit": {
            "primary_window": {"used_percent": 40, "limit_window_seconds": 604_800},
            "secondary_window": {"used_percent": 12.4, "limit_window_seconds": 18_000}}});
        let l = parse_live(&both).unwrap();
        assert_eq!((l.whole(Span::Hours5).map(|w| w.pct), l.whole(Span::Days7).map(|w| w.pct)), (Some(12), Some(40)));
    }

    #[test]
    fn the_sign_in_is_used_only_while_it_is_good() {
        use base64::Engine as _;
        let token = |exp: i64| format!("h.{}.s", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#)));
        let v = serde_json::json!({"tokens": {"access_token": token(1_000), "account_id": "acct"}});
        assert_eq!(sign_in_of(&v, 500).map(|(_, a)| a), Some(Some("acct".into())));
        assert_eq!(sign_in_of(&v, 1_000), None, "a sign-in that has run out was sent");
        assert_eq!(sign_in_of(&serde_json::json!({"OPENAI_API_KEY": "sk"}), 0), None, "an API key was taken for a sign-in");
    }

    /// The service's reading wins unless the records were written after it.
    #[test]
    fn the_newer_reading_is_the_one_shown() {
        let at = |taken: Option<i64>, pct| Limits {
            wins: vec![Allowance::whole(Span::Days7, Window { pct, resets_at: None })],
            as_of: taken,
            taken,
            from_record: false,
        };
        assert_eq!(newest(Some(at(None, 1)), Some(at(Some(50), 2))).unwrap().wins[0].window.pct, 1, "a reading from just now lost to an old record");
        assert_eq!(newest(Some(at(Some(10), 1)), Some(at(Some(50), 2))).unwrap().wins[0].window.pct, 2, "a record written after the service last answered lost");
        assert_eq!(newest(None, Some(at(Some(50), 2))).unwrap().wins[0].window.pct, 2);
        assert_eq!(newest(None, None), None);
    }

    #[test]
    fn what_is_not_codex_own_reading_is_nothing() {
        let w = r#"{"used_percent":3.0,"window_minutes":300,"resets_at":1789751640}"#;
        assert_eq!(parse_line(&codex_line("other_model", w, "null"), WRITTEN), None, "another allowance was shown as Codex's");
        assert_eq!(parse_line(r#"{"payload":{"rate_limits":null}}"#, WRITTEN), None);
        assert_eq!(parse_line("not json", WRITTEN), None);
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
            codex_line("codex", &format!(r#"{{"used_percent":{pct}.0,"window_minutes":300,"resets_at":1789751640}}"#), "null")
        };
        // Written in this order: the resumed session in yesterday's folder
        // is the newest reading, and today's session has not had a turn
        let resumed = old_day.join("rollout-a.jsonl");
        std::fs::write(new_day.join("rollout-b.jsonl"), format!("{}\n", reading(1))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&resumed, format!("{}\n{}\n", reading(5), reading(7))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(new_day.join("rollout-c.jsonl"), "{\"type\":\"session_meta\"}\n").unwrap();
        let files = newest_records(&root);
        assert_eq!(files.first().and_then(|f| f.file_name()), Some(std::ffi::OsStr::new("rollout-c.jsonl")));
        let l = files.iter().find_map(|f| last_reading_in(f, WRITTEN)).unwrap();
        assert_eq!(l.whole(Span::Hours5).map(|w| w.pct), Some(7), "not the last reading of the newest record");
        let _ = std::fs::remove_dir_all(&root);
    }
}
