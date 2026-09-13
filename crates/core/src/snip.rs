//! The tools that start by taking a picture: frame a part of it, then do
//! something with that part.
//!
//! One page, whoever opens it. The window opens it over a picture of its own
//! screen, taken the moment before; a phone opens it over a picture the person
//! chooses, because a web page on a phone has no way to take one. Everything
//! after the picture -- framing it, reading a colour out of it -- is this page,
//! so it behaves the same from either end and is written once.
//!
//! What happens with the answer depends on where the tool was opened from, and
//! the page is told rather than guessing: from the left bar it is offered to
//! the clipboard or to a file.

/// The tools this page can run, in the order they are offered.
///
/// The board's menu is built from this list, so a tool that is not here is not
/// offered anywhere -- a button for something this page cannot do would be a
/// button that does nothing
pub const TOOLS: &[&str] = &["color"];

/// The seconds a person can choose to wait before the picture is taken.
///
/// The wait is how something behind this program gets into the picture: the
/// program is in front when its button is pressed, and hiding it instead would
/// make it impossible to take a picture of the program itself
pub const WAITS: &[u8] = &[0, 3, 5, 10];

/// The page, in the colours and the language the app is in.
pub fn page() -> String {
    crate::i18n::render(&crate::webui::themed(PAGE.to_string()))
        .replace("__DICT__", &crate::i18n::dict_json())
        .replace(
            "__TOOLS__",
            &serde_json::to_string(TOOLS).unwrap_or_else(|_| "[]".into()),
        )
}

const PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<title>{{snip.title}}</title>
<style>
 :root {
   {{THEME}}
   color-scheme: {{SCHEME}};
   /* The same steps the board is built on (shell.rs), so a control here is the
      height and the roundness a control is everywhere else */
   --s1:4px; --s2:8px; --s3:12px; --s4:16px; --s5:20px; --s6:24px;
   --r-ctl:6px; --r-card:10px; --r-chip:4px;
 }
 * { box-sizing:border-box; }
 html, body { margin:0; height:100%; overflow:hidden; }
 body { background:var(--bg); color:var(--text); font-size:14px; line-height:1.5;
   font-family:system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif;
   -webkit-user-select:none; user-select:none; touch-action:none; }
 .mono { font-family:ui-monospace,Consolas,"Courier New",monospace; }
 button { font:inherit; font-size:13px; height:32px; padding:0 var(--s3); border-radius:var(--r-ctl);
   border:1px solid var(--line); background:var(--panel2); color:var(--text); cursor:pointer; }
 button:hover { border-color:var(--accent); }
 button.primary { background:var(--accent); border-color:var(--accent); color:var(--bg); font-weight:600; }
 [hidden] { display:none !important; }

 /* Waiting: a small card in a corner of the screen, which the picture leaves out */
 #wait { position:fixed; inset:0; display:flex; align-items:center; justify-content:center; gap:var(--s3);
   background:var(--panel); border:1px solid var(--brand); border-radius:var(--r-card); padding:0 var(--s4); }
 #wait .n { font-size:26px; font-weight:700; color:var(--brand); min-width:1.2em; text-align:center; }
 #wait .say { font-size:13px; color:var(--text); }

 /* Choosing a picture, where there is no screen to take (a phone) */
 #pick { position:fixed; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center;
   gap:var(--s4); padding:var(--s5); text-align:center; }
 #pick .say { color:var(--dim); max-width:32em; }
 #pick label { display:inline-flex; align-items:center; height:36px; padding:0 var(--s5); border-radius:var(--r-ctl);
   background:var(--accent); color:var(--bg); font-weight:600; cursor:pointer; }
 #pick input { display:none; }

 /* Framing: the picture, still, with everything outside the frame pushed back */
 #stage { position:fixed; inset:0; cursor:crosshair; }
 #shot { position:absolute; left:0; top:0; width:100%; height:100%; display:block; }
 #shade { position:absolute; inset:0; background:#0007; pointer-events:none; }
 #frame { position:absolute; border:1px solid var(--brand); pointer-events:none;
   box-shadow:0 0 0 1px #0009, 0 0 0 100000px #0007; }
 #size { position:absolute; padding:2px var(--s2); border-radius:var(--r-chip); background:var(--brand);
   color:var(--bg); font-size:12px; pointer-events:none; white-space:nowrap; }
 #hint { position:fixed; left:50%; top:var(--s4); transform:translateX(-50%); padding:var(--s2) var(--s4);
   border-radius:var(--r-card); background:var(--panel); border:1px solid var(--line); font-size:13px;
   pointer-events:none; white-space:nowrap; box-shadow:0 8px 24px #0007; }

 /* Reading a colour: the framed part, grown until one pixel is a square you can hit */
 #zoom { position:fixed; inset:0; display:flex; background:var(--bg); }
 #area { position:relative; flex:1; min-width:0; cursor:none; }
 #zc { position:absolute; inset:0; width:100%; height:100%; display:block; }
 #panel { width:320px; flex:none; display:flex; flex-direction:column; border-left:1px solid var(--line);
   background:var(--panel); min-height:0; }
 #panel h2 { margin:0; font-size:13px; font-weight:600; color:var(--dim); padding:var(--s4) var(--s4) var(--s2); }
 .phead { display:flex; align-items:center; gap:var(--s2); padding:var(--s3) var(--s3) 0 var(--s4); }
 .phead b { flex:1; font-size:14px; }
 button.quiet { background:none; border-color:transparent; color:var(--dim); }
 button.quiet:hover { color:var(--text); border-color:var(--line); }
 .now { display:flex; gap:var(--s3); align-items:center; padding:0 var(--s4) var(--s4); border-bottom:1px solid var(--line); }
 .sw { width:44px; height:44px; flex:none; border-radius:var(--r-ctl); border:1px solid var(--line); }
 .codes { display:flex; flex-direction:column; align-items:flex-start; gap:2px; min-width:0; font-size:13px; }
 .say2 { padding:var(--s2) var(--s4); color:var(--dim); font-size:12px; }
 #list { flex:1; overflow-y:auto; padding:0 var(--s2) var(--s2); }
 .row { display:flex; align-items:center; gap:var(--s2); padding:var(--s1) var(--s2); border-radius:var(--r-ctl); }
 .row .sw { width:24px; height:24px; border-radius:var(--r-chip); }
 .row .c { display:flex; flex-wrap:wrap; gap:2px var(--s2); min-width:0; }
 .code { font-family:ui-monospace,Consolas,"Courier New",monospace; font-size:12.5px; background:none; border:none;
   height:auto; padding:1px var(--s1); color:var(--text); border-radius:var(--r-chip); text-align:left; }
 .code:hover { background:var(--panel2); }
 .foot { display:flex; gap:var(--s2); padding:var(--s3) var(--s4); border-top:1px solid var(--line); }
 .foot button { flex:1; min-width:0; }
 @media (max-width:700px), (max-aspect-ratio:1/1) {
   #zoom { flex-direction:column; }
   #panel { width:auto; height:52%; border-left:none; border-top:1px solid var(--line); }
   #area { cursor:default; }
   /* A phone's panel is short: the colour under the finger on one line, so the
      list of colours taken -- the thing being collected -- keeps the room */
   #panel h2.t-now { display:none; }
   .now { padding-bottom:var(--s3); }
   .now .sw { width:32px; height:32px; }
   .now .codes { flex-direction:row; flex-wrap:wrap; gap:0 var(--s2); }
   .say2.t-howto { padding-top:var(--s1); padding-bottom:0; }
   /* Above the buttons at the foot, not over them */
   body #toast { bottom:72px; }
 }

 /* The way out on a touch screen, where there is no Esc to press. In a corner
    of its own, above the picture and the frame alike */
 #x { position:fixed; top:var(--s3); right:var(--s3); z-index:20; }
 #toast { position:fixed; left:50%; bottom:var(--s5); transform:translateX(-50%); padding:var(--s2) var(--s4);
   border-radius:var(--r-card); background:var(--panel); border:1px solid var(--line); box-shadow:0 8px 24px #0007;
   font-size:13px; pointer-events:none; }
</style></head>
<body>
<div id="wait" hidden><span class="n"></span><span class="say"></span></div>
<div id="pick" hidden>
  <div class="say"></div>
  <label><input type="file" accept="image/*"><span class="btn"></span></label>
</div>
<div id="stage" hidden>
  <canvas id="shot"></canvas>
  <div id="shade"></div>
  <div id="frame" hidden></div>
  <div id="size" hidden></div>
</div>
<div id="hint" hidden></div>
<div id="zoom" hidden>
  <div id="area"><canvas id="zc"></canvas></div>
  <aside id="panel">
    <div class="phead"><b class="t-tool"></b><button class="quiet" id="shut"></button></div>
    <h2 class="t-now"></h2>
    <div class="now"><div class="sw" id="nowsw"></div><div class="codes" id="nowcodes"></div></div>
    <div class="say2 t-howto"></div>
    <h2 class="t-picked"></h2>
    <div id="list"></div>
    <div class="foot">
      <button class="primary" id="toclip"></button>
      <button id="tofile"></button>
    </div>
  </aside>
</div>
<button id="x" class="quiet" hidden></button>
<div id="toast" hidden></div>
<script>
const T = __DICT__;
const TOOLS = __TOOLS__;
const $ = id => document.getElementById(id);
const Q = new URLSearchParams(location.search);
const TOOL = TOOLS.includes(Q.get("tool")) ? Q.get("tool") : TOOLS[0];
// The window hands messages to the program that opened it; a page in a phone's
// browser has nobody to hand them to and does the same things itself
const HOST = !!(window.ipc && window.ipc.postMessage);
const tell = o => { if (HOST) window.ipc.postMessage(JSON.stringify(o)); };

let img = null;          // the picture everything is read from
let pixels = null;       // the framed part's colours, read once
let fit = null;          // where the picture sits on screen: {x, y, s} in CSS px
let rect = null;         // the framed part, in the picture's own pixels

// ── Saying something ───────────────────────────────
let toastTimer = 0;
function toast(text) {
  const t = $("toast");
  t.textContent = text;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, 1600);
}

// ── Where the answer goes ──────────────────────────
// One place, so the clipboard and the file mean the same thing from every tool
function copyOut(text) {
  if (HOST) {
    tell({act: "copy", text});
  } else if (navigator.clipboard && window.isSecureContext) {
    navigator.clipboard.writeText(text).catch(() => copyByHand(text));
  } else {
    copyByHand(text);
  }
  toast(T["snip.copied"] || "Copied");
}
// A phone's page is served over plain http, where the browser keeps the
// clipboard API to itself. Selecting text and copying it still works
function copyByHand(text) {
  const box = document.createElement("textarea");
  box.value = text;
  box.style.position = "fixed";
  box.style.opacity = "0";
  document.body.append(box);
  box.select();
  try { document.execCommand("copy"); } catch (e) {}
  box.remove();
}
function saveOut(text, name) {
  if (HOST) {
    tell({act: "save", text, name});
    return;
  }
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([text], {type: "text/plain"}));
  a.download = name;
  document.body.append(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
}
function shut() {
  if (HOST) { tell({act: "close"}); return; }
  // Opened as a layer over the board: the board takes it down
  if (window.parent !== window) { window.parent.postMessage({snip: "close"}, "*"); return; }
  history.back();
}
document.addEventListener("keydown", e => {
  if (e.key === "Escape") { e.preventDefault(); shut(); }
});

// ── Waiting, before the picture is taken ───────────
// Told by the window, once a second. Nothing here can be pressed: the card is
// left out of the picture and lets the pointer through, so the person can get
// the screen ready underneath it
window.__snipWait = function (left) {
  const w = $("wait");
  w.hidden = false;
  w.querySelector(".n").textContent = String(left);
  w.querySelector(".say").textContent = T["snip.wait"] || "";
};
// The picture has been taken. Asked for by number, so a picture from an earlier
// press can never be the one that arrives
window.__snipFrame = function (n) {
  $("wait").hidden = true;
  const i = new Image();
  i.onload = () => begin(i);
  i.onerror = () => { toast(T["snip.failed"] || ""); setTimeout(shut, 1600); };
  i.src = "/snip/frame.bmp?n=" + encodeURIComponent(n);
};

// ── A picture chosen, where none can be taken ──────
function offerPick() {
  const p = $("pick");
  p.hidden = false;
  p.querySelector(".say").textContent = T["snip.pick.say"] || "";
  p.querySelector(".btn").textContent = T["snip.pick"] || "";
  p.querySelector("input").onchange = e => {
    const f = e.target.files && e.target.files[0];
    if (!f) return;
    const url = URL.createObjectURL(f);
    const i = new Image();
    i.onload = () => { p.hidden = true; begin(i); };
    i.src = url;
  };
}

// ── Framing ────────────────────────────────────────
function begin(picture) {
  img = picture;
  $("stage").hidden = false;
  const hint = $("hint");
  hint.textContent = T["snip.frame.hint"] || "";
  hint.hidden = false;
  drawShot();
  window.addEventListener("resize", drawShot);
}
// The whole picture, as large as the screen allows and never stretched. On
// the window the picture is the screen itself, so this comes out at exactly one
// picture pixel to one screen pixel and the screen looks as if it stopped
function drawShot() {
  const c = $("shot");
  const dpr = window.devicePixelRatio || 1;
  const vw = innerWidth, vh = innerHeight;
  c.width = Math.round(vw * dpr);
  c.height = Math.round(vh * dpr);
  const s = Math.min(vw / img.naturalWidth, vh / img.naturalHeight);
  const w = img.naturalWidth * s, h = img.naturalHeight * s;
  fit = {x: (vw - w) / 2, y: (vh - h) / 2, s};
  const ctx = c.getContext("2d");
  ctx.imageSmoothingEnabled = s * dpr < 1;
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, c.width, c.height);
  ctx.drawImage(img, fit.x * dpr, fit.y * dpr, w * dpr, h * dpr);
}
// A point on screen, as a pixel of the picture
const toPicture = (cx, cy) => ({
  x: Math.max(0, Math.min(img.naturalWidth, (cx - fit.x) / fit.s)),
  y: Math.max(0, Math.min(img.naturalHeight, (cy - fit.y) / fit.s)),
});
(function () {
  const stage = $("stage");
  let from = null;
  const show = (a, b) => {
    const x = Math.min(a.x, b.x), y = Math.min(a.y, b.y);
    const w = Math.abs(a.x - b.x), h = Math.abs(a.y - b.y);
    const f = $("frame");
    f.hidden = false;
    $("shade").hidden = true;
    f.style.left = x + "px"; f.style.top = y + "px";
    f.style.width = w + "px"; f.style.height = h + "px";
    const p = toPicture(x, y), q = toPicture(x + w, y + h);
    const size = $("size");
    size.hidden = false;
    size.textContent = Math.round(q.x - p.x) + " × " + Math.round(q.y - p.y);
    size.style.left = Math.min(x, innerWidth - size.offsetWidth - 4) + "px";
    size.style.top = (y > 28 ? y - 26 : (y + h + 30 < innerHeight ? y + h + 4 : y + 4)) + "px";
  };
  stage.addEventListener("pointerdown", e => {
    if (e.button !== 0 || !img) return;
    stage.setPointerCapture(e.pointerId);
    from = {x: e.clientX, y: e.clientY};
    $("hint").hidden = true;
  });
  stage.addEventListener("pointermove", e => {
    if (from) show(from, {x: e.clientX, y: e.clientY});
  });
  stage.addEventListener("pointerup", e => {
    if (!from) return;
    const a = toPicture(from.x, from.y), b = toPicture(e.clientX, e.clientY);
    from = null;
    let x = Math.floor(Math.min(a.x, b.x)), y = Math.floor(Math.min(a.y, b.y));
    let w = Math.ceil(Math.max(a.x, b.x)) - x, h = Math.ceil(Math.max(a.y, b.y)) - y;
    // A press that did not move frames nothing. Somebody who pressed once
    // meant the place they pressed, so a small square around it stands in
    if (w < 3 || h < 3) {
      const R = 12;
      x = Math.max(0, Math.round(a.x) - R); y = Math.max(0, Math.round(a.y) - R);
      w = Math.min(img.naturalWidth - x, R * 2 + 1); h = Math.min(img.naturalHeight - y, R * 2 + 1);
    }
    rect = {x, y, w, h};
    $("stage").hidden = true;
    $("hint").hidden = true;
    if (TOOL === "color") openColor();
  });
})();

// ── Reading a colour ───────────────────────────────
const hex2 = n => n.toString(16).padStart(2, "0");
function codesOf(r, g, b) {
  const R = r / 255, G = g / 255, B = b / 255;
  const max = Math.max(R, G, B), min = Math.min(R, G, B), d = max - min;
  const l = (max + min) / 2;
  let h = 0, s = 0;
  if (d) {
    s = d / (1 - Math.abs(2 * l - 1));
    h = max === R ? ((G - B) / d) % 6 : max === G ? (B - R) / d + 2 : (R - G) / d + 4;
    h = Math.round(h * 60);
    if (h < 0) h += 360;
  }
  return {
    hex: ("#" + hex2(r) + hex2(g) + hex2(b)).toUpperCase(),
    rgb: "rgb(" + r + ", " + g + ", " + b + ")",
    hsl: "hsl(" + h + ", " + Math.round(s * 100) + "%, " + Math.round(l * 100) + "%)",
  };
}
const zoom = {s: 1, ox: 0, oy: 0, cur: null, hover: null};
const picked = [];

function openColor() {
  // The framed part's colours, read once. Reading them from the picture
  // rather than from the screen means the magnified view itself can never be
  // in what is being read
  const off = document.createElement("canvas");
  off.width = rect.w; off.height = rect.h;
  const octx = off.getContext("2d", {willReadFrequently: true});
  octx.drawImage(img, rect.x, rect.y, rect.w, rect.h, 0, 0, rect.w, rect.h);
  pixels = octx.getImageData(0, 0, rect.w, rect.h).data;
  zoom.cur = {x: Math.floor(rect.w / 2), y: Math.floor(rect.h / 2)};
  document.querySelector(".t-tool").textContent = T["snip.tool.color"] || "";
  document.querySelector(".t-now").textContent = T["snip.color.now"] || "";
  // What a finger does and what a mouse and keyboard do are said differently
  const touch = window.matchMedia && matchMedia("(pointer: coarse)").matches;
  document.querySelector(".t-howto").textContent =
    (touch ? T["snip.color.howto_touch"] : T["snip.color.howto"]) || "";
  document.querySelector(".t-picked").textContent = T["snip.color.picked"] || "";
  $("toclip").textContent = T["snip.to.clipboard"] || "";
  $("tofile").textContent = T["snip.to.file"] || "";
  $("shut").textContent = T["snip.close"] || "";
  $("zoom").hidden = false;
  fitZoom();
  drawZoom();
  drawNow(zoom.cur);
  drawPicked();
  window.addEventListener("resize", () => { fitZoom(); drawZoom(); });
}
// The largest whole number of screen pixels one picture pixel can take while
// the framed part still fits. Whole, because a pixel drawn 2.7 wide is a row of
// squares of two different sizes, and the edge between them is where a press
// lands on the wrong colour
function fitZoom() {
  const a = $("area");
  const dpr = window.devicePixelRatio || 1;
  const W = a.clientWidth * dpr, H = a.clientHeight * dpr;
  zoom.s = Math.max(1, Math.floor(Math.min(W / rect.w, H / rect.h)));
  zoom.ox = Math.floor((W - rect.w * zoom.s) / 2);
  zoom.oy = Math.floor((H - rect.h * zoom.s) / 2);
}
function drawZoom() {
  const a = $("area"), c = $("zc");
  const dpr = window.devicePixelRatio || 1;
  c.width = Math.round(a.clientWidth * dpr);
  c.height = Math.round(a.clientHeight * dpr);
  const ctx = c.getContext("2d");
  ctx.imageSmoothingEnabled = false;
  ctx.fillStyle = getComputedStyle(document.body).backgroundColor;
  ctx.fillRect(0, 0, c.width, c.height);
  const s = zoom.s;
  ctx.drawImage(img, rect.x, rect.y, rect.w, rect.h, zoom.ox, zoom.oy, rect.w * s, rect.h * s);
  // The edges between pixels, once a pixel is big enough to be counted
  if (s >= 8) {
    ctx.fillStyle = "rgba(128,128,128,0.35)";
    for (let i = 0; i <= rect.w; i++) {
      const x = zoom.ox + i * s;
      if (x >= 0 && x <= c.width) ctx.fillRect(x, Math.max(0, zoom.oy), 1, rect.h * s);
    }
    for (let j = 0; j <= rect.h; j++) {
      const y = zoom.oy + j * s;
      if (y >= 0 && y <= c.height) ctx.fillRect(Math.max(0, zoom.ox), y, rect.w * s, 1);
    }
  }
  // The pixel that a press would take: dark and light, so it shows on any colour
  const at = zoom.hover || zoom.cur;
  if (at) {
    const x = zoom.ox + at.x * s, y = zoom.oy + at.y * s, w = Math.max(s, 3);
    ctx.lineWidth = 2;
    ctx.strokeStyle = "#000";
    ctx.strokeRect(x - 2, y - 2, w + 4, w + 4);
    ctx.strokeStyle = "#fff";
    ctx.strokeRect(x, y, w, w);
  }
}
function colourAt(p) {
  const i = (p.y * rect.w + p.x) * 4;
  return codesOf(pixels[i], pixels[i + 1], pixels[i + 2]);
}
function pixelUnder(e) {
  const a = $("area").getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const x = Math.floor(((e.clientX - a.left) * dpr - zoom.ox) / zoom.s);
  const y = Math.floor(((e.clientY - a.top) * dpr - zoom.oy) / zoom.s);
  return (x < 0 || y < 0 || x >= rect.w || y >= rect.h) ? null : {x, y};
}
function codeButton(text) {
  return Object.assign(document.createElement("button"), {
    className: "code", textContent: text, title: T["snip.color.copy_one"] || "",
    onclick: () => copyOut(text),
  });
}
function drawNow(p) {
  const box = $("nowcodes");
  box.textContent = "";
  if (!p) return;
  const k = colourAt(p);
  $("nowsw").style.background = k.hex;
  box.append(codeButton(k.hex), codeButton(k.rgb), codeButton(k.hsl));
}
function pick(p) {
  if (!p) return;
  const k = colourAt(p);
  // The same colour taken twice in a row is one colour
  if (!picked.length || picked[picked.length - 1].hex !== k.hex) picked.push(k);
  drawPicked();
  toast((T["snip.color.took"] || "{hex}").replace("{hex}", k.hex));
}
function drawPicked() {
  const list = $("list");
  list.textContent = "";
  if (!picked.length) {
    const none = document.createElement("div");
    none.className = "say2";
    none.textContent = T["snip.color.none"] || "";
    list.append(none);
  }
  for (const k of picked) {
    const row = document.createElement("div");
    row.className = "row";
    const sw = document.createElement("div");
    sw.className = "sw";
    sw.style.background = k.hex;
    const c = document.createElement("div");
    c.className = "c";
    c.append(codeButton(k.hex), codeButton(k.rgb), codeButton(k.hsl));
    row.append(sw, c);
    list.append(row);
  }
  list.scrollTop = list.scrollHeight;
  $("toclip").disabled = $("tofile").disabled = !picked.length;
}
const listText = () => picked.map(k => k.hex + "\t" + k.rgb + "\t" + k.hsl).join("\n") + "\n";
(function () {
  const a = $("area");
  a.addEventListener("pointermove", e => {
    zoom.hover = pixelUnder(e);
    if (zoom.hover) zoom.cur = zoom.hover;
    drawZoom();
    drawNow(zoom.cur);
  });
  a.addEventListener("pointerleave", () => { zoom.hover = null; drawZoom(); });
  a.addEventListener("pointerdown", e => {
    if (e.button !== 0) return;
    const p = pixelUnder(e);
    if (p) { zoom.cur = p; pick(p); drawZoom(); drawNow(p); }
  });
  // Closer or further, keeping the pixel under the pointer where it is
  a.addEventListener("wheel", e => {
    e.preventDefault();
    const r = a.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const mx = (e.clientX - r.left) * dpr, my = (e.clientY - r.top) * dpr;
    const fx = (mx - zoom.ox) / zoom.s, fy = (my - zoom.oy) / zoom.s;
    const s = Math.max(1, Math.min(256, zoom.s + (e.deltaY < 0 ? Math.max(1, Math.round(zoom.s * 0.25)) : -Math.max(1, Math.round(zoom.s * 0.2)))));
    zoom.s = s;
    zoom.ox = Math.round(mx - fx * s);
    zoom.oy = Math.round(my - fy * s);
    zoom.hover = pixelUnder(e);
    drawZoom();
  }, {passive: false});
  document.addEventListener("keydown", e => {
    if ($("zoom").hidden || !zoom.cur) return;
    const step = {ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1]}[e.key];
    if (step) {
      e.preventDefault();
      zoom.hover = null;
      zoom.cur = {
        x: Math.max(0, Math.min(rect.w - 1, zoom.cur.x + step[0])),
        y: Math.max(0, Math.min(rect.h - 1, zoom.cur.y + step[1])),
      };
      drawZoom();
      drawNow(zoom.cur);
    } else if (e.key === "Enter") {
      e.preventDefault();
      pick(zoom.cur);
    }
  });
  $("toclip").onclick = () => copyOut(listText());
  $("tofile").onclick = () => saveOut(listText(), "colors.txt");
  $("shut").onclick = shut;
})();

// A page on a phone has no Esc key, so it is given a button instead. The
// window says "Esc to cancel" where the frame is drawn, and keeps its screen
// clear of anything that is not the picture
if (!HOST) {
  const x = $("x");
  x.textContent = T["snip.close"] || "";
  x.hidden = false;
  x.onclick = shut;
}
new MutationObserver(() => { if (!HOST) $("x").hidden = !$("zoom").hidden; })
  .observe($("zoom"), {attributes: true, attributeFilter: ["hidden"]});

// ── Where the picture comes from ───────────────────
// The window says, by number, once it has one; a page with nobody to take a
// picture for it asks the person for one
if (Q.get("src") === "pick" || !HOST) {
  offerPick();
} else if (Q.get("wait")) {
  window.__snipWait(Number(Q.get("wait")));
} else if (Q.get("n")) {
  window.__snipFrame(Q.get("n"));
}
</script>
</body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// The page arrives filled in: no marker the builder should have replaced
    /// is left standing, which would turn the whole script into a syntax error
    /// and leave a blank screen over the person's own
    #[test]
    fn the_page_is_filled_in() {
        let page = PAGE
            .replace("{{THEME}}", "")
            .replace("{{SCHEME}}", "dark")
            .replace("{{__lang__}}", "en");
        let built = crate::i18n::render(&page)
            .replace("__DICT__", "{}")
            .replace("__TOOLS__", &serde_json::to_string(TOOLS).unwrap());
        assert!(!built.contains("__DICT__") && !built.contains("__TOOLS__"));
        assert!(built.contains("const TOOLS = [\"color\"];"), "道具の一覧が入っていない");
    }

    /// Every tool the board may offer is one the page can run, and the page
    /// starts a tool only by a name on that list
    #[test]
    fn the_tools_offered_are_the_tools_the_page_runs() {
        for t in TOOLS {
            assert!(
                PAGE.contains(&format!("TOOL === \"{t}\"")),
                "{t} を提示しているのにページが動かせない"
            );
        }
        assert!(PAGE.contains("const TOOL = TOOLS.includes(Q.get(\"tool\"))"), "一覧に無い名前で道具が始まる");
    }

    /// A colour is read in whole pixels. A pixel drawn a fraction wide makes
    /// squares of two sizes, and the edge between them is where a press lands
    /// on the neighbouring colour
    #[test]
    fn a_colour_is_read_in_whole_pixels() {
        assert!(PAGE.contains("zoom.s = Math.max(1, Math.floor(Math.min(W / rect.w, H / rect.h)));"));
        assert!(PAGE.contains("ctx.imageSmoothingEnabled = false;"), "拡大がぼける");
    }

    /// Nothing the page does with an answer sends it anywhere but the
    /// person's own clipboard or file: the window is told to copy or to save,
    /// and a phone does both itself
    #[test]
    fn an_answer_goes_to_the_clipboard_or_a_file_and_nowhere_else() {
        let acts: Vec<&str> = PAGE.match_indices("tell({act: \"").map(|(i, _)| {
            let rest = &PAGE[i + "tell({act: \"".len()..];
            &rest[..rest.find('"').unwrap()]
        }).collect();
        assert!(!acts.is_empty());
        for a in acts {
            assert!(["copy", "save", "close"].contains(&a), "知らない行き先 {a}");
        }
    }

    /// Every tool the board offers has a name to be offered under. The board
    /// builds the key from the tool's name at run time, which the check for
    /// missing words cannot follow, so this one does
    #[test]
    fn every_tool_has_a_name_on_the_menu() {
        for t in TOOLS {
            let key = format!("snip.tool.{t}");
            assert_ne!(crate::i18n::t(&key), key, "{key} が言葉の表に無い");
        }
    }

    /// The waits offered are short and include none at all -- no wait is how
    /// a picture of this program itself is taken
    #[test]
    fn the_waits_start_at_none() {
        assert_eq!(WAITS.first(), Some(&0));
        assert!(WAITS.iter().all(|w| *w <= 10));
    }
}
