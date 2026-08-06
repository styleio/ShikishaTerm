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
  /* 入力を受ける場所。カーソルの上に重ねる。
     IMEの候補窓はこの要素の位置に出るので、置き場所がそのまま候補の位置になる。
     変換中の文字はブラウザが下線付きで描くので、こちらでは描かない */
  #kbd { position:absolute; border:0; padding:0; margin:0; outline:none;
    background:transparent; color:#e8eef4; caret-color:transparent;
    overflow:hidden; resize:none; white-space:pre;
    font:inherit; line-height:inherit; width:1px; }
  #stat { position:fixed; right:8px; bottom:6px; font-size:11px; color:#5a6a78; }
  /* カーソル。文字の上に重ねるので、下の文字は読めるまま残す */
  #cur { position:absolute; background:#00aaff; opacity:.75; pointer-events:none;
    animation:blink 1.06s step-end infinite; }
  @keyframes blink { 0%,50% { opacity:.75 } 50.01%,100% { opacity:0 } }
  /* 大きさを測るための見えない文字 */
  #probe, #tprobe { position:absolute; visibility:hidden; white-space:pre;
    left:0; top:0; }
</style></head><body>
<pre id="scr"></pre>
<div id="cur" hidden></div>
<pre id="probe">MMMMMMMMMM</pre>
<pre id="tprobe"></pre>
<textarea id="kbd" autocomplete="off" autocorrect="off" spellcheck="false"></textarea>
<div id="stat"></div>
<script>
  const send = o => window.ipc.postMessage(JSON.stringify(o));
  const scr = document.getElementById("scr");
  const kbd = document.getElementById("kbd");
  const stat = document.getElementById("stat");

  // 1マスの大きさは環境で変わるので、実際に描いて測る
  const probe = document.getElementById("probe");
  const cur = document.getElementById("cur");
  let cellW = 0, cellH = 0;
  // font の一括指定は、直せない組み合わせだと空文字になる。
  // 空を代入しても何も起きず、別のフォントで測ることになるので個別に写す
  function copyFont(el) {
    const c = getComputedStyle(scr);
    el.style.fontFamily = c.fontFamily;
    el.style.fontSize = c.fontSize;
    el.style.fontWeight = c.fontWeight;
    el.style.letterSpacing = c.letterSpacing;
  }
  function measure() {
    copyFont(probe);
    const r = probe.getBoundingClientRect();
    cellW = r.width / 10;
    cellH = parseFloat(getComputedStyle(scr).lineHeight) || r.height;
  }
  // 窓に何行何桁入るかを知らせる。
  // 相手はこの数を信じて折り返すので、食い違うと画面の外へ書き続ける。
  //
  // 測って確かめてある (1280px / 余白8px / 1マス7px → 180桁。
  // 181桁だと 1283px で入らない)。桁数の計算は合っている。
  // 窓を広げたときに古い行の折り返しが狭いままなのは別の話で、
  // vt100 が既存の行を組み直さないため
  let lastRC = "";
  function report() {
    measure();
    if (!cellW || !cellH) return;
    const pad = (parseFloat(getComputedStyle(scr).paddingLeft) || 0) * 2;
    const padV = (parseFloat(getComputedStyle(scr).paddingTop) || 0) * 2;
    const cols = Math.max(20, Math.floor((window.innerWidth - pad) / cellW));
    const rows = Math.max(5, Math.floor((window.innerHeight - padV) / cellH));
    const key = rows + "x" + cols;
    if (key === lastRC) return;
    lastRC = key;
    send({ kind: "resize", rows: rows, cols: cols });
  }
  let rt = 0;
  window.addEventListener("resize", () => {
    clearTimeout(rt);
    rt = setTimeout(report, 80);
  });

  let frames = 0, bytes = 0, t0 = performance.now();
  let curX = 8, curY = 8;
  window.__cursor = function (row, col, shown) {
    if (!cellW) measure();
    const pad = parseFloat(getComputedStyle(scr).paddingLeft) || 0;
    const padT = parseFloat(getComputedStyle(scr).paddingTop) || 0;
    curX = pad + col * cellW;
    curY = padT + row * cellH;
    // 入力欄をカーソルへ動かす。候補窓はこの要素に付いてくる
    kbd.style.left = curX + "px";
    kbd.style.top = curY + "px";
    kbd.style.height = cellH + "px";
    // 変換中は、ブラウザが描く未確定文字とぶつかるので四角は出さない
    cur.hidden = !shown || composing;
    if (!cur.hidden) {
      cur.style.left = curX + "px";
      cur.style.top = curY + "px";
      cur.style.width = cellW + "px";
      cur.style.height = cellH + "px";
    }
  };

  window.__screen = function (text) {
    scr.textContent = text;
    if (!cellW) measure();
    frames++; bytes += text.length;
    const s = (performance.now() - t0) / 1000;
    if (s >= 1) {
      stat.textContent = frames + " 描画/秒  " + Math.round(bytes / s / 1024) + " KB/秒";
      frames = 0; bytes = 0; t0 = performance.now();
    }
  };

  // 変換中は送らない。確定した文字だけを送る
  let composing = false;
  // 変換中の文字が入る幅を持たせる。1pxのままだと未確定文字が見えない。
  // 文字数からの見積もりは、全角と半角が混ざると必ず外れる。
  // 足りないと左へスクロールして先頭が切れるので、実際に描いて測る
  const tprobe = document.getElementById("tprobe");
  const widen = s => {
    copyFont(tprobe);
    tprobe.textContent = s || "";
    const need = tprobe.getBoundingClientRect().width + cellW * 2;
    // 窓からはみ出すなら、収まる位置まで左へ寄せる
    let left = curX;
    if (curX + need > window.innerWidth - 8) {
      left = Math.max(0, window.innerWidth - need - 8);
    }
    kbd.style.left = left + "px";
    // 下限は「そこから窓の右端まで」。測り違えても、
    // 画面に収まる長さならスクロールしない
    const room = Math.max(cellW, window.innerWidth - left - 8);
    kbd.style.width = Math.max(need, room) + "px";
  };
  kbd.addEventListener("compositionstart", () => {
    composing = true;
    cur.hidden = true;
    widen("");
  });
  kbd.addEventListener("compositionupdate", e => widen(e.data));
  kbd.addEventListener("compositionend", e => {
    composing = false;
    kbd.value = "";
    kbd.style.width = "1px";
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

  // PuTTY と同じ作法: 選んだ時点でコピーされる。Ctrl+C は要らない。
  // 選択が残っている間はフォーカスを奪わない (奪うと選択が消える)
  const focus = () => kbd.focus();
  document.addEventListener("mouseup", () => {
    const s = window.getSelection();
    const t = s ? s.toString() : "";
    if (t) { send({ kind:"copy", text:t }); return; }
    focus();
  });

  // 右クリックで貼り付け。ブラウザの右クリックメニューは出さない
  document.addEventListener("contextmenu", e => {
    e.preventDefault();
    send({ kind:"paste" });
    focus();
  });
  window.addEventListener("focus", focus);
  // 選択した文字は、そのままコピーできる (ブラウザの機能をそのまま使う)
  focus();
  measure();
  report();
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
    let mut last_cursor = (u16::MAX, u16::MAX, false);
    let mut last_draw = Instant::now();
    loop {
        for ev in win.drain() {
            match ev {
                Ev::Closed => return Ok(()),
                // 窓に合わせてPTYの大きさを直す。
                // これを伝えないと、相手は古い桁数のまま折り返す
                Ev::Resize { rows, cols } => {
                    let _ = tab.resize(rows, cols);
                }
                // 選んだ時点でコピーする (PuTTY と同じ)
                Ev::Copy { text } => {
                    if let Ok(mut c) = arboard::Clipboard::new() {
                        let _ = c.set_text(text);
                    }
                }
                // 右クリックで貼り付け。括弧貼り付けの判定は既存の処理が持っている
                Ev::Paste => {
                    let _ = crate::paste_clipboard(&tab);
                }
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
            let (now, cur_row, cur_col, cur_on) = {
                let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
                let s = p.screen();
                let (r, c) = s.cursor_position();
                (
                    crate::tab::visible_text(s),
                    r,
                    c,
                    !s.hide_cursor(),
                )
            };
            let cursor = (cur_row, cur_col, cur_on);
            if now != last || cursor != last_cursor {
                last_cursor = cursor;
                if now != last {
                    last = now;
                    let js = format!(
                        "return window.__screen({});",
                        serde_json::to_string(&last).unwrap_or_else(|_| "\"\"".into())
                    );
                    let _ = win.eval(&js);
                }
                let _ = win.eval(&format!(
                    "return window.__cursor({cur_row},{cur_col},{cur_on});"
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}
