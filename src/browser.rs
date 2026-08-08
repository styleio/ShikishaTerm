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
    // 押したことがその場で分かるようにする。手応えが無いと、
    // 押せたのか、押せていないのか、何も起きない仕事なのかが区別できない。
    // 二度押しも防げる (受け取る側は1回しか来ないと思っている)
    b.onclick = () => {
      if (b.disabled) return;
      b.disabled = true;
      b.style.opacity = ".45";
      b.style.cursor = "default";
      send({ kind: "button" });
    };
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

  // 「読み込み終わった」は load まで待つ。DOMContentLoaded の時点では
  // 画像もCSSも来ておらず、JSが後から作る中身も入っていない。
  //
  // ただし広告のページは外部の計測タグを待つので、load が数秒遅れる、
  // 来ないことがある。待ち切れなければDOMだけの時点で名乗り、
  // どちらだったかを complete に入れる。当てにいって外すより正直
  let told = false;
  const announce = complete => {
    if (told) return;
    told = true;
    send({ kind: "ready", url: location.href, complete: !!complete });
  };
  const SETTLE_MS = 8000;
  if (document.readyState === "complete") {
    announce(true);
  } else {
    addEventListener("load", () => announce(true), { once: true });
    const armFallback = () => setTimeout(() => announce(false), SETTLE_MS);
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", armFallback, { once: true });
    } else {
      armFallback();
    }
  }
})();
"#;

/// 指揮者からブラウザへの指示
#[derive(Debug, Clone)]
pub enum Cmd {
    /// JSを評価して結果を返す (`id` で対応づける)。
    /// `to` は宛先のページ名。None は主画面
    Eval {
        id: u64,
        to: Option<String>,
        js: String,
    },
    /// 人へ呼びかける帯を出す
    Ask {
        to: Option<String>,
        text: String,
        label: String,
    },
    /// 帯を消す
    Unask { to: Option<String> },
    /// 同じ窓の中に、名前を付けてページを置く
    AddChild {
        name: String,
        url: String,
        rect: (i32, i32, i32, i32),
    },
    /// 置いたページの場所と大きさを決める。幅か高さが0なら隠す
    ChildBounds {
        name: String,
        rect: (i32, i32, i32, i32),
    },
    /// 置いたページを取り除く
    RemoveChild { name: String },
    /// キーボードの焦点をこのページへ移す。`to` が None なら主画面。
    ///
    /// ページの中の焦点 (activeElement) と、OSが見ている焦点は別物。
    /// 重ねたページを出し入れするとOS側だけが余所へ残り、
    /// 打鍵は届くのに日本語の変換窓だけが画面の隅に出る、という形で現れる
    Focus { to: Option<String> },
    /// 置いたページを動かす (人が上のバーを押した)
    Move { to: Option<String>, go: Go },
    /// 今どこに居るか、戻れるか進めるかを聞く。
    /// 答えは `Ev::Where` で返る
    Where { to: Option<String> },
    /// 画面の中継 (VNC相当) を始める/止める。
    /// 始めると変化のたびに `Ev::Frame` が届く。`to` は対象ページ (None は主画面)
    Screencast {
        to: Option<String>,
        on: bool,
    },
    /// 中継中の画面へ、実際の入力を注入する (CDP経由。合成ではなく本物の入力扱い)。
    /// 人の指の軌跡もCAPTCHAのスワイプも、届いた点をそのまま再生する
    Inject {
        to: Option<String>,
        input: Input,
    },
    /// 窓を閉じる (指揮者がいなくなったとき)
    Close,
}

/// 中継画面への入力ひとつ。座標は中継フレーム上の割合 (0.0〜1.0) で受け取り、
/// 実ピクセルへ直す。端末の大きさやDPRが送り手と違っても、同じ場所を指せる
#[derive(Debug, Clone)]
pub enum Input {
    /// マウスの押下/移動/解放。ドラッグは move を連ねて表す
    Mouse {
        /// "pressed" / "released" / "moved"
        phase: String,
        x: f64,
        y: f64,
        /// 押している間の移動なら true (ドラッグの再生に要る)
        down: bool,
    },
    /// ホイール。dx/dy はピクセル
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    /// 確定済みの文字列を今の焦点へ挿入する (IME変換は送り手側で済ませる)
    Text { text: String },
    /// 名前付きの制御キー (Enter / Backspace / Tab など)
    Key { named: String },
}

/// ブラウザに頼む移動。
///
/// ページに `history.back()` をやらせる手もあるが、それだと
/// 「もう戻れない」が分からず、押せないボタンを押せる顔で出すことになる。
/// 窓の側は戻れるかを知っているので、そちらに頼む
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Go {
    Back,
    Forward,
    Reload,
    To(String),
}

/// 画面からの意図をひとつ読む。
///
/// 窓 (ipc) からもスマホ (HTTP) からも同じ形で届く。読み方が2か所にあると、
/// 同じ押下が2通りに解釈される日が来るので、ここだけに置く。
/// 知らない `kind` は `None`。黙って捨てるのが正しい
pub fn parse_intent(v: &serde_json::Value) -> Option<Ev> {
    Some(match v.get("kind").and_then(|k| k.as_str()) {
        Some("ready") => Ev::Ready {
            from: None,
            complete: v
                .get("complete")
                .and_then(|x| x.as_bool())
                .unwrap_or(true),
            url: v
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("button") => Ev::Button { from: None },
        Some("select") => Ev::Select {
            tab: v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        },
        Some("addtab") => Ev::AddTab,
        Some("menu") => Ev::Menu {
            key: v
                .get("key")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("stop") => Ev::Stop,
        Some("scroll") => Ev::Scroll {
            // 目盛りは人の指の数。桁外れの数が来ても意味がない
            by: v.get("by").and_then(|x| x.as_i64()).unwrap_or(0).clamp(-64, 64) as i32,
            row: v.get("row").and_then(|x| x.as_u64()).unwrap_or(0).min(9999) as u16,
            col: v.get("col").and_then(|x| x.as_u64()).unwrap_or(0).min(9999) as u16,
        },
        // 上のバー。行き先は人が打った文字なので、ここで型を絞る
        Some("go") => Ev::Go {
            go: match v.get("what").and_then(|x| x.as_str()) {
                Some("back") => Go::Back,
                Some("forward") => Go::Forward,
                Some("reload") => Go::Reload,
                Some("to") => Go::To(
                    v.get("url")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                ),
                _ => return None,
            },
        },
        Some("jserror") => Ev::JsError {
            msg: v
                .get("msg")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("password") => Ev::Password {
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
        },
        Some("resize") => {
            let a = v.get("area").and_then(|x| x.as_array());
            let num = |i: usize| {
                a.and_then(|a| a.get(i))
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0) as i32
            };
            Ev::Resize {
                rows: v.get("rows").and_then(|x| x.as_u64()).unwrap_or(24) as u16,
                cols: v.get("cols").and_then(|x| x.as_u64()).unwrap_or(80) as u16,
                area: (num(0), num(1), num(2), num(3)),
            }
        }
        Some("copy") => Ev::Copy {
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("paste") => Ev::Paste,
        // 中継画面の上でのタッチ/マウス。座標は割合 (0..1) で来る
        Some("inject") => {
            let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
            let what = v.get("what").and_then(|x| x.as_str()).unwrap_or("");
            let input = match what {
                "mouse" => Input::Mouse {
                    phase: v.get("phase").and_then(|x| x.as_str()).unwrap_or("moved").to_string(),
                    x: f("x").clamp(0.0, 1.0),
                    y: f("y").clamp(0.0, 1.0),
                    down: v.get("down").and_then(|x| x.as_bool()).unwrap_or(false),
                },
                "wheel" => Input::Wheel {
                    x: f("x").clamp(0.0, 1.0),
                    y: f("y").clamp(0.0, 1.0),
                    dx: f("dx"),
                    dy: f("dy"),
                },
                "text" => Input::Text {
                    text: v.get("text").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                },
                "key" => Input::Key {
                    named: v.get("named").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                },
                _ => return None,
            };
            Ev::Inject { to: None, input }
        }
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
    _ => return None,
    })
}

/// ブラウザから指揮者への報告
#[derive(Debug, Clone)]
pub enum Ev {
    /// 文書が読み込まれた (遷移のたびに来る)。
    /// `from` は読み込んだページの名前 (None は主画面)
    Ready {
        from: Option<String>,
        url: String,
        /// 参照しているものまで揃ったか。
        /// false は「load が来ないので、DOMだけの時点で名乗った」
        complete: bool,
    },
    /// `Eval` の結果。`value` はJSON
    Result { id: u64, ok: bool, value: String },
    /// 帯のボタンが押された = 人が自分の番を終えた。
    /// `from` は押されたページの名前 (None は主画面)。
    /// 何枚も置ける以上、どれで押されたかを持っていないと
    /// 隣のブラウザの番が終わったことにできてしまう
    Button { from: Option<String> },
    /// 窓の大きさが変わった (何行何桁入るか)
    Resize {
        rows: u16,
        cols: u16,
        /// 中身の領域 (x, y, 幅, 高さ)。ブラウザはここに置く
        area: (i32, i32, i32, i32),
    },
    /// このタブを見たい (0 = 稼働盤)
    Select { tab: usize },
    /// タブバーの + が押された (設定画面をタブ追加の状態で開く)
    AddTab,
    /// 稼働盤のメニューが押された
    Menu { key: String },
    /// 緊急停止
    Stop,
    /// ホイールを回した (正 = 遡る、負 = 戻る)。数は目盛りの数。
    /// `row`/`col` は指していたマス (全画面のプログラムへ渡すのに要る)
    Scroll { by: i32, row: u16, col: u16 },
    /// パスワードの入力結果 (None = 取り消し)
    Password { text: Option<String> },
    /// 画面の中で失敗した
    JsError { msg: String },
    /// 上のバーが押された。宛先は「今見ているブラウザ」なので、
    /// どれ宛かは指揮者が決める (バーは1枚しか出ていない)
    Go { go: Go },
    /// `Cmd::Where` の答え
    Where {
        from: Option<String>,
        url: String,
        can_back: bool,
        can_forward: bool,
    },
    /// 中継画面の1フレーム。base64のJPEG (そのまま data URL にできる)。
    /// `from` は送り元ページ。`w`/`h` はフレームの実ピクセル寸法
    Frame {
        from: Option<String>,
        data: String,
        w: u32,
        h: u32,
    },
    /// 選択された文字 (PuTTY と同じで、選んだ時点でコピーする)
    Copy { text: String },
    /// 貼り付けの要求 (右クリック)
    Paste,
    /// 中継画面への入力要求 (クライアントから届き、指揮者が Cmd::Inject に直す)
    Inject { to: Option<String>, input: Input },
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
    /// 出しっぱなしにしておく帯。ページごとに1つ。
    /// 鍵の None は主画面
    pending_ask: std::sync::Mutex<std::collections::HashMap<Option<String>, (String, String)>>,
    /// 何かを待っている間に届いた、別の合図。
    ///
    /// 読み飛ばして捨てると、待ちの前に送られたものが永久に消える。
    /// 実際それで、窓の桁数が一度も届かなかった
    spare: std::sync::Mutex<Vec<Ev>>,
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
        Self::start(url, title)
    }

    fn start(url: &str, title: &str) -> Result<Self> {
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

        let me = Self {
            proxy,
            events: ev_rx,
            next_id: AtomicU64::new(1),
            pending_ask: std::sync::Mutex::new(std::collections::HashMap::new()),
            spare: std::sync::Mutex::new(Vec::new()),
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
                Ok(Ev::Ready { from, url, .. }) => {
                    self.reask(from.as_deref());
                    return Ok(url);
                }
                Ok(Ev::Closed) => return Err(anyhow!("ブラウザが閉じました")),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!("ページが用意できません")),
            }
        }
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.proxy
            .send_event(cmd)
            .map_err(|_| anyhow!("ブラウザが閉じています"))
    }

    /// JSを評価する。結果は `Ev::Result` で後から届く
    pub fn eval(&self, js: &str) -> Result<u64> {
        self.eval_in(None, js)
    }

    /// 宛先を指してJSを評価する。None は主画面
    pub fn eval_in(&self, to: Option<&str>, js: &str) -> Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Eval {
            id,
            to: to.map(str::to_string),
            js: js.to_string(),
        })?;
        Ok(id)
    }

    pub fn ask(&self, to: Option<&str>, text: &str, label: &str) -> Result<()> {
        self.pending_ask.lock().unwrap().insert(
            to.map(str::to_string),
            (text.to_string(), label.to_string()),
        );
        self.send(Cmd::Ask {
            to: to.map(str::to_string),
            text: text.to_string(),
            label: label.to_string(),
        })
    }

    /// 置いたページを動かす
    pub fn go(&self, to: Option<&str>, go: Go) -> Result<()> {
        self.send(Cmd::Move {
            to: to.map(str::to_string),
            go,
        })
    }

    /// 画面の中継 (VNC相当) を始める/止める。始めると `Ev::Frame` が届く
    pub fn screencast(&self, to: Option<&str>, on: bool) -> Result<()> {
        self.send(Cmd::Screencast {
            to: to.map(str::to_string),
            on,
        })
    }

    /// 中継画面へ入力を注入する (人の指の軌跡・スワイプ・文字)
    pub fn inject(&self, to: Option<&str>, input: Input) -> Result<()> {
        self.send(Cmd::Inject {
            to: to.map(str::to_string),
            input,
        })
    }

    /// キーボードの焦点を移す (None = 主画面)
    pub fn focus(&self, to: Option<&str>) -> Result<()> {
        self.send(Cmd::Focus {
            to: to.map(str::to_string),
        })
    }

    /// 今どこに居るかを聞く (答えは報告として届く)
    pub fn ask_where(&self, to: Option<&str>) -> Result<()> {
        self.send(Cmd::Where {
            to: to.map(str::to_string),
        })
    }

    pub fn unask(&self, to: Option<&str>) -> Result<()> {
        self.pending_ask
            .lock()
            .unwrap()
            .remove(&to.map(str::to_string));
        self.send(Cmd::Unask {
            to: to.map(str::to_string),
        })
    }


    /// 同じ窓の中にページを置く。
    ///
    /// 別窓にすると、所有関係も位置の追従も、
    /// Windows Terminal のタブ切替での露出も、全部こちらの持ち物になる。
    /// 同じ窓に入れれば、どれも起きない
    pub fn open_child(&self, name: &str, url: &str, rect: (i32, i32, i32, i32)) -> Result<()> {
        if !is_openable(url) {
            return Err(anyhow!("開けないURLです: {url}"));
        }
        self.send(Cmd::AddChild {
            name: name.to_string(),
            url: url.to_string(),
            rect,
        })
    }

    /// 置いたページの場所と大きさ。幅か高さを0にすると隠れる
    pub fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> Result<()> {
        self.send(Cmd::ChildBounds {
            name: name.to_string(),
            rect,
        })
    }

    pub fn close_child(&self, name: &str) -> Result<()> {
        self.send(Cmd::RemoveChild {
            name: name.to_string(),
        })
    }

    /// JSを1回呼んで、結果が返るまで待つ
    fn call(
        &self,
        to: Option<&str>,
        func: &str,
        args: &[serde_json::Value],
        timeout_ms: u64,
    ) -> Result<String> {
        let id = self.eval_in(to, &call_js(func, args))?;
        self.wait_result(id, std::time::Duration::from_millis(timeout_ms))
    }

    /// その要素が今どこにいるか
    pub fn find(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        Ok(Found::parse(&self.call(
            to,
            "__shikisha_state",
            &[sel.json()],
            timeout_ms,
        )?))
    }

    /// 文字を読む (入力欄なら中身、それ以外は表示文字列)
    pub fn text(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Option<String>> {
        let v = self.call(to, "__shikisha_text", &[sel.json()], timeout_ms)?;
        Ok(serde_json::from_str::<Option<String>>(&v).unwrap_or(None))
    }

    /// 押す
    pub fn click(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        Ok(Found::parse(&self.call(
            to,
            "__shikisha_click",
            &[sel.json()],
            timeout_ms,
        )?))
    }

    /// 入力欄に値を入れる
    pub fn fill(
        &self,
        to: Option<&str>,
        sel: &Sel,
        value: &str,
        timeout_ms: u64,
    ) -> Result<Found> {
        Ok(Found::parse(&self.call(
            to,
            "__shikisha_fill",
            &[sel.json(), serde_json::Value::String(value.to_string())],
            timeout_ms,
        )?))
    }

    /// 解釈済みのHTML全文
    pub fn html(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        let v = self.call(to, "__shikisha_html", &[], timeout_ms)?;
        Ok(serde_json::from_str::<String>(&v).unwrap_or(v))
    }

    /// 溜まっている報告を取り出す (待たない)。
    /// 新しい文書に移っていたら、出しておくべき帯を出し直す
    pub fn drain(&self) -> Vec<Ev> {
        // 待っている間に来たものを先に返す (届いた順を保つ)
        let mut evs: Vec<Ev> = std::mem::take(&mut *self.spare.lock().unwrap());
        evs.extend(self.events.try_iter());
        for e in &evs {
            if let Ev::Ready { from, .. } = e {
                self.reask(from.as_deref());
            }
        }
        evs
    }

    /// 遷移で消えた帯を出し直す。出し直すのは、遷移したページの分だけ
    fn reask(&self, to: Option<&str>) {
        let key = to.map(str::to_string);
        let want = self.pending_ask.lock().unwrap().get(&key).cloned();
        if let Some((t, l)) = want {
            let _ = self.send(Cmd::Ask {
                to: key,
                text: t,
                label: l,
            });
        }
    }

    /// パスワードが入力されるまで待つ。
    /// 待っている間に来た他の合図は取っておく (捨てると永久に消える)
    pub fn wait_password(&self, timeout: std::time::Duration) -> Result<Option<String>> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!("入力がありません"))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Password { text }) => return Ok(text),
                Ok(Ev::Closed) => return Err(anyhow!("窓が閉じました")),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!("入力がありません")),
            }
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
                Ok(Ev::Ready { from, .. }) => {
                    self.reask(from.as_deref());
                    continue;
                }
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
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

/// 指示の宛先を解く。None は主画面、名前はそのページ。
/// 名前があるのに見つからないときは None を返す。
/// 主画面に落とすと、サイト向けのJSが自分の画面に対して走る
/// 中継画面へ送る制御キーの名前を、CDP が要る (key名, Windows仮想キーコード) に直す
fn named_vk(named: &str) -> Option<(&'static str, u32)> {
    Some(match named {
        "enter" => ("Enter", 13),
        "backspace" => ("Backspace", 8),
        "tab" => ("Tab", 9),
        "escape" | "esc" => ("Escape", 27),
        "delete" => ("Delete", 46),
        "up" => ("ArrowUp", 38),
        "down" => ("ArrowDown", 40),
        "left" => ("ArrowLeft", 37),
        "right" => ("ArrowRight", 39),
        _ => return None,
    })
}

fn target<'a>(
    main: &'a wry::WebView,
    children: &'a std::collections::HashMap<String, wry::WebView>,
    to: &Option<String>,
) -> Option<&'a wry::WebView> {
    match to {
        None => Some(main),
        Some(name) => children.get(name),
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

/// 場所と大きさを wry の形に直す
fn to_rect((x, y, w, h): (i32, i32, i32, i32)) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(x, y).into(),
        size: wry::dpi::LogicalSize::new(w.max(0), h.max(0)).into(),
    }
}

/// 人が打った文字を、開いてよい行き先に直す。
///
/// 綴りを省いたときだけ補う (`example.com` → `https://example.com`)。
/// それ以外は書いたとおりに扱い、http/https でなければ開かない。
/// `file:` は手元のファイルを、`javascript:` は今のページを乗っ取れるので、
/// URL欄という「どこへでも行ける口」から届かせるわけにいかない
pub fn openable(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // 綴りが無いものは全部「相手先」として扱う。`javascript:alert(1)` は
    // https://javascript:alert(1) という壊れた行き先になって開けずに終わる。
    // 綴りかどうかを当てにいくより、当たらなくても危なくない方を選ぶ
    let with_scheme = if s.contains("://") {
        s.to_string()
    } else {
        format!("https://{s}")
    };
    let low = with_scheme.to_ascii_lowercase();
    (low.starts_with("http://") || low.starts_with("https://")).then_some(with_scheme)
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
            let Some(ev) = parse_intent(&v) else {
                return;
            };
            let _ = ipc.send(ev);
        })
        .build(&window)?;

    // 同じ窓に置いたページたち。名前で引く
    let mut children: std::collections::HashMap<String, wry::WebView> =
        std::collections::HashMap::new();

    // 画面中継。対象ごとに1つ。持っている間だけフレームが届く
    let mut casts: std::collections::HashMap<Option<String>, cdp::Cast> =
        std::collections::HashMap::new();
    // 直近フレームのCSSピクセル寸法 (入力注入の座標変換に使う)。
    // フレーム通知と入力注入は同じスレッドなので Rc<Cell> で足りる
    let cast_dims = std::rc::Rc::new(std::cell::Cell::new((0.0f64, 0.0f64)));
    // ドラッグ判定用: 今ボタンを押し下げているか
    let mut mouse_down = false;

    // ループの中でも報告を送るので、閉じたことを伝える分を先に取っておく
    let closed_tx = ev_tx.clone();
    // 「今どこに居るか」の答えを返す線。窓の中でしか分からないので、ここから返す
    let where_tx = ev_tx.clone();
    ev_loop.run_return(move |event, _, control| {
        *control = ControlFlow::Wait;
        match event {
            Event::UserEvent(cmd) => match cmd {
                Cmd::Eval { id, to, js } => {
                    // 宛先が見つからないとき、主画面には落とさない。
                    // サイト向けのJSが自分の画面に対して走ってしまう
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = v.evaluate_script(&wrap_eval(id, &js));
                    } else {
                        let _ = ev_tx.send(Ev::Result {
                            id,
                            ok: false,
                            value: serde_json::Value::String(format!(
                                "ページ '{}' は置かれていません",
                                to.unwrap_or_default()
                            ))
                            .to_string(),
                        });
                    }
                }
                Cmd::Ask { to, text, label } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = v.evaluate_script(&ask_js(&text, &label));
                    }
                }
                Cmd::Unask { to } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = v.evaluate_script(
                            "window.__shikisha_unask&&window.__shikisha_unask();",
                        );
                    }
                }
                Cmd::AddChild { name, url, rect } => {
                    let bounds = to_rect(rect);
                    // 子にも主画面と同じ道具を積む。
                    // 積まないと、置いたページは映っているだけになる
                    let ipc = ev_tx.clone();
                    let who = name.clone();
                    match WebViewBuilder::new()
                        .with_url(&url)
                        .with_bounds(bounds)
                        .with_initialization_script(INIT_JS)
                        .with_ipc_handler(move |req| {
                            let body: &str = req.body();
                            let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
                                return;
                            };
                            let Some(ev) = parse_intent(&v) else {
                                return;
                            };
                            // 誰が押したのかを、ここでしか知りようがない
                            let ev = match ev {
                                Ev::Button { .. } => Ev::Button {
                                    from: Some(who.clone()),
                                },
                                Ev::Ready { url, complete, .. } => Ev::Ready {
                                    from: Some(who.clone()),
                                    url,
                                    complete,
                                },
                                other => other,
                            };
                            let _ = ipc.send(ev);
                        })
                        .build_as_child(&window)
                    {
                        Ok(v) => {
                            children.insert(name, v);
                        }
                        Err(e) => {
                            crate::append_hook_log(&format!("ページを置けません {name}: {e}"))
                        }
                    }
                }
                Cmd::ChildBounds { name, rect } => {
                    if let Some(v) = children.get(&name) {
                        let _ = v.set_bounds(to_rect(rect));
                    }
                }
                Cmd::RemoveChild { name } => {
                    children.remove(&name);
                }
                Cmd::Focus { to } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        if let Err(e) = v.focus() {
                            crate::append_hook_log(&format!("焦点を移せません {to:?}: {e}"));
                        }
                    }
                }
                Cmd::Move { to, go } => match target(&webview, &children, &to) {
                    Some(v) => {
                        let r = match &go {
                            Go::Back => v.go_back(),
                            Go::Forward => v.go_forward(),
                            Go::Reload => v.reload(),
                            Go::To(u) => v.load_url(u),
                        };
                        if let Err(e) = r {
                            crate::append_hook_log(&format!("移動できません {go:?}: {e}"));
                        }
                    }
                    None => crate::append_hook_log(&format!("移動先のページがありません: {to:?}")),
                },
                Cmd::Where { to } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = where_tx.send(Ev::Where {
                            from: to,
                            url: v.url().unwrap_or_default(),
                            can_back: v.can_go_back().unwrap_or(false),
                            can_forward: v.can_go_forward().unwrap_or(false),
                        });
                    }
                }
                Cmd::Screencast { to, on } => {
                    if on {
                        if casts.contains_key(&to) {
                            // 既に流している。二重登録はしないが、startScreencast を
                            // 打ち直して今の画面を1枚出す (新しい視聴者が入ったとき、
                            // ページが静止しているといつまでも空のままになるため)
                            if let Some(view) = target(&webview, &children, &to) {
                                cdp::kick(&cdp::webview_of(view));
                            }
                        } else if let Some(view) = target(&webview, &children, &to) {
                            let wv = cdp::webview_of(view);
                            let tx = ev_tx.clone();
                            let from = to.clone();
                            let dims = cast_dims.clone();
                            if let Some(cast) = cdp::start(&wv, move |data, w, h| {
                                dims.set((w, h));
                                let _ = tx.send(Ev::Frame {
                                    from: from.clone(),
                                    data,
                                    w: w as u32,
                                    h: h as u32,
                                });
                            }) {
                                casts.insert(to.clone(), cast);
                            } else {
                                crate::append_hook_log("画面中継を開始できません (CDP)");
                            }
                        }
                    } else if let Some(cast) = casts.remove(&to) {
                        cdp::stop(cast);
                    }
                }
                Cmd::Inject { to, input } => {
                    if let Some(view) = target(&webview, &children, &to) {
                        let wv = cdp::webview_of(view);
                        let (cw, ch) = cast_dims.get();
                        match input {
                            Input::Mouse { phase, x, y, down } => {
                                let (px, py) = (x * cw, y * ch);
                                let (kind, buttons) = match phase.as_str() {
                                    "pressed" => {
                                        mouse_down = true;
                                        ("mousePressed", 1)
                                    }
                                    "released" => {
                                        mouse_down = false;
                                        ("mouseReleased", 0)
                                    }
                                    _ => ("mouseMoved", if down || mouse_down { 1 } else { 0 }),
                                };
                                let params = serde_json::json!({
                                    "type": kind, "x": px, "y": py,
                                    "button": "left", "buttons": buttons, "clickCount": 1,
                                })
                                .to_string();
                                cdp::call(&wv, "Input.dispatchMouseEvent", &params);
                            }
                            Input::Wheel { x, y, dx, dy } => {
                                let params = serde_json::json!({
                                    "type": "mouseWheel", "x": x * cw, "y": y * ch,
                                    "deltaX": dx, "deltaY": dy,
                                })
                                .to_string();
                                cdp::call(&wv, "Input.dispatchMouseEvent", &params);
                            }
                            Input::Text { text } => {
                                let params = serde_json::json!({ "text": text }).to_string();
                                cdp::call(&wv, "Input.insertText", &params);
                            }
                            Input::Key { named } => {
                                if let Some((key, vk)) = named_vk(&named) {
                                    for kind in ["keyDown", "keyUp"] {
                                        let params = serde_json::json!({
                                            "type": kind, "key": key,
                                            "windowsVirtualKeyCode": vk,
                                            "nativeVirtualKeyCode": vk,
                                        })
                                        .to_string();
                                        cdp::call(&wv, "Input.dispatchKeyEvent", &params);
                                    }
                                }
                            }
                        }
                    }
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

    let _ = closed_tx.send(Ev::Closed);
    Ok(())
}

/// CDP (Chrome DevTools Protocol) 越しの画面中継と入力注入。
///
/// WebView2 は中身が Chromium なので、開発者ツール用のプロトコルを話せる。
/// これを使うと「変化したところだけ」をJPEGフレームで受け取れ (VNCより軽い)、
/// マウス・ホイール・文字を**本物の入力として**注入できる (合成イベントではない)。
///
/// COMのオブジェクトはスレッドに縛られるので、呼び出しは必ず窓のイベントループ
/// スレッド (run_window の中) から行う。フレーム通知も同じスレッドに届く。
mod cdp {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceivedEventArgs,
        ICoreWebView2DevToolsProtocolEventReceiver,
    };
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    };
    use windows::core::{HSTRING, PCWSTR};

    /// 中継の後始末に要るもの (これを持っている間だけ通知が届く)
    pub struct Cast {
        pub receiver: ICoreWebView2DevToolsProtocolEventReceiver,
        pub token: i64,
        pub webview: ICoreWebView2,
    }

    /// CDPのメソッドを1つ呼ぶ (結果は捨てる)。params_json は "{}" でよい
    pub fn call(webview: &ICoreWebView2, method: &str, params_json: &str) {
        let method = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let handler =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_hr, _json| Ok(())));
        unsafe {
            let _ = webview.CallDevToolsProtocolMethod(
                PCWSTR(method.as_ptr()),
                PCWSTR(params.as_ptr()),
                &handler,
            );
        }
    }

    /// wry の WebView から下の ICoreWebView2 を取り出す
    pub fn webview_of(view: &wry::WebView) -> ICoreWebView2 {
        use wry::WebViewExtWindows;
        view.webview()
    }

    /// 画面中継を始める。フレームが来るたびに `on_frame(base64_jpeg, css_w, css_h)` を呼ぶ。
    /// フレームの ack もここで自動的に返す (返さないと次が来ない)
    pub fn start<F>(webview: &ICoreWebView2, on_frame: F) -> Option<Cast>
    where
        F: FnMut(String, f64, f64) + 'static,
    {
        let cb = std::rc::Rc::new(std::cell::RefCell::new(on_frame));
        let wv = webview.clone();
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
            move |_sender, args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    unsafe {
                        if args.ParameterObjectAsJson(&mut raw).is_ok() {
                            let json = webview2_com::take_pwstr(raw);
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                                let data = v
                                    .get("data")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let meta = v.get("metadata");
                                let w = meta
                                    .and_then(|m| m.get("deviceWidth"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let h = meta
                                    .and_then(|m| m.get("deviceHeight"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let sid = v.get("sessionId").and_then(|x| x.as_i64()).unwrap_or(0);
                                // 先に ack を返してから届ける (詰まらせない)
                                call(
                                    &wv,
                                    "Page.screencastFrameAck",
                                    &format!("{{\"sessionId\":{sid}}}"),
                                );
                                if !data.is_empty() {
                                    (cb.borrow_mut())(data, w, h);
                                }
                            }
                        }
                    }
                }
                Ok(())
            },
        ));

        let name = HSTRING::from("Page.screencastFrame");
        let mut token = 0i64;
        unsafe {
            let receiver = webview
                .GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr()))
                .ok()?;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .ok()?;
            call(webview, "Page.enable", "{}");
            call(
                webview,
                "Page.startScreencast",
                "{\"format\":\"jpeg\",\"quality\":60,\"maxWidth\":1600,\"maxHeight\":1200,\"everyNthFrame\":1}",
            );
            Some(Cast {
                receiver,
                token,
                webview: webview.clone(),
            })
        }
    }

    /// 今の画面を1枚出させる (startScreencast を打ち直す)。
    /// 新しい視聴者が入ったが、ページが静止していて次の変化が来ないときに使う
    pub fn kick(webview: &ICoreWebView2) {
        call(
            webview,
            "Page.startScreencast",
            "{\"format\":\"jpeg\",\"quality\":60,\"maxWidth\":1600,\"maxHeight\":1200,\"everyNthFrame\":1}",
        );
    }

    /// 中継を止め、通知の登録も外す
    pub fn stop(cast: Cast) {
        unsafe {
            call(&cast.webview, "Page.stopScreencast", "{}");
            let _ = cast
                .receiver
                .remove_DevToolsProtocolEventReceived(cast.token);
        }
    }
}

#[cfg(test)]
mod nav_tests {
    use super::*;

    /// 綴りを省いたら補い、http/https 以外へは行かせないこと。
    ///
    /// URL欄は「どこへでも行ける口」なので、`file:` で手元のファイルを、
    /// `javascript:` で今のページを開かれると、そこから先は
    /// 自動化の目に触れる。行き先はここで絞る
    #[test]
    fn the_address_box_only_opens_web_pages() {
        assert_eq!(openable("example.com").as_deref(), Some("https://example.com"));
        assert_eq!(
            openable("  https://a.example/x?y=1  ").as_deref(),
            Some("https://a.example/x?y=1"),
            "前後の空白は落とす"
        );
        assert_eq!(
            openable("http://127.0.0.1:8080/").as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        for bad in ["", "   ", "file:///C:/secret.txt", "ftp://x/y"] {
            assert!(openable(bad).is_none(), "開けてしまう: {bad}");
        }
        // 綴りとして扱わないので、壊れた行き先になって開けずに終わる
        let js = openable("javascript:alert(1)").unwrap_or_default();
        assert!(
            js.starts_with("https://"),
            "そのままの綴りで渡している: {js}"
        );
    }

    /// ホイールの合図が、遡る量として読めること。
    /// 上へ回したら過去へ (正)、下へ回したら今へ (負)
    #[test]
    fn the_wheel_asks_to_go_back_through_the_log() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(
            read(r#"{"kind":"scroll","by":3,"row":4,"col":9}"#),
            Some(Ev::Scroll { by: 3, row: 4, col: 9 })
        ));
        assert!(matches!(
            read(r#"{"kind":"scroll","by":-3,"row":0,"col":0}"#),
            Some(Ev::Scroll { by: -3, .. })
        ));
        // 量が無ければ動かない (0 は「何もしない」であって捨てない)
        assert!(matches!(read(r#"{"kind":"scroll"}"#), Some(Ev::Scroll { by: 0, .. })));
        // 指の数として意味のない量は抑える
        assert!(matches!(
            read(r#"{"kind":"scroll","by":999999}"#),
            Some(Ev::Scroll { by: 64, .. })
        ));
    }

    /// 画面からの合図が、そのまま移動の指示になること
    #[test]
    fn the_bar_speaks_the_same_words_as_the_loop() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(
            read(r#"{"kind":"go","what":"back"}"#),
            Some(Ev::Go { go: Go::Back })
        ));
        assert!(matches!(
            read(r#"{"kind":"go","what":"reload"}"#),
            Some(Ev::Go { go: Go::Reload })
        ));
        match read(r#"{"kind":"go","what":"to","url":"example.com"}"#) {
            Some(Ev::Go { go: Go::To(u) }) => assert_eq!(u, "example.com"),
            other => panic!("行き先が読めていない: {other:?}"),
        }
        // 知らない指示は捨てる。黙って別の動きをするより何もしない方がいい
        assert!(read(r#"{"kind":"go","what":"quit"}"#).is_none());
        assert!(read(r#"{"kind":"go"}"#).is_none());
    }
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
        assert_eq!(b.find(None, &Sel::Css("#here".into()), t).unwrap(), Found::Visible);
        assert_eq!(b.find(None, &Sel::Css("#far".into()), t).unwrap(), Found::OffScreen);
        assert_eq!(b.find(None, &Sel::Css("#nope".into()), t).unwrap(), Found::NotFound);

        // XPath: CSSでは書けない探し方 (ラベルの隣のセル)
        let name = b
            .text(None, &Sel::Xpath("//td[text()='氏名']/following-sibling::td".into()), t)
            .unwrap();
        assert_eq!(name.as_deref(), Some("山田"), "XPathで隣のセルが取れない");

        // 押す
        assert_eq!(b.click(None, &Sel::Css("#go".into()), t).unwrap(), Found::Visible);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            b.text(None, &Sel::Css("#log".into()), t).unwrap().as_deref(),
            Some("pushed"),
            "押した結果がページに出ていない"
        );

        // 入れる。値を書くだけでなく input が飛ぶこと
        // (Reactなどは飛ばさないと状態が動かない)
        assert_eq!(
            b.fill(None, &Sel::Css("#q".into()), "ふつうの値", t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#q".into()), t).unwrap().as_deref(),
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
            b.fill(None, &Sel::Css("#q".into()), nasty, t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some(nasty),
            "値が一字一句そのまま入っていない"
        );

        // 改行を含む値。1行の input は改行を落とす (HTMLの仕様) ので、
        // 複数行を渡すなら textarea でなければならない。
        // 値が壊れたのではなく、入れ物が保持できないだけ
        let multi = format!("1行目\n2行目\t{nasty}");
        assert_eq!(
            b.fill(None, &Sel::Css("#multi".into()), &multi, t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#multi".into()), t).unwrap().as_deref(),
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
        let html = b.html(None, t).unwrap();
        assert!(html.contains("ここにいる"), "HTMLが取れていない");
        assert!(html.len() > 200, "HTMLが短すぎる: {}", html.len());
        println!("HTML {} 文字 / すべて通過", html.chars().count());

        drop(b);
    }


    /// 同じ窓の中にページを置けること。
    ///
    ///   cargo test child_view -- --ignored --nocapture
    ///
    /// 別窓だと、所有関係も位置の追従も、Windows Terminal の
    /// タブ切替での露出も、全部こちらの持ち物になっていた
    #[test]
    #[ignore]
    fn a_page_can_sit_inside_the_window() {
        let b = Browser::spawn(&serve(PAGE), "child probe").expect("窓が開かない");
        b.open_child("side", "https://example.com/", (400, 0, 400, 500))
            .expect("置けない");
        std::thread::sleep(std::time::Duration::from_secs(3));
        // 場所を変えられる
        b.child_bounds("side", (200, 0, 600, 500)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(600));
        // 幅0で隠れる
        b.child_bounds("side", (0, 0, 0, 0)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        b.close_child("side").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        // 外皮のほうは生きたまま
        let id = b.eval("return 1+1;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_secs(10)).unwrap(),
            "2",
            "子を置いたら外皮が動かなくなった"
        );
        println!("子ページの出し入れ: 通過");
        drop(b);
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

        b.ask(None, "ログインしてください", "できました").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let id = b.eval("return !!document.getElementById('__shikisha_bar');").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).unwrap();
        println!("帯が出ているか = {v}");
        assert_eq!(v, "true", "呼びかけの帯が出ていない");

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
        println!("閉じてもここまで来た (プロセスは生きている)");
    }
}
