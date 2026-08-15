//! Notifies about new versions. On startup, checks GitHub Releases once in
//! the background, and shows a toast if a newer version is out.
//! No auto-update (portable distribution; a human decides whether to swap
//! it in). Doesn't repeat the same notification on every launch (records
//! the version already notified about).

use std::sync::mpsc;

/// Checks the latest version in the background and sends only the version
/// number that should be notified about.
/// If there's nothing to report, nothing arrives (the receiver can just
/// peek with try_recv).
pub fn spawn_check() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(v) = latest_unnotified() {
            let _ = tx.send(v);
        }
    });
    rx
}

fn latest_unnotified() -> Option<String> {
    let latest = fetch_latest()?;
    if !is_newer(&latest, env!("CARGO_PKG_VERSION")) {
        return None;
    }
    if load_notified().as_deref() == Some(latest.as_str()) {
        return None;
    }
    save_notified(&latest);
    Some(latest)
}

/// The releases endpoint is built from the repository in Cargo.toml.
fn releases_api() -> Option<String> {
    let repo = env!("CARGO_PKG_REPOSITORY").strip_prefix("https://github.com/")?;
    Some(format!(
        "https://api.github.com/repos/{}/releases/latest",
        repo.trim_end_matches('/')
    ))
}

fn fetch_latest() -> Option<String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let mut resp = agent
        .get(&releases_api()?)
        .header(
            "User-Agent",
            concat!("shikisha-term/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    let tag = v.get("tag_name")?.as_str()?;
    let ver = tag.trim().trim_start_matches(['v', 'V']).to_string();
    (!ver.is_empty()).then_some(ver)
}

/// Numeric comparison such that "0.1.10" > "0.1.9". Any non-numeric part is
/// treated as 0.
fn parse(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().unwrap_or(0)
        })
        .collect()
}

fn is_newer(remote: &str, local: &str) -> bool {
    parse(remote) > parse(local)
}

/// Where the last-notified version is stored (not written back into
/// config).
fn notified_path() -> std::path::PathBuf {
    crate::config::state_path("update-notified")
}

fn load_notified() -> Option<String> {
    let s = std::fs::read_to_string(notified_path()).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Silently give up on failure (it'll just be notified again next launch).
fn save_notified(v: &str) {
    let _ = crate::crypto::write_atomic(&notified_path(), v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"), "文字列比較だと 10 < 9 になる");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("1.0", "0.99.99"));
    }

    #[test]
    fn api_url_comes_from_cargo_metadata() {
        assert_eq!(
            releases_api().as_deref(),
            Some("https://api.github.com/repos/styleio/ShikishaTerm/releases/latest")
        );
    }
}
