//! 窓の外皮。タブバー、稼働盤、ステータス。
//!
//! ここは本物のHTMLで書く。文字マスをそのまま変換すると早いが、
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
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>SHIKISHA-TERM</title>
<style>
  :root {
    --bg:#0a0c0e; --panel:#11151a; --line:#1d2630; --text:#e8eef4;
    --dim:#7a8896; --brand:#00aaff; --live:#4ade80; --warn:#ffc857; --stop:#ff6b6b;
    /* 罫線と記号を1マスで描けるものを先に。
       日本語は等幅の MS ゴシックへ回す (Meiryo は等幅ではない) */
    --mono:"Cascadia Mono","Consolas","MS Gothic","MS ゴシック",monospace;
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
  /* 「+」は普段は控えめに。触れたときだけ普通の濃さになる */
  .tab.addtab { color:var(--dim); }
  .tab.addtab:hover { color:inherit; }
  /* 出力量。文字ではなく本物の棒 */
  .spark { display:flex; align-items:flex-end; gap:1px; height:14px; flex:none; }
  .spark i { width:2px; background:var(--brand); opacity:.75; }

  /* ハンバーガーと幕。広い画面では出さない (サイドバー常時表示のまま) */
  #hamburger { display:none; position:fixed; top:6px; left:6px; z-index:40;
    width:34px; height:30px; align-items:center; justify-content:center;
    background:var(--panel); border:1px solid var(--line); border-radius:6px;
    color:var(--text); font-size:16px; cursor:pointer; }
  #backdrop { display:none; }

  /* ── 中身 ───────────────────────────────── */
  #main { position:relative; overflow:hidden; }
  /* font-family を書くのは飾りではない。<pre> にはブラウザ自身が
     monospace を当てていて、それは body から受け継ぐ指定より強い。
     書かないと、選んだフォントは端末の中身にだけ効かない */
  /* --cw はマス1つの幅。画面が測って入れる (中身とカーソルを同じ数で置く) */
  #screen { position:absolute; inset:0; margin:0; padding:8px; white-space:pre;
    overflow:auto; line-height:1.25; font-family:var(--mono); --cw:1ch; }
  /* 画面中継。ブラウザタブを見ているとき、端末の代わりにここへ映す。
     縦横比は保ちつつ枠に収める。touch-action:none で既定のスクロールを止め、
     指の動きを軌跡としてそのまま送る */
  #cast { position:absolute; inset:0; width:100%; height:100%;
    object-fit:contain; object-position:top center; background:#000; touch-action:none; }
  /* トラックパッド式の合成カーソル。Windows風の矢印で、先端がクリック点。
     負マージンで矢印の先端 (SVG座標 2,1) を left/top にぴったり合わせる */
  #castcursor { position:absolute; width:19px; height:30px; margin:-2px 0 0 -2px;
    pointer-events:none; z-index:15; display:none;
    filter:drop-shadow(0 1px 2px rgba(0,0,0,.6)); }
  #castcursor svg { display:block; }
  /* クリックの波紋 (押せたことのフィードバック) */
  .ripple { position:absolute; width:10px; height:10px; margin:-5px 0 0 -5px;
    border-radius:50%; border:2px solid var(--brand); pointer-events:none; z-index:14;
    animation:rip .48s ease-out forwards; }
  @keyframes rip { from { transform:scale(.4); opacity:.9 } to { transform:scale(4.5); opacity:0 } }
  /* 文字入力バー (画面下部の変換プレビュー付き入力欄) */
  #castbar { display:flex; align-items:center; gap:6px; padding:6px 8px;
    background:var(--panel); border-top:1px solid var(--brand); }
  #castinput { flex:1; min-width:0; font-size:16px; padding:8px 10px;
    background:var(--bg); color:var(--text); border:1px solid var(--line);
    border-radius:8px; outline:none; }
  #castbar .castsend { padding:8px 14px; border:0; border-radius:8px;
    background:var(--brand); color:#04121c; font-weight:700; cursor:pointer; }
  #castbar .castbtn { padding:8px 11px; border:1px solid var(--line);
    border-radius:8px; background:var(--bg); color:var(--text); cursor:pointer; }
  /* 操作モード中の表示 (タップで解除) */
  /* 下 (bottom) だとキーボードで隠れるので画面の上に置く。
     ナビバー (上端36px) を避けて少し下げた位置に浮かせる */
  #castmode { position:absolute; left:50%; top:42px; transform:translateX(-50%);
    background:#16202b; border:1px solid var(--brand); color:var(--text);
    padding:6px 14px; border-radius:16px; font-size:12px; z-index:19;
    display:none; cursor:pointer; }
  /* 補助キー列＋文字入力バーをまとめた下部ドック。visualViewport で
     キーボードの上へ持ち上げる。列は横スクロール、入力は下段 */
  #castdock { position:absolute; left:0; right:0; bottom:0; z-index:18;
    display:none; flex-direction:column; }
  #castkeys { display:flex; gap:6px; overflow-x:auto; white-space:nowrap;
    padding:6px 8px; background:var(--panel); border-top:1px solid var(--line);
    -webkit-overflow-scrolling:touch; scrollbar-width:none; }
  #castkeys::-webkit-scrollbar { display:none; }
  .castkey { flex:0 0 auto; min-width:40px; padding:8px 10px; font-size:14px;
    border:1px solid var(--line); border-radius:8px; background:var(--bg);
    color:var(--text); cursor:pointer; user-select:none; }
  .castkey:active { background:var(--brand); color:#04121c; }
  /* Ctrl/Alt は固定トグル。押している間は光らせて次の一打を待つ */
  .castkey.mod.on { background:var(--brand); color:#04121c; border-color:var(--brand); }
  /* ここだけ選べる。タブバーや枠は選択に混ざらない */
  #screen { user-select:text; }

  /* ── ブラウザの上のバー ──────────────────
     ページの中には描かない。ページを一段下げて、空いた場所に描く。
     中に描くと相手のCSSと喧嘩し、遷移のたびに消え、
     サイト自身の固定ヘッダーを上から覆ってしまう */
  #nav { position:absolute; left:0; right:0; top:0; height:36px; z-index:5;
    display:flex; align-items:center; gap:6px; padding:0 8px;
    border-bottom:1px solid var(--line); background:var(--panel);
    transition:background .15s, border-color .15s; }
  #nav[hidden] { display:none; }
  #nav button { font:inherit; font-size:13px; color:var(--text); cursor:pointer;
    background:transparent; border:1px solid var(--line); border-radius:6px;
    width:28px; height:24px; line-height:1; padding:0; flex:none; }
  #nav button:hover:not(:disabled) { background:#16202b; border-color:var(--brand); }
  #nav button:disabled { color:#2b3540; cursor:default; }
  #nav input { flex:1; min-width:60px; font:inherit; font-size:12px;
    color:var(--text); background:#0a0c0e; border:1px solid var(--line);
    border-radius:6px; padding:3px 8px; outline:none; }
  #nav input:focus { border-color:var(--brand); }
  /* 読み込み中はバー全体を青く染めて、一目で通信中と分かるようにする。
     一瞬の通信でも見えるよう、本体側で最低0.5秒は点けたままにしている */
  #nav.loading { background:#0d2a3a; border-bottom-color:var(--brand); }
  /* さらに下端を光が流れる帯 (動きの手がかり) */
  #nav.loading::after { content:""; position:absolute; left:0; right:0; bottom:0;
    height:3px; background:linear-gradient(90deg,transparent,var(--brand),transparent);
    background-size:40% 100%; background-repeat:no-repeat;
    animation:navload 1s linear infinite; }
  @keyframes navload { from { background-position:-40% 0 } to { background-position:140% 0 } }
  /* 更新ボタンは青く光らせて回す (どこを見ればいいか分かりやすい) */
  #nav button.spin { color:var(--brand); border-color:var(--brand); background:#0a1f2b; }
  #nav button.spin .ico { display:inline-block; animation:spin .8s linear infinite; }
  @keyframes spin { to { transform:rotate(360deg) } }
  /* ページを置く場所。バーを出したぶんだけ下がる */
  #page { position:absolute; inset:0; pointer-events:none; }

  /* ── 稼働盤 ─────────────────────────────── */
  #board { position:absolute; inset:0; overflow:auto; padding:22px 26px; }
  .mark { color:var(--brand); font-weight:700; letter-spacing:.5px;
    font-size:13px; line-height:1.15; white-space:pre; }
  /* 狭い画面用の素直なタイトル (既定は隠す) */
  .mark-lite { display:none; color:var(--brand); font-weight:700;
    font-size:22px; letter-spacing:2px; }
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
  .row.dim { color:var(--dim); margin-top:8px; }
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
  #status { grid-column:2; display:flex; align-items:center; gap:12px;
    padding:5px 12px; border-top:1px solid var(--line); background:var(--panel);
    font-size:12px; color:var(--dim); flex-wrap:nowrap; }
  #status .grow { flex:1; }
  /* ワークスペース名だけは詰まったら省略。ピルや STOP は縮めない */
  #status > span:first-child { min-width:0; white-space:nowrap;
    overflow:hidden; text-overflow:ellipsis; }
  #status .pill, #status .build, #stop { flex:none; white-space:nowrap; }
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
  /* 測る側も同じ理由で書く。測るものと描くものが違っては話にならない */
  #probe, #tprobe { position:absolute; visibility:hidden; white-space:pre;
    left:0; top:0; margin:0; font-family:var(--mono); }
  /* 覆いかぶさる画面。押せる場所を見失わないよう、外は暗くする */
  .dot.WEB { background:var(--brand); }
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
  /* 過去を見ている印。出ていないと、出力が止まったように見える */
  #back { position:absolute; right:14px; top:10px; z-index:6;
    background:#16202b; border:1px solid var(--brand); color:var(--text);
    padding:4px 12px; border-radius:14px; font-size:12px; cursor:pointer; }
  #back:hover { background:var(--brand); color:#04121c; }
  #flash { position:absolute; left:50%; bottom:52px; transform:translateX(-50%);
    background:#16202b; border:1px solid var(--brand); color:var(--text);
    padding:8px 16px; border-radius:8px; font-size:13px; }

  /* ── 狭い画面・縦長のレスポンシブ (スマホ・小型PC・縦長ディスプレイ) ──
     必ず全ての基本ルールの後ろに置く。前に置くと、後で定義された基本ルールが
     同じ詳細度で後勝ちして上書きが効かない */
  @media (max-width:700px), (max-aspect-ratio:1/1) {
    /* 何があっても横スクロール (謎の右空間) を出さない */
    html, body, #app { max-width:100vw; overflow-x:hidden; }
    /* グリッドの「幻の2列目」を避けるため flex の縦積みに */
    #app { display:flex; flex-direction:column; }
    #main { flex:1; min-height:0; }

    /* フッターを上部バーに回す。ハンバーガーはこのバーの中に収まるので、
       本文のどの要素にも重ならない (position:fixed の ☰ がバー左の余白に載る) */
    #status { order:-1; width:100vw; box-sizing:border-box; gap:8px;
      min-height:42px; padding-left:48px;
      border-top:none; border-bottom:1px solid var(--line); }
    /* ☰ は上部バーの左に載せる (バーと同じ高さで中央に) */
    #hamburger { display:flex; top:6px; left:8px; }
    /* 緊急停止は横幅節約のため赤い ■ だけにする (■ は世界共通の停止記号) */
    #stop { font-size:0; padding:2px 9px; }
    #stop::after { content:"\25A0"; font-size:13px; }
    /* build 刻印は場所を取るので隠す (STOP を確実に残す) */
    #status .build { display:none; }

    /* 引き出し式タブバー */
    #tabs { position:fixed; top:0; left:0; bottom:0; z-index:30; width:240px;
      transform:translateX(-100%); transition:transform .2s ease;
      box-shadow:2px 0 14px rgba(0,0,0,.5);
      padding-top:46px; }   /* 先頭の項目がハンバーガーに隠れないよう空ける */
    #app.drawer #tabs { transform:none; }
    #app.drawer #backdrop { display:block; position:fixed; inset:0; z-index:20;
      background:rgba(0,0,0,.45); }

    /* 本文は上部バーの下に来るので、もう ☰ 用の余白は要らない */
    #board { padding:16px 12px; }
    .card { overflow-x:auto; }          /* 幅超えの表はカード内でスクロール */
    .menu { grid-template-columns:1fr; } /* メニューは1列 */
    /* アスキーのワードマークは崩れるので、素直なテキスト側に切り替える */
    .mark { display:none; }
    .mark-lite { display:block; }
  }
</style></head><body>
<div id="app">
  <div id="hamburger">&#9776;</div>
  <div id="backdrop"></div>
  <nav id="tabs"></nav>
  <div id="main">
    <div id="nav" hidden></div>
    <div id="page"></div>
    <div id="board" hidden></div>
    <pre id="screen" hidden></pre>
    <canvas id="cast" hidden></canvas>
    <div id="cur" hidden></div>
    <textarea id="kbd" autocomplete="off" autocorrect="off" spellcheck="false"></textarea>
    <pre id="probe">MMMMMMMMMM</pre>
    <pre id="tprobe"></pre>
    <div id="back" hidden></div>
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
// 盤面が出すメニュー。押されたら、その文字がそのまま INDEX に届く
const MENU_KEYS = {{MENU_KEYS}};
const MENU_WORDS = {{MENU_WORDS}};
// 中継画面の補助キー列の並び (configでカスタマイズ可)
const CAST_KEYS = {{CAST_KEYS}};
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

// 押している間は作り直さない。
// click は「同じ要素で始まって終わる」ことで成立する。押し下げと押し上げの
// 間に盤面を作り直すと、押した要素はもう無く、その押下はどこにも届かない。
// 活動グラフは絶えず動くので、これは稀な事故ではなく既定の動作だった。
let holding = false, queued = null, holdTimer = 0;
const release = () => {
  holding = false;
  clearTimeout(holdTimer);
  if (queued !== null) { const j = queued; queued = null; window.__state(j); }
};
addEventListener("pointerdown", () => {
  holding = true;
  // 離した合図が届かないことがある。重ねたページの上で指を離すと、
  // こちらには pointerup が来ない。そのまま押しっぱなし扱いになると
  // 画面が二度と描き直されず、タブを押しても効かないように見える。
  // 押下を守るのに1秒あれば足りる
  clearTimeout(holdTimer);
  holdTimer = setTimeout(release, 1000);
}, true);
addEventListener("pointerup", release, true);
addEventListener("pointercancel", release, true);
// 窓の外で離された時のために。押しっぱなしのまま固まる方が困る
addEventListener("blur", release, true);

// 引き出し式タブバー (狭い画面・縦長)。広い画面では常時表示なので効かない
{
  const app = document.getElementById("app");
  const ham = document.getElementById("hamburger");
  const bd = document.getElementById("backdrop");
  if (ham) ham.onclick = () => app.classList.toggle("drawer");
  if (bd) bd.onclick = () => app.classList.remove("drawer");
  // タブを選んだら畳んで全幅へ戻す
  const tabs = document.getElementById("tabs");
  if (tabs) tabs.addEventListener("click", () => app.classList.remove("drawer"));
  // 上のバー (キャストの外) を押したら操作モードを抜ける
  const st = document.getElementById("status");
  if (st) st.addEventListener("pointerdown", () => { if (typeof exitCast === "function") exitCast(); });
}

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
  // 一覧の最後に「+」。設定画面をタブ追加の状態で開く
  nav.append(el("div", {class:"tab addtab", onclick:() => send({kind:"addtab"})},
    el("span", {class:"num"}, "+"),
    el("span", {class:"nm"}, T["tui.tab.add"] || "ADD TAB")));
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
  // 広い画面はアスキーアートのワードマーク。狭い画面は崩れるので
  // 素直な太字テキストに切り替える (CSS のメディアクエリで出し分け)
  b.append(el("div", {class:"mark"}, WORDMARK.join("\n")),
           el("div", {class:"mark-lite"}, "SHIKISHA-TERM"),
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
      el("td", {}, el("span", {class:"dot " + t.state}), " " + (t.state_label || t.state)),
      el("td", {}, t.profile),
      el("td", {}, spark(t.activity))));
  }
  b.append(el("div", {class:"card"}, el("h2", {}, "SESSIONS"), rows));

  // メニュー。並びは MENU_KEYS が決める (受け手と突き合わせるため)
  const items = MENU_KEYS.map(k => [k, T[MENU_WORDS[k]]]);
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
  // append に null を渡すと、DOMは文字列 "null" にして並べる。
  // el() の中では弾いているが、ここは素の append なので自分で弾く
  [
    el("span", {}, S.workspace || ""),
    el("span", {class:"pill " + (S.auto_enabled ? "on" : "off")},
      "AUTO " + (S.auto_enabled ? "ON" : "OFF")),
    S.remote_on ? el("span", {class:"pill on"}, "REMOTE") : null,
    el("span", {class:"grow"}),
    el("span", {class:"build"}, BUILD),
    el("span", {id:"stop", onclick:() => send({kind:"stop"})},
      T["tui.stop"] || "STOP"),
  ].forEach(x => { if (x) s.append(x); });
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
    box.append(el("h3", {}, T["tui.help.title"] || "HELP"));
    // 訳語は1行ずつ別のキーで持っている。並べる順はここで決める
    for (const k of ["quit", "tabs", "ws", "lock", "restart", "copy", "auto", "raw",
                     "mouse", "mouse.wheel", "mouse.drag", "mouse.right",
                     "mouse.tab", "mouse.divider"]) {
      box.append(el("div", {class:"row"}, T["tui.help." + k]));
    }
    box.append(el("div", {class:"row dim"}, T["tui.help.close"]));
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

// 上のバー。押した先はRustが決める (出ているバーは常に1枚なので、
// どのページ宛かをこちらから言う必要がない)
const goTo = () => {
  const inp = document.querySelector("#nav input");
  if (inp && inp.value.trim()) send({kind:"go", what:"to", url:inp.value});
};
function drawNav() {
  const n = document.getElementById("nav");
  const want = S && S.nav;
  n.hidden = !want;
  n.classList.toggle("loading", !!(want && want.loading));   // 通信中の帯
  if (!want) { n.textContent = ""; layout(); return; }
  // 打っている途中に組み直すと、1文字ごとに書きかけが消える
  const inp = n.querySelector("input");
  const typing = inp && document.activeElement === inp;
  if (!typing) {
    n.textContent = "";
    const btn = (mark, word, what, on) => {
      const b = el("button", {title:T[word]}, mark);
      b.disabled = !on;
      b.onclick = () => send({kind:"go", what:what});
      return b;
    };
    if (want.back) n.append(btn("←", "tui.nav.back", "back", want.can_back));
    if (want.forward) n.append(btn("→", "tui.nav.forward", "forward", want.can_forward));
    if (want.reload) {
      // 更新アイコンは span に包んで、読み込み中だけ回す
      const rb = el("button", {title:T["tui.nav.reload"], onclick:() => send({kind:"go", what:"reload"})},
        el("span", {class:"ico"}, "⟳"));
      if (want.loading) rb.classList.add("spin");
      n.append(rb);
    }
    if (want.edit) {
      const box = el("input", {type:"text", spellcheck:"false",
        title:T["tui.nav.url"], placeholder:T["tui.nav.url"], value:want.at || ""});
      box.onkeydown = e => {
        if (e.key === "Enter") { e.preventDefault(); goTo(); }
        // 打鍵は端末へ流さない。ここはページの行き先を書く場所
        e.stopPropagation();
      };
      box.onfocus = () => box.select();
      n.append(box);
    }
  } else if (want.edit) {
    // 打っていないボタンだけは、押せるかどうかを直す
    const bs = n.querySelectorAll("button");
    let i = 0;
    if (want.back && bs[i]) bs[i++].disabled = !want.can_back;
    if (want.forward && bs[i]) bs[i++].disabled = !want.can_forward;
  }
  layout();
}

// ページを置く場所を、バーのぶんだけ下げる。
// 中継キャンバスも同じだけ下げないと、ブラウザ上端 (ログイン等がよくある)
// がバーの裏に隠れてしまう。カーソル座標はキャンバスの位置から測るので追従する
function layout() {
  const n = document.getElementById("nav");
  const top = n.hidden ? "0" : "36px";
  document.getElementById("page").style.top = top;
  document.getElementById("cast").style.top = top;
  report();
}

window.__state = function (json) {
  if (holding) { queued = json; return; }
  S = JSON.parse(json);
  drawTabs();
  drawStatus();
  drawNav();
  const board = document.getElementById("board");
  const screen = document.getElementById("screen");
  // ブラウザのタブを見ているときは、置いたページが同じ場所を覆う。
  // 端末の中身を残しておくと、切り替えた瞬間だけ前のタブが見える
  const web = S.tabs.some(t => t.index === S.active && t.kind === "browser");
  board.hidden = S.active !== 0;
  screen.hidden = S.active === 0 || web;
  // ブラウザタブを見ているとき、スマホでは中継画面 (canvas) を出す。
  // 窓 (PC) は今までどおり本物のページを重ねるので中継は使わない
  const cast = document.getElementById("cast");
  cast.hidden = !(web && REMOTE);
  if (web && REMOTE) castStart(); else castStop();
  if (S.active === 0) drawBoard();
  drawVeil();
  // 遡っているあいだは、そう言っておく。押せば今へ戻る
  const b = document.getElementById("back");
  const away = !screen.hidden && S.scrolled > 0;
  b.hidden = !away;
  if (away) {
    b.textContent = (T["tui.scrolled"] || "").replace("{lines}", S.scrolled);
    b.onclick = () => send({kind:"scroll", by: -1000000});
  }
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
// 最後に言われたカーソルの居場所。測り直したら、ここへ置き直す
let lastCur = null;

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
  // 中身の桁もカーソルも、この1つの数から置く。
  //
  // 以前は中身が ch (フォントが言う「0」の送り)、カーソルが測った値、と
  // 別々の数で並んでいた。2つが少しでも違うと、桁が進むほど差が積もり、
  // 打つほどカーソルが右へ離れていった。どちらが正しいかではなく、
  // 同じ数で置くことが要る
  scr.style.setProperty("--cw", cellW + "px");
}

// フォントは後から届く。届く前に測ると、代役の字幅で桁が決まってしまう。
// 届いたら測り直して、置き直す
if (document.fonts && document.fonts.ready) {
  document.fonts.ready.then(() => {
    cellW = 0;
    measure();
    lastRC = "";
    report();
    if (lastCur) window.__cursor(lastCur[0], lastCur[1], lastCur[2]);
  });
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
  // 行桁は #main から、ブラウザの置き場所は #page から取る。
  // 1つの矩形から両方出すと、上のバーを出しただけで端末まで縮み、
  // ブラウザのタブへ切り替えただけでAIの画面が折り返し直される
  const area = document.getElementById("page").getBoundingClientRect();
  // ブラウザを置く場所。外皮のCSSを変えても、知っているのは
  // ページなので、Rust側で座標を推測させない
  const key = rows + "x" + cols + "@" + Math.round(area.left) + "," +
    Math.round(area.top) + "," + Math.round(area.width) + "," + Math.round(area.height);
  if (key === lastRC) return;
  lastRC = key;
  send({kind:"resize", rows:rows, cols:cols,
    area:[Math.round(area.left), Math.round(area.top),
          Math.round(area.width), Math.round(area.height)]});
}
let rt = 0;
window.addEventListener("resize", () => { clearTimeout(rt); rt = setTimeout(report, 80); });

window.__cursor = function (row, col, shown) {
  lastCur = [row, col, shown];
  if (!cellW) measure();
  const pad = parseFloat(getComputedStyle(scr).paddingLeft) || 0;
  const padT = parseFloat(getComputedStyle(scr).paddingTop) || 0;
  // #cur も #kbd も #main の中にある。left/top は #main からの距離なので、
  // 画面全体の座標を入れるとタブバーの幅だけ右へずれる
  const frame = document.getElementById("main").getBoundingClientRect();
  const box = scr.getBoundingClientRect();
  // 中身は動かせる。動いた分だけ、文字も一緒に動いている
  curX = (box.left - frame.left) + pad + col * cellW - scr.scrollLeft;
  curY = (box.top - frame.top) + padT + row * cellH - scr.scrollTop;
  kbd.style.left = curX + "px";
  kbd.style.top = curY + "px";
  kbd.style.height = cellH + "px";
  // 端末を見ていないときは出さない。盤面やブラウザの上に
  // 前のカーソルが残ると、そこに何かあるように見える
  cur.hidden = !shown || composing || S === null || scr.hidden;
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
  // 幅も #main の中で数える。はみ出すなら左へ寄せる
  const room0 = document.getElementById("main").clientWidth;
  let left = curX;
  if (curX + need > room0 - 8) {
    left = Math.max(0, room0 - need - 8);
  }
  kbd.style.left = left + "px";
  const room = Math.max(cellW, room0 - left - 8);
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
// ただしURL欄を打っている最中は奪わない。奪うと1文字も入らない
// 端末タブ (セッション) を見ているか。INDEX(0)・ブラウザ・状態不明は false
const onTerminal = () => S && S.active !== 0 && S.tabs &&
  S.tabs.some(t => t.index === S.active && t.kind !== "browser");
const focus = () => {
  const a = document.activeElement;
  if (a && a.closest && a.closest("#nav")) return;
  // 入力バーに乗っているフォーカスは奪わない (castInput は後で let 宣言される
  // ので起動時に評価しても TDZ で落ちないよう id で見る)
  if (a && a.id === "castinput") return;
  // スマホは端末タブのときだけキーボードを出す。読み込み直後(S未取得)や
  // INDEX・ブラウザでは出さない — 勝手に鍵盤が立ち上がって邪魔になるため。
  // 窓 (PC) は従来どおり (メニューのキー操作に #kbd が要る)
  if (REMOTE && !onTerminal()) return;
  kbd.focus();
};
// ホイールで過去を遡る。
//
// 端末に見えているのは今の1画面だけで、続きは向こうが持っている。
// ここで巻き戻す量を伝え、どこを見せるかは持っている側が決める
scr.addEventListener("wheel", e => {
  if (!S || S.active === 0 || scr.hidden) return;
  e.preventDefault();
  if (!cellW || !cellH) measure();
  // 全画面のプログラムは自分で巻き戻す。どのマスの上かを一緒に渡す
  const pad = parseFloat(getComputedStyle(scr).paddingLeft) || 0;
  const padT = parseFloat(getComputedStyle(scr).paddingTop) || 0;
  const box = scr.getBoundingClientRect();
  const col = Math.max(0, Math.floor((e.clientX - box.left - pad) / cellW));
  const row = Math.max(0, Math.floor((e.clientY - box.top - padT + scr.scrollTop) / cellH));
  // 上へ回す (deltaY < 0) = 遡る。1目盛りを1つと数える
  const n = Math.max(1, Math.round(Math.abs(e.deltaY) / 100));
  send({kind:"scroll", by: e.deltaY < 0 ? n : -n, row: row, col: col});
}, {passive:false});

// 上のバーは普通の入力欄として振る舞わせる。選んだだけで写し取られたり、
// 右クリックが端末への貼り付けになったりしては、URLを直せない
const inBar = e => e.target && e.target.closest && e.target.closest("#nav");
document.addEventListener("mouseup", e => {
  if (inBar(e)) return;
  const s = window.getSelection();
  const t = s ? s.toString() : "";
  if (t) { send({kind:"copy", text:t}); return; }
  focus();
});
document.addEventListener("contextmenu", e => {
  if (inBar(e)) return;
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

// ── 画面中継 (ブラウザタブをスマホから見る・触る) ──────────────
// フレームは /ws で下り、指の操作は /ws-in で上り。別々の単方向WSにして
// どちらも詰まらせない。座標は 0..1 の割合で送り、端末の大きさに依らせない
let castWs = null, castIn = null, castCtx = null, castBound = false;
function castStart() {
  if (!REMOTE || castWs) return;
  const cv = document.getElementById("cast");
  castCtx = cv.getContext("2d");
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const base = proto + "//" + location.host;
  const tok = encodeURIComponent(TOKEN);
  castWs = new WebSocket(base + "/ws?t=" + tok);
  castWs.binaryType = "blob";
  castWs.onmessage = async (e) => {
    try {
      const bmp = await createImageBitmap(e.data);
      if (cv.width !== bmp.width || cv.height !== bmp.height) {
        cv.width = bmp.width; cv.height = bmp.height;
        // フレームでキャンバス寸法が変わったら、カーソルの位置も計算し直す
        // (最初のフレーム前は 300x150 の既定寸法でズレるため)
        if (castMode) posCursor();
      }
      castCtx.drawImage(bmp, 0, 0);
      if (bmp.close) bmp.close();
    } catch (err) {}
  };
  castWs.onclose = () => { castWs = null; };
  castIn = new WebSocket(base + "/ws-in?t=" + tok);
  castIn.onclose = () => { castIn = null; };
  bindCastInput(cv);
}
function castStop() {
  if (castWs) { castWs.close(); castWs = null; }
  if (castIn) { castIn.close(); castIn = null; }
  exitCast();
}
function sendIn(o) {
  if (castIn && castIn.readyState === 1) castIn.send(JSON.stringify(o));
}
// 中身 (object-fit:contain のレターボックス) の矩形を client 座標で返す。
// 画像は object-position:top center で置いているので、横は中央・縦は上詰め。
// ここを中央計算のままにすると、カーソルとクリック位置が縦にずれる
function castRect(cv) {
  const r = cv.getBoundingClientRect();
  const cw = cv.width || 1, ch = cv.height || 1;
  const s = Math.min(r.width / cw, r.height / ch);
  const dw = cw * s, dh = ch * s;
  return { ox: r.left + (r.width - dw) / 2, oy: r.top, dw, dh };
}

// トラックパッド式カーソル。
//   1) キャストをタップ → 操作モードに入りカーソルが出る (この時はクリックしない)
//   2) ドラッグ → カーソルが相対移動 (指で対象が隠れないので小さい的も狙える)
//   3) タップ → カーソル位置をクリック
//   4) 素早く2度目タップして動かす → 掴んでドラッグ (CAPTCHAのスライダー等)
//   5) 2本指ドラッグ → スクロール
//   6) 上のバーや操作中バッジをタップ → 解除
let castMode = false, cx = 0.5, cy = 0.5, cursorEl = null, modeEl = null, dragging = false;
let modCtrl = false, modAlt = false;   // Ctrl/Alt 固定トグル
const CURSOR_ACCEL = 1.25;
function clamp01(v) { return v < 0 ? 0 : v > 1 ? 1 : v; }
function ensureCursor() {
  if (!cursorEl) {
    cursorEl = el("div", {id:"castcursor"});
    // Windows標準に似た矢印。先端 (viewBox の 1,1) がクリック点。
    // CSSの負マージンで先端を left/top にぴったり合わせる
    cursorEl.innerHTML = '<svg width="19" height="30" viewBox="0 0 12 19">' +
      '<path d="M1 1 L1 15 L4.5 11.5 L7 17 L9 16 L6.5 10.5 L11 10.5 Z" ' +
      'fill="#000" stroke="#fff" stroke-width="1" stroke-linejoin="round"/></svg>';
    document.getElementById("main").append(cursorEl);
  }
  if (!modeEl) {
    // 解除の案内 (タップで操作モードを抜ける)。下だとキーボードに隠れるので
    // 画面の上に置く。キーボードは下から出るので、上なら常に押せる
    const lbl = el("span", {}, T["tui.cast.control"] || "操作中 — タップで解除");
    modeEl = el("div", {id:"castmode"}, lbl);
    modeEl.onclick = exitCast;
    document.getElementById("main").append(modeEl);
  }
}
// クリックした場所に波紋を出す (押せたことが分かるように)
function spawnRipple() {
  const cv = document.getElementById("cast");
  const cw = cv.width || 1, ch = cv.height || 1, mw = cv.clientWidth, mh = cv.clientHeight;
  const s = Math.min(mw / cw, mh / ch), dw = cw * s, dh = ch * s, ox = (mw - dw) / 2;
  const r = el("div", {class:"ripple"});
  r.style.left = (cv.offsetLeft + ox + cx * dw) + "px";
  r.style.top = (cv.offsetTop + cy * dh) + "px";
  document.getElementById("main").append(r);
  setTimeout(() => r.remove(), 480);
}
// 文字入力バー。画面下部にプレビュー入力欄を出し、そこで日本語変換を
// 見ながら打ち、確定してから「送信」でまとめて送る。こうすると:
//   - 変換の途中経過が自分の欄で見える (漢字に直してから送れる)
//   - 中継画面はバーの上にそのまま見えるので、入力先を見失わない
//   - キーボードはバーの下に出る (visualViewport でバーを鍵盤の上へ)
// 補助キーの表示名。無い名前はそのまま大文字化して出す
const CAST_LABEL = {
  esc:"Esc", tab:"Tab", space:"Space", enter:"⏎", backspace:"⌫", delete:"Del",
  left:"←", up:"↑", down:"↓", right:"→",
  home:"Home", end:"End", pageup:"PgUp", pagedown:"PgDn", ctrl:"Ctrl", alt:"Alt" };
function castKeyLabel(name) { return CAST_LABEL[name] || name.toUpperCase(); }
// 補助キーを一発送る。Ctrl/Alt が固定トグル中なら合成し、送ったら解除する
function sendCastKey(name) {
  sendIn({kind:"inject", what:"key", named:name, ctrl:modCtrl, alt:modAlt});
  if (modCtrl || modAlt) { modCtrl = false; modAlt = false; refreshMods(); }
}
// 固定トグルの見た目を今の状態に合わせる
function refreshMods() {
  if (!castKeysEl) return;
  castKeysEl.querySelectorAll(".castkey.mod").forEach(b => {
    const on = (b.dataset.k === "ctrl" && modCtrl) || (b.dataset.k === "alt" && modAlt);
    b.classList.toggle("on", on);
  });
}
function buildCastKeys() {
  const row = el("div", {id:"castkeys"});
  const keys = (CAST_KEYS && CAST_KEYS.length) ? CAST_KEYS
    : ["esc","tab","left","up","down","right","space","enter","backspace"];
  keys.forEach(name => {
    const isMod = (name === "ctrl" || name === "alt");
    const b = el("button", {class:"castkey" + (isMod ? " mod" : ""), "data-k":name}, castKeyLabel(name));
    // 入力欄のフォーカス(＝キーボード)を保つため、押下で既定動作を止める
    b.addEventListener("pointerdown", (e) => e.preventDefault());
    b.onclick = (e) => {
      e.stopPropagation();
      if (isMod) {
        if (name === "ctrl") modCtrl = !modCtrl; else modAlt = !modAlt;
        refreshMods();
      } else { sendCastKey(name); }
    };
    row.append(b);
  });
  return row;
}
let castDock = null, castBar = null, castInput = null, castKeysEl = null;
function ensureBar() {
  if (castDock) return;
  castInput = el("input", {id:"castinput", autocomplete:"off", autocorrect:"off",
    autocapitalize:"off", spellcheck:"false",
    placeholder: T["tui.cast.type.ph"] || "ここで入力して送信 (日本語は変換してから)"});
  const send = el("button", {class:"castsend", onclick:sendBar}, T["tui.cast.send"] || "送信");
  const bs = el("button", {class:"castbtn", onclick:() => sendCastKey("backspace")}, "⌫");
  // ✕ はキーボードを下げるだけ (サブ入力欄は操作モード中ずっと出しておく)
  const close = el("button", {class:"castbtn", onclick:() => { if (castInput) castInput.blur(); }}, "✕");
  castBar = el("div", {id:"castbar"}, bs, castInput, send, close);
  castKeysEl = buildCastKeys();
  // 上段=補助キー列、下段=文字入力バー。まとめてキーボードの上へ持ち上げる
  castDock = el("div", {id:"castdock"}, castKeysEl, castBar);
  document.getElementById("main").append(castDock);
  castInput.addEventListener("keydown", (e) => {
    if (e.isComposing) return;
    if (e.key === "Enter") { sendBar(); e.preventDefault(); }
  });
  // キーボードの高さぶんドックを持ち上げる (鍵盤に隠れないように)
  if (window.visualViewport) {
    const fit = () => {
      const gap = window.innerHeight - window.visualViewport.height - window.visualViewport.offsetTop;
      castDock.style.bottom = Math.max(0, gap) + "px";
    };
    window.visualViewport.addEventListener("resize", fit);
    window.visualViewport.addEventListener("scroll", fit);
  }
}
// サブ入力欄 (補助キー列＋入力欄) を出す。フォーカスはしない=キーボードは
// 勝手に出さない。ユーザーが入力欄をタップしたときだけ鍵盤が上がる
function showDock() { ensureBar(); castDock.style.display = "flex"; }
function closeBar() { if (castDock) castDock.style.display = "none"; if (castInput) castInput.blur(); }
function sendBar() {
  if (!castInput) return;
  const t = castInput.value;
  if (t) {
    sendIn({kind:"inject", what:"text", text:t});   // 確定済みの文字列をまとめて送る
  } else {
    sendCastKey("enter");   // 空のまま送信 = Enter (検索確定・フォーム送信など)
  }
  castInput.value = "";
  castInput.focus();
}
// カーソルは #main 内の絶対配置。#cast の中身 (contain・上詰め) の位置を
// #main 基準で求める。ビューポートや上部バーの高さに依存させない
function posCursor() {
  const cv = document.getElementById("cast");
  const cw = cv.width || 1, ch = cv.height || 1;
  const mw = cv.clientWidth, mh = cv.clientHeight;
  const s = Math.min(mw / cw, mh / ch);
  const dw = cw * s, dh = ch * s;
  const ox = (mw - dw) / 2;   // 横は中央
  // キャンバス自身の位置 (バーのぶん下がっている) を足して #main 基準に直す
  cursorEl.style.left = (cv.offsetLeft + ox + cx * dw) + "px";
  cursorEl.style.top = (cv.offsetTop + cy * dh) + "px";   // 縦は上詰め (oy=0)
}
// 操作モードに入ったら、サブ入力欄を常時出す (ボタンを押さなくても補助キーが使える)
function enterCast() { ensureCursor(); castMode = true; cursorEl.style.display = "block"; modeEl.style.display = "block"; showDock(); posCursor(); }
function exitCast() { castMode = false; dragging = false; if (cursorEl) cursorEl.style.display = "none"; if (modeEl) modeEl.style.display = "none"; closeBar(); }
// 矢印の先端の真下にある要素。自前の矢印と波紋は pointer-events:none なので
// 透けて、その下の本物 (バーのボタン/URL欄、または中継キャンバス) が返る
function underCursor() {
  if (!cursorEl) return null;
  const m = document.getElementById("main").getBoundingClientRect();
  const x = m.left + (parseFloat(cursorEl.style.left) || 0);
  const y = m.top + (parseFloat(cursorEl.style.top) || 0);
  return document.elementFromPoint(x, y);
}
const click = () => {
  // 矢印が自前のバー (戻る/進む/更新/URL) の上にあるなら、ブラウザへ注入せず
  // そのUIを直接操作する。直タップと同じ挙動をカーソルでも得られる
  const hit = underCursor();
  if (hit && hit.closest && hit.closest("#nav")) {
    const b = hit.closest("button");
    if (b) { b.click(); spawnRipple(); return; }
    const inp = hit.closest("input");
    if (inp) { inp.focus(); if (inp.select) inp.select(); spawnRipple(); return; }
    return;   // バーの余白 — 誤ってページを押さないよう何もしない
  }
  sendIn({kind:"inject", what:"mouse", phase:"pressed",  x:cx, y:cy, down:true});
  sendIn({kind:"inject", what:"mouse", phase:"released", x:cx, y:cy, down:false});
  spawnRipple();
};
function bindCastInput(cv) {
  if (castBound) return; castBound = true;
  const pts = new Map(); let lastTapT = 0, moved = false, startT = 0;
  cv.addEventListener("pointerdown", (e) => {
    pts.set(e.pointerId, 1); try { cv.setPointerCapture(e.pointerId); } catch (x) {}
    e.preventDefault();
    if (!castMode) { enterCast(); return; }   // 最初のタップは入場だけ
    if (pts.size >= 2) return;                 // 2本指はスクロール
    startT = Date.now(); moved = false;
    if (Date.now() - lastTapT < 300) {         // タップ&ドラッグ = 掴む
      dragging = true;
      sendIn({kind:"inject", what:"mouse", phase:"pressed", x:cx, y:cy, down:true});
    }
  });
  cv.addEventListener("pointermove", (e) => {
    if (!castMode) return; e.preventDefault();
    if (pts.size >= 2) {                        // 2本指: 縦の動きをホイールに
      const dy = e.movementY || 0;
      if (dy) sendIn({kind:"inject", what:"wheel", x:cx, y:cy, dx:0, dy:-dy * 3});
      return;
    }
    const mx = e.movementX || 0, my = e.movementY || 0;
    if (Math.abs(mx) + Math.abs(my) > 2) moved = true;
    const r = castRect(cv);
    cx = clamp01(cx + mx * CURSOR_ACCEL / r.dw);
    cy = clamp01(cy + my * CURSOR_ACCEL / r.dh);
    posCursor();
    sendIn({kind:"inject", what:"mouse", phase:"moved", x:cx, y:cy, down:dragging});
  });
  const up = (e) => {
    pts.delete(e.pointerId);
    if (!castMode) return; e.preventDefault();
    if (pts.size >= 1) return;                  // まだ指が残っている
    if (dragging) { sendIn({kind:"inject", what:"mouse", phase:"released", x:cx, y:cy, down:false}); dragging = false; return; }
    if (!moved && Date.now() - startT < 300) { click(); lastTapT = Date.now(); }
  };
  cv.addEventListener("pointerup", up);
  cv.addEventListener("pointercancel", up);
  // PCブラウザでの確認用にマウスホイールも通す
  cv.addEventListener("wheel", (e) => {
    if (!castMode) return;
    sendIn({kind:"inject", what:"wheel", x:cx, y:cy, dx:e.deltaX, dy:e.deltaY});
    e.preventDefault();
  }, {passive:false});
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
        // その区間が何マス分か。字送りではなく、これで場所が決まる
        let mut span = 0usize;
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
                    flush_run(&mut out, &prev, &run, span);
                    run.clear();
                    span = 0;
                }
                open = Some(style.clone());
            }
            let ch = cell.contents();
            let wide = cell.is_wide();
            // 素の英数字だけをまとめる。
            //
            // マスの幅は英数字を測って決めているので、英数字は箱にぴったり入る。
            // それ以外は別のフォントから来ることがあり、字送りがマスと合わない。
            // 日本語は1文字あたり2マスより狭く出るので、まとめると足りない分が
            // 区間の末尾に溜まる。40文字も打てば、文字列の終わりとカーソルの間に
            // マス10個ぶんの隙間ができていた。1文字ずつ箱に入れれば、
            // 足りない分は文字と文字の間に均され、どこにも溜まらない
            if !wide && (ch.is_empty() || ch.chars().all(|c| c.is_ascii())) {
                span += 1;
                if ch.is_empty() {
                    run.push(' ');
                } else {
                    esc_into(&mut run, ch);
                }
                continue;
            }
            // まとめられないものは、その1文字だけで1つの箱に入れる
            flush_run(&mut out, style.as_str(), &run, span);
            run.clear();
            span = 0;
            let mut one = String::new();
            esc_into(&mut one, ch);
            flush_cell(&mut out, style.as_str(), &one, if wide { 2 } else { 1 });
        }
        if let Some(prev) = open.take() {
            flush_run(&mut out, &prev, &run, span);
        }
        out.push('\n');
    }
    out
}

/// 1区間を書き出す。`span` はその区間が占めるマス数。
///
/// 幅を書いておかないと、字送りの合わないフォントが1文字混ざるだけで
/// その行の残り全部がずれる。罫線も日本語もそれに当たる
fn flush_run(out: &mut String, style: &str, run: &str, span: usize) {
    if run.is_empty() {
        return;
    }
    // 行末の空白は場所を決める必要がない (後ろに何も無い)
    if style.is_empty() && run.trim_end().is_empty() {
        out.push_str(run);
        return;
    }
    // マスの幅は、画面が測ってCSS変数に入れてくれる。
    // ch (フォントが言う「0」の送り) で置くと、カーソルを置く数と
    // 別の数になり、桁が進むほど離れていく
    box_of(out, style, run, span, false);
}

/// 1マス (全角なら2マス) を、その1文字だけで書き出す。
///
/// 字送りがマスに満たない文字は、真ん中に置く。左に寄せると
/// 文字の右側にだけ隙間が並び、揃っていないように見える
fn flush_cell(out: &mut String, style: &str, ch: &str, span: usize) {
    box_of(out, style, ch, span, true);
}

fn box_of(out: &mut String, style: &str, body: &str, span: usize, center: bool) {
    // マスの幅は、画面が測ってCSS変数に入れてくれる。
    // ch (フォントが言う「0」の送り) で置くと、カーソルを置く数と
    // 別の数になり、桁が進むほど離れていく
    out.push_str("<span style=\"display:inline-block;vertical-align:top;width:calc(var(--cw)*");
    out.push_str(&span.to_string());
    out.push_str(");");
    if center {
        out.push_str("text-align:center;overflow:hidden;");
    }
    out.push_str(style);
    out.push_str("\">");
    out.push_str(body);
    out.push_str("</span>");
}

/// 訳語とビルド刻印を埋めて、配れる形にする
/// 盤面が出すメニュー (押す文字, 訳語のキー)。
///
/// 押された文字は、INDEX を見ているときの打鍵としてそのまま届く。
/// 受け手 (INDEX の分岐) が知らない文字をここに足すと、
/// 「出ているのに押しても何も起きない」ができあがる
pub const MENU: [(&str, &str); 6] = [
    ("e", "tui.menu.settings"),
    ("i", "tui.menu.phone"),
    ("r", "tui.menu.restart"),
    ("w", "tui.menu.workspace"),
    ("t", "tui.menu.notify"),
    ("?", "tui.menu.help"),
];

pub fn page(token: &str) -> String {
    let dict = crate::i18n::dict_json();
    let keys: Vec<&str> = MENU.iter().map(|(k, _)| *k).collect();
    let words: std::collections::BTreeMap<&str, &str> = MENU.iter().copied().collect();
    PAGE.replace(
        "{{MENU_KEYS}}",
        &serde_json::to_string(&keys).unwrap_or_else(|_| "[]".into()),
    )
    .replace(
        "{{MENU_WORDS}}",
        &serde_json::to_string(&words).unwrap_or_else(|_| "{}".into()),
    )
    .replace("{{DICT}}", &dict)
        .replace(
            "{{CAST_KEYS}}",
            &serde_json::to_string(&crate::config::cast_keys()).unwrap_or_else(|_| "[]".into()),
        )
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


    /// ページが読む訳語のキーが、辞書にあること。
    ///
    /// 無いキーは空文字になる。落ちも警告も出ないので、
    /// 「押したのに何も出ない」という形でしか気づけない。
    /// 実際、ヘルプは tui.help.body という無いキーを読んでいて、
    /// 題名だけの空の箱が出ていた。押しても効いていないように見える。
    ///
    /// これを見ていた試験は前にもあったが、
    /// 古いスマホ用ページを消したときに一緒に消えてしまった
    #[test]
    fn every_word_the_page_asks_for_is_in_the_dictionary() {
        let en: serde_json::Value =
            serde_json::from_str(include_str!("../lang/en.json")).unwrap();
        let p = super::page("");
        let mut rest = p.as_str();
        let mut checked = 0;
        // T["..."] の形で読んでいるものを拾う
        while let Some(i) = rest.find("T[\"") {
            rest = &rest[i + 3..];
            let key = &rest[..rest.find('"').expect("閉じていない")];
            // 末尾が点なら組み立てて読む形 (T["tui.help." + k])。下で見る
            if key.ends_with('.') {
                continue;
            }
            assert!(en.get(key).is_some(), "lang/en.json に無いキー: {key}");
            checked += 1;
        }
        // 組み立てて読む分 (T["tui.help." + k]) も、並べた名前ごと見る
        let head = "T[\"tui.help.\" + k]";
        if p.contains(head) {
            for k in [
                "quit", "tabs", "ws", "lock", "restart", "copy", "auto", "raw", "mouse",
                "mouse.wheel", "mouse.drag", "mouse.right", "mouse.tab", "mouse.divider",
            ] {
                let key = format!("tui.help.{k}");
                assert!(en.get(&key).is_some(), "lang/en.json に無いキー: {key}");
                checked += 1;
            }
        }
        assert!(checked > 20, "訳語をほとんど読んでいない ({checked}件)");
    }

    /// 押している間は、盤面を作り直さないこと。
    ///
    /// click は「押し下げと押し上げが同じ要素で起きる」ことで成立する。
    /// 状態が届くたびに盤面を全部作り直していたので、押している最中に
    /// 作り直されると、押した要素はもう無く、押下はどこにも届かなかった。
    /// 活動グラフは絶えず動くため、これは稀な事故ではなく既定の動作で、
    /// INDEXのメニューはマウスでは押せなかった。
    ///
    /// 見た目を確かめないと気づけない類なので、ここで押さえる
    #[test]
    fn a_press_is_not_interrupted_by_a_redraw() {
        let p = super::page("");
        // 描き直しの入口に、押している間の預かりがあること
        let at = p.find("window.__state = function").expect("状態の入口が無い");
        let head = &p[at..at + 200];
        assert!(
            head.contains("holding") && head.contains("queued"),
            "状態が届いたら、押している最中でも作り直してしまう"
        );
        // 離したら、預かった分を必ず流すこと (押した後に画面が止まらない)
        assert!(
            p.contains("addEventListener(\"pointerup\", release"),
            "離したときに、預かった描き直しを流していない"
        );
        // 窓の外で離された場合の逃げ道。無いと押しっぱなしのまま固まる
        assert!(
            p.contains("addEventListener(\"pointercancel\", release")
                && p.contains("addEventListener(\"blur\", release"),
            "押しっぱなしのまま画面が止まる道が残っている"
        );
    }

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

    /// 上のバーは、ページの中ではなく外皮に描くこと。
    ///
    /// ページの中に差し込むと、相手のCSSと喧嘩し、遷移のたびに消え、
    /// サイト自身の固定ヘッダーを上から覆う。ページを一段下げて
    /// 空いた場所に描けば、どれも起きない
    #[test]
    fn the_bar_is_drawn_by_the_app_not_injected_into_the_page() {
        assert!(PAGE.contains("id=\"nav\""), "バーの置き場所が無い");
        assert!(PAGE.contains("id=\"page\""), "ページを置く場所が無い");
        // 置き場所はバーのぶんだけ下がる
        assert!(
            PAGE.contains("const top = n.hidden ? \"0\" : \"36px\"")
                && PAGE.contains("getElementById(\"page\").style.top = top"),
            "バーを出してもページが下がらない"
        );
        // 中継キャンバスも同じだけ下げる (下げないとブラウザ上端がバーに隠れる)
        assert!(
            PAGE.contains("getElementById(\"cast\").style.top = top"),
            "バーを出しても中継キャンバスが下がらず、ブラウザ上端が隠れる"
        );
        // 行桁は #main、ブラウザの置き場所は #page。
        // 1つの矩形から両方出すと、バーを出しただけで端末まで縮む
        assert!(
            PAGE.contains("document.getElementById(\"page\").getBoundingClientRect()"),
            "置き場所を #page から取っていない"
        );
    }

    /// URL欄を打っている間は、打鍵が端末へ流れないこと。
    /// 流れると、行き先を書いているつもりでAIに文字を送ることになる
    #[test]
    fn typing_an_address_does_not_reach_the_terminal() {
        assert!(PAGE.contains("e.stopPropagation();"), "打鍵を止めていない");
        // 選択やクリックで入力欄から焦点を奪わない
        assert!(
            PAGE.contains("if (a && a.closest && a.closest(\"#nav\")) return;"),
            "入力中に焦点を奪っている"
        );
        assert!(PAGE.contains("if (inBar(e)) return;"), "バーの中で端末の作法が働く");
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
    use super::{PAGE, screen_html};

    fn render(input: &str) -> String {
        let mut p: vt100::Parser = vt100::Parser::new(3, 40, 0);
        p.process(input.as_bytes());
        screen_html(p.screen())
    }

    /// どの区間も、自分が何マス分かを持っていること。
    ///
    /// 端末はマスで桁を数え、ブラウザは字送りで並べる。この2つが
    /// 一致するフォントは無い。Cascadia Mono の英字は 0.586em、
    /// 全角は 1.0em で、倍にならない。罫線を1マスで描くフォントに
    /// 替えても、日本語が並べばずれる。
    ///
    /// マス数を書いておけば、字送りが何であっても次の区間は正しい場所から
    /// 始まる。フォントは見た目だけの話になる
    #[test]
    fn every_run_carries_its_own_width() {
        // 全角・罫線・英字が混ざった行
        let html = render("\u{1b}[31mあ\u{1b}[0m\u{2502}ab");
        for piece in html.split("<span").skip(1) {
            assert!(
                piece.contains("width:calc(var(--cw)*"),
                "マス数を持たない区間がある: {piece}"
            );
        }
        // 全角は2マス
        assert!(
            html.contains("width:calc(var(--cw)*2)"),
            "全角が2マスになっていない: {html}"
        );
        // 半角3文字の区間は3マス
        let three = render("\u{1b}[31mabc\u{1b}[0m");
        assert!(
            three.contains("width:calc(var(--cw)*3)"),
            "半角3文字が3マスになっていない: {three}"
        );
        // 罫線も1マス。端末がそう数えているので、描く側も合わせる
        let line = render("\u{1b}[31m\u{2502}\u{1b}[0m");
        assert!(
            line.contains("width:calc(var(--cw)*1)"),
            "罫線が1マスになっていない: {line}"
        );
    }

    /// 字送りがマスに合わない文字は、1文字ずつ箱に入れること。
    ///
    /// 日本語の字送りは2マスより狭い (別のフォントから来るので合わない)。
    /// まとめて1つの箱に入れると、足りない分が区間の末尾に溜まる。
    /// 40文字で、文字列の終わりとカーソルの間にマス10個ぶんの隙間ができていた
    #[test]
    fn a_letter_that_does_not_fill_its_cell_gets_a_box_of_its_own() {
        let html = render("あいう");
        assert_eq!(
            html.matches("width:calc(var(--cw)*2)").count(),
            3,
            "全角がまとめられている: {html}"
        );
        assert!(html.contains("text-align:center;"), "マスの中で寄っている: {html}");

        // 英数字はマスにぴったり入るので、まとめてよい (要素を増やさない)
        let ascii = render("\u{1b}[31mabcdef\u{1b}[0m");
        assert_eq!(
            ascii.matches("width:calc(var(--cw)*").count(),
            1,
            "英数字まで1文字ずつ切っている: {ascii}"
        );
    }

    /// 中身とカーソルが、同じ1つの数で置かれること。
    ///
    /// 別々の数 (中身は ch、カーソルは測った値) にすると、
    /// その差が桁ごとに積もる。打つほどカーソルが右へ離れていった
    #[test]
    fn the_text_and_the_cursor_share_one_cell_width() {
        assert!(
            !render("ab").contains("ch;"),
            "フォントが言う字送りで桁を置いている"
        );
        assert!(
            PAGE.contains("scr.style.setProperty(\"--cw\", cellW + \"px\")"),
            "測った幅を中身へ渡していない"
        );
        // カーソルも同じ cellW から置く
        assert!(PAGE.contains("col * cellW"), "カーソルが別の数で置かれている");
    }

    /// 行末の空白は場所を決めなくてよいこと。
    /// 後ろに何も無いので、40桁ぶんの箱を毎行置く意味がない
    #[test]
    fn the_blank_tail_of_a_line_needs_no_box() {
        let html = render("ab");
        let first = html.lines().next().unwrap_or_default();
        assert!(first.starts_with("<span"), "{first}");
        assert!(
            first.trim_end().ends_with("</span>") || first.ends_with(' '),
            "行末の空白まで箱に入れている: {first:?}"
        );
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
