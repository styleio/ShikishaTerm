//! What the git panel asks of a folder, answered the one way.
//!
//! Every one of these is a primitive (`git_status`, `git_commit`, ...), and
//! the panel reaches them the way a script does: by name, through the same
//! permission table. What was two ways of doing it is one here -- the Lua
//! primitive and the panel's own worker both call [`call`] -- so what a script
//! is told and what the panel shows cannot drift apart.
//!
//! The panel runs them on a thread of their own, one folder at a time and in
//! the order they were asked (see `runtime::GitLine`). Git is a program that
//! is started and waited for, and on a network share, a large repository, a
//! server or a MicroVM that wait is long: done on the board's own loop, it
//! held every terminal still while it went on.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The primitives answered here. The rest of the git family talks to a server
/// (`git_fetch`, `git_pull`, `git_push`, `git_catch_up`) and is run by the
/// panel's own network path, or is a script's alone (`git_run`, `git_log`,
/// `git_conflicts`, `git_set_base`)
pub const PANEL: &[&str] = &[
    "git_status",
    "git_branch",
    "git_branches",
    "git_diff",
    "git_graph",
    "git_detail",
    "git_hunks",
    "git_apply",
    "git_stage",
    "git_unstage",
    "git_discard",
    "git_commit",
    "git_checkout",
    "git_merge",
    "git_remote_branches",
    "git_branch_create",
];

/// Whether a primitive signs in as the folder's account, and whether to a
/// server (see [`crate::config::GitUse::to_git`]). `None`: it runs as nobody
pub fn signs_in(method: &str) -> Option<bool> {
    match method {
        "git_commit" | "git_merge" => Some(false),
        _ => None,
    }
}

/// The top of the repository a folder is in, asked of git where the folder
/// is. What every call below is given
pub fn root(dir: &Path, at: Option<&crate::elsewhere::Elsewhere>) -> Result<PathBuf, String> {
    crate::git::there(dir, at);
    crate::git::root(dir).map_err(|e| e.to_string())
}

fn opt<'a>(args: &'a [Value], i: usize) -> Option<&'a serde_json::Map<String, Value>> {
    args.get(i).and_then(|v| v.as_object())
}

fn flag(o: Option<&serde_json::Map<String, Value>>, name: &str) -> bool {
    o.and_then(|o| o.get(name)).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn text<'a>(o: Option<&'a serde_json::Map<String, Value>>, name: &str) -> Option<&'a str> {
    o.and_then(|o| o.get(name)).and_then(|v| v.as_str())
}

fn string_arg(args: &[Value], i: usize) -> Result<String, String> {
    args.get(i)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| crate::i18n::t("err.git.bad_argument"))
}

/// One path, or several
fn paths(v: Option<&Value>) -> Result<Vec<String>, String> {
    match v {
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|p| p.as_str().map(str::to_string).ok_or_else(|| crate::i18n::t("err.git.paths")))
            .collect(),
        _ => Err(crate::i18n::t("err.git.paths")),
    }
}

/// The `encoding` a call was given: none, or empty, is "work out what each
/// file is written in"; a name nobody knows is an error rather than a quiet
/// UTF-8
fn encoding(o: Option<&serde_json::Map<String, Value>>) -> Result<Option<&'static encoding_rs::Encoding>, String> {
    match text(o, "encoding").map(str::trim).filter(|n| !n.is_empty()) {
        None => Ok(None),
        Some(name) => crate::charset::named(name)
            .map(Some)
            .ok_or_else(|| crate::i18n::tp("err.git.unknown_encoding", &[("enc", name)])),
    }
}

/// Answer one primitive about the repository at `dir` (its top, see [`root`];
/// the machine it is on already noted on this thread). `args` are what came
/// after the tab; `who` is the folder's account for the two that commit
pub fn call(
    method: &str,
    dir: &Path,
    protect: &[String],
    who: Option<&crate::git::As>,
    args: &[Value],
) -> Result<Value, String> {
    let e = |e: anyhow::Error| e.to_string();
    let signed = || who.ok_or_else(|| crate::i18n::t("err.git.no_tab"));
    Ok(match method {
        // What has changed, one row per file, in git's own two letters
        "git_status" => {
            let rows: Vec<Value> = crate::git::status(dir)
                .map_err(e)?
                .into_iter()
                .map(|ch| {
                    let mut row = json!({
                        "path": ch.path,
                        "index": ch.index.to_string(),
                        "work": ch.work.to_string(),
                        // Not opposites: a file can be both at once, the
                        // ordinary result of staging one hunk of it
                        "staged": !matches!(ch.index, ' ' | '?'),
                        "unstaged": ch.work != ' ',
                        "conflict": ch.index == 'U'
                            || ch.work == 'U'
                            || (ch.index == 'A' && ch.work == 'A')
                            || (ch.index == 'D' && ch.work == 'D'),
                        "tangled": ch.tangled,
                    });
                    if let Some(f) = ch.from {
                        row["from"] = json!(f);
                    }
                    row
                })
                .collect();
            json!(rows)
        }
        // The diff, cut into the pieces a person says yes or no to; a
        // commit's own change is cut the same way
        "git_hunks" => {
            let o = opt(args, 0);
            let path = text(o, "path");
            let raw = match text(o, "commit").filter(|c| !c.is_empty()) {
                Some(c) => crate::git::show_bytes(dir, c, path.unwrap_or_default()),
                None => crate::git::diff_bytes(dir, path, flag(o, "staged")),
            }
            .map_err(e)?;
            let rows: Vec<Value> = crate::git::split_hunks_bytes(&raw, encoding(o)?)
                .into_iter()
                .map(|h| {
                    json!({
                        "file": h.file, "header": h.header, "start": h.start, "end": h.end,
                        "patch": h.patch, "encoding": h.encoding.name(), "exact": h.exact,
                    })
                })
                .collect();
            json!(rows)
        }
        // ...and putting one back: staged, unstaged, or undone
        "git_apply" => {
            let patch = string_arg(args, 0)?;
            let o = opt(args, 1);
            let enc = encoding(o)?.unwrap_or(encoding_rs::UTF_8);
            crate::git::apply(dir, &patch, enc, flag(o, "cached"), flag(o, "reverse")).map_err(e)?;
            Value::Null
        }
        "git_diff" => {
            let o = opt(args, 0);
            let raw = crate::git::diff_bytes(dir, text(o, "path"), flag(o, "staged")).map_err(e)?;
            json!(crate::git::diff_text(&raw, encoding(o)?))
        }
        // The branch checked out, and whether committing straight onto it is
        // the kind of thing to ask about first. Null when the head is detached
        "git_branch" => match crate::git::branch(dir).map_err(e)? {
            None => Value::Null,
            Some(n) => {
                let mut row = json!({ "protected": crate::git::is_protected(&n, protect), "name": n });
                // Only when there is a branch on the server to count against
                if let Some(up) = crate::git::upstream(dir).map_err(e)? {
                    row["upstream"] = json!(up.name);
                    row["ahead"] = json!(up.ahead);
                    row["behind"] = json!(up.behind);
                    if !up.tracked {
                        row["by_name"] = json!(true);
                    }
                    if !up.sha.is_empty() {
                        row["upstream_sha"] = json!(up.sha);
                    }
                }
                // What it was cut from, and what bringing its latest in runs
                if let Some(base) = crate::git::recorded_base(dir, &n) {
                    let steps = crate::git::catch_up_steps(dir, &base);
                    if let Some(theirs) = steps.last().and_then(|s| s.last()) {
                        if let Ok(c) = crate::git::run(dir, &["rev-list", "--count", &format!("HEAD..{theirs}")])
                            && let Ok(c) = c.trim().parse::<u64>()
                        {
                            row["base_behind"] = json!(c);
                        }
                        if crate::git::merging_in(dir, theirs) {
                            row["catching_up"] = json!(theirs);
                        }
                    }
                    row["catch_up"] = json!(crate::git::said(&steps));
                    row["base"] = json!(base);
                }
                row
            }
        },
        "git_stage" => {
            crate::git::stage(dir, &paths(args.first())?).map_err(e)?;
            Value::Null
        }
        "git_unstage" => {
            crate::git::unstage(dir, &paths(args.first())?).map_err(e)?;
            Value::Null
        }
        // Throwing a change away; `plan` answers with the commands only
        "git_discard" => {
            let files = paths(args.first())?;
            let o = opt(args, 1);
            let (staged, plan) = (flag(o, "staged"), flag(o, "plan"));
            let said = match plan {
                true => crate::git::said(&crate::git::discard_steps(dir, &files, staged)),
                false => crate::git::discard(dir, &files, staged).map_err(e)?,
            };
            json!({ "plan": plan, "said": said })
        }
        // Commit what is staged. A shared branch refuses unless the caller
        // says it meant it
        "git_commit" => {
            let message = string_arg(args, 0)?;
            let o = opt(args, 1);
            json!(crate::git::commit(dir, &message, protect, flag(o, "allow_protected"), flag(o, "amend"), signed()?)
                .map_err(e)?)
        }
        // The history, with git drawing the graph
        "git_graph" => {
            let o = opt(args, 0);
            let count = o.and_then(|o| o.get("count")).and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(200);
            let rows: Vec<Value> = crate::git::graph(dir, flag(o, "all"), flag(o, "remotes"), count, text(o, "branch"))
                .map_err(e)?
                .into_iter()
                .map(|r| {
                    json!({
                        "graph": r.graph, "hash": r.hash, "short": r.short,
                        "author": r.author, "date": r.date, "subject": r.subject,
                    })
                })
                .collect();
            json!(rows)
        }
        "git_detail" => {
            let d = crate::git::detail(dir, &string_arg(args, 0)?).map_err(e)?;
            json!({
                "hash": d.hash, "parents": d.parents, "author": d.author, "author_date": d.author_date,
                "committer": d.committer, "commit_date": d.commit_date, "subject": d.subject,
                "body": d.body, "files": d.files,
            })
        }
        // The branches the servers have, each with what bringing it in runs
        "git_remote_branches" => {
            let rows: Vec<Value> = crate::git::remote_branches(dir)
                .map_err(e)?
                .into_iter()
                .map(|name| json!({ "catch_up": crate::git::said(&crate::git::catch_up_steps(dir, &name)), "name": name }))
                .collect();
            json!(rows)
        }
        // Every local branch, with a mark on the one checked out
        "git_branches" => {
            let rows: Vec<Value> = crate::git::branches(dir)
                .map_err(e)?
                .into_iter()
                .map(|(name, here)| json!({ "current": here, "protected": crate::git::is_protected(&name, protect), "name": name }))
                .collect();
            json!(rows)
        }
        "git_checkout" => {
            crate::git::checkout(dir, &string_arg(args, 0)?).map_err(e)?;
            Value::Null
        }
        "git_merge" => json!(crate::git::merge(dir, &string_arg(args, 0)?, signed()?).map_err(e)?),
        "git_branch_create" => {
            crate::git::branch_create(dir, &string_arg(args, 0)?).map_err(e)?;
            Value::Null
        }
        other => return Err(format!("{other} is not a git panel call")),
    })
}
