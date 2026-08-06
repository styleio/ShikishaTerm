//! 自前の窓でターミナルを描く試作。
//!
//! 確かめたいのは2点だけ。
//!   1. 日本語入力 (変換中の表示・確定) が実用に耐えるか
//!   2. 大量出力に描画が追いつくか
//!
//! この2つが通らなければ、この道は選べない。だから先に測る。
//! 通れば、残りは積み上げでできる。
//!
//! 画面をHTMLにする考え方は remote.rs と同じ。将来は同じ実装を
//! 共有したい (今は「見せるだけ」を2回書いていて、片方だけ壊れたことがある)。

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::browser::{Browser, Ev};
use crate::tab::{Tab, TabOptions};

/// 窓の中身。ここで受けたキーは、そのままPTYへ流す。
///
/// 変換中 (composition) の間は送らない。1文字ずつ送ると
/// 「あ」を打つために a→あ の途中経過まで相手に届いてしまう
const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<title>SHIKISHA-TERM</title>
<style>
  html,body { margin:0; height:100%; background:#0a0c0e; color:#e8eef4;
    font-family:"Cascadia Mono","Consolas","Meiryo",monospace; }
  #scr { margin:0; padding:8px; white-space:pre; font-size:14px; line-height:1.25;
    tab-size:8; }
  /* 入力を受ける場所。見えないが focus は当てる。
     IME の候補窓はこの位置に出るので、画面外へ飛ばさない */
  #kbd { position:fixed; left:8px; top:8px; width:1px; height:1em;
    opacity:0; border:0; padding:0; background:transparent; color:transparent; }
  #stat { position:fixed; right:8px; bottom:6px; font-size:11px; color:#5a6a78; }
</style></head><body>
<pre id="scr"></pre>
<textarea id="kbd" autocomplete="off" autocorrect="off" spellcheck="false"></textarea>
<div id="stat"></div>
<script>
  const send = o => window.ipc.postMessage(JSON.stringify(o));
  const scr = document.getElementById("scr");
  const kbd = document.getElementById("kbd");
  const stat = document.getElementById("stat");

  let frames = 0, bytes = 0, t0 = performance.now();
  window.__screen = function (text) {
    scr.textContent = text;
    frames++; bytes += text.length;
    const s = (performance.now() - t0) / 1000;
    if (s >= 1) {
      stat.textContent = frames + " 描画/秒  " + Math.round(bytes / s / 1024) + " KB/秒";
      frames = 0; bytes = 0; t0 = performance.now();
    }
  };

  // 変換中は送らない。確定した文字だけを送る
  let composing = false;
  kbd.addEventListener("compositionstart", () => { composing = true; });
  kbd.addEventListener("compositionend", e => {
    composing = false;
    kbd.value = "";
    if (e.data) send({ kind: "key", text: e.data });
  });
  kbd.addEventListener("input", e => {
    if (composing || e.isComposing) return;
    const v = kbd.value;
    kbd.value = "";
    if (v) send({ kind: "key", text: v });
  });

  // 制御キーは名前で送り、変換はRust側で行う
  const NAMED = {
    Enter:"enter", Backspace:"bs", Tab:"tab", Escape:"esc", Delete:"del",
    ArrowUp:"up", ArrowDown:"down", ArrowRight:"right", ArrowLeft:"left",
    Home:"home", End:"end", PageUp:"pgup", PageDown:"pgdn",
  };
  kbd.addEventListener("keydown", e => {
    if (e.isComposing) return;
    const n = NAMED[e.key];
    if (n) { e.preventDefault(); send({ kind:"key", named:n }); return; }
    if (e.ctrlKey && e.key.length === 1) {
      e.preventDefault();
      send({ kind:"key", ctrl:e.key.toLowerCase() });
    }
  });

  // どこを押しても入力は受け続ける
  const focus = () => kbd.focus();
  document.addEventListener("mousedown", e => { setTimeout(focus, 0); });
  window.addEventListener("focus", focus);
  focus();
  send({ kind:"ready", url: location.href });
</script></body></html>"#;

/// 制御キーの名前を、端末のバイト列に直す
fn named_bytes(name: &str) -> &'static [u8] {
    match name {
        "enter" => b"\r",
        "bs" => b"\x7f",
        "tab" => b"\t",
        "esc" => b"\x1b",
        "del" => b"\x1b[3~",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pgup" => b"\x1b[5~",
        "pgdn" => b"\x1b[6~",
        _ => b"",
    }
}

/// Ctrl+文字 を制御コードに直す (Ctrl+C = 0x03)
fn ctrl_byte(c: char) -> Option<u8> {
    let u = c.to_ascii_uppercase() as u8;
    (b'@'..=b'_').contains(&u).then(|| u & 0x1f)
}

/// 試作を動かす。`cmd` は起動するコマンド (既定はPowerShell)
pub fn run(cmd: &[String]) -> Result<()> {
    // 画面を配る。file:// は wry のIPCで落ちるので、ローカルHTTPで出す
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("ローカルサーバーを開けません: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow::anyhow!("ポートが取れません"))?
        .port();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let r = tiny_http::Response::from_string(PAGE).with_header(
                tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/html; charset=utf-8"[..],
                )
                .expect("header"),
            );
            let _ = req.respond(r);
        }
    });

    let (rows, cols) = (40u16, 120u16);
    let argv: Vec<String> = if cmd.is_empty() {
        vec!["powershell.exe".to_string()]
    } else {
        cmd.to_vec()
    };
    let tab = Tab::spawn(
        "window".into(),
        &argv,
        None,
        rows,
        cols,
        TabOptions::default(),
    )?;

    let win = Browser::spawn(&format!("http://127.0.0.1:{port}/"), "SHIKISHA-TERM")?;

    let mut last = String::new();
    let mut last_draw = Instant::now();
    loop {
        for ev in win.drain() {
            match ev {
                Ev::Closed => return Ok(()),
                Ev::Key { text, named, ctrl } => {
                    if let Some(n) = named {
                        let _ = tab.write_passthrough(named_bytes(&n));
                    } else if let Some(c) = ctrl.and_then(|s| s.chars().next()).and_then(ctrl_byte)
                    {
                        let _ = tab.write_passthrough(&[c]);
                    } else if let Some(t) = text {
                        let _ = tab.write_passthrough(t.as_bytes());
                    }
                }
                _ => {}
            }
        }

        // 画面が変わったときだけ描き直す。
        // 毎回送ると、動いていない画面でもCPUを食う
        if last_draw.elapsed() >= Duration::from_millis(33) {
            last_draw = Instant::now();
            let now = {
                let p = tab.parser.lock().unwrap();
                crate::tab::visible_text(p.screen())
            };
            if now != last {
                last = now;
                let js = format!(
                    "return window.__screen({});",
                    serde_json::to_string(&last).unwrap_or_else(|_| "\"\"".into())
                );
                let _ = win.eval(&js);
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}
