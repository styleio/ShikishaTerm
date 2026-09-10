//! Watches config files for changes, so edits take effect without a restart.
//! A simple implementation that just checks the modification time once a
//! second, to avoid adding a dependency.

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

    /// Swap out the watched targets (when config changes alter the workspace
    /// layout).
    pub fn retarget(&mut self, paths: Vec<PathBuf>) {
        self.stamps = paths.into_iter().map(|p| { let t = mtime(&p); (p, t) }).collect();
    }

    /// True if something changed. Each call re-baselines against the current
    /// state.
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

/// Build the list of files that should be watched
/// (the main config, workspace definitions, automation scripts, secrets).
pub fn watch_targets(cfg: Option<&crate::config::Config>, config_path: &Path) -> Vec<PathBuf> {
    let mut out = vec![config_path.to_path_buf()];
    let Some(cfg) = cfg else { return out };
    // Deliberately NOT watching secrets.json: the app writes it itself from the
    // settings UI (saving a key / a provider secret), and watching it would make
    // that self-write trigger a full config reload — which redraws the board and
    // kicks the user out of the settings screen mid-edit (they land on INDEX and
    // can't finish registering the provider). Secrets are re-resolved on the next
    // real config reload (e.g. when the whole config is saved), which is enough.
    fn add_automation(out: &mut Vec<PathBuf>, spec: Option<String>) {
        let Some(s) = spec else { return };
        let p = crate::config::resolve_data_path(&s);
        if p.is_dir() {
            // For the folder-based approach, watch each event file individually
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
        // No judgment is made immediately after the watch interval
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
