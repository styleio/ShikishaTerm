//! 設定用ローカルWeb GUI。DESIGN.md 5.5 / 10.2章。
//!
//! セキュリティ:
//!   - 127.0.0.1 のみにバインド (外部からは到達不能、ファイアウォール警告も出ない)
//!   - ランダムポート + 起動毎のワンタイムトークン
//!   - トークンはURLとリクエストヘッダの両方で検証し、同一PCの他プロセスや
//!     悪意あるWebページ (CSRF / DNS rebinding) からの設定API操作を防ぐ
//!   - Hostヘッダを127.0.0.1系に限定 (DNS rebinding対策)
//!   - マスターパスワードはここでは扱わない (TUI内で完結させる)

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use rand::TryRng as _;
use tiny_http::{Header, Response, Server};

pub struct WebUi {
    pub url: String,
    stop: Arc<AtomicBool>,
}

impl WebUi {
    /// 設定ファイルを編集するローカルサーバーを起動する。
    /// config_path は編集対象 (通常 config.json)
    pub fn start(config_path: std::path::PathBuf) -> Result<Self> {
        let token = random_token()?;
        let server = Server::http("127.0.0.1:0")
            .map_err(|e| anyhow::anyhow!("ローカルサーバーを起動できません: {e}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .context("ポート取得に失敗")?
            .port();
        let url = format!("http://127.0.0.1:{port}/?token={token}");
        let stop = Arc::new(AtomicBool::new(false));

        {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = handle(req, &token, &config_path) {
                        crate::append_hook_log(&format!("WebUI: {e}"));
                    }
                }
            });
        }
        Ok(Self { url, stop })
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 24];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| anyhow::anyhow!("トークン生成に失敗: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// 定数時間比較 (トークンの総当たりを情報漏洩なく弾く)
fn token_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn header_value(req: &tiny_http::Request, name: &'static str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn query_token(url: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .unwrap_or("")
        .to_string()
}

/// ?file=... のパスを安全に解決する。
/// 設定ファイルと同じディレクトリ配下の .json だけを許可し、
/// 絶対パス・親ディレクトリ参照 (..) は拒否する (パストラバーサル対策)
fn safe_workspace_path(
    url: &str,
    config_path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let raw = url
        .split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix("file="))?;
    let decoded = percent_decode(raw);
    let rel = std::path::Path::new(&decoded);
    if rel.is_absolute() {
        return None;
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_)))
    {
        return None;
    }
    if rel.extension().and_then(|e| e.to_str()) != Some("json") {
        return None;
    }
    let base = config_path.parent()?;
    Some(base.join(rel))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn handle(req: tiny_http::Request, token: &str, config_path: &std::path::Path) -> Result<()> {
    // DNS rebinding対策: Hostは必ずループバックであること
    let host = header_value(&req, "Host");
    let host_ok = host.starts_with("127.0.0.1:") || host.starts_with("localhost:");
    // トークンはURLか X-Token ヘッダのどちらかで一致すればよい
    let supplied = {
        let h = header_value(&req, "X-Token");
        if h.is_empty() {
            query_token(req.url())
        } else {
            h
        }
    };
    if !host_ok || !token_eq(&supplied, token) {
        return req
            .respond(Response::from_string("forbidden").with_status_code(403))
            .map_err(Into::into);
    }

    let method = req.method().as_str().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    match (method.as_str(), path.as_str()) {
        ("GET", "/") => {
            let html = PAGE.replace("__TOKEN__", token);
            let resp = Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
            req.respond(resp)?;
        }
        ("GET", "/api/config") => {
            let text = std::fs::read_to_string(config_path).unwrap_or_else(|_| "{}".into());
            let resp = Response::from_string(text).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .unwrap(),
            );
            req.respond(resp)?;
        }
        // ワークスペース定義ファイル (外部ファイル参照) の読み書き
        ("GET", "/api/workspace") => {
            let Some(p) = safe_workspace_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let text = std::fs::read_to_string(&p).unwrap_or_else(|_| r#"{"tabs":[]}"#.into());
            let resp = Response::from_string(text).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .unwrap(),
            );
            req.respond(resp)?;
        }
        ("POST", "/api/workspace") => {
            let Some(p) = safe_workspace_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(_) => {
                    if let Some(dir) = p.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    crate::crypto::write_atomic(&p, &body)?;
                    req.respond(Response::from_string(r#"{"ok":true}"#))?;
                }
                Err(e) => {
                    let msg = serde_json::json!({ "ok": false, "error": e.to_string() });
                    req.respond(Response::from_string(msg.to_string()).with_status_code(400))?;
                }
            }
        }
        ("POST", "/api/config") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            // 壊れたJSONで設定を失わないよう、保存前に必ず検証する
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(_) => {
                    crate::crypto::write_atomic(config_path, &body)?;
                    req.respond(Response::from_string(r#"{"ok":true}"#))?;
                }
                Err(e) => {
                    let msg = serde_json::json!({ "ok": false, "error": e.to_string() });
                    req.respond(
                        Response::from_string(msg.to_string()).with_status_code(400),
                    )?;
                }
            }
        }
        _ => {
            req.respond(Response::new(
                404.into(),
                vec![],
                Cursor::new(b"not found".to_vec()),
                None,
                None,
            ))?;
        }
    }
    Ok(())
}

// 色指定の "#..." が終端と衝突しないよう r##"..."## で囲む
const PAGE: &str = r##"<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<title>ShikishaTerm-AI 設定</title>
<style>
 :root { color-scheme: dark; }
 body { background:#05080a; color:#39ff14; font-family: "Consolas","Meiryo",monospace;
        margin:0; padding:24px; }
 h1 { font-size:18px; letter-spacing:2px; border-bottom:1px solid #39ff14; padding-bottom:8px; }
 .row { margin:14px 0; display:flex; gap:8px; align-items:center; flex-wrap:wrap; }
 label { color:#00aaff; min-width:130px; }
 input, textarea { background:#0a1014; color:#39ff14; border:1px solid #1f4d2a;
        padding:6px 8px; font-family:inherit; font-size:13px; border-radius:3px; }
 input[type=text], input[type=number] { width:220px; }
 textarea { width:100%; min-height:340px; }
 button { background:#39ff14; color:#05080a; border:0; padding:8px 18px; font-weight:bold;
        cursor:pointer; border-radius:3px; font-family:inherit; }
 button.ghost { background:transparent; color:#39ff14; border:1px solid #39ff14; }
 #msg { margin-left:12px; }
 .warn { color:#ffea00; font-size:12px; margin-top:6px; }
 fieldset { border:1px solid #1f4d2a; margin:18px 0; padding:12px 16px; }
 legend { color:#ffea00; padding:0 6px; }
 table { border-collapse:collapse; width:100%; margin:10px 0; }
 td { padding:4px 6px; border-bottom:1px solid #12261a; vertical-align:middle; }
 td input[type=text] { width:100%; min-width:90px; }
 td button { padding:2px 8px; font-size:12px; }
 input[type=checkbox] { width:16px; height:16px; accent-color:#39ff14; }
 b { color:#00aaff; font-weight:normal; font-size:12px; }
 .tree { color:#ffea00; margin-left:4px; }
</style></head><body>
<h1>SHIKISHA-TERM-AI :: CONFIG</h1>

<fieldset><legend>基本設定</legend>
 <div class="row"><label>タブバー幅</label>
   <input type="number" id="tabw" min="10" max="40" placeholder="自動">
   <span class="warn">空にするとタブ名に合わせて自動調整</span></div>
 <div class="row"><label>自動チェーン上限</label>
   <input type="number" id="chain" min="1" max="100">
   <span class="warn">AI同士の自動転送が何回続いたら止めるか</span></div>
 <div class="row"><label>Luaフック(全体)</label>
   <input type="text" id="lua" placeholder="scripts/hooks.lua">
   <span class="warn">ワークスペース・タブ側の指定が優先されます</span></div>
 <div class="row"><label>secretsファイル</label>
   <input type="text" id="secrets" placeholder="secrets.json"></div>
</fieldset>

<fieldset><legend>ワークスペースとタブ</legend>
 <div id="wslist"></div>
 <button class="ghost" onclick="addWs()">＋ ワークスペースを追加</button>
</fieldset>

<div class="row">
  <button onclick="save()">保存</button>
  <button class="ghost" onclick="load()">再読込</button>
  <span id="msg"></span>
</div>
<p class="warn">保存後、変更を反映するにはアプリを再起動してください。
このページはこのセッション限りのトークンで保護されています。</p>

<script>
const TOKEN = "__TOKEN__";
const api = (m, b) => fetch("/api/config", {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });

// 画面の状態。JSONは触らせず、この配列を編集して保存時に組み立てる
let current = {};
let wss = [];   // [{name, file, tabs:[{name,command,profile,lua,locked,auto_restart,depth}]}]

const el = (tag, attrs = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (v !== null && v !== undefined) n.setAttribute(k, v);
  }
  for (const c of kids) n.append(c);
  return n;
};

/// config上の入れ子(children)を、画面用のフラットな配列に変換する
function flatten(tabs, depth, out) {
  for (const t of tabs || []) {
    out.push({ name: t.name || "", command: cmdToText(t.command), profile: t.profile || "",
               lua: t.lua || "", locked: !!t.locked, auto_restart: !!t.auto_restart, depth });
    flatten(t.children, depth + 1, out);
  }
  return out;
}
/// フラットな配列を depth に従って children へ組み直す
function nest(flat) {
  const roots = [], stack = [];
  for (const f of flat) {
    const node = { name: f.name, command: f.command };
    if (f.profile) node.profile = f.profile;
    if (f.lua) node.lua = f.lua;
    if (f.locked) node.locked = true;
    if (f.auto_restart) node.auto_restart = true;
    const d = Math.min(f.depth, stack.length);
    if (d === 0) roots.push(node);
    else {
      const parent = stack[d - 1];
      (parent.children = parent.children || []).push(node);
    }
    stack[d] = node; stack.length = d + 1;
  }
  return roots;
}
const cmdToText = c => Array.isArray(c) ? c.join(" ") : (c || "");

function render() {
  const box = document.getElementById("wslist");
  box.textContent = "";
  wss.forEach((ws, wi) => {
    const head = el("div", {class:"row"},
      el("label", {}, "ワークスペース"),
      input(ws, "name", "名前", "text"),
      el("span", {class:"warn"}, "Lua"),
      input(ws, "lua", "scripts/xxx.lua", "text"),
      el("span", {class:"warn"}, ws.file ? "定義ファイル: " + ws.file : "config.json内に定義"),
      el("button", {class:"ghost", onclick:() => { wss.splice(wi,1); render(); }}, "削除"),
      el("button", {class:"ghost", onclick:() => { moveWs(wi,-1); }}, "↑"),
      el("button", {class:"ghost", onclick:() => { moveWs(wi, 1); }}, "↓"));
    const table = el("table");
    table.append(el("tr", {},
      th("階層"), th("タブ名"), th("コマンド"), th("プロファイル"),
      th("Luaフック"), th("ロック"), th("自動再起動"), th("")));
    (ws.tabs || []).forEach((t, ti) => {
      table.append(el("tr", {},
        td(el("button", {class:"ghost", title:"1段下げる (親タブの子にする)",
             onclick:() => { t.depth = Math.min((t.depth||0)+1, ti); render(); }}, "→"),
           el("button", {class:"ghost", title:"1段上げる",
             onclick:() => { t.depth = Math.max((t.depth||0)-1, 0); render(); }}, "←"),
           el("span", {class:"tree"}, "　".repeat(t.depth||0) + (t.depth ? "└" : ""))),
        td(input(t, "name", "A:実装", "text")),
        td(input(t, "command", "claude / ssh user@host", "text")),
        td(input(t, "profile", "claude", "text")),
        td(input(t, "lua", "scripts/reviewer.lua", "text")),
        td(check(t, "locked")),
        td(check(t, "auto_restart")),
        td(el("button", {class:"ghost", onclick:() => { ws.tabs.splice(ti,1); render(); }}, "削除"),
           el("button", {class:"ghost", onclick:() => { moveTab(ws, ti,-1); }}, "↑"),
           el("button", {class:"ghost", onclick:() => { moveTab(ws, ti, 1); }}, "↓"))));
    });
    box.append(el("fieldset", {}, el("legend", {}, ws.name || "(名称未設定)"), head, table,
      el("button", {class:"ghost", onclick:() => {
        (ws.tabs = ws.tabs || []).push({name:"", command:"", profile:"", lua:"",
                                        locked:false, auto_restart:false, depth:0});
        render();
      }}, "＋ タブを追加")));
  });
}
const th = t => el("td", {}, el("b", {}, t));
const td = (...k) => el("td", {}, ...k);
function input(obj, key, ph, type) {
  const i = el("input", {type, placeholder: ph});
  i.value = obj[key] ?? "";
  i.addEventListener("input", () => { obj[key] = i.value; });
  return i;
}
function check(obj, key) {
  const i = el("input", {type:"checkbox"});
  i.checked = !!obj[key];
  i.addEventListener("change", () => { obj[key] = i.checked; });
  return i;
}
function moveWs(i, d) { const j=i+d; if(j<0||j>=wss.length) return;
  [wss[i],wss[j]]=[wss[j],wss[i]]; render(); }
function moveTab(ws, i, d) { const j=i+d; if(j<0||j>=ws.tabs.length) return;
  [ws.tabs[i],ws.tabs[j]]=[ws.tabs[j],ws.tabs[i]]; render(); }
function addWs() { wss.push({name:"新しいワークスペース", lua:"", tabs:[]}); render(); }

// 外部ファイル参照のワークスペース定義を読み書きする
const wsApi = (m, file, b) => fetch("/api/workspace?file=" + encodeURIComponent(file), {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });

async function load() {
  current = await (await api("GET")).json();
  document.getElementById("tabw").value    = current.tab_bar_width ?? "";
  document.getElementById("chain").value   = current.max_chain ?? "";
  document.getElementById("lua").value     = current.lua ?? "";
  document.getElementById("secrets").value = current.secrets ?? "";
  const list = (Array.isArray(current.workspaces) && current.workspaces.length)
      ? current.workspaces
      : [{ name: "DEFAULT", tabs: current.tabs || [] }];
  wss = [];
  for (const w of list) {
    const ws = { name: w.name || "", file: w.file || null, lua: w.lua || "", tabs: [] };
    if (ws.file) {
      // 定義ファイルの中身も読み込み、GUIから編集できるようにする
      try {
        const f = await (await wsApi("GET", ws.file)).json();
        ws.tabs = flatten(f.tabs, 0, []);
        if (!ws.lua && f.lua) ws.lua = f.lua;
      } catch (e) { ws.loadError = String(e); }
    } else {
      ws.tabs = flatten(w.tabs, 0, []);
    }
    wss.push(ws);
  }
  render();
  msg("読み込みました", "#39ff14");
}

async function save() {
  const out = Object.assign({}, current);
  const tabw  = document.getElementById("tabw").value;
  const chain = document.getElementById("chain").value;
  const lua   = document.getElementById("lua").value.trim();
  const sec   = document.getElementById("secrets").value.trim();
  tabw  ? out.tab_bar_width = Number(tabw) : delete out.tab_bar_width;
  chain ? out.max_chain     = Number(chain) : delete out.max_chain;
  lua   ? out.lua           = lua : delete out.lua;
  sec   ? out.secrets       = sec : delete out.secrets;

  delete out.tabs;
  // 外部ファイル参照のワークスペースは、その定義ファイル側に中身を書き戻す
  for (const w of wss) {
    if (!w.file) continue;
    const body = { name: w.name, tabs: nest(w.tabs) };
    if (w.lua) body.lua = w.lua;
    const rf = await wsApi("POST", w.file, JSON.stringify(body, null, 2));
    const jf = await rf.json().catch(() => ({ok:false, error:"保存に失敗"}));
    if (!jf.ok) return msg(w.file + " の保存に失敗: " + (jf.error || ""), "#ff4646");
  }
  out.workspaces = wss.map(w => {
    const o = { name: w.name };
    // 定義ファイル側にluaを書いたので、config側は参照だけにする
    if (w.file) o.file = w.file;
    else {
      if (w.lua) o.lua = w.lua;
      o.tabs = nest(w.tabs);
    }
    return o;
  });
  const r = await api("POST", JSON.stringify(out, null, 2));
  const j = await r.json();
  msg(j.ok ? "保存しました (アプリ再起動で反映)" : "保存失敗: " + j.error,
      j.ok ? "#39ff14" : "#ff4646");
}

function msg(t, c) { const m = document.getElementById("msg"); m.textContent = t; m.style.color = c; }
load();
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare_rejects_mismatch() {
        assert!(token_eq("abc123", "abc123"));
        assert!(!token_eq("abc123", "abc124"));
        assert!(!token_eq("abc", "abc123"));
    }

    #[test]
    fn token_is_parsed_from_query() {
        assert_eq!(query_token("/?token=deadbeef"), "deadbeef");
        assert_eq!(query_token("/api/config?x=1&token=zz"), "zz");
        assert_eq!(query_token("/"), "");
    }

    #[test]
    fn workspace_path_rejects_traversal() {
        let cfg = std::path::Path::new("C:/app/config.json");
        // 正常系
        assert!(safe_workspace_path("/api/workspace?file=workspaces/x.json", cfg).is_some());
        // パストラバーサル・絶対パス・非JSONは拒否
        assert!(safe_workspace_path("/api/workspace?file=../secrets.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=workspaces/../../x.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=C:/windows/x.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=workspaces/x.lua", cfg).is_none());
        // URLエンコードされた .. も拒否
        assert!(safe_workspace_path("/api/workspace?file=%2E%2E%2Fsecrets.json", cfg).is_none());
    }

    #[test]
    fn tokens_are_unique_and_long() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
    }

    /// 実際にサーバーを起動し、認証と保存の動作を確認する
    #[test]
    fn server_requires_token_and_saves_valid_json() {
        let dir = std::env::temp_dir().join("shikisha-webui-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, r#"{"max_chain":10}"#).unwrap();

        let ui = WebUi::start(cfg.clone()).unwrap();
        let token = ui.url.split("token=").nth(1).unwrap().to_string();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let agent = ureq::Agent::new_with_defaults();

        // トークン無し → 403
        let status = agent
            .get(&format!("{base}/api/config"))
            .call()
            .map(|r| r.status().as_u16())
            .unwrap_or_else(|e| match e {
                ureq::Error::StatusCode(c) => c,
                other => panic!("unexpected: {other}"),
            });
        assert_eq!(status, 403, "トークン無しは拒否される");

        // 正しいトークン → 現在の設定が読める
        let body = agent
            .get(&format!("{base}/api/config?token={token}"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(body.contains("max_chain"));

        // 壊れたJSONは保存されない
        let bad = agent
            .post(&format!("{base}/api/config?token={token}"))
            .send("{ broken")
            .map(|r| r.status().as_u16())
            .unwrap_or_else(|e| match e {
                ureq::Error::StatusCode(c) => c,
                other => panic!("unexpected: {other}"),
            });
        assert_eq!(bad, 400);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            r#"{"max_chain":10}"#,
            "検証に失敗したら元の設定は保たれる"
        );

        // 正しいJSONは保存される
        agent
            .post(&format!("{base}/api/config?token={token}"))
            .send(r#"{"max_chain":5}"#)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            r#"{"max_chain":5}"#
        );

        ui.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
