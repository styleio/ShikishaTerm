//! `shikisha`: the command an AI in a tab runs to use this app.
//!
//! Every tab finds it on its PATH (`shim_dir`), and it finds the app the way
//! the rest of a tab's children do -- `SHIKISHA_PIPE` and the tab's own key --
//! so what it asks is counted against that tab, under that tab's permissions.
//! Nothing to install, nothing to point at the app.
//!
//! **One door, the commands Lua has.** `shikisha tab_run shell "make"`
//! is `shikisha.tab_run("shell", "make")`: the first word names the
//! command, the rest are its arguments, and the call goes down the same pipe
//! to the same code. Nothing here knows what any command does, so a command
//! added for Lua is a command here the moment it exists, and the permission
//! table decides what this tab may call, as it does for every other caller.
//! An argument written as JSON (`{"want":3}`) is handed over as that value;
//! every other one is a string.
//!
//! A command rather than an MCP server for the same reason the skill that
//! explains it is written only on the person's say-so: registering a server
//! means writing into the CLI's own settings, and a command on the tab's PATH
//! changes nothing outside the tab.
//!
//! **How long it waits.** The commands that wait for another tab (`HELD`)
//! wait less than two minutes, then say so and stop: an AI's shell tool gives
//! up on a command at its own limit (Claude Code's is two minutes unless
//! asked for more), and a command killed there would say nothing at all.
//! Stopping first, the AI is told to end its turn -- and the other tab's
//! reply, when it comes, goes into this tab's inbox (see `asktab`).
//!
//! Printed for an AI to read: a text as it is, anything else as JSON, and the
//! answer of a hand-off ending in one line that starts `[shikisha]` and says,
//! in a word, what happened.

use std::io::Write as _;

use serde_json::{Value, json};

use crate::api::ApiClient;

/// How long a hand-off holds on before handing the wait over to the tab
const WAIT_MS: u64 = 100_000;

/// The names this command answered to before it took the commands' own, and
/// what each is now -- for an AI still following an older copy of the skill
const RENAMED: [(&str, &str); 4] = [
    ("ask", "ask_tab"),
    ("run", "tab_run"),
    ("do", "browser_do"),
    ("tabs", "tab_list"),
];

pub fn run(args: &[String]) -> i32 {
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    match args.first().map(String::as_str) {
        // The skill this app teaches an AI, as it would be written: for a
        // person to read before agreeing, and for a check to put in a folder
        // The skill this app teaches an AI, and the guides beside it
        Some("skill") => skill(args.get(1).map(String::as_str), &mut out, &mut err),
        Some("help") | Some("--help") | Some("-h") | None => {
            let _ = writeln!(out, "{}", usage());
            0
        }
        Some(old) if RENAMED.iter().any(|(was, _)| *was == old) => {
            let now = RENAMED.iter().find(|(was, _)| *was == old).map(|(_, n)| *n).unwrap_or_default();
            let _ = writeln!(err, "[shikisha] `{old}` is now `{now}`: run `shikisha {now} ...` with the same arguments.");
            2
        }
        Some(command) => call(command, &args[1..], &mut out, &mut err),
    }
}

/// The skill as it would be written -- for a person to read before agreeing,
/// and for a check to put in a folder -- or a guide it points to. The guides
/// are kept in the program so they always describe this version's commands
fn skill(topic: Option<&str>, out: &mut impl std::io::Write, err: &mut impl std::io::Write) -> i32 {
    match topic {
        None => {
            let _ = write!(out, "{}", crate::skill::text());
            0
        }
        Some(t) if t == "orchestration" => {
            let _ = write!(out, "{}", crate::orch::text::guide());
            0
        }
        Some(t) if t == "worktree" => {
            let _ = write!(out, "{}", crate::skill::worktree_guide());
            0
        }
        Some(other) => {
            let _ = writeln!(err, "[shikisha] There is no guide called {other}. Try: shikisha skill orchestration, shikisha skill worktree");
            2
        }
    }
}

fn usage() -> &'static str {
    "Usage: shikisha COMMAND [ARGUMENT...]
  Runs the SHIKISHA-TERM command of that name -- the one Lua calls shikisha.COMMAND --
  for this tab. An argument written as JSON ({\"want\":3}) is passed as that value.
    shikisha ask_tab ID \"a line\" \"what to do\"   -- hand work to the AI in <@ID>, print its reply; the line is
                                                    what the chat shows (one short line, like to a colleague)
    shikisha tab_run ID \"a command\"                -- run a command in the terminal <@ID>, print its output
    shikisha browser_do ID \"what to get done\"      -- drive the web page <@ID> toward a goal, print what it found
    shikisha say \"a line\"                          -- say one short line to the other tabs, in the chat
    shikisha react ID 👍                            -- mark the last line <@ID> said (👍 ❤️ 🎉 👀 ✅ ❓)
    shikisha share commit|pr|file|url WHAT [TITLE]  -- show the others a commit, a pull request, a file or a page
    shikisha tab_list                               -- the tabs of this desk
    shikisha tab_conversation ID '{\"want\":3}'      -- the last things said in <@ID>'s conversation
    shikisha adr_list                               -- where this project keeps its decision records, and each one's status
    shikisha list                                   -- every command this tab may call
    shikisha skill                                  -- print the skill that explains this to an AI
    shikisha skill orchestration                    -- the guide for handing a job out to other AI tabs
    shikisha skill worktree                         -- the guide for a git worktree with an AI working in it"
}

/// One argument as the command is handed it: JSON when it is written as a
/// JSON object or list, a string otherwise -- so a request that happens to be
/// a number is still a request
fn argument(arg: &str) -> Value {
    let t = arg.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        if let Ok(v) = serde_json::from_str(arg) {
            return v;
        }
    }
    json!(arg)
}

/// The arguments of `command`. A hand-off that names no wait of its own is
/// given this command's, so it answers before the AI's shell gives up on it
fn arguments(command: &str, args: &[String]) -> Vec<Value> {
    let mut params: Vec<Value> = args.iter().map(|a| argument(a)).collect();
    // Where the options go: after the tab and what to do, and for an ask
    // after its line as well
    let options_at = if command == "ask_tab" { 3 } else { 2 };
    if crate::asktab::HELD.contains(&command) && params.len() == options_at {
        params.push(json!({"timeout_ms": WAIT_MS}));
    }
    params
}

/// One call to the app, printed for an AI
fn call(
    command: &str,
    args: &[String],
    out: &mut impl std::io::Write,
    err: &mut impl std::io::Write,
) -> i32 {
    let mut client = match ApiClient::from_env() {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(
                err,
                "[shikisha] Not in a SHIKISHA-TERM tab, or its API is off ({e}). Run this inside a tab of the app."
            );
            return 1;
        }
    };
    let answer = match client.call(command, arguments(command, args)) {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(err, "[shikisha] The app stopped answering: {e}");
            return 1;
        }
    };
    if answer.get("ok").and_then(Value::as_bool) != Some(true) {
        let why = answer
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the app refused without saying why");
        let _ = writeln!(err, "[shikisha] {command}: {why}");
        return 1;
    }
    let _ = writeln!(out, "{}", printed(&answer.get("result").cloned().unwrap_or(Value::Null)));
    0
}

/// What a command answered, as an AI reads it: a hand-off's answer ends in the
/// line that says what happened, an answer that says what to do next ends in
/// those commands, a text is itself, the rest is JSON
fn printed(r: &Value) -> String {
    if r.get("tab").is_some() && r.get("state").and_then(Value::as_str).is_some() {
        return said(r);
    }
    if let Some(next) = r.get("next").and_then(Value::as_array) {
        return with_next(r, next);
    }
    match r {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// The answer, then the commands to run next, one to a line and last, where
/// an AI reading the output from the bottom finds them first
fn with_next(r: &Value, next: &[Value]) -> String {
    let mut rest = r.clone();
    if let Some(o) = rest.as_object_mut() {
        o.remove("next");
    }
    let body = serde_json::to_string_pretty(&rest).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    if r.get("state").and_then(Value::as_str).is_some_and(|s| s == "NOTHING YET") {
        lines.push("[shikisha] NOTHING YET: nothing has arrived. This is not a failure.".to_string());
    }
    for n in next.iter().filter_map(Value::as_str) {
        lines.push(format!("[shikisha] next: {n}"));
    }
    format!("{body}

{}", lines.join("
"))
}

/// The answer, as an AI reads it: the reply, then one line of what happened
fn said(r: &Value) -> String {
    let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default();
    let tab = s("tab");
    let round = match (
        r.get("round").and_then(Value::as_u64),
        r.get("max_rounds").and_then(Value::as_u64),
    ) {
        (Some(n), Some(m)) => format!("round {n}/{m}"),
        (Some(n), None) => format!("round {n}"),
        _ => String::new(),
    };
    let folder = match r.get("same_folder").and_then(Value::as_bool) {
        Some(true) => "same folder: yes",
        Some(false) => "same folder: no",
        None => "",
    };
    let facts: Vec<&str> = [round.as_str(), folder]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect();
    let facts = if facts.is_empty() {
        String::new()
    } else {
        format!(" ({})", facts.join(", "))
    };
    let reply = s("reply").trim();
    match s("state") {
        "DONE" => {
            let note = r
                .get("note")
                .and_then(Value::as_str)
                .map(|n| format!(" -- {n}"))
                .unwrap_or_default();
            format!("{reply}\n\n[shikisha] DONE: the reply of <@{tab}> is above{facts}{note}")
        }
        "PENDING" => format!(
            "[shikisha] STILL WORKING: <@{tab}> has not finished. End your turn now; when it is done you will be told here, and shikisha inbox gives you its reply{facts}"
        ),
        "STUCK" => format!(
            "{reply}\n\n[shikisha] NOT DONE: <@{tab}> could not get it done (the reason is above){facts}"
        ),
        "STOPPED" => format!(
            "[shikisha] NOT DONE: the run on <@{tab}> was stopped before it finished{facts}"
        ),
        "QUESTION" => format!(
            "{reply}\n\n[shikisha] WAITING: <@{tab}> is waiting for a person to approve or choose (its screen is above). Tell the person{facts}"
        ),
        "BUSY" => format!(
            "[shikisha] NOT SENT: <@{tab}> stayed busy the whole time, so nothing was typed into it{facts}"
        ),
        "LIMIT" => format!("[shikisha] NOT DONE: <@{tab}> hit its usage limit{facts}"),
        "FAILED" => {
            format!("{reply}\n\n[shikisha] NOT DONE: the turn of <@{tab}> ended in an error{facts}")
        }
        "EXIT" => format!("[shikisha] NOT DONE: the program in <@{tab}> has ended{facts}"),
        "GONE" => format!("[shikisha] NOT DONE: <@{tab}> was closed{facts}"),
        other => format!("{reply}\n\n[shikisha] {other}{facts}"),
    }
}

/// The folder a tab puts in front of its PATH, holding `shikisha` for every
/// shell a CLI might run it from: a `.cmd` for cmd and PowerShell, and a
/// script without an extension for the bash a CLI brings on Windows -- the
/// pair npm installs for the same reason. Written once per run of the app,
/// pointing at this copy of it. `None` when it could not be made, and a tab is
/// then started without it
pub fn shim_dir() -> Option<std::path::PathBuf> {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let dir = crate::config::state_path("bin");
        std::fs::create_dir_all(&dir).ok()?;
        let exe_s = exe.display().to_string();
        #[cfg(windows)]
        {
            std::fs::write(
                dir.join("shikisha.cmd"),
                format!("@\"{exe_s}\" --cli %*\r\n"),
            )
            .ok()?;
            // Git's bash reads C:\x as /c/x; a path it cannot read runs nothing
            let unix = exe_s.replace('\\', "/");
            let unix = match unix.split_once(":/") {
                Some((drive, rest)) if drive.len() == 1 => {
                    format!("/{}/{rest}", drive.to_ascii_lowercase())
                }
                _ => unix,
            };
            std::fs::write(
                dir.join("shikisha"),
                format!("#!/bin/sh\nexec \"{unix}\" --cli \"$@\"\n"),
            )
            .ok()?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let f = dir.join("shikisha");
            std::fs::write(&f, format!("#!/bin/sh\nexec \"{exe_s}\" --cli \"$@\"\n")).ok()?;
            let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755));
        }
        Some(dir)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guide the skill sends an AI to is printed by its name, and a name
    /// there is no guide for says which there are
    #[test]
    fn the_worktree_guide_is_printed_by_its_name() {
        // Named through a variable: `this_door_has_no_commands_of_its_own`
        // reads every quoted word handed to `Some` in this file as one
        let (guide, none) = ("worktree", "nothing");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(skill(Some(guide), &mut out, &mut err), 0);
        assert_eq!(String::from_utf8(out).unwrap(), crate::skill::worktree_guide());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(skill(Some(none), &mut out, &mut err), 2);
        assert!(String::from_utf8(err).unwrap().contains("shikisha skill worktree"));
    }

    #[test]
    fn every_answer_ends_in_a_line_that_says_what_happened() {
        let done = said(
            &json!({"tab":"otter","state":"DONE","reply":"LGTM","round":2,"max_rounds":40,"same_folder":true}),
        );
        assert!(done.starts_with("LGTM"));
        assert!(
            done.ends_with(
                "[shikisha] DONE: the reply of <@otter> is above (round 2/40, same folder: yes)"
            ),
            "{done}"
        );
        let pending = said(&json!({"tab":"otter","state":"PENDING","round":1}));
        assert!(
            pending.contains("STILL WORKING") && pending.contains("End your turn"),
            "{pending}"
        );
        let q = said(&json!({"tab":"otter","state":"QUESTION","reply":"Allow? (y/n)"}));
        assert!(q.contains("WAITING") && q.starts_with("Allow?"), "{q}");
    }

    /// The words after the command are its arguments, as Lua would pass them
    #[test]
    fn the_arguments_are_the_commands_own() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // A hand-off with no wait of its own answers before the AI's shell gives up
        assert_eq!(
            arguments("ask_tab", &s(&["otter", "Can you review this?", "review src/p.rs"])),
            vec![json!("otter"), json!("Can you review this?"), json!("review src/p.rs"), json!({"timeout_ms": WAIT_MS})]
        );
        // An ask written the old way is passed on as written, for the app to
        // refuse with the new form rather than to read wrongly
        assert_eq!(arguments("ask_tab", &s(&["otter", "review this"])), vec![json!("otter"), json!("review this")]);
        // ...and one that names its own keeps it
        assert_eq!(
            arguments("tab_run", &s(&["sh", "make", r#"{"timeout_ms":5000}"#])),
            vec![json!("sh"), json!("make"), json!({"timeout_ms": 5000})]
        );
        // JSON only where it is written as an object or a list: a request that
        // is a number, or a word, is still text
        assert_eq!(
            arguments("tab_conversation", &s(&["otter", r#"{"want":3}"#])),
            vec![json!("otter"), json!({"want": 3})]
        );
        assert_eq!(arguments("send_to_tab", &s(&["otter", "42"])), vec![json!("otter"), json!("42")]);
        assert_eq!(arguments("send_to_tab", &s(&["otter", "{not json"])), vec![json!("otter"), json!("{not json")]);
    }

    #[test]
    fn what_comes_back_is_printed_for_an_ai() {
        assert!(printed(&json!({"tab":"otter","state":"DONE","reply":"LGTM"})).ends_with("is above"));
        assert_eq!(printed(&json!("on the screen")), "on the screen");
        assert_eq!(printed(&Value::Null), "");
        assert!(printed(&json!([{"id":"otter"}])).contains("\"id\": \"otter\""));
    }

    /// The words this command answers by itself, without the app. Everything
    /// else is a command's own name
    const LOCAL: [&str; 4] = ["skill", "help", "--help", "-h"];

    /// This door has no commands of its own. A word answered here instead of
    /// being handed to the app is a second name for something, and the two
    /// names are what drift apart: an `ask` here beside `shikisha.ask_tab` in Lua,
    /// each with its own behaviour to keep in step
    #[test]
    fn this_door_has_no_commands_of_its_own() {
        let src = include_str!("cli.rs");
        let body = src
            .split("pub fn run(args: &[String]) -> i32 {")
            .nth(1)
            .and_then(|r| r.split("\n}\n").next())
            .expect("run() is not where the command's words are read any more");
        let answered: Vec<&str> = body
            .split("Some(\"")
            .skip(1)
            .filter_map(|p| p.split('"').next())
            .collect();
        assert!(!answered.is_empty(), "reading run() found no words at all");
        for word in &answered {
            assert!(
                LOCAL.contains(word),
                "run() answers `{word}` itself. Make it a command Lua has (grants::CATALOG) and let it through to the app"
            );
        }
        for word in LOCAL.iter().chain(RENAMED.iter().map(|(was, _)| was)) {
            assert!(
                !crate::grants::CATALOG.iter().any(|e| e.name == *word),
                "`{word}` is a command now, and this door would keep it from the app"
            );
        }
    }

    /// Every "shikisha WORD" written for someone to run -- in the skill, in
    /// the manuals, in what the app answers -- names a command that exists.
    /// Renaming a command and leaving the old name written somewhere fails here
    #[test]
    fn every_shikisha_written_anywhere_is_a_command() {
        let root = crate::repo_root();
        let mut texts: Vec<(String, String)> = vec![("the skill".into(), crate::skill::text()), ("usage".into(), usage().into())];
        for dir in ["crates/core/src", "crates/shared/src", "docs", "lang"] {
            for entry in std::fs::read_dir(root.join(dir)).expect(dir).flatten() {
                let p = entry.path();
                if matches!(p.extension().and_then(|e| e.to_str()), Some("rs" | "md" | "json")) {
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        texts.push((p.display().to_string(), t));
                    }
                }
            }
        }
        let known = |w: &str| LOCAL.contains(&w) || crate::grants::CATALOG.iter().any(|e| e.name == w);
        let mut found = 0;
        let mut wrong = Vec::new();
        for (where_, text) in &texts {
            for line in text.lines() {
                // Written to be run: in backquotes, or at the head of a line
                // of an example -- not prose that says "the shikisha table"
                let heads = line
                    .match_indices("`shikisha ")
                    .map(|(i, _)| i + "`shikisha ".len())
                    .chain(line.trim_start().starts_with("shikisha ").then(|| {
                        line.len() - line.trim_start().len() + "shikisha ".len()
                    }));
                for at in heads {
                    let word: String = line[at..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .collect();
                    if word.is_empty() {
                        continue;
                    }
                    found += 1;
                    if !known(&word) {
                        wrong.push(format!("{where_}: shikisha {word}"));
                    }
                }
            }
        }
        assert!(found > 10, "reading the texts found almost nothing ({found})");
        assert!(wrong.is_empty(), "written as \"shikisha WORD\" but no command has that name:\n{}", wrong.join("\n"));
    }

    /// An AI following an older copy of the skill is told the new name
    #[test]
    fn an_old_name_says_the_new_one() {
        assert_eq!(run(&["ask".to_string(), "otter".to_string(), "hi".to_string()]), 2);
        for (was, now) in RENAMED {
            assert!(crate::grants::CATALOG.iter().any(|e| e.name == now), "{was} points at {now}, which is no command");
        }
    }
}
