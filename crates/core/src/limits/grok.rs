//! Grok's allowance, asked of the service the Grok CLI talks to, with the
//! sign-in the CLI keeps on this PC. The service turns away an ask that does
//! not say it comes from the CLI, so it is asked the way the CLI asks.

use super::{Allowance, Limits, Span, Window};

fn sign_in_file() -> Option<std::path::PathBuf> {
    Some(super::home_or("GROK_HOME", &[".grok"])?.join("auth.json"))
}

pub(super) fn signed_in() -> bool {
    sign_in_file().and_then(|f| super::read_json(&f)).and_then(|v| entry(&v, i64::MIN)).is_some()
}

/// The xAI sign-in's own name in the file. Other names may sit beside it
/// (an older sign-in, another issuer); those stand in only when it is absent
const XAI: &str = "https://auth.x.ai";

/// A usable sign-in from the file: its token and the account it is for.
///
/// The file is an object of sign-ins by issuer. The xAI one is taken while
/// it has life left; when the xAI one has run out there is nothing to send
/// (the CLI renews it next time it runs), and another issuer's is used only
/// when there is no xAI one at all
fn entry(v: &serde_json::Value, now: i64) -> Option<(String, Option<String>)> {
    let o = v.as_object()?;
    let usable = |e: &serde_json::Value| -> Option<(String, Option<String>)> {
        let token = e.get("key")?.as_str()?.trim();
        if token.is_empty() {
            return None;
        }
        let expires = e.get("expires_at").and_then(|x| x.as_str()).and_then(super::epoch_of);
        if expires.is_some_and(|at| at - now <= super::SPARE) {
            return None;
        }
        Some((token.to_string(), e.get("user_id").and_then(|u| u.as_str()).map(str::to_string)))
    };
    let is_xai = |k: &str| k == XAI || k.starts_with(&format!("{XAI}::"));
    if o.keys().any(|k| is_xai(k)) {
        return o.iter().filter(|(k, _)| is_xai(k)).find_map(|(_, e)| usable(e));
    }
    o.values().find_map(usable)
}

pub(super) fn ask() -> Option<Limits> {
    let (token, user) = entry(&super::read_json(&sign_in_file()?)?, super::now_ms() / 1000)?;
    let base = std::env::var("GROK_CLI_CHAT_PROXY_BASE_URL").ok().filter(|b| !b.trim().is_empty());
    let base = base.as_deref().unwrap_or("https://cli-chat-proxy.grok.com/v1").trim_end_matches('/');
    let mut req = super::client()
        .get(&format!("{base}/billing?format=credits"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-XAI-Token-Auth", "xai-grok-cli")
        .header("Accept", "application/json");
    if let Some(u) = &user {
        req = req.header("x-userid", u);
    }
    let mut resp = req.call().ok()?;
    parse(&resp.body_mut().read_json().ok()?)
}

/// A money amount as the service writes it: `{"val": "12.5"}`
fn money(v: Option<&serde_json::Value>) -> Option<f64> {
    super::number(v?.get("val"))
}

/// The billing view: a credit percentage for the week, and for accounts
/// billed by the month, what is used of the month's amount. Only what the
/// service says: a missing percentage is not taken to mean nothing is used
fn parse(v: &serde_json::Value) -> Option<Limits> {
    let c = v.get("config").unwrap_or(v);
    let ends = super::instant_of(c.pointer("/currentPeriod/end").or_else(|| c.get("billingPeriodEnd")));
    let mut wins = Vec::new();
    if let Some(pct) = super::number(c.get("creditUsagePercent")) {
        wins.push(Allowance::whole(Span::Days7, Window::of_percent(pct, ends)));
    } else if let (Some(of), Some(used)) = (money(c.get("monthlyLimit")).filter(|m| *m > 0.0), money(c.get("used"))) {
        wins.push(Allowance::whole(Span::Month, Window::of_percent(used / of * 100.0, ends)));
    }
    Limits::of(wins, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xai_sign_in_is_the_one_sent() {
        let v = serde_json::json!({
            "https://old.example": {"key": "old", "user_id": "u0"},
            "https://auth.x.ai::cli": {"key": "new", "user_id": "u1", "expires_at": "2026-09-30T00:00:00Z"}
        });
        let now = super::super::epoch_of("2026-09-29T00:00:00Z").unwrap();
        assert_eq!(entry(&v, now), Some(("new".into(), Some("u1".into()))));
        // Run out: nothing is sent, and the other issuer does not stand in
        let later = super::super::epoch_of("2026-09-30T00:00:00Z").unwrap();
        assert_eq!(entry(&v, later), None, "an expired sign-in, or another issuer's, was sent");
        // With no xAI sign-in at all, another issuer's is used
        assert_eq!(entry(&serde_json::json!({"x": {"key": "k"}}), now), Some(("k".into(), None)));
        assert_eq!(entry(&serde_json::json!({"x": {"key": ""}}), now), None);
    }

    #[test]
    fn a_week_of_credits_or_a_month_of_money() {
        let week = serde_json::json!({"config": {"creditUsagePercent": 42.4, "currentPeriod": {"end": "2026-10-01T00:00:00Z"}}});
        let l = parse(&week).unwrap();
        assert_eq!(l.whole(Span::Days7).map(|w| w.pct), Some(42));
        assert!(l.whole(Span::Days7).unwrap().resets_at.is_some());
        let month = serde_json::json!({"monthlyLimit": {"val": "20"}, "used": {"val": "5"}, "billingPeriodEnd": "2026-10-01T00:00:00Z"});
        assert_eq!(parse(&month).unwrap().whole(Span::Month).map(|w| w.pct), Some(25));
        assert_eq!(parse(&serde_json::json!({"monthlyLimit": {"val": "0"}, "used": {"val": "5"}})), None);
        assert_eq!(parse(&serde_json::json!({"subscriptionTier": "free"})), None, "a body with no amounts made a percentage");
    }
}
