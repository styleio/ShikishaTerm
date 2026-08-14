//! ワークスペースを1つのファイルに持ち出し、取り込む。
//!
//! ワークスペースはタブの並びだけではない。タブが指す自動化スクリプトが
//! 別の場所にあり、それが無ければ持ち出したものは動かない。
//! だから設定とスクリプトを1つの入れ物に入れる。
//!
//! 取り込むときは、置き場所が既に埋まっていたら別名にする。
//! 同じ名前のフォルダを持つワークスペースを2つ入れたとき、
//! 後から入れた方が先の中身を上書きしたら、気づく術がない。
//!
//! 入らないもの: 通知先・secrets.json・能力(capabilities)。
//! これらは全体の設定であってワークスペースの持ち物ではないし、
//! 資格情報を配って回ることになる

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// 書式の版。読めない版は断る。黙って一部だけ取り込むより良い
const FORMAT: u64 = 1;
/// 1つの入れ物に入るスクリプトの数
const MAX_FILES: usize = 500;
/// 1つの入れ物の大きさ
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// 別名を探す回数。ここまで埋まっているなら、原因は別にある
const MAX_RENAME: u32 = 100;

/// 取り込んだ結果。何がどこへ置かれたかを人に見せるために返す
pub struct Placed {
    /// 実際に付いた名前 (重複していれば変えてある)
    pub name: String,
    /// スクリプトの置き場所 (元の名前から変わったものだけ)
    pub moved: Vec<(String, String)>,
    /// 書いたスクリプトの数
    pub files: usize,
}

/// 設定ファイルの置き場所。相対パスはすべてここが基準
fn base_of(config_path: &Path) -> Result<&Path> {
    config_path
        .parent()
        .ok_or_else(|| anyhow!(crate::i18n::t("err.wspack.no_config_dir")))
}

/// 設定フォルダの中を指しているか確かめる。
///
/// 取り込むファイルは他人が書いたものなので、行き先は必ず自分で決める。
/// 絶対パス・親への遡り・ドライブ指定は受け付けない
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

/// 設定の中の自動化の指定。新しい綴りを優先し、旧称にも応じる
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

/// ワークスペースとその全タブが指す自動化の場所を集める
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

/// 実際に持ち出す単位。存在するものだけを残し、
/// 別のフォルダの中に入っているものはその親に任せる
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

/// フォルダの中の .lua を集める。鍵は設定フォルダから見た場所
fn read_dir_lua(base: &Path, rel: &str, out: &mut Map<String, Value>) -> Result<()> {
    let Some(dir) = under_base(base, rel) else {
        return Ok(());
    };
    let mut entries: Vec<_> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok()).collect();
    // 並びを決めておく。書き出すたびに順が変わると、差分が読めない
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

/// 設定の workspaces を取り出す。
/// 昔の書き方 (tabs直書き) は、名前なしのワークスペース1つとして見る
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

/// 別ファイルに切り出されているものを、その場に展開する。
/// 持ち出した先では1枚で完結していてほしい
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
    // 旧い書き方のブラウザ宣言。設定画面は触らないが、持ち出しでは落とさない
    if let Some(b) = pick("browsers") {
        ws.insert("browsers".into(), b);
    }
    Ok(Value::Object(ws))
}

/// 番号で指したワークスペースを、1枚のファイルの中身にまとめる。
/// 返すのは (勧めるファイル名, 中身)
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
        // 貼り直す単位。ここが埋まっていたら別名にする
        "roots": keep,
        "scripts": Value::Object(scripts),
    });
    Ok((format!("{}.stws.json", safe_file_name(&name)), serde_json::to_string_pretty(&bundle)?))
}

/// ファイル名に使えない文字を落とす。空になったら既定の名前にする
fn safe_file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "workspace".into() } else { s }
}

/// 埋まっていない名前を探す。`scripts/ws2` が埋まっていれば `scripts/ws2-2`
fn free_name(base: &Path, rel: &str) -> Result<String> {
    if under_base(base, rel).is_some_and(|p| !p.exists()) {
        return Ok(rel.to_string());
    }
    // 拡張子があれば、その手前に付ける (ws2.lua-2 では読めない)
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

/// 前置きの置き換え。`scripts/ws2` を `scripts/ws2-2` にすると、
/// その中を指していた `scripts/ws2/html` も一緒に動く
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

/// ワークスペースとタブが指す自動化の場所を、置いた先へ書き換える
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

/// 既に使われていない表示名にする。同じ名前が2つ並ぶと、
/// タブバーでもスクリプトでもどちらを指しているのか分からない
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

/// 持ち出したファイルを取り込む。設定にワークスペースを1つ足し、
/// スクリプトを空いている場所へ置く
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

    // 置き場所を先に決める。1つでも置けないなら何も書かない
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
    // ワークスペースに移した以上、昔の直書きは残しておくと二重に見える
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

    /// 試験用の設定一式を作る
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

    /// スクリプトごと持ち出せること。
    /// 設定だけ渡しても、指す先が無ければ相手の手元では動かない
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
        // 貼り直す単位は親だけ。中のフォルダは親と一緒に動く
        assert_eq!(v["roots"], json!(["scripts/ws1"]));
    }

    /// 同じものを2回取り込んでも、先に入れた方が壊れないこと。
    /// 上書きしてしまうと、消えたことに誰も気づけない
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

        // 元のスクリプトは手つかず
        assert_eq!(
            std::fs::read_to_string(d.join("scripts/ws1/on_start.lua")).unwrap(),
            "-- はじめ"
        );
        // 入れた分はそれぞれ別の場所にある
        assert!(d.join("scripts/ws1-2/on_start.lua").exists());
        assert!(d.join("scripts/ws1-3/on_start.lua").exists());
        assert!(d.join("scripts/ws1-2/html/on_load.lua").exists());
    }

    /// 置き場所が変わったら、指している側も一緒に変わること
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

    /// 設定フォルダの外へは書かないこと。
    /// 取り込むファイルは他人が書いたもので、中身は信用できない
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

    /// Lua以外は書かないこと。取り込みは実行ファイルの配り口にしない
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

    /// 知らない書式は断ること。読めないものを黙って半分だけ入れない
    #[test]
    fn an_unknown_format_is_refused() {
        let d = tmp("format");
        let cfg = setup(&d);
        for bad in [json!({"workspace": {}}), json!({"shikisha_workspace": 99})] {
            assert!(unpack(&cfg, &bad.to_string()).is_err());
        }
    }

    /// 別ファイルに切り出したワークスペースも、1枚で持ち出せること
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
