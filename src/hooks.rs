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

/// フックからRust側へ依頼される操作。main側で実行される
#[derive(Debug)]
pub enum Command {
    /// プロンプトとして他タブへ送信 (bracketed paste + Enter、チェーン深度を継承)
    SendPrompt {
        target: usize,
        text: String,
        origin: usize,
    },
    /// 生のキー列を送信 (on_questionの自動応答等。エンコードせずそのまま)
    SendKeys { target: usize, keys: String },
    /// 登録済み通知先への通知 (Phase 4-3でSlack/Telegram実装、現状はログ+表示)
    Notify { dest: String, text: String },
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
function shikisha.wait(tab, pattern, timeout_ms)
  local idx = (type(tab) == "table") and tab.index or tab
  return coroutine.yield({ op = "wait", tab = idx, pattern = pattern, timeout_ms = timeout_ms or 10000 })
end
function shikisha.sleep(ms)
  return coroutine.yield({ op = "sleep", ms = ms })
end
"#;

pub struct HookEngine {
    lua: Lua,
    commands: Rc<RefCell<Vec<Command>>>,
    current_origin: Rc<Cell<usize>>,
    pending: Vec<Pending>,
    defined: HashSet<String>,
}

const HOOK_NAMES: [&str; 6] = [
    "on_start",
    "on_question",
    "on_busy",
    "on_done",
    "on_exit",
    "on_tick",
];

impl HookEngine {
    pub fn load(script_path: &std::path::Path) -> Result<Self> {
        let source = std::fs::read_to_string(script_path)
            .with_context(|| format!("Luaスクリプトを読めません: {}", script_path.display()))?;
        Self::from_source(&source)
    }

    pub fn from_source(source: &str) -> Result<Self> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::COROUTINE,
            LuaOptions::default(),
        )
        .map_err(lerr)?;
        lua.set_memory_limit(64 * 1024 * 1024).map_err(lerr)?;

        let commands: Rc<RefCell<Vec<Command>>> = Rc::new(RefCell::new(Vec::new()));
        let current_origin = Rc::new(Cell::new(1usize));

        let shikisha = lua.create_table().map_err(lerr)?;
        {
            let c = Rc::clone(&commands);
            let o = Rc::clone(&current_origin);
            shikisha
                .set(
                    "send_to_tab",
                    lua.create_function(move |_, (target, text): (usize, String)| {
                        c.borrow_mut().push(Command::SendPrompt {
                            target,
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
                        let target = tab_index_of(&tab)?;
                        c.borrow_mut().push(Command::SendKeys { target, keys });
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
                    "log",
                    lua.create_function(move |_, text: String| {
                        c.borrow_mut().push(Command::Log(text));
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        lua.globals().set("shikisha", shikisha).map_err(lerr)?;
        lua.load(PRELUDE).exec().map_err(lerr)?;
        lua.load(source)
            .exec()
            .map_err(|e| anyhow::anyhow!("Luaスクリプトの実行に失敗: {e}"))?;

        let mut defined = HashSet::new();
        for name in HOOK_NAMES {
            if lua.globals().get::<mlua::Function>(name).is_ok() {
                defined.insert(name.to_string());
            }
        }

        Ok(Self {
            lua,
            commands,
            current_origin,
            pending: Vec::new(),
            defined,
        })
    }

    pub fn has(&self, hook: &str) -> bool {
        self.defined.contains(hook)
    }

    /// フックを発火する。extra は on_question の画面テキスト等
    pub fn fire(&mut self, hook: &str, ctx: &TabCtx, extra: Option<&str>) {
        if !self.has(hook) {
            return;
        }
        self.current_origin.set(ctx.index);
        let result = (|| -> mlua::Result<()> {
            let func: mlua::Function = self.lua.globals().get(hook)?;
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
                        target: origin,
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
        Ok(t)
    }

    fn push_log(&self, msg: String) {
        self.commands.borrow_mut().push(Command::Log(msg));
    }
}

fn tab_index_of(v: &Value) -> mlua::Result<usize> {
    match v {
        Value::Integer(n) => Ok(*n as usize),
        Value::Table(t) => t.get("index"),
        _ => Err(mlua::Error::runtime("tabはインデックスかtabテーブルで指定")),
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
            Command::SendPrompt { target: 1, origin: 2, text } if text.starts_with("fix:")
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
            Command::SendKeys { target: 1, keys } if keys == "1\r"
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
            Command::SendKeys { target: 1, keys } if keys == "cd /work\r"
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
