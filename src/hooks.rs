//! Lua hook engine. See DESIGN.md chapter 8.
//!
//! - Sandbox: only string/table/math/coroutine are loaded (no io/os/network)
//! - Capability injection: only the shikisha.* functions implemented on the
//!   Rust side are windows to the outside world
//! - Coroutine execution model: shikisha.wait/sleep use coroutine.yield to
//!   wait for the next detection tick (200ms) without blocking the UI
//! - Auto-sent prompts inherit the calling tab's chain depth + 1 (runaway
//!   protection lives on the main side)

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use mlua::thread::ThreadStatus;
use mlua::{Lua, LuaOptions, MultiValue, RegistryKey, StdLib, Table, Thread, Value};

/// Returns the current date/time formatted per the given directives (local clock).
///
/// The directives match `os.date`, since anyone writing Lua already knows
/// them — no need to make them learn a new set. Only the directives needed
/// for dates/times are supported:
///
/// | Directive | Meaning              | Example  |
/// |-----------|----------------------|----------|
/// | `%Y` | Year (4 digits)           | `2026`   |
/// | `%y` | Year (last 2 digits)      | `26`     |
/// | `%m` | Month                     | `08`     |
/// | `%d` | Day                       | `07`     |
/// | `%H` | Hour (24h)                | `01`     |
/// | `%M` | Minute                    | `05`     |
/// | `%S` | Second                    | `09`     |
/// | `%%` | Literal `%`               | `%`      |
///
/// Unknown directives are left as-is. Silently dropping them would make it
/// impossible to notice that what was written has disappeared.
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
            // Unknown directives are kept. Dropping them would hide the fact that what was written disappeared
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// mlua errors aren't Send+Sync, so convert them to anyhow via a string
fn lerr(e: mlua::Error) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// Location of the rally replay script. Executed browser-operation Lua is
/// appended here. Only the statement body is written, so it can be pasted
/// straight into on_done.lua to replay
fn rally_record_path() -> std::path::PathBuf {
    crate::config::state_path("last-rally.lua")
}

/// Restart recording (clears the previous run and writes only the header)
fn rally_record_reset() -> std::io::Result<()> {
    std::fs::write(
        rally_record_path(),
        crate::i18n::t("transcript.record.header"),
    )
}

/// Append one executed Lua move to the record
fn rally_record_append(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(rally_record_path())?;
    writeln!(f, "{}", text.trim_end())
}

/// Resolve a selector spec: "#id" (CSS) or { xpath = "..." } / { css = "..." }
fn sel_of(v: &Value) -> mlua::Result<crate::browser::Sel> {
    match v {
        Value::String(s) => Ok(crate::browser::Sel::Css(s.to_str()?.to_string())),
        // A bare number is a digest ref — the friendliest spelling for a
        // small model: browser_click(BR, 12)
        Value::Integer(n) => u32::try_from(*n)
            .map(crate::browser::Sel::Ref)
            .map_err(|_| mlua::Error::runtime(crate::i18n::t("err.hooks.selector"))),
        Value::Table(t) => {
            if let Ok(n) = t.get::<u32>("ref") {
                Ok(crate::browser::Sel::Ref(n))
            } else if let Ok(x) = t.get::<String>("xpath") {
                Ok(crate::browser::Sel::Xpath(x))
            } else if let Ok(x) = t.get::<String>("css") {
                Ok(crate::browser::Sel::Css(x))
            } else {
                Err(mlua::Error::runtime(crate::i18n::t("err.hooks.selector")))
            }
        }
        _ => Err(mlua::Error::runtime(crate::i18n::t("err.hooks.selector"))),
    }
}

/// Resolve a browser_go spec (back / forward / reload / to(URL))
fn go_of(what: &str, url: Option<String>) -> mlua::Result<crate::browser::Go> {
    use crate::browser::Go;
    Ok(match what {
        "back" => Go::Back,
        "forward" => Go::Forward,
        "reload" => Go::Reload,
        "to" => Go::To(url.unwrap_or_default()),
        _ => return Err(mlua::Error::runtime(crate::i18n::t("err.hooks.browser_go"))),
    })
}

/// Whether to stop or continue when not found (selectable per call)
fn missing_ok(opts: &Option<Table>) -> bool {
    opts.as_ref()
        .and_then(|t| t.get::<String>("on_missing").ok())
        .is_some_and(|s| s == "continue")
}

fn check(what: &str, state: &str, opts: &Option<Table>) -> mlua::Result<String> {
    if state == "not_found" && !missing_ok(opts) {
        return Err(mlua::Error::runtime(crate::i18n::tp(
            "err.hooks.not_found",
            &[("what", what)],
        )));
    }
    Ok(state.to_string())
}

/// A Lua string literal for `s`, safe for any content (quotes, newlines, \)
fn lua_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The durable Lua spelling of a selector, for the replay journal.
/// css/xpath pass through as they were written; a `{ref=N}` is replaced by
/// the anchor derived from the element it actually touched. None = a ref
/// with nothing durable to anchor to (the journal notes it instead of lying)
fn sel_replay(sel: &crate::browser::Sel, anchor: &Option<(String, String)>) -> Option<String> {
    use crate::browser::Sel;
    match sel {
        Sel::Css(s) => Some(lua_str(s)),
        Sel::Xpath(x) => Some(format!("{{xpath={}}}", lua_str(x))),
        Sel::Ref(_) => anchor.as_ref().map(|(kind, v)| {
            if kind == "css" {
                lua_str(v)
            } else {
                format!("{{xpath={}}}", lua_str(v))
            }
        }),
    }
}

/// Convert browser_fetch's opts (a Lua table) into JSON
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

/// Run Lua written by the AI in a restricted environment (P3 sandbox).
///
/// Only the browser functions (limited to a single allowed tab) and log are
/// exposed. file/http/os/io/load/require/debug/coroutine, recording,
/// sending, raw secret values, and every other browser are completely off
/// limits. Even an attempt to touch them fails with nil, since those names
/// don't exist in the environment (`_ENV` is replaced, with no __index
/// fallback to globals). Returns nil on success, or an error string on
/// failure (so the orchestrator can relay it back to the AI)
/// Syntax-check a Lua snippet without running it — the same compile-only check
/// `shikisha.lint` exposes to automations, callable off the engine (the settings
/// server lints a quick action's Lua before letting it be saved). Returns the
/// compiler's message, or None if it parses. Catches broken syntax only, not
/// calls to names that don't exist at run time (Lua resolves those lazily).
pub fn lint_lua(code: &str) -> Option<String> {
    let lua = mlua::Lua::new();
    // Name the chunk "action" so a syntax error reads cleanly, instead of citing
    // this Rust file:line (mlua's default names the chunk after the caller).
    match lua.load(code).set_name("action").into_function() {
        Ok(_) => None,
        // A bare expression is accepted at run time (run_scoped compiles it
        // REPL-style), so it must pass lint too — the two disagreeing would
        // reject code that actually runs
        Err(e) => match lua.load(format!("return {code}")).set_name("action").into_function() {
            Ok(_) => None,
            Err(_) => Some(e.to_string()),
        },
    }
}

/// Render one Lua value for the AI's eyes (the "what came back" trace).
/// Tables are walked a few levels deep; beyond that, `{…}` says "there was
/// more" instead of pretending there wasn't
fn stringify_lua(v: &Value, depth: usize, out: &mut String) {
    match v {
        Value::Nil => out.push_str("nil"),
        Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Integer(i) => out.push_str(&i.to_string()),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&s.to_string_lossy()),
        Value::Table(t) => {
            if depth == 0 {
                out.push_str("{…}");
                return;
            }
            out.push('{');
            let mut first = true;
            let mut count = 0;
            for pair in t.clone().pairs::<Value, Value>() {
                let Ok((k, val)) = pair else { continue };
                if count >= 40 {
                    out.push_str(", …");
                    break;
                }
                if !first {
                    out.push_str(", ");
                }
                first = false;
                count += 1;
                match &k {
                    // The array part reads best without its indexes
                    Value::Integer(_) => {}
                    other => {
                        stringify_lua(other, 1, out);
                        out.push('=');
                    }
                }
                stringify_lua(&val, depth - 1, out);
            }
            out.push('}');
        }
        other => {
            out.push('<');
            out.push_str(other.type_name());
            out.push('>');
        }
    }
}

/// The environment a whole script runs in: an empty table that falls through
/// to the globals for reading, so two scripts defining `on_done` don't collide
/// and neither can overwrite the other's world by assigning a global.
///
/// Shared by the two places that run full-powered Lua — loading a script file
/// and `shikisha.lua(code)`. One builder on purpose: two would drift, and the
/// day they drift is the day one of them is handing out something the other
/// deliberately withholds.
fn full_env(lua: &mlua::Lua) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    let mt = lua.create_table()?;
    mt.set("__index", lua.globals())?;
    env.set_metatable(Some(mt))?;
    Ok(env)
}

/// Compile a chunk REPL-style: a bare expression is worth its value.
///
/// `browser_text(BR, "body")` on its own line should answer, not vanish, so
/// `return` is prepended first and the plain statement form is the fallback
fn compile_repl(
    lua: &mlua::Lua,
    env: Table,
    name: &str,
    code: &str,
) -> mlua::Result<mlua::Function> {
    lua.load(format!("return {code}"))
        .set_name(name)
        .set_environment(env.clone())
        .into_function()
        .or_else(|_| {
            lua.load(code)
                .set_name(name)
                .set_environment(env)
                .into_function()
        })
}

/// The name of every primitive that exists, read off the `shikisha` table.
///
/// This table *is* the registry. Registering a primitive for Lua is what makes
/// it callable from outside, and renaming one renames it for both — there is no
/// second list, no mapping table, nothing to forget to update. Keys that don't
/// hold a function (`__vars`) are not commands and are left out.
fn primitive_names(lua: &mlua::Lua) -> mlua::Result<Vec<String>> {
    let sh: Table = lua.globals().get("shikisha")?;
    let mut names = Vec::new();
    for pair in sh.pairs::<Value, Value>() {
        if let Ok((Value::String(k), Value::Function(_))) = pair {
            names.push(k.to_string_lossy().to_string());
        }
    }
    Ok(names)
}

/// Lua in, JSON out. A table numbered 1..n comes back as an array and anything
/// else as an object; a value with no JSON counterpart (a function, userdata)
/// comes back as null rather than as a lie about what it was
fn lua_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => J::from(*i),
        Value::Number(n) => serde_json::Number::from_f64(*n).map_or(J::Null, J::Number),
        Value::String(s) => J::String(s.to_string_lossy().to_string()),
        Value::Table(t) => {
            let len = t.raw_len();
            let pairs: Vec<(Value, Value)> =
                t.clone().pairs::<Value, Value>().filter_map(|p| p.ok()).collect();
            if len > 0 && pairs.len() == len {
                J::Array((1..=len).map(|i| lua_to_json(&t.get(i).unwrap_or(Value::Nil))).collect())
            } else {
                let mut o = serde_json::Map::new();
                for (k, val) in pairs {
                    let key = match &k {
                        Value::String(s) => s.to_string_lossy().to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => continue,
                    };
                    o.insert(key, lua_to_json(&val));
                }
                J::Object(o)
            }
        }
        _ => J::Null,
    }
}

/// Run AI-authored Lua in the browser sandbox. Returns `(err, out)`:
/// `err` is the error text (nil on success), `out` is what the code returned,
/// rendered as text (nil when it returned nothing).
///
/// A bare expression is worth its value — `browser_text(BR, "body")` on its
/// own line should answer, not vanish — so the chunk is first compiled
/// REPL-style with `return` prepended, falling back to the plain statement
/// form when that isn't valid Lua
fn run_scoped(
    lua: &mlua::Lua,
    caps: &Caps,
    browser: &str,
    code: &str,
) -> mlua::Result<(Value, Value)> {
    let env = build_sandbox_env(lua, caps, browser)?;
    let func = compile_repl(lua, env, "ai-lua", code);
    match func.and_then(|f| f.call::<MultiValue>(())) {
        Ok(vals) => {
            let mut out = String::new();
            for (i, v) in vals.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                stringify_lua(v, 3, &mut out);
            }
            // A lone `nil` return says nothing worth relaying
            if out.is_empty() || out == "nil" {
                Ok((Value::Nil, Value::Nil))
            } else {
                Ok((Value::Nil, Value::String(lua.create_string(&out)?)))
            }
        }
        Err(e) => Ok((
            Value::String(lua.create_string(e.to_string())?),
            Value::Nil,
        )),
    }
}

/// Build the sandbox's `_ENV`. Contains only safe standard functions plus a restricted shikisha table
fn build_sandbox_env(lua: &mlua::Lua, caps: &Caps, browser: &str) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    // Copy over only the safe standard functions. load/require/os/io/debug/coroutine/getmetatable etc. are excluded
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
    // Check whether the called browser name matches the one allowed for this rally
    fn guard(name: &str, allowed: &str) -> mlua::Result<()> {
        if name == allowed {
            Ok(())
        } else {
            Err(mlua::Error::runtime(crate::i18n::tp(
                "err.hooks.browser_not_allowed",
                &[("name", name), ("allowed", allowed)],
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
    bind!("browser_open", (String, String, Option<String>, Option<bool>), |lua_, c, al, (name, url, profile, private)| {
        guard(&name, &al)?;
        let prof = crate::browser::BrowserProfile::new(
            profile.as_deref().unwrap_or_default(),
            private.unwrap_or(false),
        );
        c.browser_open(&name, &url, prof)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        c.push_replay(format!("browser_open({}, {})", lua_str(&name), lua_str(&url)));
        Ok(())
    });
    bind!("browser_go", (String, String, Option<String>), |lua_, c, al, (name, what, url)| {
        guard(&name, &al)?;
        c.browser_go(&name, go_of(&what, url.clone())?)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        let mut line = format!("browser_go({}, {}", lua_str(&name), lua_str(&what));
        if let Some(u) = &url {
            line.push_str(&format!(", {}", lua_str(u)));
        }
        line.push(')');
        c.push_replay(line);
        Ok(())
    });
    bind!("browser_find", (String, Value), |lua_, c, al, (name, sel)| {
        guard(&name, &al)?;
        c.browser_find(&name, &sel_of(&sel)?)
            .map(|s| s.to_string())
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    // Click/fill return two values: the state, and — on the {ref=N} path —
    // an echo of what was really operated on. A wrong ref number answers
    // for itself instead of failing silently. Each executed op is also
    // journaled in its durable spelling for replay.lua (a ref becomes the
    // anchor of the element it touched; digest never appears in a replay)
    bind!("browser_click", (String, Value, Option<Table>), |lua_, c, al, (name, sel, opts)| {
        guard(&name, &al)?;
        let sel = sel_of(&sel)?;
        let rep = c
            .browser_click(&name, &sel)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        match sel_replay(&sel, &rep.anchor) {
            Some(s) => c.push_replay(format!("browser_click({}, {})", lua_str(&name), s)),
            None => c.push_replay(format!(
                "-- click ({}): {}",
                crate::i18n::t("replay.no_anchor"),
                rep.echo.clone().unwrap_or_default()
            )),
        }
        Ok((check("browser_click", rep.state.as_str(), &opts)?, rep.echo))
    });
    bind!("browser_fill", (String, Value, String, Option<Table>), |lua_, c, al, (name, sel, value, opts)| {
        guard(&name, &al)?;
        let sel = sel_of(&sel)?;
        let rep = c
            .browser_fill(&name, &sel, &value)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        match sel_replay(&sel, &rep.anchor) {
            Some(s) => c.push_replay(format!(
                "browser_fill({}, {}, {})",
                lua_str(&name),
                s,
                lua_str(&value)
            )),
            None => c.push_replay(format!(
                "-- fill ({}): {}",
                crate::i18n::t("replay.no_anchor"),
                rep.echo.clone().unwrap_or_default()
            )),
        }
        Ok((check("browser_fill", rep.state.as_str(), &opts)?, rep.echo))
    });
    // Press a single named key (enter/tab/escape/…) on the focused element.
    // browser_fill only sets a value; this is how the AI submits a form or
    // runs a search: fill the box, then browser_press(BR, "enter").
    bind!("browser_press", (String, String), |lua_, c, al, (name, key)| {
        guard(&name, &al)?;
        c.browser_press(&name, &key)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        c.push_replay(format!("browser_press({}, {})", lua_str(&name), lua_str(&key)));
        Ok(())
    });
    // Fill a field with a secret value. The value is referenced by name and
    // resolved/filled by Rust. The AI never sees the value (only the state
    // is returned). secret_value rejects any key not on the allowlist
    bind!("browser_fill_secret", (String, Value, String), |lua_, c, al, (name, sel, secret_key)| {
        guard(&name, &al)?;
        let value = c
            .secret_value(&secret_key)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        let sel = sel_of(&sel)?;
        // The echo names only the field (attributes), never its value —
        // still safe to relay for a secret fill. The journal keeps the key
        // NAME, exactly like the human recorder does
        let rep = c
            .browser_fill(&name, &sel, &value)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        if let Some(s) = sel_replay(&sel, &rep.anchor) {
            c.push_replay(format!(
                "browser_fill_secret({}, {}, {})",
                lua_str(&name),
                s,
                lua_str(&secret_key)
            ));
        }
        Ok((rep.state.as_str().to_string(), rep.echo))
    });
    // Set up basic auth (credentials come from an allowlisted secret; the value never reaches the AI)
    bind!("browser_auth", (String, String), |lua_, c, al, (name, secret_key)| {
        guard(&name, &al)?;
        c.browser_auth(&name, &secret_key)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_text", (String, Value), |lua_, c, al, (name, sel)| {
        guard(&name, &al)?;
        // The read result goes to the AI, so redact any known secret values
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
    // The page distilled to its operable elements, each numbered for
    // {ref=N} clicks/fills. The intended first move on any new page
    bind!("browser_digest", String, |lua_, c, al, name| {
        guard(&name, &al)?;
        c.browser_digest(&name)
            .map(|s| c.redact(&s))
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    });
    bind!("browser_fetch", (String, String, Option<Table>), |lua_, c, al, (name, url, opts)| {
        guard(&name, &al)?;
        let json = c
            .browser_fetch(&name, &url, &fetch_opts_json(&opts))
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        // Even if secrets appear in the body/headers, redact them before they reach the AI
        let json = c.redact(&json);
        let v: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| mlua::Error::runtime(e.to_string()))?;
        json_to_lua(lua_, &v)
    });
    // Sign in once, save it, load it forever. The count comes back so a rally
    // can tell an empty save (nothing was logged in) from a real one
    bind!("browser_state_save", (String, String), |lua_, c, al, (name, label)| {
        guard(&name, &al)?;
        let n = c.browser_state_save(&name, &label)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        c.push_replay(format!("browser_state_save({}, {})", lua_str(&name), lua_str(&label)));
        Ok(n)
    });
    bind!("browser_state_load", (String, String), |lua_, c, al, (name, label)| {
        guard(&name, &al)?;
        c.browser_state_load(&name, &label)
            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
        c.push_replay(format!("browser_state_load({}, {})", lua_str(&name), lua_str(&label)));
        Ok(())
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
    // Only logging is allowed (so the AI can explain its own moves). No
    // other side effects. Even if the AI tries to log a secret value, it
    // gets redacted here
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
    // In the one-line Lua the AI writes, calling by bare name
    // (browser_go(...)) reads naturally. Place the same functions as
    // shikisha.* directly on the sandbox env's bare globals too (this adds
    // no new capability — it's just an alias for the same function). This
    // also matches how the prompt describes them
    for pair in sh.pairs::<String, Value>() {
        let (k, v) = pair?;
        env.set(k, v)?;
    }
    env.set("shikisha", sh)?;
    Ok(env)
}

/// Map a JSON value directly to a Lua value (so browser_fetch results can be
/// returned as a table, and so the external API can hand a call its arguments).
/// Objects become keyed tables, arrays become 1-indexed sequences
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

/// How to specify a tab. Index numbers change on reorder, so specifying by name is safer
#[derive(Debug, Clone)]
pub enum TabRef {
    Index(usize),
    Name(String),
}

/// How automation identifies a tab (ID takes priority, falling back to tab name)
#[derive(Debug, Clone, Default)]
pub struct TabKey {
    pub id: Option<String>,
    pub name: String,
}

impl TabKey {
    fn matches(&self, s: &str) -> bool {
        // Match by ID if one is set, otherwise match by name
        match &self.id {
            Some(id) => id == s,
            None => self.name == s,
        }
    }
}

impl TabRef {
    /// Resolve the actual (1-indexed) number from a tab list.
    /// A string spec is looked up first by ID, then by tab name
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

/// Operations requested from hooks to the Rust side. Executed by main
#[derive(Debug)]
pub enum Command {
    /// Send as a prompt to another tab (bracketed paste + Enter, inherits chain depth)
    SendPrompt {
        target: TabRef,
        text: String,
        origin: usize,
    },
    /// Send a raw key sequence (e.g. on_question auto-replies; sent as-is, unencoded)
    SendKeys { target: TabRef, keys: String },
    /// Place into the input field without sending (a human can add to it and send it themselves)
    DraftPrompt {
        target: TabRef,
        text: String,
        origin: usize,
    },
    /// Switch the displayed tab (spectator mode: lets a human watch the
    /// AI<->browser turns). 0 = the dashboard (INDEX)
    ShowTab { target: TabRef },
    /// The rally's final result (an exit code and reason the AI produces by
    /// judging whether the goal was met). Written to data/last-result.json,
    /// the log, and the UI
    SetResult {
        code: i32,
        reason: String,
        origin: usize,
    },
    /// Notify a destination. `None` = the primary (config's primary_notify,
    /// or the single configured destination)
    Notify { dest: Option<String>, text: String },
    /// Restart a tab (recovery from an SSH disconnect or a CLI self-update).
    /// The conversation is carried over unless `fresh` asks for a clean one
    Restart { target: TabRef, fresh: bool },
    /// Rearrange the panes. One request, because they are one subject: the
    /// division of the screen belongs to the main loop, which owns the tree
    Pane(PaneOp),
    /// "The conversation running in this tab is <id>." Reported by the tab
    /// itself — from its own hook, or by anything else speaking for it — so the
    /// tab is `origin`, never a name the caller chose to give
    SetSession { id: String, origin: usize },
    /// "This is what I am doing." The caller's own tab unless it names another
    /// — a build script run by hand is nobody's tab and still has something to
    /// say. An empty value takes the entry away, so finishing needs no second verb
    SetStatus { key: String, value: String, target: Option<TabRef>, origin: usize },
    /// "This is how far along I am." `None` takes the bar away
    SetProgress { value: Option<f32>, label: String, target: Option<TabRef>, origin: usize },
    Log(String),
}

/// What a pane request asks for. Each one is a Lua primitive of the same name,
/// so what is written and what happens are the same idea.
///
/// Deliberately NOT one command per compound gesture: "divide and put the
/// browser there" is `split_pane` followed by `show`, written by whoever wants
/// it. Two primitives compose; a fused `split_with_browser` would only ever be
/// the one arrangement somebody thought of first
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaneOp {
    /// Divide the focused pane. The new half takes the surface nothing else shows
    Split(crate::layout::Dir),
    /// Close the focused pane. The tab behind it keeps running
    Close,
    /// Move focus to the neighbour in a direction
    Focus(crate::layout::Move),
    /// Put every divider back to even halves
    Equalize,
}

/// Read a direction the way a person would write it.
///
/// Both spellings of each axis are taken: "right" is what you mean, "row" is
/// what the tree calls it, and refusing either would be pedantry
fn split_dir_of(word: Option<String>) -> mlua::Result<crate::layout::Dir> {
    match word.unwrap_or_else(|| "right".into()).to_ascii_lowercase().as_str() {
        "right" | "row" | "side" => Ok(crate::layout::Dir::Row),
        "down" | "col" | "below" => Ok(crate::layout::Dir::Col),
        other => Err(mlua::Error::runtime(crate::i18n::tp(
            "err.hooks.split_dir",
            &[("other", other)],
        ))),
    }
}

fn move_dir_of(word: &str) -> mlua::Result<crate::layout::Move> {
    match word.to_ascii_lowercase().as_str() {
        "left" => Ok(crate::layout::Move::Left),
        "right" => Ok(crate::layout::Move::Right),
        "up" => Ok(crate::layout::Move::Up),
        "down" => Ok(crate::layout::Move::Down),
        other => Err(mlua::Error::runtime(crate::i18n::tp(
            "err.hooks.focus_dir",
            &[("other", other)],
        ))),
    }
}

/// Page state passed to browser hooks
#[derive(Clone)]
pub struct PageCtx {
    /// The screen's index (same number a human would press)
    pub index: usize,
    /// The name automation refers to it by
    pub id: String,
    /// The human-readable name
    pub name: String,
    pub url: String,
    /// Whether everything the page references has finished loading.
    /// false means "load hasn't fired yet, so this arrived at the DOM-only stage"
    pub complete: bool,
}

/// A snapshot of tab info passed to Lua when a hook fires
#[derive(Clone)]
pub struct TabCtx {
    pub index: usize,
    pub name: String,
    /// The automation name given in the settings, when there is one. This is the
    /// one handle that survives a rename, so it is what a hook should branch on
    /// ("is this the reviewer?") — the number shifts when tabs are reordered and
    /// the display name is the very thing a person changes. `nil` when unset
    pub id: Option<String>,
    pub state: String,
    pub profile: String,
    pub output: String,
    /// Depth of the automation chain. 0 = a conversation started by a human.
    /// Writing `if tab.chain_depth == 0 then return end` lets a hook ignore
    /// human-issued instructions
    pub chain_depth: u32,
    pub locked: bool,
    /// Whether this tab is a model bridge (API). Used for things like
    /// auto-kicking off a discussion's opening speaker
    pub is_model: bool,
    /// For a browser-brain model, its latest reply verbatim (exposed as
    /// `tab.reply`). The orchestrator pulls the ```lua block from this rather
    /// than `output`, whose screen copy is line-wrapped to the tab width and
    /// would split long URLs. None for CLI tabs and plain chat.
    pub reply: Option<String>,
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
-- Wait until the state changes (built from state + sleep, so it's implemented on the Lua side)
function shikisha.wait_state(tab, want, timeout_ms)
  local left = timeout_ms or 60000
  while left > 0 do
    if shikisha.state(tab) == want then return true end
    shikisha.sleep(200)
    left = left - 200
  end
  return false
end
-- Whichever comes first wins: the page reaching a state, a human pressing
-- the button, or a timeout.
-- Always show the button: so it doesn't get stuck if the selector misses
function shikisha.browser_wait(name, opts)
  opts = opts or {}
  local left = opts.timeout_ms or 300000
  local step = 300
  if opts.ask then
    shikisha.browser_ask(name, opts.ask, opts.label or shikisha.t("agent.browser.wait.label"))
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

/// A single Lua script. Loaded with its own environment (_ENV), so
/// multiple files defining the same `on_done` don't collide
struct Script {
    /// Path used for display
    path: String,
    /// This script's environment table (hook functions are looked up from here)
    env: Table,
    defined: HashSet<String>,
}

/// Where a hook resolves to. The more specific one wins (tab > workspace > base)
#[derive(Default)]
struct Attach {
    base: Option<usize>,
    workspace: Option<usize>,
    tabs: std::collections::HashMap<usize, usize>,
}

/// Capabilities granted to automation (default is empty = no file or network access)
pub type Caps = std::rc::Rc<crate::caps::Capabilities>;

/// How often the VM stops to let us count, and how much one entry into Lua
/// may spend before it is called a runaway.
///
/// Nothing this app runs in Lua is compute — it is glue that calls straight
/// back into Rust — so a chunk burning tens of millions of VM instructions is
/// not working, it is spinning. Without a ceiling, `while true do end` freezes
/// the window outright: the engine runs on the main loop, and the loop cannot
/// come back until Lua returns. Three doors accept hand-written code (the
/// composer's ▶, the same ▶ from a phone over the network, and the external
/// API), and all of them pass through here.
///
/// The budget counts *instructions*, never wall time. Waiting inside a
/// primitive — an auto-waiting click is allowed 30 seconds — executes no VM
/// instructions at all, so a slow page can never be mistaken for a loop.
const LUA_STEP_TRIGGER: u32 = 500_000;
const LUA_STEP_BUDGET: u64 = 20_000_000;

/// The instruction allowance of the entry into Lua that is currently running.
///
/// Shared with the VM hook, which charges it every `LUA_STEP_TRIGGER`
/// instructions and raises once it is overdrawn. `depth` is what makes the
/// allowance belong to the *entry* rather than to each call: a primitive that
/// runs more Lua (`shikisha.run_scoped`) must not hand the loop it is nested
/// in a fresh budget, or the ceiling never arrives.
#[derive(Clone, Default)]
struct StepBudget {
    spent: Rc<Cell<u64>>,
    depth: Rc<Cell<u32>>,
}

impl StepBudget {
    /// Arm the budget for one entry into Lua. Dropping the guard disarms it
    fn arm(&self) -> StepGuard {
        if self.depth.get() == 0 {
            self.spent.set(0);
        }
        self.depth.set(self.depth.get() + 1);
        StepGuard(self.clone())
    }

    /// Charge one trigger interval to the running entry
    fn charge(&self) -> mlua::Result<()> {
        // Lua running outside any armed entry is not ours to police
        if self.depth.get() == 0 {
            return Ok(());
        }
        let spent = self.spent.get() + LUA_STEP_TRIGGER as u64;
        self.spent.set(spent);
        if spent > LUA_STEP_BUDGET {
            return Err(mlua::Error::runtime(crate::i18n::t("err.hooks.step_limit")));
        }
        Ok(())
    }
}

struct StepGuard(StepBudget);

impl Drop for StepGuard {
    fn drop(&mut self) {
        self.0.depth.set(self.0.depth.get().saturating_sub(1));
    }
}

pub struct HookEngine {
    lua: Lua,
    commands: Rc<RefCell<Vec<Command>>>,
    current_origin: Rc<Cell<usize>>,
    /// Each tab's (identifier, current state). Made readable from inside loops
    states: Rc<RefCell<Vec<(TabKey, String)>>>,
    /// Each tab's (identifier, latest captured reply), same order as `states`. Lets
    /// an operator read the tab it's driving (shikisha.tab_output) when that tab is
    /// another AI rather than a browser.
    outputs: Rc<RefCell<Vec<(TabKey, String)>>>,
    pending: Vec<Pending>,
    scripts: Vec<Script>,
    attach: Attach,
    /// Kept so composer-typed Lua (▶ run mode) can enter the same run_scoped
    /// sandbox the rally uses, without threading capabilities through the loop.
    caps: Caps,
    /// The phone board's URL (with token), pushed in by the main loop.
    /// None while remote is off
    remote_url: Rc<RefCell<Option<String>>>,
    /// The runaway ceiling, armed at every door that enters Lua
    budget: StepBudget,
}

const HOOK_NAMES: [&str; 5] = ["on_start", "on_question", "on_busy", "on_done", "on_exit"];

/// Hooks available on a browser tab.
///
/// Session state doesn't apply to a page, so the vocabulary is kept
/// separate. Add more only once there's a reason to
pub const PAGE_HOOK_NAMES: [&str; 2] = ["on_load", "on_press"];

impl HookEngine {
    /// Load a single script and attach it as the base config (for tests / simple setups)
    #[cfg(test)]
    pub fn from_source(source: &str) -> Result<Self> {
        let mut e = Self::new()?;
        let id = e.load_source("(inline)", source)?;
        e.attach.base = Some(id);
        Ok(e)
    }

    /// An engine with no capabilities granted (for tests / simple setups)
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

        // The runaway ceiling. Set globally rather than per-thread so every
        // coroutine mlua makes later (wait/sleep run inside one) inherits it —
        // a per-thread hook would leave exactly the long-running code unguarded
        let budget = StepBudget::default();
        {
            let b = budget.clone();
            lua.set_global_hook(
                mlua::HookTriggers::new().every_nth_instruction(LUA_STEP_TRIGGER),
                move |_, _| b.charge().map(|()| mlua::VmState::Continue),
            )
            .map_err(lerr)?;
        }

        let commands: Rc<RefCell<Vec<Command>>> = Rc::new(RefCell::new(Vec::new()));
        let current_origin = Rc::new(Cell::new(1usize));
        let states: Rc<RefCell<Vec<(TabKey, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let outputs: Rc<RefCell<Vec<(TabKey, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let remote_url: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        let shikisha = lua.create_table().map_err(lerr)?;
        {
            // Where a phone can reach this app (None while remote is off).
            // Lets a "human needed" notification carry a tappable way in
            let u = Rc::clone(&remote_url);
            shikisha
                .set(
                    "remote_url",
                    lua.create_function(move |_, ()| Ok(u.borrow().clone()))
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Read the current state. Used as a loop's exit condition
            // (the tab.state hook argument is a snapshot at fire time and never changes)
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
            // Read another tab's latest reply — used by an operator driving a
            // second AI tab to see what it answered. Empty string if unknown.
            let o = Rc::clone(&outputs);
            shikisha
                .set(
                    "tab_output",
                    lua.create_function(move |_, tab: Value| {
                        let r = tab_ref_of(&tab)?;
                        let outputs = o.borrow();
                        let keys: Vec<TabKey> = outputs.iter().map(|(k, _)| k.clone()).collect();
                        Ok(r.resolve(&keys)
                            .and_then(|i| outputs.get(i - 1).map(|(_, out)| out.clone()))
                            .unwrap_or_default())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Translate display text. en.json base + language overlay (i18n).
            // Keeps the strings the built-in orchestrators (discussion / browser
            // operation) send to the AI, and the transcript headings, English-first.
            // Where interpolation is needed, tf(key, {name="…"}) substitutes {name}
            shikisha
                .set(
                    "t",
                    lua.create_function(|_, key: String| Ok(crate::i18n::t(&key)))
                        .map_err(lerr)?,
                )
                .map_err(lerr)?;
            shikisha
                .set(
                    "tf",
                    lua.create_function(
                        |_, (key, args): (String, std::collections::HashMap<String, String>)| {
                            let mut s = crate::i18n::t(&key);
                            for (k, v) in args {
                                s = s.replace(&format!("{{{k}}}"), &v);
                            }
                            Ok(s)
                        },
                    )
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
                    // Take the text as a raw Lua string, not `String`: a turn's
                    // relayed context can carry stray invalid UTF-8 from the
                    // terminal capture, and a strict conversion would raise here
                    // and abort the whole discussion mid-round. Lossy-decode so
                    // the round keeps going (a bad byte becomes U+FFFD).
                    lua.create_function(move |_, (target, text): (Value, mlua::LuaString)| {
                        c.borrow_mut().push(Command::SendPrompt {
                            target: tab_ref_of(&target)?,
                            text: text.to_string_lossy(),
                            origin: o.get(),
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Switch the displayed tab. The core of spectator mode (moves the
            // visible screen on each turn). The target may be a session or a
            // browser (addressable by index, name, id, or a tab table)
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
            // The rally's final result: an exit code and reason the AI produces by judging whether the goal was met
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
        {
            // Open the result view for a finished run (discussion / rally).
            // Hands the run folder over; the main loop turns its transcript.md
            // into the chat-style result tab and switches to it. A thin signal,
            // not a merged primitive: the orchestrator decides *when* a run is
            // done, the main loop decides *how* the page is shown.
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "open_result",
                    lua.create_function(move |_, run: String| {
                        c.request_open_result(&run);
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        // ── Browser ──────────────────────────────
        // Selector / on_missing interpretation lives in the module functions
        // sel_of / check (the sandboxed run_scoped uses the same ones)
        {
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_open",
                    lua.create_function(move |_, (name, url, profile, private): (String, String, Option<String>, Option<bool>)| {
                        let prof = crate::browser::BrowserProfile::new(
                            profile.as_deref().unwrap_or_default(),
                            private.unwrap_or(false),
                        );
                        c.browser_open(&name, &url, prof)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Drive the current tab in place: back / forward / reload /
            // to(URL). Unlike browser_open, this doesn't recreate the
            // webview, so any auth already set up survives
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
                            let rep = c
                                .browser_click(&name, &sel_of(&sel)?)
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            Ok((check("browser_click", rep.state.as_str(), &opts)?, rep.echo))
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
                            let rep = c
                                .browser_fill(&name, &sel_of(&sel)?, &value)
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            Ok((check("browser_fill", rep.state.as_str(), &opts)?, rep.echo))
                        },
                    )
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Fill a field with a secret value. Referenced by name and
            // resolved/filled by Rust, so the value never reaches Lua/the
            // AI. Only the key name is kept in the record (so it can still
            // be pasted to replay)
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_fill_secret",
                    lua.create_function(move |_, (name, sel, key): (String, Value, String)| {
                        let value = c
                            .secret_value(&key)
                            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                        // The echo names only the field, never its value
                        c.browser_fill(&name, &sel_of(&sel)?, &value)
                            .map(|rep| (rep.state.as_str().to_string(), rep.echo))
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Set up basic auth. Credentials are resolved from an
            // allowlisted secret. Calling this before navigating to /
            // reloading a protected page answers the 401 automatically
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
                    // For the orchestrator's control loop: when unreadable
                    // (no response, timeout, etc.) return nil instead of
                    // raising, so a single failed move doesn't take down the
                    // whole loop (the judge's safety net keeps running).
                    // The error is only recorded to the log
                    lua.create_function(move |_, (name, sel): (String, Value)| {
                        match c.browser_text(&name, &sel_of(&sel)?) {
                            Ok(v) => Ok(v),
                            Err(e) => {
                                crate::append_hook_log(&crate::i18n::tp(
                                    "err.hooks.browser_text_unreadable",
                                    &[("e", &format!("{e}"))],
                                ));
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
                    // Same as browser_text: return nil instead of raising when unreadable
                    lua.create_function(move |lua, name: String| match c.browser_html(&name) {
                        Ok(h) => Ok(Value::String(lua.create_string(&h)?)),
                        Err(e) => {
                            crate::append_hook_log(&crate::i18n::tp(
                                "err.hooks.browser_html_unreadable",
                                &[("e", &format!("{e}"))],
                            ));
                            Ok(Value::Nil)
                        }
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // The page distilled to its operable elements, numbered for
            // {ref=N} operations. Raises on failure (unlike browser_html:
            // an automation asking for a digest wants to know why it failed)
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_digest",
                    lua.create_function(move |_, name: String| {
                        c.browser_digest(&name)
                            .map(|s| c.redact(&s))
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Make a request from inside the page. Returns a table of
            // {status,ok,url,headers,body}. opts is
            // {method=..,headers={..},body=..} (optional)
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
            // Save a page's login (its cookies) under a name, and put it back
            // later. The count comes back so an empty save is distinguishable
            // from a real one
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "browser_state_save",
                    lua.create_function(move |_, (name, label): (String, String)| {
                        c.browser_state_save(&name, &label)
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
                    "browser_state_load",
                    lua.create_function(move |_, (name, label): (String, String)| {
                        c.browser_state_load(&name, &label)
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
                        // If nothing is passed, show everything. Picking
                        // individually is only for those who want to
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
                    // notify(text) reaches the primary destination;
                    // notify(dest, text) names one (the original two-arg
                    // spelling keeps working unchanged)
                    lua.create_function(move |_, (a, b): (String, Option<String>)| {
                        let (dest, text) = match b {
                            Some(text) => (Some(a), text),
                            None => (None, a),
                        };
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
                    // The conversation carries over by default, because the
                    // reasons to restart a tab (it died, it hung, it updated
                    // itself) are reasons to want it back. `"fresh"` asks for
                    // a clean one
                    lua.create_function(move |_, (tab, how): (Value, Option<String>)| {
                        c.borrow_mut().push(Command::Restart {
                            target: tab_ref_of(&tab)?,
                            fresh: how.as_deref() == Some("fresh"),
                        });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Date/time. Using it in file names is a common enough need
            // that this is exposed on its own. Handing over the whole os
            // module would also hand over the tools for spawning
            // processes. The caller decides the format; the default
            // sorts chronologically when reordered
            shikisha
                .set(
                    "now",
                    lua.create_function(|_, fmt: Option<String>| {
                        Ok(local_stamp(fmt.as_deref().unwrap_or("%Y%m%d%H%M%S")))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // For measuring elapsed time: milliseconds since the UNIX
            // epoch. Used by the judge's "time" stop condition (now is for
            // display/filenames and has no %s, so a separate numeric clock
            // is exposed)
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
        // File/HTTP access is only allowed through a "registered gateway"
        // (caps.rs). Raw io/os is never granted; only Rust-side functions
        // are injected
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
        // For power users: raw paths / raw URLs (always fails if allow_dirs / allow_hosts is empty)
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
            // Rally recording. The orchestrator appends each turn's
            // executed Lua, keeping it in a form that can be pasted back in
            // to replay. Trusted (orchestrator) side only — never exposed
            // to the AI-authored Lua sandbox
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
            // The rally hand-off area (exchange). A bridge for receiving
            // raw Lua the AI wrote byte-exact from a file, rather than by
            // "reading the screen". Orchestrator only (never exposed to
            // the sandbox).
            // exchange_new: create a folder for this run and return its
            // path (forward slashes normalized)
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
            // exchange_take: read the file, delete it, and return its contents (nil if absent). Consumes a temp file
            shikisha
                .set(
                    "exchange_take",
                    lua.create_function(|_, path: String| {
                        Ok(crate::exchange::take(std::path::Path::new(&path)))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // exchange_append: append one move's worth to the record (record.lua)
            shikisha
                .set(
                    "exchange_append",
                    // `text` as a raw Lua string (see send_to_tab): an AI turn
                    // recorded into the transcript can carry invalid UTF-8, and
                    // a strict String conversion would raise and abort the round.
                    lua.create_function(|_, (path, text): (String, mlua::LuaString)| {
                        crate::exchange::append(std::path::Path::new(&path), &text.to_string_lossy())
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // exchange_write: overwrite a file (stage the current screen for a
            // file-reading CLI operator). Same raw-string handling as append.
            shikisha
                .set(
                    "exchange_write",
                    lua.create_function(|_, (path, text): (String, mlua::LuaString)| {
                        crate::exchange::write(std::path::Path::new(&path), &text.to_string_lossy())
                            .map_err(|e| mlua::Error::runtime(e.to_string()))
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            // lint: syntax-check the Lua only (never executes it). Returns
            // an error string if broken, nil if sound. The actual
            // permission sandboxing is enforced at run time by
            // run_scoped's environment
            shikisha
                .set(
                    "lint",
                    lua.create_function(|lua, code: String| match lint_lua(&code) {
                        None => Ok(Value::Nil),
                        Some(e) => Ok(Value::String(lua.create_string(e)?)),
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Run Lua written by the AI in a restricted environment
            // (browser functions only, on a single allowed tab). The
            // boundary that lets the orchestrator run untrusted input
            // (Lua generated from web content) without touching
            // file/http/raw secret values/other tabs
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
        {
            // Drain the replay journal: the durable spellings of every op
            // executed since the last take. The orchestrator appends them to
            // the run's replay.lua — a script with no digest/ref dependency
            let c = Caps::clone(&caps);
            shikisha
                .set(
                    "take_replay",
                    lua.create_function(move |lua, ()| {
                        let t = lua.create_table()?;
                        for (i, line) in c.take_replay().into_iter().enumerate() {
                            t.set(i + 1, line)?;
                        }
                        Ok(t)
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // The tab says which conversation it is running.
            //
            // No tab argument: the caller IS the tab. Inside the app that is
            // the tab whose hook fired; over the external API it is the tab
            // whose key opened the connection. Letting a caller name someone
            // else's tab would turn an authenticated report into a claim
            let c = Rc::clone(&commands);
            let o = Rc::clone(&current_origin);
            shikisha
                .set(
                    "set_session",
                    lua.create_function(move |_, id: String| {
                        c.borrow_mut().push(Command::SetSession { id, origin: o.get() });
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // What the thing in this tab is doing, in its own words.
            //
            // The state dot is READ OFF THE SCREEN and can only ever say
            // "busy" or "waiting"; this is the other half — an agent that
            // knows what it is doing saying so. Detection is for the CLIs that
            // will not tell us; this is for the ones that will.
            //
            // Keyed, so a build script and the agent driving it can both speak
            // without overwriting each other. No icon, no colour, no priority:
            // a tab row is eighteen columns wide, and a row that cannot show
            // them should not accept them
            let c = Rc::clone(&commands);
            let o = Rc::clone(&current_origin);
            shikisha
                .set(
                    "set_status",
                    lua.create_function(
                        move |_, (key, value, tab): (String, Option<String>, Option<Value>)| {
                            c.borrow_mut().push(Command::SetStatus {
                                key,
                                value: value.unwrap_or_default(),
                                target: tab.as_ref().map(tab_ref_of).transpose()?,
                                origin: o.get(),
                            });
                            Ok(())
                        },
                    )
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            let c = Rc::clone(&commands);
            let o = Rc::clone(&current_origin);
            shikisha
                .set(
                    "set_progress",
                    lua.create_function(
                        move |_, (value, label, tab): (Option<f32>, Option<String>, Option<Value>)| {
                            c.borrow_mut().push(Command::SetProgress {
                                value: value.map(|v| v.clamp(0.0, 1.0)),
                                label: label.unwrap_or_default(),
                                target: tab.as_ref().map(tab_ref_of).transpose()?,
                                origin: o.get(),
                            });
                            Ok(())
                        },
                    )
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // The division of the screen, in the same words the keys use.
            // Composable on purpose: "divide, and put the browser in the new
            // half" is these two lines, not a command of its own —
            //   shikisha.split_pane("right"); shikisha.show("br")
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "split_pane",
                    lua.create_function(move |_, dir: Option<String>| {
                        c.borrow_mut().push(Command::Pane(PaneOp::Split(split_dir_of(dir)?)));
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "close_pane",
                    lua.create_function(move |_, ()| {
                        c.borrow_mut().push(Command::Pane(PaneOp::Close));
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "focus_pane",
                    lua.create_function(move |_, dir: String| {
                        c.borrow_mut().push(Command::Pane(PaneOp::Focus(move_dir_of(&dir)?)));
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
            let c = Rc::clone(&commands);
            shikisha
                .set(
                    "equalize_panes",
                    lua.create_function(move |_, ()| {
                        c.borrow_mut().push(Command::Pane(PaneOp::Equalize));
                        Ok(())
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Run a whole chunk with everything in reach — loops, branches and
            // several primitives in one go. `run_scoped` is the walled version
            // of this (one browser, nothing else) and stays the one AI-authored
            // code gets; this one is for the people and programs that already
            // hold the keys: a script file, and the external API.
            //
            // Returns `(err, ...)`: the error text (nil when it ran) followed by
            // whatever the chunk returned — the values themselves, not a
            // rendering of them, so a caller can go on using them.
            //
            // Always at least two values, even when there is nothing to say.
            // Over the external API those become a JSON array, and a lone value
            // would collapse to a bare one: an answer of `"..."` would then read
            // as either an error or a returned string, with no way to tell
            shikisha
                .set(
                    "lua",
                    lua.create_function(move |lua, code: String| {
                        let mut out = MultiValue::new();
                        match compile_repl(lua, full_env(lua)?, "lua", &code)
                            .and_then(|f| f.call::<MultiValue>(()))
                        {
                            Ok(vals) => {
                                out.push_back(Value::Nil);
                                out.extend(vals);
                            }
                            Err(e) => {
                                out.push_back(Value::String(lua.create_string(e.to_string())?));
                            }
                        }
                        while out.len() < 2 {
                            out.push_back(Value::Nil);
                        }
                        Ok(out)
                    })
                    .map_err(lerr)?,
                )
                .map_err(lerr)?;
        }
        {
            // Every primitive there is, read off the table itself at the moment
            // it is asked. Not a hand-kept list: one written by hand would
            // answer for the day it was written, and the external API's idea of
            // what exists would drift from what Lua actually has
            shikisha
                .set(
                    "list",
                    lua.create_function(move |lua, ()| {
                        let mut names = primitive_names(lua)?;
                        names.sort();
                        lua.create_sequence_from(names)
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
            outputs,
            pending: Vec::new(),
            scripts: Vec::new(),
            attach: Attach::default(),
            caps,
            remote_url,
            budget,
        })
    }

    /// Run composer-typed Lua (▶ run mode) against one browser, in the very
    /// sandbox the rally runs AI-authored code in: browser functions on that
    /// one tab, nothing else. None on success, the error text on failure
    /// Call one primitive by the name Lua calls it by, with positional
    /// arguments. The door the external API knocks on.
    ///
    /// Dispatch is a lookup in the `shikisha` table, not a list of arms: a
    /// primitive written for Lua is reachable from outside the same minute,
    /// and nothing here has to be told about it. Several return values come
    /// back as an array, since that is what the caller was handed.
    /// The same call, made in the name of the tab whose token opened the
    /// connection.
    ///
    /// What that tab sends then inherits its chain depth, exactly as it would
    /// if it had gone through the screen — the runaway brake counts AI handing
    /// work to AI, and this door must not be the way around it. A caller from
    /// outside every tab counts as a person: the chain starts over.
    pub fn call_primitive_as(
        &self,
        caller: Option<&str>,
        method: &str,
        params: &[serde_json::Value],
    ) -> std::result::Result<serde_json::Value, String> {
        let previously = self.current_origin.get();
        let origin = caller
            .and_then(|c| {
                let states = self.states.borrow();
                let keys: Vec<TabKey> = states.iter().map(|(k, _)| k.clone()).collect();
                TabRef::Name(c.to_string()).resolve(&keys)
            })
            .unwrap_or(0);
        self.current_origin.set(origin);
        let out = self.call_primitive(method, params);
        self.current_origin.set(previously);
        out
    }

    pub fn call_primitive(
        &self,
        method: &str,
        params: &[serde_json::Value],
    ) -> std::result::Result<serde_json::Value, String> {
        let _budget = self.budget.arm();
        let sh: Table = self.lua.globals().get("shikisha").map_err(|e| e.to_string())?;
        let Ok(Value::Function(f)) = sh.get::<Value>(method) else {
            return Err(format!("no such primitive: {method}"));
        };
        let mut args = MultiValue::new();
        for p in params {
            args.push_back(json_to_lua(&self.lua, p).map_err(|e| e.to_string())?);
        }
        let vals: MultiValue = f.call(args).map_err(|e| e.to_string())?;
        let mut out: Vec<serde_json::Value> = vals.iter().map(lua_to_json).collect();
        Ok(match out.len() {
            0 => serde_json::Value::Null,
            1 => out.remove(0),
            _ => serde_json::Value::Array(out),
        })
    }

    pub fn run_browser_lua(&self, browser: &str, code: &str) -> Option<String> {
        let _budget = self.budget.arm();
        let out = match run_scoped(&self.lua, &self.caps, browser, code) {
            Ok((Value::String(s), _)) => Some(s.to_string_lossy().to_string()),
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };
        // The composer shows this error too, but the composer's display is
        // ephemeral — a failed ▶ run that leaves no trace is undebuggable
        // after the fact (only the error, never the code's output/values)
        if let Some(e) = &out {
            crate::append_hook_log(&format!("run lua error: {e}"));
        }
        // Composer-run code is already durable Lua in the user's hands —
        // discard its journal so it never bleeds into a rally's replay.lua
        let _ = self.caps.take_replay();
        out
    }

    /// Reflect every tab's (identifier, state) on each detection tick
    pub fn set_states(&self, states: Vec<(TabKey, String)>) {
        *self.states.borrow_mut() = states;
    }

    /// Reflect every tab's (identifier, latest reply) on each detection tick, so an
    /// operator can read the AI tab it's driving via shikisha.tab_output.
    pub fn set_outputs(&self, outputs: Vec<(TabKey, String)>) {
        *self.outputs.borrow_mut() = outputs;
    }

    /// The phone board's URL (token included), or None while remote is off.
    /// Orchestrators put it into "a human is needed" notifications so the
    /// person can act from wherever the notification reached them
    pub fn set_remote_url(&self, url: Option<String>) {
        *self.remote_url.borrow_mut() = url;
    }

    /// Drop any loop waiting on that tab (on exit / restart)
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

    /// Drop every waiting loop (emergency stop)
    pub fn cancel_all(&mut self) {
        for p in std::mem::take(&mut self.pending) {
            let _ = self.lua.remove_registry_value(p.key);
        }
    }

    /// Load an automation. The same path is reused if seen again.
    /// A directory is treated as the per-event-file layout (`on_done.lua`
    /// etc.); a `.lua` file is treated as the legacy function-definition
    /// layout
    pub fn load_path(&mut self, path: &std::path::Path) -> Result<usize> {
        let key = path.display().to_string();
        if let Some(i) = self.scripts.iter().position(|s| s.path == key) {
            return Ok(i);
        }
        if path.is_dir() {
            self.load_dir(path)
        } else {
            let source = std::fs::read_to_string(path)
                .with_context(|| crate::i18n::tp("err.hooks.cannot_read_script", &[("key", &key)]))?;
            self.load_source(&key, &source)
        }
    }

    /// The per-event-file layout. Each file's contents are "just the
    /// processing body", so wrap it in a function on the Rust side before
    /// registering it as a hook
    fn load_dir(&mut self, dir: &std::path::Path) -> Result<usize> {
        let key = dir.display().to_string();
        // Load shared helper functions first (namespace is shared within the same directory)
        let shared = dir.join("_shared.lua");
        let mut source = String::new();
        if shared.is_file() {
            source.push_str(
                &std::fs::read_to_string(&shared)
                    .with_context(|| crate::i18n::tp("err.hooks.cannot_read", &[("path", &shared.display().to_string())]))?,
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
                .with_context(|| crate::i18n::tp("err.hooks.cannot_read", &[("path", &f.display().to_string())]))?;
            // on_question receives the screen text as its second argument
            source.push_str(&format!("function {hook}(tab, screen)\n{body}\nend\n"));
            found = true;
        }
        // Browser hooks receive a page, not a tab
        for hook in PAGE_HOOK_NAMES {
            let f = dir.join(format!("{hook}.lua"));
            if !f.is_file() {
                continue;
            }
            let body = std::fs::read_to_string(&f)
                .with_context(|| crate::i18n::tp("err.hooks.cannot_read", &[("path", &f.display().to_string())]))?;
            source.push_str(&format!("function {hook}(page)\n{body}\nend\n"));
            found = true;
        }
        if !found && source.is_empty() {
            anyhow::bail!(crate::i18n::tp(
                "err.hooks.no_event_files",
                &[("key", &key)]
            ));
        }
        self.load_source(&key, &source)
    }

    /// Load a script with its own independent environment.
    /// The environment's __index points at the globals (string/math/shikisha
    /// etc.), so standard functions and the API are usable, but hook
    /// functions stay scoped to their own script
    fn load_source(&mut self, path: &str, source: &str) -> Result<usize> {
        // A script's top level is code too — a loop there would hang the app
        // before a single hook had a chance to fire
        let _budget = self.budget.arm();
        let env = full_env(&self.lua).map_err(lerr)?;
        self.lua
            .load(source)
            .set_environment(env.clone())
            .exec()
            .map_err(|e| {
                anyhow::anyhow!(crate::i18n::tp(
                    "err.hooks.script_run_failed",
                    &[("path", path), ("e", &format!("{e}"))]
                ))
            })?;

        let mut defined = HashSet::new();
        // Check both session hooks and browser hooks
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

    /// Load the built-in orchestrator for browser-operation mode, targeting
    /// the given browser (chat-style).
    ///
    /// The user never writes Lua. The goal is **typed into the input
    /// field**, not configured. What's typed (chain 0) is picked up as a
    /// new goal/correction, and the AI repeats: write one browser move at a
    /// time to in.lua -> execute -> return the screen. Once the AI reports
    /// instead of writing a move, that's treated as a checkpoint and it
    /// waits for the next input. Runaway loops are always stopped by the
    /// safety net. BR (the id of the browser being operated) is injected up
    /// front
    pub fn load_browser_agent(&mut self, browser: &str, stops_lua: &str) -> Result<usize> {
        // Runaway limits and the on-limit policy come from config; fold them into
        // the cache key so editing them in settings yields a fresh script.
        let op = crate::config::operate();
        let key = format!(
            "<browser-agent:{browser}>{stops_lua}|{}|{}|{}|{}|{}|{}",
            op.max_rounds, op.max_seconds, op.max_tokens, op.on_limit, op.settle_ms, op.confirm
        );
        if let Some(i) = self.scripts.iter().position(|s| s.path == key) {
            return Ok(i);
        }
        // Built-in orchestrator (function-definition layout). BR, STOPS and the
        // limits (MAX_ROUNDS/MAX_SEC/MAX_TOK) plus ON_LIMIT are injected below.
        const SRC: &str = r##"

-- Judge: evaluate the configured stop conditions (STOPS) top to bottom and
-- return the one that matched (nil if none did).
-- screen/css/xpath look at the tab (default BR). console looks at this
-- turn's AI output (screen_out)
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

-- A model brain can't write files, so it hands over the next move as a fenced
-- ```lua block in its reply. Pull that block out (any/no language tag). Fall
-- back to gathering bare browser_*/shikisha./local lines if the model forgot
-- the fence. Returns nil when there's nothing runnable.
local function extract_lua(reply)
  if not reply or #reply == 0 then return nil end
  local body = reply:match("```%s*%w*%s*\n(.-)```")
  if not body then body = reply:match("```%s*%w*%s*(.-)```") end
  if body and #(body:gsub("%s", "")) > 0 then return body end
  local lines = {}
  for line in (reply .. "\n"):gmatch("(.-)\n") do
    if line:match("browser_%w+%s*%(") or line:match("shikisha%.")
        or line:match("^%s*local%s") or line:match("^%s*for%s") or line:match("^%s*if%s") then
      lines[#lines + 1] = line
    end
  end
  if #lines > 0 then return table.concat(lines, "\n") end
  return nil
end

-- A brain signals completion by replying with a bare DONE (no code block).
local function is_done(reply)
  if not reply then return false end
  for line in (reply .. "\n"):gmatch("(.-)\n") do
    local w = line:gsub("[%s%p]", "")
    if w == "DONE" or w == "done" then return true end
  end
  return false
end

local function protocol(run)
  local infile = run .. "/in.lua"
  local humanfile = run .. "/human.txt"
  return table.concat({
    shikisha.tf("agent.browser.proto.intro", { br = BR }),
    shikisha.t("agent.browser.proto.each_turn"),
    "  " .. infile,
    shikisha.tf("agent.browser.proto.funcs_header", { br = BR }),
    "    browser_go(\"" .. BR .. "\", \"to\"|\"reload\"|\"back\"|\"forward\", url?)",
    "    browser_digest(\"" .. BR .. "\")",
    "    browser_click(\"" .. BR .. "\", sel)   browser_fill(\"" .. BR .. "\", sel, value)   browser_press(\"" .. BR .. "\", key)",
    "    browser_fill_secret(\"" .. BR .. "\", sel, " .. shikisha.t("agent.browser.secret_name") .. ")   browser_auth(\"" .. BR .. "\", " .. shikisha.t("agent.browser.secret_name") .. ")",
    "    browser_text(\"" .. BR .. "\", sel)   browser_find(\"" .. BR .. "\", sel)",
    shikisha.t("agent.browser.proto.sel_note"),
    shikisha.t("agent.browser.proto.digest_note"),
    shikisha.tf("agent.browser.proto.result_note", { br = BR }),
    shikisha.t("agent.browser.proto.press_note"),
    shikisha.t("agent.browser.proto.human_before") .. humanfile .. shikisha.t("agent.browser.proto.human_after"),
    shikisha.t("agent.browser.proto.done_note"),
  }, "\n")
end

local function reset_budget()
  shikisha.set_var("rally_round", 0)
  shikisha.set_var("rally_t0", shikisha.epoch_ms())
  shikisha.set_var("rally_tok", 0)
end

-- Append one entry to the human-readable record (transcript). Used for downloads
local function tx(entry)
  local p = shikisha.get_var("rally_tx")
  if p then shikisha.exchange_append(p, entry) end
end

-- Cut a string to at most n bytes without splitting a UTF-8 character
-- (a naive sub() would leave a broken half-character at the cut)
local function clip(s, n)
  if #s <= n then return s end
  local cut = n
  while cut > 0 do
    local b = s:byte(cut + 1)
    if not b or b < 0x80 or b >= 0xC0 then break end
    cut = cut - 1
  end
  return s:sub(1, cut) .. "…"
end

-- Where to tell the AI to put its next move. A CLI agent writes a file; a
-- model brain just replies with a ```lua block (or DONE).
local function fix_hint(tab, infile)
  if tab.is_model then return shikisha.t("agent.browser.model.fix") end
  return shikisha.tf("agent.browser.lint.fix", { infile = infile })
end
local function retry_hint(tab, infile)
  if tab.is_model then return shikisha.t("agent.browser.model.retry") end
  return shikisha.tf("agent.browser.run.retry", { infile = infile })
end
local function next_hint(tab, infile)
  if tab.is_model then return shikisha.t("agent.browser.model.next") end
  return shikisha.t("agent.browser.next_action.before") .. infile .. shikisha.t("agent.browser.next_action.after")
end

-- Does this move submit/click/authenticate (vs. only read)? Used by the brake in
-- CONFIRM="sends" mode to pause before a step that changes the page.
local function touches_send(code)
  code = code or ""
  return code:find("browser_press", 1, true) ~= nil
    or code:find("browser_click", 1, true) ~= nil
    or code:find("browser_auth", 1, true) ~= nil
    or code:find("browser_fill_secret", 1, true) ~= nil
end

-- Hand the turn back to the AI, with the screen on it. These two go together on
-- every path here: the AI is about to work and watching it is the whole point.
-- Passing work no longer moves the screen by itself, so asking for it is the job
-- of whoever wants to be watched
local function back_to_ai(ai, msg)
  shikisha.show(ai)
  shikisha.send_to_tab(ai, msg)
end

-- The brake (CONFIRM). Before a move runs, optionally hold for a person to approve
-- it via a button on the page. Returns true to proceed, false if it wasn't approved.
local function brake_ok(code)
  if CONFIRM ~= "all" and not (CONFIRM == "sends" and touches_send(code)) then return true end
  shikisha.show(BR)
  local r = shikisha.browser_wait(BR, {
    ask = shikisha.tf("agent.brake.ask", { code = code }),
    label = shikisha.t("agent.brake.go"),
  })
  return r == "button"
end

function on_start(tab)
  local run = shikisha.exchange_new()
  shikisha.set_var("rally_run", run)
  shikisha.set_var("rally_record", run .. "/record.lua")
  shikisha.set_var("rally_tx", run .. "/transcript.md")
  shikisha.set_var("rally_nocode", 0)
  reset_budget()
  -- Start the durable replay fresh: drop journal lines left over from any
  -- earlier context, and stamp the header
  shikisha.take_replay()
  pcall(shikisha.exchange_write, run .. "/replay.lua", shikisha.t("transcript.replay.header") .. "\n")
  -- Stage the opening digest too, so a task on an already-open page can act
  -- on move one without spending it on browser_digest
  local okd, dg = pcall(shikisha.browser_digest, BR)
  if okd and type(dg) == "string" then
    pcall(shikisha.exchange_write, run .. "/digest.txt", dg)
  end
  tx(shikisha.t("transcript.rally.header") .. "\n")
  tx(shikisha.tf("transcript.rally.mode", { br = BR }) .. "\n")
  -- A model brain already carries the operating rules in its system prompt and
  -- can't write files, so it isn't handed the file-based protocol; it waits for
  -- the human's goal in the chat box. A CLI agent gets the file-handoff brief.
  if not tab.is_model then
    shikisha.send_to_tab(tab.index, table.concat({
      protocol(run),
      "",
      shikisha.t("agent.browser.start.ready"),
    }, "\n"))
  end
end

function on_done(tab)
  local ai = tab.index
  local run = shikisha.get_var("rally_run")
  if not run then return end
  local infile = run .. "/in.lua"
  -- A brain hands its move over inside its reply; a CLI agent writes files, so
  -- its reply text is on screen (tab.output). Use whichever carries the move.
  local said = tab.reply or tab.output or ""

  -- A human typed into the input field (chain 0) = a new goal/correction. Reset the budget (safety net)
  if tab.chain_depth == 0 then
    reset_budget()
    shikisha.set_var("rally_nocode", 0)
  end
  shikisha.set_var("rally_tok", (shikisha.get_var("rally_tok") or 0) + #said)

  -- Human-assistance-request file
  local human = shikisha.exchange_take(run .. "/human.txt")
  if human and #human > 0 then
    tx("\n### " .. shikisha.t("transcript.rally.human_request") .. "\n" .. human .. "\n")
    -- The person may be away from the machine: ring the primary
    -- notification, with the phone board's URL when remote is on, so they
    -- can come and do their part (login, CAPTCHA, …)
    local note = shikisha.tf("agent.browser.human.notify", { text = human })
    local url = shikisha.remote_url()
    if url then note = note .. "\n" .. url end
    shikisha.notify(note)
    shikisha.show(BR)
    -- A notified human needs time to get here — wait up to 30 minutes,
    -- not the 5-minute default
    local why = shikisha.browser_wait(BR, {
      ask = human, label = shikisha.t("agent.browser.human.label"), timeout_ms = 1800000,
    })
    if why == "timeout" then
      tx(shikisha.t("transcript.rally.human_timeout") .. "\n")
      back_to_ai(ai, shikisha.t("agent.browser.human.timeout") .. "\n" .. next_hint(tab, infile))
      return
    end
    tx(shikisha.t("transcript.rally.human_done") .. "\n")
    back_to_ai(ai, shikisha.t("agent.browser.human.resumed_before") .. infile .. shikisha.t("agent.browser.human.resumed_after"))
    return
  end

  -- Move: a CLI agent overwrites in.lua; a model brain returns a ```lua block
  -- in its reply, which we pull out here. Either way it lands as `code` and the
  -- rest of the pipeline (lint -> execute -> record -> judge) is shared.
  local code = shikisha.exchange_take(infile)
  if (not code or #code == 0) and tab.is_model then
    code = extract_lua(tab.reply)
  end
  if code and #code > 0 then
    local lint = shikisha.lint(code)
    if lint then
      back_to_ai(ai, shikisha.t("agent.browser.lint.error") .. "\n" .. lint .. "\n" .. fix_hint(tab, infile))
      return
    end
    -- Brake: optionally hold for a person to approve this move before it runs.
    if not brake_ok(code) then
      back_to_ai(ai, shikisha.t("agent.brake.declined") .. "\n" .. next_hint(tab, infile))
      return
    end
    shikisha.show(BR)
    local err, out = shikisha.run_scoped(BR, code)
    -- Ops that ran before an error still happened; the replay keeps them.
    -- These are the durable spellings (anchors, not refs) journaled per op
    local rl = shikisha.take_replay()
    if rl and #rl > 0 then
      pcall(shikisha.exchange_append, shikisha.get_var("rally_run") .. "/replay.lua", table.concat(rl, "\n") .. "\n")
    end
    if err then
      back_to_ai(ai, shikisha.t("agent.browser.run.error") .. "\n" .. err .. "\n" .. retry_hint(tab, infile))
      return
    end
    shikisha.exchange_append(shikisha.get_var("rally_record"), code)
    shikisha.set_var("rally_nocode", 0)
    local n = (shikisha.get_var("rally_round") or 0) + 1
    shikisha.set_var("rally_round", n)
    -- Record the executed move in the human-readable transcript (4-space indent = Markdown code block)
    tx("\n### " .. shikisha.t("transcript.rally.action") .. " " .. n .. "\n    " .. code:gsub("\n", "\n    ") .. "\n")
    -- Settle: wait until the page's body text stops changing (stable across two
    -- reads) or SETTLE_MS elapses, watching stop conditions meanwhile. Reading a
    -- half-rendered page would otherwise feed the operator a partial screen. The
    -- loop is skipped entirely when SETTLE_MS = 0.
    local v = nil
    local prev = nil
    local waited = 0
    while waited < SETTLE_MS do
      shikisha.sleep(180)
      waited = waited + 180
      local t = shikisha.browser_text(BR, "body")
      if t and #(t:gsub("%s", "")) > 0 then
        v = judge(said)
        if v then break end
        if prev and t == prev then break end   -- unchanged => settled
        prev = t
      end
    end
    if not v then v = judge(said) end   -- evaluate stops even when settle was off/short
    local body0 = shikisha.browser_text(BR, "body") or ""
    tx("- " .. shikisha.t("transcript.rally.screen") .. ": " .. (clip(body0, 400):gsub("%s+", " ")) .. "\n")
    if out and #out > 0 then
      tx("- " .. shikisha.t("transcript.rally.result") .. ": " .. (clip(out, 400):gsub("%s+", " ")) .. "\n")
    end
    -- The judge (configured stop conditions). Once satisfied, emit an exit code and pause (back to waiting)
    if v then
      tx("\n## " .. shikisha.t("agent.verdict.label") .. ": " .. (v.outcome == "success" and shikisha.t("agent.verdict.success") or shikisha.t("agent.verdict.fail"))
        .. " (code=" .. (v.code or 0) .. ")\n" .. (v.reason or "") .. "\n")
      shikisha.show(v.outcome == "success" and ai or BR)
      shikisha.set_result(v.code or 0, v.reason or v.outcome)
      shikisha.open_result(run)
      shikisha.send_to_tab(ai, shikisha.t("agent.verdict.label") .. ": " .. (v.reason or v.outcome)
        .. " (code=" .. (v.code or 0) .. ")" .. shikisha.t("agent.browser.next_instruction"))
      return
    end
    -- Safety net (runaway insurance). Each limit is off when set to 0. When one
    -- is hit, ON_LIMIT decides: "continue" resets the budget and carries on
    -- (never stop on the user; the operator still judges DONE), anything else
    -- ("stop") halts and hands back to the human.
    local t0 = shikisha.get_var("rally_t0") or shikisha.epoch_ms()
    local over = (MAX_ROUNDS > 0 and n >= MAX_ROUNDS)
      or (MAX_SEC > 0 and (shikisha.epoch_ms() - t0) >= MAX_SEC * 1000)
      or (MAX_TOK > 0 and (shikisha.get_var("rally_tok") or 0) >= MAX_TOK)
    if over then
      if ON_LIMIT == "continue" then
        reset_budget()
      else
        back_to_ai(ai, shikisha.t("agent.browser.safety_net"))
        return
      end
    end
    -- Return what the move gave back, plus the screen, and prompt for the
    -- next move. The full, untruncated screen is staged to a file each round:
    -- a CLI operator reads it there (no truncation), while a model brain —
    -- which can't read files — gets it inline (capped). A long return value
    -- gets the same file treatment (out.txt) so a digest never floods the chat
    shikisha.show(ai)
    local text = shikisha.browser_text(BR, "body") or ""
    local screenfile = run .. "/screen.txt"
    pcall(shikisha.exchange_write, screenfile, text)
    -- A fresh digest every round, taken after the settle so it reflects the
    -- page as it now stands. The operator never needs to spend a move on
    -- browser_digest: the numbered element list is simply always current
    local okd, dg = pcall(shikisha.browser_digest, BR)
    if not okd or type(dg) ~= "string" then dg = nil end
    local digestfile = run .. "/digest.txt"
    if dg then pcall(shikisha.exchange_write, digestfile, dg) end
    local outline = nil
    if out and #out > 0 then
      if not tab.is_model and #out > 1500 then
        local outfile = run .. "/out.txt"
        pcall(shikisha.exchange_write, outfile, out)
        outline = shikisha.tf("agent.browser.result_file", { file = outfile }) .. "\n" .. clip(out, 700)
      elseif tab.is_model and #out > 3000 then
        outline = shikisha.t("agent.browser.result") .. "\n" .. clip(out, 3000) .. shikisha.t("agent.browser.truncated")
      else
        outline = shikisha.t("agent.browser.result") .. "\n" .. out
      end
    end
    local msg = {}
    if outline then msg[#msg + 1] = outline end
    if tab.is_model then
      local inline = text
      if #inline > 3000 then inline = clip(inline, 3000) .. shikisha.t("agent.browser.truncated") end
      msg[#msg + 1] = shikisha.t("agent.browser.executed_screen")
      msg[#msg + 1] = "----"
      msg[#msg + 1] = inline
      msg[#msg + 1] = "----"
      if dg then
        -- A model brain can't read files: the digest rides inline (capped)
        msg[#msg + 1] = shikisha.t("agent.browser.digest_inline")
        local dgi = dg
        if #dgi > 3500 then dgi = clip(dgi, 3500) .. shikisha.t("agent.browser.truncated") end
        msg[#msg + 1] = dgi
      end
      msg[#msg + 1] = next_hint(tab, infile)
    else
      msg[#msg + 1] = shikisha.tf("agent.browser.executed_file", { file = screenfile })
      msg[#msg + 1] = clip(text, 800)
      if dg then
        msg[#msg + 1] = shikisha.tf("agent.browser.digest_file", { file = digestfile })
        msg[#msg + 1] = clip(dg, 1200)
      end
      msg[#msg + 1] = next_hint(tab, infile)
    end
    shikisha.send_to_tab(ai, table.concat(msg, "\n"))
    return
  end

  -- No runnable move.
  if tab.is_model then
    -- A brain replying with a bare DONE means the goal is met.
    if is_done(said) then
      tx("\n## " .. shikisha.t("agent.verdict.label") .. ": " .. shikisha.t("agent.verdict.success") .. "\n")
      shikisha.set_result(0, shikisha.t("agent.verdict.success"))
      shikisha.open_result(run)
      return
    end
    -- Neither code nor DONE: remind, but cap consecutive empty turns so a
    -- chatty model can't loop forever prompting itself.
    local nc = (shikisha.get_var("rally_nocode") or 0) + 1
    shikisha.set_var("rally_nocode", nc)
    if nc >= 3 then
      back_to_ai(ai, shikisha.t("agent.browser.model.stuck"))
      return
    end
    back_to_ai(ai, shikisha.t("agent.browser.model.remind"))
    return
  end

  -- CLI no-code: if a human just typed the goal (chain 0), nudge once.
  if tab.chain_depth == 0 then
    back_to_ai(ai,
      shikisha.t("agent.browser.first_action.before") .. infile .. shikisha.t("agent.browser.first_action.after"))
    return
  end
  -- Mid-rally, a turn with no move is usually the AI narrating ("I wrote
  -- the move") without actually writing the file this turn — left silent,
  -- both sides wait on each other forever. Remind a couple of times (the
  -- counter resets whenever a move actually runs), then go quiet so an AI
  -- that genuinely finished and reported isn't pestered endlessly
  local nc = (shikisha.get_var("rally_nocode") or 0) + 1
  shikisha.set_var("rally_nocode", nc)
  if nc <= 2 then
    back_to_ai(ai, next_hint(tab, infile))
  end
end
"##;
        let src = format!(
            "local BR = {browser:?}\nlocal STOPS = {stops_lua}\n\
             local MAX_ROUNDS, MAX_SEC, MAX_TOK = {}, {}, {}\nlocal ON_LIMIT = {:?}\n\
             local SETTLE_MS = {}\nlocal CONFIRM = {:?}\n{SRC}",
            op.max_rounds, op.max_seconds, op.max_tokens, op.on_limit, op.settle_ms, op.confirm
        );
        self.load_source(&key, &src)
    }

    /// Attach the built-in "operate another AI tab" orchestrator to the operator.
    /// The operator writes one instruction per turn (to in.txt, or inline for a
    /// model brain); it's relayed to the target AI `target`, whose reply is read
    /// back and handed to the operator, until the operator replies DONE. Shares the
    /// operate limits/policy with the browser agent. Cached per (target, limits).
    pub fn load_ai_agent(&mut self, target: &str) -> Result<usize> {
        let op = crate::config::operate();
        let key = format!(
            "<ai-agent:{target}>|{}|{}|{}|{}",
            op.max_rounds, op.max_seconds, op.max_tokens, op.on_limit
        );
        if let Some(i) = self.scripts.iter().position(|s| s.path == key) {
            return Ok(i);
        }
        // TARGET (the AI tab being driven) and the limits are injected below.
        const SRC: &str = r##"
local function is_done(reply)
  if not reply then return false end
  for line in (reply .. "\n"):gmatch("(.-)\n") do
    local w = line:gsub("[%s%p]", "")
    if w == "DONE" or w == "done" then return true end
  end
  return false
end
local function reset_budget()
  shikisha.set_var("op_round", 0)
  shikisha.set_var("op_t0", shikisha.epoch_ms())
  shikisha.set_var("op_tok", 0)
end
-- Hand the turn back to the AI, with the screen on it. These two go together on
-- every path here: the AI is about to work and watching it is the whole point.
-- Passing work no longer moves the screen by itself, so asking for it is the job
-- of whoever wants to be watched
local function back_to_ai(ai, msg)
  shikisha.show(ai)
  shikisha.send_to_tab(ai, msg)
end

local function tx(entry)
  local p = shikisha.get_var("op_tx")
  if p then shikisha.exchange_append(p, entry) end
end

function on_start(tab)
  local run = shikisha.exchange_new()
  shikisha.set_var("op_run", run)
  shikisha.set_var("op_tx", run .. "/transcript.md")
  shikisha.set_var("op_nocode", 0)
  reset_budget()
  tx(shikisha.tf("transcript.ai.header", { target = TARGET }) .. "\n")
  -- A model brain carries its rules in the system prompt and can't write files,
  -- so it just waits for the human's goal. A CLI gets the file-handoff brief.
  if not tab.is_model then
    shikisha.send_to_tab(tab.index,
      shikisha.tf("agent.ai.brief", { target = TARGET, infile = run .. "/in.txt" }))
  end
end

function on_done(tab)
  local ai = tab.index
  local run = shikisha.get_var("op_run")
  if not run then return end
  local infile = run .. "/in.txt"
  local said = tab.reply or tab.output or ""
  -- A human typed into the input (chain 0) = a fresh goal. Reset the safety budget.
  if tab.chain_depth == 0 then reset_budget(); shikisha.set_var("op_nocode", 0) end
  shikisha.set_var("op_tok", (shikisha.get_var("op_tok") or 0) + #said)

  -- The operator's next instruction: a CLI writes in.txt; a model replies inline.
  local instr = shikisha.exchange_take(infile)
  if (not instr or #instr == 0) and tab.is_model and not is_done(said) then
    instr = said
  end
  if instr and #(instr:gsub("%s", "")) > 0 and not is_done(instr) then
    shikisha.set_var("op_nocode", 0)
    local n = (shikisha.get_var("op_round") or 0) + 1
    shikisha.set_var("op_round", n)
    tx("\n### " .. shikisha.t("transcript.ai.instruction") .. " " .. n .. "\n" .. instr .. "\n")
    -- Relay to the target AI and wait for its reply.
    shikisha.show(TARGET)
    shikisha.send_to_tab(TARGET, instr)
    shikisha.sleep(1500)                          -- let the target begin
    shikisha.wait_state(TARGET, "DONE", 300000)   -- ...then finish
    local reply = ""
    for _ = 1, 10 do
      reply = shikisha.tab_output(TARGET) or ""
      if #(reply:gsub("%s", "")) > 0 then break end
      shikisha.sleep(300)
    end
    tx("- " .. shikisha.t("transcript.ai.reply") .. ": " .. reply:sub(1, 400):gsub("%s+", " ") .. "\n")
    -- Safety net (same policy as the browser agent).
    local t0 = shikisha.get_var("op_t0") or shikisha.epoch_ms()
    local over = (MAX_ROUNDS > 0 and n >= MAX_ROUNDS)
      or (MAX_SEC > 0 and (shikisha.epoch_ms() - t0) >= MAX_SEC * 1000)
      or (MAX_TOK > 0 and (shikisha.get_var("op_tok") or 0) >= MAX_TOK)
    if over then
      if ON_LIMIT == "continue" then
        reset_budget()
      else
        back_to_ai(ai, shikisha.t("agent.browser.safety_net"))
        return
      end
    end
    back_to_ai(ai, shikisha.tf("agent.ai.replied",
      { target = TARGET, reply = reply, infile = infile }))
    return
  end

  -- No instruction.
  if is_done(said) then
    tx("\n## " .. shikisha.t("agent.verdict.label") .. ": " .. shikisha.t("agent.verdict.success") .. "\n")
    shikisha.set_result(0, shikisha.t("agent.verdict.success"))
    shikisha.open_result(run)
    return
  end
  if tab.is_model then
    local nc = (shikisha.get_var("op_nocode") or 0) + 1
    shikisha.set_var("op_nocode", nc)
    if nc >= 3 then back_to_ai(ai, shikisha.t("agent.browser.model.stuck")) return end
    back_to_ai(ai, shikisha.t("agent.browser.model.remind"))
    return
  end
  -- CLI no-code: nudge once right after the goal; otherwise wait for the human.
  if tab.chain_depth == 0 then
    back_to_ai(ai, shikisha.tf("agent.ai.first", { infile = infile }))
  end
end
"##;
        let src = format!(
            "local TARGET = {target:?}\n\
             local MAX_ROUNDS, MAX_SEC, MAX_TOK = {}, {}, {}\nlocal ON_LIMIT = {:?}\n{SRC}",
            op.max_rounds, op.max_seconds, op.max_tokens, op.on_limit
        );
        self.load_source(&key, &src)
    }

    /// Load the AI-vs-AI (N-party) discussion orchestrator for this
    /// participant (chat-style).
    ///
    /// Each turn, every participant **writes** their statement to say.txt
    /// (a file, not the screen). on_done reads it, records it to the
    /// transcript, and hands off to the next participant (round-robin).
    /// Once the round limit is hit, it's handed to the judge, and the
    /// judge's statement becomes the verdict that ends things. The user
    /// never writes Lua. me/next/judge are tab ids; flags and limits are
    /// injected below
    #[allow(clippy::too_many_arguments)]
    pub fn load_discuss_agent(
        &mut self,
        me: &str,
        next: &str,
        is_first: bool,
        is_judge: bool,
        judge: Option<&str>,
        max_turns: usize,
        agents_lua: &str,
        names_lua: &str,
        stops_lua: &str,
        verdict: &str,
        order: &str,
        moderator: Option<&str>,
        is_mod: bool,
        persona: &str,
    ) -> Result<usize> {
        let key = format!(
            "<discuss:{me}:{next}:{is_first}:{is_judge}:{is_mod}:{max_turns}:{verdict}:{order}:{persona}>{stops_lua}"
        );
        if let Some(i) = self.scripts.iter().position(|s| s.path == key) {
            return Ok(i);
        }
        const SRC: &str = r##"
-- Display helpers: routing/keys always use the stable tab id, but anything a
-- human (or another AI) reads shows the display name instead of the id.
local function nm(id)
  local n = NAMES[id]
  if n ~= nil and #n > 0 then return n else return id end
end
local function agent_names()
  local t = {}
  for _, a in ipairs(AGENTS) do t[#t + 1] = nm(a) end
  return t
end

local function ensure_run()
  local r = shikisha.get_var("discuss_run")
  if r then return r end
  r = shikisha.exchange_new()
  shikisha.set_var("discuss_run", r)
  shikisha.set_var("discuss_tx", r .. "/transcript.md")
  shikisha.set_var("discuss_round", 0)
  shikisha.exchange_append(r .. "/transcript.md", shikisha.t("transcript.discuss.header") .. "\n")
  return r
end

local function tx(entry)
  shikisha.exchange_append(shikisha.get_var("discuss_tx"), entry)
end

-- Aggregate stops (automatic judging). Evaluated across every
-- participant's latest statement (discuss_says).
-- when="console" + agents="all"/"any"/"majority" + pattern. Returns the
-- condition that matched
local function agg_judge()
  local says = shikisha.get_var("discuss_says") or {}
  for _, s in ipairs(STOPS or {}) do
    if s.when == "console" and s.agents and s.pattern then
      local hit, total = 0, 0
      for _, ag in ipairs(AGENTS) do
        total = total + 1
        local st = says[ag]
        if st and st:find(s.pattern, 1, true) then hit = hit + 1 end
      end
      local ok = false
      if s.agents == "all" then ok = (total > 0 and hit == total)
      elseif s.agents == "any" then ok = (hit >= 1)
      elseif s.agents == "majority" then ok = (hit * 2 > total) end
      if ok then return s end
    end
  end
  return nil
end

function on_start(tab)
  local run = ensure_run()
  -- A model bridge is stateless: it answers whatever prompt it is handed
  -- straight away, so it cannot hold an intro the way a CLI waits at its
  -- prompt. Briefing it at startup would make it speak before any topic
  -- exists. Its persona rides on every turn's system message instead, so it
  -- needs no startup briefing -- stay silent until its turn actually arrives.
  -- (ensure_run() still runs above so the shared run exists even if the
  -- opening speaker happens to be a model, which self-kicks from on_done.)
  if tab.is_model then return end
  local say = run .. "/say.txt"
  local lines
  if IS_MOD then
    lines = {
      shikisha.tf("agent.discuss.mod.intro", { me = nm(ME) }),
      shikisha.t("agent.discuss.mod.nominate"),
      "  " .. say,
      shikisha.t("agent.discuss.mod.write_to") .. table.concat(agent_names(), ", "),
      shikisha.t("agent.discuss.mod.wait_turn"),
    }
  elseif IS_JUDGE then
    local ask = (MODE == "synthesis")
      and shikisha.t("agent.discuss.judge.ask_synthesis")
      or  shikisha.t("agent.discuss.judge.ask_winner")
    lines = {
      shikisha.tf("agent.discuss.judge.intro", { me = nm(ME) }),
      shikisha.t("agent.discuss.judge.ruling_before") .. ask .. shikisha.t("agent.discuss.judge.ruling_after"),
      "  " .. say,
      shikisha.t("agent.discuss.judge.rubric"),
      shikisha.t("agent.discuss.judge.wait_turn"),
    }
  else
    lines = {
      shikisha.tf("agent.discuss.part.intro", { me = nm(ME) }),
      shikisha.t("agent.discuss.part.each_turn"),
      "  " .. say,
      shikisha.t("agent.discuss.part.others"),
    }
    if IS_FIRST then
      lines[#lines + 1] = ""
      lines[#lines + 1] = shikisha.t("agent.discuss.part.ready")
    else
      lines[#lines + 1] = shikisha.t("agent.discuss.part.wait_turn")
    end
  end
  -- If a persona (stance/character) is set, put it at the very top. Keep to that stance throughout the discussion
  if PERSONA ~= nil and #PERSONA > 0 then
    table.insert(lines, 1, shikisha.t("agent.discuss.persona.label") .. PERSONA)
    table.insert(lines, 2, shikisha.t("agent.discuss.persona.keep"))
    table.insert(lines, 3, "")
  end
  shikisha.send_to_tab(tab.index, table.concat(lines, "\n"))
end

function on_done(tab)
  if shikisha.get_var("discuss_done") then
    -- The discussion has already reached a verdict. Only a fresh human topic on
    -- the opening speaker (chain_depth == 0 means a person just typed) opens a
    -- new round; every other on_done stays finished. Reuse the same run so the
    -- participants keep the say.txt path they were told, and just clear the
    -- per-round state so the next topic starts clean.
    if IS_FIRST and tab.chain_depth == 0 then
      shikisha.set_var("discuss_done", nil)
      shikisha.set_var("discuss_says", nil)
      shikisha.set_var("discuss_round", 0)
      shikisha.set_var("discuss_ask_judge", nil)
      tx("\n---\n")
    else
      return
    end
  end
  local run = shikisha.get_var("discuss_run")
  if not run then return end
  local say = run .. "/say.txt"
  -- When handing off a turn, append say.txt's location in a machine-readable
  -- form at the end. Harmless hint for a CLI AI; the model bridge (ours)
  -- uses this line as the turn signal
  local function speak(pane, msg)
    shikisha.send_to_tab(pane, msg .. "\nSHIKISHA_SAY=" .. say)
  end
  -- Hand the next speaker the record file to READ, never the transcript pasted
  -- inline. The record grows without bound while the prompt must not, and any
  -- inline excerpt would just tempt the AI to skip the fuller context. With
  -- nothing but the file to go on, it has to read to take part.
  local tx_path = shikisha.get_var("discuss_tx")
  local function ctx_ref()
    return shikisha.tf("agent.discuss.read_ctx", { path = tx_path })
  end

  local msg = shikisha.exchange_take(say)
  if not (msg and #msg > 0) then
    -- Hasn't spoken yet. Nudge the opening speaker to start.
    -- For a CLI, this is right after a human typed the topic
    -- (chain_depth==0). A model bridge can't receive human input, so let
    -- it kick off automatically from its persona/context instead (is_model)
    if IS_FIRST and (tab.chain_depth == 0 or tab.is_model) then
      speak(tab.index, shikisha.t("agent.discuss.first.before") .. say .. shikisha.t("agent.discuss.first.after"))
    end
    return
  end

  -- The judge speaks only when it has been formally asked to rule. Until then,
  -- ignore anything it writes (Claude tends to eagerly acknowledge its
  -- instructions into say.txt), so it can't hand down a verdict before the
  -- debate has even taken place.
  if IS_JUDGE and not shikisha.get_var("discuss_ask_judge") then
    return
  end

  -- The moderator's turn: not a statement but a "next speaker or END" instruction. Not recorded as a participant statement
  if IS_MOD then
    if msg:upper():find("END") then
      tx("\n" .. shikisha.tf("transcript.discuss.mod_close", { me = nm(ME) }) .. "\n")
      if JUDGE ~= nil and #JUDGE > 0 then
        shikisha.set_var("discuss_ask_judge", true)
        shikisha.show(JUDGE)
        speak(JUDGE, table.concat({
          ctx_ref(), shikisha.t("agent.discuss.judge_request"),
        }, "\n"))
      else
        shikisha.set_result(0, shikisha.t("agent.discuss.result.mod_closed"))
        shikisha.set_var("discuss_done", true)
        shikisha.open_result(run)
      end
      return
    end
    local pick = nil
    for _, ag in ipairs(AGENTS) do
      if msg:find(ag, 1, true) or msg:find(nm(ag), 1, true) then pick = ag; break end
    end
    pick = pick or AGENTS[1]
    tx("\n" .. shikisha.tf("transcript.discuss.mod_next", { me = nm(ME), pick = nm(pick) }) .. "\n")
    shikisha.show(pick)
    speak(pick, table.concat({
      ctx_ref(),
      shikisha.t("agent.discuss.your_turn.before") .. nm(pick) .. shikisha.t("agent.discuss.your_turn.mid") .. say .. shikisha.t("agent.discuss.your_turn.after"),
    }, "\n"))
    return
  end

  -- Record the statement to the human-readable transcript. That file is also
  -- the full history the next speaker is told to read — there is no second,
  -- pasted copy to keep in sync.
  local r = (shikisha.get_var("discuss_round") or 0) + 1
  shikisha.set_var("discuss_round", r)
  tx("\n### " .. shikisha.tf("transcript.discuss.entry", { me = nm(ME), r = tostring(r) }) .. "\n" .. msg .. "\n")
  -- Keep each participant's latest statement (material for the aggregate stops)
  local says = shikisha.get_var("discuss_says")
  if not says then says = {}; shikisha.set_var("discuss_says", says) end
  says[ME] = msg

  -- The judge's statement = the verdict. Ends here
  if IS_JUDGE then
    tx("\n## " .. shikisha.tf("transcript.discuss.verdict_judge", { me = nm(ME) }) .. "\n" .. msg .. "\n")
    shikisha.show(tab.index)
    shikisha.set_result(0, shikisha.t("agent.discuss.result.judge_ruled"))
    shikisha.set_var("discuss_done", true)
    shikisha.open_result(run)
    return
  end

  -- If an aggregate stop is met (unanimous agreement, anyone objects, majority, etc.), settle it here
  local agg = agg_judge()
  if agg then
    tx("\n## " .. shikisha.t("agent.discuss.verdict_agg") .. ": " .. (agg.outcome == "success" and shikisha.t("agent.verdict.success") or shikisha.t("agent.verdict.fail"))
      .. " (code=" .. (agg.code or 0) .. ")\n" .. (agg.reason or "") .. "\n")
    shikisha.set_result(agg.code or 0, agg.reason or shikisha.t("agent.discuss.agg_met"))
    shikisha.set_var("discuss_done", true)
    shikisha.open_result(run)
    return
  end

  -- Once the round limit is hit, hand off to the judge if there is one, otherwise end
  if r >= MAX_TURNS then
    if JUDGE ~= nil and #JUDGE > 0 then
      shikisha.set_var("discuss_ask_judge", true)
      shikisha.show(JUDGE)
      speak(JUDGE, table.concat({
        ctx_ref(), shikisha.t("agent.discuss.judge_request_final"),
      }, "\n"))
    else
      tx("\n## " .. shikisha.t("agent.verdict.label") .. "\n" .. shikisha.t("agent.discuss.round_limit_no_judge") .. "\n")
      shikisha.set_result(0, shikisha.t("agent.discuss.result.round_limit"))
      shikisha.set_var("discuss_done", true)
      shikisha.open_result(run)
    end
    return
  end

  -- Hand off to the next speaker (also switches the visible screen for
  -- spectating).
  -- If moderated, ask the moderator "who's next"; otherwise (round-robin,
  -- the default) go to the static NEXT
  if ORDER == "moderated" and MODERATOR ~= nil and #MODERATOR > 0 then
    shikisha.show(MODERATOR)
    speak(MODERATOR, table.concat({
      ctx_ref(),
      shikisha.t("agent.discuss.mod.pick_before") .. say
        .. shikisha.t("agent.discuss.mod.pick_after") .. table.concat(agent_names(), ", "),
    }, "\n"))
  else
    shikisha.show(NEXT)
    speak(NEXT, table.concat({
      ctx_ref(),
      shikisha.t("agent.discuss.next_turn.before") .. nm(NEXT) .. shikisha.t("agent.discuss.next_turn.mid") .. say .. shikisha.t("agent.discuss.next_turn.after"),
    }, "\n"))
  end
end
"##;
        let judge_lua = match judge {
            Some(j) => format!("{j:?}"),
            None => "nil".into(),
        };
        let mod_lua = match moderator {
            Some(m) => format!("{m:?}"),
            None => "nil".into(),
        };
        let src = format!(
            "local ME={me:?}\nlocal NEXT={next:?}\nlocal IS_FIRST={is_first}\nlocal IS_JUDGE={is_judge}\nlocal IS_MOD={is_mod}\nlocal JUDGE={judge_lua}\nlocal MODERATOR={mod_lua}\nlocal ORDER={order:?}\nlocal MAX_TURNS={max_turns}\nlocal AGENTS={agents_lua}\nlocal NAMES={names_lua}\nlocal STOPS={stops_lua}\nlocal MODE={verdict:?}\nlocal PERSONA={persona:?}\n{SRC}"
        );
        self.load_source(&key, &src)
    }

    pub fn set_base(&mut self, id: usize) {
        self.attach.base = Some(id);
    }

    pub fn set_workspace(&mut self, id: usize) {
        self.attach.workspace = Some(id);
    }

    /// Attach a script to a tab index (1-based)
    pub fn set_tab(&mut self, tab_index: usize, id: usize) {
        self.attach.tabs.insert(tab_index, id);
    }

    /// Resolve which script is responsible for that hook on that tab
    /// (tab > workspace > base. Never runs more than one)
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

    /// Fire a hook. extra is e.g. the screen text for on_question
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
            self.push_log(crate::i18n::tp("err.hooks.lua_error", &[("hook", hook), ("e", &format!("{e}"))]));
        }
    }

    /// Run a quick-action's Lua on demand, in the same environment automations
    /// use (a fresh scope whose __index is the globals, so the shikisha API and
    /// standard library are available), with `tab` bound to the active tab. Its
    /// commands are collected like a hook's and drained by the caller.
    pub fn fire_action(&mut self, code: &str, ctx: &TabCtx) {
        self.current_origin.set(ctx.index);
        let result = (|| -> mlua::Result<()> {
            let env = self.lua.create_table()?;
            let mt = self.lua.create_table()?;
            mt.set("__index", self.lua.globals())?;
            env.set_metatable(Some(mt))?;
            env.set("tab", self.make_tab_table(ctx)?)?;
            let func = self
                .lua
                .load(code)
                .set_name("action")
                .set_environment(env)
                .into_function()?;
            let thread = self.lua.create_thread(func)?;
            self.resume_thread(thread, "action", ctx.index, MultiValue::from_vec(vec![]));
            Ok(())
        })();
        if let Err(e) = result {
            self.push_log(crate::i18n::tp(
                "err.hooks.lua_error",
                &[("hook", "action"), ("e", &format!("{e}"))],
            ));
        }
    }

    /// Start an ad-hoc "operate a browser" session: attach the built-in browser
    /// agent to `source_pane` (the AI that will drive) targeting browser `browser`,
    /// and run its on_start so the operator is briefed. This is the same script a
    /// configured Agent tab uses; the goal is delivered separately by the caller,
    /// and from then on the normal on_done cycle runs the loop. Idempotent per
    /// browser (the script is cached), with no stop conditions (the AI judges DONE).
    pub fn start_operate(&mut self, source_pane: usize, browser: &str, ctx: &TabCtx) -> Result<()> {
        let stops = crate::config::stops_to_lua(&[]);
        let id = self.load_browser_agent(browser, &stops)?;
        self.set_tab(source_pane, id);
        crate::append_hook_log(&format!(
            "operate: briefing operator pane{source_pane} on browser {browser:?}"
        ));
        self.fire("on_start", ctx, None);
        Ok(())
    }

    /// Start an ad-hoc "operate another AI tab" session: attach the AI-operate
    /// agent to `source_pane` (the driver), targeting the AI tab `target` (its
    /// id/name), and brief the operator. The goal is delivered separately.
    pub fn start_operate_ai(&mut self, source_pane: usize, target: &str, ctx: &TabCtx) -> Result<()> {
        let id = self.load_ai_agent(target)?;
        self.set_tab(source_pane, id);
        crate::append_hook_log(&format!(
            "operate: briefing operator pane{source_pane} on AI tab {target:?}"
        ));
        self.fire("on_start", ctx, None);
        Ok(())
    }

    /// Detach an ad-hoc operate: the source tab goes back to being a plain tab
    /// (its on_done no longer runs the browser loop).
    pub fn stop_operate(&mut self, source_pane: usize) {
        self.attach.tabs.remove(&source_pane);
    }

    /// Hand a goal to the operator, queued as a command like on_start's brief —
    /// so it arrives after the protocol (not before it, which a raw write would).
    pub fn deliver_goal(&self, pane: usize, text: &str) {
        self.commands.borrow_mut().push(Command::SendPrompt {
            target: TabRef::Index(pane),
            text: text.to_string(),
            origin: self.current_origin.get(),
        });
    }

    /// Call a browser hook.
    ///
    /// What's passed is a page, not a tab. A page has no state and no
    /// output. Faking those fields to make it look like a tab would let
    /// the person writing it confuse it with something else.
    /// Whether that page has a resolution target for that hook.
    ///
    /// A control whose press has nowhere to land just looks like a broken
    /// button. This should be knowable before it's shown, or at the
    /// moment it's pressed
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
            self.push_log(crate::i18n::tp("err.hooks.lua_error", &[("hook", hook), ("e", &format!("{e}"))]));
        }
    }

    /// Called on every detection tick. Evaluates the condition for coroutines waiting on wait/sleep and resumes them
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
                        Err(e) => self.push_log(crate::i18n::tp(
                            "err.hooks.lua_resume",
                            &[("e", &format!("{e}"))],
                        )),
                    }
                    let _ = self.lua.remove_registry_value(p.key);
                }
                None => self.pending.push(p),
            }
        }
    }

    /// Drain the operation requests hooks have queued (executed by main)
    pub fn drain_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut *self.commands.borrow_mut())
    }

    fn resume_thread(&mut self, thread: Thread, hook: &str, origin: usize, args: MultiValue) {
        let _budget = self.budget.arm();
        match thread.resume::<MultiValue>(args) {
            Ok(vals) => {
                if thread.status() == ThreadStatus::Resumable {
                    // yield: a wait/sleep wait request
                    match self.parse_yield(&vals) {
                        Ok(wait) => match self.lua.create_registry_value(thread) {
                            Ok(key) => self.pending.push(Pending {
                                key,
                                hook: hook.to_string(),
                                origin,
                                wait,
                            }),
                            Err(e) => self.push_log(crate::i18n::tp(
                                "err.hooks.lua_register",
                                &[("e", &format!("{e}"))],
                            )),
                        },
                        Err(e) => self.push_log(crate::i18n::tp(
                            "err.hooks.lua_yield_bad",
                            &[("hook", hook), ("e", &format!("{e}"))],
                        )),
                    }
                } else {
                    self.on_complete(hook, origin, vals);
                }
            }
            Err(e) => self.push_log(crate::i18n::tp("err.hooks.lua_error", &[("hook", hook), ("e", &format!("{e}"))])),
        }
    }

    /// Handle a hook's return value on completion: if on_question returns a string, send it as auto-reply keys
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
            anyhow::bail!(crate::i18n::t("err.hooks.yield_only"));
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
                        .with_context(|| crate::i18n::tp("err.hooks.wait_regex", &[("pattern", &pattern)]))?,
                    deadline: Instant::now() + Duration::from_millis(timeout_ms),
                })
            }
            other => anyhow::bail!(crate::i18n::tp("err.hooks.unknown_yield", &[("other", other)])),
        }
    }

    fn make_tab_table(&self, ctx: &TabCtx) -> mlua::Result<Table> {
        let t = self.lua.create_table()?;
        t.set("index", ctx.index)?;
        t.set("name", ctx.name.as_str())?;
        // Left unset (nil in Lua) rather than "" when the tab has no id: an empty
        // string would quietly equal another id-less tab's
        if let Some(id) = ctx.id.as_deref() {
            t.set("id", id)?;
        }
        t.set("state", ctx.state.as_str())?;
        t.set("profile", ctx.profile.as_str())?;
        t.set("output", ctx.output.as_str())?;
        t.set("chain_depth", ctx.chain_depth)?;
        t.set("locked", ctx.locked)?;
        t.set("is_model", ctx.is_model)?;
        // Only a brain sets this; leave it nil otherwise so `tab.reply or ""`
        // reads cleanly in Lua.
        if let Some(r) = &ctx.reply {
            t.set("reply", r.as_str())?;
        }
        Ok(t)
    }

    fn push_log(&self, msg: String) {
        self.commands.borrow_mut().push(Command::Log(msg));
    }
}

/// Accept a tab spec: an index, a tab name, or a tab table all work
fn tab_ref_of(v: &Value) -> mlua::Result<TabRef> {
    match v {
        Value::Integer(n) => Ok(TabRef::Index(*n as usize)),
        Value::Number(n) => Ok(TabRef::Index(*n as usize)),
        Value::String(s) => Ok(TabRef::Name(s.to_str()?.to_string())),
        Value::Table(t) => Ok(TabRef::Index(t.get("index")?)),
        _ => Err(mlua::Error::runtime(crate::i18n::t("err.hooks.tab_ref"))),
    }
}

#[cfg(test)]
mod stamp_tests {
    /// The date/time must be in a form that sorts chronologically.
    ///
    /// Used in file names, so shifting digit widths would break the
    /// ordering. Lua isn't given os, so this is the only source of it.
    /// It must be returned exactly as the given format specifies
    #[test]
    fn the_shape_is_the_caller_s_to_choose() {
        let ymd = super::local_stamp("%Y-%m-%d");
        assert_eq!(ymd.len(), 10, "{ymd}");
        assert_eq!(&ymd[4..5], "-");
        assert_eq!(&ymd[7..8], "-");
        assert_eq!(super::local_stamp("%y").len(), 2);
        // Anything that isn't a directive passes through unchanged
        assert_eq!(super::local_stamp("報告_%%.html"), "報告_%.html");
        // Unknown directives are kept. Silently dropping them would hide the fact that they disappeared
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

    /// Tests touching data/last-rally.lua share the same file, so serialize them
    static RALLY_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    /// The tab a hook receives must carry its automation name.
    ///
    /// It is the only handle that survives a rename — the number shifts when tabs
    /// are reordered and the display name is the very thing a person edits — yet
    /// it was the one thing `tab` did not carry, so `if tab.id == "reviewer"` was
    /// silently always false. Unset stays nil rather than "", or two tabs without
    /// one would compare equal.
    #[test]
    fn a_hook_can_tell_which_tab_it_is_on() {
        let eng = super::HookEngine::new().expect("engine");
        let named = eng
            .make_tab_table(&TabCtx { id: Some("reviewer".into()), ..ctx(2, "") })
            .expect("table");
        assert_eq!(named.get::<String>("id").ok().as_deref(), Some("reviewer"));

        let plain = eng
            .make_tab_table(&TabCtx { id: None, ..ctx(3, "") })
            .expect("table");
        assert!(
            plain.get::<mlua::Value>("id").map(|v| v.is_nil()).unwrap_or(false),
            "id が無いタブは nil であるべき (空文字だと id無し同士が一致してしまう)"
        );
    }

    /// Every command automation can call must be in both manuals.
    ///
    /// The manuals are not just for people: the settings screen hands them to an
    /// AI as the specification when it writes automation. A command missing from
    /// them does not exist as far as that AI is concerned — which is how half of
    /// them, `show` included, ended up unreachable. English is the base and
    /// Japanese is laid over it, so both have to carry the same list.
    #[test]
    fn every_command_is_in_both_manuals() {
        // Every command automation can call, read off this file. Line by line on
        // purpose: a pattern spanning lines would carry whatever ending the
        // checkout used, and a CRLF checkout then finds 21 of the 55 — which is
        // exactly how this passed here and failed on CI.
        let collect = |text: &str| {
            let mut names: Vec<String> = Vec::new();
            let mut add = |n: &str| {
                let ok = !n.is_empty()
                    && n.len() < 32
                    && n.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit());
                if ok && !names.iter().any(|x| x == n) {
                    names.push(n.to_string());
                }
            };
            let rows: Vec<&str> = text.lines().map(str::trim).collect();
            for (i, row) in rows.iter().enumerate() {
                // `.set(` on the line above, the name alone on this one
                if i > 0 && rows[i - 1].ends_with(".set(") {
                    if let Some(n) = row.strip_prefix("\"").and_then(|r| r.strip_suffix("\",")) {
                        add(n);
                    }
                }
                // ...or all on one line
                for open in ["bind!(\"", "function shikisha."] {
                    if let Some(rest) = row.strip_prefix(open) {
                        let close = if open.starts_with("bind") { "\"" } else { "(" };
                        if let Some(n) = rest.split(close).next() {
                            add(n);
                        }
                    }
                }
            }
            names
        };
        let src = include_str!("hooks.rs");
        let names = collect(src);
        assert!(names.len() > 40, "命令の抽出に失敗している ({} 件)", names.len());

        // The `tab` table an event receives, built in tab_table below
        let mut fields: Vec<&str> = Vec::new();
        for part in src.split("t.set(\"").skip(1) {
            if let Some(f) = part.split('"').next() {
                // __index is the metatable link that lets a hook see the globals,
                // not something anyone reads off `tab`
                if !f.is_empty() && f != "__index" && !fields.contains(&f) {
                    fields.push(f);
                }
            }
        }
        assert!(fields.len() > 5, "tab の項目の抽出に失敗している");

        for (doc, text) in [
            ("AUTOMATION.md", include_str!("../docs/AUTOMATION.md")),
            ("AUTOMATION.ja.md", include_str!("../docs/AUTOMATION.ja.md")),
        ] {
            let missing: Vec<&String> = names
                .iter()
                .filter(|n| !text.contains(&format!("shikisha.{n}")))
                .collect();
            assert!(
                missing.is_empty(),
                "{doc} に載っていない命令: {missing:?} (AIに渡す仕様書なので、無い＝使えない)"
            );
            let no_var: Vec<&&str> = fields
                .iter()
                .filter(|f| !text.contains(&format!("tab.{f}")))
                .collect();
            assert!(no_var.is_empty(), "{doc} に載っていない tab の項目: {no_var:?}");
        }
    }

    /// Every built-in orchestrator must actually compile.
    ///
    /// The user never writes these — they are the app's own Lua, edited in Rust
    /// string literals where no editor checks them. A typo, or a helper called in
    /// one template but defined only in another, would surface as an orchestrator
    /// that silently does nothing on the first real rally.
    #[test]
    fn the_built_in_orchestrators_compile() {
        let caps: crate::hooks::Caps = std::rc::Rc::new(crate::caps::Capabilities::new(
            Default::default(),
            std::path::PathBuf::from("."),
            std::collections::HashMap::new(),
        ));
        let mut eng = super::HookEngine::with_caps(caps).expect("engine");
        eng.load_browser_agent("BR", "{}").expect("browser rally template");
        eng.load_ai_agent("target").expect("operate template");

        // ...and each must define every local helper it calls. Templates are
        // separate Lua worlds, so a helper borrowed from the one above compiles
        // fine and blows up only when that path is actually walked — which, for
        // a branch like "the AI got stuck", can be weeks later
        let file = include_str!("hooks.rs");
        let mut checked = 0;
        for chunk in file.split("const SRC: &str = r##\"").skip(1) {
            let body = chunk.split("\"##;").next().unwrap_or("");
            for helper in ["back_to_ai", "next_hint", "fix_hint", "retry_hint", "clip"] {
                if !body.contains(&format!("{helper}(")) {
                    continue;
                }
                assert!(
                    body.contains(&format!("local function {helper}(")),
                    "テンプレートが {helper} を呼んでいるのに、そのテンプレート内に定義が無い"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "テンプレートを1つも見ていない (目印が変わった?)");
    }

    fn lint_lua_flags_broken_syntax_only() {
        // Sound code parses (nil), including calls to names that won't exist at run
        // time — lint is syntax-only, so undefined-name misuse isn't its job.
        assert!(super::lint_lua("local x = 1 return x + 1").is_none());
        assert!(super::lint_lua("shikisha.draft_to_tab(tab.index, 'hi')").is_none());
        assert!(super::lint_lua("os.date('%Y')").is_none());
        // Broken syntax is reported.
        assert!(super::lint_lua("if true then").is_some());
        assert!(super::lint_lua("local = = 3").is_some());
        assert!(super::lint_lua("send_to_tab(tab,").is_some());
    }

    fn ctx(index: usize, output: &str) -> TabCtx {
        TabCtx {
            index,
            name: format!("tab{index}"),
            id: Some(format!("id{index}")),
            state: "DONE".into(),
            profile: "test".into(),
            is_model: false,
            output: output.into(),
            chain_depth: 0,
            locked: false,
            reply: None,
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

        // Condition not met -> stays pending
        e.tick_pending(&|_| Some("loading...".to_string()));
        assert!(e.drain_commands().is_empty());

        // Prompt appeared -> resumes and executes send
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
        // Even after reordering, the same name still resolves to the correct tab
        let key = |n: &str| TabKey { id: None, name: n.to_string() };
        assert_eq!(target.resolve(&[key("実装"), key("検査")]), Some(2));
        assert_eq!(target.resolve(&[key("検査"), key("実装")]), Some(1));
        // A nonexistent name can't resolve (avoids false hits)
        assert_eq!(target.resolve(&[key("別名")]), None);
    }

    #[test]
    fn show_switches_the_visible_tab_for_spectating() {
        // Spectator mode: moves the visible screen on each turn. The target can be a browser or the INDEX
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
        // A name resolves to the screen's index (sessions and browsers are listed together)
        let key = |n: &str| TabKey { id: None, name: n.to_string() };
        assert_eq!(target.resolve(&[key("AI"), key("ブラウザ")]), Some(2));
        // 0 is the dashboard (INDEX). resolve doesn't catch it; main handles it specially
        assert!(
            matches!(&cmds[1], Command::ShowTab { target: TabRef::Index(0) }),
            "show(0) は INDEX"
        );
    }

    #[test]
    fn fetch_result_json_becomes_a_lua_table() {
        // browser_fetch maps its JSON-string result into a Lua table before returning it.
        // status/ok/body/headers/arrays must all be readable directly
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
        // When the orchestrator appends executed Lua, what's left behind must be pasteable to replay
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
        // The built-in browser-operation mode must load, and on_start must send the "operation protocol".
        // Since the goal is given via the input field rather than config, the prompt needs to explain the input field
        let mut e = HookEngine::new().unwrap();
        // Configured stop conditions (the judge) must still be readable once injected into Lua
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
                        && text.contains("input field"))),
            "on_start がブラウザ操作プロトコル(入力欄でゴール)を送っていない: {cmds:?}"
        );
    }

    #[test]
    fn ad_hoc_operate_attaches_the_agent_and_detaches() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        let mut e = HookEngine::new().unwrap();
        // Start operating browser "br" from the tab in pane 1 (no config needed).
        e.start_operate(1, "br", &ctx(1, "")).expect("operate should start");
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c, Command::SendPrompt { text, .. } if text.contains("browser_go"))),
            "on_start should brief the operator with the browser protocol: {cmds:?}"
        );
        // After detaching, the tab is plain again: on_done runs nothing for it.
        e.stop_operate(1);
        e.fire("on_done", &ctx(1, "DONE"), None);
        assert!(e.drain_commands().is_empty(), "a detached tab must not run the operate loop");
    }

    #[test]
    fn ad_hoc_ai_operate_briefs_the_operator_about_the_target() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        let mut e = HookEngine::new().unwrap();
        // Drive the AI tab "helper" from pane 1.
        e.start_operate_ai(1, "helper", &ctx(1, "")).expect("ai operate should start");
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c, Command::SendPrompt { text, .. } if text.contains("helper"))),
            "on_start should brief the operator about driving the target AI: {cmds:?}"
        );
        e.stop_operate(1);
        e.fire("on_done", &ctx(1, "DONE"), None);
        assert!(e.drain_commands().is_empty(), "a detached tab must not run the operate loop");
    }

    fn ctx_model(index: usize, reply: &str) -> TabCtx {
        let mut c = ctx(index, "");
        c.is_model = true;
        c.reply = Some(reply.into());
        c
    }

    fn load_brain() -> HookEngine {
        let empty: &[crate::config::StopCond] = &[];
        let stops = crate::config::stops_to_lua(empty);
        let mut e = HookEngine::new().unwrap();
        let id = e.load_browser_agent("br", &stops).expect("内蔵司令塔が読めない");
        e.set_tab(1, id);
        e
    }

    #[test]
    fn a_browser_brain_gets_no_file_protocol_at_start() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // A model brain carries its rules in the system prompt and can't write
        // files, so on_start must NOT hand it the in.lua file-handoff protocol.
        let mut e = load_brain();
        e.fire("on_start", &ctx_model(1, ""), None);
        let cmds = e.drain_commands();
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::SendPrompt { text, .. } if text.contains("in.lua"))),
            "model brain should not receive the file-handoff protocol: {cmds:?}"
        );
    }

    #[test]
    fn a_browser_brain_move_is_pulled_from_its_reply() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // The brain never writes in.lua; the orchestrator must EXTRACT the
        // fenced ```lua from its reply and run it through the same pipeline.
        // A block that isn't valid Lua proves extraction reached the linter
        // (rather than falling through to the "no move" path).
        let mut e = load_brain();
        e.fire("on_start", &ctx_model(1, ""), None);
        let _ = e.drain_commands();
        e.fire(
            "on_done",
            &ctx_model(1, "Sure, next:\n```lua\n=== not lua ===\n```"),
            None,
        );
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c, Command::SendPrompt { text, .. }
                if text.contains(&crate::i18n::t("agent.browser.lint.error")))),
            "the ```lua block should have been extracted and linted: {cmds:?}"
        );
    }

    #[test]
    fn a_browser_brain_finishes_on_a_bare_done() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        let mut e = load_brain();
        e.fire("on_start", &ctx_model(1, ""), None);
        let _ = e.drain_commands();
        e.fire("on_done", &ctx_model(1, "DONE\nPosted the article."), None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::SetResult { code: 0, .. })),
            "a bare DONE should end the rally with success: {cmds:?}"
        );
    }

    #[test]
    fn a_browser_brain_is_reminded_when_it_only_chats() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // A reply with neither a code block nor DONE gets nudged back toward
        // emitting Lua (so a chatty model doesn't silently stall).
        let mut e = load_brain();
        e.fire("on_start", &ctx_model(1, ""), None);
        let _ = e.drain_commands();
        e.fire("on_done", &ctx_model(1, "I think we should log in first."), None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::SendPrompt { text, .. } if text.contains("```lua"))),
            "a chatty no-code reply should be reminded to send lua: {cmds:?}"
        );
    }

    #[test]
    fn discuss_agent_is_built_in_and_first_waits_for_topic() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // The built-in AI-vs-AI discussion orchestrator must load, and the opening
        // speaker (is_first) must prompt for the topic via the input field.
        // The user never writes Lua
        let mut e = HookEngine::new().unwrap();
        let a = e
            .load_discuss_agent(
                "ai1",
                "ai2",
                true,
                false,
                Some("ref"),
                4,
                r#"{"ai1","ai2"}"#,
                "{}",
                "{}",
                "winner",
                "round-robin",
                None,
                false,
                "",
            )
            .expect("議論の内蔵司令塔が読めない");
        e.set_tab(1, a);
        e.fire("on_start", &ctx(1, ""), None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c,
                Command::SendPrompt { text, .. }
                    if text.contains("say.txt") && text.contains("participant") && text.contains("open the discussion"))),
            "口火役の on_start が待機の案内をしていない: {cmds:?}"
        );
    }

    #[test]
    fn set_result_carries_the_exit_code_and_reason() {
        // The exit code and reason the AI judged must be queued as coming from that tab
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
        // run_scoped restricts AI-authored Lua to the browser functions plus a single allowed tab.
        // os/io/load/require, shikisha.write_file/http, and other tabs must all be off limits
        let mut e = HookEngine::from_source(
            r##"
            function on_done(t)
              shikisha.log("os=" .. tostring(shikisha.run_scoped("br", "return os.time()")))
              shikisha.log("write=" .. tostring(shikisha.run_scoped("br", "shikisha.write_file('a','b','c')")))
              shikisha.log("load=" .. tostring(shikisha.run_scoped("br", "load('return 1')")))
              shikisha.log("wrong=" .. tostring(shikisha.run_scoped("br", "shikisha.browser_click('evil', '#x')")))
              shikisha.log("hasclick=" .. tostring(shikisha.run_scoped("br", "assert(type(shikisha.browser_click)=='function')")))
              shikisha.log("haspress=" .. tostring(shikisha.run_scoped("br", "assert(type(shikisha.browser_press)=='function')")))
              shikisha.log("badkey=" .. tostring(shikisha.run_scoped("br", "shikisha.browser_press('br', 'notakey')")))
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
            find("wrong=").contains("Browser not allowed"),
            "他タブを操作できてしまう: {:?}",
            find("wrong=")
        );
        assert_eq!(find("hasclick="), "hasclick=nil", "browser_click は使えるはず");
        assert_eq!(find("haspress="), "haspress=nil", "browser_press は使えるはず");
        assert!(
            find("badkey=").contains("Unknown key"),
            "不正なキー名を弾いていない: {:?}",
            find("badkey=")
        );
    }

    #[test]
    fn replay_spelling_is_durable_and_quoted() {
        use crate::browser::Sel;
        // Values survive quoting untouched (quotes, backslashes, newlines)
        assert_eq!(lua_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        // css/xpath pass through as written
        assert_eq!(sel_replay(&Sel::Css("#x".into()), &None).as_deref(), Some("\"#x\""));
        assert_eq!(
            sel_replay(&Sel::Xpath("//a[@href='x']".into()), &None).as_deref(),
            Some("{xpath=\"//a[@href='x']\"}")
        );
        // a ref becomes the anchor derived from the element it touched
        assert_eq!(
            sel_replay(&Sel::Ref(3), &Some(("css".into(), "#go".into()))).as_deref(),
            Some("\"#go\"")
        );
        assert_eq!(
            sel_replay(
                &Sel::Ref(3),
                &Some(("xpath".into(), "//button[normalize-space()=\"押す\"]".into()))
            )
            .as_deref(),
            Some("{xpath=\"//button[normalize-space()=\\\"押す\\\"]\"}")
        );
        // no durable anchor = nothing to record (the journal notes it instead)
        assert_eq!(sel_replay(&Sel::Ref(3), &None), None);
    }

    #[test]
    fn the_screen_divides_from_lua_and_composes_with_show() {
        // The arrangement this exists for — an agent with the browser it is
        // driving beside it — is two primitives in the order you would say
        // them, not one command that only ever makes that one arrangement
        let mut e = HookEngine::from_source(
            r#"
            function on_done(t)
              shikisha.split_pane("right")
              shikisha.show("br")
              shikisha.split_pane("down")
              shikisha.focus_pane("left")
              shikisha.equalize_panes()
              shikisha.close_pane()
            end
            "#,
        )
        .unwrap();
        e.fire("on_done", &ctx(1, ""), None);
        let got: Vec<String> = e
            .drain_commands()
            .into_iter()
            .map(|c| match c {
                Command::Pane(op) => format!("{op:?}"),
                Command::ShowTab { target } => format!("show {target:?}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            got,
            vec![
                "Split(Row)".to_string(),
                "show Name(\"br\")".to_string(),
                "Split(Col)".to_string(),
                "Focus(Left)".to_string(),
                "Equalize".to_string(),
                "Close".to_string(),
            ]
        );
    }

    #[test]
    fn a_direction_nobody_recognises_is_refused_out_loud() {
        let e = HookEngine::new().unwrap();
        let err = e
            .call_primitive("split_pane", &[serde_json::json!("sideways")])
            .unwrap_err();
        assert!(err.contains("sideways"), "{err}");
        // ...while the words a person would actually reach for are all taken
        for word in ["right", "row", "down", "col"] {
            assert!(
                e.call_primitive("split_pane", &[serde_json::json!(word)]).is_ok(),
                "{word} が通らない"
            );
        }
    }

    #[test]
    fn saying_what_you_are_doing_means_your_own_tab_unless_you_name_one() {
        // The agent in a tab says "I am running tests" and means itself. A
        // build script started by hand is nobody's tab, and has to be able to
        // say which one it is talking about
        let mut e = HookEngine::from_source(
            r#"
            function on_done(t)
              shikisha.set_status("build", "running tests")
              shikisha.set_progress(0.5, "tests")
              shikisha.set_status("build", "done", "other")
              shikisha.set_status("build", "")
            end
            "#,
        )
        .unwrap();
        e.fire("on_done", &ctx(3, ""), None);
        let said: Vec<String> = e
            .drain_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::SetStatus { key, value, target, origin } => {
                    Some(format!("status {key}={value:?} to {target:?}/{origin}"))
                }
                Command::SetProgress { value, target, origin, .. } => {
                    Some(format!("progress {value:?} to {target:?}/{origin}"))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            said,
            vec![
                "status build=\"running tests\" to None/3".to_string(),
                "progress Some(0.5) to None/3".to_string(),
                "status build=\"done\" to Some(Name(\"other\"))/3".to_string(),
                // An empty value is how something finishes: no second verb
                "status build=\"\" to None/3".to_string(),
            ]
        );
    }

    #[test]
    fn the_outside_calls_primitives_by_the_name_lua_uses() {
        // The external API invents no vocabulary of its own: the method name
        // is what comes after `shikisha.` and nothing translates in between
        let e = HookEngine::new().unwrap();
        assert_eq!(
            e.call_primitive("set_var", &[serde_json::json!("round"), serde_json::json!(3)]),
            Ok(serde_json::Value::Null)
        );
        assert_eq!(
            e.call_primitive("get_var", &[serde_json::json!("round")]),
            Ok(serde_json::json!(3))
        );
        // A name that was made up (an earlier sketch of this API said
        // "tab.send") is refused out loud rather than doing nothing
        let err = e.call_primitive("tab.send", &[]).unwrap_err();
        assert!(err.contains("no such primitive"), "{err}");
    }

    #[test]
    fn the_command_list_is_the_table_itself() {
        // Nobody keeps a second list. Whatever Lua has, the outside can call
        let e = HookEngine::new().unwrap();
        let mut names = primitive_names(&e.lua).unwrap();
        names.sort();
        for expected in [
            "send_to_tab", // registered from Rust
            "get_var",     // defined in the Lua prelude
            "run_scoped",  // the walled evaluator
            "lua",         // the full-powered one
        ] {
            assert!(names.contains(&expected.to_string()), "{expected} が一覧に無い");
        }
        assert_eq!(
            e.call_primitive("list", &[]).unwrap(),
            serde_json::to_value(&names).unwrap(),
            "内側から見た一覧と外側から見た一覧は同じもの"
        );
    }

    #[test]
    fn a_chunk_from_outside_loops_and_names_its_own_target() {
        // The point of handing over a whole chunk: branches and loops in one
        // round trip. And with nothing bound to "the tab on screen" — every
        // call inside says who it is talking to, because the caller isn't
        // standing in front of the window
        let mut e = HookEngine::new().unwrap();
        let code = "for i = 1, 3 do shikisha.send_to_tab(i, 'ping ' .. i) end                     shikisha.set_var('sent', 3) return shikisha.get_var('sent')";
        assert_eq!(
            e.call_primitive("lua", &[serde_json::json!(code)]).unwrap(),
            serde_json::json!([null, 3]),
            "エラー無し(nil)に続いて、チャンクが返した値そのもの"
        );
        let targets: Vec<usize> = e
            .drain_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::SendPrompt { target: TabRef::Index(i), .. } => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(targets, vec![1, 2, 3]);

        // A chunk that fails answers with the reason in the first value, in
        // the same shape as a chunk that worked — so one place always says
        // whether it ran, whatever the chunk itself had to say
        let answer = e
            .call_primitive("lua", &[serde_json::json!("error('nope')")])
            .unwrap();
        assert!(answer[0].as_str().unwrap_or_default().contains("nope"), "{answer}");
        assert_eq!(answer[1], serde_json::json!(null));
        // ...and one that ran, but returned nothing, still says so in the same place
        assert_eq!(
            e.call_primitive("lua", &[serde_json::json!("local x = 1")]).unwrap(),
            serde_json::json!([null, null])
        );
    }

    #[test]
    fn a_runaway_loop_is_stopped_instead_of_holding_the_whole_app() {
        // The engine runs on the main loop, so Lua that never returns freezes
        // the window — no keystroke, no redraw, no way out but the task
        // manager. Every door that takes hand-written code (composer ▶, the
        // same ▶ from a phone, the external API) arrives through here
        let e = HookEngine::new().unwrap();
        let err = e
            .run_browser_lua("br", "while true do end")
            .expect("an endless loop has to come back as an error");
        assert!(
            err.contains(&crate::i18n::t("err.hooks.step_limit")),
            "止めた理由が読み手に伝わる文言で返る: {err}"
        );
        // ...and the ceiling belongs to the entry, not to the process: the
        // very next run starts with a full allowance (None = no error)
        assert_eq!(e.run_browser_lua("br", "for _ = 1, 100000 do end"), None);
    }

    #[test]
    fn a_script_that_loops_at_its_top_level_is_stopped_too() {
        // A loop outside any hook would hang the app at startup, before a
        // single hook had a chance to fire
        let err = match HookEngine::from_source("while true do end") {
            Err(e) => e.to_string(),
            Ok(_) => panic!("a top-level endless loop has to be refused"),
        };
        assert!(err.contains(&crate::i18n::t("err.hooks.step_limit")), "{err}");
    }

    #[test]
    fn run_scoped_hands_back_what_the_code_returned() {
        // The operator relays run_scoped's second value to the AI, so a move
        // that returns something must produce it — and a bare expression
        // (REPL-style) must count as returning
        let mut e = HookEngine::from_source(
            r##"
            function on_done(t)
              local err, out = shikisha.run_scoped("br", "return 1 + 1")
              shikisha.log("ret=" .. tostring(err) .. "/" .. tostring(out))
              local err2, out2 = shikisha.run_scoped("br", "('あ') .. ('い')")
              shikisha.log("expr=" .. tostring(err2) .. "/" .. tostring(out2))
              local err3, out3 = shikisha.run_scoped("br", "local x = 1")
              shikisha.log("stmt=" .. tostring(err3) .. "/" .. tostring(out3))
              local err4, out4 = shikisha.run_scoped("br", "return {1, 'a', k='v'}")
              shikisha.log("tbl=" .. tostring(err4) .. "/" .. tostring(out4))
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
        assert_eq!(find("ret="), "ret=nil/2", "returnの値が返る");
        assert_eq!(find("expr="), "expr=nil/あい", "裸の式もREPL式に値になる");
        assert_eq!(find("stmt="), "stmt=nil/nil", "何も返さない文はout=nil");
        let tbl = find("tbl=");
        assert!(
            tbl.starts_with("tbl=nil/{") && tbl.contains('1') && tbl.contains("k=v"),
            "テーブルは中身が見える形で文字列化される: {tbl}"
        );
    }

    #[test]
    fn rally_example_orchestrator_parses_and_runs() {
        let _g = RALLY_FILE_LOCK.lock().unwrap();
        // The template (docs/rally-example) must parse, and the essentials of start and judging must work
        let dir = std::path::Path::new("docs/rally-example");
        let mut e = HookEngine::new().unwrap();
        let id = e.load_path(dir).expect("雛形が読めない (構文エラー?)");
        e.set_base(id);

        // on_start: sends the file-handoff protocol to the AI (has it write to in.lua rather than the screen)
        e.fire("on_start", &ctx(1, ""), None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c,
                Command::SendPrompt { text, .. }
                    if text.contains("in.lua") && text.contains("browser_go"))),
            "on_start がファイル受け渡しのプロトコルを送っていない: {cmds:?}"
        );

        // on_done: the judge's safety net must emit an exit code (deterministically, even with no browser).
        // Blow past the estimated cost (tokens) budget with a huge output and confirm it stops with code=125
        let huge = "x".repeat(300_001);
        let mut c = ctx(1, &huge);
        c.chain_depth = 1;
        e.fire("on_done", &c, None);
        let cmds = e.drain_commands();
        assert!(
            cmds.iter().any(|c| matches!(c, Command::SetResult { code: 125, .. })),
            "審判(tokens上限)が終了コードを出していない: {cmds:?}"
        );

        // Must not auto-react to a conversation a human started (chain_depth=0)
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
        // With an ID attached, a tab can still be addressed even after its name changes
        assert_eq!(r.resolve(&[plain("実装"), with_id("reviewer", "検査")]), Some(2));
        assert_eq!(
            r.resolve(&[plain("実装"), with_id("reviewer", "レビュー担当")]),
            Some(2),
            "タブ名を変えても壊れない"
        );
        // Even with duplicate tab names, the ID still disambiguates
        let dup = [with_id("a", "claude"), with_id("b", "claude")];
        assert_eq!(TabRef::Name("b".into()).resolve(&dup), Some(2));
    }

    #[test]
    fn loop_can_read_live_state_and_exit() {
        // Periodic processing must be expressible as "on start + loop + sleep" instead of an on_tick hook
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
        // The loop keeps going while the state is BUSY
        std::thread::sleep(std::time::Duration::from_millis(1100));
        e.tick_pending(&|_| None);
        // Once the state changes, the loop exits
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
        // Only "the processing body" goes in the file; no function...end wrapper is written
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

        // Tab 2 has a tab-specific script, so it wins
        e.fire("on_done", &ctx(2, ""), None);
        // Tab 1 has no tab-specific script, so the workspace one is used
        e.fire("on_done", &ctx(1, ""), None);
        // If the tab-specific script has no on_exit, fall back to the base one
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


    /// A file name containing a timestamp must be able to pass through without Lua ever having a clock.
    ///
    /// The sandbox has no os (deliberately left out), so Lua can't
    /// construct a date/time itself. The shell is the one with a clock, so
    /// the shell generates the name and Lua reads it back out of its output.
    ///
    /// The name read this way can be used from other scripts too, via set_var
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
        // The same name must be referenceable from a different file too
        let other = e
            .load_source(
                "other",
                r#"function on_done(t) shikisha.log("覚えている: " .. tostring(shikisha.get_var("lp_path"))) end"#,
            )
            .unwrap();
        e.set_tab(1, fetch);
        e.set_tab(2, other);

        // The actual shape the shell outputs (the typed line itself isn't part of the extracted text)
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


    /// Lua must be able to actually drive a real page.
    ///
    ///   cargo test lua_drives_a_real_page -- --ignored --nocapture
    ///
    /// Testing just the intermediate layers can't tell you it's actually
    /// "wired up", so this runs the whole path from a Lua string to a real page
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
