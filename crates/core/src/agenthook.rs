//! Asking an AI CLI to report the conversation it is running.
//!
//! Every CLI worth resuming can run a command of your choosing when a
//! conversation starts. That command is this app in hook mode (`--hook
//! session`), which reports the id back through the API pipe — see
//! `main::hook_mode`. What this module does is put that command into the CLI's
//! own config file, and take it out again.
//!
//! Three rules, because this writes into a file that belongs to another
//! program and to the person using it:
//!
//!   - **Nothing else in the file is touched.** Only entries this app put there
//!     are replaced or removed; every other hook the person has set up survives
//!   - **A file that doesn't parse is left exactly as it is.** Rewriting a
//!     config we cannot read would destroy settings to install a convenience
//!   - **The previous contents are kept** beside it as `.bak` before the first
//!     change. It costs nothing and it is the difference between "undo it" and
//!     "what did it just do to my settings"

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::profile::HookFormat;

/// How this app's own hook entry is recognised again later.
///
/// Not a comment or a marker field — the command itself. A marker can be
/// dropped by an editor that rewrites the file; the command cannot, because
/// removing it removes the hook
const MARK: &str = "--hook";

/// How long a CLI should wait for one of these, in seconds.
///
/// Nothing here answers back, so the number only matters as a ceiling. Three
/// rather than five because Codex caps two of its events at three and warns,
/// on every single launch, about anything higher — a line of complaint in
/// someone's terminal forever, to allow a wait we do not want in the first
/// place
const TIMEOUT_S: u32 = 3;

/// One event we ask a CLI to report, and what our end makes of it.
///
/// The meaning travels in the command line rather than being looked up when
/// the event lands. A person opening their own settings file has to be able to
/// see what this app will do with each event, and `--hook state:QUESTION`
/// under `PermissionRequest` says it without a manual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The CLI's own name for the event
    pub event: String,
    /// What we ask to be run for it: `session`, or `state:<STATE>`
    pub arg: String,
}

/// One CLI that can be asked to report on itself.
#[derive(Debug, Clone)]
pub struct Target {
    /// The CLI's display name, from its profile
    pub name: String,
    pub file: PathBuf,
    pub format: HookFormat,
    /// How long the CLI may wait, already in the unit that CLI counts in
    pub timeout: u32,
    pub entries: Vec<Entry>,
}

/// Where the target stands right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The CLI keeps no config here — most likely it isn't installed
    NoConfig,
    /// A config exists, without our entry
    Absent,
    /// Our entry is there and points at this app where it now lives
    Installed,
    /// Our entry is there but names a different path — the app was moved or
    /// copied. It would run the old one, or nothing at all
    Stale,
    /// The file exists and is not JSON we can read. Nothing will be written
    Unreadable(String),
}

/// Every CLI in the profiles that says how to hook it.
pub fn targets() -> Vec<Target> {
    crate::profile::all()
        .into_iter()
        .filter_map(|p| {
            let hook = p.resume.as_ref()?.hook.as_ref()?;
            let mut entries: Vec<Entry> = hook
                .events
                .iter()
                .map(|event| Entry { event: event.clone(), arg: "session".into() })
                .collect();
            // A state this app has no name for would install a hook that fires
            // into nothing, so the profile is checked here rather than trusted
            for (event, state) in &hook.states {
                match crate::detect::TabState::from_label(state) {
                    Some(s) => entries.push(Entry {
                        event: event.clone(),
                        arg: format!("state:{}", s.label()),
                    }),
                    None => crate::append_hook_log(&format!(
                        "profile {}: {event} says {state:?}, which is not a state",
                        p.name
                    )),
                }
            }
            Some(Target {
                name: p.name.clone(),
                file: expand(&hook.file),
                format: hook.format,
                timeout: hook.timeout_unit.from_seconds(TIMEOUT_S),
                entries,
            })
        })
        .collect()
}

/// `{home}` is the only thing a profile may stand in for. A profile that could
/// name any path would be naming a file to overwrite
fn expand(path: &str) -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(path.replace("{home}", &home))
}

/// This app, as the CLI will have to spell it.
fn me() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("SHIKISHA-TERM.exe"))
}

/// The same path with no space in it, for a CLI that will not take quotes.
///
/// A path that has none already is its own answer, on every system. `None`
/// means there is no way to name this program without a space, and saying so
/// is the point: the alternative is installing a hook that can never run and
/// finding out from a dot that never moves.
///
/// Nothing but Windows keeps a second name for a file, so on everything else
/// a path with a space in it has no answer at all. In practice that is rare
/// there, and a person can move the program; on Windows a space is the normal
/// case, which is why the other half of this exists.
#[cfg(not(windows))]
fn spaceless(path: &Path) -> Option<String> {
    let long = path.display().to_string();
    (!long.contains(' ')).then_some(long)
}

/// Windows has kept a second, space-free name for every file since the days
/// when eight characters was the whole allowance (`C:\PROGRA~1\...`), and it
/// is the only way to name a program in a command line that cannot be quoted.
/// `None` when the path still has a space afterwards — that happens where the
/// short names have been turned off for a volume.
#[cfg(windows)]
fn spaceless(path: &Path) -> Option<String> {
    let long = path.display().to_string();
    if !long.contains(' ') {
        return Some(long);
    }
    let wide: Vec<u16> = long.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: both slices are ours, and the call is told the size of the one
    // it writes into
    let n = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            wide.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let short = String::from_utf16_lossy(&buf[..n]);
    (!short.contains(' ')).then_some(short)
}

/// One handler, in the spelling this CLI accepts.
///
/// `async` is not a nicety. A hook is a program the CLI runs and waits for,
/// and these fire on every turn and every permission dialog -- a fifth of a
/// second of process startup, charged to the person's turn, for a report
/// nobody is waiting on. Nothing here answers back, so nothing here should be
/// waited for. The timeout stays for the CLIs that still honour one.
fn handler(format: HookFormat, timeout: u32, arg: &str) -> serde_json::Value {
    let exe = me().display().to_string();
    match format {
        // Nothing quoted, and a path chosen so that nothing needs to be
        HookFormat::Bare => serde_json::json!({
            "type": "command",
            "command": format!("{} --hook {arg}", spaceless(&me()).unwrap_or(exe)),
            "timeout": timeout,
            "async": true,
        }),
        HookFormat::Args => serde_json::json!({
            "type": "command",
            "command": exe,
            "args": ["--hook", arg],
            "timeout": timeout,
            "async": true,
        }),
        // One command line, so the path is quoted: on Windows it usually has a
        // space in it, and an unquoted one would be read as two arguments
        HookFormat::Shell => serde_json::json!({
            "type": "command",
            "command": format!("\"{exe}\" --hook {arg}"),
            "timeout": timeout,
            "async": true,
        }),
    }
}

/// Whether this handler is one of ours, wherever the app now lives.
fn is_ours(h: &serde_json::Value) -> bool {
    let line = h.get("command").and_then(|c| c.as_str()).unwrap_or_default();
    let args = h
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let both = format!("{line} {args}");
    both.contains(MARK) && both.to_ascii_lowercase().contains("shikisha")
}

/// Whether a handler of ours is exactly one of the ones we would write today
/// -- same place, same argument, same way of running it, same patience.
///
/// Asked against this target's whole wish for that event rather than against
/// one argument, because one event can be asked for more than one thing.
/// Asked against this target rather than against any spelling we know, so that
/// an entry written for a CLI that has since changed its mind reads as out of
/// date and gets rewritten
fn is_current(wanted: &[serde_json::Value], h: &serde_json::Value) -> bool {
    wanted.contains(h)
}

/// What the file says about us, without changing anything.
pub fn status(t: &Target) -> Status {
    let Ok(text) = std::fs::read_to_string(&t.file) else {
        return Status::NoConfig;
    };
    let doc: serde_json::Value = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(e) => return Status::Unreadable(e.to_string()),
    };
    let mut seen = 0;
    let mut current = 0;
    // Once per event, not once per thing wanted from it: an event asked for
    // two things was being read twice, and counted both of its handlers each
    // time -- so a correctly installed entry never added up
    for (event, hs) in wanted(t) {
        for group in doc
            .pointer(&format!("/hooks/{event}"))
            .and_then(|g| g.as_array())
            .into_iter()
            .flatten()
        {
            for h in group.pointer("/hooks").and_then(|h| h.as_array()).into_iter().flatten() {
                if is_ours(h) {
                    seen += 1;
                    if is_current(&hs, h) {
                        current += 1;
                    }
                }
            }
        }
    }
    match (seen, current) {
        (0, _) => Status::Absent,
        (s, c) if s == c && c == t.entries.len() => Status::Installed,
        _ => Status::Stale,
    }
}

/// Everything we want from one event, gathered.
///
/// One event can be asked for more than one thing -- Gemini CLI has four
/// events in total, so the start of a turn has to carry both "this is the
/// conversation" and "it is working" -- and writing them one at a time meant
/// each one removed the last. Gathering first is what makes that impossible
fn wanted(t: &Target) -> Vec<(String, Vec<serde_json::Value>)> {
    let mut out: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    for entry in &t.entries {
        let h = handler(t.format, t.timeout, &entry.arg);
        match out.iter_mut().find(|(event, _)| *event == entry.event) {
            Some((_, list)) => list.push(h),
            None => out.push((entry.event.clone(), vec![h])),
        }
    }
    out
}

/// Exactly what would be added, for a person to read before agreeing to it.
///
/// Shown rather than described: this writes into someone else's config, and
/// "trust me" is not an acceptable substitute for the four lines involved
pub fn preview(t: &Target) -> String {
    let mut hooks = serde_json::Map::new();
    for (event, hs) in wanted(t) {
        hooks.insert(event, serde_json::json!([{ "hooks": hs }]));
    }
    serde_json::to_string_pretty(&serde_json::json!({ "hooks": hooks }))
        .unwrap_or_default()
}

/// Put our entry in (or bring it up to date), leaving everything else alone.
pub fn install(t: &Target) -> Result<()> {
    edit(t, true)
}

/// Take our entry out, leaving everything else alone.
pub fn uninstall(t: &Target) -> Result<()> {
    edit(t, false)
}

fn edit(t: &Target, want: bool) -> Result<()> {
    // Said before anything is written, not discovered later by a dot that
    // never moves: a CLI that will not take quotes cannot be handed a path
    // with a space in it, and on a volume with no short names there is no
    // second spelling to fall back on
    if want && t.format == HookFormat::Bare && spaceless(&me()).is_none() {
        anyhow::bail!(crate::i18n::tp(
            "err.hook.path_has_space",
            &[("name", &t.name), ("path", &me().display().to_string())]
        ));
    }
    let existing = std::fs::read_to_string(&t.file).ok();
    let mut doc: serde_json::Value = match existing.as_deref() {
        Some(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| {
            crate::i18n::tp(
                "err.hookfile.unreadable",
                &[("path", &t.file.display().to_string())],
            )
        })?,
        None if want => serde_json::json!({}),
        // Nothing to remove from a file that isn't there
        None => return Ok(()),
    };
    if !doc.is_object() {
        anyhow::bail!(crate::i18n::tp(
            "err.hookfile.unreadable",
            &[("path", &t.file.display().to_string())]
        ));
    }

    for (event, hs) in wanted(t) {
        let list = doc
            .as_object_mut()
            .and_then(|o| o.entry("hooks").or_insert_with(|| serde_json::json!({})).as_object_mut())
            .map(|h| h.entry(event).or_insert_with(|| serde_json::json!([])));
        let Some(slot) = list else { continue };
        if !slot.is_array() {
            *slot = serde_json::json!([]);
        }
        let groups = slot.as_array_mut().expect("just made an array");
        // Ours goes, whether we are replacing it or removing it. Everyone
        // else's stays exactly where it was
        for group in groups.iter_mut() {
            if let Some(hs) = group.pointer_mut("/hooks").and_then(|h| h.as_array_mut()) {
                hs.retain(|h| !is_ours(h));
            }
        }
        groups.retain(|g| {
            g.pointer("/hooks")
                .and_then(|h| h.as_array())
                .map(|h| !h.is_empty())
                .unwrap_or(true)
        });
        if want {
            groups.push(serde_json::json!({ "hooks": hs }));
        }
    }
    // Leave no empty scaffolding behind after a removal — including the map
    // itself when we were the only thing in it, which is the whole file for a
    // CLI whose hook config exists because we made it
    if !want
        && let Some(hooks) = doc.get_mut("hooks").and_then(|h| h.as_object_mut())
    {
        hooks.retain(|_, v| !v.as_array().map(|a| a.is_empty()).unwrap_or(false));
        let empty = hooks.is_empty();
        if let Some(o) = doc.as_object_mut().filter(|_| empty) {
            o.remove("hooks");
        }
    }

    if let Some(dir) = t.file.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    // The way back, before the first change
    if let Some(text) = existing.as_deref() {
        let _ = std::fs::write(t.file.with_extension("bak"), text);
    }
    crate::crypto::write_atomic(&t.file, &serde_json::to_string_pretty(&doc)?)?;
    crate::append_hook_log(&format!(
        "{} hook {} in {}",
        t.name,
        if want { "installed" } else { "removed" },
        t.file.display()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dir: &std::path::Path, format: HookFormat) -> Target {
        Target {
            name: "Test CLI".into(),
            file: dir.join("hooks.json"),
            format,
            timeout: TIMEOUT_S,
            entries: vec![Entry { event: "SessionStart".into(), arg: "session".into() }],
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-hook-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn installing_leaves_every_other_hook_exactly_where_it_was() {
        let dir = tmp("keep");
        let t = target(&dir, HookFormat::Args);
        let theirs = serde_json::json!({
            "model": "opus",
            "hooks": {
                "SessionStart": [{ "hooks": [{ "type": "command", "command": "their-tool" }] }],
                "Stop": [{ "hooks": [{ "type": "command", "command": "beep" }] }]
            }
        });
        std::fs::write(&t.file, serde_json::to_string_pretty(&theirs).unwrap()).unwrap();

        assert_eq!(status(&t), Status::Absent);
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(after["model"], "opus", "関係ない設定はそのまま");
        assert_eq!(after["hooks"]["Stop"], theirs["hooks"]["Stop"], "別イベントは無傷");
        let starts = after["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(starts.len(), 2, "相手のフックの隣に足す");
        assert_eq!(starts[0], theirs["hooks"]["SessionStart"][0]);

        // The file it replaced is still there to go back to
        assert!(t.file.with_extension("bak").exists());

        // Installing twice does not stack up
        install(&t).unwrap();
        let again: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(again["hooks"]["SessionStart"].as_array().unwrap().len(), 2);

        // ...and removing ours puts the file back the way it was
        uninstall(&t).unwrap();
        let back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(back, theirs, "自分の分だけを取り去る");
        assert_eq!(status(&t), Status::Absent);
    }

    /// A path with no space in it is its own space-free spelling, wherever
    /// this is running.
    ///
    /// Only Windows keeps a second name for a file, and the half that said so
    /// answered `None` to everything -- so on every other system a hook that
    /// cannot be quoted refused to install, whatever it was being pointed at,
    /// and told the person their path had a space in it when it did not.
    /// Nothing caught it because no test had installed that spelling before
    #[test]
    fn a_path_already_free_of_spaces_needs_no_second_name() {
        let plain = std::env::temp_dir().join("shikisha-plain").join("app");
        assert!(!plain.display().to_string().contains(' '), "前提: {plain:?}");
        assert_eq!(spaceless(&plain), Some(plain.display().to_string()));

        // And this program's own path answers, or nothing that needs the
        // unquoted spelling could ever be installed from here
        assert!(spaceless(&me()).is_some(), "自分自身を名指せない: {:?}", me());
    }

    /// One event can be asked for more than one thing, and all of it has to
    /// survive.
    ///
    /// Gemini CLI has four events in total, so the start of a turn is the only
    /// place to say both "this is the conversation" and "it is working".
    /// Written one at a time, each removed the one before it: only the last
    /// was installed, the session was never reported, and the entry could
    /// never read as up to date because one of the three was always missing
    #[test]
    fn an_event_asked_for_two_things_keeps_both() {
        let dir = tmp("twice");
        let t = Target {
            name: "Test CLI".into(),
            file: dir.join("hooks.json"),
            format: HookFormat::Bare,
            timeout: 3_000,
            entries: vec![
                Entry { event: "BeforeAgent".into(), arg: "session".into() },
                Entry { event: "BeforeAgent".into(), arg: "state:BUSY".into() },
                Entry { event: "AfterAgent".into(), arg: "state:DONE".into() },
            ],
        };
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed, "3つ書いたなら3つ揃っている");

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        let said = |event: &str| {
            after["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|g| g["hooks"].as_array().unwrap().clone())
                .map(|h| h["command"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        };
        let first = said("BeforeAgent");
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(first.iter().any(|c| c.ends_with("--hook session")), "{first:?}");
        assert!(first.iter().any(|c| c.ends_with("--hook state:BUSY")), "{first:?}");
        assert_eq!(said("AfterAgent").len(), 1);

        // What the person was shown is what was written
        let shown: serde_json::Value = serde_json::from_str(&preview(&t)).unwrap();
        assert_eq!(shown["hooks"]["BeforeAgent"][0]["hooks"].as_array().unwrap().len(), 2);

        // And taking it out takes out both
        uninstall(&t).unwrap();
        assert_eq!(status(&t), Status::Absent);
    }

    /// Each event carries what this app will make of it, in the entry itself.
    /// A person reading their own settings file should not have to be told
    /// which of their CLI's events this app treats as "waiting for you"
    #[test]
    fn every_event_is_written_with_the_meaning_it_was_given() {
        let dir = tmp("states");
        let t = Target {
            name: "Test CLI".into(),
            file: dir.join("hooks.json"),
            format: HookFormat::Args,
            timeout: TIMEOUT_S,
            entries: vec![
                Entry { event: "SessionStart".into(), arg: "session".into() },
                Entry { event: "UserPromptSubmit".into(), arg: "state:BUSY".into() },
                Entry { event: "PermissionRequest".into(), arg: "state:QUESTION".into() },
                Entry { event: "Stop".into(), arg: "state:DONE".into() },
            ],
        };
        assert_eq!(status(&t), Status::NoConfig);
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);

        let made: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        let arg_of = |event: &str| -> String {
            made["hooks"][event][0]["hooks"][0]["args"][1].as_str().unwrap().to_string()
        };
        assert_eq!(arg_of("UserPromptSubmit"), "state:BUSY");
        assert_eq!(arg_of("PermissionRequest"), "state:QUESTION");
        assert_eq!(arg_of("Stop"), "state:DONE");
        assert_eq!(arg_of("SessionStart"), "session");
        // Nothing that fires on every turn may be waited for
        assert_eq!(made["hooks"]["Stop"][0]["hooks"][0]["async"], serde_json::json!(true));

        // An entry that says the right thing for the wrong event is not ours
        // to keep: it would report "working" where a turn ends
        let mut crossed = made.clone();
        crossed["hooks"]["Stop"][0]["hooks"][0]["args"] =
            serde_json::json!(["--hook", "state:BUSY"]);
        std::fs::write(&t.file, serde_json::to_string_pretty(&crossed).unwrap()).unwrap();
        assert_eq!(status(&t), Status::Stale, "意味が違うものは入れ直す対象");
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);
        assert_eq!(
            std::fs::read_to_string(&t.file)
                .map(|s| s.matches("--hook").count())
                .unwrap(),
            4,
            "入れ直しても増えない"
        );

        uninstall(&t).unwrap();
        let back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(back, serde_json::json!({}), "全イベントから引き上げる");
    }

    /// The profiles that ship with the app are read here rather than described:
    /// a state name nobody implements would install a hook that fires into
    /// nothing, and the failure would be a dot that simply never moves
    #[test]
    fn the_profiles_that_ship_name_states_this_app_knows() {
        for p in crate::profile::all() {
            let Some(hook) = p.resume.as_ref().and_then(|r| r.hook.as_ref()) else {
                continue;
            };
            for (event, state) in &hook.states {
                assert!(
                    crate::detect::TabState::from_label(state).is_some(),
                    "{}: {event} says {state:?}, which is not a state",
                    p.name
                );
            }
        }
    }

    /// Measured against Codex CLI 0.150/0.151 on Windows, which splits the
    /// command line itself: a quoted path never ran, the same path bare ran,
    /// and neither spelling ran when the path contained a space. So the bare
    /// form must carry no quotes at all, and a path with a space in it has to
    /// be written in Windows' second, space-free spelling.
    #[test]
    fn the_bare_form_carries_no_quotes() {
        let h = handler(HookFormat::Bare, TIMEOUT_S, "state:DONE");
        let line = h["command"].as_str().unwrap();
        assert!(!line.contains('"'), "引用符があると起動しない: {line}");
        assert!(line.ends_with(" --hook state:DONE"), "{line}");
        assert!(h.get("args").is_none(), "一行形式に args は書かない");
        // A CLI that waits on this is waiting for nothing; and above three
        // seconds Codex complains at every launch
        assert_eq!(h["async"], serde_json::json!(true));
        assert_eq!(h["timeout"], serde_json::json!(3));
        // The program part is the half that must survive being split on
        // spaces. (Where the exe lives during a test run has none, so this
        // checks the rule rather than the workaround)
        let program = line.trim_end_matches(" --hook state:DONE");
        assert!(!program.contains(' '), "空白のあるパスは分割されて見失われる: {program}");
    }

    #[test]
    fn a_config_that_cannot_be_read_is_not_written_over() {
        let dir = tmp("broken");
        let t = target(&dir, HookFormat::Args);
        std::fs::write(&t.file, "{ this is not json").unwrap();
        assert!(matches!(status(&t), Status::Unreadable(_)));
        assert!(install(&t).is_err(), "読めない設定は書き換えない");
        assert_eq!(
            std::fs::read_to_string(&t.file).unwrap(),
            "{ this is not json",
            "一文字も変わっていない"
        );
    }

    #[test]
    fn a_missing_config_is_created_from_nothing() {
        let dir = tmp("fresh");
        let t = target(&dir.join("deeper"), HookFormat::Shell);
        assert_eq!(status(&t), Status::NoConfig);
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);
        let made: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        let h = &made["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(h["type"], "command");
        let line = h["command"].as_str().unwrap();
        assert!(line.starts_with('"') && line.contains("--hook session"), "{line}");
        assert!(h.get("args").is_none(), "一行形式に args は書かない");
    }

    #[test]
    fn an_entry_pointing_at_the_old_place_is_noticed() {
        let dir = tmp("moved");
        let t = target(&dir, HookFormat::Args);
        let old = serde_json::json!({
            "hooks": { "SessionStart": [{ "hooks": [{
                "type": "command",
                "command": "C:\\\\somewhere\\\\else\\\\SHIKISHA-TERM.exe",
                "args": ["--hook", "session"],
                "timeout": 5
            }] }] }
        });
        std::fs::write(&t.file, serde_json::to_string_pretty(&old).unwrap()).unwrap();
        assert_eq!(status(&t), Status::Stale, "動かした後は古い場所を指したまま");
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed, "入れ直すと今の場所を指す");
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(
            after["hooks"]["SessionStart"].as_array().unwrap().len(),
            1,
            "古い方は残さない"
        );
    }
}
