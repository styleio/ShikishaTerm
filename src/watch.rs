//! 設定ファイルの変更監視。保存したら再起動なしで反映するために使う。
//! 依存を増やさないよう、1秒ごとに更新時刻を見るだけの素朴な実装。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const INTERVAL: Duration = Duration::from_millis(1000);

pub struct Watcher {
    stamps: HashMap<PathBuf, Option<SystemTime>>,
    last_check: Instant,
}

impl Watcher {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let stamps = paths.into_iter().map(|p| { let t = mtime(&p); (p, t) }).collect();
        Self {
            stamps,
            last_check: Instant::now(),
        }
    }

    /// 監視対象を入れ替える (設定変更でワークスペース構成が変わったとき)
    pub fn retarget(&mut self, paths: Vec<PathBuf>) {
        self.stamps = paths.into_iter().map(|p| { let t = mtime(&p); (p, t) }).collect();
    }

    /// 変更があれば true。呼ぶたびに現在の状態を基準にし直す
    pub fn changed(&mut self) -> bool {
        if self.last_check.elapsed() < INTERVAL {
            return false;
        }
        self.last_check = Instant::now();
        let mut changed = false;
        for (path, stamp) in self.stamps.iter_mut() {
            let now = mtime(path);
            if now != *stamp {
                *stamp = now;
                changed = true;
            }
        }
        changed
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok())
}

/// 監視すべきファイル一覧を組み立てる
/// (設定本体・ワークスペース定義・自動化スクリプト・secrets)
pub fn watch_targets(cfg: Option<&crate::config::Config>, config_path: &Path) -> Vec<PathBuf> {
    let mut out = vec![config_path.to_path_buf()];
    let Some(cfg) = cfg else { return out };
    if let Some(p) = cfg.secrets_path() {
        out.push(p);
    }
    fn add_automation(out: &mut Vec<PathBuf>, spec: Option<String>) {
        let Some(s) = spec else { return };
        let p = crate::config::resolve_data_path(&s);
        if p.is_dir() {
            // フォルダ方式はイベントファイルを個別に見る
            if let Ok(entries) = std::fs::read_dir(&p) {
                for e in entries.flatten() {
                    let f = e.path();
                    if f.extension().and_then(|x| x.to_str()) == Some("lua") {
                        out.push(f);
                    }
                }
            }
        }
        out.push(p);
    }
    add_automation(&mut out, cfg.automation_path());
    for ws in &cfg.workspaces {
        if let Some(f) = &ws.file {
            out.push(crate::config::resolve_data_path(f));
        }
    }
    let (workspaces, _) = cfg.resolve_workspaces();
    for ws in &workspaces {
        add_automation(&mut out, ws.automation.clone());
        for t in &ws.tabs {
            add_automation(&mut out, t.cfg.automation_path());
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_change_and_rearms() {
        let dir = std::env::temp_dir().join("shikisha-watch");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("config.json");
        std::fs::write(&f, "{}").unwrap();

        let mut w = Watcher::new(vec![f.clone()]);
        // 監視間隔の直後は判定しない
        assert!(!w.changed());
        w.last_check = Instant::now() - INTERVAL * 2;
        assert!(!w.changed(), "変更が無ければ false");

        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&f, "{\"max_chain\":3}").unwrap();
        w.last_check = Instant::now() - INTERVAL * 2;
        assert!(w.changed(), "変更を検出する");

        w.last_check = Instant::now() - INTERVAL * 2;
        assert!(!w.changed(), "一度検出したら基準を更新する");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_appearing_counts_as_change() {
        let dir = std::env::temp_dir().join("shikisha-watch2");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("later.json");
        let mut w = Watcher::new(vec![f.clone()]);
        std::fs::write(&f, "{}").unwrap();
        w.last_check = Instant::now() - INTERVAL * 2;
        assert!(w.changed(), "後から作られたファイルも検出する");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
