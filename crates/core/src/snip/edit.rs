//! Editing a picture: the part of the screen that was framed, with arrows,
//! boxes, words, hidden parts and lines drawn on it, cut, resized or given
//! more room, and handed over -- to the clipboard, to a file, to a printer.
//!
//! Three pieces laid into the tool page (`snip.rs`), because they are the same
//! page: the framing before it, and the ways out after it, are shared with the
//! other tools. What is drawn stays a list of things on top of the picture
//! until the picture itself changes size, so a word written a moment ago can
//! still be put right.

/// The editor's styles, inside the page's own `<style>`.
pub const CSS: &str = r##"
 /* Editing a picture: the bar of what to do with it along the top, the tools
    down the left, the picture in the rest */
 #edit { position:fixed; inset:0; display:flex; flex-direction:column; background:var(--bg); }
 .ebar { display:flex; align-items:center; gap:var(--s2); padding:var(--s2) var(--s3);
   background:var(--panel); border-bottom:1px solid var(--line); overflow-x:auto; flex:none;
   scrollbar-width:thin; }
 .ebar > button { flex:none; white-space:nowrap; }
 .ebar.away, .erail.away { opacity:.4; pointer-events:none; }
 .esep { flex:none; width:1px; align-self:stretch; margin:var(--s1) 0; background:var(--line); }
 .espace { flex:1; min-width:var(--s2); }
 button.off, button.off:hover { background:var(--panel2); border-color:var(--line); color:var(--faint); cursor:not-allowed; }
 .eopts { display:flex; align-items:center; gap:var(--s2); flex:none; }
 .swatch { width:24px; height:24px; padding:0; border-radius:50%; border:1px solid var(--edge); flex:none; }
 .swatch.on { box-shadow:0 0 0 2px var(--panel), 0 0 0 4px var(--brand); }
 .chip { height:28px; padding:0 var(--s2); font-size:11.5px; border-radius:var(--r-chip); flex:none; }
 .chip.on { border-color:var(--brand); color:var(--text); background:var(--panel2); }
 .chip .ln { display:inline-block; width:18px; background:currentColor; border-radius:2px; vertical-align:middle; margin-right:var(--s1); }
 .ebody { flex:1; min-height:0; display:flex; }
 .erail { flex:none; width:72px; display:flex; flex-direction:column; gap:var(--s1); padding:var(--s2);
   background:var(--panel); border-right:1px solid var(--line); overflow-y:auto; }
 .erail button { height:auto; min-height:52px; padding:var(--s1); display:flex; flex-direction:column;
   align-items:center; justify-content:center; gap:2px; font-size:11px; background:none; border-color:transparent; color:var(--dim); }
 .erail button:hover { color:var(--text); border-color:var(--edge); }
 .erail button.on { background:var(--panel2); border-color:var(--brand); color:var(--text); }
 .erail svg { width:22px; height:22px; stroke:currentColor; fill:none; stroke-width:1.8; stroke-linecap:round; stroke-linejoin:round; }
 .earea { position:relative; flex:1; min-width:0; overflow:hidden; cursor:crosshair; touch-action:none; }
 .earea.texting { cursor:text; }
 #e-view { position:absolute; left:0; top:0; display:block; }
 #e-cropbox { position:absolute; border:1px solid var(--brand); pointer-events:none;
   box-shadow:0 0 0 1px #0009, 0 0 0 100000px #0007; }
 #e-text { position:absolute; margin:0; padding:0 2px; border:1px dashed var(--brand); border-radius:2px;
   background:transparent; resize:none; overflow:hidden; white-space:pre; line-height:1.25;
   font-weight:600; font-family:system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif;
   -webkit-user-select:text; user-select:text; outline:none; min-width:1em; }
 .ecropbar { position:absolute; left:50%; top:var(--s3); transform:translateX(-50%); display:flex; align-items:center;
   gap:var(--s2); padding:var(--s2) var(--s2) var(--s2) var(--s4); background:var(--panel); border:1px solid var(--line);
   border-radius:var(--r-card); box-shadow:0 8px 24px #0007; white-space:nowrap; z-index:5; }
 .ecropbar .say { font-size:12px; color:var(--dim); }
 .estatus { flex:none; display:flex; gap:var(--s3); align-items:center; min-height:28px; padding:var(--s1) var(--s3);
   background:var(--panel); border-top:1px solid var(--line); font-size:11.5px; color:var(--dim); }
 .estatus .size { font-variant-numeric:tabular-nums; flex:none; }
 .estatus .why { color:var(--warn); }
 /* A decision about the picture: the settings page's dialog, head, body and foot */
 .eshade { position:fixed; inset:0; z-index:50; background:#0007; display:flex; justify-content:center;
   align-items:flex-start; padding:56px var(--s4) var(--s4); }
 .edlg { width:min(420px, 100%); background:var(--panel); border:1px solid var(--line); border-radius:var(--r-card);
   box-shadow:0 8px 24px #0007; display:flex; flex-direction:column; max-height:100%; }
 .edlg .dh { display:flex; align-items:center; gap:var(--s2); padding:var(--s4) var(--s5); border-bottom:1px solid var(--line); }
 .edlg .dh b { flex:1; font-size:13.5px; }
 .edlg .db { padding:var(--s5); display:flex; flex-direction:column; gap:var(--s5); overflow-y:auto; }
 .edlg .df { display:flex; flex-wrap:wrap; justify-content:flex-end; align-items:center; gap:var(--s2); padding:var(--s3) var(--s5);
   border-top:1px solid var(--line); }
 .edlg .df .lead { margin-right:auto; }
 .edlg .df .why { flex-basis:100%; color:var(--warn); font-size:11.5px; text-align:right; }
 .edlg button.danger { background:none; border-color:transparent; color:var(--stop); }
 .field { display:flex; flex-direction:column; gap:var(--s2); }
 .field > span { font-size:12px; color:var(--text); }
 .field .hint { font-size:11.5px; color:var(--faint); }
 .pair { display:flex; gap:var(--s3); align-items:flex-end; }
 .pair .field { flex:1; min-width:0; }
 .edlg input[type=number] { height:36px; font:inherit; font-size:13px; padding:0 var(--s3); border:1px solid var(--edge);
   border-radius:var(--r-ctl); background:var(--bg); color:var(--text); width:100%; font-variant-numeric:tabular-nums; }
 .edlg input[type=number]:focus { border-color:var(--brand); outline:none; box-shadow:0 0 0 3px color-mix(in srgb, var(--brand) 22%, transparent); }
 .edlg label.check { display:flex; align-items:center; gap:var(--s2); font-size:14px; cursor:pointer; }
 .edlg label.check input { width:15px; height:15px; margin:0; accent-color:var(--brand); }
 .chips { display:flex; flex-wrap:wrap; gap:var(--s2); }
 .anchor { display:grid; grid-template-columns:repeat(3, 32px); gap:var(--s1); }
 .anchor button { width:32px; height:32px; padding:0; }
 .anchor button.on { background:var(--brand); border-color:var(--brand); }
 #printimg { display:none; }
 @media (max-width:700px) {
   /* Every button in sight: a bar that scrolls sideways hides the ones past
      the edge, and nothing says they are there */
   .ebar { flex-wrap:wrap; overflow-x:visible; row-gap:var(--s2); }
   .ebar > .esep, .eopts > .esep { display:none; }
   .eopts { flex-wrap:wrap; }
   .erail { width:60px; padding:var(--s1); }
   .erail button { min-height:48px; font-size:10.5px; }
 }
 @media print {
   html, body { height:auto; overflow:visible; background:#fff; }
   body > *:not(#printimg) { display:none !important; }
   #printimg { display:block !important; max-width:100%; max-height:100vh; margin:0 auto; object-fit:contain; }
 }
"##;

/// The editor's markup, inside the page's `<body>`.
pub const HTML: &str = r##"
<div id="edit" hidden>
  <div class="ebar" id="e-bar">
    <button class="primary" id="e-clip"></button>
    <button id="e-save"></button>
    <button id="e-print"></button>
    <span class="esep"></span>
    <button id="e-undo"></button>
    <button id="e-redo"></button>
    <span class="esep"></span>
    <button id="e-crop"></button>
    <button id="e-resize"></button>
    <button id="e-canvas"></button>
    <span class="esep"></span>
    <div class="eopts" id="e-opts"></div>
    <span class="espace"></span>
    <button class="quiet" id="e-close"></button>
  </div>
  <div class="ebody">
    <nav class="erail" id="e-tools"></nav>
    <div class="earea" id="e-area">
      <canvas id="e-view"></canvas>
      <div id="e-cropbox" hidden></div>
      <textarea id="e-text" hidden spellcheck="false" rows="1"></textarea>
      <div class="ecropbar" id="e-cropbar" hidden>
        <span class="say"></span>
        <button class="quiet" id="e-crop-no"></button>
        <button class="primary" id="e-crop-ok"></button>
      </div>
    </div>
  </div>
  <div class="estatus"><span class="size" id="e-size"></span><span id="e-say"></span></div>
  <div class="eshade" id="e-shade" hidden><div class="edlg" id="e-dlg" role="dialog" aria-modal="true"></div></div>
</div>
<img id="printimg" alt="">
"##;

/// The editor's script. Runs after the page's own, whose helpers it uses
/// (`$`, `T`, `tell`, `toast`, `shut`, the picture and the framed part).
pub const JS: &str = r##"<script>
// ── Editing a picture ──────────────────────────────
// What is drawn is a list of things laid over the picture, drawn again in order
// whenever the list changes. Cutting, resizing and growing the picture are the
// moments the things become part of it
const EDIT_TOOLS = ["arrow", "rect", "text", "hide", "pen"];
const EDIT_KEYS = {arrow: "a", rect: "r", text: "t", hide: "b", pen: "p"};
const EDIT_COLORS = ["#E5484D", "#F76B15", "#FFC53D", "#30A46C", "#0090FF", "#111111", "#FFFFFF"];
const EDIT_WIDTHS = {thin: 3, mid: 6, thick: 12};
const EDIT_SIZES = {small: 18, mid: 28, large: 44};
const EDIT_HOWS = ["mosaic", "blur", "fill"];
const EDIT_MAX = 16384;
const EDIT_HISTORY = 60;
const EDIT_ICONS = {
  arrow: '<svg viewBox="0 0 24 24"><path d="M5 19 19 5"/><path d="M10 5h9v9"/></svg>',
  rect: '<svg viewBox="0 0 24 24"><rect x="4" y="6" width="16" height="12" rx="1"/></svg>',
  text: '<svg viewBox="0 0 24 24"><path d="M5 6V5h14v1"/><path d="M12 5v14"/><path d="M9 19h6"/></svg>',
  hide: '<svg viewBox="0 0 24 24"><rect x="4" y="4" width="16" height="16" rx="1"/><path d="M4 12h16M12 4v16M8 4v16M16 4v16M4 8h16M4 16h16"/></svg>',
  pen: '<svg viewBox="0 0 24 24"><path d="M4 20c3-1 4-4 7-7l6-6a2 2 0 0 0-3-3l-6 6c-3 3-6 4-7 7z"/></svg>',
};

const E = {
  base: null,          // the picture, as it is now
  objs: [],            // what is drawn over it, in order
  committed: null,     // the picture with everything drawn, full size
  undo: [], redo: [],
  tool: "arrow", color: EDIT_COLORS[0], width: "mid", size: "mid", how: "mosaic",
  view: {s: 1, ox: 0, oy: 0},
  draft: null,         // what is being drawn this moment
  crop: null,          // {from, r} while choosing where to cut
  editing: null,       // {x, y, obj} while words are being typed
  dirty: false,        // drawn on since it was last handed anywhere
  why: "",             // why the last press did nothing, until something changes
};
// The clipboard takes a picture from the window, and from a browser only over a
// secure connection -- a phone reaching this PC over plain http has none
const EDIT_CLIP = HOST || !!(navigator.clipboard && window.ClipboardItem && window.isSecureContext);

const eW = () => E.base.width, eH = () => E.base.height;
// Line widths and letter sizes are for a picture the size of a screen. A
// picture cut from a 4K screen is drawn with the same lines a person sees
const eUnit = () => Math.max(1, Math.max(eW(), eH()) / 1920);

function editPrefs(save) {
  try {
    if (save) {
      localStorage.setItem("shikisha.edit", JSON.stringify({tool: E.tool, color: E.color, width: E.width, size: E.size, how: E.how}));
      return;
    }
    const p = JSON.parse(localStorage.getItem("shikisha.edit") || "{}");
    if (EDIT_TOOLS.includes(p.tool)) E.tool = p.tool;
    if (EDIT_COLORS.includes(p.color)) E.color = p.color;
    if (p.width in EDIT_WIDTHS) E.width = p.width;
    if (p.size in EDIT_SIZES) E.size = p.size;
    if (EDIT_HOWS.includes(p.how)) E.how = p.how;
  } catch (e) {}
}

function openEdit() {
  const c = document.createElement("canvas");
  c.width = rect.w; c.height = rect.h;
  c.getContext("2d").drawImage(img, rect.x, rect.y, rect.w, rect.h, 0, 0, rect.w, rect.h);
  E.base = c; E.objs = []; E.undo = []; E.redo = []; E.dirty = false; E.why = "";
  editPrefs(false);
  const say = (id, key, keyHint) => { const b = $(id); b.textContent = T[key] || ""; if (keyHint) b.title = (T[key] || "") + " (" + keyHint + ")"; };
  say("e-clip", "snip.to.clipboard", "Ctrl+C");
  say("e-save", "snip.to.file", "Ctrl+S");
  say("e-print", "snip.edit.print", "Ctrl+P");
  say("e-undo", "snip.edit.undo", "Ctrl+Z");
  say("e-redo", "snip.edit.redo", "Ctrl+Y");
  say("e-crop", "snip.edit.crop");
  say("e-resize", "snip.edit.resize");
  say("e-canvas", "snip.edit.canvas");
  say("e-close", "snip.close");
  say("e-crop-no", "snip.edit.cancel");
  say("e-crop-ok", "snip.edit.crop.ok");
  document.querySelector("#e-cropbar .say").textContent = T["snip.edit.crop.say"] || "";
  // Without a clipboard to hand it to, saving is the way out
  $("e-clip").hidden = !EDIT_CLIP;
  $("e-save").classList.toggle("primary", !EDIT_CLIP);
  editRail();
  editOpts();
  $("edit").hidden = false;
  editCommit();
  editFit();
  editDraw();
  editStatus();
}

// ── Drawing ────────────────────────────────────────
const lum = hex => {
  const n = parseInt(hex.slice(1), 16);
  return (0.2126 * (n >> 16 & 255) + 0.7152 * (n >> 8 & 255) + 0.0722 * (n & 255)) / 255;
};
const fontOf = size => "600 " + size + 'px system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif';

function drawObj(ctx, o) {
  ctx.save();
  ctx.lineCap = "round"; ctx.lineJoin = "round";
  ctx.strokeStyle = ctx.fillStyle = o.color;
  ctx.lineWidth = o.w;
  if (o.type === "arrow") {
    const dx = o.x2 - o.x1, dy = o.y2 - o.y1, len = Math.hypot(dx, dy);
    if (len >= 1) {
      const head = Math.min(len, Math.max(o.w * 4, 12));
      const ux = dx / len, uy = dy / len;
      const bx = o.x2 - ux * head * 0.8, by = o.y2 - uy * head * 0.8;
      ctx.beginPath(); ctx.moveTo(o.x1, o.y1); ctx.lineTo(bx, by); ctx.stroke();
      ctx.beginPath();
      ctx.moveTo(o.x2, o.y2);
      ctx.lineTo(o.x2 - ux * head - uy * head * 0.55, o.y2 - uy * head + ux * head * 0.55);
      ctx.lineTo(o.x2 - ux * head + uy * head * 0.55, o.y2 - uy * head - ux * head * 0.55);
      ctx.closePath(); ctx.fill();
    }
  } else if (o.type === "rect") {
    ctx.lineJoin = "miter";
    ctx.strokeRect(o.x, o.y, o.rw, o.rh);
  } else if (o.type === "pen") {
    const p = o.pts;
    ctx.beginPath();
    ctx.moveTo(p[0].x, p[0].y);
    if (p.length === 1) ctx.lineTo(p[0].x + 0.01, p[0].y);
    for (let i = 1; i < p.length - 1; i++) {
      ctx.quadraticCurveTo(p[i].x, p[i].y, (p[i].x + p[i + 1].x) / 2, (p[i].y + p[i + 1].y) / 2);
    }
    if (p.length > 1) ctx.lineTo(p[p.length - 1].x, p[p.length - 1].y);
    ctx.stroke();
  } else if (o.type === "text") {
    ctx.font = fontOf(o.size);
    ctx.textBaseline = "top";
    // A rim in the opposite shade, so the words read on any part of a screen
    ctx.lineWidth = Math.max(2, o.size / 7);
    ctx.strokeStyle = lum(o.color) > 0.6 ? "rgba(0,0,0,.85)" : "rgba(255,255,255,.9)";
    o.text.split("\n").forEach((line, i) => {
      const y = o.y + i * o.size * 1.25 + o.size * 0.1;
      ctx.strokeText(line, o.x + 2, y);
      ctx.fillText(line, o.x + 2, y);
    });
  } else if (o.type === "hide") {
    hideIn(ctx, o);
  }
  ctx.restore();
}

// A part hidden for good. Reads what is under it at the moment it is drawn,
// so it hides what was drawn before it as well as the picture
function hideIn(ctx, o) {
  const x = Math.max(0, Math.floor(o.x)), y = Math.max(0, Math.floor(o.y));
  const w = Math.min(ctx.canvas.width, Math.ceil(o.x + o.rw)) - x;
  const h = Math.min(ctx.canvas.height, Math.ceil(o.y + o.rh)) - y;
  if (w < 1 || h < 1) return;
  if (o.how === "fill") { ctx.fillStyle = o.color; ctx.fillRect(x, y, w, h); return; }
  // Blocks as large as a line of text, so what was written cannot be read back
  // out of what is left
  const k = o.how === "mosaic"
    ? Math.max(10 * o.unit, Math.min(48 * o.unit, Math.min(w, h) / 4))
    : Math.max(8 * o.unit, Math.min(w, h) / 3);
  const t = document.createElement("canvas");
  t.width = Math.max(1, Math.round(w / k)); t.height = Math.max(1, Math.round(h / k));
  const tc = t.getContext("2d");
  tc.imageSmoothingEnabled = true; tc.imageSmoothingQuality = "high";
  tc.drawImage(ctx.canvas, x, y, w, h, 0, 0, t.width, t.height);
  ctx.save();
  ctx.beginPath(); ctx.rect(x, y, w, h); ctx.clip();
  ctx.imageSmoothingEnabled = o.how === "blur";
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(t, 0, 0, t.width, t.height, x, y, w, h);
  ctx.restore();
}

// The picture with everything drawn, once, for the view to show and to hand out
function editCommit() {
  const c = document.createElement("canvas");
  c.width = eW(); c.height = eH();
  const ctx = c.getContext("2d");
  ctx.drawImage(E.base, 0, 0);
  for (const o of E.objs) {
    if (E.editing && E.editing.obj === o) continue;
    drawObj(ctx, o);
    if (o.type === "text") {
      ctx.font = fontOf(o.size);
      const lines = o.text.split("\n");
      o.bw = Math.max(...lines.map(l => ctx.measureText(l).width)) + 4;
      o.bh = lines.length * o.size * 1.25;
    }
  }
  E.committed = c;
}

function editFit() {
  const a = $("e-area"), pad = 24;
  const aw = Math.max(1, a.clientWidth - pad * 2), ah = Math.max(1, a.clientHeight - pad * 2);
  const s = Math.min(aw / eW(), ah / eH(), 4);
  E.view = {s, ox: Math.round((a.clientWidth - eW() * s) / 2), oy: Math.round((a.clientHeight - eH() * s) / 2)};
}

let checker = null;
function editDraw() {
  const a = $("e-area"), c = $("e-view");
  const dpr = window.devicePixelRatio || 1;
  const cw = a.clientWidth, ch = a.clientHeight;
  if (c.width !== Math.round(cw * dpr) || c.height !== Math.round(ch * dpr)) {
    c.width = Math.round(cw * dpr); c.height = Math.round(ch * dpr);
    c.style.width = cw + "px"; c.style.height = ch + "px";
  }
  const ctx = c.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cw, ch);
  const {s, ox, oy} = E.view, w = eW() * s, h = eH() * s;
  // What shows through a part with nothing in it, as squares
  if (!checker) {
    const p = document.createElement("canvas");
    p.width = p.height = 16;
    const pc = p.getContext("2d");
    pc.fillStyle = "#cfcfcf"; pc.fillRect(0, 0, 16, 16);
    pc.fillStyle = "#f4f4f4"; pc.fillRect(0, 0, 8, 8); pc.fillRect(8, 8, 8, 8);
    checker = ctx.createPattern(p, "repeat");
  }
  ctx.fillStyle = checker;
  ctx.fillRect(ox, oy, w, h);
  ctx.imageSmoothingEnabled = s * dpr < 2;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(E.committed, ox, oy, w, h);
  if (E.draft) {
    ctx.save();
    ctx.translate(ox, oy); ctx.scale(s, s);
    if (E.draft.type === "hide") {
      // Hidden when let go; until then, where it will be
      ctx.setLineDash([6 / s, 4 / s]);
      ctx.lineWidth = 1.5 / s;
      ctx.strokeStyle = "#fff"; ctx.strokeRect(E.draft.x, E.draft.y, E.draft.rw, E.draft.rh);
      ctx.lineDashOffset = 5 / s; ctx.strokeStyle = "#000"; ctx.strokeRect(E.draft.x, E.draft.y, E.draft.rw, E.draft.rh);
    } else {
      drawObj(ctx, E.draft);
    }
    ctx.restore();
  }
  ctx.strokeStyle = getComputedStyle(document.body).getPropertyValue("--line") || "#444";
  ctx.lineWidth = 1;
  ctx.strokeRect(ox - 0.5, oy - 0.5, w + 1, h + 1);
  if (E.crop && E.crop.r) {
    const r = E.crop.r, box = $("e-cropbox");
    box.hidden = false;
    box.style.left = ox + r.x * s + "px"; box.style.top = oy + r.y * s + "px";
    box.style.width = r.w * s + "px"; box.style.height = r.h * s + "px";
  } else {
    $("e-cropbox").hidden = true;
  }
}

function editStatus() {
  $("e-size").textContent = (T["snip.edit.size_now"] || "{w} × {h}").replace("{w}", eW()).replace("{h}", eH());
  const say = $("e-say");
  say.className = E.why ? "why" : "";
  say.textContent = E.why || T["snip.edit.hint." + (E.crop ? "crop" : E.tool)] || "";
  $("e-undo").classList.toggle("off", !E.undo.length);
  $("e-redo").classList.toggle("off", !E.redo.length);
}
// A press that did nothing says why, where it stays until something changes
function editWhy(key) { E.why = T[key] || ""; editStatus(); }

// ── Tools and their choices ────────────────────────
function editRail() {
  const rail = $("e-tools");
  rail.textContent = "";
  for (const t of EDIT_TOOLS) {
    const b = document.createElement("button");
    b.className = t === E.tool ? "on" : "";
    b.title = (T["snip.edit.tool." + t] || t) + " (" + EDIT_KEYS[t].toUpperCase() + ")";
    b.innerHTML = EDIT_ICONS[t];
    b.append(Object.assign(document.createElement("span"), {textContent: T["snip.edit.tool." + t] || t}));
    b.onclick = () => editTool(t);
    rail.append(b);
  }
}
function editTool(t) {
  editEndText();
  E.tool = t; E.why = "";
  editPrefs(true);
  editRail(); editOpts(); editStatus();
  $("e-area").classList.toggle("texting", t === "text");
}
function editOpts() {
  const box = $("e-opts");
  box.textContent = "";
  const chip = (label, on, pick, line) => {
    const b = Object.assign(document.createElement("button"), {className: "chip" + (on ? " on" : "")});
    if (line) b.append(Object.assign(document.createElement("span"), {className: "ln"}));
    if (line) b.firstChild.style.height = line + "px";
    b.append(document.createTextNode(label));
    b.onclick = pick;
    box.append(b);
  };
  const sep = () => box.append(Object.assign(document.createElement("span"), {className: "esep"}));
  if (E.tool === "hide") {
    for (const h of EDIT_HOWS) chip(T["snip.edit.how." + h] || h, E.how === h, () => { E.how = h; editPrefs(true); editOpts(); });
    if (E.how !== "fill") return;
    sep();
  }
  for (const col of EDIT_COLORS) {
    const b = Object.assign(document.createElement("button"), {className: "swatch" + (col === E.color ? " on" : ""),
      title: T["snip.edit.color"] || ""});
    b.style.background = col;
    b.onclick = () => { E.color = col; editPrefs(true); editOpts(); };
    box.append(b);
  }
  if (E.tool === "hide") return;
  sep();
  if (E.tool === "text") {
    for (const k in EDIT_SIZES) chip(T["snip.edit.size." + k] || k, E.size === k, () => { E.size = k; editPrefs(true); editOpts(); });
  } else {
    for (const k in EDIT_WIDTHS) chip(T["snip.edit.width." + k] || k, E.width === k, () => { E.width = k; editPrefs(true); editOpts(); },
      Math.max(1, Math.round(EDIT_WIDTHS[k] / 3)));
  }
}

// ── History ────────────────────────────────────────
function editChange(apply) {
  E.undo.push({base: E.base, objs: E.objs.slice()});
  if (E.undo.length > EDIT_HISTORY) E.undo.shift();
  E.redo = [];
  apply();
  E.dirty = true; E.why = "";
  const resized = E.committed && (E.committed.width !== eW() || E.committed.height !== eH());
  editCommit();
  if (resized) editFit();
  editDraw(); editStatus();
}
function editStep(from, to, emptyKey) {
  editEndText();
  if (!from.length) return editWhy(emptyKey);
  to.push({base: E.base, objs: E.objs.slice()});
  const s = from.pop();
  E.base = s.base; E.objs = s.objs; E.dirty = true; E.why = "";
  editCommit(); editFit(); editDraw(); editStatus();
}
const editUndo = () => editStep(E.undo, E.redo, "snip.edit.no_undo");
const editRedo = () => editStep(E.redo, E.undo, "snip.edit.no_redo");

// ── Pointer ────────────────────────────────────────
function editAt(e) {
  const r = $("e-area").getBoundingClientRect();
  return {x: (e.clientX - r.left - E.view.ox) / E.view.s, y: (e.clientY - r.top - E.view.oy) / E.view.s};
}
const clampX = x => Math.max(0, Math.min(eW(), x)), clampY = y => Math.max(0, Math.min(eH(), y));
(function () {
  const a = $("e-area");
  a.addEventListener("pointerdown", e => {
    if (e.button !== 0 || $("edit").hidden) return;
    const p = editAt(e);
    if (E.crop) {
      a.setPointerCapture(e.pointerId);
      E.crop.from = {x: clampX(p.x), y: clampY(p.y)};
      E.crop.r = null;
      editDraw();
      return;
    }
    if (E.editing) { editEndText(); if (E.tool !== "text") return; }
    if (E.tool === "text") {
      e.preventDefault();
      // Words already written are put right by pressing them
      const hit = [...E.objs].reverse().find(o => o.type === "text" &&
        p.x >= o.x && p.x <= o.x + o.bw && p.y >= o.y && p.y <= o.y + o.bh);
      editText(hit ? {x: hit.x, y: hit.y} : p, hit || null);
      return;
    }
    a.setPointerCapture(e.pointerId);
    const u = eUnit(), w = EDIT_WIDTHS[E.width] * u;
    const x = clampX(p.x), y = clampY(p.y);
    E.draft = E.tool === "pen" ? {type: "pen", pts: [{x, y}], color: E.color, w}
      : E.tool === "arrow" ? {type: "arrow", x1: x, y1: y, x2: x, y2: y, color: E.color, w}
      : {type: E.tool, x, y, rw: 0, rh: 0, fx: x, fy: y, color: E.color, w, how: E.how, unit: u};
  });
  a.addEventListener("pointermove", e => {
    const p = editAt(e), x = clampX(p.x), y = clampY(p.y);
    if (E.crop && E.crop.from) {
      const f = E.crop.from;
      E.crop.r = {x: Math.round(Math.min(f.x, x)), y: Math.round(Math.min(f.y, y)),
                  w: Math.round(Math.abs(f.x - x)), h: Math.round(Math.abs(f.y - y))};
      editDraw();
      return;
    }
    const d = E.draft;
    if (!d) return;
    if (d.type === "pen") {
      const last = d.pts[d.pts.length - 1];
      if (Math.hypot(last.x - x, last.y - y) * E.view.s < 1.5) return;
      d.pts.push({x, y});
    } else if (d.type === "arrow") {
      d.x2 = x; d.y2 = y;
    } else {
      d.x = Math.min(d.fx, x); d.y = Math.min(d.fy, y);
      d.rw = Math.abs(d.fx - x); d.rh = Math.abs(d.fy - y);
    }
    editDraw();
  });
  const done = () => {
    if (E.crop && E.crop.from) {
      E.crop.from = null;
      editDraw();
      return;
    }
    const d = E.draft;
    if (!d) return;
    E.draft = null;
    // A press that barely moved drew nothing, except with a pen, where it is a dot
    const tiny = 3 / E.view.s;
    const nothing = d.type === "arrow" ? Math.hypot(d.x2 - d.x1, d.y2 - d.y1) < tiny
      : d.type === "pen" ? false : (d.rw < tiny || d.rh < tiny);
    if (nothing) { editDraw(); return; }
    delete d.fx; delete d.fy;
    editChange(() => { E.objs = E.objs.concat([d]); });
  };
  a.addEventListener("pointerup", done);
  a.addEventListener("pointercancel", () => { E.draft = null; if (E.crop) E.crop.from = null; editDraw(); });
})();

// ── Words ──────────────────────────────────────────
function editText(at, obj) {
  const ta = $("e-text");
  const size = obj ? obj.size : EDIT_SIZES[E.size] * eUnit();
  const color = obj ? obj.color : E.color;
  E.editing = {x: at.x, y: at.y, obj, size, color};
  if (obj) { editCommit(); editDraw(); }
  ta.value = obj ? obj.text : "";
  ta.style.left = E.view.ox + at.x * E.view.s + "px";
  ta.style.top = E.view.oy + at.y * E.view.s + "px";
  ta.style.fontSize = size * E.view.s + "px";
  ta.style.color = color;
  ta.hidden = false;
  editGrow();
  // Now, while the press is still a press (a phone opens its keyboard only
  // then), and again once the press has finished moving focus about
  const focus = () => { ta.focus(); ta.setSelectionRange(ta.value.length, ta.value.length); };
  focus();
  setTimeout(focus, 0);
}
function editGrow() {
  const ta = $("e-text");
  ta.style.width = "1em"; ta.style.height = "1em";
  ta.style.width = ta.scrollWidth + 8 + "px";
  ta.style.height = ta.scrollHeight + "px";
}
// Finish the words being typed: kept if there are any, put right if they
// were already there, gone if they were emptied
function editEndText(cancel) {
  const ed = E.editing;
  if (!ed) return;
  E.editing = null;
  const ta = $("e-text");
  ta.hidden = true;
  const text = cancel ? null : ta.value.replace(/\s+$/, "");
  const old = ed.obj;
  if (text === null || (old && text === old.text) || (!old && !text)) {
    if (old) { editCommit(); editDraw(); }
    return;
  }
  editChange(() => {
    const fresh = text ? [{type: "text", x: ed.x, y: ed.y, text, size: ed.size, color: ed.color}] : [];
    const at = old ? E.objs.indexOf(old) : -1;
    E.objs = at < 0 ? E.objs.concat(fresh) : E.objs.slice(0, at).concat(fresh, E.objs.slice(at + 1));
  });
}
(function () {
  const ta = $("e-text");
  ta.addEventListener("input", editGrow);
  ta.addEventListener("keydown", e => {
    e.stopPropagation();
    // A word still being converted is not the end of the words
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); editEndText(); }
    else if (e.key === "Escape") { e.preventDefault(); editEndText(true); }
  });
  // Losing focus to the press that is placing these very words is not the end
  // of them; losing it to anything else is
  ta.addEventListener("blur", () => setTimeout(() => { if (document.activeElement !== ta) editEndText(); }, 0));
})();

// ── Cutting ────────────────────────────────────────
function editCropStart() {
  editEndText();
  E.crop = {from: null, r: null};
  E.why = "";
  $("e-cropbar").hidden = false;
  $("e-bar").classList.add("away");
  $("e-tools").classList.add("away");
  editStatus(); editDraw();
}
function editCropEnd() {
  E.crop = null;
  $("e-cropbar").hidden = true;
  $("e-bar").classList.remove("away");
  $("e-tools").classList.remove("away");
  editStatus(); editDraw();
}
function editCropApply() {
  const r = E.crop && E.crop.r;
  if (!r || r.w < 2 || r.h < 2) {
    document.querySelector("#e-cropbar .say").textContent = T["snip.edit.crop.need"] || "";
    return;
  }
  const flat = E.committed;
  editCropEnd();
  document.querySelector("#e-cropbar .say").textContent = T["snip.edit.crop.say"] || "";
  editChange(() => {
    const c = document.createElement("canvas");
    c.width = r.w; c.height = r.h;
    c.getContext("2d").drawImage(flat, r.x, r.y, r.w, r.h, 0, 0, r.w, r.h);
    E.base = c; E.objs = [];
  });
}

// ── Dialogs ────────────────────────────────────────
let dialogOk = null;
function editDialog(title, body, foot) {
  const d = $("e-dlg");
  d.textContent = "";
  const head = document.createElement("div");
  head.className = "dh";
  head.append(Object.assign(document.createElement("b"), {textContent: title}));
  head.append(Object.assign(document.createElement("button"), {className: "quiet", textContent: "✕",
    title: T["snip.close"] || "", onclick: editDialogShut}));
  const b = document.createElement("div");
  b.className = "db";
  b.append(...body);
  const f = document.createElement("div");
  f.className = "df";
  f.append(...foot);
  d.append(head, b, f);
  $("e-shade").hidden = false;
  const first = d.querySelector("input, .db button");
  setTimeout(() => (first || f.querySelector(".primary") || d).focus(), 0);
}
function editDialogShut() { $("e-shade").hidden = true; dialogOk = null; }
(function () {
  const shade = $("e-shade");
  let pressedOutside = false;
  // Pressing outside closes it -- unless the press began inside, which is a
  // selection that ended outside and not a way out
  shade.addEventListener("pointerdown", e => { pressedOutside = e.target === shade; });
  shade.addEventListener("click", e => { if (e.target === shade && pressedOutside) editDialogShut(); });
  shade.addEventListener("keydown", e => {
    e.stopPropagation();
    if (e.key === "Escape") { e.preventDefault(); editDialogShut(); }
    else if (e.key === "Enter" && dialogOk && e.target.tagName !== "BUTTON") { e.preventDefault(); dialogOk(); }
  });
})();
const btn = (cls, key, onclick) => Object.assign(document.createElement("button"), {className: cls, textContent: T[key] || "", onclick});
const numField = (key, value) => {
  const f = document.createElement("label");
  f.className = "field";
  const input = Object.assign(document.createElement("input"), {type: "number", min: 1, max: EDIT_MAX, step: 1, value});
  f.append(Object.assign(document.createElement("span"), {textContent: T[key] || ""}), input);
  return {f, input};
};
const whole = v => { const n = Number(v); return Number.isInteger(n) && n >= 1 && n <= EDIT_MAX ? n : 0; };

function editResize() {
  editEndText();
  const W = eW(), H = eH();
  const w = numField("snip.edit.resize.width", W), h = numField("snip.edit.resize.height", H);
  const keep = Object.assign(document.createElement("input"), {type: "checkbox", checked: true});
  const keepRow = document.createElement("label");
  keepRow.className = "check";
  keepRow.append(keep, document.createTextNode(T["snip.edit.resize.keep"] || ""));
  w.input.oninput = () => { if (keep.checked && whole(w.input.value)) h.input.value = Math.max(1, Math.round(whole(w.input.value) * H / W)); };
  h.input.oninput = () => { if (keep.checked && whole(h.input.value)) w.input.value = Math.max(1, Math.round(whole(h.input.value) * W / H)); };
  const pair = document.createElement("div");
  pair.className = "pair";
  pair.append(w.f, h.f);
  const chips = document.createElement("div");
  chips.className = "chips";
  for (const pct of [25, 50, 75, 150, 200]) {
    chips.append(Object.assign(document.createElement("button"), {className: "chip", textContent: pct + "%",
      onclick: () => { w.input.value = Math.max(1, Math.round(W * pct / 100)); h.input.value = Math.max(1, Math.round(H * pct / 100)); }}));
  }
  const field = document.createElement("div");
  field.className = "field";
  field.append(Object.assign(document.createElement("span"), {textContent: T["snip.edit.resize.scale"] || ""}), chips);
  const why = Object.assign(document.createElement("div"), {className: "why"});
  dialogOk = () => {
    const nw = whole(w.input.value), nh = whole(h.input.value);
    if (!nw || !nh) { why.textContent = (T["snip.edit.bad_size"] || "").replace("{max}", EDIT_MAX); return; }
    const flat = E.committed;
    editDialogShut();
    if (nw === W && nh === H) return;
    editChange(() => {
      const c = document.createElement("canvas");
      c.width = nw; c.height = nh;
      const ctx = c.getContext("2d");
      ctx.imageSmoothingEnabled = true; ctx.imageSmoothingQuality = "high";
      ctx.drawImage(flat, 0, 0, nw, nh);
      E.base = c; E.objs = [];
    });
  };
  editDialog(T["snip.edit.resize.title"] || "", [pair, keepRow, field],
    [why, btn("quiet", "snip.edit.cancel", editDialogShut), btn("primary", "snip.edit.apply", () => dialogOk())]);
}

function editCanvas() {
  editEndText();
  const W = eW(), H = eH();
  const w = numField("snip.edit.resize.width", W), h = numField("snip.edit.resize.height", H);
  const pair = document.createElement("div");
  pair.className = "pair";
  pair.append(w.f, h.f);
  const hint = Object.assign(document.createElement("div"), {className: "hint", textContent: T["snip.edit.canvas.hint"] || ""});
  const sizeField = document.createElement("div");
  sizeField.className = "field";
  sizeField.append(pair, hint);
  let ax = 1, ay = 1, bg = "white";
  const margins = document.createElement("div");
  margins.className = "chips";
  for (const m of [16, 32, 64]) {
    margins.append(Object.assign(document.createElement("button"), {className: "chip",
      textContent: (T["snip.edit.canvas.add"] || "+{n}px").replace("{n}", m),
      onclick: () => { w.input.value = W + m * 2; h.input.value = H + m * 2; ax = ay = 1; drawAnchor(); }}));
  }
  const marginField = document.createElement("div");
  marginField.className = "field";
  marginField.append(Object.assign(document.createElement("span"), {textContent: T["snip.edit.canvas.margin"] || ""}), margins);
  const grid = document.createElement("div");
  grid.className = "anchor";
  const drawAnchor = () => {
    grid.textContent = "";
    for (let j = 0; j < 3; j++) for (let i = 0; i < 3; i++) {
      grid.append(Object.assign(document.createElement("button"), {className: i === ax && j === ay ? "on" : "",
        title: T["snip.edit.canvas.anchor"] || "", onclick: () => { ax = i; ay = j; drawAnchor(); }}));
    }
  };
  drawAnchor();
  const anchorField = document.createElement("div");
  anchorField.className = "field";
  anchorField.append(Object.assign(document.createElement("span"), {textContent: T["snip.edit.canvas.anchor"] || ""}), grid);
  const bgs = document.createElement("div");
  bgs.className = "chips";
  const drawBg = () => {
    bgs.textContent = "";
    for (const k of ["white", "black", "clear"]) {
      bgs.append(Object.assign(document.createElement("button"), {className: "chip" + (bg === k ? " on" : ""),
        textContent: T["snip.edit.canvas.bg." + k] || k, onclick: () => { bg = k; drawBg(); }}));
    }
  };
  drawBg();
  const bgField = document.createElement("div");
  bgField.className = "field";
  bgField.append(Object.assign(document.createElement("span"), {textContent: T["snip.edit.canvas.bg"] || ""}), bgs);
  const why = Object.assign(document.createElement("div"), {className: "why"});
  dialogOk = () => {
    const nw = whole(w.input.value), nh = whole(h.input.value);
    if (!nw || !nh) { why.textContent = (T["snip.edit.bad_size"] || "").replace("{max}", EDIT_MAX); return; }
    const flat = E.committed;
    editDialogShut();
    if (nw === W && nh === H) return;
    editChange(() => {
      const c = document.createElement("canvas");
      c.width = nw; c.height = nh;
      const ctx = c.getContext("2d");
      if (bg !== "clear") { ctx.fillStyle = bg === "white" ? "#fff" : "#000"; ctx.fillRect(0, 0, nw, nh); }
      ctx.drawImage(flat, Math.round((nw - W) * ax / 2), Math.round((nh - H) * ay / 2));
      E.base = c; E.objs = [];
    });
  };
  editDialog(T["snip.edit.canvas.title"] || "", [sizeField, marginField, anchorField, bgField],
    [why, btn("quiet", "snip.edit.cancel", editDialogShut), btn("primary", "snip.edit.apply", () => dialogOk())]);
}

// ── Handing it over ────────────────────────────────
function pngOf(canvas) {
  return new Promise(done => canvas.toBlob(blob => done(blob), "image/png"));
}
function base64Of(blob) {
  return new Promise(done => {
    const r = new FileReader();
    r.onload = () => done(String(r.result).split(",")[1] || "");
    r.readAsDataURL(blob);
  });
}
function editName() {
  const d = new Date(), two = n => String(n).padStart(2, "0");
  return "snip-" + d.getFullYear() + two(d.getMonth() + 1) + two(d.getDate()) + "-" + two(d.getHours()) + two(d.getMinutes()) + two(d.getSeconds()) + ".png";
}
async function editCopy() {
  editEndText();
  if (!EDIT_CLIP) return;
  const blob = await pngOf(E.committed);
  if (HOST) {
    tell({act: "copy_image", png: await base64Of(blob)});
  } else {
    try { await navigator.clipboard.write([new ClipboardItem({"image/png": blob})]); }
    catch (e) { toast(T["snip.edit.copy_failed"] || ""); return; }
  }
  E.dirty = false;
  toast(T["snip.edit.copied"] || "");
}
async function editSave() {
  editEndText();
  const blob = await pngOf(E.committed);
  const name = editName();
  if (HOST) {
    // The window steps aside for the dialog and comes back with the answer
    tell({act: "save_image", png: await base64Of(blob), name});
    return;
  }
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  document.body.append(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
  E.dirty = false;
  toast(T["snip.edit.downloaded"] || "");
}
window.__snipSaved = function (how) {
  if (how === "saved") { E.dirty = false; toast(T["snip.edit.saved"] || ""); }
  else if (how === "failed") toast(T["snip.edit.save_failed"] || "");
};
function editPrint() {
  editEndText();
  const p = $("printimg");
  p.onload = () => { window.print(); };
  p.src = E.committed.toDataURL("image/png");
}
window.addEventListener("afterprint", () => { const p = $("printimg"); p.onload = null; p.removeAttribute("src"); });

// Closing with drawing nobody has taken anywhere asks first
function editClose() {
  editEndText();
  if (!E.dirty) return shut();
  const say = Object.assign(document.createElement("div"), {textContent: T["snip.edit.leave.say"] || ""});
  dialogOk = () => editDialogShut();
  const leave = btn("danger lead", "snip.edit.leave.discard", () => { editDialogShut(); E.dirty = false; shut(); });
  editDialog(T["snip.edit.leave.title"] || "", [say], [leave, btn("primary", "snip.edit.leave.back", editDialogShut)]);
}

(function () {
  $("e-clip").onclick = editCopy;
  $("e-save").onclick = editSave;
  $("e-print").onclick = editPrint;
  $("e-undo").onclick = editUndo;
  $("e-redo").onclick = editRedo;
  $("e-crop").onclick = editCropStart;
  $("e-resize").onclick = editResize;
  $("e-canvas").onclick = editCanvas;
  $("e-close").onclick = editClose;
  $("e-crop-no").onclick = editCropEnd;
  $("e-crop-ok").onclick = editCropApply;
  window.addEventListener("resize", () => { if (!$("edit").hidden) { editFit(); editDraw(); } });
  document.addEventListener("keydown", e => {
    if ($("edit").hidden || !$("e-shade").hidden) return;
    const k = e.key.toLowerCase(), mod = e.ctrlKey || e.metaKey;
    if (e.key === "Escape") {
      e.preventDefault();
      if (E.crop) editCropEnd();
      else if (E.draft) { E.draft = null; editDraw(); }
      else editClose();
    } else if (E.crop && e.key === "Enter") {
      e.preventDefault(); editCropApply();
    } else if (mod && k === "z" && !e.shiftKey) { e.preventDefault(); editUndo(); }
    else if (mod && (k === "y" || (k === "z" && e.shiftKey))) { e.preventDefault(); editRedo(); }
    else if (mod && k === "c") { e.preventDefault(); editCopy(); }
    else if (mod && k === "s") { e.preventDefault(); editSave(); }
    else if (mod && k === "p") { e.preventDefault(); editPrint(); }
    else if (!mod && !e.altKey && !E.crop) {
      const t = EDIT_TOOLS.find(t => EDIT_KEYS[t] === k);
      if (t) { e.preventDefault(); editTool(t); }
    }
  });
})();
</script>"##;
