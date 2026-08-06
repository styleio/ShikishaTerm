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

/// mluaのエラーはSend+Syncでないためanyhowへ文字列で変換する
fn lerr(e: mlua::Error) -> anyhow::Error {
    anyhow::anyhow!("{e}")
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
    DraftPrompt { target: TabRef, text: String },
    /// 登録済み通知先への通知 (Phase 4-3でSlack/Telegram実装、現状はログ+表示)
    Notify { dest: String, text: String },
    /// タブの再起動 (SSH切断・CLI自己更新からの復帰)
    Restart { target: TabRef },
    Log(String),
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
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "draft_to_tab",
                    lua.create_function(move |_, (target, text): (Value, String)| {
                        c.borrow_mut().push(Command::DraftPrompt {
                            target: tab_ref_of(&target)?,
                            text,
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
        for name in HOOK_NAMES {
            if env.get::<mlua::Function>(name).is_ok() {
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
mod tests {
    use super::*;

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
