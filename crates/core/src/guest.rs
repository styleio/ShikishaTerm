//! Which AI is running inside a tab that was not started as one.
//!
//! A tab is its command: open a shell and the tab is a shell, for as long as
//! it lives. But a person who opens a shell and types `claude` is looking at
//! an AI, and the tab went on saying "shell" -- no mark, no state, and the
//! emergency stop had nothing to press. What that person did is the ordinary
//! way to start one; the tab simply was not watching.
//!
//! So this asks the question the tab could not: **what is running in there
//! now**. Two answers, in this order.
//!
//!   1. **The processes the tab started.** They are in the tab's job, which
//!      means the list is exactly this tab's and cannot include anything else
//!      on the computer. A name is matched against the same `command_match`
//!      the tab's own command is matched against -- one table, not two
//!   2. **The window title.** Weaker on purpose: anything in the tab can write
//!      any title it likes. It is here because it is the only answer that
//!      survives a trip -- over ssh, into a container, into WSL, the processes
//!      are on the other side and the title still arrives
//!
//! Neither answer is allowed near a fence. What is running in a tab decides
//! what is drawn and how the screen is read; who may do what is decided by the
//! command the tab was started with, which nothing in here can change. A title
//! is a string the tab itself chose, and a fence that can be talked out of the
//! way is not one.
//!
//! The awkward case is the CLI that is not a program but a script: `codex` and
//! `gemini` are both `node` with a file to run, so the process name is `node`
//! and says nothing. For those the command line is read, and the rule that
//! keeps it honest is that **only a token that looks like a path** may name
//! the CLI. A question that mentions codex is a question, not a program.

/// Programs that run somebody else's code, so their own name says nothing
/// about what is running.
///
/// The shells are here as well as the two that host AI CLIs today. A wrapper
/// that runs a script is the same shape whatever it is written in, and one
/// missing from this list is a CLI this cannot see.
const INTERPRETERS: &[&str] = &[
    "node", "python", "python3", "bash", "zsh", "sh", "fish", "pwsh", "powershell", "cmd",
];

/// Options whose value is source code rather than a file. Once one of these
/// appears there is no file to find, and whatever follows is not a path
const SOURCE_OPTIONS: &[&str] = &["-e", "--eval", "-p", "--print", "--check"];

/// Options that take a value in the next word. The value is skipped rather
/// than read: it is a file, but it is a file this program was told to load
/// first, not the one it was told to run
const VALUE_OPTIONS: &[&str] = &["-r", "--require", "--import", "--loader", "--experimental-loader"];

/// What is running inside this tab, as the CLI's own command name (`claude`),
/// or `None` when the answer is "whatever the tab was started with".
///
/// The command name rather than the profile's title because it is the name
/// every other part of this program already uses for a CLI -- the mark on the
/// tab, the usage meter, the profile file itself -- so nothing has to be
/// translated on the way out.
///
/// `pids` are the tab's own processes (`job::Job::pids`); `title` is the
/// window title as the tab last saw it.
pub fn spot(pids: &[u32], title: &str) -> Option<String> {
    for &pid in pids {
        if let Some(name) = of_process(pid) {
            return Some(name);
        }
    }
    of_title(title)
}

/// One process's answer: its own name first, then -- only if that name is a
/// program that runs other people's code -- the file it was told to run.
fn of_process(pid: u32) -> Option<String> {
    let image = image_of(pid)?;
    let leaf = leaf_of(&image);
    if let Some(name) = of_program(&leaf) {
        // A one-shot run prints an answer and ends. It is a command somebody
        // ran, not a conversation to mark a tab with
        let line = command_line_of(pid).unwrap_or_default();
        return (!one_shot(&name, &split(&line))).then_some(name);
    }
    if !INTERPRETERS.contains(&leaf.as_str()) {
        return None;
    }
    let line = command_line_of(pid)?;
    let tokens = split(&line);
    let entry = entry_of(&tokens)?;
    let name = of_program(&leaf_of(&entry)).or_else(|| of_script(&entry))?;
    (!one_shot(&name, &tokens)).then_some(name)
}

/// The program name a path or a command word comes down to: no folders, no
/// quotes, no extension, lower case. `"C:\npm\claude.CMD"` and `claude` are
/// the same program written twice
fn leaf_of(token: &str) -> String {
    let bare = token.trim().trim_matches(['"', '\'']);
    let leaf = bare.rsplit(['/', '\\']).next().unwrap_or(bare).to_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(cut) = leaf.strip_suffix(ext) {
            return cut.to_string();
        }
    }
    leaf
}

/// The name a profile is known by, which is the command that starts it.
///
/// A profile with no command cannot be spotted running: there would be no
/// name to answer with, and no file for the detector to be loaded from
fn key(pf: &crate::profile::ProfileFile) -> Option<String> {
    pf.command_match.first().map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty())
}

/// The profile whose command this program name is.
///
/// Matched whole rather than as a substring: the tab's own command is matched
/// loosely, because a person writing a command means the thing they named, but
/// a name read off a running process is already exact, and `codex-helper` is
/// not codex
fn of_program(leaf: &str) -> Option<String> {
    if leaf.is_empty() {
        return None;
    }
    crate::profile::files()
        .into_iter()
        .find(|pf| pf.command_match.iter().any(|m| m.trim().eq_ignore_ascii_case(leaf)))
        .and_then(|pf| key(&pf))
}

/// The profile whose script this file is.
///
/// Compared as a whole path, with the slashes made to agree and the case
/// dropped. The mark is the package this CLI is installed as, so a file of
/// somebody's own that happens to be called `codex.js` is not it
fn of_script(entry: &str) -> Option<String> {
    let path = entry.trim().trim_matches(['"', '\'']).replace('\\', "/").to_lowercase();
    if path.is_empty() {
        return None;
    }
    crate::profile::files()
        .into_iter()
        .find(|pf| {
            pf.script_match.iter().any(|m| {
                let m = m.trim().replace('\\', "/").to_lowercase();
                !m.is_empty() && path.contains(&m)
            })
        })
        .and_then(|pf| key(&pf))
}

/// The profile whose mark this window title carries.
///
/// The weakest answer there is, and the last one asked. Matched against the
/// whole title because a CLI that writes one writes its own name into it; what
/// this must not do is claim a tab because the word appeared in a question
/// somebody typed, which is why a profile says which titles are its own rather
/// than this guessing from the CLI's name
pub fn of_title(title: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    crate::profile::files()
        .into_iter()
        .find(|pf| pf.title_match.iter().any(|m| !m.trim().is_empty() && title.contains(m.trim())))
        .and_then(|pf| key(&pf))
}

/// Whether this command prints one answer and ends.
///
/// `claude -p "..."` is a command somebody ran, the same as `grep`. Marking
/// the tab as an AI for the second it takes would be a mark that means nothing,
/// and the notice that comes with the first one would fire on it
fn one_shot(name: &str, tokens: &[String]) -> bool {
    let Some(flags) = crate::profile::files()
        .into_iter()
        .find(|pf| key(pf).as_deref() == Some(name))
        .map(|pf| pf.one_shot)
    else {
        return false;
    };
    if flags.is_empty() {
        return false;
    }
    for token in tokens.iter().skip(1) {
        // Everything after `--` is what the CLI was given, not how it was
        // asked. A question that reads like a flag is still a question
        if token == "--" {
            return false;
        }
        let head = token.split_once('=').map_or(token.as_str(), |(h, _)| h);
        if flags.iter().any(|f| f.trim() == head) {
            return true;
        }
    }
    false
}

/// The file an interpreter was told to run, out of the words it was given.
///
/// The rule that matters is the last one: a word only counts as a file when it
/// **looks like** one. Without it, every word of every question typed at an AI
/// would be read as a program name, and a tab would become whatever the person
/// happened to be asking about
fn entry_of(tokens: &[String]) -> Option<String> {
    let mut i = 1;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        if token == "--" {
            i += 1;
            continue;
        }
        if token.starts_with('-') {
            let head = token.split_once('=').map_or(token, |(h, _)| h);
            if SOURCE_OPTIONS.contains(&head) {
                return None;
            }
            if VALUE_OPTIONS.contains(&head) && head == token {
                i += 1;
            }
            i += 1;
            continue;
        }
        if token.contains('/')
            || token.contains('\\')
            || [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs", ".cjs", ".py"]
                .iter()
                .any(|e| token.to_lowercase().ends_with(e))
        {
            return Some(token.to_string());
        }
        i += 1;
    }
    None
}

/// A command line as the words it was built from.
///
/// Quotes group, and a quote inside a word ends where it began. This is not a
/// shell -- nothing is expanded and nothing is run -- it only has to break the
/// line where the program that reads it would
fn split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None => word.push(c),
        }
    }
    if started || !word.is_empty() {
        out.push(word);
    }
    out
}

/// Where a running process's program lives on disk.
#[cfg(windows)]
fn image_of(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    // The weakest right that answers this question. A process this program is
    // not allowed to look at simply has no answer, which is the right outcome
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    // SAFETY: the handle is open for the duration, and the buffer's length is
    // passed with it and written back
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) };
    unsafe { CloseHandle(handle) };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

#[cfg(not(windows))]
fn image_of(_pid: u32) -> Option<String> {
    None
}

#[cfg(windows)]
#[repr(C)]
struct UnicodeString {
    length: u16,
    capacity: u16,
    buffer: *mut u16,
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: *mut std::ffi::c_void,
        class: u32,
        info: *mut std::ffi::c_void,
        len: u32,
        written: *mut u32,
    ) -> i32;
}

/// The words a running process was started with.
///
/// Asked of the process itself rather than of the machine's process table:
/// the table is a scan of everything running, and this is one question about
/// one process, asked while a person waits for a tab to show a mark.
#[cfg(windows)]
fn command_line_of(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    /// `ProcessCommandLineInformation`, which hands the line over as it is
    /// rather than making this program read another one's memory to find it
    const COMMAND_LINE: u32 = 60;
    const LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = vec![0u8; 1024];
    let mut answer = None;
    for _ in 0..2 {
        let mut written = 0u32;
        // SAFETY: the handle is open for the duration; the buffer is at least
        // as large as the length passed with it, and the call writes how much
        // room it actually needed
        let status = unsafe {
            NtQueryInformationProcess(
                handle,
                COMMAND_LINE,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut written,
            )
        };
        if status == LENGTH_MISMATCH && written as usize > buf.len() {
            buf = vec![0u8; written as usize];
            continue;
        }
        if status != 0 {
            break;
        }
        // SAFETY: a successful call wrote the string's head at the front of
        // the buffer, and its text somewhere inside the same buffer
        let s = unsafe { &*buf.as_ptr().cast::<UnicodeString>() };
        if !s.buffer.is_null() {
            // SAFETY: the length is in bytes, as this string type counts, and
            // the text it points at lives in the buffer above
            let text = unsafe {
                std::slice::from_raw_parts(s.buffer, usize::from(s.length) / 2)
            };
            answer = Some(String::from_utf16_lossy(text));
        }
        break;
    }
    unsafe { CloseHandle(handle) };
    answer
}

#[cfg(not(windows))]
fn command_line_of(_pid: u32) -> Option<String> {
    None
}

/// One tab's standing question: who is in there now.
///
/// Kept beside the tab rather than asked afresh every time the screen is read.
/// Looking costs a question per process, and a tab is read many times a
/// second, so the answer is only looked for again when something could have
/// changed it: the job gained or lost a process, or the title moved. Both are
/// numbers the tab already has.
#[derive(Default)]
pub struct Watch {
    /// The job's population at the last look
    seen: Option<u32>,
    /// The title at the last look
    title: String,
    /// The answer, while it stands
    who: Option<String>,
}

impl Watch {
    /// Who is in there, by command name, while somebody is
    pub fn who(&self) -> Option<&str> {
        self.who.as_deref()
    }

    /// Whether anything has happened that could change the answer.
    ///
    /// Asked first so the looking itself can be skipped: a tab where nothing
    /// has started, ended or renamed itself is a tab whose answer cannot have
    /// moved
    pub fn due(&self, active: Option<u32>, title: &str) -> bool {
        active != self.seen || title != self.title
    }

    /// Look again, and say whether the answer changed.
    ///
    /// A changed answer is what makes the tab hand the screen to somebody
    /// else, so it is reported rather than left to be noticed
    pub fn settle(&mut self, active: Option<u32>, title: &str, pids: &[u32]) -> bool {
        self.seen = active;
        self.title.clear();
        self.title.push_str(title);
        let now = spot(pids, title);
        let changed = now != self.who;
        self.who = now;
        changed
    }

    /// Forget what was seen, so the next look starts from nothing. For a tab
    /// that has just been restarted or pointed at a different profile
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names of the same program, written every way a process table and a
    /// command line write them
    #[test]
    fn a_program_is_the_same_program_however_its_path_is_spelled() {
        for spelling in [
            "claude",
            "claude.exe",
            "CLAUDE.EXE",
            r"C:\Users\x\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe",
            "\"C:\\Program Files\\claude.cmd\"",
            "/usr/local/bin/claude",
        ] {
            assert_eq!(leaf_of(spelling), "claude", "{spelling} is claude");
        }
    }

    /// A command line is broken where the program that reads it breaks one
    #[test]
    fn a_command_line_comes_apart_where_the_quotes_say() {
        assert_eq!(
            split(r#""C:\npm\node.exe"  "C:\npm\node_modules\@openai\codex\bin\codex.js" resume"#),
            vec![
                r"C:\npm\node.exe",
                r"C:\npm\node_modules\@openai\codex\bin\codex.js",
                "resume"
            ]
        );
        // An empty argument was written on purpose and is still an argument
        assert_eq!(split(r#"node script.js "" last"#), vec!["node", "script.js", "", "last"]);
        assert!(split("   ").is_empty());
    }

    /// The whole point of the path test: what an AI was asked about must never
    /// decide what the tab is
    #[test]
    fn a_question_that_names_a_cli_is_a_question_and_not_a_program() {
        let asked = split(r#"node app.js "compare codex and claude for me""#);
        assert_eq!(entry_of(&asked).as_deref(), Some("app.js"));
        // ...and with no file at all in it, there is nothing to find
        assert_eq!(entry_of(&split("node -e \"require('codex')\"")), None);
        assert_eq!(entry_of(&split("node --eval=codex")), None);
    }

    /// The words in front of the file are how it was asked to run, not what it
    /// is
    #[test]
    fn what_is_loaded_first_is_not_what_is_run() {
        let line = split(r"node --require C:\hooks\pre.js --enable-source-maps C:\npm\node_modules\@openai\codex\bin\codex.js");
        assert_eq!(
            entry_of(&line).as_deref(),
            Some(r"C:\npm\node_modules\@openai\codex\bin\codex.js")
        );
        // `--` ends the options and the file can follow it
        assert_eq!(entry_of(&split("node -- ./cli.js")).as_deref(), Some("./cli.js"));
    }

    /// Read against the profiles this repo ships: the two CLIs that are a
    /// script rather than a program have to be found through the file they run
    #[test]
    fn the_clis_that_are_really_node_are_found_by_the_package_they_live_in() {
        assert_eq!(
            of_script(r"C:\Users\x\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js"),
            Some("codex".into())
        );
        assert_eq!(
            of_script("/usr/lib/node_modules/@google/gemini-cli/bundle/gemini.js"),
            Some("gemini".into())
        );
        // A file of somebody's own with the same name is not the CLI
        assert_eq!(of_script(r"C:\work\tools\codex.js"), None);
    }

    /// The tab's own command is matched loosely on purpose; a name read off a
    /// running process is not
    #[test]
    fn a_program_whose_name_merely_starts_the_same_is_a_different_program() {
        assert_eq!(of_program("claude"), Some("claude".into()));
        assert_eq!(of_program("claude-helper"), None);
        assert_eq!(of_program(""), None);
    }

    /// A one-shot run is a command, not a conversation
    #[test]
    fn a_printed_answer_is_not_a_tab_to_mark() {
        assert!(one_shot("claude", &split(r#"claude -p "what changed?""#)));
        assert!(one_shot("claude", &split(r#"claude --print "what changed?""#)));
        assert!(!one_shot("claude", &split("claude")));
        // After `--` it is the question, however it is spelled
        assert!(!one_shot("claude", &split(r#"claude -- "-p is the flag I mean""#)));
    }

    /// A title claims a tab only when a profile says that title is its own
    #[test]
    fn a_title_claims_a_tab_only_where_a_profile_said_it_would() {
        assert_eq!(of_title("\u{2733} Claude Code"), Some("claude".into()));
        assert_eq!(of_title(""), None);
        assert_eq!(of_title(r"C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe"), None);
    }
}

/// What the machine actually answers, asked of real running processes.
///
/// Every part above this is a string test, and string tests cannot tell
/// whether Windows will hand over a command line at all -- which is the one
/// thing the two CLIs that are really `node` depend on.
#[cfg(all(test, windows))]
mod live {
    use super::*;

    /// A folder that looks like the package a CLI is installed as, with a
    /// program in it that waits. Returns the file to run, or `None` where
    /// there is no `node` to run it -- a machine without one has nothing to
    /// say about this and must not fail for it
    fn a_package_that_waits(dir: &std::path::Path) -> Option<std::path::PathBuf> {
        if which_node().is_none() {
            return None;
        }
        let bin = dir.join("node_modules/@openai/codex/bin");
        std::fs::create_dir_all(&bin).ok()?;
        let file = bin.join("codex.js");
        // Waits without reading anything and ends on its own, so a test that
        // is killed before it can tidy up leaves nothing behind for long
        std::fs::write(&file, "setTimeout(() => {}, 30000)
").ok()?;
        Some(file)
    }

    fn which_node() -> Option<std::process::Output> {
        std::process::Command::new("node").arg("--version").output().ok().filter(|o| o.status.success())
    }

    /// The whole road, walked once: a process this tab started, found through
    /// the job, read back as the CLI it is running even though the program is
    /// called `node`.
    #[test]
    fn a_cli_that_is_really_node_is_recognised_from_a_live_process() {
        use std::os::windows::process::CommandExt as _;
        use std::process::Stdio;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let dir = std::env::temp_dir().join(format!("shikisha-guest-{}", std::process::id()));
        let Some(script) = a_package_that_waits(&dir) else {
            return;
        };
        let mut child = std::process::Command::new("node")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("node cannot start");
        let job = crate::job::Job::new().expect("a job cannot be made");
        assert!(job.take(child.id()), "it cannot be put in the job");
        // The command line is read from a process that has started, so give it
        // the moment that takes rather than assuming it is instant
        std::thread::sleep(std::time::Duration::from_millis(300));

        let pids = job.pids();
        assert!(pids.contains(&child.id()), "the job does not list what it holds: {pids:?}");
        assert_eq!(
            spot(&pids, ""),
            Some("codex".into()),
            "a running node was not read back as the CLI it is running"
        );

        let _ = child.kill();
        let _ = child.wait();
        drop(job);
        // ...and once it is gone, so is the answer
        assert_eq!(spot(&[child.id()], ""), None, "a process that ended still answers");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
