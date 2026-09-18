//! The same primitives, spoken as MCP tools.
//!
//! An AI client that speaks the Model Context Protocol -- Claude Code, and the
//! others -- wants a list of tools and a way to call one. This app already has
//! both: `api` carries a call to a primitive and carries the answer back, and
//! `list` says what the caller of that door may ask for. So nothing here
//! decides anything. It translates one shape of request into the other, and
//! that is the whole of it:
//!
//! ```text
//!   ← {"method":"tools/call","params":{"name":"shikisha_send_to_tab",
//!                                      "arguments":{"params":["reviewer","how is it going?"]}}}
//!   → {"id":"1","method":"send_to_tab","params":["reviewer","how is it going?"]}   (the pipe)
//! ```
//!
//! **Why it stays thin.** A second list of what can be called would answer for
//! the day it was written. `tools/list` asks the running app instead, through
//! the same `list` an AI in a tab uses, so what a client is offered and what
//! the app will actually do can never drift apart -- including the part that
//! depends on who is asking, since a primitive switched off for an AI is not on
//! the list an AI is handed.
//!
//! **Whose call it is.** The token is passed through, never minted here and
//! never read out of `data/api-token` by this module. A server started by a
//! tab's CLI inherits that tab's own key from the environment and arrives
//! identified as that tab, which is what counts its calls against that tab's
//! chain and its automation permissions. A server pointed at an instance from
//! outside carries the key it was given and arrives as "outside the tabs".
//! Laundering one into the other would be handing an AI a way around the
//! brakes, so this module has no code for it.
//!
//! **Which instance.** The environment first (`SHIKISHA_PIPE` /
//! `SHIKISHA_TOKEN`, which is how a tab's children are already told), then
//! `--pipe` / `--pid` and `--token` / `--token-file` for pointing at another
//! copy on purpose. That last part is what lets an AI living in one instance
//! drive a separate one: the copy somebody is working in is not the copy being
//! tested.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use anyhow::{Context as _, Result, anyhow};
use serde_json::{Value, json};

use crate::api::ApiClient;

/// Every tool's name starts with this.
///
/// A client lays the tools of several servers out in one list, and `send`,
/// `list` and `state` are words any of them might use. The prefix is what keeps
/// this app's `send` from being somebody else's.
const PREFIX: &str = "shikisha_";

/// The manual, carried in the binary rather than read from beside it.
///
/// The descriptions a client shows come from the same table a person reads
/// (section 9 of the automation manual), so there is one place to write them.
/// Embedded because a file beside the exe has to be distributed in three
/// places to stay there, and a missing one would leave every tool described as
/// nothing at all.
const MANUAL: &str = include_str!("../../../docs/AUTOMATION.md");

/// The revision of the protocol this speaks, when the client does not say.
const PROTOCOL: &str = "2025-06-18";

/// What a client is told this server is for, once, at the start.
const INSTRUCTIONS: &str = "\
Each tool is one SHIKISHA-TERM automation primitive, named as it is in the \
app's manual with a `shikisha_` prefix. Arguments go in `params`, in the order \
the tool's description shows between the brackets. `shikisha_list` answers what \
this connection is allowed to call, and `shikisha_lua` runs a whole piece of \
Lua when a loop or a branch is wanted in one call.";

// ── Which instance, and with what key ──────────────────────────────────────

/// The door this server stands in front of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pipe: String,
    pub token: String,
}

impl Target {
    /// Work out the instance and the key from what was passed and what is in
    /// the environment.
    ///
    /// The environment is the default because that is how a tab's children are
    /// already told where the door is; the arguments are for saying otherwise,
    /// which is the whole point when the copy being tested is not the copy this
    /// is running inside.
    pub fn decide<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut pipe = std::env::var(crate::api::ENV_PIPE).ok();
        let mut token = std::env::var(crate::api::ENV_TOKEN).ok();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| anyhow!("{arg} needs a value after it"))
            };
            match arg.as_str() {
                "--pipe" => pipe = Some(value()?),
                "--pid" => pipe = Some(door_of(&value()?)?),
                "--token" => token = Some(value()?),
                "--token-file" => {
                    let at = value()?;
                    let read = std::fs::read_to_string(&at)
                        .with_context(|| format!("the key file {at} could not be read"))?;
                    token = Some(read.trim().to_string());
                }
                other => return Err(anyhow!("{other} is not something --mcp takes")),
            }
        }
        let pipe = pipe.ok_or_else(|| {
            anyhow!(
                "no instance to speak to. A server started by a tab's CLI is told which one in \
                 SHIKISHA_PIPE; point this at another copy with --pid <its process id> (or \
                 --pipe <its door>) and --token-file <its root>/data/api-token"
            )
        })?;
        // No key is not an error to raise here: the door itself refuses an
        // unknown one, and its refusal says more than a guess made in advance
        Ok(Self { pipe, token: token.unwrap_or_default() })
    }
}

/// The door of the copy running under a given process id.
///
/// The same name `api::door_path` builds for itself, made for somebody else's
/// process instead of this one.
fn door_of(pid: &str) -> Result<String> {
    let pid: u32 = pid
        .trim()
        .parse()
        .with_context(|| format!("{pid} is not a process id"))?;
    #[cfg(windows)]
    {
        Ok(format!(r"\\.\pipe\shikisha-{pid}"))
    }
    #[cfg(unix)]
    {
        // A socket lives beside the layout's own state, so this is only right
        // for a copy on this layout. Another root has to be named with --pipe
        Ok(crate::config::state_path(&format!("api-{pid}.sock"))
            .display()
            .to_string())
    }
}

// ── The two shapes, and the one thing that carries a call ──────────────────

/// Whatever can carry a call to a primitive and bring the answer back.
///
/// The pipe is the one that does in earnest; a test puts something simpler
/// here, which is what keeps the protocol answerable without an app running.
pub trait Asks {
    fn ask(&mut self, method: &str, params: Vec<Value>) -> Result<Value, String>;
}

/// The pipe, held open, with one reconnection in hand.
///
/// Debugging restarts the thing being debugged. A server that died with the
/// app would have to be restarted by the person whose client owns it, so a
/// call that finds the door shut opens it again and asks once more.
pub struct Door {
    target: Target,
    client: Option<ApiClient>,
}

impl Door {
    pub fn to(target: Target) -> Self {
        Self { target, client: None }
    }

    fn client(&mut self) -> Result<&mut ApiClient, String> {
        if self.client.is_none() {
            self.client = Some(
                ApiClient::connect(&self.target.pipe, &self.target.token)
                    .map_err(|e| format!("{} could not be opened: {e}", self.target.pipe))?,
            );
        }
        Ok(self.client.as_mut().expect("just opened"))
    }

    /// One call, and the envelope taken off the answer.
    fn once(&mut self, method: &str, params: Vec<Value>) -> Result<Value, String> {
        let answer = self
            .client()?
            .call(method, params)
            .map_err(|e| format!("the app stopped answering: {e}"))?;
        if answer.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(answer.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(answer
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the app refused the call without saying why")
            .to_string())
    }
}

impl Asks for Door {
    fn ask(&mut self, method: &str, params: Vec<Value>) -> Result<Value, String> {
        match self.once(method, params.clone()) {
            Err(e) if self.client.is_some() => {
                // The line may simply be old. Drop it and try the call once on
                // a fresh one -- and if that fails too, say what the second
                // attempt said, which is the state of things now
                self.client = None;
                self.once(method, params).map_err(|again| {
                    if again == e { again } else { format!("{again} (before that: {e})") }
                })
            }
            other => other,
        }
    }
}

// ── The manual, as descriptions ────────────────────────────────────────────

/// One row of the manual: how it is called, and what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    /// `shikisha.send_to_tab(tab, "text")`, as written
    pub signature: String,
    pub text: String,
}

/// Every primitive the manual has a line for.
///
/// Read out of the tables rather than kept as a second list here, for the same
/// reason `tools/list` asks the app: two lists of the same thing disagree, and
/// the one nobody reads is the one that is wrong.
pub fn manual() -> BTreeMap<String, Told> {
    read_manual(MANUAL)
}

fn read_manual(doc: &str) -> BTreeMap<String, Told> {
    let mut found = BTreeMap::new();
    for line in doc.lines() {
        let line = line.trim();
        // A table row, and only its first cell is a call. Backticks in the
        // description are ordinary prose and must not be read as signatures
        let Some(row) = line.strip_prefix('|') else { continue };
        let mut cells = row.splitn(2, '|');
        let Some(first) = cells.next() else { continue };
        let rest = cells.next().unwrap_or_default();
        if !first.contains("shikisha.") {
            continue;
        }
        let text = tidy(rest);
        if text.is_empty() {
            continue;
        }
        // A cell can hold two calls that are described together
        // (`get_var` / `set_var`), and both of them are told this line
        for span in first.split('`') {
            let Some(after) = span.trim().strip_prefix("shikisha.") else { continue };
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            found
                .entry(name)
                .or_insert_with(|| Told { signature: span.trim().to_string(), text: text.clone() });
        }
    }
    found
}

/// A description cell, as a line of text: the trailing pipes of a wider table
/// taken off, and the ones inside turned into something that reads.
fn tidy(cell: &str) -> String {
    let cell = cell.trim().trim_end_matches('|');
    let mut out = String::new();
    for (i, part) in cell.split('|').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if i > 0 && !out.is_empty() {
            out.push_str(" -- ");
        }
        out.push_str(part);
    }
    out
}

/// What a client is shown for one primitive.
fn describe(name: &str, manual: &BTreeMap<String, Told>) -> String {
    match manual.get(name) {
        Some(told) => format!("{}\n\n{}", told.signature, told.text),
        // Said plainly rather than left blank. A primitive with no line in the
        // manual is still callable, and a client that is told nothing about it
        // guesses instead
        None => format!(
            "`shikisha.{name}(...)` -- the manual has no line for this one. \
             Arguments go in `params`, in order."
        ),
    }
}

/// The shape of every tool's arguments.
///
/// One array, positional, exactly as the pipe takes them. Naming each
/// primitive's arguments would mean writing out seventy signatures and keeping
/// them right; the order is already in the description, from the manual.
fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "params": {
                "type": "array",
                "description": "The arguments, in the order the description's signature shows. \
                                Leave out for a primitive that takes none."
            }
        },
        "additionalProperties": false
    })
}

// ── The protocol ───────────────────────────────────────────────────────────

/// Answer one request. `None` is the right answer to a notification.
pub fn respond(req: &Value, door: &mut impl Asks) -> Option<Value> {
    let method = req.get("method").and_then(Value::as_str).unwrap_or_default();
    // A message with no id is a notification: it wants nothing back, and
    // answering one is a protocol error of our own making
    let id = req.get("id").cloned();
    let id = match id {
        Some(Value::Null) | None => {
            return None;
        }
        Some(id) => id,
    };
    Some(match method {
        "initialize" => {
            let asked = req
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL);
            answer(
                &id,
                json!({
                    "protocolVersion": asked,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "shikisha-term", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => answer(&id, json!({})),
        "tools/list" => match tools(door) {
            Ok(tools) => answer(&id, json!({ "tools": tools })),
            // The app is the only place that knows what may be called, so when
            // it cannot be asked there is no list to fall back on
            Err(e) => failure(&id, -32603, &e),
        },
        "tools/call" => answer(&id, called(req.get("params"), door)),
        other => failure(&id, -32601, &format!("{other} is not a method this server has")),
    })
}

/// Every tool, as the running app says it may be called right now.
fn tools(door: &mut impl Asks) -> Result<Vec<Value>, String> {
    let names = door.ask("list", Vec::new())?;
    let names = names
        .as_array()
        .ok_or_else(|| "the app's list of primitives did not arrive as a list".to_string())?;
    let manual = manual();
    Ok(names
        .iter()
        .filter_map(Value::as_str)
        .map(|name| {
            json!({
                "name": format!("{PREFIX}{name}"),
                "description": describe(name, &manual),
                "inputSchema": schema(),
            })
        })
        .collect())
}

/// One `tools/call`, carried out.
///
/// A primitive that refuses is a tool that failed, not a protocol that broke:
/// the refusal goes back inside the result, where the model reads it and can
/// try something else, instead of as an error the client swallows.
fn called(params: Option<&Value>, door: &mut impl Asks) -> Value {
    let Some(name) = params.and_then(|p| p.get("name")).and_then(Value::as_str) else {
        return trouble("a call needs the name of a tool");
    };
    let method = name.strip_prefix(PREFIX).unwrap_or(name);
    let args = params.and_then(|p| p.get("arguments"));
    let given = match args {
        // `arguments` is an object holding `params`, which is the shape the
        // schema asks for. An array sent in its place is taken as the
        // arguments themselves, because a client that does that means exactly
        // that and refusing it would be pedantry
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => match o.get("params") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a.clone(),
            Some(one) => vec![one.clone()],
        },
        Some(one) => vec![one.clone()],
    };
    match door.ask(method, given) {
        Ok(result) => json!({ "content": [text(&result)], "isError": false }),
        Err(e) => trouble(&e),
    }
}

/// An answer as a model reads it: a string as itself, anything else as JSON.
fn text(result: &Value) -> Value {
    let body = match result {
        Value::Null => "ok".to_string(),
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };
    json!({ "type": "text", "text": body })
}

fn trouble(why: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": why }], "isError": true })
}

fn answer(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

// ── The mode ───────────────────────────────────────────────────────────────

/// The `--mcp` process: one JSON object per line in, one per line out.
///
/// Nothing is ever printed except an answer. A client reads this process's
/// output as the protocol itself, so a friendly word on stdout would be a
/// parse error at the other end; what has to be said goes to stderr, where
/// clients show it.
pub fn run() -> Result<()> {
    let target = match Target::decide(std::env::args().skip(2)) {
        Ok(target) => target,
        Err(e) => {
            // Said once, by name, on stderr -- which is where a client shows
            // what a server it started had to say. Not returned: the runtime
            // prints what main gives back, and the same sentence twice reads
            // like two things went wrong
            eprintln!("shikisha-term --mcp: {e}");
            std::process::exit(1);
        }
    };
    let mut door = Door::to(target);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.context("stdin ended badly")?;
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                // Nothing to answer to: a line that is not JSON has no id to
                // answer under, and the protocol has no way to say so
                eprintln!("shikisha-term --mcp: a line that was not JSON ({e})");
                continue;
            }
        };
        if let Some(answer) = respond(&req, &mut door) {
            writeln!(out, "{answer}")?;
            out.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A door that answers from a script, and remembers what it was asked
    struct Fake {
        answers: Vec<(String, Result<Value, String>)>,
        heard: Vec<(String, Vec<Value>)>,
    }

    impl Fake {
        fn saying(answers: Vec<(&str, Result<Value, String>)>) -> Self {
            Self {
                answers: answers.into_iter().map(|(m, a)| (m.to_string(), a)).collect(),
                heard: Vec::new(),
            }
        }
    }

    impl Asks for Fake {
        fn ask(&mut self, method: &str, params: Vec<Value>) -> Result<Value, String> {
            self.heard.push((method.to_string(), params));
            self.answers
                .iter()
                .find(|(m, _)| m == method)
                .map(|(_, a)| a.clone())
                .unwrap_or_else(|| Err(format!("no such primitive: {method}")))
        }
    }

    fn req(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// The manual is where the descriptions come from, so a row of it has to
    /// arrive as a signature and a sentence
    #[test]
    fn the_manual_describes_the_tools() {
        let m = manual();
        let told = m.get("send_to_tab").expect("send_to_tab is in the manual");
        assert!(told.signature.starts_with("shikisha.send_to_tab("), "{told:?}");
        assert!(told.text.contains("instruction"), "{told:?}");
        // The one written with two calls in one cell tells both of them
        assert!(m.contains_key("get_var"), "get_var");
        assert!(m.contains_key("set_var"), "set_var");
        // Enough of them to be the manual and not an accident
        assert!(m.len() > 80, "only {} rows were read", m.len());
    }

    /// Backticks in a description are prose. Read as signatures, they would
    /// invent primitives that do not exist
    #[test]
    fn only_the_first_cell_is_a_call() {
        let m = read_manual(
            "| `shikisha.real(a)` | Does a thing. See `shikisha.invented(b)` for the other |",
        );
        assert!(m.contains_key("real"));
        assert!(!m.contains_key("invented"), "{m:?}");
    }

    /// A primitive the manual has no row for is still callable, and says so
    #[test]
    fn what_the_manual_leaves_out_is_still_described() {
        let said = describe("no_such_row", &BTreeMap::new());
        assert!(said.contains("no line for this one"), "{said}");
    }

    #[test]
    fn a_client_is_told_what_this_is() {
        let mut door = Fake::saying(vec![]);
        let out = respond(&req(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#), &mut door).unwrap();
        // The version the client asked for, not one of ours it has never heard of
        assert_eq!(out["result"]["protocolVersion"], "2024-11-05");
        assert!(out["result"]["capabilities"]["tools"].is_object());
        assert_eq!(out["result"]["serverInfo"]["name"], "shikisha-term");
    }

    /// Nothing goes back to a notification. An answer to one is a protocol
    /// error, and the client that gets it is entitled to close the line
    #[test]
    fn a_notification_is_not_answered() {
        let mut door = Fake::saying(vec![]);
        assert!(respond(&req(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), &mut door).is_none());
    }

    /// The list is the app's own, with every name prefixed so it cannot be
    /// mistaken for another server's
    #[test]
    fn the_tools_are_the_apps_own_list() {
        let mut door = Fake::saying(vec![("list", Ok(json!(["send_to_tab", "tab_screen"])))]);
        let out = respond(&req(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#), &mut door).unwrap();
        let tools = out["result"]["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "shikisha_send_to_tab");
        assert!(tools[0]["description"].as_str().unwrap().contains("shikisha.send_to_tab("));
        assert_eq!(tools[0]["inputSchema"]["properties"]["params"]["type"], "array");
        assert_eq!(door.heard, vec![("list".to_string(), Vec::new())]);
    }

    /// The arguments arrive in order, under the name with the prefix taken off
    #[test]
    fn a_call_is_carried_through_as_it_is() {
        let mut door = Fake::saying(vec![("send_to_tab", Ok(Value::Null))]);
        let out = respond(
            &req(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shikisha_send_to_tab","arguments":{"params":["reviewer","go"]}}}"#,
            ),
            &mut door,
        )
        .unwrap();
        assert_eq!(out["result"]["isError"], false);
        assert_eq!(out["result"]["content"][0]["text"], "ok");
        assert_eq!(
            door.heard,
            vec![("send_to_tab".to_string(), vec![json!("reviewer"), json!("go")])]
        );
    }

    /// Text comes back as itself; anything else as JSON a model can read
    #[test]
    fn an_answer_arrives_as_something_readable() {
        let mut door = Fake::saying(vec![("tab_screen", Ok(json!("$ ls\nfile.txt")))]);
        let out = respond(
            &req(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"shikisha_tab_screen","arguments":{"params":[1]}}}"#),
            &mut door,
        )
        .unwrap();
        assert_eq!(out["result"]["content"][0]["text"], "$ ls\nfile.txt");

        let mut door = Fake::saying(vec![("git_status", Ok(json!({"staged": ["a.rs"]})))]);
        let out = respond(
            &req(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"shikisha_git_status"}}"#),
            &mut door,
        )
        .unwrap();
        assert!(out["result"]["content"][0]["text"].as_str().unwrap().contains("a.rs"));
        // No arguments at all is a call with none, not a call that failed
        assert_eq!(door.heard, vec![("git_status".to_string(), Vec::new())]);
    }

    /// A primitive that refuses is a tool that failed. The model has to be able
    /// to read the refusal and try something else
    #[test]
    fn a_refusal_comes_back_inside_the_result() {
        let mut door = Fake::saying(vec![("send", Err("not allowed here".into()))]);
        let out = respond(
            &req(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"shikisha_send","arguments":{"params":[1,"x"]}}}"#),
            &mut door,
        )
        .unwrap();
        assert_eq!(out["result"]["isError"], true);
        assert_eq!(out["result"]["content"][0]["text"], "not allowed here");
        assert!(out.get("error").is_none(), "a refused primitive is not a broken protocol");
    }

    #[test]
    fn a_method_this_does_not_have_is_said_so() {
        let mut door = Fake::saying(vec![]);
        let out = respond(&req(r#"{"jsonrpc":"2.0","id":7,"method":"resources/list"}"#), &mut door).unwrap();
        assert_eq!(out["error"]["code"], -32601);
    }

    /// The environment is how a tab's children are told; the arguments are for
    /// pointing at a copy on purpose, which is what keeps a test instance apart
    /// from the one somebody is working in
    #[test]
    fn the_instance_can_be_named_on_purpose() {
        let t = Target::decide(["--pipe".into(), "door".into(), "--token".into(), "key".into()])
            .unwrap();
        assert_eq!(t, Target { pipe: "door".into(), token: "key".into() });

        #[cfg(windows)]
        {
            let t = Target::decide(["--pid".into(), "4242".into()]).unwrap();
            assert_eq!(t.pipe, r"\\.\pipe\shikisha-4242");
        }

        // A key in a file, which is where an instance leaves it for a script
        let at = std::env::temp_dir().join(format!("shikisha-mcp-test-{}", std::process::id()));
        std::fs::write(&at, "  written-down\n").unwrap();
        let t = Target::decide([
            "--pipe".into(),
            "door".into(),
            "--token-file".into(),
            at.display().to_string(),
        ])
        .unwrap();
        assert_eq!(t.token, "written-down");
        let _ = std::fs::remove_file(&at);

        // Said outright rather than guessed at
        assert!(Target::decide(["--pid".into(), "not-a-number".into()]).is_err());
        assert!(Target::decide(["--wat".into()]).is_err());
        assert!(Target::decide(["--token".into()]).is_err());
    }
}
