//! Cursor's allowance for the month, asked of the page Cursor's own
//! dashboard reads, with the sign-in Cursor keeps on this PC.
//!
//! Two places hold that sign-in on Windows: the file the command-line agent
//! wrote before it moved its sign-in into the system's key store, and the
//! editor's own settings database. Both are read, never written. The
//! dashboard answers only an ask that looks like the dashboard's own (its
//! cookie, its origin); anything else is turned away.

use super::{Allowance, Limits, Span, Window};

/// Where Cursor keeps its things on Windows
fn cursor_dir() -> Option<std::path::PathBuf> {
    match std::env::var_os("APPDATA") {
        Some(d) if !d.is_empty() => Some(std::path::PathBuf::from(d).join("Cursor")),
        _ => Some(super::home()?.join("AppData").join("Roaming").join("Cursor")),
    }
}

fn token_in_agent_file(dir: &std::path::Path) -> Option<String> {
    let v = super::read_json(&dir.join("auth.json"))?;
    Some(v.get("accessToken")?.as_str()?.trim().to_string()).filter(|t| !t.is_empty())
}

fn token_in_editor(dir: &std::path::Path) -> Option<String> {
    let path = dir.join("User").join("globalStorage").join("state.vscdb");
    if !path.is_file() {
        return None;
    }
    let conn = crate::sqlite::peek(&path).ok()?;
    // The value is text in some builds and bytes in others
    let value: rusqlite::types::Value = conn
        .query_row("SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'", [], |r| r.get(0))
        .ok()?;
    let text = match value {
        rusqlite::types::Value::Text(t) => t,
        rusqlite::types::Value::Blob(b) => String::from_utf8(b).ok()?,
        _ => return None,
    };
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

/// Who the sign-in is for and until when, out of the token itself (its
/// middle part is JSON). The account's id is half of the dashboard's cookie
fn subject_of(token: &str) -> Option<(String, Option<i64>)> {
    let v = super::token_claims(token)?;
    let sub = v.get("sub")?.as_str()?.trim();
    (!sub.is_empty()).then(|| (sub.to_string(), v.get("exp").and_then(|e| e.as_i64())))
}

/// A sign-in, from whichever place has one that has not run out
fn sign_in(now: i64) -> Option<(String, String)> {
    let dir = cursor_dir()?;
    [token_in_agent_file(&dir), token_in_editor(&dir)].into_iter().flatten().find_map(|token| {
        let (sub, exp) = subject_of(&token)?;
        (!exp.is_some_and(|e| e - now <= super::SPARE)).then_some((token, sub))
    })
}

pub(super) fn signed_in() -> bool {
    cursor_dir().is_some_and(|d| token_in_agent_file(&d).or_else(|| token_in_editor(&d)).is_some())
}

pub(super) fn ask() -> Option<Limits> {
    let (token, sub) = sign_in(super::now_ms() / 1000)?;
    let cookie = format!("WorkosCursorSessionToken={}%3A%3A{token}", crate::runtime::percent_encode(&sub));
    let mut resp = super::client()
        .get("https://cursor.com/api/usage-summary")
        .header("Cookie", &cookie)
        .header("Accept", "application/json")
        .header("Origin", "https://cursor.com")
        .header("Referer", "https://cursor.com/dashboard")
        .call()
        .ok()?;
    parse(&resp.body_mut().read_json().ok()?)
}

/// The month's plan: what is used of it, and when the month ends. The plan
/// a team pays for is not the person's own and says nothing here
fn parse(v: &serde_json::Value) -> Option<Limits> {
    let plan = v.pointer("/individualUsage/plan")?;
    if plan.get("enabled").and_then(|e| e.as_bool()) == Some(false) {
        return None;
    }
    let from_amounts = || {
        let limit = super::number(plan.get("limit")).filter(|l| *l > 0.0)?;
        Some(super::number(plan.get("used"))? / limit * 100.0)
    };
    let pct = from_amounts().or_else(|| super::number(plan.get("totalPercentUsed")))?;
    let ends = super::instant_of(v.get("billingCycleEnd"));
    Limits::of(vec![Allowance::whole(Span::Month, Window::of_percent(pct, ends))], None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(payload: &str) -> String {
        use base64::Engine as _;
        format!("h.{}.s", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn the_account_and_its_time_are_read_out_of_the_token() {
        assert_eq!(subject_of(&token(r#"{"sub":"auth0|user_1","exp":1900000000}"#)), Some(("auth0|user_1".into(), Some(1_900_000_000))));
        assert_eq!(subject_of(&token(r#"{"sub":"","exp":1}"#)), None);
        assert_eq!(subject_of("not-a-token"), None);
    }

    #[test]
    fn the_month_is_read_from_the_amounts_first() {
        let v = serde_json::json!({
            "billingCycleEnd": "2026-10-15T00:00:00.000Z",
            "individualUsage": {"plan": {"enabled": true, "used": 500, "limit": 2000, "totalPercentUsed": 30}}
        });
        let l = parse(&v).unwrap();
        assert_eq!(l.whole(Span::Month).map(|w| w.pct), Some(25), "the rounded percentage won over the amounts");
        assert!(l.whole(Span::Month).unwrap().resets_at.is_some());
        let only_pct = serde_json::json!({"individualUsage": {"plan": {"totalPercentUsed": 61.2}}});
        assert_eq!(parse(&only_pct).unwrap().whole(Span::Month).map(|w| w.pct), Some(61));
        let team = serde_json::json!({"individualUsage": {"plan": {"enabled": false, "totalPercentUsed": 0}}});
        assert_eq!(parse(&team), None, "a plan somebody else pays for was shown as this person's");
    }

    /// The editor's database is looked into without being changed.
    #[test]
    fn the_editor_s_sign_in_is_read_without_writing() {
        let dir = std::env::temp_dir().join(format!("shikisha-cursor-{}", crate::random_hex(6)));
        let store = dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&store).unwrap();
        let db = store.join("state.vscdb");
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch("CREATE TABLE ItemTable (key TEXT UNIQUE, value BLOB);").unwrap();
            c.execute("INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', ?1)", [b"tok-9".to_vec()]).unwrap();
        }
        let before = std::fs::read(&db).unwrap();
        assert_eq!(token_in_editor(&dir).as_deref(), Some("tok-9"));
        assert_eq!(std::fs::read(&db).unwrap(), before, "looking changed Cursor's file");
        assert_eq!(token_in_agent_file(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
