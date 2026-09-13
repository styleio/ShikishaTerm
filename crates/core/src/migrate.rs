//! What happens to a person's files the first time a new version runs over them.
//!
//! A settings file outlives the version that wrote it, and the version that
//! reads it next may want it in another shape. Two things are promised here:
//!
//! 1. **Nothing is changed before a copy of it is kept.** The first start of
//!    a version copies `config/*.json` and `desks/*.json` into
//!    `data/backup/<old version>-<stamp>/`, and only then touches anything.
//!    If the copy cannot be made, nothing is touched. The five newest copies
//!    are kept.
//! 2. **A version that was skipped is not skipped.** The steps that reshape
//!    a file are kept here in order, one per version that changed the shape,
//!    and a start over a file from four versions ago runs all four in turn.
//!    The executable is replaced in one go; the data is carried forward one
//!    step at a time -- that is where "gradual" earns its keep, and it costs
//!    no extra download and no extra restart.
//!
//! Each step is a function over the file as a JSON value, so a key it has
//! never heard of comes out exactly as it went in. A step that fails stops
//! the rest: the file is left as the last good step left it, the version
//! stamp is not advanced (so the next start tries again from there), and the
//! person is told where the copy is.
//!
//! The version that last ran is `data/version`. A layout that has settings
//! but no stamp ran a version from before the stamp existed, and is treated
//! as [`BASELINE`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The last version that wrote no stamp. Settings with no `data/version`
/// beside them are assumed to be in this version's shape
pub const BASELINE: &str = "0.8.0";

/// How many backups are kept
const KEEP: usize = 5;

/// One reshaping: what version the file is in afterwards, and how
pub struct Step {
    pub to: &'static str,
    pub apply: fn(&mut serde_json::Value) -> Result<()>,
}

/// Every reshaping there has ever been, oldest first. A version that changes
/// the shape of a settings file adds one line here, and a fixture of a real
/// file from the version before it under `tests/fixtures/`, so the test that
/// walks every fixture to the present keeps walking.
const STEPS: &[Step] = &[Step { to: "0.10.0", apply: to_0_10_0 }];

/// The unit a person switches between is called a desk.
///
/// It had another name, and the name was the key the settings file is written
/// with. Read by a version that only knows the new one, a file written with the
/// old key is a file with no desks in it -- the app comes up empty and nothing
/// says why, which is the worst way for a rename to arrive. So the file is
/// brought forward here, including the paths of the definition files, which
/// lived in a folder named after the old word
fn to_0_10_0(doc: &mut serde_json::Value) -> Result<()> {
    let Some(root) = doc.as_object_mut() else {
        return Ok(());
    };
    if let Some(list) = root.remove("workspaces") {
        root.entry("desks").or_insert(list);
    }
    if let Some(note) = root.remove("//workspaces") {
        root.entry("//desks").or_insert(note);
    }
    for desk in root
        .get_mut("desks")
        .and_then(|d| d.as_array_mut())
        .into_iter()
        .flatten()
    {
        let Some(file) = desk.get_mut("file").and_then(|f| f.as_str()).map(str::to_string) else {
            continue;
        };
        if let Some(rest) = file.strip_prefix("workspaces/").or_else(|| file.strip_prefix("workspaces\\")) {
            desk["file"] = serde_json::json!(format!("desks/{rest}"));
        }
    }
    Ok(())
}

/// What the first start of this version did, for the person and the log
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    /// The version the files were in before, when they were not already current
    pub from: Option<String>,
    /// Where the copy was put, when one was made
    pub backup: Option<PathBuf>,
    /// The step that failed, and why, when one did
    pub failed: Option<String>,
}

fn version_path(root: &Path) -> PathBuf {
    root.join("data").join("version")
}

/// The version that last ran on a layout, as recorded
fn recorded_at(root: &Path) -> Option<String> {
    let s = std::fs::read_to_string(version_path(root)).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn record(root: &Path, v: &str) {
    let _ = crate::crypto::write_atomic(&version_path(root), v);
}

/// What a person wrote and would miss: settings, secrets, desk files
fn owned_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["config", "desks"] {
        let Ok(rd) = std::fs::read_dir(root.join(dir)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") && p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Copies the owned files into a new folder under `data/backup`, named for
/// the version they came from. Nothing is copied if there is nothing to copy
fn back_up(root: &Path, from: &str) -> Result<Option<PathBuf>> {
    let files = owned_files(root);
    if files.is_empty() {
        return Ok(None);
    }
    let stamp = crate::hooks::local_stamp("%Y%m%d-%H%M%S");
    let dest = root.join("data").join("backup").join(format!("{from}-{stamp}"));
    for f in &files {
        let rel = f.strip_prefix(root).unwrap_or(f);
        let to = dest.join(rel);
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
        }
        std::fs::copy(f, &to).with_context(|| format!("copy {} to {}", f.display(), to.display()))?;
    }
    prune_backups(&root.join("data").join("backup"));
    Ok(Some(dest))
}

/// Keeps the newest [`KEEP`] backup folders. Names sort by version then
/// stamp, and the stamp is what decides age, so the sort is by stamp
fn prune_backups(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let stamp = name.rsplit('-').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("-");
            (stamp, e.path())
        })
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in dirs.into_iter().skip(KEEP) {
        let _ = std::fs::remove_dir_all(p);
    }
}

/// Runs every step later than `from` over the document, in order. Returns
/// the version the document is in afterwards, and the error of the step that
/// failed, if one did. The document is left as the last good step left it
pub fn run_steps(doc: &mut serde_json::Value, from: &str, steps: &[Step]) -> (String, Option<String>) {
    let mut at = from.to_string();
    for s in steps {
        if !crate::update::is_newer(s.to, &at) {
            continue;
        }
        if let Err(e) = (s.apply)(doc) {
            let msg = format!("{at} -> {}: {e}", s.to);
            return (at, Some(msg));
        }
        at = s.to.to_string();
    }
    (at, None)
}

/// Reads a JSON file as a value, tolerating a BOM. `None` when it cannot be
/// read as JSON at all -- a file that cannot be read is not one to rewrite
fn read_doc(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// The first start of this version over this layout: back up, then carry
/// the files forward one step at a time. Says what happened.
pub fn on_start() -> Outcome {
    on_start_at(&crate::config::root_dir(), env!("CARGO_PKG_VERSION"), STEPS)
}

fn on_start_at(root: &Path, current: &str, steps: &[Step]) -> Outcome {
    let has_settings = !owned_files(root).is_empty();
    let from = match recorded_at(root) {
        Some(v) => v,
        // No stamp and nothing written yet: a first start. Nothing to carry
        None if !has_settings => {
            record(root, current);
            return Outcome::default();
        }
        None => BASELINE.to_string(),
    };
    if from == current {
        // Already this version; a layout from before the stamp gets one now
        if recorded_at(root).is_none() {
            record(root, current);
        }
        return Outcome::default();
    }
    let mut out = Outcome { from: Some(from.clone()), ..Default::default() };
    if !has_settings {
        record(root, current);
        return out;
    }
    match back_up(root, &from) {
        Ok(p) => out.backup = p,
        Err(e) => {
            // Not safe to change anything. Try again next start
            out.failed = Some(format!("backup: {e}"));
            crate::append_hook_log(&format!("Update from {from} to {current}: no backup could be made, files left as they are: {e}"));
            return out;
        }
    }
    // The steps run over the settings file and every desk file alike:
    // a desk file is the same shape as a desk inside the settings
    let mut reached = current.to_string();
    for f in owned_files(root) {
        if f.file_name().is_some_and(|n| n == "secrets.json") {
            continue;
        }
        let Some(mut doc) = read_doc(&f) else { continue };
        let before = doc.clone();
        let (at, err) = run_steps(&mut doc, &from, steps);
        if doc != before
            && let Ok(text) = serde_json::to_string_pretty(&doc)
                && let Err(e) = crate::crypto::write_atomic(&f, &text) {
                    out.failed = Some(format!("write {}: {e}", f.display()));
                    break;
                }
        if let Some(e) = err {
            out.failed = Some(e);
            if crate::update::is_newer(&reached, &at) {
                reached = at;
            }
            break;
        }
    }
    match &out.failed {
        None => {
            record(root, current);
            crate::append_hook_log(&format!(
                "Updated from {from} to {current}; files backed up to {}",
                out.backup.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
            ));
        }
        Some(e) => {
            // Advance only as far as every file got, so the next start
            // resumes from there rather than repeating what succeeded
            if reached != current && crate::update::is_newer(&reached, &from) {
                record(root, &reached);
            }
            crate::append_hook_log(&format!("Updating files from {from} to {current} stopped: {e}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bump(doc: &mut serde_json::Value) -> Result<()> {
        let n = doc.get("n").and_then(|v| v.as_u64()).unwrap_or(0);
        doc["n"] = serde_json::json!(n + 1);
        Ok(())
    }
    fn fail(_: &mut serde_json::Value) -> Result<()> {
        anyhow::bail!("no")
    }

    /// A file four versions behind takes every step between, in order, and
    /// none of the ones it has already taken
    #[test]
    fn a_skipped_version_is_not_skipped() {
        let steps = [
            Step { to: "0.9.0", apply: bump },
            Step { to: "0.10.0", apply: bump },
            Step { to: "0.11.0", apply: bump },
        ];
        let mut doc = serde_json::json!({"n": 0, "kept": "as written"});
        let (at, err) = run_steps(&mut doc, "0.8.0", &steps);
        assert_eq!((at.as_str(), err), ("0.11.0", None));
        assert_eq!(doc["n"], 3, "0.8 から 0.11 へは3段");
        assert_eq!(doc["kept"], "as written", "知らないキーが消えた");

        let mut doc = serde_json::json!({"n": 0});
        let (at, _) = run_steps(&mut doc, "0.10.0", &steps);
        assert_eq!((at.as_str(), doc["n"].as_u64()), ("0.11.0", Some(1)), "済んだ段をもう一度やった");
    }

    /// A failing step stops the walk where it is, and says so
    #[test]
    fn a_failing_step_stops_the_walk_where_it_is() {
        let steps = [
            Step { to: "0.9.0", apply: bump },
            Step { to: "0.10.0", apply: fail },
            Step { to: "0.11.0", apply: bump },
        ];
        let mut doc = serde_json::json!({"n": 0});
        let (at, err) = run_steps(&mut doc, "0.8.0", &steps);
        assert_eq!(at, "0.9.0", "失敗した段の手前で止まる");
        assert!(err.as_deref().is_some_and(|e| e.contains("0.9.0 -> 0.10.0")), "{err:?}");
        assert_eq!(doc["n"], 1, "失敗した段より先を当てた");
    }

    /// The real steps, over every fixture, reach the present and stay a
    /// settings file the present can read. Running them twice changes nothing
    #[test]
    fn every_fixture_walks_to_the_present_and_is_idempotent() {
        let dir = crate::repo_root().join("tests").join("fixtures");
        let mut seen = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(ver) = name.strip_prefix("config-").and_then(|s| s.strip_suffix(".json")) else { continue };
            seen += 1;
            let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
            let (at, err) = run_steps(&mut doc, ver, STEPS);
            assert_eq!(err, None, "{name}: {err:?}");
            let last = STEPS.last().map(|s| s.to).unwrap_or(ver);
            assert!(!crate::update::is_newer(last, &at), "{name} は {at} で止まった");
            let once = doc.clone();
            let (_, err) = run_steps(&mut doc, &at, STEPS);
            assert_eq!(err, None);
            assert_eq!(doc, once, "{name}: 二度目で変わった");
            let _: crate::config::Config = serde_json::from_value(doc).expect("移行後に読めない");
        }
        assert!(seen >= 1, "fixture が1つも無い");
    }

    /// The rename arrives without anybody losing what they had written.
    ///
    /// A settings file written before it is a file whose desks are filed under
    /// the old word. Read as it stands, the app comes up with none of them and
    /// says nothing, so the step has to move them -- and move the paths of the
    /// definition files too, since those named a folder that has also been
    /// renamed
    #[test]
    fn the_older_word_for_a_desk_is_carried_over() {
        let mut doc = serde_json::json!({
            "//workspaces": "what it was for",
            "workspaces": [
                {"name": "A", "file": "workspaces/projectx.json"},
                {"name": "B", "folders": []}
            ]
        });
        to_0_10_0(&mut doc).unwrap();
        assert!(doc.get("workspaces").is_none(), "古い呼び名が残っている");
        assert_eq!(doc["desks"][0]["file"], "desks/projectx.json");
        assert_eq!(doc["desks"][1]["name"], "B");
        assert_eq!(doc["//desks"], "what it was for");
        // ...and a file already in the new shape is left exactly as it is
        let once = doc.clone();
        to_0_10_0(&mut doc).unwrap();
        assert_eq!(doc, once);
    }

    /// A layout with settings and no stamp is backed up before anything
    /// else, the stamp is written, and the second start does nothing
    #[test]
    fn the_first_start_of_a_version_backs_up_first() {
        let root = std::env::temp_dir().join(format!("shikisha-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("desks")).unwrap();
        std::fs::write(root.join("config/config.json"), r#"{"n": 0, "mine": true}"#).unwrap();
        std::fs::write(root.join("config/secrets.json"), r#"{"s": 1}"#).unwrap();
        std::fs::write(root.join("desks/p.json"), r#"{"n": 0}"#).unwrap();
        let steps = [Step { to: "0.9.0", apply: bump }];
        let out = on_start_at(&root, "0.9.0", &steps);
        assert_eq!(out.from.as_deref(), Some(BASELINE), "刻印が無ければ基準の版");
        assert_eq!(out.failed, None);
        let backup = out.backup.expect("バックアップが無い");
        assert_eq!(std::fs::read_to_string(backup.join("config/config.json")).unwrap(), r#"{"n": 0, "mine": true}"#);
        assert!(backup.join("config/secrets.json").is_file());
        assert!(backup.join("desks/p.json").is_file());
        let cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("config/config.json")).unwrap()).unwrap();
        assert_eq!((cfg["n"].as_u64(), cfg["mine"].as_bool()), (Some(1), Some(true)));
        assert_eq!(std::fs::read_to_string(root.join("config/secrets.json")).unwrap(), r#"{"s": 1}"#, "secrets に触った");
        assert_eq!(recorded_at(&root).as_deref(), Some("0.9.0"));
        assert_eq!(on_start_at(&root, "0.9.0", &steps), Outcome::default(), "二度目に何かした");
        // A layout from before the stamp, already at the baseline, is stamped
        let _ = std::fs::remove_file(version_path(&root));
        assert_eq!(on_start_at(&root, BASELINE, &steps), Outcome::default());
        assert_eq!(recorded_at(&root).as_deref(), Some(BASELINE), "刻印が付かない");

        // A failing step: the file stays as the last good step left it, the
        // stamp does not reach the present, and the person is told
        let steps = [Step { to: "0.9.0", apply: bump }, Step { to: "0.10.0", apply: fail }];
        let out = on_start_at(&root, "0.10.0", &steps);
        assert!(out.failed.is_some());
        assert_eq!(recorded_at(&root).as_deref(), Some("0.9.0"), "失敗したのに版が進んだ");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The five newest backups stay; the rest go
    #[test]
    fn only_the_newest_backups_are_kept() {
        let dir = std::env::temp_dir().join(format!("shikisha-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for i in 0..8 {
            std::fs::create_dir_all(dir.join(format!("0.8.0-20260909-00000{i}"))).unwrap();
        }
        prune_backups(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left.len(), KEEP);
        assert_eq!(left[0], "0.8.0-20260909-000003", "古い方が残った");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
