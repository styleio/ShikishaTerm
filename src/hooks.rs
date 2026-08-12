//! Luaフックエンジン。DESIGN.md 8章。
//!
//! - サンドボックス: string/table/math/coroutine のみロード (io/os/ネットワーク無し)
//! - ケーパビリティ注入: Rust側が実装した shikisha.* だけが外界への窓口
//! - コルーチン実行モデル: shikisha.wait/sleep は coroutine.yield で
//!   検出ティック(200ms)に待機し、UIをブロックしない
//! - 自動送信は呼び出し元タブのチェーン深度+1を引き継ぐ (暴走対策はmain側)

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use mlua::thread::ThreadStatus;
use mlua::{Lua, LuaOptions, MultiValue, RegistryKey, StdLib, Table, Thread, Value};

/// いまの日時を、渡された書き方で返す (手元の時計)。
///
/// 綴りは `os.date` と同じにする。Lua を書く人は既にそれを知っているので、
/// 覚え直すものを増やさない。使えるのは日時に要るものだけ:
///
/// | 綴り | 中身            | 例       |
/// |------|-----------------|----------|
/// | `%Y` | 年 (4桁)        | `2026`   |
/// | `%y` | 年 (下2桁)      | `26`     |
/// | `%m` | 月              | `08`     |
/// | `%d` | 日              | `07`     |
/// | `%H` | 時 (24)         | `01`     |
/// | `%M` | 分              | `05`     |
/// | `%S` | 秒              | `09`     |
/// | `%%` | `%` そのもの    | `%`      |
///
/// 知らない綴りは、そのまま残す。黙って消すと、書いたつもりのものが
/// 消えたことに気づけない
pub fn local_stamp(fmt: &str) -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    let mut out = String::with_capacity(fmt.len() + 8);
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out.push_str(&format!("{:04}", t.wYear)),
            Some('y') => out.push_str(&format!("{:02}", t.wYear % 100)),
            Some('m') => out.push_str(&format!("{:02}", t.wMonth)),
            Some('d') => out.push_str(&format!("{:02}", t.wDay)),
            Some('H') => out.push_str(&format!("{:02}", t.wHour)),
            Some('M') => out.push_str(&format!("{:02}", t.wMinute)),
            Some('S') => out.push_str(&format!("{:02}", t.wSecond)),
            Some('%') => out.push('%'),
            // 知らない綴りは残す。消すと、書いたものが消えたと気づけない
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// mluaのエラーはSend+Syncでないためanyhowへ文字列で変換する
fn lerr(e: mlua::Error) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// ラリーの再生用スクリプトの置き場。実行したブラウザ操作Luaをここに積む。
/// 本文 (文の並び) だけを書くので、そのまま on_done.lua に貼れば再生できる
fn rally_record_path() -> std::path::PathBuf {
    crate::config::state_path("last-rally.lua")
}

/// 記録を始め直す (前回分を消してヘッダだけ置く)
fn rally_record_reset() -> std::io::Result<()> {
    std::fs::write(
        rally_record_path(),
        "-- SHIKISHA ラリー記録: 実行したブラウザ操作Lua。\n\
         -- これを on_done.lua (または on_load.lua) に貼れば、AI無しで同じ動きを再生できる。\n\n",
    )
}

/// 実行したLuaを1手ぶん追記する
fn rally_record_append(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(rally_record_path())?;
    writeln!(f, "{}", text.trim_end())
}

/// セレクタの指定を解く。"#id" (CSS) か { xpath = "..." } / { css = "..." }
fn sel_of(v: &Value) -> mlua::Result<crate::browser::Sel> {
    match v {
        Value::String(s) => Ok(crate::browser::Sel::Css(s.to_str()?.to_string())),
        Value::Table(t) => {
            if let Ok(x) = t.get::<String>("xpath") {
                Ok(crate::browser::Sel::Xpath(x))
            } else if let Ok(x) = t.get::<String>("css") {
                Ok(crate::browser::Sel::Css(x))
            } else {
                Err(mlua::Error::runtime("セレクタは \"#id\" か { xpath = ... }"))
            }
        }
        _ => Err(mlua::Error::runtime("セレクタは \"#id\" か { xpath = ... }")),
    }
}

/// browser_go の指定を解く (back / forward / reload / to(URL))
fn go_of(what: &str, url: Option<String>) -> mlua::Result<crate::browser::Go> {
    use crate::browser::Go;
    Ok(match what {
        "back" => Go::Back,
        "forward" => Go::Forward,
        "reload" => Go::Reload,
        "to" => Go::To(url.unwrap_or_default()),
        _ => return Err(mlua::Error::runtime("browser_go は back/forward/reload/to のいずれか")),
    })
}

/// 見つからなかったときに止めるか進むか (呼び出しごとに選べる)
fn missing_ok(opts: &Option<Table>) -> bool {
    opts.as_ref()
        .and_then(|t| t.get::<String>("on_missing").ok())
        .is_some_and(|s| s == "continue")
}

fn check(what: &str, state: &str, opts: &Option<Table>) -> mlua::Result<String> {
    if state == "not_found" && !missing_ok(opts) {
        return Err(mlua::Error::runtime(format!(
            "{what}: 要素が見つかりません (進めたいなら on_missing=\"continue\")"
        )));
    }
    Ok(state.to_string())
}

/// browser_fetch の opts (Luaテーブル) を JSON に直す
fn fetch_opts_json(opts: &Option<Table>) -> serde_json::Value {
    let mut o = serde_json::Map::new();
    if let Some(t) = opts {
        if let Ok(Some(m)) = t.get::<Option<String>>("method") {
            o.insert("method".into(), m.into());
        }
        if let Ok(Some(b)) = t.get::<Option<String>>("body") {
            o.insert("body".into(), b.into());
        }
        if let Ok(Some(h)) = t.get::<Option<Table>>("headers") {
            let mut hm = serde_json::Map::new();
            for pair in h.pairs::<String, String>().flatten() {
                hm.insert(pair.0, pair.1.into());
            }
            o.insert("headers".into(), serde_json::Value::Object(hm));
        }
    }
    serde_json::Value::Object(o)
}

/// AIが書いたLuaを、限定した環境で実行する (P3 サンドボックス)。
///
/// 見せるのは browser 系 (許可された1タブに限る) と log だけ。
/// file/http/os/io/load/require/debug/coroutine、記録・送信・秘密の生値、
/// そして他のブラウザには一切触れさせない。触ろうとしても、その名前が
/// 環境に無いので nil で弾かれる (`_ENV` を差し替え、グローバルへの __index も張らない)。
/// 成功なら nil、失敗ならエラー文字列を返す (司令塔がAIへ返せるように)
fn run_scoped(lua: &mlua::Lua, caps: &Caps, browser: &str, code: &str) -> mlua::Result<Value> {
    let env = build_sandbox_env(lua, caps, browser)?;
    match lua.load(code).set_name("ai-lua").set_environment(env).exec() {
        Ok(()) => Ok(Value::Nil),
        Err(e) => Ok(Value::String(lua.create_string(e.to_string())?)),
    }
}

/// サンドボックスの `_ENV` を組む。安全な標準機能と、限定した shikisha だけを入れる
fn build_sandbox_env(lua: &mlua::Lua, caps: &Caps, browser: &str) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    // 安全な標準機能だけを移す。load/require/os/io/debug/coroutine/getmetatable 等は入れない
    let g = lua.globals();
    for name in [
        "assert", "error", "ipairs", "pairs", "next", "pcall", "xpcall", "select", "tonumber",
        "tostring", "type", "string", "table", "math",
    ] {
        if let Ok(v) = g.get::<Value>(name) {
            env.set(name, v)?;
        }
    }
    let sh = lua.create_table()?;
    let allowed = browser.to_string();
    // 呼ばれたブラウザ名が、このラリーで許された1つと一致するか
    fn guard(name: &str, allowed: &str) -> mlua::Result<()> {
        if name == allowed {
            Ok(())
        } else {
            Err(mlua::Error::runtime(format!(
                "許可されていないブラウザ: {name} (このラリーは {allowed} のみ)"
            )))
        }
    }
    macro_rules! bind {
        ($n:literal, $args:ty, |$lua:ident, $c:ident, $a:ident, $p:pat_param| $body:expr) => {{
            let $c = Caps::clone(caps);
            let $a = allowed.clone();
            sh.set(
                $n,
                lua.create_function(move |$lua, $p: $args| {
                    let _ = &$lua;
                    $body
                })?,
            )?;
        }};
    }
    bind!("browser_open", (String, String), |lua_, c, al, (name, url)| {
        guard(&name, &al)?;
        c.browser_open(&name, &url)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_go", (String, String, Option<String>), |lua_, c, al, (name, what, url)| {
        guard(&name, &al)?;
        c.browser_go(&name, go_of(&what, url)?)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_find", (String, Value), |lua_, c, al, (name, sel)| {
        guard(&name, &al)?;
        c.browser_find(&name, &sel_of(&sel)?)
            .map(|s| s.to_string())
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_click", (String, Value, Option<Table>), |lua_, c, al, (name, sel, opts)| {
        guard(&name, &al)?;
        let st = c
            .browser_click(&name, &sel_of(&sel)?)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        check("browser_click", st, &opts)
    });
    bind!("browser_fill", (String, Value, String, Option<Table>), |lua_, c, al, (name, sel, value, opts)| {
        guard(&name, &al)?;
        let st = c
            .browser_fill(&name, &sel_of(&sel)?, &value)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        check("browser_fill", st, &opts)
    });
    // 秘密の値を欄に入れる。値は名前で参照し、Rustが解決して入れる。
    // AIには値が渡らない (返るのは状態だけ)。許可リストに無い鍵は secret_value が弾く
    bind!("browser_fill_secret", (String, Value, String), |lua_, c, al, (name, sel, secret_key)| {
        guard(&name, &al)?;
        let value = c
            .secret_value(&secret_key)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        c.browser_fill(&name, &sel_of(&sel)?, &value)
            .map(|s| s.to_string())
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    // ベーシック認証を仕込む (資格情報は許可リスト内の秘密から。値はAIに渡らない)
    bind!("browser_auth", (String, String), |lua_, c, al, (name, secret_key)| {
        guard(&name, &al)?;
        c.browser_auth(&name, &secret_key)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_text", (String, Value), |lua_, c, al, (name, sel)| {
        guard(&name, &al)?;
        // 読み取り結果はAIへ渡るので、既知の秘密値を伏字にする
        c.browser_text(&name, &sel_of(&sel)?)
            .map(|o| o.map(|s| c.redact(&s)))
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_html", String, |lua_, c, al, name| {
        guard(&name, &al)?;
        c.browser_html(&name)
            .map(|h| c.redact(&h))
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_fetch", (String, String, Option<Table>), |lua_, c, al, (name, url, opts)| {
        guard(&name, &al)?;
        let json = c
            .browser_fetch(&name, &url, &fetch_opts_json(&opts))
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        // 本文・ヘッダに秘密が載っていても、AIへ渡る前に伏字にする
        let json = c.redact(&json);
        let v: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        json_to_lua(lua_, &v)
    });
    bind!("browser_ask", (String, String, Option<String>), |lua_, c, al, (name, text, label)| {
        guard(&name, &al)?;
        c.browser_ask(&name, &text, label.as_deref().unwrap_or("OK"))
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_unask", String, |lua_, c, al, name| {
        guard(&name, &al)?;
        c.browser_unask(&name)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_pressed", String, |lua_, c, al, name| {
        guard(&name, &al)?;
        c.browser_pressed(&name)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    // ログだけは許す (AIが自分の手を説明できる)。ほかの副作用は無い。
    // AIが秘密値をログに書こうとしても、ここで伏字にする
    {
        let c = Caps::clone(caps);
        sh.set(
            "log",
            lua.create_function(move |_, text: String| {
                crate::append_hook_log(&format!("[ai] {}", c.redact(&text)));
                Ok(())
            })?,
        )?;
    }
    // AIが書く1行Luaでは bare 名 (browser_go(...)) で呼べるのが自然。
    // shikisha.* と同じ関数を、サンドボックス env の素のグローバルにも置く
    // (新しい能力は増えない。同じ関数の別名にすぎない)。プロンプトの表記とも一致する
    for pair in sh.pairs::<String, Value>() {
        let (k, v) = pair?;
        env.set(k, v)?;
    }
    env.set("shikisha", sh)?;
    Ok(env)
}

/// JSONの値をそのままLuaの値へ写す (browser_fetch の結果をテーブルで返すため)。
/// オブジェクトはキー付きテーブル、配列は1始まりの並び
fn json_to_lua(lua: &mlua::Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match v {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => Value::String(lua.create_string(s)?),
        serde_json::Value::Array(a) => {
            let t = lua.create_table()?;
            for (i, e) in a.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, e)?)?;
            }
            Value::Table(t)
        }
        serde_json::Value::Object(m) => {
            let t = lua.create_table()?;
            for (k, e) in m {
                t.set(k.as_str(), json_to_lua(lua, e)?)?;
            }
            Value::Table(t)
        }
    })
}

/// タブの指定方法。番号は並べ替えで変わるので、名前で指定するのが安全
#[derive(Debug, Clone)]
pub enum TabRef {
    Index(usize),
    Name(String),
}

/// 自動化から見たタブの見分け方 (ID優先、無ければタブ名)
#[derive(Debug, Clone, Default)]
pub struct TabKey {
    pub id: Option<String>,
    pub name: String,
}

impl TabKey {
    fn matches(&self, s: &str) -> bool {
        // IDが設定されていればIDで、無ければ名前で照合する
        match &self.id {
            Some(id) => id == s,
            None => self.name == s,
        }
    }
}

impl TabRef {
    /// タブ一覧から実際の番号 (1始まり) を求める。
    /// 文字列指定は ID → タブ名 の順に探す
    pub fn resolve(&self, keys: &[TabKey]) -> Option<usize> {
        match self {
            TabRef::Index(i) => (*i >= 1 && *i <= keys.len()).then_some(*i),
            TabRef::Name(s) => keys
                .iter()
                .position(|k| k.matches(s))
                .or_else(|| keys.iter().position(|k| &k.name == s))
                .map(|i| i + 1),
        }
    }
}

/// フックからRust側へ依頼される操作。main側で実行される
#[derive(Debug)]
pub enum Command {
    /// プロンプトとして他タブへ送信 (bracketed paste + Enter、チェーン深度を継承)
    SendPrompt {
        target: TabRef,
        text: String,
        origin: usize,
    },
    /// 生のキー列を送信 (on_questionの自動応答等。エンコードせずそのまま)
    SendKeys { target: TabRef, keys: String },
    /// 送らずに入力欄へ置くだけ (人が書き足して自分で送る)
    DraftPrompt {
        target: TabRef,
        text: String,
        origin: usize,
    },
    /// 表示するタブを切り替える (観戦モード: AI↔ブラウザの手番を人が目視できる)。
    /// 0 = 稼働盤 (INDEX)
    ShowTab { target: TabRef },
    /// ラリーの終了結果 (AIが目的達成を判定して出す終了コードと理由)。
    /// data/last-result.json とログとUIに出す
    SetResult {
        code: i32,
        reason: String,
        origin: usize,
    },
    /// 登録済み通知先への通知 (Phase 4-3でSlack/Telegram実装、現状はログ+表示)
    Notify { dest: String, text: String },
    /// タブの再起動 (SSH切断・CLI自己更新からの復帰)
    Restart { target: TabRef },
    Log(String),
}

/// ブラウザのフックへ渡すページの様子
#[derive(Clone)]
pub struct PageCtx {
    /// 画面の番号 (人が押す番号と同じ)
    pub index: usize,
    /// 自動化から指す呼び名
    pub id: String,
    /// 人が読む名前
    pub name: String,
    pub url: String,
    /// 参照しているものまで揃ったか。
    /// false は「load が来ないので、DOMだけの時点で来た」
    pub complete: bool,
}

/// フック発火時にLuaへ渡すタブ情報のスナップショット
#[derive(Clone)]
pub struct TabCtx {
    pub index: usize,
    pub name: String,
    pub state: String,
    pub profile: String,
    pub output: String,
    /// 自動チェーンの深度。0 = 人間が始めた会話。
    /// `if tab.chain_depth == 0 then return end` で人間の指示に反応しないフックが書ける
    pub chain_depth: u32,
    pub locked: bool,
}

enum WaitKind {
    Sleep {
        deadline: Instant,
    },
    Screen {
        tab: usize,
        re: regex::Regex,
        deadline: Instant,
    },
}

struct Pending {
    key: RegistryKey,
    hook: String,
    origin: usize,
    wait: WaitKind,
}

const PRELUDE: &str = r#"
shikisha.__vars = {}
function shikisha.get_var(k) return shikisha.__vars[k] end
function shikisha.set_var(k, v) shikisha.__vars[k] = v end
-- 状態が変わるまで待つ (state と sleep で組み立てられるのでLua側で実装)
function shikisha.wait_state(tab, want, timeout_ms)
  local left = timeout_ms or 60000
  while left > 0 do
    if shikisha.state(tab) == want then return true end
    shikisha.sleep(200)
    left = left - 200
  end
  return false
end
-- ページの状態か、人のボタンか、時間切れか。先に来た方で抜ける。
-- ボタンは常に出しておく: セレクタが外れたときに詰まないように
function shikisha.browser_wait(name, opts)
  opts = opts or {}
  local left = opts.timeout_ms or 300000
  local step = 300
  if opts.ask then
    shikisha.browser_ask(name, opts.ask, opts.label or "できました")
  end
  while left > 0 do
    if opts.selector and shikisha.browser_find(name, opts.selector) == "visible" then
      if opts.ask then shikisha.browser_unask(name) end
      return "selector"
    end
    if opts.ask and shikisha.browser_pressed(name) then
      shikisha.browser_unask(name)
      return "button"
    end
    shikisha.sleep(step)
    left = left - step
  end
  if opts.ask then shikisha.browser_unask(name) end
  return "timeout"
end
function shikisha.wait(tab, pattern, timeout_ms)
  local idx = (type(tab) == "table") and tab.index or tab
  return coroutine.yield({ op = "wait", tab = idx, pattern = pattern, timeout_ms = timeout_ms or 10000 })
end
function shikisha.sleep(ms)
  return coroutine.yield({ op = "sleep", ms = ms })
end
"#;

/// 1つのLuaスクリプト。独立した環境(_ENV)で読み込むので、
/// 複数ファイルが同じ `on_done` を定義しても衝突しない
struct Script {
    /// 表示用のパス
    path: String,
    /// このスクリプトの環境テーブル (ここからフック関数を引く)
    env: Table,
    defined: HashSet<String>,
}

/// フックの引き当て先。より具体的な方が優先される (タブ > ワークスペース > 基本)
#[derive(Default)]
struct Attach {
    base: Option<usize>,
    workspace: Option<usize>,
    tabs: std::collections::HashMap<usize, usize>,
}

/// 自動化に与える能力 (既定は空 = ファイル・通信ともに不可)
pub type Caps = std::rc::Rc<crate::caps::Capabilities>;

pub struct HookEngine {
    lua: Lua,
    commands: Rc<RefCell<Vec<Command>>>,
    current_origin: Rc<Cell<usize>>,
    /// 各タブの (見分け方, 現在の状態)。ループ中から読めるようにする
    states: Rc<RefCell<Vec<(TabKey, String)>>>,
    pending: Vec<Pending>,
    scripts: Vec<Script>,
    attach: Attach,
}

const HOOK_NAMES: [&str; 5] = ["on_start", "on_question", "on_busy", "on_done", "on_exit"];

/// ブラウザのタブで使えるフック。
///
/// セッションの状態はページには当てはまらないので、言葉を分ける。
/// 増やすのは、増やす理由が出てからでいい
pub const PAGE_HOOK_NAMES: [&str; 2] = ["on_load", "on_press"];

impl HookEngine {
    /// スクリプトを1本だけ読み込んで基本設定に紐づける (テスト・単純構成用)
    #[cfg(test)]
    pub fn from_source(source: &str) -> Result<Self> {
        let mut e = Self::new()?;
        let id = e.load_source("(inline)", source)?;
        e.attach.base = Some(id);
        Ok(e)
    }

    /// 能力を与えないエンジン (テスト・単純構成用)
    #[cfg(test)]
    pub fn new() -> Result<Self> {
        Self::with_caps(std::rc::Rc::new(crate::caps::Capabilities::disabled()))
    }

    pub fn with_caps(caps: Caps) -> Result<Self> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::COROUTINE,
            LuaOptions::default(),
        )
        .map_err(lerr)?;
        lua.set_memory_limit(64 * 1024 * 1024).map_err(lerr)?;

        let commands: Rc<RefCell<Vec<Command>>> = Rc::new(RefCell::new(Vec::new()));
        let current_origin = Rc::new(Cell::new(1usize));
        let states: Rc<RefCell<Vec<(TabKey, String)>>> = Rc::new(RefCell::new(Vec::new()));

        let shikisha = lua.create_table().map_err(lerr)?;
        {
            // 現在の状態を読む。ループの終了条件に使う
            // (フック引数の tab.state は発火時点のスナップショットなので変化しない)
            let s = Rc::clone(&states);
            shikisha
                .set(
                    "state",
                    lua.create_function(move |_, tab: Value| {
                        let r = tab_ref_of(&tab)?;
                        let states = s.borrow();
                        let keys: Vec<TabKey> = states.iter().map(|(k, _)| k.clone()).collect();
                        Ok(r.resolve(&keys)
                            .and_then(|i| states.get(i - 1).map(|(_, st)| st.clone()))
                            .unwrap_or_else(|| "EXIT".to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Rc::clone(&commands);
            let o = Rc::clone(&current_origin);
            shikisha
                .set(
                    "send_to_tab",
                    lua.create_function(move |_, (target, text): (Value, String)| {
                        c.borrow_mut().push(Command::SendPrompt {
                            target: tab_ref_of(&target)?,
                            text,
                            origin: o.get(),
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // 表示タブを切り替える。観戦モードの要 (手番ごとに見える画面を動かす)。
            // 相手はセッションでもブラウザでもよい (番号・名前・id・tab表で指せる)
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "show",
                    lua.create_function(move |_, target: Value| {
                        c.borrow_mut().push(Command::ShowTab {
                            target: tab_ref_of(&target)?,
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // ラリーの終了結果。AIが目的達成を判定して出す終了コードと理由
            let c = Rc::clone(&commands);
            let o = Rc::clone(&current_origin);
            shikisha
                .set(
                    "set_result",
                    lua.create_function(move |_, (code, reason): (i64, Option<String>)| {
                        c.borrow_mut().push(Command::SetResult {
                            code: code as i32,
                            reason: reason.unwrap_or_default(),
                            origin: o.get(),
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        // ── ブラウザ ──────────────────────────────
        // セレクタ・on_missing の解釈はモジュール関数 sel_of / check にある
        // (サンドボックス run_scoped からも同じものを使う)
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_open",
                    lua.create_function(move |_, (name, url): (String, String)| {
                        c.browser_open(&name, &url)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // 今のタブをその場で動かす: back / forward / reload / to(URL)。
            // browser_open と違い webview を作り直さないので、仕込んだ認証も残る
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_go",
                    lua.create_function(move |_, (name, what, url): (String, String, Option<String>)| {
                        let go = go_of(&what, url)?;
                        c.browser_go(&name, go)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_find",
                    lua.create_function(move |_, (name, sel): (String, Value)| {
                        c.browser_find(&name, &sel_of(&sel)?)
                            .map(|s| s.to_string())
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_click",
                    lua.create_function(
                        move |_, (name, sel, opts): (String, Value, Option<Table>)| {
                            let st = c
                                .browser_click(&name, &sel_of(&sel)?)
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            check("browser_click", st, &opts)
                        },
                    )
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_fill",
                    lua.create_function(
                        move |_, (name, sel, value, opts): (String, Value, String, Option<Table>)| {
                            let st = c
                                .browser_fill(&name, &sel_of(&sel)?, &value)
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            check("browser_fill", st, &opts)
                        },
                    )
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // 秘密の値を欄に入れる。名前で参照し、Rustが解決して入れるので
            // 値はLua/AIに渡らない。記録に残るのも鍵名だけ (貼れば再生できる)
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_fill_secret",
                    lua.create_function(move |_, (name, sel, key): (String, Value, String)| {
                        let value = c
                            .secret_value(&key)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                        c.browser_fill(&name, &sel_of(&sel)?, &value)
                            .map(|s| s.to_string())
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // ベーシック認証を仕込む。資格情報は秘密 (許可リスト内) から解決。
            // 呼んでから保護ページへ移動/再読込すると、401 に自動で答える
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_auth",
                    lua.create_function(move |_, (name, key): (String, String)| {
                        c.browser_auth(&name, &key)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_text",
                    // 司令塔の制御ループ用: 読めないとき (無応答・タイムアウト等) は
                    // 例外にせず nil を返す。1手の失敗でループごと落とさないため
                    // (審判の安全網が回り続ける)。エラーはログにだけ残す
                    lua.create_function(move |_, (name, sel): (String, Value)| {
                        match c.browser_text(&name, &sel_of(&sel)?) {
                            Ok(v) => Ok(v),
                            Err(e) => {
                                crate::append_hook_log(&format!("browser_text 読めず: {e}"));
                                Ok(None)
                            }
                        }
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_html",
                    // browser_text と同じく、読めないときは例外にせず nil を返す
                    lua.create_function(move |lua, name: String| match c.browser_html(&name) {
                        Ok(h) => Ok(Value::String(lua.create_string(&h)?)),
                        Err(e) => {
                            crate::append_hook_log(&format!("browser_html 読めず: {e}"));
                            Ok(Value::Nil)
                        }
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // ページの中から通信する。返りは {status,ok,url,headers,body} のテーブル。
            // opts は {method=..,headers={..},body=..}（省略可）
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_fetch",
                    lua.create_function(
                        move |lua, (name, url, opts): (String, String, Option<Table>)| {
                            let mut o = serde_json::Map::new();
                            if let Some(t) = &opts {
                                if let Ok(Some(m)) = t.get::<Option<String>>("method") {
                                    o.insert("method".into(), m.into());
                                }
                                if let Ok(Some(b)) = t.get::<Option<String>>("body") {
                                    o.insert("body".into(), b.into());
                                }
                                if let Ok(Some(h)) = t.get::<Option<Table>>("headers") {
                                    let mut hm = serde_json::Map::new();
                                    for pair in h.pairs::<String, String>().flatten() {
                                        hm.insert(pair.0, pair.1.into());
                                    }
                                    o.insert("headers".into(), serde_json::Value::Object(hm));
                                }
                            }
                            let json = c
                                .browser_fetch(&name, &url, &serde_json::Value::Object(o))
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            let v: serde_json::Value = serde_json::from_str(&json)
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            json_to_lua(lua, &v)
                        },
                    )
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_ask",
                    lua.create_function(move |_, (name, text, label): (String, String, Option<String>)| {
                        c.browser_ask(&name, &text, label.as_deref().unwrap_or("OK"))
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_unask",
                    lua.create_function(move |_, name: String| {
                        c.browser_unask(&name)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_nav",
                    lua.create_function(move |_, (name, opts): (String, Option<mlua::Table>)| {
                        // 何も渡さなければ全部出す。1つずつ選ぶのは、
                        // 選びたい人だけがやればいい
                        let spec = match opts {
                            None => crate::config::NavSpec::all(),
                            Some(t) => {
                                let get = |k: &str| t.get::<Option<bool>>(k).ok().flatten().unwrap_or(false);
                                crate::config::NavSpec {
                                    back: get("back"),
                                    forward: get("forward"),
                                    reload: get("reload"),
                                    url: get("url"),
                                }
                            }
                        };
                        c.browser_nav(&name, spec)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_unnav",
                    lua.create_function(move |_, name: String| {
                        c.browser_unnav(&name)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_pressed",
                    lua.create_function(move |_, name: String| {
                        c.browser_pressed(&name)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_close",
                    lua.create_function(move |_, name: String| {
                        c.browser_close(&name)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Rc::clone(&commands);
            let o = Rc::clone(&current_origin);
            shikisha
                .set(
                    "draft_to_tab",
                    lua.create_function(move |_, (target, text): (Value, String)| {
                        c.borrow_mut().push(Command::DraftPrompt {
                            target: tab_ref_of(&target)?,
                            text,
                            origin: o.get(),
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "send",
                    lua.create_function(move |_, (tab, keys): (Value, String)| {
                        c.borrow_mut().push(Command::SendKeys {
                            target: tab_ref_of(&tab)?,
                            keys,
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "notify",
                    lua.create_function(move |_, (dest, text): (String, String)| {
                        c.borrow_mut().push(Command::Notify { dest, text });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "restart",
                    lua.create_function(move |_, tab: Value| {
                        c.borrow_mut().push(Command::Restart {
                            target: tab_ref_of(&tab)?,
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // 日時。ファイル名に使うのはごく普通の用なので、ここだけ出す。
            // os を丸ごと渡すと、プロセスを起こす道具も一緒に渡ることになる。
            // 書き方は呼ぶ側が決める。既定は並べ替えで時間順になる形
            shikisha
                .set(
                    "now",
                    lua.create_function(|_, fmt: Option<String>| {
                        Ok(local_stamp(fmt.as_deref().unwrap_or("%Y%m%d%H%M%S")))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // 経過時間の計測用。UNIXエポックからのミリ秒。審判の time 停止条件で使う
            // (now は表示・ファイル名向けで %s を持たないため、数値の時計を別に出す)
            shikisha
                .set(
                    "epoch_ms",
                    lua.create_function(|_, ()| {
                        Ok(std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "log",
                    lua.create_function(move |_, text: String| {
                        c.borrow_mut().push(Command::Log(text));
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        // ファイル・HTTPは「登録済みの窓口」経由でのみ許可される (caps.rs)。
        // 生のio/osは一切与えず、Rust側の関数だけを注入する
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "write_file",
                    lua.create_function(move |_, (name, rel, data): (String, String, String)| {
                        c.write(&name, &rel, &data)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "read_file",
                    lua.create_function(move |_, (name, rel): (String, String)| {
                        c.read(&name, &rel)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "http",
                    lua.create_function(move |_, (name, body): (String, String)| {
                        c.http(&name, &body)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        // 玄人向け: 生パス・生URL (allow_dirs / allow_hosts が空なら常に失敗する)
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "write_path",
                    lua.create_function(move |_, (p, data): (String, String)| {
                        c.write_raw(&p, &data)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "read_path",
                    lua.create_function(move |_, p: String| {
                        c.read_raw(&p)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "http_raw",
                    lua.create_function(move |_, (url, body): (String, String)| {
                        c.http_raw(&url, &body)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // ラリーの記録。司令塔が各手番の実行Luaを積み、貼れば再生できる形にする。
            // 信頼側 (司令塔) 専用。AI製Luaのサンドボックスには公開しない
            shikisha
                .set(
                    "record_reset",
                    lua.create_function(|_, ()| {
                        rally_record_reset().map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            shikisha
                .set(
                    "record",
                    lua.create_function(|_, text: String| {
                        rally_record_append(&text).map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // ラリーの受け渡し置き場 (exchange)。AIが書いた生Luaを「画面を読む」のではなく
            // ファイルからバイト正確に受け取るための橋。司令塔専用 (サンドボックスには出さない)。
            // exchange_new: run 用フォルダを作りパスを返す (前方スラッシュ正規化)
            shikisha
                .set(
                    "exchange_new",
                    lua.create_function(|_, ()| {
                        crate::exchange::new_run()
                            .map(|p| p.display().to_string().replace('\\', "/"))
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // exchange_take: ファイルを読んで削除し中身を返す (無ければ nil)。一時ファイルの消費
            shikisha
                .set(
                    "exchange_take",
                    lua.create_function(|_, path: String| {
                        Ok(crate::exchange::take(std::path::Path::new(&path)))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // exchange_append: 記録 (record.lua) に1手ぶん追記する
            shikisha
                .set(
                    "exchange_append",
                    lua.create_function(|_, (path, text): (String, String)| {
                        crate::exchange::append(std::path::Path::new(&path), &text)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // lint: Luaの構文チェックだけ行う (実行はしない)。壊れていればエラー文字列、健全なら nil。
            // 実際の権限封じは run_scoped の環境が実行時に担保する
            shikisha
                .set(
                    "lint",
                    lua.create_function(|lua, code: String| match lua.load(&code).into_function() {
                        Ok(_) => Ok(Value::Nil),
                        Err(e) => Ok(Value::String(lua.create_string(e.to_string())?)),
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // AIが書いたLuaを、限定した環境で実行する (browser系だけ・許可された1タブだけ)。
            // 司令塔が受け取った危険入力 (Webの内容から生成されたLua) を、
            // file/http/秘密の生値/他タブに触れさせずに走らせるための境界
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "run_scoped",
                    lua.create_function(move |lua, (browser, code): (String, String)| {
                        run_scoped(lua, &c, &browser, &code)
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        lua.globals().set("shikisha", shikisha).map_err(lerr)?;
        lua.load(PRELUDE).exec().map_err(lerr)?;

        Ok(Self {
            lua,
            commands,
            current_origin,
            states,
            pending: Vec::new(),
            scripts: Vec::new(),
            attach: Attach::default(),
        })
    }

    /// 検出ティックごとに全タブの (見分け方, 状態) を反映する
    pub fn set_states(&self, states: Vec<(TabKey, String)>) {
        *self.states.borrow_mut() = states;
    }

    /// そのタブで待機中のループを破棄する (終了・再起動時)
    pub fn cancel_tab(&mut self, tab: usize) {
        let dropped: Vec<Pending> = {
            let (keep, drop): (Vec<_>, Vec<_>) =
                std::mem::take(&mut self.pending).into_iter().partition(|p| p.origin != tab);
            self.pending = keep;
            drop
        };
        for p in dropped {
            let _ = self.lua.remove_registry_value(p.key);
        }
    }

    /// 待機中のループを全て破棄する (緊急停止)
    pub fn cancel_all(&mut self) {
        for p in std::mem::take(&mut self.pending) {
            let _ = self.lua.remove_registry_value(p.key);
        }
    }

    /// 自動化を読み込む。同じパスは再利用する。
    /// ディレクトリなら `on_done.lua` 等のイベント別ファイル方式、
    /// `.lua` ファイルなら従来の関数定義方式として扱う
    pub fn load_path(&mut self, path: &std::path::Path) -> Result<usize> {
        let key = path.display().to_string();
        if let Some(i) = self.scripts.iter().position(|s| s.path == key) {
            return Ok(i);
        }
        if path.is_dir() {
            self.load_dir(path)
        } else {
            let source = std::fs::read_to_string(path)
                .with_context(|| format!("自動化スクリプトを読めません: {key}"))?;
            self.load_source(&key, &source)
        }
    }

    /// イベント別ファイル方式。各ファイルの中身は「処理の本体」なので、
    /// Rust側で関数に包んでからフックとして登録する
    fn load_dir(&mut self, dir: &std::path::Path) -> Result<usize> {
        let key = dir.display().to_string();
        // 共通の下請け関数を先に読み込む (同一ディレクトリ内で名前空間を共有)
        let shared = dir.join("_shared.lua");
        let mut source = String::new();
        if shared.is_file() {
            source.push_str(
                &std::fs::read_to_string(&shared)
                    .with_context(|| format!("読めません: {}", shared.display()))?,
            );
            source.push('\n');
        }
        let mut found = false;
        for hook in HOOK_NAMES {
            let f = dir.join(format!("{hook}.lua"));
            if !f.is_file() {
                continue;
            }
            let body = std::fs::read_to_string(&f)
                .with_context(|| format!("読めません: {}", f.display()))?;
            // on_question は画面テキストを第2引数で受け取る
            source.push_str(&format!("function {hook}(tab, screen)\n{body}\nend\n"));
            found = true;
        }
        // ブラウザのフックは、受け取るものが tab ではなく page
        for hook in PAGE_HOOK_NAMES {
            let f = dir.join(format!("{hook}.lua"));
            if !f.is_file() {
                continue;
            }
            let body = std::fs::read_to_string(&f)
                .with_context(|| format!("読めません: {}", f.display()))?;
            source.push_str(&format!("function {hook}(page)\n{body}\nend\n"));
            found = true;
        }
        if !found && source.is_empty() {
            anyhow::bail!(
                "{key} にイベントファイル (on_done.lua 等) がありません"
            );
        }
        self.load_source(&key, &source)
    }

    /// スクリプトを独立した環境で読み込む。
    /// 環境の __index はグローバル (string/math/shikisha 等) を指すので
    /// 標準機能とAPIは使えるが、フック関数は各スクリプトに閉じる
    fn load_source(&mut self, path: &str, source: &str) -> Result<usize> {
        let env = self.lua.create_table().map_err(lerr)?;
        let mt = self.lua.create_table().map_err(lerr)?;
        mt.set("__index", self.lua.globals()).map_err(lerr)?;
        env.set_metatable(Some(mt)).map_err(lerr)?;
        self.lua
            .load(source)
            .set_environment(env.clone())
            .exec()
            .map_err(|e| anyhow::anyhow!("{path}: Luaスクリプトの実行に失敗: {e}"))?;

        let mut defined = HashSet::new();
        // セッションのフックと、ブラウザのフックの両方を見る
        for name in HOOK_NAMES.iter().chain(PAGE_HOOK_NAMES.iter()) {
            if env.get::<mlua::Function>(*name).is_ok() {
                defined.insert(name.to_string());
            }
        }
        self.scripts.push(Script {
            path: path.to_string(),
            env,
            defined,
        });
        Ok(self.scripts.len() - 1)
    }

    /// ブラウザ操作モードの内蔵司令塔を、指定ブラウザ向けに読み込む (チャット型)。
    ///
    /// ユーザーはLuaを書かない。ゴールは設定ではなく **入力欄に打つ**。
    /// 打った文(chain 0)を新しいゴール/訂正として拾い、AIがブラウザ操作を
    /// 1手ずつ in.lua に書く→実行→画面を返す、を繰り返す。AIが操作を書かず
    /// 報告したら一区切りとして次の入力を待つ。暴走は安全網で必ず止める。
    /// BR (操作対象ブラウザの id) は先頭に注入する
    pub fn load_browser_agent(&mut self, browser: &str, stops_lua: &str) -> Result<usize> {
        let key = format!("<browser-agent:{browser}>{stops_lua}");
        if let Some(i) = self.scripts.iter().position(|s| s.path == key) {
            return Ok(i);
        }
        // 内蔵オーケストレータ (関数定義方式)。BR と STOPS は下で注入
        const SRC: &str = r##"
local MAX_ROUNDS, MAX_SEC, MAX_TOK = 40, 900, 400000

-- 審判: 設定した停止条件(STOPS)を上から評価し、成立した条件を返す(無ければnil)。
-- screen/css/xpath はタブ(既定BR)を見る。console は今の手番のAI発言(screen_out)を見る
local function judge(screen_out)
  for _, s in ipairs(STOPS or {}) do
    local hit = false
    local target = s.tab or BR
    if s.when == "screen" then
      local ok, body = pcall(shikisha.browser_text, target, "body")
      hit = ok and body ~= nil and s.pattern ~= nil and body:find(s.pattern, 1, true) ~= nil
    elseif s.when == "css" or s.when == "xpath" then
      local sel = (s.when == "xpath") and { xpath = s.sel } or s.sel
      local ok, st = pcall(shikisha.browser_find, target, sel)
      hit = ok and st == "visible"
    elseif s.when == "console" then
      hit = s.pattern ~= nil and (screen_out or ""):find(s.pattern, 1, true) ~= nil
    elseif s.when == "rounds" then
      hit = (shikisha.get_var("rally_round") or 0) >= (s.max or 0)
    elseif s.when == "time" then
      local t0 = shikisha.get_var("rally_t0") or shikisha.epoch_ms()
      hit = (shikisha.epoch_ms() - t0) >= (s.sec or 0) * 1000
    elseif s.when == "tokens" then
      hit = (shikisha.get_var("rally_tok") or 0) >= (s.max or 0)
    end
    if hit then return s end
  end
  return nil
end

local function protocol(run)
  local infile = run .. "/in.lua"
  local humanfile = run .. "/human.txt"
  return table.concat({
    "あなたはブラウザ \"" .. BR .. "\" を操作するエージェントです。",
    "毎手番、次の1手のLuaを **このファイルに上書き保存** して渡す(画面には貼らない):",
    "  " .. infile,
    "  使える関数(対象は \"" .. BR .. "\"):",
    "    browser_go(\"" .. BR .. "\", \"to\"|\"reload\"|\"back\"|\"forward\", url?)",
    "    browser_click(\"" .. BR .. "\", sel)   browser_fill(\"" .. BR .. "\", sel, value)",
    "    browser_fill_secret(\"" .. BR .. "\", sel, 秘密名)   browser_auth(\"" .. BR .. "\", 秘密名)",
    "    browser_text(\"" .. BR .. "\", sel)   browser_find(\"" .. BR .. "\", sel)",
    "    sel は \"#id\" か {xpath=\"...\"}。1ファイル=1手。書いたら手番を終える。",
    "  ログイン/CAPTCHA等で人手が要るときは、依頼を " .. humanfile .. " に(1〜2文で端的に)。",
    "  ゴールを達成した/これ以上操作が不要と思ったら、**in.luaを書かず**に結果を短く報告して待機。",
  }, "\n")
end

local function reset_budget()
  shikisha.set_var("rally_round", 0)
  shikisha.set_var("rally_t0", shikisha.epoch_ms())
  shikisha.set_var("rally_tok", 0)
end

function on_start(tab)
  local run = shikisha.exchange_new()
  shikisha.set_var("rally_run", run)
  shikisha.set_var("rally_record", run .. "/record.lua")
  reset_budget()
  shikisha.send_to_tab(tab.index, table.concat({
    protocol(run),
    "",
    "準備OK。やってほしいことを入力欄に書いてください(例: ログインして投稿画面へ)。",
  }, "\n"))
end

function on_done(tab)
  local ai = tab.index
  local run = shikisha.get_var("rally_run")
  if not run then return end
  local infile = run .. "/in.lua"

  -- 入力欄に人間が打った(chain 0) = 新しいゴール/訂正。予算(安全網)を切り直す
  if tab.chain_depth == 0 then
    reset_budget()
  end
  shikisha.set_var("rally_tok", (shikisha.get_var("rally_tok") or 0) + #(tab.output or ""))

  -- 人手依頼ファイル
  local human = shikisha.exchange_take(run .. "/human.txt")
  if human and #human > 0 then
    shikisha.show(BR)
    shikisha.browser_wait(BR, { ask = human, label = "できたら押す" })
    shikisha.show(ai)
    shikisha.send_to_tab(ai, "対応しました。続けてください(次の1手を " .. infile .. " に)。")
    return
  end

  -- 操作ファイル → LINT → 実行 → 記録
  local code = shikisha.exchange_take(infile)
  if code and #code > 0 then
    local lint = shikisha.lint(code)
    if lint then
      shikisha.send_to_tab(ai, "Luaが構文エラーです:\n" .. lint .. "\n直して " .. infile .. " に。")
      return
    end
    shikisha.show(BR)
    local err = shikisha.run_scoped(BR, code)
    if err then
      shikisha.show(ai)
      shikisha.send_to_tab(ai, "実行でエラー:\n" .. err .. "\n別の手を " .. infile .. " に。")
      return
    end
    shikisha.exchange_append(shikisha.get_var("rally_record"), code)
    local n = (shikisha.get_var("rally_round") or 0) + 1
    shikisha.set_var("rally_round", n)
    for _ = 1, 12 do
      shikisha.sleep(150)
      local t = shikisha.browser_text(BR, "body")
      if t and #(t:gsub("%s", "")) > 0 then break end
    end
    -- 審判(設定した停止条件)。成立したら終了コードを出して一区切り(待機に戻る)
    local v = judge(tab.output)
    if v then
      shikisha.show(v.outcome == "success" and ai or BR)
      shikisha.set_result(v.code or 0, v.reason or v.outcome)
      shikisha.send_to_tab(ai, "判定: " .. (v.reason or v.outcome)
        .. " (code=" .. (v.code or 0) .. ")。次の指示があればどうぞ。")
      return
    end
    -- 安全網(暴走保険)。ゴールごとに予算はリセットされる
    local t0 = shikisha.get_var("rally_t0") or shikisha.epoch_ms()
    if n >= MAX_ROUNDS or (shikisha.epoch_ms() - t0) >= MAX_SEC * 1000
        or (shikisha.get_var("rally_tok") or 0) >= MAX_TOK then
      shikisha.show(ai)
      shikisha.send_to_tab(ai, "安全網(上限)に達したので一旦止めます。指示を見直すか、新しく指示してください。")
      return
    end
    -- 画面を返して次の1手を促す
    shikisha.show(ai)
    local text = shikisha.browser_text(BR, "body") or ""
    if #text > 3000 then text = text:sub(1, 3000) .. "…(略)" end
    shikisha.send_to_tab(ai, table.concat({
      "実行しました。今の画面テキスト:", "----", text, "----",
      "次の1手を " .. infile .. " に。達成/不要と思ったら in.lua を書かず結果を報告して待機を。",
    }, "\n"))
    return
  end

  -- 操作なし: 人間がゴールを打った直後(chain 0)なら一度だけ促す。
  -- そうでなければ(AIが報告/待機した)、何も送らず人間の次の入力を待つ
  if tab.chain_depth == 0 then
    shikisha.send_to_tab(ai,
      "了解。ブラウザ操作は1手ずつ " .. infile .. " に書いてください。まず最初の1手を。")
  end
end
"##;
        let src = format!("local BR = {browser:?}\nlocal STOPS = {stops_lua}\n{SRC}");
        self.load_source(&key, &src)
    }

    pub fn set_base(&mut self, id: usize) {
        self.attach.base = Some(id);
    }

    pub fn set_workspace(&mut self, id: usize) {
        self.attach.workspace = Some(id);
    }

    /// タブ番号 (1始まり) にスクリプトを紐づける
    pub fn set_tab(&mut self, tab_index: usize, id: usize) {
        self.attach.tabs.insert(tab_index, id);
    }

    /// そのタブのそのフックを担当するスクリプトを解決する
    /// (タブ > ワークスペース > 基本。両方は実行しない)
    fn resolve(&self, hook: &str, tab_index: usize) -> Option<usize> {
        [
            self.attach.tabs.get(&tab_index).copied(),
            self.attach.workspace,
            self.attach.base,
        ]
        .into_iter()
        .flatten()
        .find(|&id| self.scripts[id].defined.contains(hook))
    }

    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    /// フックを発火する。extra は on_question の画面テキスト等
    pub fn fire(&mut self, hook: &str, ctx: &TabCtx, extra: Option<&str>) {
        let Some(id) = self.resolve(hook, ctx.index) else {
            return;
        };
        self.current_origin.set(ctx.index);
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function = self.scripts[id].env.get(hook)?;
            let thread = self.lua.create_thread(func)?;
            let tbl = self.make_tab_table(ctx)?;
            let args = match extra {
                Some(s) => MultiValue::from_vec(vec![
                    Value::Table(tbl),
                    Value::String(self.lua.create_string(s)?),
                ]),
                None => MultiValue::from_vec(vec![Value::Table(tbl)]),
            };
            self.resume_thread(thread, hook, ctx.index, args);
            Ok(())
        })();
        if let Err(e) = result {
            self.push_log(format!("Luaエラー({hook}): {e}"));
        }
    }

    /// ブラウザのフックを呼ぶ。
    ///
    /// 渡すのは page で、tab ではない。ページには状態も出力も無い。
    /// 無いものを埋めて似せると、書く人が別のものと取り違える
    /// そのページに、そのフックの引き当て先があるか。
    ///
    /// 押しても受ける先が無い帯は、壊れたボタンにしか見えない。
    /// 出す前に、あるいは押された時点で、そう言えるようにしておく
    pub fn has_page_hook(&self, hook: &str, index: usize) -> bool {
        self.resolve(hook, index).is_some()
    }

    pub fn fire_page(&mut self, hook: &str, page: &PageCtx) {
        let Some(id) = self.resolve(hook, page.index) else {
            return;
        };
        self.current_origin.set(page.index);
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function = self.scripts[id].env.get(hook)?;
            let thread = self.lua.create_thread(func)?;
            let tbl = self.lua.create_table()?;
            tbl.set("index", page.index)?;
            tbl.set("id", page.id.clone())?;
            tbl.set("name", page.name.clone())?;
            tbl.set("url", page.url.clone())?;
            tbl.set("complete", page.complete)?;
            let args = MultiValue::from_vec(vec![Value::Table(tbl)]);
            self.resume_thread(thread, hook, page.index, args);
            Ok(())
        })();
        if let Err(e) = result {
            self.push_log(format!("Luaエラー({hook}): {e}"));
        }
    }

    /// 検出ティック毎に呼ぶ。wait/sleep中のコルーチンの条件を評価して再開する
    pub fn tick_pending(&mut self, screens: &dyn Fn(usize) -> Option<String>) {
        let now = Instant::now();
        let pending = std::mem::take(&mut self.pending);
        for p in pending {
            let ready: Option<bool> = match &p.wait {
                WaitKind::Sleep { deadline } => (now >= *deadline).then_some(true),
                WaitKind::Screen { tab, re, deadline } => match screens(*tab) {
                    Some(text) if re.is_match(&text) => Some(true),
                    _ if now >= *deadline => Some(false),
                    _ => None,
                },
            };
            match ready {
                Some(result) => {
                    self.current_origin.set(p.origin);
                    match self.lua.registry_value::<Thread>(&p.key) {
                        Ok(thread) => {
                            let args = MultiValue::from_vec(vec![Value::Boolean(result)]);
                            self.resume_thread(thread, &p.hook, p.origin, args);
                        }
                        Err(e) => self.push_log(format!("Lua再開エラー: {e}")),
                    }
                    let _ = self.lua.remove_registry_value(p.key);
                }
                None => self.pending.push(p),
            }
        }
    }

    /// フックが積んだ操作依頼を取り出す (main側で実行)
    pub fn drain_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut *self.commands.borrow_mut())
    }

    fn resume_thread(&mut self, thread: Thread, hook: &str, origin: usize, args: MultiValue) {
        match thread.resume::<MultiValue>(args) {
            Ok(vals) => {
                if thread.status() == ThreadStatus::Resumable {
                    // yield: wait/sleep の待機要求
                    match self.parse_yield(&vals) {
                        Ok(wait) => match self.lua.create_registry_value(thread) {
                            Ok(key) => self.pending.push(Pending {
                                key,
                                hook: hook.to_string(),
                                origin,
                                wait,
                            }),
                            Err(e) => self.push_log(format!("Lua登録エラー: {e}")),
                        },
                        Err(e) => self.push_log(format!("Lua yield不正({hook}): {e}")),
                    }
                } else {
                    self.on_complete(hook, origin, vals);
                }
            }
            Err(e) => self.push_log(format!("Luaエラー({hook}): {e}")),
        }
    }

    /// フック完了時の返値処理: on_question が文字列を返したら自動応答キーとして送信
    fn on_complete(&mut self, hook: &str, origin: usize, vals: MultiValue) {
        if hook == "on_question" {
            if let Some(Value::String(s)) = vals.into_iter().next() {
                if let Ok(keys) = s.to_str() {
                    self.commands.borrow_mut().push(Command::SendKeys {
                        target: TabRef::Index(origin),
                        keys: keys.to_string(),
                    });
                }
            }
        }
    }

    fn parse_yield(&self, vals: &MultiValue) -> Result<WaitKind> {
        let Some(Value::Table(t)) = vals.iter().next() else {
            anyhow::bail!("yieldはshikisha.wait/sleep経由のみ対応");
        };
        let op: String = t.get("op").map_err(lerr)?;
        match op.as_str() {
            "sleep" => {
                let ms: u64 = t.get("ms").map_err(lerr)?;
                Ok(WaitKind::Sleep {
                    deadline: Instant::now() + Duration::from_millis(ms),
                })
            }
            "wait" => {
                let tab: usize = t.get("tab").map_err(lerr)?;
                let pattern: String = t.get("pattern").map_err(lerr)?;
                let timeout_ms: u64 = t.get("timeout_ms").map_err(lerr)?;
                Ok(WaitKind::Screen {
                    tab,
                    re: regex::Regex::new(&pattern)
                        .with_context(|| format!("waitの正規表現が不正: {pattern}"))?,
                    deadline: Instant::now() + Duration::from_millis(timeout_ms),
                })
            }
            other => anyhow::bail!("不明なyield op: {other}"),
        }
    }

    fn make_tab_table(&self, ctx: &TabCtx) -> mlua::Result<Table> {
        let t = self.lua.create_table()?;
        t.set("index", ctx.index)?;
        t.set("name", ctx.name.as_str())?;
        t.set("state", ctx.state.as_str())?;
        t.set("profile", ctx.profile.as_str())?;
        t.set("output", ctx.output.as_str())?;
        t.set("chain_depth", ctx.chain_depth)?;
        t.set("locked", ctx.locked)?;
        Ok(t)
    }

    fn push_log(&self, msg: String) {
        self.commands.borrow_mut().push(Command::Log(msg));
    }
}

/// タブ指定を受け取る。番号・タブ名・tabテーブルのいずれでもよい
fn tab_ref_of(v: &Value) -> mlua::Result<TabRef> {
    match v {
        Value::Integer(n) => Ok(TabRef::Index(*n as usize)),
        Value::Number(n) => Ok(TabRef::Index(*n as usize)),
        Value::String(s) => Ok(TabRef::Name(s.to_str()?.to_string())),
        Value::Table(t) => Ok(TabRef::Index(t.get("index")?)),
        _ => Err(mlua::Error::runtime(
            "タブは番号かタブ名で指定してください",
        )),
    }
}

#[cfg(test)]
mod stamp_tests {
    /// 日時が、並べ替えで時間順になる形であること。
    ///
    /// ファイル名に使うので、桁が揺れると並びが崩れる。
    /// Lua には os を渡していないので、ここが唯一の出どころ
    /// 渡した書き方のとおりに返ること
    #[test]
    fn the_shape_is_the_caller_s_to_choose() {
        let ymd = super::local_stamp("%Y-%m-%d");
        assert_eq!(ymd.len(), 10, "{ymd}");
        assert_eq!(&ymd[4..5], "-");
        assert_eq!(&ymd[7..8], "-");
        assert_eq!(super::local_stamp("%y").len(), 2);
        // 綴りでないものは、そのまま出る
        assert_eq!(super::local_stamp("報告_%%.html"), "報告_%.html");
        // 知らない綴りは残す。黙って消えると、消えたことに気づけない
        assert_eq!(super::local_stamp("%Q"), "%Q");
        assert_eq!(super::local_stamp(""), "");
    }

    #[test]
    fn the_stamp_sorts_by_time() {
        let s = super::local_stamp("%Y%m%d%H%M%S");
        assert_eq!(s.len(), 14, "桁が違う: {s}");
        assert!(s.chars().all(|c| c.is_ascii_digit()), "数字以外がある: {s}");
        let year: u32 = s[..4].parse().expect("年が読めない");
        assert!((2020..2200).contains(&year), "年がおかしい: {s}");
        let month: u32 = s[4..6].parse().unwrap();
        let day: u32 = s[6..8].parse().unwrap();
        assert!((1..=12).contains(&month) && (1..=31).contains(&day), "{s}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// data/last-rally.lua を触るテストは同じファイルを共有するので直列化する
    static RALLY_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn ctx(index: usize, output: &str) -> TabCtx {
        TabCtx {
            index,
            name: format!("tab{index}"),
            state: "DONE".into(),
            profile: "test".into(),
            output: output.into(),
            chain_depth: 0,
            locked: false,
        }
    }

    #[test]
    fn on_done_pipeline_branching() {
        let mut e = HookEngine::from_source(
            r#"
            function on_done(tab)
              if tab.output:match("NG") then
                shikisha.send_to_tab(1, "fix: " .. tab.output)
              else
                shikisha.send_to_tab(3, tab.output)
              end
            end
            "#,
        )
        .unwrap();
        e.fire("on_done", &ctx(2, "NG: bad code"), None);
        let cmds = e.drain_commands();
        assert!(matches!(
            &cmds[0],
            Command::SendPrompt { origin: 2, text, .. } if text.starts_with("fix:")
        ));
    }

    #[test]
    fn on_question_returns_keys() {
        let mut e = HookEngine::from_source(
            r#"
            function on_question(tab, screen)
              if screen:match("削除") then return nil end
              return "1\r"
            end
            "#,
        )
        .unwrap();
        e.fire("on_question", &ctx(1, ""), Some("Do you want to proceed? 1. Yes"));
        let cmds = e.drain_commands();
        assert!(matches!(
            &cmds[0],
            Command::SendKeys { keys, .. } if keys == "1\r"
        ));

        e.fire("on_question", &ctx(1, ""), Some("ファイルを削除しますか?"));
        assert!(e.drain_commands().is_empty(), "危険系はnil=人間へ");
    }

    #[test]
    fn wait_resumes_when_pattern_appears() {
        let mut e = HookEngine::from_source(
            r#"
            function on_start(tab)
              if shikisha.wait(tab, "\\$ $", 5000) then
                shikisha.send(tab, "cd /work\r")
              end
            end
            "#,
        )
        .unwrap();
        e.fire("on_start", &ctx(1, ""), None);
        assert!(e.drain_commands().is_empty(), "まだ待機中");

        // 条件不成立 → 保留のまま
        e.tick_pending(&|_| Some("loading...".to_string()));
        assert!(e.drain_commands().is_empty());

        // プロンプトが出た → 再開してsend実行
        e.tick_pending(&|_| Some("user@host:~$ ".to_string()));
        let cmds = e.drain_commands();
        assert!(matches!(
            &cmds[0],
            Command::SendKeys { keys, .. } if keys == "cd /work\r"
        ));
    }

    #[test]
    fn vars_are_shared_between_fires() {
        let mut e = HookEngine::from_source(
            r#"
            function on_done(tab)
              local n = (shikisha.get_var("n") or 0) + 1
              shikisha.set_var("n", n)
              shikisha.log("round " .. n)
            end
            "#,
        )
        .unwrap();
        e.fire("on_done", &ctx(1, ""), None);
        e.fire("on_done", &ctx(1, ""), None);
        let cmds = e.drain_commands();
        assert!(matches!(&cmds[1], Command::Log(m) if m == "round 2"));
    }

    #[test]
    fn tabs_can_be_addressed_by_name_so_reordering_is_safe() {
        let mut e = HookEngine::from_source(
            r#"
            function on_done(tab)
              shikisha.send_to_tab("検査", "レビューして: " .. tab.output)
            end
            "#,
        )
        .unwrap();
        e.fire("on_done", &ctx(1, "code"), None);
        let cmds = e.drain_commands();
        let Command::SendPrompt { target, .. } = &cmds[0] else {
            panic!("送信コマンドが積まれるはず");
        };
        // 並べ替えても、名前が同じなら正しいタブに解決される
        let key = |n: &str| TabKey { id: None, name: n.to_string() };
        assert_eq!(target.resolve(&[key("実装"), key("検査")]), Some(2));
        assert_eq!(target.resolve(&[key("検査"), key("実装")]), Some(1));
        // 存在しない名前は解決できない (誤爆させない)
        assert_eq!(target.resolve(&[key("別名")]), None);
    }

    #[test]
    fn show_switches_the_visible_tab_for_spectating() {
        // 観戦モード: 手番ごとに見える画面を動かす。相手はブラウザでもINDEXでも指せる
        let mut e = HookEngine::from_source(
            r#"
            function on_done(tab)
              shikisha.show("ブラウザ")   -- ブラウザタブへ切替
              shikisha.show(0)            -- 稼働盤(INDEX)へ
            end
            "#,
        )
        .unwrap();
        e.fire("on_done", &ctx(1, ""), None);
        let cmds = e.drain_commands();
        assert_eq!(cmds.len(), 2, "show 2回ぶん積まれる");
        let Command::ShowTab { target } = &cmds[0] else {
            panic!("ShowTabが積まれるはず");
        };
        // 名前は画面の番号 (セッションもブラウザも並ぶ) に解決される
        let key = |n: &str| TabKey { id: None, name: n.to_string() };
        assert_eq!(target.resolve(&[key("AI"), key("ブラウザ")]), Some(2));
        // 0 は稼働盤 (INDEX)。resolveでは拾わず、main側が特別扱いする
        assert!(
            matches!(&cmds[1], Command::ShowTab { target: TabRef::Index(0) }),
            "show(0) は INDEX"
        );
    }

    #[test]
    fn fetch_result_json_becomes_a_lua_table() {
        // browser_fetch はJSON文字列で返る結果を、Luaのテーブルに写して渡す。
        // status/ok/body/headers/配列が素直に引けること
        let lua = mlua::Lua::new();
        let v: serde_json::Value = serde_json::from_str(
            r#"{"ok":true,"status":200,"headers":{"content-type":"text/html"},"body":"hi","nums":[1,2,3]}"#,
        )
        .unwrap();
        let val = super::json_to_lua(&lua, &v).unwrap();
        lua.globals().set("r", val).unwrap();
        let out: String = lua
            .load(
                r#"return tostring(r.ok)..","..r.status..","..r.headers["content-type"]..","..r.body..","..r.nums[3]"#,
            )
            .eval()
            .unwrap();
        assert_eq!(out, "true,200,text/html,hi,3");
    }

    #[test]
    fn rally_recording_is_appendable_replayable_lua() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // 司令塔が実行Luaを積むと、貼れば再生できる本文が残ること
        let mut e = HookEngine::from_source(
            r##"
            function on_start(tab)
              shikisha.record_reset()
              shikisha.record('shikisha.browser_click("br", "#login")')
              shikisha.record('shikisha.browser_fill("br", "#body", "hi")')
            end
            "##,
        )
        .unwrap();
        e.fire("on_start", &ctx(1, ""), None);
        let path = super::rally_record_path();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("-- SHIKISHA"), "ヘッダが要る");
        assert!(content.contains(r##"shikisha.browser_click("br", "#login")"##));
        assert!(content.contains(r##"shikisha.browser_fill("br", "#body", "hi")"##));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn browser_agent_mode_is_built_in_and_needs_no_lua() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // 内蔵のブラウザ操作モードが読め、on_start が「操作プロトコル」を送ること。
        // ゴールは設定に持たず入力欄から与える方式なので、プロンプトに入力欄の案内が要る
        let mut e = HookEngine::new().unwrap();
        // 設定した停止条件(審判)が Lua に注入されても読めること
        let stops = crate::config::stops_to_lua(&[crate::config::StopCond {
            when: "screen".into(),
            tab: Some("br".into()),
            pattern: Some("公開に進む".into()),
            outcome: "success".into(),
            code: 0,
            reason: Some("エディタ表示".into()),
            ..Default::default()
        }]);
        let id = e.load_browser_agent("br", &stops).expect("内蔵司令塔が読めない");
        e.set_tab(1, id);
        e.fire("on_start", &ctx(1, ""), None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c,
                Command::SendPrompt { text, .. }
                    if text.contains("in.lua")
                        && text.contains("browser_go")
                        && text.contains("入力欄"))),
            "on_start がブラウザ操作プロトコル(入力欄でゴール)を送っていない: {cmds:?}"
        );
    }

    #[test]
    fn set_result_carries_the_exit_code_and_reason() {
        // AIが判定した終了コードと理由が、そのタブ発で積まれること
        let mut e = HookEngine::from_source(
            r#"
            function on_done(tab)
              shikisha.set_result(0, "投稿できた")
            end
            "#,
        )
        .unwrap();
        e.fire("on_done", &ctx(3, ""), None);
        let cmds = e.drain_commands();
        match &cmds[0] {
            Command::SetResult { code, reason, origin } => {
                assert_eq!(*code, 0);
                assert_eq!(reason, "投稿できた");
                assert_eq!(*origin, 3, "発したタブの番号を持つ");
            }
            other => panic!("SetResultが積まれるはず: {other:?}"),
        }
    }

    #[test]
    fn sandbox_blocks_dangerous_access_and_other_tabs() {
        // run_scoped は AI製Luaを browser系＋許可された1タブに限定する。
        // os/io/load/require、shikisha.write_file/http、他タブへは触れさせない
        let mut e = HookEngine::from_source(
            r##"
            function on_done(t)
              shikisha.log("os=" .. tostring(shikisha.run_scoped("br", "return os.time()")))
              shikisha.log("write=" .. tostring(shikisha.run_scoped("br", "shikisha.write_file('a','b','c')")))
              shikisha.log("load=" .. tostring(shikisha.run_scoped("br", "load('return 1')")))
              shikisha.log("wrong=" .. tostring(shikisha.run_scoped("br", "shikisha.browser_click('evil', '#x')")))
              shikisha.log("hasclick=" .. tostring(shikisha.run_scoped("br", "assert(type(shikisha.browser_click)=='function')")))
            end
            "##,
        )
        .unwrap();
        e.fire("on_done", &ctx(1, ""), None);
        let logs: Vec<String> = e
            .drain_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Log(m) => Some(m),
                _ => None,
            })
            .collect();
        let find = |k: &str| logs.iter().find(|l| l.starts_with(k)).cloned().unwrap_or_default();
        assert!(find("os=").contains("os"), "os が露出している: {:?}", find("os="));
        assert!(find("write=").contains("write_file"), "write_file が使えてしまう: {:?}", find("write="));
        assert!(find("load=").contains("load"), "load が使えてしまう: {:?}", find("load="));
        assert!(
            find("wrong=").contains("許可されていないブラウザ"),
            "他タブを操作できてしまう: {:?}",
            find("wrong=")
        );
        assert_eq!(find("hasclick="), "hasclick=nil", "browser_click は使えるはず");
    }

    #[test]
    fn rally_example_orchestrator_parses_and_runs() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // 雛形 (docs/rally-example) が構文的に読め、開始と審判の要が動くこと
        let dir = std::path::Path::new("docs/rally-example");
        let mut e = HookEngine::new().unwrap();
        let id = e.load_path(dir).expect("雛形が読めない (構文エラー?)");
        e.set_base(id);

        // on_start: ファイル受け渡しのプロトコルをAIへ送る (画面ではなく in.lua に書かせる)
        e.fire("on_start", &ctx(1, ""), None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c,
                Command::SendPrompt { text, .. }
                    if text.contains("in.lua") && text.contains("browser_go"))),
            "on_start がファイル受け渡しのプロトコルを送っていない: {cmds:?}"
        );

        // on_done: 審判の安全網が終了コードを出す (ブラウザ無しでも決定的に効く)。
        // 巨大な出力で概算コスト(tokens)を一気に超えさせ、code=125 で停止することを見る
        let huge = "x".repeat(300_001);
        let mut c = ctx(1, &huge);
        c.chain_depth = 1;
        e.fire("on_done", &c, None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c, Command::SetResult { code: 125, .. })),
            "審判(tokens上限)が終了コードを出していない: {cmds:?}"
        );

        // 人間が始めた会話 (chain_depth=0) には反応しない
        e.fire("on_done", &ctx(1, ""), None);
        assert!(e.drain_commands().is_empty(), "人間の入力に自動反応してはいけない");
    }

    #[test]
    fn explicit_id_survives_renaming_the_tab() {
        let r = TabRef::Name("reviewer".into());
        let with_id = |id: &str, name: &str| TabKey {
            id: Some(id.to_string()),
            name: name.to_string(),
        };
        let plain = |name: &str| TabKey {
            id: None,
            name: name.to_string(),
        };
        // IDを付けておけば、タブ名を変えても指し続けられる
        assert_eq!(r.resolve(&[plain("実装"), with_id("reviewer", "検査")]), Some(2));
        assert_eq!(
            r.resolve(&[plain("実装"), with_id("reviewer", "レビュー担当")]),
            Some(2),
            "タブ名を変えても壊れない"
        );
        // 同名タブがあってもIDで区別できる
        let dup = [with_id("a", "claude"), with_id("b", "claude")];
        assert_eq!(TabRef::Name("b".into()).resolve(&dup), Some(2));
    }

    #[test]
    fn loop_can_read_live_state_and_exit() {
        // on_tick の代わりに「開始時 + ループ + sleep」で定期処理が書けること
        let mut e = HookEngine::from_source(
            r#"
            function on_busy(tab)
              while shikisha.state(tab) == "BUSY" do
                shikisha.log("working")
                shikisha.sleep(1000)
              end
              shikisha.log("done")
            end
            "#,
        )
        .unwrap();
        e.set_states(vec![(TabKey { id: None, name: "tab1".into() }, "BUSY".into())]);
        e.fire("on_busy", &ctx(1, ""), None);
        // 状態がBUSYの間はループが続く
        std::thread::sleep(std::time::Duration::from_millis(1100));
        e.tick_pending(&|_| None);
        // 状態が変わればループを抜ける
        e.set_states(vec![(TabKey { id: None, name: "tab1".into() }, "DONE".into())]);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        e.tick_pending(&|_| None);
        let logs: Vec<String> = e
            .drain_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Log(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(logs, vec!["working", "working", "done"]);
    }

    #[test]
    fn pending_loops_are_dropped_when_tab_ends() {
        let mut e = HookEngine::from_source(
            r#"
            function on_start(tab)
              shikisha.sleep(1000)
              shikisha.log("これは実行されないはず")
            end
            "#,
        )
        .unwrap();
        e.fire("on_start", &ctx(1, ""), None);
        e.cancel_tab(1);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        e.tick_pending(&|_| None);
        assert!(e.drain_commands().is_empty(), "破棄後は再開されない");
    }

    #[test]
    fn event_files_in_directory_are_loaded_as_bodies() {
        let dir = std::env::temp_dir().join("shikisha-auto-dir");
        std::fs::create_dir_all(&dir).unwrap();
        // 中身は「処理の本体」だけ。function...end は書かない
        std::fs::write(dir.join("_shared.lua"), "function greet(n) return 'hi ' .. n end").unwrap();
        std::fs::write(dir.join("on_done.lua"), "shikisha.log(greet(tab.name))").unwrap();
        std::fs::write(dir.join("on_question.lua"), "if screen:match('削除') then return nil end\nreturn '1\\r'").unwrap();

        let mut e = HookEngine::new().unwrap();
        let id = e.load_path(&dir).unwrap();
        e.set_base(id);

        e.fire("on_done", &ctx(1, ""), None);
        e.fire("on_question", &ctx(1, ""), Some("Do you want to proceed?"));
        e.fire("on_question", &ctx(1, ""), Some("ファイルを削除しますか"));
        let cmds = e.drain_commands();
        assert!(matches!(&cmds[0], Command::Log(m) if m == "hi tab1"), "共通関数が使える");
        assert!(matches!(&cmds[1], Command::SendKeys { keys, .. } if keys == "1\r"));
        assert_eq!(cmds.len(), 2, "削除確認は人間へ回るので送信されない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tab_script_wins_over_workspace_and_base() {
        let mut e = HookEngine::new().unwrap();
        let base = e
            .load_source("base", r#"function on_done(t) shikisha.log("base") end
                                    function on_exit(t) shikisha.log("base-exit") end"#)
            .unwrap();
        let ws = e
            .load_source("ws", r#"function on_done(t) shikisha.log("ws") end"#)
            .unwrap();
        let tab = e
            .load_source("tab", r#"function on_done(t) shikisha.log("tab") end"#)
            .unwrap();
        e.set_base(base);
        e.set_workspace(ws);
        e.set_tab(2, tab);

        // タブ2はタブ用が勝つ
        e.fire("on_done", &ctx(2, ""), None);
        // タブ1はタブ用が無いのでワークスペース用
        e.fire("on_done", &ctx(1, ""), None);
        // タブ用にon_exitが無ければ基本へフォールバック
        e.fire("on_exit", &ctx(2, ""), None);

        let logs: Vec<String> = e
            .drain_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Log(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(logs, vec!["tab", "ws", "base-exit"]);
    }

    #[test]
    fn scripts_share_vars_but_not_hook_names() {
        let mut e = HookEngine::new().unwrap();
        let a = e
            .load_source(
                "a",
                r#"function on_done(t) shikisha.set_var("n", (shikisha.get_var("n") or 0) + 1) end"#,
            )
            .unwrap();
        let b = e
            .load_source(
                "b",
                r#"function on_done(t) shikisha.log("n=" .. tostring(shikisha.get_var("n"))) end"#,
            )
            .unwrap();
        e.set_tab(1, a);
        e.set_tab(2, b);
        e.fire("on_done", &ctx(1, ""), None);
        e.fire("on_done", &ctx(2, ""), None);
        let logs: Vec<String> = e
            .drain_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Log(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(logs, vec!["n=1"], "別ファイルでも共有変数は共有される");
    }


    /// 時計を持たないまま、時刻入りのファイル名を受け渡せること。
    ///
    /// サンドボックスに os は無い (意図的に外してある) ので、Lua 側で
    /// 日時を作ることはできない。時計を持っているのはシェルなので、
    /// 名前はシェルに作らせ、その出力から読み取る。
    ///
    /// 読み取った名前は set_var で他のスクリプトからも使える
    #[test]
    fn a_name_made_by_the_shell_can_be_carried_into_the_prompt() {
        let mut e = HookEngine::new().unwrap();
        let fetch = e
            .load_source(
                "fetch",
                r#"
function on_done(t)
  local path = t.output:match("SAVED (%S+%.html)")
  if not path then shikisha.log("保存先が読み取れません") return end
  shikisha.set_var("lp_path", path)
  shikisha.draft_to_tab("ai", path .. " を読んでください。\n\n")
end"#,
            )
            .unwrap();
        // 別ファイルからでも同じ名前を参照できる
        let other = e
            .load_source(
                "other",
                r#"function on_done(t) shikisha.log("覚えている: " .. tostring(shikisha.get_var("lp_path"))) end"#,
            )
            .unwrap();
        e.set_tab(1, fetch);
        e.set_tab(2, other);

        // シェルが実際に吐く形 (打ち込んだ行は切り出しに含まれない)
        let shell_output = "\r\nSAVED tmp/LP20260806154212.html\r\nD:\\Test>";
        e.fire("on_done", &ctx(1, shell_output), None);
        e.fire("on_done", &ctx(2, ""), None);

        let cmds = e.drain_commands();
        let drafted: Vec<&String> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::DraftPrompt { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(drafted.len(), 1, "下書きが1つだけ出るはず: {cmds:?}");
        assert!(
            drafted[0].starts_with("tmp/LP20260806154212.html を読んでください。"),
            "名前が渡っていない: {:?}",
            drafted[0]
        );
        assert!(
            drafted[0].ends_with("\n\n"),
            "人が書き足すための空行が無い: {:?}",
            drafted[0]
        );

        let logs: Vec<&String> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Log(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(
            logs,
            vec!["覚えている: tmp/LP20260806154212.html"],
            "別ファイルへ名前が渡っていない"
        );
    }


    /// Lua から実際のページを触れること。
    ///
    ///   cargo test lua_drives_a_real_page -- --ignored --nocapture
    ///
    /// 途中の層だけを試しても「繋がっている」ことは分からないので、
    /// Lua の文字列から本物のページまで通す
    #[test]
    #[ignore]
    fn lua_drives_a_real_page() {
        const PAGE: &str = r#"<!doctype html><meta charset=utf-8><body>
<input id=q value="">
<button id=go onclick="document.getElementById('out').textContent='押された:'+document.getElementById('q').value">送信</button>
<div id=out></div>
<table><tr><td>氏名</td><td id=name>山田</td></tr></table>"#;

        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let _ = req.respond(
                    tiny_http::Response::from_string(PAGE).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });

        let mut e = HookEngine::new().unwrap();
        let src = format!(
            r##"
function on_done(t)
  shikisha.browser_open("br", "http://127.0.0.1:{port}/")

  -- 見つかる / 画面外でない / 無い を区別できること
  shikisha.log("find=" .. shikisha.browser_find("br", "#q"))
  shikisha.log("none=" .. shikisha.browser_find("br", "#nope", nil))

  -- XPath: CSSでは書けない探し方
  shikisha.log("xpath=" .. tostring(
    shikisha.browser_text("br", {{ xpath = "//td[text()='氏名']/following-sibling::td" }})))

  -- AIの答えを欄に入れて送る。引用符を含む値でも壊れない
  shikisha.browser_fill("br", "#q", [[それは"良い"案です']])
  shikisha.browser_click("br", "#go")
  shikisha.log("out=" .. tostring(shikisha.browser_text("br", "#out")))

  -- 見つからないときの方針を呼び出しごとに選べること
  local ok, err = pcall(function() shikisha.browser_click("br", "#nope") end)
  shikisha.log("raise=" .. tostring(ok))
  shikisha.log("continue=" .. shikisha.browser_click("br", "#nope", {{ on_missing = "continue" }}))

  shikisha.log("html=" .. tostring(#shikisha.browser_html("br") > 100))
  shikisha.browser_close("br")
end"##
        );
        let a = e.load_source("a", &src).unwrap();
        e.set_tab(1, a);
        e.fire("on_done", &ctx(1, ""), None);

        let logs: Vec<String> = e
            .drain_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Log(m) => Some(m),
                _ => None,
            })
            .collect();
        for l in &logs {
            println!("[lua] {l}");
        }
        assert!(logs.contains(&"find=visible".to_string()), "{logs:?}");
        assert!(logs.contains(&"none=not_found".to_string()), "{logs:?}");
        assert!(logs.contains(&"xpath=山田".to_string()), "XPathが効いていない: {logs:?}");
        assert!(
            logs.contains(&"out=押された:それは\"良い\"案です'".to_string()),
            "値が崩れているか、押せていない: {logs:?}"
        );
        assert!(logs.contains(&"raise=false".to_string()), "既定で止まっていない: {logs:?}");
        assert!(
            logs.contains(&"continue=not_found".to_string()),
            "on_missing=continue で進めていない: {logs:?}"
        );
        assert!(logs.contains(&"html=true".to_string()), "{logs:?}");
    }

    #[test]
    fn sandbox_has_no_io_os() {
        let e = HookEngine::from_source("x = 1");
        assert!(e.is_ok());
        let e = e.unwrap();
        let io_val: Value = e.lua.globals().get("io").unwrap();
        let os_val: Value = e.lua.globals().get("os").unwrap();
        assert!(matches!(io_val, Value::Nil), "ioは無効のはず");
        assert!(matches!(os_val, Value::Nil), "osは無効のはず");
    }
}
