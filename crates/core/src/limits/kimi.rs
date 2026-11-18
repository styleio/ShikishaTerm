//! Kimi Code's allowance, asked of its service with the sign-in the Kimi CLI
//! keeps on this PC -- the same question the CLI's own usage command asks.
//!
//! The CLI renews that sign-in every quarter of an hour or so while it runs,
//! and renewing it hands out a new renewal key; a second program renewing it
//! would sign the running CLI out. So a sign-in past its time is left alone,
//! and the reading waits for the CLI to be run again.

use super::{Allowance, Limits, Span, Window};

/// The service Kimi Code signs in to and asks of, as it decides it
const OAUTH_HOST: &str = "https://auth.kimi.com";
const BASE_URL: &str = "https://api.kimi.com/coding/v1";

/// Kimi Code's own folder (its own variable moves it)
fn kimi_home() -> Option<std::path::PathBuf> {
    super::home_or("KIMI_CODE_HOME", &[".kimi-code"])
}

/// Where Kimi Code signs in and asks, as it works it out: its variables,
/// else its settings (`[providers."managed:kimi-code"]`), else its own
/// service. Either variable set puts the settings aside for both
fn service(config: &str, var: &dyn Fn(&str) -> Option<String>) -> (String, String) {
    let trim = |s: String| s.trim().trim_end_matches('/').to_string();
    let env_base = var("KIMI_CODE_BASE_URL");
    let env_host = var("KIMI_CODE_OAUTH_HOST").or_else(|| var("KIMI_OAUTH_HOST"));
    let provider = ["providers", "managed:kimi-code"];
    let (base, host) = if env_base.is_some() || env_host.is_some() {
        (env_base, env_host)
    } else {
        (
            crate::accounts::toml_text(config, &[&provider[..], &["base_url"]].concat()),
            crate::accounts::toml_text(config, &[&provider[..], &["oauth", "oauth_host"]].concat()),
        )
    };
    (
        host.map(trim).filter(|h| !h.is_empty()).unwrap_or_else(|| OAUTH_HOST.to_string()),
        base.map(trim).filter(|b| !b.is_empty()).unwrap_or_else(|| BASE_URL.to_string()),
    )
}

/// The name of the sign-in file for that service: Kimi Code 2 keeps one per
/// service it is pointed at, named after a hash of where that is, and the
/// plain name for its own
fn token_name(host: &str, base: &str) -> String {
    use sha2::Digest as _;
    if host == OAUTH_HOST && base == BASE_URL {
        return "kimi-code".to_string();
    }
    // As the CLI writes it out (`{"oauthHost":..,"baseUrl":..}`, in that order)
    let scope = format!(
        "{{\"oauthHost\":{},\"baseUrl\":{}}}",
        serde_json::Value::String(host.to_string()),
        serde_json::Value::String(base.to_string())
    );
    let hash = sha2::Sha256::digest(scope.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("kimi-code-env-{}", &hex[..16])
}

/// The sign-in file and the address to ask, for the service Kimi Code is
/// pointed at
fn where_to_ask() -> Option<(std::path::PathBuf, String)> {
    let home = kimi_home()?;
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
    let var = |k: &str| std::env::var(k).ok();
    let (host, base) = service(&config, &var);
    Some((home.join("credentials").join(format!("{}.json", token_name(&host, &base))), format!("{base}/usages")))
}

/// The sign-in file, where the CLI keeps it
fn sign_in_file() -> Option<std::path::PathBuf> {
    where_to_ask().map(|(file, _)| file)
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

/// The account Kimi Code is signed in as, from the token's own subject
pub(super) fn account() -> Option<String> {
    super::subject_in_token(super::read_json(&sign_in_file()?)?.get("access_token")?.as_str()?)
}

pub(super) fn ask() -> Option<Limits> {
    let (file, usages) = where_to_ask()?;
    let token = token_in(&super::read_json(&file)?, super::now_ms() / 1000)?;
    let mut resp = super::get(&usages)
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

    /// Kimi Code 2 names its sign-in after the service it is pointed at:
    /// its own by the plain name, any other by a hash of where it is
    #[test]
    fn the_sign_in_is_found_for_the_service_kimi_is_pointed_at() {
        let none = |_: &str| None;
        assert_eq!(service("", &none), (OAUTH_HOST.to_string(), BASE_URL.to_string()));
        assert_eq!(token_name(OAUTH_HOST, BASE_URL), "kimi-code");
        let config = "[providers.\"managed:kimi-code\"]\nbase_url = \"https://kimi.example/coding/v1/\"\n\n[providers.\"managed:kimi-code\".oauth]\noauth_host = \"https://login.example\"\n";
        let (host, base) = service(config, &none);
        assert_eq!((host.as_str(), base.as_str()), ("https://login.example", "https://kimi.example/coding/v1"));
        let name = token_name(&host, &base);
        assert!(name.starts_with("kimi-code-env-") && name.len() == "kimi-code-env-".len() + 16, "{name}");
        assert_ne!(name, token_name(OAUTH_HOST, "https://kimi.example/coding/v1"));
        // A variable puts the settings aside for both
        let env = |k: &str| (k == "KIMI_CODE_BASE_URL").then(|| BASE_URL.to_string());
        assert_eq!(service(config, &env), (OAUTH_HOST.to_string(), BASE_URL.to_string()));
        // The hash of a known scope, worked out apart: sha256 of
        // {"oauthHost":"https://auth.kimi.com","baseUrl":"https://kimi.example/v1"}
        assert_eq!(token_name(OAUTH_HOST, "https://kimi.example/v1"), format!("kimi-code-env-{}", KNOWN_SCOPE_HASH));
    }

    /// Worked out apart (sha256 in another program), not by the code above
    const KNOWN_SCOPE_HASH: &str = "29838f8d9214a2f3";

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
