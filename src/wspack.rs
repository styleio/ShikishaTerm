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
//! shipping them around would mean handing out credentials

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Format version. An unreadable version is refused. Better than silently
/// importing only part of it
const FORMAT: u64 = 1;
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

/// Where the config file lives. Every relative path is based on this
fn base_of(config_path: &Path) -> Result<&Path> {
    config_path
        .parent()
        .ok_or_else(|| anyhow!(crate::i18n::t("err.wspack.no_config_dir")))
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

/// Collects every automation location referenced by the workspace and all its tabs
fn referenced(ws: &Value) -> Vec<String> {
    fn walk(tabs: Option<&Value>, out: &mut Vec<String>) {
        for t in tabs.and_then(Value::as_array).into_iter().flatten() {
            if let Some(a) = automation_of(t) {
                out.push(a);
            }
            walk(t.get("children"), out);
        }
    }
    let mut out = Vec::new();
    if let Some(a) = automation_of(ws) {
        out.push(a);
    }
    walk(ws.get("tabs"), &mut out);
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

/// Pulls `workspaces` out of the config.
/// The old style (tabs written directly) is treated as a single unnamed workspace
fn workspace_list(cfg: &Value) -> Vec<Value> {
    match cfg.get("workspaces").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a.clone(),
        _ => {
            let tabs = cfg.get("tabs").cloned().unwrap_or_else(|| json!([]));
            if tabs.as_array().is_some_and(|a| a.is_empty()) {
                Vec::new()
            } else {
                vec![json!({ "name": "DEFAULT", "tabs": tabs })]
            }
        }
    }
}

/// Expands anything that was split out into a separate file, inline.
/// Once taken out, it should be self-contained in one piece
fn inline(base: &Path, entry: &Value) -> Result<Value> {
    let mut ws = Map::new();
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("");

    let file_body = match entry.get("file").and_then(Value::as_str) {
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
            serde_json::from_str::<Value>(&text).map_err(|e| {
                anyhow!(crate::i18n::tp(
                    "err.wspack.file_bad_json",
                    &[("f", f), ("e", &e.to_string())]
                ))
            })?
        }
        None => Value::Null,
    };
    let pick = |key: &str| -> Option<Value> {
        entry
            .get(key)
            .filter(|v| !v.is_null())
            .or_else(|| file_body.get(key).filter(|v| !v.is_null()))
            .cloned()
    };

    let shown = if name.is_empty() {
        file_body
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("UNNAMED")
    } else {
        name
    };
    ws.insert("name".into(), Value::String(shown.to_string()));
    if let Some(a) = pick("automation").or_else(|| pick("lua")) {
        ws.insert("automation".into(), a);
    }
    ws.insert("tabs".into(), pick("tabs").unwrap_or_else(|| json!([])));
    // Old-style browser declaration. The settings screen doesn't touch it, but export shouldn't drop it
    if let Some(b) = pick("browsers") {
        ws.insert("browsers".into(), b);
    }
    Ok(Value::Object(ws))
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
        if let Some(kids) = v.get_mut("children").and_then(Value::as_array_mut) {
            for k in kids {
                one(k, moves);
            }
        }
    }
    one(ws, moves);
    if let Some(tabs) = ws.get_mut("tabs").and_then(Value::as_array_mut) {
        for t in tabs {
            one(t, moves);
        }
    }
}

/// Picks a display name that isn't already used. If the same name appears
/// twice, there's no way to tell which one the tab bar or a script is pointing at
fn free_title(list: &[Value], want: &str) -> String {
    let taken = |n: &str| {
        list.iter()
            .any(|w| w.get("name").and_then(Value::as_str) == Some(n))
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
        Some(v) if v == FORMAT => {}
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
    list.push(ws);
    cfg["workspaces"] = Value::Array(list);
    // Now that it's moved into workspaces, keeping the old direct-write form around would show it twice
    if let Some(o) = cfg.as_object_mut() {
        o.remove("tabs");
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

    /// Builds a set of config files for testing
    fn setup(dir: &Path) -> PathBuf {
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "workspaces": [{
                    "name": "編集部",
                    "automation": "scripts/ws1",
                    "tabs": [
                        {"name": "AI", "command": "claude"},
                        {"name": "html", "command": "browser https://example.com",
                         "automation": "scripts/ws1/html"}
                    ]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("scripts/ws1/html")).unwrap();
        std::fs::write(dir.join("scripts/ws1/on_start.lua"), "-- はじめ").unwrap();
        std::fs::write(dir.join("scripts/ws1/html/on_load.lua"), "-- よみこみ").unwrap();
        cfg
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-wspack-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Confirms scripts travel along with the workspace.
    /// Passing only the config, without what it points at, leaves it dead on the other end
    #[test]
    fn a_workspace_travels_with_its_scripts() {
        let d = tmp("pack");
        let cfg = setup(&d);
        let (name, text) = pack(&cfg, 0).unwrap();
        assert_eq!(name, "編集部.stws.json");
        let v: Value = serde_json::from_str(&text).unwrap();
        let s = v["scripts"].as_object().unwrap();
        assert_eq!(
            s["scripts/ws1/on_start.lua"], "-- はじめ",
            "ワークスペースのスクリプトが入っていない"
        );
        assert_eq!(
            s["scripts/ws1/html/on_load.lua"], "-- よみこみ",
            "タブのスクリプトが入っていない"
        );
        // The re-place unit is only the parent. Nested folders travel with their parent
        assert_eq!(v["roots"], json!(["scripts/ws1"]));
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

        let second = unpack(&cfg, &text).unwrap();
        assert_eq!(second.name, "編集部-3");

        // The original scripts are untouched
        assert_eq!(
            std::fs::read_to_string(d.join("scripts/ws1/on_start.lua")).unwrap(),
            "-- はじめ"
        );
        // Each import lands in its own separate location
        assert!(d.join("scripts/ws1-2/on_start.lua").exists());
        assert!(d.join("scripts/ws1-3/on_start.lua").exists());
        assert!(d.join("scripts/ws1-2/html/on_load.lua").exists());
    }

    /// Confirms that when the destination changes, whatever points at it changes along with it
    #[test]
    fn the_tabs_point_at_where_the_scripts_actually_landed() {
        let d = tmp("rewrite");
        let cfg = setup(&d);
        let (_, text) = pack(&cfg, 0).unwrap();
        unpack(&cfg, &text).unwrap();

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let added = &v["workspaces"][1];
        assert_eq!(added["automation"], "scripts/ws1-2");
        assert_eq!(
            added["tabs"][1]["automation"], "scripts/ws1-2/html",
            "中のフォルダが元の場所を指したまま"
        );
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
                "workspace": {"name": "わるいもの", "tabs": []},
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
            "workspace": {"name": "わるいもの", "tabs": []},
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
        for bad in [json!({"workspace": {}}), json!({"shikisha_workspace": 99})] {
            assert!(unpack(&cfg, &bad.to_string()).is_err());
        }
    }

    /// Confirms a workspace kept in its own separate file still travels whole
    #[test]
    fn a_workspace_kept_in_its_own_file_still_travels_whole() {
        let d = tmp("file");
        let cfg = d.join("config.json");
        std::fs::create_dir_all(d.join("workspaces")).unwrap();
        std::fs::write(
            d.join("workspaces/x.json"),
            json!({"tabs": [{"name": "AI", "command": "claude"}]}).to_string(),
        )
        .unwrap();
        std::fs::write(
            &cfg,
            json!({"workspaces": [{"name": "外だし", "file": "workspaces/x.json"}]}).to_string(),
        )
        .unwrap();

        let (_, text) = pack(&cfg, 0).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["workspace"]["tabs"][0]["name"], "AI");
        assert!(v["workspace"].get("file").is_none(), "参照のまま持ち出している");
    }
}
