//! 新しい版の知らせ。起動時に GitHub Releases を裏で一度だけ見に行き、
//! 新しい版が出ていたらトーストで知らせる。
//! 自動更新はしない (ポータブル配布。入れ替えるかどうかは人が決める)。
//! 同じ版を起動のたびに繰り返し知らせない (一度知らせた版を書き留める)。

use std::sync::mpsc;

/// 裏で最新版を確かめ、知らせるべき版番号だけを送る。
/// 何も無ければ何も届かない (受け手は try_recv で覗くだけでよい)
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

/// リリースの置き場は Cargo.toml の repository から組み立てる
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

/// "0.1.10" > "0.1.9" のような数値比較。数字にならない部分は 0 とみなす
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

/// 一度知らせた版の置き場 (configには書き戻さない)
fn notified_path() -> std::path::PathBuf {
    crate::config::state_path("update-notified")
}

fn load_notified() -> Option<String> {
    let s = std::fs::read_to_string(notified_path()).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// 失敗しても黙って諦める (次の起動でもう一度知らせるだけ)
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
