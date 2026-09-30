//! Gemini's allowance, for somebody signed in to the Gemini CLI with a Google
//! account: asked of the service the CLI itself talks to, with the sign-in
//! the CLI keeps on this PC.
//!
//! The sign-in is used only while it is good. Renewing one that has run out
//! would mean writing the CLI's own file, which is the CLI's to write; until
//! the CLI next runs and renews it, there is no reading. Somebody using the
//! CLI with an API key has no allowance of this kind, and nothing to read.
//!
//! The service counts by model, each on its own clock, and does not say how
//! long a window is. The one with the least left is what is shown, under its
//! model's name.

use std::sync::Mutex;

use super::{Allowance, Limits, Window};

fn sign_in_file() -> Option<std::path::PathBuf> {
    Some(super::home()?.join(".gemini").join("oauth_creds.json"))
}

/// The access token, while it has life left in it (`expiry_date` is in
/// milliseconds)
fn token_in(v: &serde_json::Value, now: i64) -> Option<String> {
    let expires = super::number(v.get("expiry_date"))? as i64 / 1000;
    if expires - now <= super::SPARE {
        return None;
    }
    Some(v.get("access_token")?.as_str()?.trim().to_string()).filter(|t| !t.is_empty())
}

/// The account the Google sign-in is for: the subject of the ID token kept
/// beside the access token (the access token itself says nothing of it)
pub(super) fn account() -> Option<String> {
    super::subject_in_token(super::read_json(&sign_in_file()?)?.get("id_token")?.as_str()?)
}

pub(super) fn signed_in() -> bool {
    sign_in_file().is_some_and(|f| f.is_file())
}

const SERVICE: &str = "https://cloudcode-pa.googleapis.com/v1internal";

/// The project the service keeps this account's allowance under, by the
/// token it was learnt with. Asked once per sign-in rather than once per
/// reading: it does not change while the sign-in lasts
static PROJECT: Mutex<Option<(String, String)>> = Mutex::new(None);

fn project(token: &str) -> Option<String> {
    if let Ok(p) = PROJECT.lock()
        && let Some((t, id)) = p.as_ref()
        && t == token
    {
        return Some(id.clone());
    }
    let mut resp = super::client()
        .post(&format!("{SERVICE}:loadCodeAssist"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send_json(serde_json::json!({"metadata": {"ideType": "GEMINI_CLI", "pluginType": "GEMINI"}}))
        .ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    let id = v.get("cloudaicompanionProject")?.as_str()?.to_string();
    if let Ok(mut p) = PROJECT.lock() {
        *p = Some((token.to_string(), id.clone()));
    }
    Some(id)
}

pub(super) fn ask() -> Option<Limits> {
    let token = token_in(&super::read_json(&sign_in_file()?)?, super::now_ms() / 1000)?;
    let project = project(&token)?;
    let mut resp = super::client()
        .post(&format!("{SERVICE}:retrieveUserQuota"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send_json(serde_json::json!({"project": project}))
        .ok()?;
    parse(&resp.body_mut().read_json().ok()?)
}

/// The model with the least left, out of every model's count
fn parse(v: &serde_json::Value) -> Option<Limits> {
    let list = v.get("buckets").or(Some(v)).and_then(|b| b.as_array())?;
    let tightest = list
        .iter()
        .filter_map(|b| {
            let left = super::number(b.get("remainingFraction"))?;
            let model = b.get("modelId")?.as_str()?.trim();
            (!model.is_empty()).then(|| (1.0 - left.clamp(0.0, 1.0), model, super::instant_of(b.get("resetTime"))))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))?;
    let (used, model, reset) = tightest;
    Limits::of(
        vec![Allowance { span: None, only: Some(model.to_string()), window: Window::of_percent(used * 100.0, reset) }],
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_is_sent_only_while_it_is_good() {
        let v = serde_json::json!({"access_token": "ya29.x", "refresh_token": "r", "expiry_date": 2_000_000});
        assert_eq!(token_in(&v, 1_000).as_deref(), Some("ya29.x"));
        assert_eq!(token_in(&v, 2_000), None, "a sign-in that has run out was sent");
        assert_eq!(token_in(&serde_json::json!({"access_token": "t"}), 0), None);
    }

    #[test]
    fn the_model_with_the_least_left_is_shown() {
        let v = serde_json::json!({"buckets": [
            {"modelId": "gemini-2.5-flash", "remainingFraction": 0.9, "resetTime": "2026-09-29T10:00:00Z"},
            {"modelId": "gemini-2.5-pro", "remainingFraction": 0.35, "resetTime": "2026-09-29T08:00:00Z"},
            {"modelId": "", "remainingFraction": 0.0}
        ]});
        let l = parse(&v).unwrap();
        assert_eq!(l.wins.len(), 1);
        assert_eq!(l.wins[0].only.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(l.wins[0].span, None, "a window of unknown length was given a name");
        assert_eq!(l.wins[0].window.pct, 65);
        assert_eq!(parse(&serde_json::json!({"buckets": []})), None);
    }
}
