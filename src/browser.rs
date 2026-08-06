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

  window.__shikisha = true;
  send({ kind: "ready", url: location.href });
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
    pub fn spawn(url: &str, title: &str) -> Result<Self> {
        if !is_openable(url) {
            return Err(anyhow!("開けないURLです: {url}"));
        }
        let (proxy_tx, proxy_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        let url = url.to_string();
        let title = title.to_string();

        std::thread::Builder::new()
            .name("shikisha-browser".into())
            .spawn(move || {
                if let Err(e) = run_window(&url, &title, proxy_tx, ev_tx.clone()) {
                    crate::append_hook_log(&format!("ブラウザを開けません: {e}"));
                    let _ = ev_tx.send(Ev::Closed);
                }
            })?;

        // 窓ができるまで待つ (作れなければ proxy は届かない)
        let proxy = proxy_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| anyhow!("ブラウザの起動が終わりません (WebView2 が無い可能性)"))?;

        Ok(Self {
            proxy,
            events: ev_rx,
            next_id: AtomicU64::new(1),
            pending_ask: std::sync::Mutex::new(None),
        })
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

    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 900.0))
        .build(&ev_loop)?;

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
