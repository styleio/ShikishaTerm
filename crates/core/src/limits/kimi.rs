//! Kimi Code's allowance, asked of its service with the sign-in the Kimi CLI
//! keeps on this PC -- the same question the CLI's own usage command asks.
//!
//! The CLI renews that sign-in every quarter of an hour or so while it runs,
//! and renewing it hands out a new renewal key; a second program renewing it
//! would sign the running CLI out. So a sign-in past its time is left alone,
//! and the reading waits for the CLI to be run again.

use super::{Allowance, Limits, Span, Window};

/// The sign-in file, where the CLI keeps it (its own variable moves it)
fn sign_in_file() -> Option<std::path::PathBuf> {
    Some(super::home_or("KIMI_CODE_HOME", &[".kimi-code"])?.join("credentials").join("kimi-code.json"))
}

pub(super) fn signed_in() -> bool {
    sign_in_file().is_some_and(|f| f.is_file())
}

/// The access token, while it has life left in it
fn token_in(v: &serde_json::Value, now: i64) -> Option<String> {
    let expires = super::number(v.get("expires_at"))? as i64;
    if expires - now <= super::SPARE {
        return None;
    }
    let t = v.get("access_token")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

pub(super) fn ask() -> Option<Limits> {
    let token = token_in(&super::read_json(&sign_in_file()?)?, super::now_ms() / 1000)?;
    let base = std::env::var("KIMI_CODE_BASE_URL").ok().filter(|b| !b.trim().is_empty());
    let base = base.as_deref().unwrap_or("https://api.kimi.com/coding/v1").trim_end_matches('/');
    let mut resp = super::client()
        .get(&format!("{base}/usages"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .ok()?;
    parse(&resp.body_mut().read_json().ok()?)
}

/// One counted allowance: how much of a limit is gone, by `used` or by
/// what `remaining` leaves
fn counted(d: &serde_json::Value) -> Option<Window> {
    let limit = super::number(d.get("limit")).filter(|l| *l > 0.0)?;
    let used = super::number(d.get("used")).or_else(|| Some(limit - super::number(d.get("remaining"))?))?;
    let reset = super::instant_of(d.get("resetTime").or_else(|| d.get("resetAt")));
    Some(Window::of_percent(used / limit * 100.0, reset))
}

/// A window's length as the service writes it: a count and a unit
fn minutes(w: &serde_json::Value) -> Option<i64> {
    let n = super::number(w.get("duration"))?;
    let unit = w.get("timeUnit").and_then(|u| u.as_str()).unwrap_or_default().to_ascii_uppercase();
    let per = if unit.contains("SECOND") {
        1.0 / 60.0
    } else if unit.contains("MINUTE") {
        1.0
    } else if unit.contains("HOUR") {
        60.0
    } else if unit.contains("DAY") {
        1440.0
    } else {
        return None;
    };
    Some((n * per).round() as i64)
}

/// The body: the week as a whole under `usage`, and the shorter windows in
/// `limits`, each with its length. A window of a length with no name here is
/// left out, the way Codex's are
fn parse(v: &serde_json::Value) -> Option<Limits> {
    let mut wins = Vec::new();
    if let Some(w) = v.get("usage").and_then(counted) {
        wins.push(Allowance::whole(Span::Days7, w));
    }
    for l in v.get("limits").and_then(|l| l.as_array()).into_iter().flatten() {
        let span = l.get("window").and_then(minutes).and_then(Span::of_minutes);
        if let (Some(span), Some(w)) = (span, l.get("detail").and_then(counted))
            && !wins.iter().any(|a: &Allowance| a.span == Some(span))
        {
            wins.push(Allowance::whole(span, w));
        }
    }
    Limits::of(wins, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_is_sent_only_with_life_left_in_it() {
        let v = serde_json::json!({"access_token": "k-1", "expires_at": 1_000});
        assert_eq!(token_in(&v, 900).as_deref(), Some("k-1"));
        assert_eq!(token_in(&v, 1_000 - super::super::SPARE), None, "a sign-in about to run out was sent");
        assert_eq!(token_in(&serde_json::json!({"access_token": "k"}), 0), None, "a sign-in with no time on it was trusted");
        assert_eq!(token_in(&serde_json::json!({"access_token": " ", "expires_at": 9e9}), 0), None);
    }

    #[test]
    fn the_week_and_the_five_hours_are_read_by_their_lengths() {
        let v = serde_json::json!({
            "usage": {"limit": "200", "remaining": "150", "resetTime": "2026-09-12T04:59:59Z"},
            "limits": [
                {"window": {"duration": 60, "timeUnit": "TIME_UNIT_MINUTE"}, "detail": {"limit": 10, "used": 9}},
                {"window": {"duration": 5, "timeUnit": "TIME_UNIT_HOUR"}, "detail": {"limit": 40, "used": 10, "resetAt": 1_789_000_000}}
            ]
        });
        let l = parse(&v).unwrap();
        assert_eq!(l.whole(Span::Days7), Some(&Window { pct: 25, resets_at: Some(1_789_189_199) }));
        assert_eq!(l.whole(Span::Hours5), Some(&Window { pct: 25, resets_at: Some(1_789_000_000) }));
        assert_eq!(l.wins.len(), 2, "an hour-long window was shown under another's name");
        assert_eq!(parse(&serde_json::json!({"usage": {"limit": 0, "used": 1}})), None, "a limit of nothing made a percentage");
        assert_eq!(parse(&serde_json::json!({})), None);
    }
}
