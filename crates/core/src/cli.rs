//! `shikisha`: the command an AI in a tab runs to hand work to another tab.
//!
//! Every tab finds it on its PATH (`shim_dir`), and it finds the app the way
//! the rest of a tab's children do -- `SHIKISHA_PIPE` and the tab's own key --
//! so what it asks is counted against that tab, under that tab's permissions.
//! Nothing to install, nothing to point at the app.
//!
//! A command rather than an MCP server for the same reason the skill that
//! explains it is written only on the person's say-so: registering a server
//! means writing into the CLI's own settings, and a command on the tab's PATH
//! changes nothing outside the tab.
//!
//! **How long it waits.** Less than two minutes, then it says so and stops:
//! an AI's shell tool gives up on a command at its own limit (Claude Code's is
//! two minutes unless asked for more), and a command killed there would say
//! nothing at all. Stopping first, the AI is told to end its turn -- and the
//! other tab's reply, when it comes, is typed into this tab (see `asktab`).
//!
//! Printed for an AI to read, so every answer ends in one line that starts
//! `[shikisha]` and says, in a word, what happened.

use std::io::Write as _;

use serde_json::{Value, json};

use crate::api::ApiClient;

/// How long `ask` holds on before handing the wait over to the tab
const WAIT_MS: u64 = 100_000;

pub fn run(args: &[String]) -> i32 {
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    match args.first().map(String::as_str) {
        Some("ask") => held("ask_tab", &args[1..], &mut out, &mut err),
        Some("run") => held("tab_run", &args[1..], &mut out, &mut err),
        Some("do") => held("browser_do", &args[1..], &mut out, &mut err),
        Some("tabs") => tabs(&mut out, &mut err),
        // The skill this app teaches an AI, as it would be written: for a
        // person to read before agreeing, and for a check to put in a folder
        Some("skill") => {
            let _ = write!(out, "{}", crate::skill::text());
            0
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            let _ = writeln!(out, "{}", usage());
            0
        }
        Some(other) => {
            let _ = writeln!(err, "[shikisha] {other} is not a command. {}", usage());
            2
        }
    }
}

/// The tabs of this desk as the AI addresses them, one to a line
fn tabs(out: &mut impl std::io::Write, err: &mut impl std::io::Write) -> i32 {
    let answer = ApiClient::from_env()
        .map_err(|e| e.to_string())
        .and_then(|mut c| c.call("tab_list", Vec::new()).map_err(|e| e.to_string()));
    let answer = match answer {
        Ok(a) if a.get("ok").and_then(Value::as_bool) == Some(true) => {
            a.get("result").cloned().unwrap_or(Value::Null)
        }
        Ok(a) => {
            let _ = writeln!(
                err,
                "[shikisha] {}",
                a.get("error").and_then(Value::as_str).unwrap_or("refused")
            );
            return 1;
        }
        Err(e) => {
            let _ = writeln!(
                err,
                "[shikisha] Not in a SHIKISHA-TERM tab, or its API is off ({e})."
            );
            return 1;
        }
    };
    for t in answer.as_array().cloned().unwrap_or_default() {
        let s = |k: &str| {
            t.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let flag = |k: &str| t.get(k).and_then(Value::as_bool).unwrap_or(false);
        let mut line = format!("<@{}>  {}  \"{}\"", s("id"), s("kind"), s("name"));
        if !s("folder").is_empty() {
            line.push_str(&format!("  {}", s("folder")));
        }
        if flag("you") {
            line.push_str("  (this tab)");
        } else if flag("named") {
            line.push_str("  (named by the person: you may drive it)");
        }
        let _ = writeln!(out, "{line}");
    }
    0
}

fn usage() -> &'static str {
    "Usage: shikisha ask ID \"what you want it to do\"   -- hand work to the AI in the tab <@ID> and print its reply
       shikisha run ID \"a command\"                -- run a command in the terminal <@ID> and print its output
       shikisha do ID \"what to get done\"          -- have the web page <@ID> driven toward a goal, and print what it found
       shikisha tabs                                -- list the tabs of this desk
       shikisha skill                               -- print the skill that explains this to an AI"
}

/// One held call to the app: `ask` an AI, `run` a command in a terminal, or
/// `do` something on a page -- the tab first, then what to say to it
fn held(
    method: &str,
    args: &[String],
    out: &mut impl std::io::Write,
    err: &mut impl std::io::Write,
) -> i32 {
    let Some(tab) = args.first() else {
        let _ = writeln!(err, "[shikisha] Which tab? {}", usage());
        return 2;
    };
    let text = args[1..].join(" ");
    if text.trim().is_empty() {
        let _ = writeln!(err, "[shikisha] What should it do? {}", usage());
        return 2;
    }
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
    let tab = tab
        .trim_start_matches('<')
        .trim_start_matches('@')
        .trim_end_matches('>');
    let answer = match client.call(
        method,
        vec![json!(tab), json!(text), json!({"timeout_ms": WAIT_MS})],
    ) {
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
        let _ = writeln!(err, "[shikisha] Not sent: {why}");
        return 1;
    }
    let r = answer.get("result").cloned().unwrap_or(Value::Null);
    let _ = writeln!(out, "{}", said(&r));
    0
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
            "[shikisha] STILL WORKING: <@{tab}> has not finished. End your turn now; its reply will be typed into this tab when it is done{facts}"
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

    #[test]
    fn nothing_to_ask_is_said_rather_than_sent() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(held("ask_tab", &["otter".into()], &mut out, &mut err), 2);
        assert!(String::from_utf8_lossy(&err).contains("What should it do?"));
    }
}
