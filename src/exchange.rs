//! Data hand-off area for rallies (exchange).
//!
//! Lua files handed from AI to browser, and records that can be pasted and
//! replayed, are stored in a local area not synced by Drive (%LOCALAPPDATA%),
//! organized **one folder per run**.
//!
//! Design decisions:
//! - No fixed file names (that would prevent running multiple tasks
//!   concurrently). Each run gets a unique folder, and simple names are fine
//!   inside it.
//! - Cleanup is mandatory so folders don't bloat over time:
//!   - Temp files (in.lua / human.txt) are deleted as soon as they're
//!     consumed (`take` deletes them).
//!   - Leftover junk from abnormal exits is reclaimed by sweeping old
//!     folders at startup.
//!
//! Raw Lua written by an AI is received byte-exact from files in this area,
//! rather than by "reading the TUI screen." This makes false detections from
//! rendering artifacts (fence loss, instruction echo) structurally impossible.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The exchange root. %LOCALAPPDATA%\ShikishaTerm\exchange (falls back to the
/// temp folder if unavailable). Never placed under Drive sync (beside the
/// app binary).
pub fn root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShikishaTerm").join("exchange")
}

/// A per-process sequence number, so multiple runs starting within the same
/// millisecond don't collide.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a new run folder and return its path.
/// The name is unique as "epoch-ms-sequence" (avoids fixed names so runs can
/// happen concurrently).
pub fn new_run() -> std::io::Result<PathBuf> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = root().join(format!("{ms:013}-{n:04}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Whether the given path is under the exchange root (prevents path
/// traversal). A not-yet-created file can't be canonicalized, so fall back
/// to checking the parent folder.
pub fn within_root(p: &Path) -> bool {
    let Ok(rr) = root().canonicalize() else {
        return false;
    };
    if let Ok(pp) = p.canonicalize() {
        return pp.starts_with(&rr);
    }
    p.parent()
        .and_then(|par| par.canonicalize().ok())
        .is_some_and(|par| par.starts_with(&rr))
}

/// Read a file and delete it, returning its contents (None if absent).
/// Used to "consume" temp files (in.lua / human.txt). Restricted to under
/// the exchange root.
pub fn take(path: &Path) -> Option<String> {
    if !within_root(path) {
        return None;
    }
    // Read as bytes and lossy-decode rather than read_to_string: a statement an
    // AI writes here can contain invalid UTF-8, and a strict read would drop the
    // whole turn (returning None), stalling the discussion. Lossy keeps the turn.
    let bytes = std::fs::read(path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Append one line of text (for the record.lua log). Restricted to under the
/// exchange root.
pub fn append(path: &Path, text: &str) -> std::io::Result<()> {
    if !within_root(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            crate::i18n::t("err.exchange.outside_root"),
        ));
    }
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", text.trim_end())
}

/// Return the newest run folder (by modification time). Used for result
/// downloads.
pub fn latest_run() -> Option<PathBuf> {
    let rd = std::fs::read_dir(root()).ok()?;
    rd.flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let m = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((m, e.path()))
        })
        .max_by_key(|(m, _)| *m)
        .map(|(_, p)| p)
}

/// Return recent run folders, newest first (up to `limit` entries). Used for
/// the result-download history.
pub fn recent_runs(limit: usize) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(root()) else {
        return Vec::new();
    };
    let mut runs: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let m = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((m, e.path()))
        })
        .collect();
    runs.sort_by(|a, b| b.0.cmp(&a.0));
    runs.into_iter().take(limit).map(|(_, p)| p).collect()
}

/// Resolve a run folder from its run id (folder name). Only direct children
/// of the exchange root are allowed.
pub fn run_by_id(id: &str) -> Option<PathBuf> {
    // Anti-traversal: reject any id containing a path separator.
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return None;
    }
    let p = root().join(id);
    (p.is_dir() && within_root(&p)).then_some(p)
}

/// Startup cleanup. Removes any run folder/file whose modification time is
/// older than `days` days. On a clean exit temp files are already gone, so
/// this is only a rescue for junk left by abnormal exits.
pub fn sweep_old(days: u64) {
    let Ok(rd) = std::fs::read_dir(root()) else {
        return;
    };
    let Some(cutoff) =
        std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(days * 86_400))
    else {
        return;
    };
    let mut removed = 0u32;
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if !old {
            continue;
        }
        let p = e.path();
        let ok = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        if ok.is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        crate::append_hook_log(&crate::i18n::tp(
            "err.exchange.swept_old_runs",
            &[("removed", &removed.to_string())],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_run_makes_unique_dirs_under_root() {
        let a = new_run().unwrap();
        let b = new_run().unwrap();
        assert_ne!(a, b, "run フォルダは毎回ユニーク");
        assert!(within_root(&a));
        assert!(a.starts_with(root()));
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn take_reads_then_deletes() {
        let run = new_run().unwrap();
        let f = run.join("in.lua");
        std::fs::write(&f, "browser_go(\"br\",\"reload\")").unwrap();
        let got = take(&f);
        assert_eq!(got.as_deref(), Some("browser_go(\"br\",\"reload\")"));
        assert!(!f.exists(), "消費後は削除されている");
        assert_eq!(take(&f), None, "二度目は None");
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn append_confined_to_root() {
        let run = new_run().unwrap();
        let rec = run.join("record.lua");
        append(&rec, "line1").unwrap();
        append(&rec, "line2\n").unwrap();
        let body = std::fs::read_to_string(&rec).unwrap();
        assert_eq!(body, "line1\nline2\n");
        // Outside the root is rejected
        assert!(append(Path::new("C:/windows/x.lua"), "x").is_err());
        let _ = std::fs::remove_dir_all(&run);
    }
}
