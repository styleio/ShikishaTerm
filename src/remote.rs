//! スマホ等から見る監視・指示用のリモートUI。DESIGN.md 10.4章。
//!
//! 端末画面をそのまま再現するのではなく「状況を見て、一言指示する」ことに絞る。
//! 実装は既存の材料 (状態検出・応答キャプチャ・画面テキスト) をJSONで返すだけで、
//! WebSocketも端末エミュレータも要らない。
//!
//! 安全性:
//!   - 既定で無効。設定で明示的に有効化したときだけ待ち受ける
//!   - 待ち受け先はプライベート網に限定 (netaddr.rs)
//!   - 長さ32バイトのトークン必須。定数時間比較
//!   - 遠隔からの入力は「人間の操作」として扱う (自動チェーンをリセットする)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use serde::Serialize;
use tiny_http::{Header, Response, Server};

/// 画面に見せるタブの状態 (本体から毎ティック更新される)
#[derive(Clone, Serialize, Default)]
pub struct RemoteTab {
    pub index: usize,
    pub name: String,
    pub state: String,
    pub locked: bool,
    /// 直近の応答 (無ければ画面の末尾)
    pub output: String,
    /// 確認待ちのときの画面 (選択肢を読むため)
    pub screen: String,
}

#[derive(Clone, Serialize, Default)]
pub struct Snapshot {
    pub workspace: String,
    pub tabs: Vec<RemoteTab>,
    pub auto_enabled: bool,
}

/// リモートから届く操作。本体のループで実行される
#[derive(Debug)]
pub enum RemoteCmd {
    /// タブへ指示を送る (人間の入力として扱う)
    Send { tab: usize, text: String },
    /// 確認への返答など、生のキーを送る
    Keys { tab: usize, keys: String },
    /// 自動化の緊急停止 / 再開
    SetAuto(bool),
}

pub struct RemoteUi {
    pub url: String,
    pub note: Option<String>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub rx: Receiver<RemoteCmd>,
    stop: Arc<AtomicBool>,
}

impl RemoteUi {
    pub fn start(bind: std::net::Ipv4Addr, port: u16, token: String) -> Result<Self> {
        let server = Server::http((bind, port))
            .map_err(|e| {
                anyhow::anyhow!(crate::i18n::tp(
                    "remote.err.start",
                    &[("addr", &format!("{bind}:{port}")), ("error", &e.to_string())]
                ))
            })?;
        let real_port = server
            .server_addr()
            .to_ip()
            .context("ポート取得に失敗")?
            .port();
        let url = format!("http://{bind}:{real_port}/?t={token}");
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel::<RemoteCmd>();
        let stop = Arc::new(AtomicBool::new(false));

        {
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = handle(req, &token, &snapshot, &tx) {
                        crate::append_hook_log(&format!("リモートUI: {e}"));
                    }
                }
            });
        }
        Ok(Self {
            url,
            note: None,
            snapshot,
            rx,
            stop,
        })
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn query_value(url: &str, key: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .unwrap_or("")
        .to_string()
}

fn json_response(v: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(v.to_string()).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap(),
    )
}

fn handle(
    req: tiny_http::Request,
    token: &str,
    snapshot: &Arc<Mutex<Snapshot>>,
    tx: &Sender<RemoteCmd>,
) -> Result<()> {
    let supplied = {
        let h = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-Token"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if h.is_empty() {
            query_value(req.url(), "t")
        } else {
            h
        }
    };
    if !token_eq(&supplied, token) {
        return req
            .respond(Response::from_string("forbidden").with_status_code(403))
            .map_err(Into::into);
    }

    let method = req.method().as_str().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    match (method.as_str(), path.as_str()) {
        ("GET", "/") => {
            let html = crate::i18n::render(PAGE)
                .replace("__TOKEN__", token)
                .replace("__DICT__", &crate::i18n::dict_json());
            req.respond(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ))?;
        }
        ("GET", "/api/state") => {
            let snap = snapshot.lock().unwrap().clone();
            req.respond(json_response(serde_json::to_value(snap)?))?;
        }
        ("POST", "/api/send") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let tab = v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            if let Some(text) = v.get("text").and_then(|x| x.as_str()) {
                let _ = tx.send(RemoteCmd::Send {
                    tab,
                    text: text.to_string(),
                });
            } else if let Some(keys) = v.get("keys").and_then(|x| x.as_str()) {
                let _ = tx.send(RemoteCmd::Keys {
                    tab,
                    keys: keys.to_string(),
                });
            }
            req.respond(json_response(serde_json::json!({"ok": true})))?;
        }
        ("POST", "/api/auto") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            let _ = tx.send(RemoteCmd::SetAuto(on));
            req.respond(json_response(serde_json::json!({"ok": true})))?;
        }
        _ => {
            req.respond(Response::from_string("not found").with_status_code(404))?;
        }
    }
    Ok(())
}

const PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="theme-color" content="#0f1115">
<title>{{app.title}}</title>
<style>
 :root { --bg:#0f1115; --panel:#161a20; --panel2:#1b2027; --line:#262d37;
   --text:#e6e9ef; --muted:#8b95a5; --accent:#35c46a; --warn:#e3b341; --danger:#e5534b;
   color-scheme: dark; }
 * { box-sizing:border-box; -webkit-tap-highlight-color:transparent; }
 body { margin:0; background:var(--bg); color:var(--text); font-size:15px; line-height:1.6;
   font-family:system-ui,-apple-system,"Segoe UI","Hiragino Sans",sans-serif;
   padding:env(safe-area-inset-top) 0 env(safe-area-inset-bottom); }
 header { position:sticky; top:0; z-index:3; display:flex; align-items:center; gap:10px;
   padding:12px 16px; background:rgba(15,17,21,.95); border-bottom:1px solid var(--line); }
 header b { font-size:15px; font-weight:600; }
 .spacer { flex:1; }
 main { padding:12px 14px 24px; max-width:720px; margin:0 auto; }

 .tab { display:block; width:100%; text-align:left;
   background:var(--panel); border:1px solid var(--line); color:var(--text);
   border-radius:12px; padding:14px; margin-bottom:10px; font-size:15px; font-family:inherit; }
 .tab .row { display:flex; align-items:center; gap:12px; }
 /* いま画面に出ている行。長いので1行に収める */
 .tab .now { margin-top:6px; color:var(--muted); font-size:12px;
   font-family:ui-monospace,Consolas,monospace; white-space:nowrap;
   overflow:hidden; text-overflow:ellipsis; }
 .tab .now:empty { display:none; }
 .tab.sel { border-color:var(--accent); }
 .dot { width:10px; height:10px; border-radius:50%; flex:none; }
 .busy { background:var(--warn); } .done { background:var(--accent); }
 .quest { background:#4aa3ff; } .wait { background:#4aa3ff; opacity:.5; }
 .exit { background:var(--danger); }
 .tab .st { color:var(--muted); font-size:12.5px; margin-left:auto; }
 .name { font-weight:600; }

 .out { background:var(--panel); border:1px solid var(--line); border-radius:12px;
   padding:12px 14px; margin:12px 0; white-space:pre-wrap; word-break:break-word;
   font-family:ui-monospace,Consolas,monospace; font-size:12.5px; color:#cfd6e0;
   max-height:46vh; overflow:auto; }
 .composer { position:sticky; bottom:0; background:linear-gradient(transparent,var(--bg) 22%);
   padding:10px 0 4px; }
 textarea { width:100%; background:var(--panel2); color:var(--text); border:1px solid var(--line);
   border-radius:12px; padding:12px; font-size:16px; font-family:inherit; resize:none; }
 .btns { display:flex; gap:8px; margin-top:8px; flex-wrap:wrap; }
 button { font-family:inherit; font-size:15px; border-radius:10px; padding:11px 16px;
   border:1px solid var(--line); background:var(--panel2); color:var(--text); }
 button.primary { background:var(--accent); border-color:var(--accent); color:#08130c; font-weight:700; }
 button.stop { color:var(--danger); }
 .quick button { padding:9px 14px; }
 .hint { color:var(--muted); font-size:12.5px; }
</style></head><body>
<header>
  <b id="ws">{{app.title}}</b>
  <span class="spacer"></span>
  <span class="hint" id="autost"></span>
  <button class="stop" onclick="toggleAuto()" id="autobtn">{{phone.stop}}</button>
</header>
<main>
  <div id="list"></div>
  <div id="detail" style="display:none">
    <div class="hint" id="dname"></div>
    <div class="out" id="out"></div>
    <div class="quick btns" id="quick"></div>
    <div class="composer">
      <textarea id="msg" rows="3" placeholder="{{phone.instruct.ph}}"></textarea>
      <div class="btns">
        <button class="primary" onclick="send()">{{phone.send}}</button>
        <button onclick="back()">{{phone.back}}</button>
        <button onclick="toggleView()" id="viewbtn">{{phone.view.live}}</button>
        <span class="spacer"></span>
        <span class="hint" id="sent"></span>
      </div>
    </div>
  </div>
</main>
<script>
const TOKEN = "__TOKEN__";
const api = (p, b) => fetch(p, {method: b ? "POST" : "GET",
  headers:{"X-Token":TOKEN,"Content-Type":"application/json"}, body: b});
let snap = {tabs:[]}, sel = null, dirty = false;
// 表示するもの: null は自動 (動いていれば今の画面、終わっていれば応答)。
// 利用者が選んだらそちらを優先する
let view = null, shownView = "live";

// 画面の最後の「中身がある行」。空行と、入力欄の枠だけの行は飛ばす
// (Claude Code や Codex は入力欄を画面下に描くので、素の最終行だと枠が出る)
function lastLine(screen) {
  // 区切り線と入力欄の枠。`-` は範囲指定と読まれないよう先に置く
  const frame = /^[-\s=_|>+*.·─-╿]+$/;
  const lines = (screen || "").split("\n")
    .map(l => l.trim())
    .filter(l => l && !frame.test(l));
  return lines.length ? lines[lines.length - 1] : "";
}

function toggleView() { view = shownView === "live" ? "result" : "live"; render(); }

const CLS = {BUSY:"busy", DONE:"done", QUESTION:"quest", WAIT:"wait", EXIT:"exit"};
const T = __DICT__;
const LABEL = {BUSY:T["state.busy"], DONE:T["state.done"], QUESTION:T["state.question"],
               WAIT:T["state.wait"], EXIT:T["state.exit"]};

async function poll() {
  try {
    snap = await (await api("/api/state")).json();
    render();
  } catch (e) {}
  setTimeout(poll, 700);
}

function render() {
  document.getElementById("ws").textContent = snap.workspace || T["app.title"];
  document.getElementById("autost").textContent = snap.auto_enabled ? T["phone.automation_on"] : T["phone.automation_off"];
  document.getElementById("autobtn").textContent = snap.auto_enabled ? T["phone.stop"] : T["phone.resume"];
  const list = document.getElementById("list");
  const detail = document.getElementById("detail");
  if (sel === null) {
    list.style.display = ""; detail.style.display = "none";
    list.textContent = "";
    for (const t of snap.tabs) {
      const b = document.createElement("button");
      b.className = "tab";
      b.onclick = () => { sel = t.index; render(); };
      const d = document.createElement("span");
      d.className = "dot " + (CLS[t.state] || "wait");
      const n = document.createElement("span");
      n.className = "name";
      n.textContent = (t.locked ? "🔒 " : "") + t.name;
      const s = document.createElement("span");
      s.className = "st";
      s.textContent = LABEL[t.state] || t.state;
      const head = document.createElement("div");
      head.className = "row";
      head.append(d, n, s);
      // いま画面に出ている最後の行。「考え中」や実行中の操作がそのまま出る
      const now = document.createElement("div");
      now.className = "now";
      now.textContent = lastLine(t.screen);
      b.append(head, now);
      list.append(b);
    }
    return;
  }
  const t = snap.tabs.find(x => x.index === sel);
  if (!t) { sel = null; return render(); }
  list.style.display = "none"; detail.style.display = "";
  document.getElementById("dname").textContent =
    t.name + " — " + (LABEL[t.state] || t.state);
  const out = document.getElementById("out");
  const atBottom = out.scrollTop + out.clientHeight >= out.scrollHeight - 20;
  // 動いている最中は最後の応答ではなく今の画面を見せる。
  // でないと「考え中」の間ずっと1つ前の完了内容が出たままになる
  const auto = (t.state === "BUSY" || t.state === "QUESTION" || !t.output) ? "live" : "result";
  shownView = view || auto;
  const text = (shownView === "live" ? (t.screen || t.output) : (t.output || t.screen)) || "";
  document.getElementById("viewbtn").textContent =
    shownView === "live" ? T["phone.view.result"] : T["phone.view.live"];
  if (out.textContent !== text) {
    out.textContent = text;
    if (atBottom) out.scrollTop = out.scrollHeight;
  }
  // 確認待ちのときだけ、よく使う返答を出す
  const q = document.getElementById("quick");
  q.textContent = "";
  if (t.state === "QUESTION") {
    for (const [label, keys] of [["1","1\r"],["2","2\r"],[T["phone.answer.yes"],"y\r"],
                                 [T["phone.answer.no"],"n\r"],["Enter","\r"]]) {
      const b = document.createElement("button");
      b.textContent = label;
      b.onclick = () => sendKeys(keys);
      q.append(b);
    }
  }
}

async function send() {
  const box = document.getElementById("msg");
  const text = box.value.trim();
  if (!text || sel === null) return;
  await api("/api/send", JSON.stringify({tab: sel, text}));
  box.value = "";
  flash(T["phone.sent"]);
}
async function sendKeys(keys) {
  await api("/api/send", JSON.stringify({tab: sel, keys}));
  flash(T["phone.sent"]);
}
async function toggleAuto() {
  await api("/api/auto", JSON.stringify({on: !snap.auto_enabled}));
}
// タブを離れたら表示の選択も戻す (次のタブでは自動判定させる)
function back() { sel = null; view = null; render(); }
function flash(t) {
  const s = document.getElementById("sent");
  s.textContent = t;
  setTimeout(() => { s.textContent = ""; }, 1500);
}
document.getElementById("msg").addEventListener("keydown", e => {
  // スマホでは改行を優先。Ctrl/⌘+Enterで送信
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); send(); }
});
poll();
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;


    /// トップレベルの const/let が重複していないこと。
    /// 重複すると SyntaxError でスクリプト全体が動かず、静的なHTMLだけが残る。
    /// 画面は出ているのに何も動かないという、原因の分かりにくい壊れ方をする
    fn top_level_bindings(page: &str) -> Vec<String> {
        page.lines()
            .filter_map(|l| l.strip_prefix("const ").or_else(|| l.strip_prefix("let ")))
            .filter_map(|rest| rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect()
    }
    
    fn assert_no_duplicate_bindings(name: &str, page: &str) {
        let names = top_level_bindings(page);
        for (i, n) in names.iter().enumerate() {
            assert!(
                !names[..i].contains(n),
                "{name}: `{n}` がトップレベルで二重宣言されている (JS全体が動かなくなる)"
            );
        }
    }

    /// スマホ画面に `{{key}}` や `__DICT__` がそのまま出ていないこと
    #[test]
    fn page_is_fully_rendered_and_uses_known_keys() {
        let html = crate::i18n::render(PAGE)
            .replace("__TOKEN__", "t")
            .replace("__DICT__", "{}");
        assert!(!html.contains("{{"), "未置換の {{{{key}}}} が残っている");
        assert!(!html.contains("__"), "未置換のプレースホルダが残っている");

        assert_no_duplicate_bindings("remote PAGE", PAGE);

        let en: serde_json::Value = serde_json::from_str(include_str!("../lang/en.json")).unwrap();
        let mut rest = PAGE;
        while let Some(i) = rest.find("T[\"") {
            rest = &rest[i + 3..];
            let key = &rest[..rest.find('"').unwrap()];
            assert!(en.get(key).is_some(), "lang/en.json に無いキー: {key}");
        }
    }

    #[test]
    fn token_is_required_and_compared_safely() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
        assert_eq!(query_value("/?t=xyz", "t"), "xyz");
        assert_eq!(query_value("/api/state", "t"), "");
    }

    /// 実際にサーバーを起動し、認証と操作の受け渡しを確認する
    #[test]
    fn serves_state_and_forwards_commands() {
        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into()).unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let agent = ureq::Agent::new_with_defaults();
        let status = |r: Result<ureq::http::Response<ureq::Body>, ureq::Error>| match r {
            Ok(x) => x.status().as_u16(),
            Err(ureq::Error::StatusCode(c)) => c,
            Err(e) => panic!("unexpected: {e}"),
        };

        // トークン無しは拒否
        assert_eq!(status(agent.get(&format!("{base}/api/state")).call()), 403);

        // 状態を返す
        ui.snapshot.lock().unwrap().tabs = vec![RemoteTab {
            index: 1,
            name: "実装".into(),
            state: "QUESTION".into(),
            ..Default::default()
        }];
        let body = agent
            .get(&format!("{base}/api/state?t=tok123456789012"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(body.contains("実装") && body.contains("QUESTION"), "{body}");

        // 指示が本体へ届く
        agent
            .post(&format!("{base}/api/send?t=tok123456789012"))
            .send(r#"{"tab":1,"text":"続けて"}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Send { tab, text } => {
                assert_eq!((tab, text.as_str()), (1, "続けて"));
            }
            other => panic!("想定外: {other:?}"),
        }
        ui.shutdown();
    }
}
