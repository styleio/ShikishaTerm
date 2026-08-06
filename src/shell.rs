//! 窓の外皮。タブバー、稼働盤、ステータス。
//!
//! ここは本物のHTMLで書く。ratatui のマス目を変換すると早いが、
//! それだと窓を持った意味の半分が消える:
//!   - ボールが永遠に文字の ● のまま
//!   - 出力を選ぶとタブバーと罫線まで付いてくる (全部が1枚のマス目なので)
//!   - スマホ表示との重複が消えない
//!
//! ターミナルの中身だけはマス目のままでいい。あれは本当にマス目だから。
//!
//! 状態は uistate から来る。ここには「何が起きているか」を書かず、
//! 「どう見せるか」だけを書く。

/// 外皮のページ。`{{DICT}}` に訳語、`{{BUILD}}` にビルド刻印が入る
pub const PAGE: &str = r####"<!doctype html><html><head><meta charset="utf-8">
<title>SHIKISHA-TERM</title>
<style>
  :root {
    --bg:#0a0c0e; --panel:#11151a; --line:#1d2630; --text:#e8eef4;
    --dim:#7a8896; --brand:#00aaff; --live:#4ade80; --warn:#ffc857; --stop:#ff6b6b;
    --mono:"Cascadia Mono","Consolas","Meiryo",monospace;
  }
  * { box-sizing:border-box; }
  html,body { margin:0; height:100%; overflow:hidden;
    background:var(--bg); color:var(--text); font-family:var(--mono); font-size:14px; }
  #app { display:grid; grid-template-columns:auto 1fr; grid-template-rows:1fr auto;
    height:100%; }

  /* ── 左のタブバー ───────────────────────── */
  #tabs { grid-row:1/3; width:210px; background:var(--panel);
    border-right:1px solid var(--line); overflow-y:auto; padding:6px 0; }
  .tab { display:flex; align-items:center; gap:8px; padding:7px 10px;
    cursor:pointer; border-left:3px solid transparent; user-select:none; }
  .tab:hover { background:#161c23; }
  .tab.sel { background:#16202b; border-left-color:var(--brand); }
  .dot { width:8px; height:8px; border-radius:50%; flex:none; background:var(--dim); }
  .dot.BUSY, .dot.Working { background:var(--live); animation:pulse 1.2s ease-in-out infinite; }
  .dot.DONE { background:var(--brand); }
  .dot.ASK  { background:var(--warn); }
  .dot.EXIT { background:var(--stop); }
  @keyframes pulse { 0%,100% { opacity:1 } 50% { opacity:.35 } }
  .num { color:var(--dim); font-size:12px; min-width:14px; }
  .nm { flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .lock { color:var(--warn); font-size:11px; }
  /* 出力量。文字ではなく本物の棒 */
  .spark { display:flex; align-items:flex-end; gap:1px; height:14px; flex:none; }
  .spark i { width:2px; background:var(--brand); opacity:.75; }

  /* ── 中身 ───────────────────────────────── */
  #main { position:relative; overflow:hidden; }
  #screen { position:absolute; inset:0; margin:0; padding:8px; white-space:pre;
    overflow:auto; line-height:1.25; }
  /* ここだけ選べる。タブバーや枠は選択に混ざらない */
  #screen { user-select:text; }

  /* ── 稼働盤 ─────────────────────────────── */
  #board { position:absolute; inset:0; overflow:auto; padding:22px 26px; }
  .mark { color:var(--brand); font-weight:700; letter-spacing:.5px;
    font-size:13px; line-height:1.15; white-space:pre; }
  .sub { color:var(--dim); font-size:12px; margin-top:4px; }
  .card { margin-top:20px; border:1px solid var(--line); border-radius:10px;
    background:var(--panel); padding:14px 16px; }
  .card h2 { margin:0 0 10px; font-size:12px; font-weight:600; color:var(--dim);
    letter-spacing:1px; text-transform:uppercase; }
  /* 連鎖のゲージ。文字の ━ ではなく本物の帯 */
  .gauge { height:8px; border-radius:4px; background:#141a21; overflow:hidden; }
  .gauge i { display:block; height:100%; background:var(--live);
    transition:width .3s ease, background .3s ease; }
  .rows { width:100%; border-collapse:collapse; font-size:13px; }
  .rows th { text-align:left; color:var(--dim); font-weight:600; font-size:11px;
    letter-spacing:1px; padding:0 8px 6px; }
  .rows td { padding:5px 8px; border-top:1px solid var(--line); }
  .rows tr { cursor:pointer; }
  .rows tr:hover td { background:#161c23; }
  .menu { display:grid; grid-template-columns:repeat(auto-fill,minmax(230px,1fr));
    gap:6px; }
  .mi { display:flex; gap:9px; align-items:center; padding:7px 9px; border-radius:7px;
    cursor:pointer; }
  .mi:hover { background:#161c23; }
  .key { font-size:11px; color:#04121c; background:var(--brand); border-radius:4px;
    padding:1px 6px; font-weight:700; }

  /* ── ボール。本物の円が本当に動く ────────── */
  #lanes { position:relative; height:44px; margin-top:6px; }
  #ball { position:absolute; width:14px; height:14px; border-radius:50%;
    background:var(--live); box-shadow:0 0 12px var(--live);
    transition:left .35s cubic-bezier(.4,1.4,.5,1), top .35s ease, background .3s;
    transform:translate(-50%,-50%); }
  #ball.human { background:var(--brand); box-shadow:0 0 12px var(--brand); }
  #ball.wait { animation:pulse 1s ease-in-out infinite; }
  .lane { position:absolute; top:50%; height:2px; background:var(--line);
    transform:translateY(-50%); }
  .peg { position:absolute; top:50%; width:7px; height:7px; border-radius:50%;
    background:#26313d; transform:translate(-50%,-50%); }
  .peg b { position:absolute; top:12px; left:50%; transform:translateX(-50%);
    font-size:10px; color:var(--dim); font-weight:400; white-space:nowrap; }

  /* ── ステータス ─────────────────────────── */
  #status { grid-column:2; display:flex; align-items:center; gap:14px;
    padding:5px 12px; border-top:1px solid var(--line); background:var(--panel);
    font-size:12px; color:var(--dim); }
  #status .grow { flex:1; }
  .pill { padding:1px 8px; border-radius:9px; border:1px solid var(--line); }
  .pill.on { color:var(--live); border-color:#1f3d2b; }
  .pill.off { color:var(--dim); }
  #stop { cursor:pointer; color:var(--stop); border:1px solid #3d2020;
    padding:2px 10px; border-radius:7px; font-weight:700; }
  #stop:hover { background:var(--stop); color:#0a0c0e; }
  /* 入力を受ける場所。カーソルに重ねる。
     IMEの候補窓はこの要素に付いてくるので、置き場所がそのまま候補の位置になる。
     変換中の文字はブラウザが下線付きで描くので、こちらでは描かない */
  #kbd { position:absolute; border:0; padding:0; margin:0; outline:none;
    background:transparent; color:var(--text); caret-color:transparent;
    overflow:hidden; resize:none; white-space:pre; font:inherit;
    line-height:inherit; width:1px; }
  #cur { position:absolute; background:var(--brand); opacity:.75;
    pointer-events:none; animation:blink 1.06s step-end infinite; }
  @keyframes blink { 0%,50% { opacity:.75 } 50.01%,100% { opacity:0 } }
  #probe, #tprobe { position:absolute; visibility:hidden; white-space:pre;
    left:0; top:0; margin:0; }
  /* 覆いかぶさる画面。押せる場所を見失わないよう、外は暗くする */
  #veil { position:fixed; inset:0; background:#00000099; display:flex;
    align-items:center; justify-content:center; z-index:50; }
  /* hidden は既定で display:none にするが、自分で display を書くと
     そちらが勝つ。書いた以上、消す指定も自分で持つ */
  #veil[hidden] { display:none; }

  #veil .box { background:var(--panel); border:1px solid var(--brand);
    border-radius:12px; padding:20px 24px; max-width:min(760px,86vw);
    max-height:84vh; overflow:auto; }
  #veil h3 { margin:0 0 12px; font-size:13px; color:var(--brand);
    letter-spacing:1px; text-transform:uppercase; }
  #veil .row { display:flex; gap:10px; align-items:center; padding:5px 0;
    font-size:13px; }
  #veil .pick { cursor:pointer; padding:7px 10px; border-radius:7px; }
  #veil .pick:hover { background:#16202b; }
  #veil .qr { background:#fff; padding:12px; border-radius:8px; }
  #veil .url { font-size:12px; color:var(--dim); margin-top:10px;
    word-break:break-all; user-select:text; }
  #flash { position:absolute; left:50%; bottom:52px; transform:translateX(-50%);
    background:#16202b; border:1px solid var(--brand); color:var(--text);
    padding:8px 16px; border-radius:8px; font-size:13px; }
</style></head><body>
<div id="app">
  <nav id="tabs"></nav>
  <div id="main">
    <div id="board" hidden></div>
    <pre id="screen" hidden></pre>
    <div id="cur" hidden></div>
    <textarea id="kbd" autocomplete="off" autocorrect="off" spellcheck="false"></textarea>
    <pre id="probe">MMMMMMMMMM</pre>
    <pre id="tprobe"></pre>
    <div id="flash" hidden></div>
  </div>
  <div id="veil" hidden></div>
  <div id="status"></div>
</div>
<script>
// 画面の中で失敗したら知らせる。黙って止まると、外からは
// 「出るはずのものが出ない」としか見えない
window.onerror = function (msg, src, line, col) {
  try {
    window.ipc.postMessage(JSON.stringify(
      {kind:"jserror", msg:String(msg) + " @" + line + ":" + col}));
  } catch (e) {}
};
const T = {{DICT}};
const BUILD = {{BUILD}};
const TOKEN = {{TOKEN}};
// 窓の中なら直に渡せる。スマホからはHTTPで届ける。
// ページは同じものを使う (見た目を2回書かないため)
const REMOTE = !window.ipc;
const send = REMOTE
  ? (o => fetch("api/intent?t=" + encodeURIComponent(TOKEN),
      {method:"POST", body:JSON.stringify(o)}).catch(() => {}))
  : (o => window.ipc.postMessage(JSON.stringify(o)));
const el = (t, a, ...kids) => {
  const n = document.createElement(t);
  for (const k in (a||{})) {
    if (k === "class") n.className = a[k];
    else if (k.startsWith("on")) n[k] = a[k];
    else if (a[k] !== null && a[k] !== undefined) n.setAttribute(k, a[k]);
  }
  for (const c of kids) if (c !== null && c !== undefined) n.append(c);
  return n;
};

let S = null;   // 直近の状態

// ── 左のタブバー ────────────────────────────
function drawTabs() {
  const nav = document.getElementById("tabs");
  nav.textContent = "";
  nav.append(el("div", {class:"tab" + (S.active === 0 ? " sel" : ""),
      onclick:() => send({kind:"select", tab:0})},
    el("span", {class:"num"}, "0"),
    el("span", {class:"nm"}, T["tui.index"] || "INDEX")));
  for (const t of S.tabs) {
    nav.append(el("div", {class:"tab" + (S.active === t.index ? " sel" : ""),
        onclick:() => send({kind:"select", tab:t.index})},
      el("span", {class:"dot " + t.state}),
      el("span", {class:"num"}, String(t.index)),
      el("span", {class:"nm", title:t.profile}, t.name),
      t.locked ? el("span", {class:"lock"}, "\u{1F512}") : null,
      spark(t.activity)));
  }
}

// 出力量を本物の棒で描く。文字の ▁▄█ ではない
function spark(a) {
  const box = el("div", {class:"spark"});
  for (const v of (a || []).slice(-10)) {
    const b = el("i");
    b.style.height = Math.max(1, v * 2) + "px";
    box.append(b);
  }
  return box;
}

// ── 稼働盤 ──────────────────────────────────
const WORDMARK = [
  "█▀▀ █ █ █ █ █ █ █▀▀ █ █ █▀█    ▀█▀ █▀▀ █▀█ █▄█",
  "▀▀█ █▀█ █ █▀▄ █ ▀▀█ █▀█ █▀█ ▀▀  █  █▀▀ █▀▄ █ █",
  "▀▀▀ ▀ ▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀     ▀  ▀▀▀ ▀ ▀ ▀ ▀",
];

function drawBoard() {
  const b = document.getElementById("board");
  b.textContent = "";
  b.append(el("div", {class:"mark"}, WORDMARK.join("\n")),
           el("div", {class:"sub"}, (S.workspace || "") + "   " + BUILD));

  // 連鎖
  const heat = S.ball.max ? Math.min(1, S.ball.depth / S.ball.max) : 0;
  const bar = el("i");
  bar.style.width = Math.round(heat * 100) + "%";
  bar.style.background = heat >= .8 ? "var(--stop)" : heat >= .5 ? "var(--warn)" : "var(--live)";
  b.append(el("div", {class:"card"},
    el("h2", {}, T["tui.chain"] || "CHAIN"),
    el("div", {class:"gauge"}, bar),
    el("div", {class:"sub"}, S.ball.depth + " / " + S.ball.max),
    lanes()));

  // タブ一覧
  const rows = el("table", {class:"rows"});
  rows.append(el("tr", {},
    el("th", {}, "#"), el("th", {}, T["tui.col.name"] || "NAME"),
    el("th", {}, T["tui.col.state"] || "STATE"),
    el("th", {}, T["tui.col.profile"] || "PROFILE"),
    el("th", {}, T["tui.col.activity"] || "ACTIVITY")));
  for (const t of S.tabs) {
    rows.append(el("tr", {onclick:() => send({kind:"select", tab:t.index})},
      el("td", {}, String(t.index)),
      el("td", {}, t.name),
      el("td", {}, el("span", {class:"dot " + t.state}), " " + t.state),
      el("td", {}, t.profile),
      el("td", {}, spark(t.activity))));
  }
  b.append(el("div", {class:"card"}, el("h2", {}, "SESSIONS"), rows));

  // メニュー
  const items = [
    ["e", T["tui.menu.settings"]], ["i", T["tui.menu.phone"]],
    ["r", T["tui.menu.restart"]], ["w", T["tui.menu.workspace"]],
    ["t", T["tui.menu.notify"]], ["?", T["tui.menu.help"]],
  ];
  const m = el("div", {class:"menu"});
  for (const [k, label] of items) {
    if (!label) continue;
    m.append(el("div", {class:"mi", onclick:() => send({kind:"menu", key:k})},
      el("span", {class:"key"}, k), el("span", {}, label)));
  }
  b.append(el("div", {class:"card"}, el("h2", {}, "MENU"), m));
}

// ボールの通り道。人(0)と各タブを並べ、円を実際に動かす
function lanes() {
  const box = el("div", {id:"lanes"});
  const n = S.tabs.length + 1;
  const line = el("div", {class:"lane"});
  line.style.left = "6%"; line.style.right = "6%";
  box.append(line);
  const at = i => 6 + (n <= 1 ? 44 : (i * 88) / (n - 1));
  for (let i = 0; i < n; i++) {
    const p = el("div", {class:"peg"},
      el("b", {}, i === 0 ? (T["tui.human"] || "YOU") : S.tabs[i-1].name));
    p.style.left = at(i) + "%";
    box.append(p);
  }
  const ball = el("div", {id:"ball"});
  ball.style.left = at(S.ball.holder) + "%";
  ball.style.top = "50%";
  if (S.ball.holder === 0) ball.className = "human";
  if (S.ball.awaiting_human) ball.className += " wait";
  box.append(ball);
  return box;
}

// ── ステータス ──────────────────────────────
function drawStatus() {
  const s = document.getElementById("status");
  s.textContent = "";
  s.append(
    el("span", {}, S.workspace || ""),
    el("span", {class:"pill " + (S.auto_enabled ? "on" : "off")},
      "AUTO " + (S.auto_enabled ? "ON" : "OFF")),
    S.remote_on ? el("span", {class:"pill on"}, "REMOTE") : null,
    el("span", {class:"grow"}),
    el("span", {}, BUILD),
    el("span", {id:"stop", onclick:() => send({kind:"stop"})},
      T["tui.stop"] || "STOP"));
}

// ── 受け口 ──────────────────────────────────
// 覆いかぶさる画面。閉じるのは Esc かどこかを押すこと
function drawVeil() {
  const v = document.getElementById("veil");
  const shown = S.help_open || S.ws_open || !!S.qr;
  v.hidden = !shown;
  if (!shown) return;
  v.textContent = "";
  const box = el("div", {class:"box"});
  if (S.ws_open) {
    box.append(el("h3", {}, T["tui.workspace"] || "WORKSPACE"));
    S.workspaces.forEach((w, i) => {
      box.append(el("div", {class:"pick" + (i === S.ws_index ? " sel" : ""),
        onclick:() => send({kind:"menu", key:String(i + 1)})},
        (i + 1) + ".  " + w));
    });
  } else if (S.qr) {
    box.append(el("h3", {}, T["tui.menu.phone"] || "PHONE"));
    // QRはRust側が作った画像をそのまま出す (作り方を2箇所に持たない)
    const img = el("img", {class:"qr", src:"qr.svg?u=" + encodeURIComponent(S.qr)});
    box.append(img, el("div", {class:"url"}, S.qr));
  } else {
    box.append(el("h3", {}, T["tui.menu.help"] || "HELP"));
    for (const line of (T["tui.help.body"] || "").split("\n")) {
      box.append(el("div", {class:"row"}, line));
    }
  }
  v.onclick = () => send({kind:"key", named:"esc"});
  v.append(box);
}

// パスワードを聞く。スマホには出さない。
// 使う場面が無いうえ、同じページを配っているので、
// 出せば公開設定を開けた人のところにも出てしまう
window.__password = function (title, note) {
  if (REMOTE) { send({kind:"password"}); return; }
  const v = document.getElementById("veil");
  v.hidden = false;
  v.textContent = "";
  const box = el("div", {class:"box"});
  const inp = el("input", {type:"password", autocomplete:"off"});
  inp.style.cssText = "font:inherit;background:#0a0c0e;color:var(--text);" +
    "border:1px solid var(--line);border-radius:6px;padding:8px 10px;width:320px";
  const done = t => { v.hidden = true; v.onclick = null; send({kind:"password", text:t}); };
  inp.onkeydown = e => {
    if (e.key === "Enter") { e.preventDefault(); done(inp.value); }
    if (e.key === "Escape") { e.preventDefault(); done(null); }
  };
  box.append(el("h3", {}, title), note ? el("div", {class:"row"}, note) : null, inp);
  v.onclick = e => { if (e.target === v) done(null); };
  v.append(box);
  inp.focus();
};

window.__state = function (json) {
  S = JSON.parse(json);
  drawTabs();
  drawStatus();
  const board = document.getElementById("board");
  const screen = document.getElementById("screen");
  board.hidden = S.active !== 0;
  screen.hidden = S.active === 0;
  if (S.active === 0) drawBoard();
  drawVeil();
  const f = document.getElementById("flash");
  f.hidden = !S.flash;
  if (S.flash) f.textContent = S.flash;
};

// ターミナルの中身。ここだけはマス目で正しいので、そのまま受ける
window.__screen = function (html) {
  document.getElementById("screen").innerHTML = html;
};

// ── ここから入力 ────────────────────────────
// ターミネルの中身だけがマス目なので、測るのも重ねるのもここに限る
const scr = document.getElementById("screen");
const kbd = document.getElementById("kbd");
const cur = document.getElementById("cur");
const probe = document.getElementById("probe");
const tprobe = document.getElementById("tprobe");
let cellW = 0, cellH = 0, curX = 8, curY = 8, composing = false;

// font の一括指定は、直せない組み合わせだと空文字になる。
// 空を代入しても何も起きず、別のフォントで測ることになるので個別に写す
function copyFont(el2) {
  const c = getComputedStyle(scr);
  el2.style.fontFamily = c.fontFamily;
  el2.style.fontSize = c.fontSize;
  el2.style.fontWeight = c.fontWeight;
  el2.style.letterSpacing = c.letterSpacing;
}
function measure() {
  copyFont(probe);
  const r = probe.getBoundingClientRect();
  cellW = r.width / 10;
  cellH = parseFloat(getComputedStyle(scr).lineHeight) || r.height;
}

// 窓に何行何桁入るかを知らせる。
// 相手はこの数を信じて折り返すので、食い違うと画面の外へ書き続ける
let lastRC = "";
function report() {
  measure();
  if (!cellW || !cellH) return;
  const box = document.getElementById("main").getBoundingClientRect();
  const pad = (parseFloat(getComputedStyle(scr).paddingLeft) || 0) * 2;
  const cols = Math.max(20, Math.floor((box.width - pad) / cellW));
  const rows = Math.max(5, Math.floor((box.height - pad) / cellH));
  // ブラウザを置く場所。外皮のCSSを変えても、知っているのは
  // ページなので、Rust側で座標を推測させない
  const key = rows + "x" + cols + "@" + Math.round(box.left) + "," +
    Math.round(box.top) + "," + Math.round(box.width) + "," + Math.round(box.height);
  if (key === lastRC) return;
  lastRC = key;
  send({kind:"resize", rows:rows, cols:cols,
    area:[Math.round(box.left), Math.round(box.top),
          Math.round(box.width), Math.round(box.height)]});
}
let rt = 0;
window.addEventListener("resize", () => { clearTimeout(rt); rt = setTimeout(report, 80); });

window.__cursor = function (row, col, shown) {
  if (!cellW) measure();
  const pad = parseFloat(getComputedStyle(scr).paddingLeft) || 0;
  const padT = parseFloat(getComputedStyle(scr).paddingTop) || 0;
  const box = scr.getBoundingClientRect();
  curX = box.left + pad + col * cellW;
  curY = box.top + padT + row * cellH;
  kbd.style.left = curX + "px";
  kbd.style.top = curY + "px";
  kbd.style.height = cellH + "px";
  cur.hidden = !shown || composing || S === null || S.active === 0;
  if (!cur.hidden) {
    cur.style.left = curX + "px";
    cur.style.top = curY + "px";
    cur.style.width = cellW + "px";
    cur.style.height = cellH + "px";
  }
};

// 変換中は送らない。確定した文字だけを送る。
// 幅は文字数から見積もらず実際に描いて測る (全角と半角が混ざると必ず外れる)
function widen(s) {
  copyFont(tprobe);
  tprobe.textContent = s || "";
  const need = tprobe.getBoundingClientRect().width + cellW * 2;
  let left = curX;
  if (curX + need > window.innerWidth - 8) {
    left = Math.max(0, window.innerWidth - need - 8);
  }
  kbd.style.left = left + "px";
  const room = Math.max(cellW, window.innerWidth - left - 8);
  kbd.style.width = Math.max(need, room) + "px";
}
kbd.addEventListener("compositionstart", () => { composing = true; cur.hidden = true; widen(""); });
kbd.addEventListener("compositionupdate", e => widen(e.data));
kbd.addEventListener("compositionend", e => {
  composing = false;
  kbd.value = "";
  kbd.style.width = "1px";
  if (e.data) send({kind:"key", text:e.data});
});
kbd.addEventListener("input", e => {
  if (composing || e.isComposing) return;
  const v = kbd.value;
  kbd.value = "";
  if (v) send({kind:"key", text:v});
});

const NAMED = {
  Enter:"enter", Backspace:"bs", Tab:"tab", Escape:"esc", Delete:"del",
  ArrowUp:"up", ArrowDown:"down", ArrowRight:"right", ArrowLeft:"left",
  Home:"home", End:"end", PageUp:"pgup", PageDown:"pgdn",
  F1:"f1", F2:"f2", F3:"f3", F4:"f4", F5:"f5", F6:"f6",
  F7:"f7", F8:"f8", F9:"f9", F10:"f10", F11:"f11", F12:"f12",
};
kbd.addEventListener("keydown", e => {
  if (e.isComposing) return;
  const nm = NAMED[e.key];
  if (nm) { e.preventDefault(); send({kind:"key", named:nm}); return; }
  if (e.ctrlKey && e.key.length === 1) {
    e.preventDefault();
    send({kind:"key", ctrl:e.key.toLowerCase()});
  }
});

// PuTTY と同じ作法: 選んだ時点でコピーされる。右クリックで貼り付け
const focus = () => kbd.focus();
document.addEventListener("mouseup", () => {
  const s = window.getSelection();
  const t = s ? s.toString() : "";
  if (t) { send({kind:"copy", text:t}); return; }
  focus();
});
document.addEventListener("contextmenu", e => {
  e.preventDefault();
  send({kind:"paste"});
  focus();
});
window.addEventListener("focus", focus);
focus();
measure();
report();

// スマホからは押しに行く。窓へは向こうから届く
if (REMOTE) {
  const pull = async () => {
    try {
      const r = await fetch("api/state?t=" + encodeURIComponent(TOKEN),
        {cache:"no-store"});
      const d = await r.json();
      if (d.ui) window.__state(JSON.stringify(d.ui));
      if (d.screen_html) window.__screen(d.screen_html);
    } catch (e) {}
  };
  pull();
  setInterval(pull, 900);
}

send({kind:"ready"});
</script></body></html>"####;

// ── ターミナルの中身 ────────────────────────────
// ここだけはマス目のままでいい。あれは本当にマス目だから。
// 外皮 (タブバー・盤面) は本物のHTMLで書いてある

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

/// 訳語とビルド刻印を埋めて、配れる形にする
pub fn page(token: &str) -> String {
    let dict = crate::i18n::dict_json();
    PAGE.replace("{{DICT}}", &dict)
        .replace(
            "{{TOKEN}}",
            &serde_json::to_string(token).unwrap_or_else(|_| "\"\"".into()),
        )
        .replace(
        "{{BUILD}}",
        &serde_json::to_string(&format!(
            "build {}  ({})",
            env!("BUILD_TIME"),
            env!("BUILD_REV")
        ))
        .unwrap_or_else(|_| "\"\"".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::PAGE;


    /// 配る形に、埋め忘れが残っていないこと。
    ///
    /// 差し込み先が残ったままだと、ページ全体がSyntaxErrorになり、
    /// 画面には何も出ないまま原因が見えない
    #[test]
    fn the_page_has_nothing_left_to_fill_in() {
        // 言語は初期化しない。ここで init すると、並行して走る
        // 他の試験の言語まで変わる (盤面の試験が CHAIN を探して落ちた)
        let p = super::page("");
        assert!(!p.contains("{{"), "差し込み先が残っている");
        assert!(p.contains("const T = {"), "訳語が入っていない");
        assert!(p.contains("const BUILD = \""), "ビルド刻印が入っていない");
    }


    /// hidden を付けた要素が、本当に隠れること。
    ///
    /// HTMLの hidden は既定で display:none にする規則だが、
    /// 自分で display を書くとそちらが勝ち、隠れなくなる。
    /// 覆いが出っぱなしになり、画面が暗いまま何も押せなくなった。
    ///
    /// 書き方の問題なので、見た目を確かめないと気づけない。
    /// だからここで押さえる
    #[test]
    fn things_marked_hidden_are_actually_hidden() {
        // markup で hidden を付けている id を拾う
        let mut ids: Vec<String> = Vec::new();
        for line in PAGE.lines() {
            let t = line.trim();
            if !t.contains("hidden") || !t.contains("id=\"") {
                continue;
            }
            if let Some(rest) = t.split("id=\"").nth(1) {
                if let Some(id) = rest.split('"').next() {
                    ids.push(id.to_string());
                }
            }
        }
        assert!(!ids.is_empty(), "hidden を使っている要素が見つからない");

        for id in ids {
            // その id に display を指定しているか
            let sets_display = PAGE.lines().any(|l| {
                let t = l.trim_start();
                t.starts_with(&format!("#{id} "))
                    && !t.contains("[hidden]")
                    && t.contains("display:")
            });
            if sets_display {
                assert!(
                    PAGE.contains(&format!("#{id}[hidden]")),
                    "#{id} は display を書いているのに、hidden で消す指定が無い"
                );
            }
        }
    }

    /// 同じ名前を2回宣言していないこと。
    ///
    /// 以前これで設定ページが丸ごと動かなくなった。宣言が重なると
    /// SyntaxError になり、スクリプト全体が実行されない。
    /// 画面には見出しだけが残り、原因がまるで見えない
    #[test]
    fn nothing_is_declared_twice() {
        let mut seen: Vec<&str> = Vec::new();
        for line in PAGE.lines() {
            let t = line.trim_start();
            for kw in ["const ", "let ", "function "] {
                let Some(rest) = t.strip_prefix(kw) else {
                    continue;
                };
                // 字下げのある行は関数の中なので、重なっても構わない
                if line.starts_with(' ') {
                    continue;
                }
                let name: &str = rest
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                assert!(!seen.contains(&name), "{name} が2回宣言されている");
                seen.push(name);
            }
        }
        assert!(seen.contains(&"send"), "走査が効いていない");
    }

    /// 状態のどの項目も、画面のどこかで使われていること。
    ///
    /// 送っているのに誰も見ていない項目があると、
    /// 「出るはずのものが出ない」を探すとき最初に疑う場所になる
    #[test]
    fn every_piece_of_state_is_used() {
        for field in [
            "workspace", "active", "auto_enabled", "remote_on", "tabs", "flash",
            "holder", "depth", "max", "awaiting_human", "locked", "profile",
            "activity", "state", "name", "index",
        ] {
            assert!(PAGE.contains(field), "状態の {field} を誰も見ていない");
        }
    }

    /// 選択がターミナルの中身に限られること。
    ///
    /// 全部を1枚のマス目にすると、出力を選ぶとタブバーと罫線まで
    /// 付いてくる。分けてあるからこそ、出力だけを選べる
    #[test]
    fn only_the_terminal_contents_are_selectable() {
        assert!(
            PAGE.contains("#screen { user-select:text; }"),
            "ターミナルの中身が選べる指定が無い"
        );
        assert!(
            PAGE.contains(".tab") && PAGE.contains("user-select:none"),
            "タブバーが選択に混ざる"
        );
    }

    /// ボールが文字ではなく、動く要素であること。
    ///
    /// 窓を持った理由の半分がこれ。マス目のままなら ● で足りていた
    #[test]
    fn the_ball_is_a_moving_thing_not_a_character() {
        assert!(PAGE.contains("#ball"), "ボールの要素が無い");
        assert!(PAGE.contains("transition:left"), "動かない");
        assert!(!PAGE.contains("\u{25CF}"), "文字の●で描いている");
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
