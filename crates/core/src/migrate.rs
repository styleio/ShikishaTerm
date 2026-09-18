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

/// One reshaping: what version the file is in afterwards, and how.
///
/// A step must leave a file already in its shape exactly as it is. A version
/// number is not one build: between one release and the next, every build
/// carries the same number, so a file stamped with that number may have been
/// written by a build from before the step existed. The step for the recorded
/// version is therefore run again, and only a file it actually changes is
/// backed up and written
pub struct Step {
    pub to: &'static str,
    pub apply: fn(&mut serde_json::Value) -> Result<()>,
}

/// Every reshaping there has ever been, oldest first. A version that changes
/// the shape of a settings file adds one line here, and a fixture of a real
/// file from the version before it under `tests/fixtures/`, so the test that
/// walks every fixture to the present keeps walking.
const STEPS: &[Step] =
    &[Step { to: "0.10.0", apply: to_0_10_0 }, Step { to: "0.16.0", apply: to_0_16_0 }];

/// A group that runs programs says which folder it runs them in.
///
/// A group with no folder written in it used to mean "wherever the app itself
/// is", which was never a place anybody chose: it was the folder the app
/// happened to be started from, so the same tab worked in one place from the
/// shortcut and another from a script, and no screen ever said which. Tabs
/// like that are now held rather than started, which would stop a setup that
/// has been working for months -- so the folder those tabs were already using
/// is written down here, before the version that holds them ever reads the
/// file. Nothing moves; what was implied becomes visible, and can be changed.
///
/// `"."` rather than a full path, because settings travel between machines:
/// a relative folder is resolved against the app's own on whichever machine
/// reads it, which is exactly what the old empty field meant.
///
/// Only groups that actually start something here are touched. A group of
/// pages, file panels or terminals on other machines needs no folder of ours,
/// and giving it one would put a folder on the screen that nobody asked for
fn to_0_16_0(doc: &mut serde_json::Value) -> Result<()> {
    match doc.get_mut("desks").and_then(|d| d.as_array_mut()) {
        // The settings file: every desk written inside it
        Some(desks) => desks.iter_mut().for_each(name_the_folder_in_use),
        // A desk file, or the shape from before desks existed: the tabs and
        // groups are the document's own
        None => name_the_folder_in_use(doc),
    }
    Ok(())
}

/// The folder a desk's group-less tabs were already running in, written into
/// the group they run in. Nothing to write when nothing there runs a program
fn name_the_folder_in_use(holder: &mut serde_json::Value) {
    let Some(obj) = holder.as_object() else { return };
    let runs = |v: Option<&serde_json::Value>| {
        v.and_then(|t| t.as_array()).is_some_and(|tabs| tabs.iter().any(starts_a_program))
    };
    let named = |g: &serde_json::Value| {
        g.get("cwd").and_then(|c| c.as_str()).is_some_and(|c| !c.trim().is_empty())
    };
    let folders = obj.get("folders").and_then(|f| f.as_array());
    // Tabs written beside the groups rather than inside one land in the first
    // group, whether that group is already there or has to be made
    let first_named = folders.and_then(|f| f.first()).is_some_and(named);
    let legacy = runs(obj.get("tabs")) && !first_named;
    let homeless = folders.into_iter().flatten().any(|g| !named(g) && runs(g.get("tabs")));
    if !legacy && !homeless {
        return;
    }
    crate::config::ensure_folders(holder);
    let Some(folders) = holder.get_mut("folders").and_then(|f| f.as_array_mut()) else { return };
    for g in folders.iter_mut() {
        if !named(g) && runs(g.get("tabs")) {
            g["cwd"] = serde_json::json!(".");
        }
    }
}

/// Whether this tab, or one nested under it, starts a program on this PC.
///
/// A page, a git panel, a file panel and the editor are drawn by the app; a
/// terminal on another machine and a conversation with a model run nothing
/// here either. Everything else is a program, and a program runs in a folder
fn starts_a_program(tab: &serde_json::Value) -> bool {
    let argv = tab
        .get("command")
        .cloned()
        .and_then(|c| serde_json::from_value::<crate::config::CommandSpec>(c).ok())
        .map(|c| c.argv())
        .unwrap_or_default();
    let here = !argv.is_empty()
        && !crate::config::is_app_panel(&argv)
        && crate::config::ssh_endpoint(&argv).is_none()
        && !crate::bridge::is_model_line(&argv);
    here || tab.get("children").and_then(|c| c.as_array()).is_some_and(|c| c.iter().any(starts_a_program))
}

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
    rename_key(root, "workspaces", "desks");
    rename_key(root, "//workspaces", "//desks");
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

/// Gives a key a new name where it stands. A settings file is somebody's own
/// document and the order they wrote it in is part of it -- a note sits above
/// the thing it describes -- so the key keeps its place rather than being
/// taken out and put back at the end. When the new name is already there, the
/// old key is dropped and the new one is left as it is
fn rename_key(map: &mut serde_json::Map<String, serde_json::Value>, old: &str, new: &str) {
    if !map.contains_key(old) {
        return;
    }
    let keep_new = map.contains_key(new);
    *map = std::mem::take(map)
        .into_iter()
        .filter_map(|(k, v)| match k.as_str() {
            k if k == old && keep_new => None,
            k if k == old => Some((new.to_string(), v)),
            _ => Some((k, v)),
        })
        .collect();
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

/// Runs every step for `from` and later over the document, in order. Returns
/// the version the document is in afterwards, and the error of the step that
/// failed, if one did. The document is left as the last good step left it.
///
/// The steps for `from` itself are included: see [`Step`] for why a file
/// stamped with a version can still be waiting for that version's step
pub fn run_steps(doc: &mut serde_json::Value, from: &str, steps: &[Step]) -> (String, Option<String>) {
    let mut at = from.to_string();
    for s in steps {
        if crate::update::is_newer(&at, s.to) {
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

/// Everything that has to happen to a person's files before anything reads
/// them, whichever program is starting: the window and the server with no
/// window both call this, so a settings file is carried forward the same way
/// wherever it is opened
pub fn prepare() {
    crate::config::migrate_legacy_config();
    crate::update::set_outcome(on_start());
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
    if !has_settings {
        if from == current {
            return Outcome::default();
        }
        record(root, current);
        return Outcome { from: Some(from), ..Default::default() };
    }
    // What the steps would make of each file, worked out before anything is
    // touched. The settings file and every desk file alike: a desk file is
    // the same shape as a desk inside the settings
    let mut changed = Vec::new();
    let mut stopped: Option<(String, String)> = None;
    for f in owned_files(root) {
        if f.file_name().is_some_and(|n| n == "secrets.json") {
            continue;
        }
        let Some(mut doc) = read_doc(&f) else { continue };
        let before = doc.clone();
        let (at, err) = run_steps(&mut doc, &from, steps);
        if doc != before {
            changed.push((f, doc));
        }
        if let Some(e) = err {
            stopped = Some((at, e));
            break;
        }
    }
    if from == current && changed.is_empty() && stopped.is_none() {
        // Already this version and already in its shape; a layout from
        // before the stamp gets one now
        if recorded_at(root).is_none() {
            record(root, current);
        }
        return Outcome::default();
    }
    let mut out = Outcome { from: Some(from.clone()), ..Default::default() };
    match back_up(root, &from) {
        Ok(p) => out.backup = p,
        Err(e) => {
            // Not safe to change anything. Try again next start
            out.failed = Some(format!("backup: {e}"));
            crate::append_hook_log(&format!("Update from {from} to {current}: no backup could be made, files left as they are: {e}"));
            return out;
        }
    }
    let mut reached = current.to_string();
    for (f, doc) in &changed {
        if let Ok(text) = serde_json::to_string_pretty(doc)
            && let Err(e) = crate::crypto::write_atomic(f, &text)
        {
            out.failed = Some(format!("write {}: {e}", f.display()));
            break;
        }
    }
    if let Some((at, e)) = stopped
        && out.failed.is_none()
    {
        out.failed = Some(e);
        if crate::update::is_newer(&reached, &at) {
            reached = at;
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

    // Test steps keep the promise real steps make: a file already in the
    // shape a step gives is left exactly as it is
    fn to_09(doc: &mut serde_json::Value) -> Result<()> {
        doc["v09"] = serde_json::json!(true);
        Ok(())
    }
    fn to_10(doc: &mut serde_json::Value) -> Result<()> {
        doc["v10"] = serde_json::json!(true);
        Ok(())
    }
    fn to_11(doc: &mut serde_json::Value) -> Result<()> {
        doc["v11"] = serde_json::json!(true);
        Ok(())
    }
    fn fail(_: &mut serde_json::Value) -> Result<()> {
        anyhow::bail!("no")
    }
    fn marks(doc: &serde_json::Value) -> Vec<&str> {
        ["v09", "v10", "v11"].into_iter().filter(|k| doc.get(*k).is_some()).collect()
    }

    /// A file four versions behind takes every step between, in order, and
    /// none from before the version it is stamped with
    #[test]
    fn a_skipped_version_is_not_skipped() {
        let steps = [
            Step { to: "0.9.0", apply: to_09 },
            Step { to: "0.10.0", apply: to_10 },
            Step { to: "0.11.0", apply: to_11 },
        ];
        let mut doc = serde_json::json!({"kept": "as written"});
        let (at, err) = run_steps(&mut doc, "0.8.0", &steps);
        assert_eq!((at.as_str(), err), ("0.11.0", None));
        assert_eq!(marks(&doc), ["v09", "v10", "v11"], "three steps from 0.8 to 0.11");
        assert_eq!(doc["kept"], "as written", "an unknown key disappeared");

        let mut doc = serde_json::json!({});
        let (at, _) = run_steps(&mut doc, "0.10.0", &steps);
        assert_eq!(at, "0.11.0");
        assert_eq!(marks(&doc), ["v10", "v11"], "it applied a step older than the stamp, or skipped the step for the stamped version");
    }

    /// A failing step stops the walk where it is, and says so
    #[test]
    fn a_failing_step_stops_the_walk_where_it_is() {
        let steps = [
            Step { to: "0.9.0", apply: to_09 },
            Step { to: "0.10.0", apply: fail },
            Step { to: "0.11.0", apply: to_11 },
        ];
        let mut doc = serde_json::json!({});
        let (at, err) = run_steps(&mut doc, "0.8.0", &steps);
        assert_eq!(at, "0.9.0", "it stops just before the step that failed");
        assert!(err.as_deref().is_some_and(|e| e.contains("0.9.0 -> 0.10.0")), "{err:?}");
        assert_eq!(marks(&doc), ["v09"], "it applied steps past the one that failed");
    }

    fn scratch_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("shikisha-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).unwrap();
        root
    }

    /// A settings file stamped with the version that is running can still be
    /// waiting for that version's step.
    ///
    /// The desk rename landed after 0.10.0 was released and kept the number,
    /// so everybody who had run 0.10.0 had `data/version` saying 0.10.0 and a
    /// settings file with the old key. Skipping on "already this version"
    /// brought the app up with no desks at all
    #[test]
    fn a_step_added_within_a_version_still_reaches_files_stamped_with_it() {
        let root = scratch_root("same-version");
        std::fs::write(
            root.join("config/config.json"),
            r#"{"language": "ja", "//workspaces": "note", "workspaces": [{"name": "Work", "folders": []}], "remote": {"port": 8787, "enabled": true}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        record(&root, "0.10.0");

        let out = on_start_at(&root, "0.10.0", STEPS);
        assert_eq!(out.failed, None);
        let backup = out.backup.expect("a backup is needed before rewriting");
        assert!(
            std::fs::read_to_string(backup.join("config/config.json")).unwrap().contains("workspaces"),
            "the backup is not the contents from before migrating"
        );
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("config/config.json")).unwrap()).unwrap();
        assert_eq!(cfg["desks"][0]["name"], "Work", "the desks were not carried over");
        assert!(cfg.get("workspaces").is_none());
        // Written back in the order the person wrote it, the renamed key in its old place
        let keys: Vec<&str> = cfg.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["language", "//desks", "desks", "remote"], "the order of the keys changed");
        let inner: Vec<&str> = cfg["remote"].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(inner, ["port", "enabled"], "the order of nested keys changed");

        // The next start finds nothing to do and makes no second copy
        assert_eq!(on_start_at(&root, "0.10.0", STEPS), Outcome::default(), "it did something the second time");
        let _ = std::fs::remove_dir_all(&root);
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
            assert!(!crate::update::is_newer(last, &at), "{name} stopped at {at}");
            let once = doc.clone();
            let (_, err) = run_steps(&mut doc, &at, STEPS);
            assert_eq!(err, None);
            assert_eq!(doc, once, "{name}: it changed the second time");
            let _: crate::config::Config = serde_json::from_value(doc).expect("it cannot be read after migrating");
        }
        assert!(seen >= 1, "there is not a single fixture");
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
        assert!(doc.get("workspaces").is_none(), "the old name is still there");
        assert_eq!(doc["desks"][0]["file"], "desks/projectx.json");
        assert_eq!(doc["desks"][1]["name"], "B");
        assert_eq!(doc["//desks"], "what it was for");
        // ...and a file already in the new shape is left exactly as it is
        let once = doc.clone();
        to_0_10_0(&mut doc).unwrap();
        assert_eq!(doc, once);
    }

    /// The folder those tabs were already working in is written down, so that
    /// the version which holds folderless terminals does not stop a setup that
    /// has been running for months
    #[test]
    fn the_folder_a_group_was_already_using_is_written_down() {
        let mut doc = serde_json::json!({"desks": [
            // A terminal with nowhere written: it was working beside the app,
            // and now says so
            {"name": "W", "folders": [
                {"tabs": [{"name": "aaa", "command": "claude --dangerously-skip-permissions"}]},
                {"cwd": "D:/work/proj", "tabs": [{"command": "powershell.exe"}]}
            ]},
            // Nothing here runs a program on this PC, so nothing needs a
            // folder here and none is invented
            {"name": "R", "folders": [{"tabs": [
                {"command": "browser https://example.com/"},
                {"command": "sftp://me@example.test:22"},
                {"command": "ssh://me@example.test:22"},
                {"command": "model acme/big"},
                {"command": "git"}
            ]}]},
            // The old shape: tabs beside the groups rather than inside one
            {"name": "C", "tabs": [{"command": "powershell.exe"}]}
        ]});
        to_0_16_0(&mut doc).unwrap();

        assert_eq!(doc["desks"][0]["folders"][0]["cwd"], ".", "the terminal has nowhere to work");
        assert_eq!(doc["desks"][0]["folders"][1]["cwd"], "D:/work/proj", "a folder somebody chose was rewritten");
        assert!(doc["desks"][1]["folders"][0].get("cwd").is_none(), "a desk of pages and panels was given a folder");
        assert_eq!(doc["desks"][2]["folders"][0]["cwd"], ".", "the old shape was left without a folder");
        assert_eq!(doc["desks"][2]["folders"][0]["tabs"][0]["command"], "powershell.exe", "the tab did not come with it");

        // ...and a file already carrying the answer is left exactly as it is
        let once = doc.clone();
        to_0_16_0(&mut doc).unwrap();
        assert_eq!(doc, once);
    }

    /// A desk file holds one desk, not a list of them, and is carried over
    /// the same way
    #[test]
    fn a_desk_of_its_own_file_is_carried_over_too() {
        let mut doc = serde_json::json!({
            "name": "W",
            "folders": [{"tabs": [{"command": "sh"}]}]
        });
        to_0_16_0(&mut doc).unwrap();
        assert_eq!(doc["folders"][0]["cwd"], ".");
    }

    /// A layout with settings and no stamp is backed up before anything
    /// else, the stamp is written, and the second start does nothing
    #[test]
    fn the_first_start_of_a_version_backs_up_first() {
        let root = scratch_root("migrate");
        std::fs::create_dir_all(root.join("desks")).unwrap();
        std::fs::write(root.join("config/config.json"), r#"{"mine": true}"#).unwrap();
        std::fs::write(root.join("config/secrets.json"), r#"{"s": 1}"#).unwrap();
        std::fs::write(root.join("desks/p.json"), r#"{}"#).unwrap();
        let steps = [Step { to: "0.9.0", apply: to_09 }];
        let out = on_start_at(&root, "0.9.0", &steps);
        assert_eq!(out.from.as_deref(), Some(BASELINE), "with no stamp, the baseline version");
        assert_eq!(out.failed, None);
        let backup = out.backup.expect("there is no backup");
        assert_eq!(std::fs::read_to_string(backup.join("config/config.json")).unwrap(), r#"{"mine": true}"#);
        assert!(backup.join("config/secrets.json").is_file());
        assert!(backup.join("desks/p.json").is_file());
        let cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("config/config.json")).unwrap()).unwrap();
        assert_eq!((marks(&cfg), cfg["mine"].as_bool()), (vec!["v09"], Some(true)));
        assert_eq!(std::fs::read_to_string(root.join("config/secrets.json")).unwrap(), r#"{"s": 1}"#, "it touched secrets");
        assert_eq!(recorded_at(&root).as_deref(), Some("0.9.0"));
        assert_eq!(on_start_at(&root, "0.9.0", &steps), Outcome::default(), "it did something the second time");
        // A layout from before the stamp, already at the baseline, is stamped
        let _ = std::fs::remove_file(version_path(&root));
        assert_eq!(on_start_at(&root, BASELINE, &steps), Outcome::default());
        assert_eq!(recorded_at(&root).as_deref(), Some(BASELINE), "no stamp is added");

        // A failing step: the file stays as the last good step left it, the
        // stamp does not reach the present, and the person is told
        let steps = [Step { to: "0.9.0", apply: to_09 }, Step { to: "0.10.0", apply: fail }];
        let out = on_start_at(&root, "0.10.0", &steps);
        assert!(out.failed.is_some());
        assert_eq!(recorded_at(&root).as_deref(), Some("0.9.0"), "the version moved on though it failed");
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
        assert_eq!(left[0], "0.8.0-20260909-000003", "the older one was kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
