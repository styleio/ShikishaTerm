//! ブラウザを1台、指揮下に置く。
//!
//! Windows 11 には Chromium エンジン (WebView2) が最初から入っていて、
//! Microsoft が更新し続けている。だから同梱しない。借りる。
//! 「インストール不要の単一exe」はそのまま保たれる。
//!
//! 窓は別スレッドで動かす。TUIの描画ループとメッセージループは
//! どちらも自分の都合で回りたがるので、混ぜない。
//!
//! `run` ではなく `run_return` を使うこと。`run` は `-> !` で、
//! 内部で `process::exit` を呼ぶ。ブラウザの窓を閉じただけで
//! アプリ全体が消える

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Result, anyhow};

/// 各文書に必ず先に流し込むもの。
///
/// 遷移のたびに実行されるので、ログインが何度リダイレクトしても
/// 帯を出す手段は生き残る。ただし「今出すべきか」はRust側が覚えていて、
/// 遷移のたびに指示し直す (JSの世界は遷移で消えるので)
const INIT_JS: &str = r#"
(function () {
  if (window.__shikisha) return;
  const send = (o) => window.ipc.postMessage(JSON.stringify(o));

  // 人への呼びかけ。ページのCSSと喧嘩しないよう影の中に閉じる
  window.__shikisha_ask = function (text, label) {
    let host = document.getElementById("__shikisha_bar");
    if (!host) {
      host = document.createElement("div");
      host.id = "__shikisha_bar";
      host.style.cssText =
        "position:fixed;left:0;right:0;bottom:0;z-index:2147483647";
      (document.body || document.documentElement).appendChild(host);
      host.attachShadow({ mode: "open" });
    }
    host.shadowRoot.innerHTML =
      '<div style="font:14px/1.5 system-ui,sans-serif;background:#0a0c0e;' +
      'color:#e8eef4;border-top:3px solid #00aaff;padding:12px 16px;' +
      'display:flex;align-items:center;gap:16px">' +
      '<span style="flex:1"></span>' +
      '<button style="font:600 14px system-ui;background:#00aaff;color:#04121c;' +
      'border:0;border-radius:6px;padding:8px 18px;cursor:pointer"></button></div>';
    host.shadowRoot.querySelector("span").textContent = text;
    const b = host.shadowRoot.querySelector("button");
    b.textContent = label;
    b.onclick = () => send({ kind: "button" });
  };

  window.__shikisha_unask = function () {
    const host = document.getElementById("__shikisha_bar");
    if (host) host.remove();
  };

  // セレクタは {css:"..."} か {xpath:"..."}。
  // XPath は「『氏名』というラベルの右隣のセル」のように、
  // CSSでは書けない探し方ができるので両方持つ
  window.__shikisha_q = function (sel) {
    if (sel && sel.xpath) {
      return document.evaluate(sel.xpath, document, null, 9, null).singleNodeValue;
    }
    return document.querySelector(sel.css);
  };

  // 「DOMに無い」と「あるが画面外」を分ける。
  // 同じ失敗で潰すと、セレクタを疑うべきか待ちを疑うべきか分からない
  window.__shikisha_state = function (sel) {
    const el = window.__shikisha_q(sel);
    if (!el) return "not_found";
    const r = el.getBoundingClientRect();
    const on =
      r.width > 0 && r.height > 0 &&
      r.bottom > 0 && r.right > 0 &&
      r.top < innerHeight && r.left < innerWidth;
    return on ? "visible" : "off_screen";
  };

  window.__shikisha_text = function (sel) {
    const el = window.__shikisha_q(sel);
    return el ? (el.value !== undefined ? el.value : el.innerText) : null;
  };

  window.__shikisha_click = function (sel) {
    const el = window.__shikisha_q(sel);
    if (!el) return "not_found";
    el.scrollIntoView({ block: "center" });
    el.click();
    // 触れた以上、届いていた。判定の語彙は find と揃える
    return "visible";
  };

  window.__shikisha_fill = function (sel, value) {
    const el = window.__shikisha_q(sel);
    if (!el) return "not_found";
    el.scrollIntoView({ block: "center" });
    el.focus();
    if (el.isContentEditable) {
      el.textContent = value;
    } else {
      // React などは value を直接書いても気づかない。
      // 元の setter を通してから input を投げると、枠組み側の状態も動く
      const proto =
        el instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype;
      const setter = Object.getOwnPropertyDescriptor(proto, "value");
      if (setter && setter.set) setter.set.call(el, value);
      else el.value = value;
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return "visible";
  };

  window.__shikisha_html = function () {
    return document.documentElement.outerHTML;
  };

  window.__shikisha = true;

  // 「準備できた」は文書が解析できてから。この初期化スクリプト自体は
  // 解析前に走るので、ここで名乗ると body すら無い時点の合図になる
  const announce = () => send({ kind: "ready", url: location.href });
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", announce, { once: true });
  } else {
    announce();
  }
})();
"#;

/// 指揮者からブラウザへの指示
#[derive(Debug, Clone)]
pub enum Cmd {
    /// このURLを開く
    Open(String),
    /// JSを評価して結果を返す (`id` で対応づける)
    Eval { id: u64, js: String },
    /// 人へ呼びかける帯を出す
    Ask { text: String, label: String },
    /// 帯を消す
    Unask,
    /// ターミナルに重ねる (位置と大きさを合わせる)
    Fit { x: i32, y: i32, w: i32, h: i32 },
    /// 見せる / 隠す
    Show(bool),
    /// 窓を閉じる
    Close,
}

/// ブラウザから指揮者への報告
#[derive(Debug, Clone)]
pub enum Ev {
    /// 文書が読み込まれた (遷移のたびに来る)
    Ready { url: String },
    /// `Eval` の結果。`value` はJSON
    Result { id: u64, ok: bool, value: String },
    /// 帯のボタンが押された = 人が自分の番を終えた
    Button,
    /// 選択された文字 (PuTTY と同じで、選んだ時点でコピーする)
    Copy { text: String },
    /// 貼り付けの要求 (右クリック)
    Paste,
    /// 窓モードでの打鍵。確定した文字、名前付きの制御キー、Ctrl+文字のいずれか
    Key {
        text: Option<String>,
        named: Option<String>,
        ctrl: Option<String>,
    },
    /// 窓が閉じられた
    Closed,
}

/// 動いているブラウザ1台への取っ手
pub struct Browser {
    proxy: tao::event_loop::EventLoopProxy<Cmd>,
    events: Receiver<Ev>,
    next_id: AtomicU64,
    /// 出しておくべき帯。遷移でJSの世界ごと消えるので、
    /// 新しい文書が用意できるたびに出し直す。
    /// ログインはSSOで2〜3回飛ぶのが普通で、
    /// 入れ直さないと「最初だけ出て途中で消える」ことになる
    pending_ask: std::sync::Mutex<Option<(String, String)>>,
}

/// 開けるURLか。http/https だけを通す。
///
/// wry はページからのIPCを受け取るとき、そのページのURLを `http::Uri` として
/// 組み立てて `unwrap` する (webview2/mod.rs)。`file:///` も `data:` も
/// そこで解釈に失敗し、**プロセスごと落ちる** (実測)。
/// こちらから流し込む初期化スクリプトが必ずIPCを送るので、
/// 開けてしまえば必ず落ちる。だから入口で止める。
///
/// 手元のファイルを見せたいときは、このソフトが持っている
/// ローカルHTTPで配れば同じことができる
pub fn is_openable(url: &str) -> bool {
    let u = url.trim();
    let scheme_ok = u.starts_with("https://") || u.starts_with("http://");
    let has_host = u.split("//").nth(1).is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        !host.is_empty()
    });
    scheme_ok && has_host && !u.contains(['\n', '\r', ' '])
}

impl Browser {
    /// 窓を開いて、指示を受け付ける状態にする
    /// `overlay` = ターミナルに重ねる。枠を付けず、位置はこちらが決める
    pub fn spawn_with(url: &str, title: &str, overlay: bool) -> Result<Self> {
        if !is_openable(url) {
            return Err(anyhow!("開けないURLです: {url}"));
        }
        Self::start(url, title, overlay)
    }

    pub fn spawn(url: &str, title: &str) -> Result<Self> {
        if !is_openable(url) {
            return Err(anyhow!("開けないURLです: {url}"));
        }
        Self::start(url, title, false)
    }

    fn start(url: &str, title: &str, overlay: bool) -> Result<Self> {
        let (proxy_tx, proxy_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        let url = url.to_string();
        let title = title.to_string();

        std::thread::Builder::new()
            .name("shikisha-browser".into())
            .spawn(move || {
                if let Err(e) = run_window(&url, &title, overlay, proxy_tx, ev_tx.clone()) {
                    crate::append_hook_log(&format!("ブラウザを開けません: {e}"));
                    let _ = ev_tx.send(Ev::Closed);
                }
            })?;

        // 窓ができるまで待つ (作れなければ proxy は届かない)
        let proxy = proxy_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| anyhow!("ブラウザの起動が終わりません (WebView2 が無い可能性)"))?;

        let me = Self {
            proxy,
            events: ev_rx,
            next_id: AtomicU64::new(1),
            pending_ask: std::sync::Mutex::new(None),
        };
        // 文書が用意できるまで返さない。窓ができた時点で返すと、
        // 呼んだ側は空の文書を触ることになり、
        // 「セレクタが違う」のか「まだ来ていない」のか区別がつかない
        me.wait_ready(std::time::Duration::from_secs(30))?;
        Ok(me)
    }

    /// 次の文書が用意できるまで待ち、そのURLを返す。
    /// 遷移のたびに1回来るので、`open` の後にも使う
    pub fn wait_ready(&self, timeout: std::time::Duration) -> Result<String> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!("ページが用意できません"))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Ready { url }) => {
                    self.reask();
                    return Ok(url);
                }
                Ok(Ev::Closed) => return Err(anyhow!("ブラウザが閉じました")),
                Ok(_) => continue,
                Err(_) => return Err(anyhow!("ページが用意できません")),
            }
        }
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.proxy
            .send_event(cmd)
            .map_err(|_| anyhow!("ブラウザが閉じています"))
    }

    pub fn open(&self, url: &str) -> Result<()> {
        if !is_openable(url) {
            return Err(anyhow!("開けないURLです: {url}"));
        }
        self.send(Cmd::Open(url.to_string()))
    }

    /// JSを評価する。結果は `Ev::Result` で後から届く
    pub fn eval(&self, js: &str) -> Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Eval {
            id,
            js: js.to_string(),
        })?;
        Ok(id)
    }

    pub fn ask(&self, text: &str, label: &str) -> Result<()> {
        *self.pending_ask.lock().unwrap() = Some((text.to_string(), label.to_string()));
        self.send(Cmd::Ask {
            text: text.to_string(),
            label: label.to_string(),
        })
    }

    pub fn unask(&self) -> Result<()> {
        *self.pending_ask.lock().unwrap() = None;
        self.send(Cmd::Unask)
    }

    pub fn close(&self) -> Result<()> {
        self.send(Cmd::Close)
    }

    /// ターミナルの上にぴったり重ねる
    pub fn fit(&self, x: i32, y: i32, w: i32, h: i32) -> Result<()> {
        self.send(Cmd::Fit { x, y, w, h })
    }

    /// 見せる / 隠す (他のタブを見ている間は隠す)
    pub fn show(&self, on: bool) -> Result<()> {
        self.send(Cmd::Show(on))
    }

    /// JSを1回呼んで、結果が返るまで待つ
    fn call(&self, func: &str, args: &[serde_json::Value], timeout_ms: u64) -> Result<String> {
        let id = self.eval(&call_js(func, args))?;
        self.wait_result(id, std::time::Duration::from_millis(timeout_ms))
    }

    /// その要素が今どこにいるか
    pub fn find(&self, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        Ok(Found::parse(&self.call(
            "__shikisha_state",
            &[sel.json()],
            timeout_ms,
        )?))
    }

    /// 文字を読む (入力欄なら中身、それ以外は表示文字列)
    pub fn text(&self, sel: &Sel, timeout_ms: u64) -> Result<Option<String>> {
        let v = self.call("__shikisha_text", &[sel.json()], timeout_ms)?;
        Ok(serde_json::from_str::<Option<String>>(&v).unwrap_or(None))
    }

    /// 押す
    pub fn click(&self, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        Ok(Found::parse(&self.call(
            "__shikisha_click",
            &[sel.json()],
            timeout_ms,
        )?))
    }

    /// 入力欄に値を入れる
    pub fn fill(&self, sel: &Sel, value: &str, timeout_ms: u64) -> Result<Found> {
        Ok(Found::parse(&self.call(
            "__shikisha_fill",
            &[sel.json(), serde_json::Value::String(value.to_string())],
            timeout_ms,
        )?))
    }

    /// 解釈済みのHTML全文
    pub fn html(&self, timeout_ms: u64) -> Result<String> {
        let v = self.call("__shikisha_html", &[], timeout_ms)?;
        Ok(serde_json::from_str::<String>(&v).unwrap_or(v))
    }

    /// 溜まっている報告を取り出す (待たない)。
    /// 新しい文書に移っていたら、出しておくべき帯を出し直す
    pub fn drain(&self) -> Vec<Ev> {
        let evs: Vec<Ev> = self.events.try_iter().collect();
        if evs.iter().any(|e| matches!(e, Ev::Ready { .. })) {
            self.reask();
        }
        evs
    }

    /// 遷移で消えた帯を出し直す
    fn reask(&self) {
        let want = self.pending_ask.lock().unwrap().clone();
        if let Some((t, l)) = want {
            let _ = self.send(Cmd::Ask { text: t, label: l });
        }
    }

    /// 特定の評価の結果が届くまで待つ
    pub fn wait_result(&self, id: u64, timeout: std::time::Duration) -> Result<String> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!("結果が返りません"))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Result { id: got, ok, value }) if got == id => {
                    return if ok {
                        Ok(value)
                    } else {
                        Err(anyhow!("JSの評価に失敗: {value}"))
                    };
                }
                Ok(Ev::Ready { .. }) => {
                    self.reask();
                    continue;
                }
                Ok(_) => continue,
                Err(_) => return Err(anyhow!("結果が返りません")),
            }
        }
    }
}

impl Drop for Browser {
    /// 指揮者がいなくなった窓を残さない。
    /// 閉じられなくても構わない (相手が先に死んでいるだけなので)
    fn drop(&mut self) {
        let _ = self.proxy.send_event(Cmd::Close);
    }
}

/// 評価式を、結果がIPCで返る形に包む
fn wrap_eval(id: u64, js: &str) -> String {
    format!(
        r#"(function(){{
  try {{
    var v = (function(){{ {js} }})();
    window.ipc.postMessage(JSON.stringify({{kind:"result",id:{id},ok:true,
      value: v === undefined ? null : v}}));
  }} catch (e) {{
    window.ipc.postMessage(JSON.stringify({{kind:"result",id:{id},ok:false,
      value: String(e && e.message || e)}}));
  }}
}})();"#
    )
}

/// ページを探す指定。CSS か XPath
#[derive(Debug, Clone)]
pub enum Sel {
    Css(String),
    Xpath(String),
}

impl Sel {
    fn json(&self) -> serde_json::Value {
        match self {
            Sel::Css(s) => serde_json::json!({ "css": s }),
            Sel::Xpath(s) => serde_json::json!({ "xpath": s }),
        }
    }
}

/// 要素の居場所。押す・入れるも同じ語彙で返す
/// (触れたなら届いていたので `Visible`)。
///
/// 「DOMに無い」と「あるが画面外」を分けるのが肝心で、
/// 前者はセレクタを、後者は待ちやスクロールを疑う。
/// 同じ「失敗」に潰すと、直す場所が分からなくなる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// 画面に見えている
    Visible,
    /// DOMにはあるが画面の外
    OffScreen,
    /// DOMに無い
    NotFound,
}

impl Found {
    pub fn as_str(self) -> &'static str {
        match self {
            Found::Visible => "visible",
            Found::OffScreen => "off_screen",
            Found::NotFound => "not_found",
        }
    }

    fn parse(json: &str) -> Self {
        match json.trim_matches('"') {
            "visible" => Found::Visible,
            "off_screen" => Found::OffScreen,
            _ => Found::NotFound,
        }
    }
}

/// JSの呼び出しを組み立てる。
///
/// **引数は必ずここを通す。** すべて `serde_json` で書き出すので、
/// 引用符も改行も外れず、渡した値がコードとして解釈されない。
/// AIの出力やページから読んだ文章をそのまま入れても、値のまま届く
fn call_js(func: &str, args: &[serde_json::Value]) -> String {
    let list: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    format!("return window.{func}({});", list.join(","))
}

fn ask_js(text: &str, label: &str) -> String {
    format!(
        "window.__shikisha_ask({}, {});",
        serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into()),
        serde_json::to_string(label).unwrap_or_else(|_| "\"OK\"".into())
    )
}

fn run_window(
    url: &str,
    title: &str,
    overlay: bool,
    proxy_tx: Sender<tao::event_loop::EventLoopProxy<Cmd>>,
    ev_tx: Sender<Ev>,
) -> Result<()> {
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::EventLoopBuilderExtWindows;
    use tao::window::WindowBuilder;
    use wry::WebViewBuilder;

    // TUIの描画ループとは別スレッドなので、メインスレッド縛りを外す
    let mut ev_loop = EventLoopBuilder::<Cmd>::with_user_event()
        .with_any_thread(true)
        .build();
    proxy_tx
        .send(ev_loop.create_proxy())
        .map_err(|_| anyhow!("指揮者との接続に失敗"))?;

    let mut wb = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 900.0));
    if overlay {
        // 枠を付けない。閉じるボタンがあると、アプリが開いていると
        // 思っているのに窓が無い状態を作れてしまう。
        // タブに従うのだから、独立に動かす手段は無い方がいい
        wb = wb.with_decorations(false).with_visible(false);
    }
    let window = wb.build(&ev_loop)?;
    if overlay {
        // ターミナルに「所有される窓」にする。
        // これだけで最小化・復元・重なり順をOSが面倒を見てくれる
        if let Some(h) = host_window() {
            set_owner(&window, h.hwnd);
        }
    }

    let ipc = ev_tx.clone();
    let webview = WebViewBuilder::new()
        .with_url(url)
        .with_initialization_script(INIT_JS)
        .with_ipc_handler(move |req| {
            let body: &str = req.body();
            let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
                return;
            };
            let ev = match v.get("kind").and_then(|k| k.as_str()) {
                Some("ready") => Ev::Ready {
                    url: v
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
                Some("button") => Ev::Button,
                Some("copy") => Ev::Copy {
                    text: v
                        .get("text")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
                Some("paste") => Ev::Paste,
                Some("key") => Ev::Key {
                    text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
                    named: v.get("named").and_then(|x| x.as_str()).map(str::to_string),
                    ctrl: v.get("ctrl").and_then(|x| x.as_str()).map(str::to_string),
                },
                Some("result") => Ev::Result {
                    id: v.get("id").and_then(|i| i.as_u64()).unwrap_or(0),
                    ok: v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false),
                    value: v
                        .get("value")
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| "null".into()),
                },
                _ => return,
            };
            let _ = ipc.send(ev);
        })
        .build(&window)?;

    ev_loop.run_return(move |event, _, control| {
        *control = ControlFlow::Wait;
        match event {
            Event::UserEvent(cmd) => match cmd {
                Cmd::Open(u) => {
                    let _ = webview.load_url(&u);
                }
                Cmd::Eval { id, js } => {
                    let _ = webview.evaluate_script(&wrap_eval(id, &js));
                }
                Cmd::Ask { text, label } => {
                    let _ = webview.evaluate_script(&ask_js(&text, &label));
                }
                Cmd::Unask => {
                    let _ = webview
                        .evaluate_script("window.__shikisha_unask&&window.__shikisha_unask();");
                }
                Cmd::Fit { x, y, w, h } => {
                    window.set_outer_position(tao::dpi::PhysicalPosition::new(x, y));
                    window.set_inner_size(tao::dpi::PhysicalSize::new(w.max(1), h.max(1)));
                }
                Cmd::Show(on) => window.set_visible(on),
                Cmd::Close => {
                    *control = ControlFlow::Exit;
                }
            },
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control = ControlFlow::Exit;
            }
            _ => {}
        }
    });

    let _ = ev_tx.send(Ev::Closed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;


    /// 試験用のページを 127.0.0.1 で配る。
    /// file:/// は wry のIPCで落ちるので、本番と同じ http にする
    fn serve(body: &'static str) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    const PAGE: &str = r#"<!doctype html><meta charset=utf-8><body>
<div id=here>ここにいる</div>
<input id=q value="">
<textarea id=multi></textarea>
<button id=go onclick="document.getElementById('log').textContent='pushed'">押す</button>
<div id=log></div>
<table><tr><td>氏名</td><td id=name>山田</td></tr></table>
<div style="height:4000px"></div>
<div id=far>ずっと下</div>
<script>
  var fired = 0;
  document.getElementById('q').addEventListener('input', function(){ fired++; });
</script>"#;

    /// 探して、押して、入れて、読めること。
    ///
    ///   cargo test browser_page_ops -- --ignored --nocapture
    #[test]
    #[ignore]
    fn browser_page_ops() {
        let b = Browser::spawn(&serve(PAGE), "SHIKISHA-TERM ops probe").expect("窓が開かない");
        let t = 20_000;

        // 「DOMに無い」と「あるが画面外」を分けること。
        // 同じ失敗にすると、セレクタを疑うのか待ちを疑うのか分からなくなる
        assert_eq!(b.find(&Sel::Css("#here".into()), t).unwrap(), Found::Visible);
        assert_eq!(b.find(&Sel::Css("#far".into()), t).unwrap(), Found::OffScreen);
        assert_eq!(b.find(&Sel::Css("#nope".into()), t).unwrap(), Found::NotFound);

        // XPath: CSSでは書けない探し方 (ラベルの隣のセル)
        let name = b
            .text(&Sel::Xpath("//td[text()='氏名']/following-sibling::td".into()), t)
            .unwrap();
        assert_eq!(name.as_deref(), Some("山田"), "XPathで隣のセルが取れない");

        // 押す
        assert_eq!(b.click(&Sel::Css("#go".into()), t).unwrap(), Found::Visible);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            b.text(&Sel::Css("#log".into()), t).unwrap().as_deref(),
            Some("pushed"),
            "押した結果がページに出ていない"
        );

        // 入れる。値を書くだけでなく input が飛ぶこと
        // (Reactなどは飛ばさないと状態が動かない)
        assert_eq!(
            b.fill(&Sel::Css("#q".into()), "ふつうの値", t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(&Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some("ふつうの値")
        );
        let id = b.eval("return fired;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_millis(t)).unwrap(),
            "1",
            "input イベントが飛んでいない"
        );

        // ここが肝心: 値はコードにならないこと。
        // AIの出力やページから読んだ文章をそのまま入れても、値のまま届く
        let nasty = "'; window.__pwned = 1; //\"</script><img src=x onerror=alert(1)>\\";
        assert_eq!(
            b.fill(&Sel::Css("#q".into()), nasty, t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(&Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some(nasty),
            "値が一字一句そのまま入っていない"
        );

        // 改行を含む値。1行の input は改行を落とす (HTMLの仕様) ので、
        // 複数行を渡すなら textarea でなければならない。
        // 値が壊れたのではなく、入れ物が保持できないだけ
        let multi = format!("1行目\n2行目\t{nasty}");
        assert_eq!(
            b.fill(&Sel::Css("#multi".into()), &multi, t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(&Sel::Css("#multi".into()), t).unwrap().as_deref(),
            Some(multi.as_str()),
            "改行やタブを含む値が崩れている"
        );
        let id = b.eval("return typeof window.__pwned;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_millis(t)).unwrap(),
            "\"undefined\"",
            "渡した値がコードとして実行された"
        );

        // 解釈済みのHTML全文
        let html = b.html(t).unwrap();
        assert!(html.contains("ここにいる"), "HTMLが取れていない");
        assert!(html.len() > 200, "HTMLが短すぎる: {}", html.len());
        println!("HTML {} 文字 / すべて通過", html.chars().count());

        b.close().unwrap();
    }

    /// 開けないURLは入口で止めること。
    ///
    /// wry はページからのIPCでURLを `http::Uri` にして unwrap するので、
    /// file:/// や data: を開くと、初期化スクリプトが最初のメッセージを
    /// 送った瞬間にプロセスごと落ちる。実測で確認済み
    #[test]
    fn only_http_pages_are_opened() {
        assert!(is_openable("https://example.com/a"));
        assert!(is_openable("http://127.0.0.1:8080/"));

        assert!(!is_openable("file:///C:/tmp/a.html"), "file: は落ちる");
        assert!(!is_openable("data:text/html,<b>x"), "data: は落ちる");
        assert!(!is_openable("about:blank"));
        assert!(!is_openable("https://"), "ホストが無い");
        assert!(!is_openable(""));
        assert!(!is_openable("https://example.com/a\nhttps://evil"), "改行の混入");
    }

    /// 窓が開き、JSが動き、結果が返り、閉じてもアプリが死なないこと。
    ///
    ///   cargo test browser_round_trip -- --ignored --nocapture
    ///
    /// 最後の一点が肝心。tao の `run` は内部で process::exit を呼ぶので、
    /// 素直に書くと窓を閉じただけでTUIごと消える
    #[test]
    #[ignore]
    fn browser_round_trip() {
        // 本番と同じ経路で試す。file:/// は wry のIPCで落ちるので使えない
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = "<title>t</title><body><div id=aaa>hello</div>";
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");

        let b = Browser::spawn(&url, "SHIKISHA-TERM browser probe").expect("窓が開かない");

        let id = b.eval("return 40 + 2;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("結果なし");
        println!("eval(40+2) = {v}");
        assert_eq!(v, "42");

        let id = b.eval("return document.querySelector('#aaa').textContent;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("結果なし");
        println!("querySelector = {v}");
        assert_eq!(v, "\"hello\"");

        let id = b.eval("return document.documentElement.outerHTML.length;").unwrap();
        println!("HTML長 = {}", b.wait_result(id, Duration::from_secs(20)).unwrap());

        b.ask("ログインしてください", "できました").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let id = b.eval("return !!document.getElementById('__shikisha_bar');").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).unwrap();
        println!("帯が出ているか = {v}");
        assert_eq!(v, "true", "呼びかけの帯が出ていない");

        b.close().unwrap();
        std::thread::sleep(Duration::from_millis(600));
        println!("閉じてもここまで来た (プロセスは生きている)");
    }
}

/// 窓を、ターミナルの「所有される窓」にする。
///
/// 最小化・復元・重なり順は、これだけでOSが面倒を見る。
/// 自前で追いかけると、隠し忘れや前後の入れ替わりが必ずどこかで漏れる
fn set_owner(window: &tao::window::Window, owner: isize) {
    use tao::platform::windows::WindowExtWindows;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, SetWindowLongPtrW};
    let hwnd = window.hwnd() as HWND;
    if hwnd.is_null() {
        return;
    }
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner);
    }
}

/// ターミナルの中身が描かれている範囲 (枠と題字を除く)。
///
/// 枠ごと覆うと、最小化・最大化・閉じるボタンまで隠れてしまう
pub fn host_client_rect() -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
    let h = host_window()?;
    unsafe {
        let hwnd = h.hwnd as HWND;
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetClientRect(hwnd, &mut r) == 0 {
            return None;
        }
        let mut p = POINT { x: r.left, y: r.top };
        if ClientToScreen(hwnd, &mut p) == 0 {
            return None;
        }
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        (w > 0 && hh > 0).then_some((p.x, p.y, w, hh))
    }
}

/// 自分を映しているターミナルの窓と、その位置・大きさ。
///
/// ConPTY 配下では `GetConsoleWindow` が大きさ0の隠し窓を返す。
/// そこから `GA_ROOTOWNER` を辿ると本物の枠に出る
/// (実測: 隠し窓は pid が自分側、辿った先は WindowsTerminal の pid)
pub struct HostWindow {
    pub hwnd: isize,
}

/// ターミナルの窓を探す。見つからなければ重ねない (そのまま別窓で出す)
pub fn host_window() -> Option<HostWindow> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOTOWNER, GetAncestor, GetWindowRect, IsWindowVisible,
    };
    unsafe {
        let console = GetConsoleWindow();
        if console.is_null() {
            return None;
        }
        let host = GetAncestor(console, GA_ROOTOWNER);
        if host.is_null() || host == console || IsWindowVisible(host) == 0 {
            return None;
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(host, &mut r) == 0 {
            return None;
        }
        // 大きさ0は隠し窓。本物の枠ではない
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(HostWindow { hwnd: host as isize })
    }
}
