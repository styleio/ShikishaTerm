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

  window.__screen = function (html) {
    scr.innerHTML = html;
    if (!cellW) measure();
    frames++; bytes += html.length;
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


/// 端末の16色。既定はロゴの青に寄せた落ち着いた配色にする。
/// 黒地に彩度100%の純色を並べると、道具ではなく侵入された画面に見える
const PALETTE: [&str; 16] = [
    "#1b2027", "#ff6b6b", "#4ade80", "#ffc857", "#00aaff", "#c792ea", "#4ec9ff", "#c8d2dc",
    "#3a4552", "#ff8f8f", "#7ceaa4", "#ffd88a", "#5cc4ff", "#dcb0ff", "#8fe0ff", "#eef3f8",
];

/// 色番号をCSSの色に直す。
///
/// 0-15 は配色表、16-231 は6段階の立方体、232-255 は灰色の階段。
/// この並びは端末の決まりごとなので、こちらで変えられない
fn color_css(c: vt100::Color, fallback: &'static str) -> String {
    match c {
        vt100::Color::Default => fallback.to_string(),
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        vt100::Color::Idx(i) => match i {
            0..=15 => PALETTE[i as usize].to_string(),
            16..=231 => {
                let i = i - 16;
                let step = |v: u8| if v == 0 { 0u8 } else { 55 + v * 40 };
                format!(
                    "#{:02x}{:02x}{:02x}",
                    step(i / 36),
                    step((i / 6) % 6),
                    step(i % 6)
                )
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                format!("#{v:02x}{v:02x}{v:02x}")
            }
        },
    }
}

fn esc_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

/// 画面をHTMLにする。
///
/// 同じ見た目が続く間は1つのまとまりにまとめる。1マスずつ要素にすると
/// 50行×180桁で9000要素になり、毎フレームの書き換えが重くなる
pub fn screen_html(screen: &vt100::Screen) -> String {
    const FG: &str = "#e8eef4";
    const BG: &str = "transparent";
    let (rows, cols) = screen.size();
    let mut out = String::with_capacity(rows as usize * cols as usize * 2);
    for r in 0..rows {
        let mut open: Option<String> = None;
        let mut run = String::new();
        for c in 0..cols {
            let Some(cell) = screen.cell(r, c) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let (mut fg, mut bg) = (cell.fgcolor(), cell.bgcolor());
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut style = String::new();
            let fgc = color_css(fg, if cell.inverse() { BG } else { FG });
            if fgc != FG {
                style.push_str(&format!("color:{fgc};"));
            }
            let bgc = color_css(bg, if cell.inverse() { FG } else { BG });
            if bgc != BG {
                style.push_str(&format!("background:{bgc};"));
            }
            if cell.bold() {
                style.push_str("font-weight:700;");
            }
            if cell.dim() {
                style.push_str("opacity:.6;");
            }
            if cell.italic() {
                style.push_str("font-style:italic;");
            }
            if cell.underline() {
                style.push_str("text-decoration:underline;");
            }
            // 見た目が変わったところで区切る
            if open.as_deref() != Some(style.as_str()) {
                if let Some(prev) = open.take() {
                    flush_run(&mut out, &prev, &run);
                    run.clear();
                }
                open = Some(style);
            }
            let ch = cell.contents();
            if ch.is_empty() {
                run.push(' ');
            } else {
                esc_into(&mut run, ch);
            }
        }
        if let Some(prev) = open.take() {
            flush_run(&mut out, &prev, &run);
        }
        out.push('\n');
    }
    out
}

fn flush_run(out: &mut String, style: &str, run: &str) {
    if run.trim_end().is_empty() && style.is_empty() {
        out.push_str(run);
        return;
    }
    if style.is_empty() {
        out.push_str(run);
    } else {
        out.push_str("<span style=\"");
        out.push_str(style);
        out.push_str("\">");
        out.push_str(run);
        out.push_str("</span>");
    }
}


/// ratatui の色をCSSに直す。端末の16色と同じ配色表を使う
fn ui_color(c: ratatui::style::Color, fallback: &str) -> String {
    use ratatui::style::Color as C;
    match c {
        C::Reset => fallback.to_string(),
        C::Black => PALETTE[0].into(),
        C::Red => PALETTE[1].into(),
        C::Green => PALETTE[2].into(),
        C::Yellow => PALETTE[3].into(),
        C::Blue => PALETTE[4].into(),
        C::Magenta => PALETTE[5].into(),
        C::Cyan => PALETTE[6].into(),
        C::Gray => PALETTE[7].into(),
        C::DarkGray => PALETTE[8].into(),
        C::LightRed => PALETTE[9].into(),
        C::LightGreen => PALETTE[10].into(),
        C::LightYellow => PALETTE[11].into(),
        C::LightBlue => PALETTE[12].into(),
        C::LightMagenta => PALETTE[13].into(),
        C::LightCyan => PALETTE[14].into(),
        C::White => PALETTE[15].into(),
        C::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        C::Indexed(i) => color_css(vt100::Color::Idx(i), ""),
    }
}

/// 画面のマス目をHTMLにする。
///
/// 中身は `draw()` が作ったものをそのまま使う。書き直さないので、
/// INDEXもボールも稼働盤も、足した機能は自動で付いてくる
pub fn buffer_html(buf: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::Modifier;
    const FG: &str = "#e8eef4";
    const BG: &str = "transparent";
    let area = buf.area;
    let mut out = String::with_capacity(area.width as usize * area.height as usize * 2);
    for y in 0..area.height {
        let mut open: Option<String> = None;
        let mut run = String::new();
        let mut skip = 0u16;
        for x in 0..area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buf[(x, y)];
            let m = cell.modifier;
            let (mut fg, mut bg) = (cell.fg, cell.bg);
            if m.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut style = String::new();
            let f = ui_color(fg, if m.contains(Modifier::REVERSED) { BG } else { FG });
            if f != FG {
                style.push_str(&format!("color:{f};"));
            }
            let b = ui_color(bg, if m.contains(Modifier::REVERSED) { FG } else { BG });
            if b != BG {
                style.push_str(&format!("background:{b};"));
            }
            if m.contains(Modifier::BOLD) {
                style.push_str("font-weight:700;");
            }
            if m.contains(Modifier::DIM) {
                style.push_str("opacity:.6;");
            }
            if m.contains(Modifier::ITALIC) {
                style.push_str("font-style:italic;");
            }
            if m.contains(Modifier::UNDERLINED) {
                style.push_str("text-decoration:underline;");
            }
            if open.as_deref() != Some(style.as_str()) {
                if let Some(prev) = open.take() {
                    flush_run(&mut out, &prev, &run);
                    run.clear();
                }
                open = Some(style);
            }
            let sym = cell.symbol();
            if sym.is_empty() {
                run.push(' ');
            } else {
                esc_into(&mut run, sym);
                // 全角は2マスを占める。次のマスは飛ばさないと二重に出る
                if unicode_width::UnicodeWidthStr::width(sym) > 1 {
                    skip = 1;
                }
            }
        }
        if let Some(prev) = open.take() {
            flush_run(&mut out, &prev, &run);
        }
        out.push('\n');
    }
    out
}

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
                Ev::Resize { rows, cols, .. } => {
                    crate::append_hook_log(&format!("窓 {rows}行 {cols}桁"));
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
                (screen_html(s), r, c, !s.hide_cursor())
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

#[cfg(test)]
mod color_tests {
    use super::screen_html;

    fn render(input: &str) -> String {
        let mut p: vt100::Parser = vt100::Parser::new(3, 40, 0);
        p.process(input.as_bytes());
        screen_html(p.screen())
    }

    /// プログラムが出す色を、そのまま描くこと。
    ///
    /// ここまでは文字だけを送っていたので、ビルドの警告もgitの差分も
    /// AIの強調も、全部同じ灰色に見えていた
    #[test]
    fn colours_reach_the_screen() {
        let h = render("\x1b[31mred\x1b[0m plain");
        assert!(h.contains("color:#ff6b6b"), "前景色が出ていない: {h}");
        assert!(h.contains(">red<"), "色の中身が入っていない: {h}");
        assert!(h.contains("plain"), "色なしの部分が消えている: {h}");

        // 背景・太字・下線
        assert!(render("\x1b[44mx").contains("background:#00aaff"), "背景色");
        assert!(render("\x1b[1mx").contains("font-weight:700"), "太字");
        assert!(render("\x1b[4mx").contains("text-decoration:underline"), "下線");

        // 反転は前景と背景を入れ替える
        let inv = render("\x1b[7mx");
        assert!(inv.contains("background:") && inv.contains("color:"), "反転: {inv}");

        // 256色の立方体と灰色の階段
        assert!(render("\x1b[38;5;196mx").contains("color:#ff0000"), "立方体の赤");
        assert!(render("\x1b[38;5;232mx").contains("color:#080808"), "灰色の下端");
        // 24bit
        assert!(render("\x1b[38;2;18;52;86mx").contains("color:#123456"), "24bit色");
    }

    /// 画面の文字がHTMLとして解釈されないこと。
    ///
    /// プログラムの出力に `<script>` が現れるのは、ごく普通にある
    /// (HTMLを cat する、grep の結果、AIの回答)
    #[test]
    fn output_is_never_treated_as_markup() {
        let h = render("<script>alert(1)</script> & <b>");
        assert!(!h.contains("<script>"), "生のタグが残っている: {h}");
        assert!(h.contains("&lt;script&gt;"), "エスケープされていない: {h}");
        assert!(h.contains("&amp;"), "アンパサンドが素通り: {h}");
    }

    /// 同じ見た目は1つのまとまりにすること。
    ///
    /// 1マス1要素にすると 50行×180桁で9000要素になり、
    /// 書き換えのたびに重くなる
    #[test]
    fn runs_of_the_same_look_are_merged() {
        let h = render("\x1b[31maaaaaaaaaa");
        assert_eq!(h.matches("<span").count(), 1, "文字ごとに分かれている: {h}");

        // 見た目が変われば分かれる
        let h = render("\x1b[31ma\x1b[32mb\x1b[31mc");
        assert_eq!(h.matches("<span").count(), 3, "{h}");
    }
}
