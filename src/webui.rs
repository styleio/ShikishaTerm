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
 table { border-collapse:collapse; width:100%; }
 td { padding:4px 6px; border-bottom:1px solid #12261a; }
</style></head><body>
<h1>SHIKISHA-TERM-AI :: CONFIG</h1>

<fieldset><legend>基本設定</legend>
 <div class="row"><label>タブバー幅</label>
   <input type="number" id="tabw" min="10" max="40" placeholder="自動">
   <span class="warn">空にするとタブ名に合わせて自動調整</span></div>
 <div class="row"><label>自動チェーン上限</label>
   <input type="number" id="chain" min="1" max="100">
   <span class="warn">AI同士の自動転送が何回続いたら止めるか</span></div>
 <div class="row"><label>Luaフック</label>
   <input type="text" id="lua" placeholder="scripts/hooks.lua"></div>
 <div class="row"><label>secretsファイル</label>
   <input type="text" id="secrets" placeholder="secrets.json"></div>
</fieldset>

<fieldset><legend>タブ / ワークスペース (JSONを直接編集)</legend>
 <textarea id="json" spellcheck="false"></textarea>
 <div class="warn">保存前にJSONの妥当性を検証します。壊れた内容では保存されません。</div>
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
let current = {};

async function load() {
  const r = await api("GET");
  current = await r.json();
  document.getElementById("tabw").value    = current.tab_bar_width ?? "";
  document.getElementById("chain").value   = current.max_chain ?? "";
  document.getElementById("lua").value     = current.lua ?? "";
  document.getElementById("secrets").value = current.secrets ?? "";
  const rest = {};
  for (const k of ["workspaces","tabs","notify"]) if (k in current) rest[k] = current[k];
  document.getElementById("json").value = JSON.stringify(rest, null, 2);
  msg("読み込みました", "#39ff14");
}

async function save() {
  let rest;
  try { rest = JSON.parse(document.getElementById("json").value || "{}"); }
  catch (e) { return msg("JSONエラー: " + e.message, "#ff4646"); }
  const out = Object.assign({}, rest);
  const tabw  = document.getElementById("tabw").value;
  const chain = document.getElementById("chain").value;
  const lua   = document.getElementById("lua").value.trim();
  const sec   = document.getElementById("secrets").value.trim();
  if (tabw)  out.tab_bar_width = Number(tabw);
  if (chain) out.max_chain     = Number(chain);
  if (lua)   out.lua           = lua;
  if (sec)   out.secrets       = sec;
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
