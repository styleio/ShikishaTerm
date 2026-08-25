//! A logged-in browser, saved to a file and put back later.
//!
//! The real cost of a rally is not the work — it is arriving at a logged-out
//! browser and having to sign in before anything can begin. Once is fine.
//! Every time is the thing worth removing.
//!
//! What this does NOT do is reach into Chrome or Firefox and take their
//! cookies. Current Chrome encrypts its cookies so that only Chrome can read
//! them (app-bound, since version 127) — on purpose, to stop exactly the sort
//! of lifting a tool like this would otherwise do. Fighting that would mean
//! impersonating Chrome to its own key service: fragile against every update,
//! and the wrong side of a line drawn deliberately by the people who built the
//! browser. So this works the honest way instead. You sign in **once**, in a
//! browser tab of ours, and save that. It is our own profile, so we can read
//! all of it, including the httpOnly cookies a login is actually made of.
//!
//! The saved file is a login. It is kept beside the browser data it came from,
//! under the one folder that already holds every profile's cookies, and moving
//! it to another machine is something a person does by hand — the same way the
//! secrets file travels. Nothing here sends it anywhere.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;

/// One saved login: the cookies, and enough beside them to know what it is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub version: u32,
    /// How many cookies are inside, so a listing can say without opening it
    pub count: usize,
    /// The cookies exactly as the browser reported them
    pub cookies: serde_json::Value,
    /// The page origin's localStorage, `[[key, value], ...]`. Where a modern
    /// web app often keeps the token a cookie does not. Absent in older files,
    /// which is why it defaults empty rather than being required
    #[serde(default)]
    pub storage: serde_json::Value,
}

/// Where saved logins live: under the browser-data root, beside the profiles
/// whose cookies they are. Not a Drive-synced place by default, for the same
/// reason the profiles themselves are not — a login is not something to copy
/// across machines without meaning to
fn dir() -> PathBuf {
    let d = crate::config::browser_data_dir().join("logins");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Turn a label into a safe file stem. A login is named by the person or the
/// rally, so it has to survive whatever they call it without reaching outside
/// its folder
fn safe(label: &str) -> String {
    let s: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
        .collect();
    let s = s.trim().trim_matches('.').trim().to_string();
    if s.is_empty() { "login".into() } else { s }
}

fn path(label: &str) -> PathBuf {
    dir().join(format!("{}.json", safe(label)))
}

/// Write a login to its file.
///
/// Atomic, because a half-written login is worse than no login: it would load
/// as an empty set and quietly sign the browser out
pub fn save(label: &str, cookies: serde_json::Value, storage: serde_json::Value) -> Result<usize> {
    let count = cookies.as_array().map(|a| a.len()).unwrap_or(0);
    let saved = Saved { version: VERSION, count, cookies, storage };
    let text = serde_json::to_string_pretty(&saved)?;
    crate::crypto::write_atomic(&path(label), &text)
        .with_context(|| crate::i18n::tp("err.login.write", &[("label", label)]))?;
    Ok(count)
}

/// Read a login back, refusing a file from a version this one cannot promise
/// to understand rather than half-loading it
pub fn load(label: &str) -> Result<(serde_json::Value, serde_json::Value)> {
    let p = path(label);
    let text = std::fs::read_to_string(&p)
        .with_context(|| crate::i18n::tp("err.login.missing", &[("label", label)]))?;
    let saved: Saved = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .with_context(|| crate::i18n::tp("err.login.unreadable", &[("label", label)]))?;
    if saved.version > VERSION {
        anyhow::bail!(crate::i18n::tp("err.login.newer", &[("label", label)]));
    }
    Ok((saved.cookies, saved.storage))
}

/// One saved login, for a listing.
pub struct Entry {
    pub label: String,
    pub count: usize,
}

/// Every saved login, by name, newest first. Just enough to manage them: the
/// name a person gave it and how much is inside, never the cookies themselves
pub fn list() -> Vec<Entry> {
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return Vec::new();
    };
    let mut out: Vec<(std::time::SystemTime, Entry)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(label) = p.file_stem().map(|s| s.to_string_lossy().to_string()) else {
            continue;
        };
        // The count is read from the file; a file we cannot read is listed with
        // zero rather than hidden, so a broken login is visible and deletable
        let count = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<Saved>(t.trim_start_matches('\u{feff}')).ok())
            .map(|s| s.count)
            .unwrap_or(0);
        let when = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        out.push((when, Entry { label, count }));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, e)| e).collect()
}

/// Delete a saved login. Missing is success: the end state is what was asked
/// for either way
pub fn delete(label: &str) -> Result<()> {
    let p = path(label);
    if p.exists() {
        std::fs::remove_file(&p)
            .with_context(|| crate::i18n::tp("err.login.delete", &[("label", label)]))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_never_reaches_outside_its_folder() {
        assert_eq!(safe("../../etc/passwd"), "etcpasswd");
        assert_eq!(safe("my login"), "my login");
        assert_eq!(safe(""), "login");
        assert_eq!(safe("..."), "login");
        assert!(!path("../escape").to_string_lossy().contains(".."));
    }

    #[test]
    fn what_is_saved_is_what_comes_back() {
        // Uses the real folder, so name it something this test owns
        let label = "shikisha-selftest-roundtrip";
        let cookies = serde_json::json!([
            {"name": "sid", "value": "abc", "domain": ".example.com", "httpOnly": true},
            {"name": "theme", "value": "dark", "domain": ".example.com"}
        ]);
        let storage = serde_json::json!([["token", "xyz"], ["theme", "dark"]]);
        let n = save(label, cookies.clone(), storage.clone()).unwrap();
        assert_eq!(n, 2);
        // Both halves come back exactly: the cookies a login needs, and the
        // localStorage a modern app keeps its token in
        assert_eq!(load(label).unwrap(), (cookies, storage));
        assert!(list().iter().any(|e| e.label == label && e.count == 2));
        delete(label).unwrap();
        assert!(load(label).is_err(), "消したものは読めない");
        // Deleting what is already gone is not a failure
        delete(label).unwrap();
    }

    #[test]
    fn a_login_from_a_newer_version_is_refused_not_half_read() {
        let label = "shikisha-selftest-newer";
        let text = serde_json::to_string(&serde_json::json!({
            "version": VERSION + 1, "count": 1,
            "cookies": [{"name": "x", "value": "y"}]
        }))
        .unwrap();
        crate::crypto::write_atomic(&path(label), &text).unwrap();
        assert!(load(label).is_err(), "後の版は推測しない");
        delete(label).unwrap();
    }
}
