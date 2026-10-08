//! Claude's allowance, asked of Claude's service the way Claude Code asks it:
//! with the sign-in Claude Code keeps on this PC, at `/api/oauth/usage`.

use super::{Allowance, Limits, Span, Window};

/// The sign-in Claude Code keeps on this PC, while it is still good.
///
/// Read, never written: renewing it is Claude Code's business, and a token
/// past its time is simply "no pill" until Claude Code has renewed it
fn token() -> Option<String> {
    let path = super::home()?.join(".claude").join(".credentials.json");
    if let Ok(text) = std::fs::read_to_string(path) {
        return token_in(&text, super::now_ms());
    }
    #[cfg(target_os = "macos")]
    if let Some(text) = keychain::credentials() {
        return token_in(&text, super::now_ms());
    }
    None
}

/// A Mac's Claude Code keeps its sign-in in the login keychain, not in a file:
/// the same text, under the item it names "Claude Code-credentials".
///
/// The keychain is the Mac's to guard: reading another program's item is asked
/// of the person by the system, once, and their answer is theirs to give. So
/// what came back is kept a minute rather than asked again for every pill, and
/// a refusal -- or no item at all -- is not asked about again for ten minutes:
/// a question the person already answered, put to them over and over, is a
/// question that has stopped being one
#[cfg(target_os = "macos")]
mod keychain {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// What the keychain last said, and when
    static LAST: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

    const KEPT: Duration = Duration::from_secs(60);
    const NOT_ASKED_AGAIN: Duration = Duration::from_secs(600);

    pub(super) fn credentials() -> Option<String> {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, said)) = last.as_ref() {
            let wait = if said.is_some() { KEPT } else { NOT_ASKED_AGAIN };
            if at.elapsed() < wait {
                return said.clone();
            }
        }
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        *last = Some((Instant::now(), out.clone()));
        out
    }
}

pub(super) fn signed_in() -> bool {
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

/// One ask. The endpoint and the header are the ones Claude Code itself uses
/// (read off its binary, 2026-09-06 build), not a guess
pub(super) fn ask() -> Option<Limits> {
    let token = token()?;
    let mut resp = super::client()
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Content-Type", "application/json")
        .call()
        .ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    parse(&v)
}

/// Every window out of the body, however much else is in it.
///
/// The whole subscription's are `five_hour` and `seven_day`. An allowance
/// that counts one model only comes two ways, and both are read: an entry
/// in `limits` scoped to a model by its display name, and a field named
/// `seven_day_<model>` (`seven_day_opus` was in the body on 2026-09-08, null).
/// One model said both ways is shown once
pub fn parse(v: &serde_json::Value) -> Option<Limits> {
    let window = |w: &serde_json::Value| -> Option<Window> {
        if w.is_null() {
            return None;
        }
        let pct = super::number(w.get("utilization").or_else(|| w.get("percent")))?;
        Some(Window::of_percent(pct, super::instant_of(w.get("resets_at"))))
    };
    let mut wins = Vec::new();
    for (key, span) in [("five_hour", Span::Hours5), ("seven_day", Span::Days7)] {
        if let Some(w) = v.get(key).and_then(window) {
            wins.push(Allowance::whole(span, w));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let mut scoped = |name: &str, w: Window, wins: &mut Vec<Allowance>| {
        let name = name.trim();
        if name.is_empty() || seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            return;
        }
        seen.push(name.to_string());
        wins.push(Allowance { span: Some(Span::Days7), only: Some(name.to_string()), window: w });
    };
    for l in v.get("limits").and_then(|l| l.as_array()).into_iter().flatten() {
        if l.get("kind").and_then(|k| k.as_str()) != Some("weekly_scoped") {
            continue;
        }
        let Some(name) = l.pointer("/scope/model/display_name").and_then(|n| n.as_str()) else { continue };
        if let Some(w) = window(l) {
            scoped(&capitalised(name), w, &mut wins);
        }
    }
    if let Some(o) = v.as_object() {
        for (key, w) in o {
            // A model's name is one word; `seven_day_oauth_apps` counts
            // something else and is not a model
            let Some(model) = key.strip_prefix("seven_day_").filter(|m| !m.is_empty() && m.chars().all(|c| c.is_ascii_alphabetic()))
            else {
                continue;
            };
            if let Some(w) = window(w) {
                scoped(&capitalised(model), w, &mut wins);
            }
        }
    }
    Limits::of(wins, None)
}

/// A model's name as a person reads it: `fable` as `Fable`
fn capitalised(s: &str) -> String {
    let mut c = s.trim().chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Where the last good reading is kept between starts
fn file() -> std::path::PathBuf {
    crate::config::state_path("claude-usage.json")
}

/// The account Claude Code is signed in as: its id, from Claude Code's own
/// settings file (written at every sign-in). `None` when that file does not
/// say, which leaves "which account" as unknown
pub(super) fn account() -> Option<String> {
    let v = super::read_json(&super::home()?.join(".claude.json"))?;
    let id = v.get("oauthAccount")?.get("accountUuid")?.as_str()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// Keep the reading, with the mark of the account it is of (`who`)
pub(super) fn save(l: &Limits, at: i64, who: Option<&str>) {
    let mut v = book_json(l, at);
    if let Some(who) = who {
        v["who"] = serde_json::Value::String(who.to_string());
    }
    let _ = crate::crypto::write_atomic(&file(), &v.to_string());
}

/// The kept reading and the mark of the account it is of. A file written
/// before readings carried the mark says no account; it is shown until the
/// next reading replaces it, as it was before
pub(super) fn load() -> Option<((Limits, i64), Option<String>)> {
    let v = super::read_json(&file())?;
    let who = v.get("who").and_then(|w| w.as_str()).map(str::to_string);
    Some((book_of(&v)?, who))
}

fn book_json(l: &Limits, at: i64) -> serde_json::Value {
    let wins: Vec<serde_json::Value> = l
        .wins
        .iter()
        .map(|a| {
            serde_json::json!({
                "span": a.span.map(|s| s.key()),
                "model": a.only,
                "pct": a.window.pct,
                "resets_at": a.window.resets_at,
            })
        })
        .collect();
    serde_json::json!({"at": at, "wins": wins})
}

/// What was kept, in either shape: the list written now, or the two fixed
/// windows written before a model's own allowance was read
fn book_of(v: &serde_json::Value) -> Option<(Limits, i64)> {
    let window = |w: &serde_json::Value| -> Option<Window> {
        Some(Window {
            pct: w.get("pct")?.as_u64()?.min(100) as u32,
            resets_at: w.get("resets_at").and_then(|r| r.as_i64()),
        })
    };
    let mut wins = Vec::new();
    if let Some(list) = v.get("wins").and_then(|w| w.as_array()) {
        for w in list {
            let Some(window) = window(w) else { continue };
            let span = w.get("span").and_then(|s| s.as_str()).and_then(Span::of_key);
            let only = w.get("model").and_then(|m| m.as_str()).map(str::to_string);
            wins.push(Allowance { span, only, window });
        }
    } else {
        for (key, span) in [("five", Span::Hours5), ("week", Span::Days7)] {
            if let Some(w) = v.get(key).filter(|w| w.is_object()).and_then(window) {
                wins.push(Allowance::whole(span, w));
            }
        }
    }
    let at = v.get("at")?.as_i64()?;
    Limits::of(wins, None).map(|l| (l, at))
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
        assert_eq!(l.whole(Span::Hours5), Some(&Window { pct: 19, resets_at: Some(1_788_884_399) }));
        assert_eq!(l.whole(Span::Days7).map(|w| w.pct), Some(23));
        assert_eq!(l.wins.len(), 2, "a model's allowance the service withheld was shown");
        // A window the service withholds is simply absent; a body with
        // neither is no reading at all
        let v: serde_json::Value = serde_json::from_str(r#"{"five_hour":null,"seven_day":{"utilization":1}}"#).unwrap();
        let l = parse(&v).unwrap();
        assert_eq!(l.whole(Span::Hours5), None);
        assert_eq!(l.whole(Span::Days7), Some(&Window { pct: 1, resets_at: None }));
        assert_eq!(parse(&serde_json::json!({"rate_limits": null})), None, "if the shape changes, quietly nothing");
        assert_eq!(parse(&serde_json::json!("nonsense")), None);
    }

    /// A model's own weekly allowance, said in a list entry or in a field
    /// of its own, is read once under the model's name.
    #[test]
    fn a_model_s_own_allowance_is_read_once_by_its_name() {
        let v = serde_json::json!({
            "seven_day": {"utilization": 30.0},
            "limits": [
                {"kind": "weekly_scoped", "percent": 81.6, "resets_at": 1_789_189_199,
                 "scope": {"model": {"display_name": "fable"}}},
                {"kind": "something_else", "percent": 5.0, "scope": {"model": {"display_name": "other"}}}
            ],
            "seven_day_fable": {"utilization": 12.0},
            "seven_day_oauth_apps": {"utilization": 3.0}
        });
        let l = parse(&v).unwrap();
        let scoped: Vec<_> = l.wins.iter().filter(|a| a.only.is_some()).collect();
        assert_eq!(scoped.len(), 1, "one model said twice was shown twice");
        assert_eq!(scoped[0].only.as_deref(), Some("Fable"));
        assert_eq!(scoped[0].window, Window { pct: 82, resets_at: Some(1_789_189_199) });
        assert_eq!(l.wins[0].only, None, "the model's allowance came before the whole subscription's");
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

    /// What is kept between starts reads back as it was written, and what an
    /// older build kept still reads.
    #[test]
    fn the_kept_reading_reads_back_in_either_shape() {
        let l = Limits::of(
            vec![
                Allowance::whole(Span::Hours5, Window { pct: 6, resets_at: Some(1_789_000_000) }),
                Allowance::whole(Span::Days7, Window { pct: 35, resets_at: None }),
                Allowance { span: Some(Span::Days7), only: Some("Fable".into()), window: Window { pct: 9, resets_at: None } },
            ],
            None,
        )
        .unwrap();
        assert_eq!(book_of(&book_json(&l, 42)), Some((l, 42)));
        let old = serde_json::json!({"at": 7, "five": {"pct": 6, "resets_at": 100}, "week": null});
        let (read, at) = book_of(&old).unwrap();
        assert_eq!((read.whole(Span::Hours5).map(|w| w.pct), at), (Some(6), 7));
        assert_eq!(book_of(&serde_json::json!({"at": 1, "five": null, "week": null})), None);
        assert_eq!(book_of(&serde_json::json!("nonsense")), None);
    }
}
