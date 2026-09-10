//! Take a workspace out into a single file, and bring one back in.
//!
//! A workspace isn't just the row of tabs. The tabs point at automation
//! scripts that live elsewhere, and without those, what you took out won't
//! run. So settings and scripts go into one container.
//!
//! When bringing one in, if the destination is already occupied, use a
//! different name. If you bring in two workspaces that share a folder name
//! and the later one silently overwrites the earlier one's contents,
//! there's no way to notice.
//!
//! Not included: notification destinations, secrets.json, capabilities.
//! Those belong to the whole app's settings, not to any one workspace, and
//! shipping them around would mean handing out credentials. The workspace's
//! own allow-list of secrets is written out (it says what the workspace
//! needs) but not read in: a file somebody else wrote does not get to grant
//! itself this machine's secrets. The person re-grants them on the settings
//! screen, knowing what they are handing over

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Format version written. An unreadable version is refused. Better than
/// silently importing only part of it.
///
/// 1: the tabs sat directly on the workspace.
/// 2: the tabs sit inside working folders (`folders[].tabs`), the shape the
///    settings screen has written since working folders arrived. A program
///    that only knows 1 refuses this rather than importing an empty workspace
const FORMAT: u64 = 2;
/// Versions this program can read. An older file is folded into the current
/// shape on the way in, the same way the config loader folds an older config
const READABLE: std::ops::RangeInclusive<u64> = 1..=FORMAT;
/// Number of scripts that fit in one container
const MAX_FILES: usize = 500;
/// Size of one container
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Number of times to search for an alternate name. If it's still taken by
/// then, something else is going on
const MAX_RENAME: u32 = 100;

/// Result of an import. Returned so the user can be shown what landed where
pub struct Placed {
    /// The name actually assigned (changed if it collided)
    pub name: String,
    /// Where scripts were placed (only the ones whose name changed from the original)
    pub moved: Vec<(String, String)>,
    /// Number of scripts written
    pub files: usize,
}

/// The folder every relative path in the config is based on: the one the
/// `config`, `scripts` and `workspaces` folders sit in together (see the
/// folder charter in RULES). The config file lives in its own folder there,
/// or directly there in the old layout; either way this is where
/// `scripts/ws1` is read from when a tab runs it. Based on the config's own
/// folder, this used to look for every script one level too deep, and
/// exported none of them without a word
fn base_of(config_path: &Path) -> Result<&Path> {
    let dir = config_path
        .parent()
        .ok_or_else(|| anyhow!(crate::i18n::t("err.wspack.no_config_dir")))?;
    Ok(match (dir.file_name().and_then(|n| n.to_str()), dir.parent()) {
        (Some("config"), Some(root)) => root,
        _ => dir,
    })
}

/// Checks that a path stays inside the settings folder.
///
/// Imported files were written by someone else, so the destination is
/// always decided here. Absolute paths, going up to a parent, and drive
/// specifiers are all rejected
fn under_base(base: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return None;
    }
    if p.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    Some(base.join(p))
}

/// The automation reference in a config entry. Prefers the new spelling, but also honors the old name
fn automation_of(v: &Value) -> Option<String> {
    for key in ["automation", "lua"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            let s = s.trim().replace('\\', "/");
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// Runs `f` over every tab of the workspace, wherever it was written: inside
/// a working folder, beside the folders the old way, or nested under another
/// tab. One walk shared by collecting and rewriting, so the two cannot
/// disagree about which tabs exist
fn each_tab_mut(ws: &mut Value, f: &mut dyn FnMut(&mut Value)) {
    fn walk(tabs: Option<&mut Value>, f: &mut dyn FnMut(&mut Value)) {
        for t in tabs.and_then(Value::as_array_mut).into_iter().flatten() {
            f(t);
            walk(t.get_mut("children"), f);
        }
    }
    walk(ws.get_mut("tabs"), f);
    for g in ws
        .get_mut("folders")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        walk(g.get_mut("tabs"), f);
    }
}

/// Collects every automation location referenced by the workspace and all its tabs
fn referenced(ws: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(a) = automation_of(ws) {
        out.push(a);
    }
    let mut ws = ws.clone();
    each_tab_mut(&mut ws, &mut |t| {
        if let Some(a) = automation_of(t) {
            out.push(a);
        }
    });
    out.sort();
    out.dedup();
    out
}

/// The actual units to take out. Keep only ones that exist, and leave
/// anything nested inside another folder to its parent
fn roots(base: &Path, refs: &[String]) -> Vec<String> {
    let alive: Vec<&String> = refs
        .iter()
        .filter(|r| under_base(base, r).is_some_and(|p| p.exists()))
        .collect();
    alive
        .iter()
        .filter(|r| {
            !alive
                .iter()
                .any(|o| o != *r && r.starts_with(&format!("{o}/")))
        })
        .map(|r| r.to_string())
        .collect()
}

/// Collects the .lua files inside a folder. The key is the location relative to the settings folder
fn read_dir_lua(base: &Path, rel: &str, out: &mut Map<String, Value>) -> Result<()> {
    let Some(dir) = under_base(base, rel) else {
        return Ok(());
    };
    let mut entries: Vec<_> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok()).collect();
    // Fix the ordering. If it changes every time we write, diffs become unreadable
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Some(name) = e.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let child = format!("{rel}/{name}");
        if e.path().is_dir() {
            read_dir_lua(base, &child, out)?;
        } else if name.to_ascii_lowercase().ends_with(".lua") {
            if out.len() >= MAX_FILES {
                bail!(crate::i18n::tp(
                    "err.wspack.too_many_scripts",
                    &[("max", &MAX_FILES.to_string())]
                ));
            }
            let code = std::fs::read_to_string(e.path())?;
            out.insert(child, Value::String(code));
        }
    }
    Ok(())
}

/// Whether a config entry holds anything for the key, or a value that stands
/// in for "not mentioned". Names the very cases in which the config loader
/// falls back to the definition file
fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Pulls `workspaces` out of the config.
/// Folders or tabs written directly, the way they were before workspaces
/// existed, are treated as a single workspace called DEFAULT -- the same
/// one the program launches from them
fn workspace_list(cfg: &Value) -> Vec<Value> {
    match cfg.get("workspaces").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a.clone(),
        _ => {
            let mut ws = json!({ "name": "DEFAULT" });
            for key in ["folders", "tabs"] {
                if let Some(v) = cfg.get(key).filter(|v| !is_blank(v)) {
                    ws[key] = v.clone();
                }
            }
            if ws.get("folders").is_none() && ws.get("tabs").is_none() {
                Vec::new()
            } else {
                vec![ws]
            }
        }
    }
}

/// Expands anything that was split out into a separate file, inline, and
/// settles the tabs into their working folders. Once taken out, it should be
/// self-contained, in one piece and in one shape
fn inline(base: &Path, entry: &Value) -> Result<Value> {
    let file = entry.get("file").and_then(Value::as_str);
    let mut ws = match file {
        Some(f) => {
            let p = under_base(base, &f.replace('\\', "/")).ok_or_else(|| {
                anyhow!(crate::i18n::tp("err.wspack.file_outside", &[("f", f)]))
            })?;
            let text = std::fs::read_to_string(&p).map_err(|e| {
                anyhow!(crate::i18n::tp(
                    "err.wspack.file_unreadable",
                    &[("f", f), ("e", &e.to_string())]
                ))
            })?;
            let body = serde_json::from_str::<Value>(&text).map_err(|e| {
                anyhow!(crate::i18n::tp(
                    "err.wspack.file_bad_json",
                    &[("f", f), ("e", &e.to_string())]
                ))
            })?;
            if body.is_object() { body } else { json!({}) }
        }
        None => json!({}),
    };

    // The config entry wins over the definition file, key by key, the way
    // the loader resolves them. What the entry leaves blank comes from the
    // file. The tabs are the file's alone once there is a file: the loader
    // never reads an entry's tabs beside a `file`, so neither does this
    for (k, v) in entry.as_object().into_iter().flatten() {
        let files_own = file.is_some() && matches!(k.as_str(), "folders" | "tabs");
        if k == "file" || files_own || is_blank(v) {
            continue;
        }
        ws[k] = v.clone();
    }
    if is_blank(ws.get("name").unwrap_or(&Value::Null)) {
        ws["name"] = Value::String("UNNAMED".into());
    }
    // One spelling on the way out. The old one is still read on the way in
    if let Some(lua) = ws.as_object_mut().and_then(|o| o.remove("lua")) {
        if ws.get("automation").map_or(true, is_blank) {
            ws["automation"] = lua;
        }
    }
    crate::config::ensure_folders(&mut ws);
    Ok(ws)
}

/// Bundles the workspace picked by index into the contents of a single
/// file. Returns (suggested file name, contents)
pub fn pack(config_path: &Path, index: usize) -> Result<(String, String)> {
    let base = base_of(config_path)?;
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(config_path)?)?;
    let list = workspace_list(&cfg);
    let entry = list
        .get(index)
        .ok_or_else(|| anyhow!(crate::i18n::t("err.wspack.no_such_workspace")))?;
    let ws = inline(base, entry)?;

    let refs = referenced(&ws);
    let keep = roots(base, &refs);
    let mut scripts = Map::new();
    for r in &keep {
        let Some(p) = under_base(base, r) else { continue };
        if p.is_dir() {
            read_dir_lua(base, r, &mut scripts)?;
        } else if r.to_ascii_lowercase().ends_with(".lua") {
            scripts.insert(r.clone(), Value::String(std::fs::read_to_string(&p)?));
        }
    }

    let name = ws
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("workspace")
        .to_string();
    let bundle = json!({
        "shikisha_workspace": FORMAT,
        "exported_at": crate::hooks::local_stamp("%Y-%m-%d %H:%M:%S"),
        "workspace": ws,
        // The unit to re-place. If this is already taken, use a different name
        "roots": keep,
        "scripts": Value::Object(scripts),
    });
    Ok((format!("{}.stws.json", safe_file_name(&name)), serde_json::to_string_pretty(&bundle)?))
}

/// Strips characters that can't be used in a file name. Falls back to the default name if it becomes empty
fn safe_file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "workspace".into() } else { s }
}

/// Finds a name that isn't taken. If `scripts/ws2` is taken, tries `scripts/ws2-2`
fn free_name(base: &Path, rel: &str) -> Result<String> {
    if under_base(base, rel).is_some_and(|p| !p.exists()) {
        return Ok(rel.to_string());
    }
    // If there's an extension, insert before it (ws2.lua-2 wouldn't be readable)
    let (stem, ext) = match rel.rsplit_once('.') {
        Some((s, e)) if !e.contains('/') && !s.is_empty() => (s, format!(".{e}")),
        _ => (rel, String::new()),
    };
    for n in 2..=MAX_RENAME {
        let cand = format!("{stem}-{n}{ext}");
        if under_base(base, &cand).is_some_and(|p| !p.exists()) {
            return Ok(cand);
        }
    }
    bail!(crate::i18n::tp("err.wspack.cannot_name", &[("rel", rel)]))
}

/// Rewrites prefixes. Renaming `scripts/ws2` to `scripts/ws2-2` also moves
/// along anything pointing inside it, like `scripts/ws2/html`
fn remap(path: &str, moves: &[(String, String)]) -> String {
    for (from, to) in moves {
        if path == from {
            return to.clone();
        }
        if let Some(rest) = path.strip_prefix(&format!("{from}/")) {
            return format!("{to}/{rest}");
        }
    }
    path.to_string()
}

/// Rewrites the automation locations referenced by the workspace and its tabs to wherever they were placed
fn rewrite(ws: &mut Value, moves: &[(String, String)]) {
    fn one(v: &mut Value, moves: &[(String, String)]) {
        for key in ["automation", "lua"] {
            let Some(s) = v.get(key).and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            let to = remap(&s.replace('\\', "/"), moves);
            v[key] = Value::String(to);
        }
    }
    one(ws, moves);
    each_tab_mut(ws, &mut |t| one(t, moves));
}

/// Picks a display name that isn't already used. If the same name appears
/// twice, there's no way to tell which one the tab bar or a script is pointing at
fn free_title(list: &[Value], want: &str) -> String {
    free_field(list, "name", want)
}

/// The automation name to file this one under.
///
/// It travels in the bundle, so a workspace carried to another machine keeps
/// the name its secrets are stored beside -- bring the secrets file too and
/// nothing has to be typed again. Two copies of the same bundle on one machine
/// are two different workspaces, though, and they cannot share a name: the
/// second one takes `-2`, and is asked for its passwords when it first needs
/// them
fn free_ws_id(list: &[Value], want: &str) -> String {
    free_field(list, "id", want)
}

fn free_field(list: &[Value], field: &str, want: &str) -> String {
    let taken = |n: &str| {
        list.iter()
            .any(|w| w.get(field).and_then(Value::as_str) == Some(n))
    };
    if !taken(want) {
        return want.to_string();
    }
    for n in 2..=MAX_RENAME {
        let cand = format!("{want}-{n}");
        if !taken(&cand) {
            return cand;
        }
    }
    want.to_string()
}

/// Imports a file that was taken out. Adds one workspace to the config, and
/// places its scripts wherever there's room
pub fn unpack(config_path: &Path, text: &str) -> Result<Placed> {
    if text.len() > MAX_BYTES {
        bail!(crate::i18n::t("err.wspack.file_too_big"));
    }
    let base = base_of(config_path)?;
    let bundle: Value = serde_json::from_str(text).map_err(|e| {
        anyhow!(crate::i18n::tp(
            "err.wspack.cannot_read",
            &[("e", &e.to_string())]
        ))
    })?;
    match bundle.get("shikisha_workspace").and_then(Value::as_u64) {
        Some(v) if READABLE.contains(&v) => {}
        Some(v) => bail!(crate::i18n::tp(
            "err.wspack.bad_version",
            &[("v", &v.to_string())]
        )),
        None => bail!(crate::i18n::t("err.wspack.not_a_workspace_file")),
    }
    let mut ws = bundle
        .get("workspace")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| anyhow!(crate::i18n::t("err.wspack.empty_content")))?;
    // A file from before working folders (format 1) has its tabs beside the
    // folders. They go into the first folder, as the loader would put them
    crate::config::ensure_folders(&mut ws);
    // What this machine's secrets the workspace may read is this machine's
    // answer, not the file's (see the module doc)
    if let Some(o) = ws.as_object_mut() {
        o.remove("secrets_allow");
        o.remove("secrets_allow_all");
    }

    let scripts = bundle
        .get("scripts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if scripts.len() > MAX_FILES {
        bail!(crate::i18n::tp(
            "err.wspack.too_many_scripts",
            &[("max", &MAX_FILES.to_string())]
        ));
    }

    // Decide the destinations first. If even one can't be placed, write nothing
    let mut moves = Vec::new();
    for r in bundle
        .get("roots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let r = r.replace('\\', "/");
        if under_base(base, &r).is_none() {
            bail!(crate::i18n::tp("err.wspack.outside_folder", &[("path", &r)]));
        }
        let to = free_name(base, &r)?;
        if to != r {
            moves.push((r, to));
        }
    }

    let mut writes: Vec<(PathBuf, String)> = Vec::new();
    for (path, code) in &scripts {
        let path = remap(&path.replace('\\', "/"), &moves);
        if !path.to_ascii_lowercase().ends_with(".lua") {
            bail!(crate::i18n::tp("err.wspack.non_lua", &[("path", &path)]));
        }
        let dest = under_base(base, &path).ok_or_else(|| {
            anyhow!(crate::i18n::tp("err.wspack.outside_folder", &[("path", &path)]))
        })?;
        if dest.exists() {
            bail!(crate::i18n::tp("err.wspack.would_overwrite", &[("path", &path)]));
        }
        let code = code.as_str().ok_or_else(|| {
            anyhow!(crate::i18n::tp("err.wspack.not_a_string", &[("path", &path)]))
        })?;
        writes.push((dest, code.to_string()));
    }

    for (dest, code) in &writes {
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::crypto::write_atomic(dest, code)?;
    }
    rewrite(&mut ws, &moves);

    let mut cfg: Value = serde_json::from_str(&std::fs::read_to_string(config_path)?)?;
    let mut list = workspace_list(&cfg);
    let want = ws.get("name").and_then(Value::as_str).unwrap_or("UNNAMED");
    let name = free_title(&list, want);
    ws["name"] = Value::String(name.clone());
    // What automation and the secret store call it. Written down here rather
    // than left for the loader, so that the answer does not change from one
    // launch to the next
    let want_id = match ws.get("id").and_then(Value::as_str).map(str::trim) {
        Some(w) if !w.is_empty() => w.to_string(),
        _ => match crate::config::slug_id(&name) {
            s if s.is_empty() => "workspace".into(),
            s => s,
        },
    };
    ws["id"] = Value::String(free_ws_id(&list, &want_id));
    list.push(ws);
    cfg["workspaces"] = Value::Array(list);
    // Now that it's moved into workspaces, the folders and tabs written
    // directly would be dead weight: the program and the settings screen
    // both stop reading them the moment a workspace list exists
    if let Some(o) = cfg.as_object_mut() {
        o.remove("tabs");
        o.remove("folders");
    }
    crate::crypto::write_atomic(config_path, &serde_json::to_string_pretty(&cfg)?)?;

    Ok(Placed {
        name,
        moved: moves,
        files: writes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a set of config files for testing, in the shape the settings
    /// screen writes: the tabs inside working folders, one of them nested
    fn setup(dir: &Path) -> PathBuf {
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "workspaces": [{
                    "name": "編集部",
                    "id": "henshu",
                    "automation": "scripts/ws1",
                    "secrets_allow": ["github"],
                    "folders": [
                        {"cwd": "D:/somewhere", "tabs": [
                            {"name": "AI", "command": "claude"},
                            {"name": "html", "command": "browser https://example.com",
                             "automation": "scripts/ws1/html"}
                        ]},
                        {"name": "second", "tabs": [
                            {"name": "outer", "command": "claude", "children": [
                                {"name": "inner", "command": "codex",
                                 "automation": "scripts/inner.lua"}
                            ]}
                        ]}
                    ]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("scripts/ws1/html")).unwrap();
        std::fs::write(dir.join("scripts/ws1/on_start.lua"), "-- はじめ").unwrap();
        std::fs::write(dir.join("scripts/ws1/html/on_load.lua"), "-- よみこみ").unwrap();
        std::fs::write(dir.join("scripts/inner.lua"), "-- うち").unwrap();
        cfg
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-wspack-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read_cfg(cfg: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(cfg).unwrap()).unwrap()
    }

    /// Confirms scripts travel along with the workspace, wherever its tabs
    /// were written. Passing only the config, without what it points at,
    /// leaves it dead on the other end
    #[test]
    fn a_workspace_travels_with_its_scripts() {
        let d = tmp("pack");
        let cfg = setup(&d);
        let (name, text) = pack(&cfg, 0).unwrap();
        assert_eq!(name, "編集部.stws.json");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["shikisha_workspace"], json!(FORMAT));
        let s = v["scripts"].as_object().unwrap();
        assert_eq!(
            s["scripts/ws1/on_start.lua"], "-- はじめ",
            "ワークスペースのスクリプトが入っていない"
        );
        assert_eq!(
            s["scripts/ws1/html/on_load.lua"], "-- よみこみ",
            "作業フォルダの中のタブのスクリプトが入っていない"
        );
        assert_eq!(
            s["scripts/inner.lua"], "-- うち",
            "入れ子のタブのスクリプトが入っていない"
        );
        // The re-place unit is only the parent. Nested folders travel with their parent
        assert_eq!(v["roots"], json!(["scripts/inner.lua", "scripts/ws1"]));
        // The tabs are where the program reads them from
        let ws = &v["workspace"];
        assert_eq!(ws["folders"][0]["tabs"][1]["name"], "html");
        assert_eq!(ws["folders"][1]["tabs"][0]["children"][0]["name"], "inner");
        assert!(ws.get("tabs").is_none(), "タブが作業フォルダの外にも書かれている");
        // What the workspace needs is written down, so the recipient knows
        assert_eq!(ws["secrets_allow"], json!(["github"]));
    }

    /// Confirms that importing the same thing twice doesn't break the first
    /// copy. Overwriting it would mean nobody notices it got wiped
    #[test]
    fn a_second_copy_does_not_overwrite_the_first() {
        let d = tmp("twice");
        let cfg = setup(&d);
        let (_, text) = pack(&cfg, 0).unwrap();

        let first = unpack(&cfg, &text).unwrap();
        assert_eq!(first.name, "編集部-2", "名前が重なったまま");
        assert!(!first.moved.is_empty(), "置き場所が重なったまま");
        assert_eq!(first.files, 3);

        let second = unpack(&cfg, &text).unwrap();
        assert_eq!(second.name, "編集部-3");

        // Two copies of one bundle are two workspaces, and each is filed under
        // a name of its own -- otherwise one copy's secrets would answer for the
        // other's. The name is written down now, not guessed at every launch
        let v = read_cfg(&cfg);
        let ids: Vec<&str> = v["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["id"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(ids, ["henshu", "henshu-2", "henshu-3"], "取り込んだ写しが同じ呼び名を名乗っている");

        // The original scripts are untouched
        assert_eq!(
            std::fs::read_to_string(d.join("scripts/ws1/on_start.lua")).unwrap(),
            "-- はじめ"
        );
        assert_eq!(std::fs::read_to_string(d.join("scripts/inner.lua")).unwrap(), "-- うち");
        // Each import lands in its own separate location
        assert!(d.join("scripts/ws1-2/on_start.lua").exists());
        assert!(d.join("scripts/ws1-3/on_start.lua").exists());
        assert!(d.join("scripts/ws1-2/html/on_load.lua").exists());
        assert!(d.join("scripts/inner-2.lua").exists(), "拡張子の前に番号が入っていない");
        assert!(d.join("scripts/inner-3.lua").exists());
    }

    /// Confirms that when the destination changes, whatever points at it
    /// changes along with it -- the tab in a folder and the tab under a tab alike
    #[test]
    fn the_tabs_point_at_where_the_scripts_actually_landed() {
        let d = tmp("rewrite");
        let cfg = setup(&d);
        let (_, text) = pack(&cfg, 0).unwrap();
        unpack(&cfg, &text).unwrap();

        let v = read_cfg(&cfg);
        let added = &v["workspaces"][1];
        assert_eq!(added["automation"], "scripts/ws1-2");
        assert_eq!(
            added["folders"][0]["tabs"][1]["automation"], "scripts/ws1-2/html",
            "中のフォルダが元の場所を指したまま"
        );
        assert_eq!(
            added["folders"][1]["tabs"][0]["children"][0]["automation"], "scripts/inner-2.lua",
            "入れ子のタブが元の場所を指したまま"
        );
        // The first workspace is exactly as it was
        assert_eq!(v["workspaces"][0]["folders"][0]["tabs"][1]["automation"], "scripts/ws1/html");
    }

    /// Confirms nothing is written outside the settings folder.
    /// Imported files were written by someone else, and their contents can't be trusted
    #[test]
    fn an_import_cannot_write_outside_the_settings_folder() {
        let d = tmp("escape");
        let cfg = setup(&d);
        for path in [
            "../逃げた.lua",
            "scripts/../../逃げた.lua",
            "C:/windows/逃げた.lua",
        ] {
            let text = serde_json::to_string(&json!({
                "shikisha_workspace": FORMAT,
                "workspace": {"name": "わるいもの", "folders": []},
                "roots": [],
                "scripts": { path: "-- ここには書けない" },
            }))
            .unwrap();
            assert!(unpack(&cfg, &text).is_err(), "外へ書けてしまう: {path}");
        }
        assert!(!d.parent().unwrap().join("逃げた.lua").exists());
    }

    /// Confirms nothing but Lua is written. Import must not be a delivery vector for executables
    #[test]
    fn an_import_writes_nothing_but_lua() {
        let d = tmp("kind");
        let cfg = setup(&d);
        let text = serde_json::to_string(&json!({
            "shikisha_workspace": FORMAT,
            "workspace": {"name": "わるいもの", "folders": []},
            "roots": [],
            "scripts": { "scripts/x.cmd": "echo" },
        }))
        .unwrap();
        assert!(unpack(&cfg, &text).is_err());
        assert!(!d.join("scripts/x.cmd").exists());
    }

    /// Confirms an unknown format is refused. Never silently import only half of something unreadable
    #[test]
    fn an_unknown_format_is_refused() {
        let d = tmp("format");
        let cfg = setup(&d);
        for bad in [
            json!({"workspace": {}}),
            json!({"shikisha_workspace": 0, "workspace": {}}),
            json!({"shikisha_workspace": FORMAT + 1, "workspace": {}}),
        ] {
            assert!(unpack(&cfg, &bad.to_string()).is_err(), "読めてしまう: {bad}");
        }
    }

    /// Confirms a file written before working folders existed still comes in
    /// whole. Its tabs sat beside the folders; they go into the first folder,
    /// which is where the program has read them from ever since
    #[test]
    fn a_file_written_before_working_folders_still_comes_in() {
        let d = tmp("v1");
        let cfg = setup(&d);
        let text = serde_json::to_string(&json!({
            "shikisha_workspace": 1,
            "workspace": {
                "name": "むかし",
                "automation": "scripts/old",
                "tabs": [{"name": "AI", "command": "claude", "automation": "scripts/old/ai"}]
            },
            "roots": ["scripts/old"],
            "scripts": { "scripts/old/on_start.lua": "-- old" },
        }))
        .unwrap();
        let placed = unpack(&cfg, &text).unwrap();
        assert_eq!(placed.name, "むかし");
        assert!(d.join("scripts/old/on_start.lua").exists());

        let added = &read_cfg(&cfg)["workspaces"][1];
        assert_eq!(added["folders"][0]["tabs"][0]["name"], "AI");
        assert_eq!(added["folders"][0]["tabs"][0]["automation"], "scripts/old/ai");
        assert!(added.get("tabs").is_none(), "古い形のまま書かれている");
    }

    /// Confirms a file cannot grant itself this machine's secrets. What the
    /// workspace may read is decided here, by the person, after it arrives
    #[test]
    fn an_import_does_not_grant_itself_secrets() {
        let d = tmp("secrets");
        let cfg = setup(&d);
        let (_, text) = pack(&cfg, 0).unwrap();
        let mut bundle: Value = serde_json::from_str(&text).unwrap();
        bundle["workspace"]["secrets_allow_all"] = json!(true);
        unpack(&cfg, &bundle.to_string()).unwrap();

        let added = &read_cfg(&cfg)["workspaces"][1];
        assert!(added.get("secrets_allow").is_none(), "許可リストが持ち込まれた");
        assert!(added.get("secrets_allow_all").is_none(), "全許可が持ち込まれた");
    }

    /// Confirms tabs written beside the folders, the old way, travel in the
    /// first folder along with their scripts -- as the program runs them
    #[test]
    fn tabs_written_beside_the_folders_travel_in_the_first_folder() {
        let d = tmp("beside");
        let cfg = d.join("config.json");
        std::fs::create_dir_all(d.join("scripts")).unwrap();
        std::fs::write(d.join("scripts/b.lua"), "-- b").unwrap();
        std::fs::write(
            &cfg,
            json!({"workspaces": [{
                "name": "混在",
                "folders": [{"name": "a", "tabs": [{"name": "A", "command": "claude"}]}],
                "tabs": [{"name": "B", "command": "claude", "automation": "scripts/b.lua"}]
            }]})
            .to_string(),
        )
        .unwrap();

        let (_, text) = pack(&cfg, 0).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        let names: Vec<_> = v["workspace"]["folders"][0]["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!(v["scripts"]["scripts/b.lua"], "-- b", "外に書かれたタブのスクリプトが入っていない");
        assert!(v["workspace"].get("tabs").is_none());
    }

    /// Confirms a config with folders and no workspace list still has
    /// something to take out: the DEFAULT workspace the program launches
    /// from them. Bringing one in then moves it into a workspace list
    #[test]
    fn a_config_without_workspaces_exports_its_folders() {
        let d = tmp("nows");
        let cfg = d.join("config.json");
        std::fs::write(
            &cfg,
            json!({"folders": [{"cwd": "D:/x", "tabs": [{"name": "AI", "command": "claude"}]}]})
                .to_string(),
        )
        .unwrap();

        let (name, text) = pack(&cfg, 0).unwrap();
        assert_eq!(name, "DEFAULT.stws.json");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["workspace"]["folders"][0]["tabs"][0]["name"], "AI");

        let placed = unpack(&cfg, &text).unwrap();
        assert_eq!(placed.name, "DEFAULT-2");
        let after = read_cfg(&cfg);
        assert_eq!(after["workspaces"].as_array().unwrap().len(), 2);
        assert_eq!(after["workspaces"][0]["folders"][0]["cwd"], "D:/x");
        assert!(after.get("folders").is_none(), "移した後も直書きの folders が残っている");
    }

    /// Confirms a workspace kept in its own separate file still travels whole
    #[test]
    fn a_workspace_kept_in_its_own_file_still_travels_whole() {
        let d = tmp("file");
        let cfg = d.join("config.json");
        std::fs::create_dir_all(d.join("workspaces")).unwrap();
        std::fs::write(
            d.join("workspaces/x.json"),
            json!({"folders": [{"tabs": [{"name": "AI", "command": "claude"}]}]}).to_string(),
        )
        .unwrap();
        std::fs::write(
            &cfg,
            json!({"workspaces": [{"name": "外だし", "file": "workspaces/x.json"}]}).to_string(),
        )
        .unwrap();

        let (_, text) = pack(&cfg, 0).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["workspace"]["name"], "外だし");
        assert_eq!(v["workspace"]["folders"][0]["tabs"][0]["name"], "AI");
        assert!(v["workspace"].get("file").is_none(), "参照のまま持ち出している");
    }

    /// Confirms the config entry wins over its definition file only where it
    /// says something, the way the program resolves the two
    #[test]
    fn an_entry_wins_over_its_file_only_where_it_says_something() {
        let d = tmp("merge");
        let cfg = d.join("config.json");
        std::fs::create_dir_all(d.join("workspaces")).unwrap();
        std::fs::write(
            d.join("workspaces/x.json"),
            json!({
                "name": "ファイルの名前",
                "lua": "scripts/file",
                "stops": [{"when": "x"}],
                "folders": [{"tabs": [{"name": "AI", "command": "claude"}]}]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &cfg,
            json!({"workspaces": [{
                "name": "",
                "file": "workspaces/x.json",
                "automation": "scripts/entry",
                "stops": [],
                "folders": [{"tabs": [{"name": "読まれない", "command": "claude"}]}]
            }]})
            .to_string(),
        )
        .unwrap();

        let (_, text) = pack(&cfg, 0).unwrap();
        let ws = &serde_json::from_str::<Value>(&text).unwrap()["workspace"];
        assert_eq!(ws["name"], "ファイルの名前", "空の名前がファイルの名前に勝った");
        assert_eq!(ws["automation"], "scripts/entry", "項目の指定がファイルに負けた");
        assert!(ws.get("lua").is_none(), "古い綴りが残っている");
        assert_eq!(ws["stops"][0]["when"], "x", "空のリストがファイルの中身に勝った");
        assert_eq!(ws["folders"][0]["tabs"][0]["name"], "AI", "ファイルの隣のタブが読まれた");
    }

    /// Confirms scripts are read from beside the config folder, where a tab
    /// runs them from, and written back there -- not inside the config folder
    #[test]
    fn scripts_live_beside_the_config_folder_not_inside_it() {
        let d = tmp("layout");
        std::fs::create_dir_all(d.join("config")).unwrap();
        let cfg = d.join("config/config.json");
        std::fs::write(
            &cfg,
            json!({"workspaces": [{
                "name": "x",
                "automation": "scripts/a",
                "folders": [{"tabs": [{"name": "AI", "command": "claude"}]}]
            }]})
            .to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(d.join("scripts/a")).unwrap();
        std::fs::write(d.join("scripts/a/on_start.lua"), "-- a").unwrap();

        let (_, text) = pack(&cfg, 0).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["scripts"]["scripts/a/on_start.lua"], "-- a", "exe の隣のスクリプトが見つからない");

        unpack(&cfg, &text).unwrap();
        assert!(d.join("scripts/a-2/on_start.lua").exists(), "exe の隣に置かれていない");
        assert!(!d.join("config/scripts").exists(), "設定フォルダの中に書かれた");
    }
}
