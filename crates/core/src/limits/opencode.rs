//! OpenCode Go's allowance: the subscription OpenCode sells for its own
//! models, asked of its service with the key OpenCode was connected with.
//!
//! OpenCode keeps that key in one of two places, by version: a JSON file of
//! connections, and -- from version 2 on -- a SQLite file of its own. Both are
//! looked in, read-only; then the variable OpenCode itself reads.

use super::{Allowance, Limits, Span};

/// Where OpenCode keeps its data (it follows XDG on every system)
fn data_dir() -> Option<std::path::PathBuf> {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(d) if !d.is_empty() => Some(std::path::PathBuf::from(d).join("opencode")),
        _ => Some(super::home()?.join(".local").join("share").join("opencode")),
    }
}

/// The name OpenCode files this subscription's key under
const GO: &str = "opencode-go";

fn key_in_file(dir: &std::path::Path) -> Option<String> {
    let v = super::read_json(&dir.join("auth.json"))?;
    let e = v.get(GO)?;
    (e.get("type")?.as_str()? == "api").then_some(())?;
    Some(e.get("key")?.as_str()?.trim().to_string()).filter(|k| !k.is_empty())
}

/// The key in OpenCode's own database files, the one OpenCode marks as in
/// use first, then the newest
fn key_in_database(dir: &std::path::Path) -> Option<String> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("opencode") && n.ends_with(".db"))
        })
        .collect();
    // The file named plainly is the one in use; the others are left behind
    files.sort_by_key(|p| p.file_name().and_then(|n| n.to_str()) != Some("opencode.db"));
    files.iter().find_map(|path| {
        let conn = crate::sqlite::peek(path).ok()?;
        let mut stmt = conn
            .prepare("SELECT value FROM credential WHERE integration_id = ?1 ORDER BY active DESC, time_created DESC")
            .ok()?;
        let rows = stmt.query_map([GO], |r| r.get::<_, String>(0)).ok()?;
        rows.flatten().find_map(|text| {
            let v: serde_json::Value = serde_json::from_str(&text).ok()?;
            (v.get("type")?.as_str()? == "key").then_some(())?;
            Some(v.get("key")?.as_str()?.trim().to_string()).filter(|k| !k.is_empty())
        })
    })
}

fn key() -> Option<String> {
    let dir = data_dir();
    dir.as_deref()
        .and_then(key_in_file)
        .or_else(|| dir.as_deref().and_then(key_in_database))
        .or_else(|| std::env::var("OPENCODE_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()))
}

pub(super) fn signed_in() -> bool {
    key().is_some()
}

pub(super) fn ask() -> Option<Limits> {
    let key = key()?;
    let mut resp = super::client()
        .get("https://opencode.ai/zen/go/v1/usage")
        .header("Authorization", &format!("Bearer {key}"))
        .header("Accept", "application/json")
        .call()
        .ok()?;
    parse(&resp.body_mut().read_json().ok()?)
}

/// The body: `usage` holds a rolling five hours, a week and a month, each a
/// percentage and when it resets
fn parse(v: &serde_json::Value) -> Option<Limits> {
    let u = v.get("usage")?;
    let mut wins = Vec::new();
    for (name, span) in [("rolling", Span::Hours5), ("weekly", Span::Days7), ("monthly", Span::Month)] {
        let Some(w) = u.get(name) else { continue };
        if let Some(pct) = super::number(w.get("percent")) {
            wins.push(Allowance::whole(span, super::Window::of_percent(pct, super::instant_of(w.get("resetsAt")))));
        }
    }
    Limits::of(wins, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_windows_are_read_by_name() {
        let v = serde_json::json!({"usage": {
            "rolling": {"status": "ok", "percent": 12.0, "resetsAt": "2026-09-29T05:00:00Z"},
            "weekly": {"status": "ok", "percent": 40},
            "monthly": {"status": "rate-limited", "percent": 100}
        }});
        let l = parse(&v).unwrap();
        assert_eq!(l.whole(Span::Hours5).map(|w| w.pct), Some(12));
        assert_eq!(l.whole(Span::Days7).map(|w| w.pct), Some(40));
        assert_eq!(l.whole(Span::Month).map(|w| w.pct), Some(100));
        assert_eq!(parse(&serde_json::json!({"error": {"type": "AuthError"}})), None);
    }

    /// The key is found in the connections file, and in OpenCode's own
    /// database when the file has none, without writing to either.
    #[test]
    fn the_key_is_found_in_either_place_opencode_keeps_it() {
        let dir = std::env::temp_dir().join(format!("shikisha-opencode-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("auth.json"), r#"{"opencode-go":{"type":"api","key":" go-1 "},"google":{"type":"oauth"}}"#).unwrap();
        assert_eq!(key_in_file(&dir).as_deref(), Some("go-1"));
        std::fs::write(dir.join("auth.json"), r#"{"anthropic":{"type":"api","key":"x"}}"#).unwrap();
        assert_eq!(key_in_file(&dir), None, "another service's key was taken");

        let db = dir.join("opencode.db");
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch(
                "CREATE TABLE credential (integration_id TEXT, value TEXT, active INTEGER, time_created INTEGER);
                 INSERT INTO credential VALUES ('opencode-go', '{\"type\":\"key\",\"key\":\"old\"}', 0, 1);
                 INSERT INTO credential VALUES ('opencode-go', '{\"type\":\"key\",\"key\":\"chosen\"}', 1, 0);
                 INSERT INTO credential VALUES ('other', '{\"type\":\"key\",\"key\":\"no\"}', 1, 9);",
            )
            .unwrap();
        }
        let before = std::fs::read(&db).unwrap();
        assert_eq!(key_in_database(&dir).as_deref(), Some("chosen"), "the key marked in use was not the one taken");
        assert_eq!(std::fs::read(&db).unwrap(), before, "looking changed OpenCode's file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
