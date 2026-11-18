//! What a bug report carries: one file, made on this machine, looked at by
//! the person, and attached by them.
//!
//! A report that says "it stopped" and nothing else is one nobody can act
//! on, and the things that would say why -- the app's log, the crash log,
//! how the last run ended, how the machine was doing over the last week,
//! how full its disks are, what the update was doing -- are scattered over
//! folders a person has no reason to know. So they are put in one zip, here,
//! when the person asks for it, with what can only be read at that moment:
//! what every program on the machine is using, what Windows itself wrote
//! down over the week, what the app keeps on disk, how its settings are
//! switched. The first file in it says in words what the rest comes to.
//!
//! **Nothing is sent anywhere.** The file is handed to the person, who
//! decides whether to attach it. And before it is handed over, what would
//! open their accounts or say who and where they are is hidden: keys by
//! their shape (`crate::secretscan`), the keys this app holds by their value,
//! the keys a link or a header carries, the home folder and the user name,
//! addresses on a network, and mail addresses. What was hidden is counted
//! and said, so the person knows to look before attaching it.

use std::io::Write as _;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

/// The most of one log that goes in: the end of it, which is where the
/// trouble a person is reporting is
const LOG_LINES: usize = 4000;
const CRASH_LINES: usize = 400;
/// The most of a week's record that goes in, the newest of it
const WEEK_ROOM: usize = 1024 * 1024;

/// A key in a link (`?t=`, `?token=`, `&key=`, `?pair=`, `?code=`) or in a
/// header (`X-Token:`), whatever its shape -- this app's own keys are plain
/// lowercase hex, which no shape-based finder takes for a key
static CARRIED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)([?&](?:t|token|key|code|pair|ticket|sst|rk|ri|rs)=)[^&\s#'\x22<>]+",
        r"(?i)(\bx-token\s*[:=]\s*)\S+",
        r"(?i)(\b(?:token|password|secret|api[_-]?key)\x22?\s*[:=]\s*\x22?)[^\s\x22,}]{6,}",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("a carried-key pattern does not compile"))
    .collect()
});

/// The marks the log puts around what it quotes of somebody's words
pub const WORDS: (char, char) = ('\u{27e6}', '\u{27e7}');

/// Where the log carries what was said rather than what happened: an AI's
/// answer, a message handed from one tab to another, a job's words, what
/// git printed. The words are somebody's work -- a customer's project, a
/// file's contents -- and what is wrong with the program is in the events
/// around them, so only the event is kept
static SAID: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // Whatever the log itself marked as quoted (`crate::runtime::log_words`)
        r"()\x{27e6}[^\x{27e7}\r\n]*\x{27e7}?",
        // ...and the lines that quoted it before there were marks, or quote
        // it without them: a prompt, a note, what a tab or an AI said, what
        // git and the others printed
        r"(scheduled prompt .*? run \d+ .*?: ).+",
        r"(quick command .*? -> new tab .*?: ).+",
        r"(reply -> .*?\): ).+",
        r"(Draft tab\d+ -> tab\d+ \(depth \d+\): ).+",
        r"(note tab\d+: ).+",
        r"(^(?:\[[ \d.]+\] )?words: ).+",
        r"(survey \[[^\]]*\]: ).+",
        // What git and the others answered: what was done and whether it
        // worked is the event, and is kept; what came with it is not
        r#"(^(?:\[[ \d.]+\] )?(?:git|issues|sftp|suggest): (?:\{"act":"[a-z_]+","ok":(?:true|false))?).+"#,
        r"(\bsays: ).+",
        r"(background tab: ).+",
        r"(\[ai\] ).+",
        r"(response \d+ chars: ).+",
        r"(auto-send tab\d+ -> tab\d+ \(depth \d+\): ).+",
        r"(added to j\d+: ).+",
        r"(\x22data\x22\s*:\s*).+",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("a said pattern does not compile"))
    .collect()
});

/// What stands where words were left out
const LEFT_OUT: &str = "[words left out]";

/// The most of one line that is kept. A log line longer than this is almost
/// always one carrying somebody's words that no pattern above knew about
const LINE_KEPT: usize = 300;

/// A link: one to anywhere but this machine and this program's own home
/// says where the person's work lives (`github.com/<them>/<their project>`)
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:https?|ssh|git)://[^\s'\x22<>()\]]+").expect("link"));
const ELSEWHERE: &str = "[link]";

fn link_is_ours(link: &str) -> bool {
    let rest = link.split_once("://").map(|(_, r)| r).unwrap_or(link).to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]", "github.com/styleio/shikishaterm", "api.github.com/repos/styleio/shikishaterm"]
        .iter()
        .any(|ours| rest.starts_with(ours))
}

/// An address on a network other than this machine's own loopback
static IPV4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b").expect("ipv4"));
static MAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b").expect("mail"));

/// What stands in for a hidden address, mail address, home and user name
const IP: &str = "[address]";
const AT: &str = "[mail]";
const HOME: &str = "~";
const USER: &str = "[user]";

/// The words of a log as they may leave this machine, and how many things
/// were hidden in them
pub fn hidden(text: &str, known: &[String]) -> (String, usize) {
    hidden_lines(text, known, true)
}

/// [`hidden`], told whether a long line is cut. The app's own record of the
/// machine (`crate::health`) is written by this program alone, a number and
/// a file name at a time, and its long lines are the ones that name the
/// programs behind the trouble: they are kept whole
fn hidden_lines(text: &str, known: &[String], cut: bool) -> (String, usize) {
    let mut n = 0;
    let mut kept = String::with_capacity(text.len());
    for line in text.lines() {
        let mut line = line.to_string();
        for re in SAID.iter() {
            if re.is_match(&line) {
                n += 1;
                line = re.replace(&line, format!("${{1}}{LEFT_OUT}")).into_owned();
            }
        }
        if cut && line.chars().count() > LINE_KEPT {
            n += 1;
            line = line.chars().take(LINE_KEPT).collect::<String>() + " " + LEFT_OUT;
        }
        kept.push_str(&line);
        kept.push('\n');
    }
    let (out, more) = crate::secretscan::hide_known(&kept, known.iter().map(String::as_str));
    n += more;
    let (mut out, more) = crate::secretscan::hide(&out);
    n += more;
    for re in CARRIED.iter() {
        let found = re.find_iter(&out).count();
        if found > 0 {
            n += found;
            out = re.replace_all(&out, format!("${{1}}{}", crate::secretscan::HIDDEN)).into_owned();
        }
    }
    let mut links = 0;
    out = LINK
        .replace_all(&out, |c: &regex::Captures| {
            if link_is_ours(&c[0]) {
                c[0].to_string()
            } else {
                links += 1;
                ELSEWHERE.to_string()
            }
        })
        .into_owned();
    n += links;
    let mut ips = 0;
    out = IPV4
        .replace_all(&out, |c: &regex::Captures| {
            let whole = &c[0];
            let own = whole.starts_with("127.") || whole == "0.0.0.0" || whole.starts_with("255.");
            let octets = (1..=4).all(|i| c[i].parse::<u16>().is_ok_and(|v| v <= 255));
            if own || !octets {
                whole.to_string()
            } else {
                ips += 1;
                IP.to_string()
            }
        })
        .into_owned();
    n += ips;
    let mails = MAIL.find_iter(&out).count();
    if mails > 0 {
        n += mails;
        out = MAIL.replace_all(&out, AT).into_owned();
    }
    // Who and where: the home folder, then the name on its own (it is in
    // paths elsewhere, in the window's title, in a git author)
    if let Some(home) = home_dir() {
        out = replace_ignoring_case(&out, &home, HOME);
        out = replace_ignoring_case(&out, &home.replace('\\', "/"), HOME);
    }
    // ...as a word of its own: a name is also a piece of other words
    if let Some(user) = user_name() {
        out = replace_ignoring_case(&out, &format!(r"\b{}\b", regex::escape(&user)), USER);
    }
    (out, n)
}

/// `text` with every `from` -- in any case, as Windows paths are -- put as
/// `to`. `from` is taken as it is written, unless it is already a pattern
/// (it starts with `\b`)
fn replace_ignoring_case(text: &str, from: &str, to: &str) -> String {
    if from.chars().count() < 3 {
        return text.to_string();
    }
    let pattern = if from.starts_with(r"\b") { from.to_string() } else { regex::escape(from) };
    match Regex::new(&format!("(?i){pattern}")) {
        Ok(re) => re.replace_all(text, regex::NoExpand(to)).into_owned(),
        Err(_) => text.to_string(),
    }
}

fn home_dir() -> Option<String> {
    std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok().filter(|h| h.len() > 3)
}

fn user_name() -> Option<String> {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).ok().filter(|u| u.chars().count() >= 3)
}

/// The keys this app holds, by their value: the board's link, the door the
/// AI CLIs knock at, a link the person wrote, and whatever the secrets file
/// holds in the clear
fn known_keys() -> Vec<String> {
    let mut keys: Vec<String> = ["remote-token", "api-token"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(crate::config::state_path(f)).ok())
        .map(|s| s.trim().to_string())
        .collect();
    // The app's own are beside it and are read at once. The ones a person
    // keeps are in a file they named, which may be on a share that has
    // stopped answering: read off to one side, and done without after a
    // moment -- a key is still hidden by its shape, and in a log by not
    // being a word of the app's
    keys.extend(later("keys", kept_keys).recv_timeout(std::time::Duration::from_secs(8)).unwrap_or_default());
    keys.retain(|k| k.chars().count() >= 6);
    keys.sort();
    keys.dedup();
    keys
}

/// The keys in the settings and in the file of secrets
fn kept_keys() -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(cfg) = crate::config::load() {
        keys.push(cfg.remote.fixed_token.trim().to_string());
        if let Some(text) = cfg.secrets_path().and_then(|p| std::fs::read_to_string(p).ok())
            && !crate::crypto::is_encrypted(&text)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
        {
            strings_in(&v, &mut keys);
        }
    }
    keys.retain(|k| k.chars().count() >= 6);
    keys.sort();
    keys.dedup();
    keys
}

fn strings_in(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| strings_in(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| strings_in(x, out)),
        _ => {}
    }
}

/// The last `n` lines of a file, when there is one
fn tail(path: &std::path::Path, n: usize) -> Option<String> {
    use std::io::{Read as _, Seek as _};
    // Only its end is read: the lines wanted are the last ones, and a log
    // can be far larger than what is kept of it
    const MOST: u64 = 2 * 1024 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(std::io::SeekFrom::Start(size.saturating_sub(MOST))).ok()?;
    let mut text = Vec::new();
    file.take(MOST).read_to_end(&mut text).ok()?;
    let text = String::from_utf8_lossy(&text);
    // Begun in the middle of a line: that piece of one is left out
    let text = match size > MOST {
        true => text.split_once('\n').map_or("", |(_, rest)| rest),
        false => &text,
    };
    let lines: Vec<&str> = text.lines().collect();
    Some(lines[lines.len().saturating_sub(n)..].join("\n"))
}

/// The most files counted to say how much the folders hold, all of them
/// together, and the longest the counting may take: a folder with a million
/// files is already answered by "more than this"
const FILES_MOST: u64 = 400_000;
const COUNT_FOR: std::time::Duration = std::time::Duration::from_secs(8);

/// How much a folder holds on disk: its bytes and files, and whether the
/// counting was stopped before the end. A second name for something
/// elsewhere (a link, a junction) is not followed: what is behind it is not
/// in this folder
fn folder_size(at: &std::path::Path, left: &mut u64, until: std::time::Instant) -> (u64, u64, bool) {
    let (mut bytes, mut files) = (0u64, 0u64);
    let mut todo = vec![at.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            if *left == 0 || std::time::Instant::now() >= until {
                return (bytes, files, true);
            }
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                todo.push(e.path());
            } else if let Ok(m) = e.metadata() {
                bytes += m.len();
                files += 1;
                *left -= 1;
            }
        }
    }
    (bytes, files, false)
}

/// What a disk reached over a network is called
const SHARE: &str = "a network share";

/// The disk a folder is on, as a report may name it: its letter, or that it
/// is on a network. Never the share's name, which says where somebody works
fn disk_name(at: &std::path::Path) -> String {
    let s = at.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return format!("{}:", (b[0] as char).to_ascii_uppercase());
    }
    if s.starts_with(r"\\") || s.starts_with("UNC") {
        return SHARE.into();
    }
    "this machine's disk".into()
}

/// How full the disks are and how much the folders of the desks hold.
///
/// A folder is said by a number, not by its name: its name is a project's or
/// a customer's, and what a report needs is that one of them holds two
/// hundred gigabytes, not which. The person reading it on their own machine
/// can tell which by its size
fn storage() -> String {
    let size = crate::pressure::size;
    let mut s = String::new();
    let mut disks: Vec<(String, u64, u64)> = Vec::new();
    let mut disk = |at: &std::path::Path| {
        let name = disk_name(at);
        if let Some((free, total)) = crate::health::disk_room(at)
            && !disks.iter().any(|(n, _, t)| *n == name && *t == total)
        {
            disks.push((name, free, total));
        }
    };
    disk(&crate::config::root_dir());
    let mut folders: Vec<(std::path::PathBuf, bool)> = Vec::new();
    if let Some(cfg) = crate::config::load() {
        let (desks, _) = cfg.resolve_desks();
        for d in &desks {
            for f in &d.folders {
                // One on another machine is not on these disks
                if f.host.is_some() {
                    continue;
                }
                // One on a share is named as that and not asked anything:
                // a share that has stopped answering holds whoever asks
                if let Some(at) = &f.cwd
                    && !folders.iter().any(|(p, _)| p == at)
                {
                    folders.push((at.clone(), disk_name(at) != SHARE && crate::repo::is_linked(at)));
                }
            }
        }
    }
    for (at, _) in &folders {
        if disk_name(at) != SHARE {
            disk(at);
        }
    }
    for (name, free, total) in &disks {
        s.push_str(&format!("disk {name}: {} free of {}\n", size(*free), size(*total)));
    }
    let (mut left, until) = (FILES_MOST, std::time::Instant::now() + COUNT_FOR);
    for (i, (at, linked)) in folders.iter().enumerate() {
        let kind = if *linked { "a worktree" } else { "a folder" };
        let on = disk_name(at);
        if on == SHARE {
            s.push_str(&format!("folder {}: on {on}, not counted\n", i + 1));
            continue;
        }
        if !at.exists() {
            s.push_str(&format!("folder {}: {kind} on {on}, not there\n", i + 1));
            continue;
        }
        let (bytes, files, more) = folder_size(at, &mut left, until);
        let more = if more { "more than " } else { "" };
        s.push_str(&format!("folder {}: {kind} on {on}, {more}{} in {more}{files} files\n", i + 1, size(bytes)));
    }
    s
}

/// The longest the disks and folders are waited for: a disk that has stopped
/// answering -- a drive letter for a share whose network is gone -- holds
/// whoever asks it for as long as it likes, and the rest of the report is
/// worth having without it
const STORAGE_FOR: std::time::Duration = std::time::Duration::from_secs(15);

/// What this copy is and what it runs on, in a few lines
fn about() -> String {
    let mut s = String::new();
    let mut line = |k: &str, v: String| s.push_str(&format!("{k}: {v}\n"));
    line("version", env!("CARGO_PKG_VERSION").to_string());
    line("built", format!("{} ({})", env!("BUILD_TIME"), env!("BUILD_REV")));
    line("system", crate::webui::os_version());
    line("arch", std::env::consts::ARCH.to_string());
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    line("memory", format!("{} MB in all, {} MB free", sys.total_memory() / 1_048_576, sys.available_memory() / 1_048_576));
    line("processors", std::thread::available_parallelism().map(|n| n.get().to_string()).unwrap_or_default());
    let update = crate::update::snapshot();
    line("installed from", if update["packaged"] == serde_json::json!(true) { "the Store".into() } else { "a download".into() });
    line("update", update["phase"].as_str().unwrap_or_default().to_string());
    if let Some(cfg) = crate::config::load() {
        line("language", cfg.language.clone().unwrap_or_else(|| "auto".into()));
        line("phone access", format!("{} (fixed link: {})", cfg.remote.enabled, cfg.remote.sticky_token));
        let (desks, _) = cfg.resolve_desks();
        line("desks", desks.len().to_string());
        // What the tabs run, by program only: the arguments are where
        // addresses and names would be
        let mut programs: std::collections::BTreeMap<String, usize> = Default::default();
        for d in &desks {
            for t in &d.tabs {
                let argv = t.cfg.command.argv();
                let name = argv.first().map(|p| {
                    std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
                });
                *programs.entry(name.unwrap_or_default()).or_default() += 1;
            }
        }
        line("tabs", programs.iter().map(|(p, n)| format!("{p} x{n}")).collect::<Vec<_>>().join(", "));
    }
    if let Some(told) = crate::lastexit::told() {
        line("last run ended", format!("badly (pid {}, {}, code {})", told.mark.pid, told.ended.when, told.ended.code));
    }
    s
}

/// Something that takes a while, begun on a thread of its own: what it
/// answers is waited for no longer than whoever asks is willing to, and the
/// rest of the report is worth having without it
fn later<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::sync::mpsc::Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name(format!("shikisha-diag-{name}")).spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

/// What a program of the system's prints, given up on after `wait`
fn said_by(program: &str, args: &[&str], wait: std::time::Duration) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // No console window flashes up for it
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let read = later("said", move || {
        let mut all = Vec::new();
        let _ = out.read_to_end(&mut all);
        all
    });
    let until = std::time::Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < until => std::thread::sleep(std::time::Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    read.recv_timeout(std::time::Duration::from_secs(2)).ok()
}

/// What a console program printed, as text: in the letters the web uses
/// when it is that, otherwise in the console's own (a Japanese Windows
/// prints its own programs' words in another set)
fn console_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.replace('\0', "");
    }
    #[cfg(windows)]
    // SAFETY: the lengths given are the buffers' own
    unsafe {
        use windows_sys::Win32::Globalization::{CP_OEMCP, MultiByteToWideChar};
        let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0);
        if n > 0 {
            let mut wide = vec![0u16; n as usize];
            let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), n);
            return String::from_utf16_lossy(&wide[..n.max(0) as usize]).replace('\0', "");
        }
    }
    String::from_utf8_lossy(bytes).replace('\0', "")
}

/// A log with what it said over and over said once: the line, and under it
/// how many more times and until when. A fault that repeats every two
/// seconds is otherwise all that is left of the log's last hour
fn collapsed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last: Option<(&str, &str, usize)> = None;
    let close = |out: &mut String, last: Option<(&str, &str, usize)>| {
        if let Some((_, line, times)) = last.filter(|l| l.2 > 0) {
            let until = line.split_once("] ").map_or("", |(t, _)| t.trim_start_matches('[').trim());
            out.push_str(&format!("    (the same {times} more times, the last at {until})\n"));
        }
    };
    for line in text.lines() {
        let words = crate::findings::words(line);
        match &mut last {
            Some((same, at, times)) if *same == words && !words.is_empty() => {
                *at = line;
                *times += 1;
            }
            _ => {
                close(&mut out, last);
                out.push_str(line);
                out.push('\n');
                last = Some((words, line, 0));
            }
        }
    }
    close(&mut out, last);
    out
}

/// Over how long the programs are measured for the report
const NOW_OVER: std::time::Duration = std::time::Duration::from_secs(3);

/// What the machine and every program on it were doing as the report was
/// made: the answer to "it is slow right now", which no record kept
/// beforehand holds as fully. With it, the look and the programs using the
/// most, for the first page to read
fn now() -> (String, crate::health::Look, crate::health::Top) {
    let size = crate::pressure::size;
    let before = crate::perf::own_threads();
    let began = std::time::Instant::now();
    let (rates, procs) = crate::perf::sample(NOW_OVER);
    let after = crate::perf::own_threads();
    let over = began.elapsed().as_millis().max(1) as f64;
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let look = crate::health::Look {
        cpu: rates.cpu.unwrap_or_else(|| procs.iter().map(|p| p.cpu).sum::<f32>().min(100.0)),
        free: sys.available_memory(),
        total: sys.total_memory(),
        commit: crate::pressure::room(),
        // Read and left where it is: the record that is kept takes it
        late: crate::watchdog::late_so_far(),
        disk: crate::health::disk_room(&crate::config::root_dir()),
        kernel: rates.kernel,
        busy: rates.busy,
        queue: rates.queue,
        paged: rates.paged,
    };
    let top = crate::health::top_of(&procs, !rates.exact);
    let mut s = format!("measured over {:.1} s at {}\n", over / 1000.0, crate::hooks::local_stamp("%Y-%m-%dT%H:%M:%S"));
    s.push_str(&format!("{}\n", crate::health::line(&look, None)));
    if !rates.exact {
        s.push_str("memory here is counted with what programs share, in each of them: the copies of one program add up to more than they hold\n");
    }
    if let Some(disk) = rates.disk {
        s.push_str(&format!("the disks read and wrote {:.1} MB/s\n", disk as f64 / 1_048_576.0));
    }
    // Every program, the copies of one together
    #[derive(Default)]
    struct Sum {
        copies: u32,
        cpu: f32,
        memory: u64,
        commit: u64,
        disk: u64,
        threads: u32,
        handles: u32,
        ours: bool,
    }
    let mut by: std::collections::BTreeMap<&str, Sum> = Default::default();
    for p in &procs {
        let sum = by.entry(p.name.as_str()).or_default();
        sum.copies += 1;
        sum.cpu += p.cpu;
        sum.memory += p.memory;
        sum.commit += p.commit;
        sum.disk += p.disk;
        sum.threads += p.threads;
        sum.handles += p.handles;
        sum.ours |= p.ours;
    }
    let mut all: Vec<(&str, &Sum)> = by.iter().map(|(n, s)| (*n, s)).collect();
    all.sort_by(|a, b| b.1.cpu.partial_cmp(&a.1.cpu).unwrap_or(std::cmp::Ordering::Equal).then(b.1.memory.cmp(&a.1.memory)));
    s.push_str(&format!("\nevery program, the copies of one together ({} programs in {} copies; * is this app or something it started)\n", all.len(), procs.len()));
    s.push_str(&format!("{:<34} {:>6} {:>7} {:>10} {:>10} {:>10} {:>8} {:>8}\n", "program", "copies", "cpu %", "memory", "committed", "io MB/s", "threads", "handles"));
    for (name, sum) in &all {
        let name = format!("{}{name}", if sum.ours { "* " } else { "  " });
        s.push_str(&format!(
            "{:<34} {:>6} {:>7.1} {:>10} {:>10} {:>10.1} {:>8} {:>8}\n",
            name.chars().take(34).collect::<String>(),
            sum.copies,
            sum.cpu,
            size(sum.memory),
            size(sum.commit),
            sum.disk as f64 / 1_048_576.0,
            sum.threads,
            sum.handles
        ));
    }
    let ours: Vec<&crate::perf::Proc> = procs.iter().filter(|p| p.ours).collect();
    s.push_str(&format!("\nthis app and what it started, one by one ({})\n", ours.len()));
    s.push_str(&format!("{:<30} {:>7} {:>7} {:>9} {:>7} {:>10} {:>10} {:>10} {:>8} {:>8}\n", "program", "pid", "parent", "running", "cpu %", "memory", "committed", "io MB/s", "threads", "handles"));
    for p in &ours {
        let running = match p.age {
            a if a >= 86_400 => format!("{} d", a / 86_400),
            a if a >= 3_600 => format!("{} h", a / 3_600),
            a => format!("{} min", a / 60),
        };
        s.push_str(&format!(
            "{:<30} {:>7} {:>7} {:>9} {:>7.1} {:>10} {:>10} {:>10.1} {:>8} {:>8}\n",
            p.name.chars().take(30).collect::<String>(),
            p.pid,
            p.parent,
            running,
            p.cpu,
            size(p.memory),
            size(p.commit),
            p.disk as f64 / 1_048_576.0,
            p.threads,
            p.handles
        ));
    }
    // Which part of the app itself was working. A thread is told by the
    // first word of its name, which is the app's word for its kind: the rest
    // of a name can be the machine it speaks to. One begun while this was
    // measured has used all its time since
    let mut threads: std::collections::BTreeMap<String, (u32, f64)> = Default::default();
    for (id, (name, ms)) in &after {
        let was = before.get(id).map_or(0, |(_, was)| *was);
        let kind = name.split_whitespace().next().unwrap_or("(unnamed)").to_string();
        let sum = threads.entry(kind).or_default();
        sum.0 += 1;
        sum.1 += ms.saturating_sub(was) as f64 * 100.0 / over;
    }
    if !threads.is_empty() {
        let mut threads: Vec<(String, (u32, f64))> = threads.into_iter().collect();
        threads.sort_by(|a, b| b.1.1.partial_cmp(&a.1.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
        s.push_str(&format!("\nthe app's own threads, by kind ({} threads; % of one processor)\n", after.len()));
        for (kind, (copies, share)) in threads.iter().take(60) {
            s.push_str(&format!("{kind:<40} {copies:>4} {share:>7.1}\n"));
        }
    }
    (s, look, top)
}

/// What Windows itself wrote down: the system saying it ran out of memory
/// (and which programs held it), a disk reporting errors, the processors
/// being held back, the machine waking from sleep -- over a month, since
/// there are few of them and one is often the whole answer -- and, over the
/// week, the system's errors and the programs that crashed or stopped
/// answering. It is kept by Windows, not by this app, so it is there for
/// the days before this app kept any record, and after the app was taken
/// off and put on again; and nothing of it adds to what this app keeps.
///
/// Who was logged in and what the PC is called are left out
#[cfg(windows)]
fn events() -> String {
    const WEEK: &str = "TimeCreated[timediff(@SystemTime) <= 604800000]";
    const MONTH: &str = "TimeCreated[timediff(@SystemTime) <= 2592000000]";
    /// The most of one asking that goes in, the newest
    const ROOM: usize = 100 * 1024;
    // Only what is written by the parts of Windows named here. An event is
    // words, and any program may write its own: a database writes the name
    // of the account that could not log in. The ones named write which
    // program, which disk, which code -- what a fault is told by
    let asks = [
        (
            "memory, disks, processors, power and sleep, over the last month",
            "System",
            format!(
                "*[System[{MONTH} and (Provider[@Name='Microsoft-Windows-Resource-Exhaustion-Detector' or @Name='disk' or @Name='Ntfs' or @Name='volmgr' or @Name='volsnap' \
                 or @Name='stornvme' or @Name='storahci' or @Name='Microsoft-Windows-WHEA-Logger' or @Name='Microsoft-Windows-Power-Troubleshooter' \
                 or @Name='Microsoft-Windows-WER-SystemErrorReporting' or @Name='Microsoft-Windows-MemoryDiagnostics-Results' or @Name='Display'] \
                 or (Provider[@Name='Microsoft-Windows-Kernel-Processor-Power'] and EventID=37) or (Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=41) \
                 or (Provider[@Name='EventLog'] and EventID=6008))]]"
            ),
            "/c:150",
            "",
        ),
        (
            "programs that crashed or stopped answering, over the last week",
            "Application",
            format!("*[System[{WEEK} and Provider[@Name='Application Error' or @Name='Application Hang' or @Name='Windows Error Reporting']]]"),
            "/c:150",
            "",
        ),
        // Where an install or an update from the Store says why it failed.
        // Of what the Store did, only what it did with this app: asked for
        // widely, since the other apps' are not counted against it
        (
            "what the Store could not do with this app, over the last month",
            "Microsoft-Windows-AppXDeploymentServer/Operational",
            format!("*[System[{MONTH} and (Level=1 or Level=2 or Level=3)]]"),
            "/c:1500",
            "shikisha",
        ),
    ];
    let pc = std::env::var("COMPUTERNAME").ok().filter(|n| n.chars().count() >= 3);
    let mut out = String::new();
    out.push_str("What Windows wrote down, the newest first. Its times are the world's (UTC); the app's own records are in this PC's time.\n\n");
    for (what, log, ask, most, about) in asks {
        out.push_str(&format!("== {what} ==\n"));
        let Some(bytes) = said_by("wevtutil", &["qe", log, &format!("/q:{ask}"), "/f:text", most, "/rd:true"], std::time::Duration::from_secs(10)) else {
            out.push_str("Windows did not answer in time.\n\n");
            continue;
        };
        // Who was logged in and what the PC is called are not for a report
        let mut text: String = console_text(&bytes)
            .lines()
            .filter(|line| {
                let l = line.trim_start();
                !(l.starts_with("User:") || l.starts_with("User Name:") || l.starts_with("Computer:"))
            })
            .map(|line| format!("{}\n", line.trim_end()))
            .collect();
        if let Some(pc) = &pc {
            text = replace_ignoring_case(&text, &format!(r"\b{}\b", regex::escape(pc)), "[this PC]");
        }
        if !about.is_empty() {
            text = events_about(&text, about);
        }
        let mut text = events_once(&folders_out(&text).0);
        if text.len() > ROOM {
            let end = (0..=ROOM).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
            let cut = text[..end].rfind("\nEvent[").unwrap_or(0);
            text.truncate(cut);
            text.push_str("\n(the older ones are left out)\n");
        }
        out.push_str(if text.trim().is_empty() { "Nothing.\n" } else { &text });
        out.push('\n');
    }
    out
}

/// What stands for the folders of a path
const FOLDER: &str = "[a folder]";

/// The endings of files that are programs or parts of them. Such a file is
/// told by its name wherever it is: which program it was is the thing to know
const PROGRAM_ENDINGS: [&str; 8] = ["exe", "dll", "sys", "drv", "so", "dylib", "node", "com"];

/// A path as no more than its file's name, and that only when the file is a
/// program or one of the app's own: the folders above it are where somebody
/// keeps their work -- `D:\customers\<who>\app.exe` in the event of a
/// program that crashed, the folder that would not be removed in the app's
/// log -- and so is the name of a document. With it, how many paths there were
fn folders_out(text: &str) -> (String, usize) {
    // From a drive, the home folder or a share, every folder on the way and
    // then the file. A folder's name does not end in a space: the words
    // after a path do. (Written into JSON, a path has each of its marks twice)
    static WINDOWS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?:\b[A-Za-z]:[\\/]+|~\\+|\\\\\?\\UNC\\|\\\\[^\\/\s.?][^\\/\s]*[\\/]+)(?:[^\\/:*?"<>|\r\n]*[^\\/:*?"<>|\r\n\s][\\/]+)*([^\\/:*?"<>|\r\n\s,;)\]'`]*)"#).expect("the path pattern does not compile")
    });
    // ...and on the other systems, from the top or the home folder: any
    // path of two parts or more, but for the system's own places and the
    // app's own addresses, which are nobody's
    static ELSEWHERE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)(^|[\s"'(=:\[,])(~|/[A-Za-z0-9._-]+)(?:/[^/\s:"'<>|,;)\]]+)*/([^/\s:"'<>|,;)\]]*)"#).expect("the path pattern does not compile")
    });
    const NOBODYS: [&str; 18] = ["/api", "/ws", "/ws-state", "/cfg", "/static", "/assets", "/proc", "/dev", "/sys", "/usr", "/bin", "/sbin", "/etc", "/lib", "/lib64", "/run", "/snap", "/boot"];
    let told = |leaf: &str, by: char| match named_file(leaf) {
        true => format!("{FOLDER}{by}{leaf}"),
        false => FOLDER.to_string(),
    };
    let found = WINDOWS.find_iter(text).count();
    let out = WINDOWS.replace_all(text, |c: &regex::Captures| told(&c[1], if c[0].contains('\\') { '\\' } else { '/' }));
    let mut others = 0;
    let out = ELSEWHERE.replace_all(&out, |c: &regex::Captures| match NOBODYS.contains(&&c[2]) {
        true => c[0].to_string(),
        false => {
            others += 1;
            format!("{}{}", &c[1], told(&c[3], '/'))
        }
    });
    (out.into_owned(), found + others)
}

/// Whether a file's name is a program's or one the app gave it
fn named_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    !lower.is_empty() && (PROGRAM_ENDINGS.contains(&lower.rsplit_once('.').map_or("", |(_, e)| e)) || own_file(&lower))
}

/// The events of a log, one by one
#[cfg_attr(not(windows), allow(dead_code))]
fn events_apart(text: &str) -> Vec<Vec<&str>> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        match blocks.last_mut() {
            Some(block) if !line.starts_with("Event[") => block.push(line),
            _ => blocks.push(vec![line]),
        }
    }
    blocks
}

/// Only the events that name `word`, in any case
#[cfg_attr(not(windows), allow(dead_code))]
fn events_about(text: &str, word: &str) -> String {
    events_apart(text).iter().filter(|b| b.iter().any(|l| l.to_lowercase().contains(word))).map(|b| b.join("\n") + "\n").collect()
}

/// The events with a run of the same one said once: a processor held back
/// is written once for each processor, and thirty-two of them say no more
/// than one. The same means the same words -- two programs that crashed are
/// two events, and so are two disks that failed -- but for the number of
/// the processor in the one event that is written for each of them
#[cfg_attr(not(windows), allow(dead_code))]
fn events_once(text: &str) -> String {
    let blocks = events_apart(text);
    let what = |block: &[&str]| -> String {
        let each_processor = block.iter().any(|l| l.trim_start().starts_with("Source:") && l.contains("Kernel-Processor-Power"));
        block
            .iter()
            .filter(|l| !l.starts_with("Event[") && !l.trim_start().starts_with("Date:"))
            // A number in an event's words is which disk, which code: part
            // of what it says -- but in this one it is which processor
            .flat_map(|l| {
                let any = each_processor && !l.trim_start().starts_with("Event ID:");
                l.chars().map(move |c| if c.is_ascii_digit() && any { '#' } else { c }).chain(std::iter::once('\n'))
            })
            .collect()
    };
    let mut out = String::new();
    let mut last: Option<(String, usize)> = None;
    let close = |out: &mut String, last: &Option<(String, usize)>| {
        if let Some((_, more)) = last.as_ref().filter(|l| l.1 > 0) {
            out.push_str(&format!("(the same event {more} more times before it)\n\n"));
        }
    };
    for block in &blocks {
        let this = what(block);
        match &mut last {
            Some((same, more)) if *same == this && !this.is_empty() => *more += 1,
            _ => {
                close(&mut out, &last);
                out.push_str(&block.join("\n"));
                out.push('\n');
                last = Some((this, 0));
            }
        }
    }
    close(&mut out, &last);
    out
}

#[cfg(not(windows))]
fn events() -> String {
    String::new()
}

/// How long ago a file was changed, as a person reads it
fn ago(meta: &std::fs::Metadata) -> String {
    let Some(secs) = meta.modified().ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()) else { return "?".into() };
    match secs {
        s if s < 3_600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3_600),
        s => format!("{} d ago", s / 86_400),
    }
}

/// The folders of the app's own that are told by name: what the app put
/// there. Anything else in a folder a person can also keep things in -- the
/// folder a download was unpacked into, the settings folder, where a key
/// file of any name can be put, the logs, where a tab's record is named
/// after the tab -- is counted and not named
const OWN_FOLDERS: [&str; 14] = ["config", "data", "logs", "desks", "lang", "profiles", "bridge", "docs", "scripts", "debug", "run", "keys", "assets", "appxmetadata"];
/// The files the app gives these very names, in small letters
const OWN_NAMES: [&str; 30] = [
    "license",
    "readme.md",
    "readme.ja.md",
    "security.md",
    "third-party-notices.txt",
    "settings.cmd",
    "update-and-start.cmd",
    "start-as-new-user.cmd",
    "config.example.json",
    "secrets.example.json",
    "config.json",
    "config.json.pre-desks",
    "secrets.json",
    "ideas.json",
    "held.json",
    "keep.log",
    "keep.log.1",
    "keep.key",
    "hooks.log",
    "hooks.log.1",
    "crash.log",
    "health.log",
    "memory.log",
    "hang.json",
    "deployed-commit.txt",
    "appxmanifest.xml",
    "appxblockmap.xml",
    "appxsignature.p7x",
    "resources.pri",
    "running.json",
];
/// ...and how the names of its programs begin
const OWN_STARTS: [&str; 4] = ["shikisha", "openconsole", "conpty", "webview2loader"];

/// Whether a file's name, in small letters, is one the app gave it
fn own_file(lower: &str) -> bool {
    // (a tab's record in the logs is named after the tab, whatever the tab
    // is called: only a program, or what is put aside of one, is known by how it begins)
    let program = ["exe", "dll", "old", "pdb", "lib", "cmd"].contains(&lower.rsplit_once('.').map_or("", |(_, e)| e));
    OWN_NAMES.contains(&lower) || (program && OWN_STARTS.iter().any(|own| lower.starts_with(own)) && own_words(lower))
}

/// Which names of a folder are told
#[derive(Clone, Copy, PartialEq)]
enum Told {
    /// Only what is the app's own by its name
    Own,
    /// Everything: a folder only the app writes into
    All,
}

/// What is in a folder: each thing, how much it holds and when it changed
fn listing(title: &str, dir: &std::path::Path, told: Told, left: &mut u64, until: std::time::Instant) -> String {
    let size = crate::pressure::size;
    let mut out = format!("== {title} ==\n");
    let Ok(read) = std::fs::read_dir(dir) else {
        out.push_str("not there\n\n");
        return out;
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    let (mut others, mut other_bytes) = (0u32, 0u64);
    for e in read.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let is_dir = meta.is_dir();
        let lower = name.to_lowercase();
        let named = told == Told::All || if is_dir { OWN_FOLDERS.contains(&lower.as_str()) } else { own_file(&lower) };
        let (bytes, what) = if meta.file_type().is_symlink() {
            (0, "a link".to_string())
        } else if is_dir {
            let (bytes, files, more) = folder_size(&e.path(), left, until);
            let more = if more { "more than " } else { "" };
            (bytes, format!("{more}{} in {more}{files} files", size(bytes)))
        } else {
            (meta.len(), format!("{} KB", meta.len().div_ceil(1024)))
        };
        if named {
            rows.push((format!("{name}{}", if is_dir { "/" } else { "" }), format!("{what}, changed {}", ago(&meta))));
        } else {
            others += 1;
            other_bytes += bytes;
        }
    }
    rows.sort();
    /// The most of one folder told thing by thing
    const ROWS: usize = 80;
    for (name, what) in rows.iter().take(ROWS) {
        out.push_str(&format!("{name:<36} {what}\n"));
    }
    if rows.len() > ROWS {
        out.push_str(&format!("(and {} more of the app's own)\n", rows.len() - ROWS));
    }
    if others > 0 {
        out.push_str(&format!("({others} more that are not the app's own: {})\n", size(other_bytes)));
    }
    out.push('\n');
    out
}

/// What the app keeps on disk and how much each part of it holds: the
/// settings, the data, the logs, the program's own files (one missing is a
/// part of the app that stopped working without a word), and what the
/// resident process keeps
fn inventory() -> String {
    let (mut left, until) = (FILES_MOST / 2, std::time::Instant::now() + COUNT_FOR);
    let root = crate::config::root_dir();
    let mut out = listing("the app's folder", &root, Told::Own, &mut left, until);
    // The settings folder is one a person puts files into, and a tab's
    // record in the logs is named after the tab; the data is written by the
    // app alone, under names of its own
    for (part, told) in [("config", Told::Own), ("data", Told::All), ("logs", Told::Own)] {
        out.push_str(&listing(&format!("the app's folder: {part}"), &root.join(part), told, &mut left, until));
    }
    let program = crate::config::exe_dir();
    if program != root {
        out.push_str(&listing("the program's folder", &program, Told::Own, &mut left, until));
    }
    if let Ok(home) = crate::localkeep::home() {
        out.push_str(&listing("the resident process's folder", &home, Told::Own, &mut left, until));
        out.push_str(&listing("the resident process's folder: run", &home.join("run"), Told::Own, &mut left, until));
    }
    out
}

/// The settings whose words are safe to say: a choice out of a few the app
/// offers, never something a person typed or named -- a theme can be one
/// they made, an AI one they set up
const CHOICES: [&str; 3] = ["access", "language", "kind"];

/// The settings that are lists by name: what each is filed under is a name
/// a person chose -- a folder, a machine, a provider, a key -- and is not
/// said. Every setting of that kind in `crate::config` is here (a test
/// reads the settings' own source to see that it is)
const BY_NAME: [&str; 18] = [
    "files",
    "http",
    "folder_colors",
    "server_marks",
    "side_panels",
    "providers",
    "sign_in_labels",
    "agreed",
    "agent_hooks",
    "worktree_trust",
    "far_hooks",
    "hotkeys",
    "headers",
    "tokens",
    "descriptions",
    "meta",
    "notify",
    "binds",
];

/// The shape of the settings: every setting by its name, a switch or a
/// number as it is set, and anything a person typed -- a name, a folder, a
/// prompt, a key -- only as "text" and how long it is. Which switches are on
/// is what a fault most often turns on, and what was typed is nobody's to
/// read. `named` says the key itself is a name a person chose
fn shape(v: &serde_json::Value, key: &str, named: bool, depth: usize, out: &mut String) {
    use serde_json::Value;
    let pad = "  ".repeat(depth);
    // What is filed under this one is filed by name when the setting is a
    // list by name -- or is such a list's own list of words
    let by_name = BY_NAME.contains(&key) || (named && v.as_object().is_some_and(|o| o.values().all(Value::is_string)));
    // A name the app gave is small letters and underscores, in words of
    // its own; anything else may be a name a person gave
    let plain = !named && !key.is_empty() && key.len() <= 40 && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') && own_words(key);
    let place = key.strip_prefix('#').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    let key = if plain || place { key } else { "(a name)" };
    match v {
        Value::Null => out.push_str(&format!("{pad}{key}: not set\n")),
        Value::Bool(b) => out.push_str(&format!("{pad}{key}: {b}\n")),
        Value::Number(n) => out.push_str(&format!("{pad}{key}: {n}\n")),
        Value::String(s) => {
            let short = s.len() <= 24 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
            match CHOICES.contains(&key) && short {
                true => out.push_str(&format!("{pad}{key}: {s}\n")),
                false => out.push_str(&format!("{pad}{key}: text, {} letters\n", s.chars().count())),
            }
        }
        Value::Array(a) => {
            out.push_str(&format!("{pad}{key}: {} of them\n", a.len()));
            if depth < 6 {
                for (i, item) in a.iter().take(30).enumerate() {
                    if item.is_object() || item.is_array() {
                        shape(item, &format!("#{}", i + 1), false, depth + 1, out);
                    }
                }
            }
        }
        Value::Object(o) => {
            out.push_str(&format!("{pad}{key}:\n"));
            if depth < 6 {
                // A line beginning `//` is a note to the person reading the file
                for (k, item) in o.iter().filter(|(k, _)| !k.starts_with("//")) {
                    shape(item, k, by_name, depth + 1, out);
                }
            }
        }
    }
}

/// The settings file, as its shape (`shape`)
fn settings() -> String {
    const ROOM: usize = 60 * 1024;
    let Ok(text) = std::fs::read_to_string(crate::config::config_file_path()) else { return "there is no settings file\n".into() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return "the settings file does not read as settings\n".into() };
    let mut out = String::new();
    shape(&v, "settings", false, 0, &mut out);
    if out.len() > ROOM {
        let cut = (0..=ROOM).rev().find(|i| out.is_char_boundary(*i)).unwrap_or(0);
        out.truncate(cut);
        out.push_str("\n(the rest is left out)\n");
    }
    out
}

/// Whether a program of that name is on the PATH this process has
fn on_path(name: &str) -> bool {
    let endings: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ".ps1", ".com"] } else { &[""] };
    // A folder on a share is not asked: one that has stopped answering holds whoever asks
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).filter(|dir| disk_name(dir) != SHARE).any(|dir| endings.iter().any(|e| dir.join(format!("{name}{e}")).is_file())))
}

/// How many minutes this PC's clock is ahead of the world's (UTC)
fn ahead_of_world() -> i64 {
    let here: Vec<i64> = crate::hooks::local_stamp("%H %M").split(' ').filter_map(|n| n.parse().ok()).collect();
    let world = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64 % 86_400 / 60;
    let (Some(h), Some(m)) = (here.first(), here.get(1)) else { return 0 };
    // The nearest quarter of an hour, within the half day either side that zones span
    let by = ((h * 60 + m - world + 720).rem_euclid(1440)) - 720;
    (by as f64 / 15.0).round() as i64 * 15
}

/// More of what the app runs on, for the questions a fault raises: how long
/// the machine has been up, what powers it, whether it is being used from
/// afar, the browser the window is drawn with, what else is set around it
fn machine(git: Option<String>) -> String {
    let mut s = String::new();
    let mut line = |k: &str, v: String| s.push_str(&format!("{k}: {v}\n"));
    let up = sysinfo::System::uptime();
    line("the machine has been up", format!("{} h {} min", up / 3_600, up % 3_600 / 60));
    // The app's records are in this PC's time, Windows' own in the world's
    let ahead = ahead_of_world();
    line("this PC's time", format!("{} ({}{:02}:{:02} from UTC)", crate::hooks::local_stamp("%Y-%m-%dT%H:%M:%S"), if ahead < 0 { '-' } else { '+' }, ahead.abs() / 60, ahead.abs() % 60));
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing().with_frequency());
    if let Some(cpu) = sys.cpus().first() {
        line("processor", format!("{} ({} of them, {} MHz)", cpu.brand().trim(), sys.cpus().len(), cpu.frequency()));
    }
    if let Some((left, most)) = crate::pressure::room() {
        line("memory the system can promise", format!("{} left of {}", crate::pressure::size(left), crate::pressure::size(most)));
    }
    // The run the log counts its seconds from
    if let Some(started) = std::fs::read_to_string(crate::config::state_path("running.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["started"].as_u64())
    {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let for_ = now.saturating_sub(started);
        line("this run", format!("pid {}, started {} h {} min ago; hooks.log counts its seconds from then", std::process::id(), for_ / 3_600, for_ % 3_600 / 60));
    }
    line(
        "this PC's resident process",
        format!(
            "{}, {}, {}",
            if crate::localkeep::wanted() { "wanted" } else { "turned off" },
            if crate::localkeep::is_there() { "one is answering" } else { "none is answering" },
            if crate::localkeep::link().is_some() { "the app has its line to it" } else { "the app has no line to it" }
        ),
    );
    #[cfg(windows)]
    {
        // SAFETY: the status is ours to be written into
        let mut power: windows_sys::Win32::System::Power::SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { windows_sys::Win32::System::Power::GetSystemPowerStatus(&mut power) } != 0 {
            let from = match power.ACLineStatus {
                0 => "its battery",
                1 => "the mains",
                _ => "it does not say what",
            };
            let battery = if power.BatteryLifePercent <= 100 { format!(", battery {}%", power.BatteryLifePercent) } else { String::new() };
            // On a battery being saved, Windows holds every program back
            let saver = if power.SystemStatusFlag == 1 { ", battery saver on" } else { "" };
            line("powered by", format!("{from}{battery}{saver}"));
        }
        // SAFETY: a plain question of the system
        let afar = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows_sys::Win32::UI::WindowsAndMessaging::SM_REMOTESESSION) } != 0;
        line("used through remote desktop", afar.to_string());
        const WEBVIEW: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
        let version = [
            (winreg::enums::HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\{WEBVIEW}")),
            (winreg::enums::HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\{WEBVIEW}")),
            (winreg::enums::HKEY_CURRENT_USER, format!(r"Software\{WEBVIEW}")),
        ]
        .into_iter()
        .find_map(|(hive, key)| winreg::RegKey::predef(hive).open_subkey(key).ok()?.get_value::<String, _>("pv").ok())
        .filter(|v| !v.is_empty() && v != "0.0.0.0");
        line("the browser the window is drawn with (WebView2)", version.unwrap_or_else(|| "not found".into()));
        // What Windows is set to that decides whether a program starts, a
        // long path can be removed, or a file is read while it is written
        let machine = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);
        let number = |key: &str, name: &str| machine.open_subkey(key).ok()?.get_value::<u32, _>(name).ok();
        line(
            "paths longer than 260 letters",
            match number(r"SYSTEM\CurrentControlSet\Control\FileSystem", "LongPathsEnabled") {
                Some(1) => "allowed".into(),
                Some(_) => "not allowed".into(),
                None => "Windows does not say".into(),
            },
        );
        // It stops programs it does not know from starting, a new build of
        // this app among them
        line(
            "Smart App Control",
            match number(r"SYSTEM\CurrentControlSet\Control\CI\Policy", "VerifiedAndReputablePolicyState") {
                Some(0) => "off".into(),
                Some(1) => "on".into(),
                Some(2) => "evaluating".into(),
                Some(other) => format!("state {other}"),
                None => "Windows does not say".into(),
            },
        );
        line(
            "the virus scanner Windows comes with, as files are opened",
            match number(r"SOFTWARE\Microsoft\Windows Defender\Real-Time Protection", "DisableRealtimeMonitoring") {
                Some(1) => "turned off".into(),
                Some(_) => "on".into(),
                None => "not turned off here (another scanner may have taken its place)".into(),
            },
        );
        // SAFETY: plain questions of the system
        let (acp, oem) = unsafe { (windows_sys::Win32::Globalization::GetACP(), windows_sys::Win32::Globalization::GetOEMCP()) };
        line("letters programs are read in (code pages)", format!("{acp} for windows, {oem} for consoles"));
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN};
        // SAFETY: the same
        let (screens, w, h, all_w, all_h) = unsafe { (GetSystemMetrics(SM_CMONITORS), GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
        line("screens", format!("{screens}; the main one {w} x {h}, all of them together {all_w} x {all_h} (as this program sees them)"));
    }
    // The programs the app starts for a person, as the app would find them:
    // found or not, never where
    // (a folder on it that has stopped answering holds whoever asks it)
    let found = later("path", || {
        ["git", "node", "claude", "codex", "gemini", "gh", "pwsh", "powershell", "wsl", "ssh"].iter().map(|name| format!("{name} {}", if on_path(name) { "yes" } else { "no" })).collect::<Vec<String>>()
    });
    line(
        "found on the PATH the app was started with",
        found.recv_timeout(std::time::Duration::from_secs(4)).map(|f| f.join(", ")).unwrap_or_else(|_| "a folder on it did not answer in time".into()),
    );
    line("git", git.map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).unwrap_or_else(|| "did not answer".into()));
    // What is set around the app, by name only: a proxy changes every
    // connection it makes, and its address is not for a report
    let set: Vec<String> = std::env::vars_os()
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .map(|k| k.to_ascii_uppercase())
        .collect();
    // Only names the app or the system gave: a name somebody made up is
    // theirs, whatever it begins with
    const KNOWN: [&str; 19] = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "NODE_OPTIONS",
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "WEBVIEW2_USER_DATA_FOLDER",
        "SHIKISHA_DOWNLOADS",
        "SHIKISHA_DEBUG_STALL",
        "SHIKISHA_TAB",
        "SHIKISHA_PIPE",
        "SHIKISHA_TOKEN",
        "SHIKISHA_BRIDGE_SOCK",
        "SHIKISHA_BRIDGE_PROGRAM",
        "SHIKISHA_KEY_FILE",
        "SHIKISHA_GIT_HOST",
        "SHIKISHA_GIT_LOGIN",
        "SHIKISHA_GIT_TOKEN",
        "SHIKISHA_DIAG_OUT",
    ];
    let known: Vec<&str> = KNOWN.iter().filter(|k| set.iter().any(|s| s == *k)).cloned().collect();
    let others = set.iter().filter(|s| s.starts_with("SHIKISHA") && !KNOWN.contains(&s.as_str())).count();
    let mut said = if known.is_empty() { "none of the ones that matter".to_string() } else { known.join(", ") };
    if others > 0 {
        said.push_str(&format!("; and {others} more beginning SHIKISHA that the app does not know"));
    }
    line("set in the environment (names only)", said);
    s
}

/// Words too common to stand for somebody's own name: a desk called "test"
/// is not hidden, since hiding it would hide the word wherever it is said
const COMMON: [&str; 62] = [
    "response", "http", "https", "link", "hidden", "words", "token", "password", "secret", "node", "python", "bash", "cargo", "docker", "code", "chrome", "error", "state", "name", "path",
    "file", "line",
    "main", "master", "test", "tests", "work", "desk", "default", "check", "home", "temp", "data", "logs", "config", "user", "users", "windows", "system", "claude", "codex", "gemini",
    "shell", "powershell", "pwsh", "terminal", "browser", "develop", "development", "project", "folder", "issue", "build", "debug", "release", "local", "server", "client", "source",
    "docs", "demo", "sample",
];

/// The names a person gave to their own things -- desks, folders, tabs,
/// machines, projects, the branch each folder is on -- each with what
/// stands for it in a report. They are what a customer's name is most
/// often in, and the app knows every one of them; the logs say them all the
/// time (which desk is in front, which folder would not be removed).
///
/// A folder is numbered as `storage` numbers it, so a line of the log that
/// names "[folder 2]" is about the folder `storage.txt` says holds so much.
/// The longest first, so a folder's whole path goes before its last part
fn own_names() -> Vec<(String, String)> {
    let Some(cfg) = crate::config::load() else { return Vec::new() };
    let (desks, _) = cfg.resolve_desks();
    let mut names: Vec<(String, String)> = Vec::new();
    // The folders whose branch is asked for afterwards, each with what stands for it
    let mut on_branches: Vec<(std::path::PathBuf, String)> = Vec::new();
    let home = home_dir();
    let mut add = |name: &str, stands: &str| {
        let name = name.trim();
        // A short name in plain letters is a word of the language as often
        // as it is somebody's; one in other letters is a name at two
        let least = if name.is_ascii() { 4 } else { 2 };
        if name.chars().count() >= least && !COMMON.contains(&name.to_lowercase().as_str()) && !names.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            names.push((name.to_string(), stands.to_string()));
        }
    };
    let mut local: Vec<std::path::PathBuf> = Vec::new();
    let mut machines: Vec<String> = Vec::new();
    for (d, desk) in desks.iter().enumerate() {
        let stands = format!("[desk {}]", d + 1);
        add(&desk.name, &stands);
        add(&desk.id, &stands);
        for folder in &desk.folders {
            // The same numbers as `storage`: the folders on this machine, each once
            let stands = match (&folder.host, &folder.cwd) {
                (None, Some(at)) => {
                    let n = local.iter().position(|p| p == at).unwrap_or_else(|| {
                        local.push(at.clone());
                        local.len() - 1
                    });
                    format!("[folder {}]", n + 1)
                }
                _ => "[a folder on another machine]".to_string(),
            };
            if let Some(at) = &folder.cwd {
                // Its whole path, in every way it is written by the time the
                // names are looked for: as it is, under the home folder's
                // stand-in, with the other mark, and with each mark twice
                // (as JSON writes it)
                // (the settings may hold it with either mark; the logs write it with the system's)
                let whole = at.to_string_lossy().replace('/', std::path::MAIN_SEPARATOR_STR);
                let mut ways = vec![whole.clone()];
                if let Some(rest) = home.as_ref().filter(|h| whole.to_lowercase().starts_with(&h.to_lowercase())).and_then(|h| whole.get(h.len()..)) {
                    ways.push(format!("{HOME}{rest}"));
                }
                for way in ways {
                    add(&way, &stands);
                    add(&way.replace('\\', "/"), &stands);
                    add(&way.replace('\\', "\\\\"), &stands);
                }
                if let Some(last) = at.file_name() {
                    add(&last.to_string_lossy(), &stands);
                }
                // The branch it is on, where asking does not mean asking a share
                if folder.host.is_none() && disk_name(at) != SHARE {
                    on_branches.push((at.clone(), format!("[the branch of {}]", stands.trim_matches(['[', ']']))));
                }
            }
            for name in [&folder.name, &folder.id, &folder.project, &folder.work_item].into_iter().flatten() {
                add(name, &stands);
            }
            if let Some(host) = &folder.host {
                let n = machines.iter().position(|m| *m == host.name).unwrap_or_else(|| {
                    machines.push(host.name.clone());
                    machines.len() - 1
                });
                // Its name, and where it is reached
                for name in [&host.name, &host.at] {
                    add(name, &format!("[machine {}]", n + 1));
                }
            }
        }
        for tab in &desk.tabs {
            for name in [&tab.cfg.name, &tab.cfg.id].into_iter().flatten() {
                add(name, "[a tab]");
            }
        }
    }
    // The branches are asked of the disk, and a disk that has stopped
    // answering -- a letter for a share whose network is gone -- holds
    // whoever asks: they are waited for a moment, and done without after it
    let asked = later("branches", move || on_branches.into_iter().filter_map(|(at, stands)| Some((crate::repo::branch_of(&at)?, stands))).collect::<Vec<_>>());
    for (branch, stands) in asked.recv_timeout(std::time::Duration::from_secs(4)).unwrap_or_default() {
        add(&branch, &stands);
    }
    names.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()).then_with(|| a.0.cmp(&b.0)));
    names
}

/// `text` with each of the person's own names put as what stands for it,
/// and how many times that was done. A name is taken as a word of its own,
/// in any case
fn names_out(text: &str, names: &[(String, String)]) -> (String, usize) {
    let mut out = text.to_string();
    let mut n = 0;
    for (name, stands) in names {
        // A word of its own, where the name begins and ends in what a word is made of
        let edge = |c: Option<char>| if c.is_some_and(|c| c.is_alphanumeric() || c == '_') { r"\b" } else { "" };
        let pattern = format!("(?i){}{}{}", edge(name.chars().next()), regex::escape(name), edge(name.chars().last()));
        let Ok(re) = Regex::new(&pattern) else { continue };
        let found = re.find_iter(&out).count();
        if found > 0 {
            n += found;
            out = re.replace_all(&out, regex::NoExpand(stands)).into_owned();
        }
    }
    // What is inside one of their folders is theirs too: the folder stands,
    // and the way further in is left out
    // (a folder's name can hold spaces: taken up to what a name cannot hold)
    static INSIDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(\[folder \d+\])(?:[\\/]+[^\\/:*?"<>|\r\n]*[^\\/:*?"<>|\r\n\s])+"#).expect("the inside pattern does not compile"));
    out = INSIDE.replace_all(&out, "${1}\\...").into_owned();
    (out, n)
}

/// What kind of file a text is, which says how much of it is somebody's words
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// One of the app's logs, or what it keeps as JSON: lines the app
    /// wrote, with somebody's names and words among its own
    Log,
    /// Where the app says how it ended: what a library said as it failed,
    /// the names of functions, the programs holding the memory. Its words
    /// are not all the app's own, and they are what says what went wrong
    Ending,
    /// What Windows wrote
    Events,
    /// What this program wrote for the report, a number and a file name at
    /// a time: nothing in it is anybody's word, and a program's name in it
    /// is a program's
    Record,
}

/// What stands for a word that is not the app's own
const A_WORD: char = '\u{25af}';

/// The words this program is written in (`build.rs` lists them): the only
/// ones a report keeps of a log
static OWN_WORDS: LazyLock<std::collections::HashSet<&'static str>> = LazyLock::new(|| include_str!(concat!(env!("OUT_DIR"), "/vocabulary.txt")).lines().collect());

/// Whether a run of letters is a word of the app's own. One or two plain
/// letters say nothing of anybody
fn own_word(run: &str) -> bool {
    let known = |run: &str| (run.is_ascii() && run.len() <= 2) || OWN_WORDS.contains(run.to_lowercase().as_str());
    if known(run) {
        return true;
    }
    // Words written as one with a capital at each: what a library calls the
    // way it failed (`UnknownIssuer`). Each of them has to be known
    let mut parts: Vec<String> = Vec::new();
    for c in run.chars() {
        match parts.last_mut() {
            Some(part) if !c.is_uppercase() => part.push(c),
            _ => parts.push(c.to_string()),
        }
    }
    parts.len() > 1 && parts.iter().all(|p| p.chars().count() > 2 && known(p))
}

/// Whether every word of a name is the app's own: a name the app gave
fn own_words(name: &str) -> bool {
    name.split(|c: char| !c.is_alphabetic()).filter(|run| !run.is_empty()).all(own_word)
}

/// A log with every word that is not the app's own left out, and how many
/// there were.
///
/// A log line is the app's words with somebody's among them, and no list of
/// the places they turn up is ever whole: a desk's name between marks, a
/// branch after "on", a machine after an `@`, a file in what git said, the
/// reason a script gave. So the question is asked the other way round. The
/// words the app can write are known (`OWN_WORDS`), and a word that is not
/// one of them is somebody's: it goes, whatever line it is in. Numbers stay,
/// and so do the marks around the words -- how long a path was, that there
/// was a name there -- and a number written in letters and digits, which is
/// what the app calls a tab, a run, a key's place
fn others_out(text: &str) -> (String, usize) {
    // What names a thing by a number: left whole, or its letters would be taken for words
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F-]+|[0-9a-fA-F]{8,})\b").expect("the number pattern does not compile"));
    // A letter written as its number (`\u5f35`, as JSON writes what is
    // not plain) is read as the letter, or a name written so would pass as
    // digits; and what is written for a link, a mark and two digits a
    // letter, is no word at all
    static AS_NUMBERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\u\{?([0-9a-fA-F]{4,6})\}?").expect("the escape pattern does not compile"));
    static FOR_A_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:%[0-9a-fA-F]{2}){2,}").expect("the link pattern does not compile"));
    let text = AS_NUMBERS.replace_all(text, |c: &regex::Captures| u32::from_str_radix(&c[1], 16).ok().and_then(char::from_u32).map_or_else(|| c[0].to_string(), String::from));
    let text = FOR_A_LINK.replace_all(&text, A_WORD.to_string().as_str());
    let text: &str = &text;
    let mut out = String::with_capacity(text.len());
    let mut n = 0;
    let mut words = |piece: &str, out: &mut String| {
        let mut run = String::new();
        for c in piece.chars().chain(std::iter::once('\0')) {
            if c.is_alphabetic() {
                run.push(c);
                continue;
            }
            if !run.is_empty() {
                match own_word(&run) {
                    true => out.push_str(&run),
                    false => {
                        n += 1;
                        out.push(A_WORD);
                    }
                }
                run.clear();
            }
            if c != '\0' {
                out.push(c);
            }
        }
    };
    let mut at = 0;
    // (with a digit in it: eight letters that could all be a number's are as often a word)
    for number in NUMBER.find_iter(text).filter(|m| m.as_str().chars().any(|c| c.is_ascii_digit())) {
        words(&text[at..number.start()], &mut out);
        out.push_str(number.as_str());
        at = number.end();
    }
    words(&text[at..], &mut out);
    (out, n)
}

/// What a report may hold of a text, and how many things were hidden to
/// get there. First what `hidden_lines` hides, on the text as it was
/// written, so that nothing put in afterwards stands where a pattern looks.
/// Then the person's own names put as numbers, where there can be any.
/// Then every path told by no more than its file. Last, in a log, every
/// word that is not the app's own
fn private(text: &str, known: &[String], names: &[(String, String)], kind: Kind) -> (String, usize) {
    let (mut text, mut n) = hidden_lines(text, known, matches!(kind, Kind::Log | Kind::Ending));
    if kind != Kind::Record {
        let (named, more) = names_out(&text, names);
        (text, n) = (named, n + more);
    }
    let (mut text, paths) = folders_out(&text);
    n += paths;
    if kind == Kind::Log {
        let (own, more) = others_out(&text);
        (text, n) = (own, n + more);
    }
    (text, n)
}

/// What kind of file each one in the report is
fn kind_of(name: &str) -> Kind {
    match name {
        "health.log" | "findings.txt" | "now.txt" | "about.txt" | "storage.txt" | "files.txt" | "settings.txt" | "dumps.txt" | "short-runs.txt" => Kind::Record,
        "events.txt" => Kind::Events,
        // (where a part stopped is the names of functions: not the app's
        // vocabulary, and what the file is for)
        "crash.log" | "last-run-ended.txt" | "memory.log" | "hang.json" => Kind::Ending,
        _ => Kind::Log,
    }
}

/// The zip, its name, and how many things were hidden in it.
///
/// One saving is to be enough: whoever reads the report cannot ask the
/// person for a second one without it costing them the person's patience.
/// So everything that can be read off the machine without reading the
/// person's work is read now -- what every program is doing at this moment,
/// what Windows wrote down over the week, what the app keeps on disk and how
/// its settings are switched -- and the first file says in words what the
/// rest comes to (`crate::findings`). None of it is kept between savings:
/// the records the app writes as it runs stay as small as they were
pub fn bundle() -> Result<(String, Vec<u8>, usize)> {
    use std::time::Duration;
    // One at a time: a second one begun while the first counts the folders
    // would measure the first one's work on the disk as the machine's trouble
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let known = known_keys();
    // The settings may be on a share that has stopped answering: read off
    // to one side, and done without after a moment. Without the names, a
    // log still keeps only the app's own words
    let names = later("names", own_names).recv_timeout(Duration::from_secs(8)).unwrap_or_default();
    let mut hid = 0;
    // The moment first and by itself: counting the folders works the disk,
    // and measured beside it that work would be told as the machine's trouble
    let moment = later("now", now).recv_timeout(NOW_OVER + Duration::from_secs(20)).ok();
    // Then what takes a while, side by side
    let storage = later("storage", storage);
    let windows = later("events", events);
    let on_disk = later("files", inventory);
    let git = later("git", || said_by("git", &["--version"], Duration::from_secs(5)).map(|b| console_text(&b)));
    // The records that say when each line was written go in by the week:
    // what a person reports has often been going on for days
    let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let week = |path: std::path::PathBuf| std::fs::read_to_string(path).ok().map(|text| crate::health::recent(&text, now_secs, WEEK_ROOM)).filter(|w| !w.is_empty());
    let health = week(crate::health::file());
    let memory = week(crate::lastexit::memory_notes_file());
    // A log, with what it said over and over said once, and then its end
    let log = |path: std::path::PathBuf, lines: usize| {
        let text = collapsed(&tail(&path, lines * 5)?);
        let all: Vec<&str> = text.lines().collect();
        Some(all[all.len().saturating_sub(lines)..].join("\n")).filter(|t| !t.trim().is_empty())
    };
    let logs = crate::config::logs_dir();
    // Hidden before it is read for the first page: what the first page
    // quotes of it has to be what a person may read
    let hooks = log(logs.join("hooks.log"), LOG_LINES).map(|text| {
        let (text, n) = private(&text, &known, &names, Kind::Log);
        hid += n;
        text
    });
    let mut first = String::from("What this report comes to, read for whoever opens it. The files beside this one are what it was read from.\n\n");
    if let Some((_, look, top)) = &moment {
        first.push_str(&crate::findings::of_now(look, top));
        first.push('\n');
    }
    first.push_str(&crate::findings::of_week(health.as_deref().unwrap_or_default()));
    first.push('\n');
    first.push_str(&crate::findings::of_hangs(&crate::watchdog::last_hangs(), now_secs));
    first.push('\n');
    first.push_str(&crate::findings::of_log(hooks.as_deref().unwrap_or_default()));
    let about = later("about", about).recv_timeout(Duration::from_secs(8)).unwrap_or_else(|_| "the settings did not answer in time\n".to_string()) + &machine(git.recv_timeout(Duration::from_secs(6)).ok().flatten());
    let mut files: Vec<(&str, String)> = vec![("findings.txt", first), ("about.txt", about)];
    if let Some((text, _, _)) = moment {
        files.push(("now.txt", text));
    }
    files.push(("storage.txt", storage.recv_timeout(STORAGE_FOR).unwrap_or_else(|_| "the disks did not answer in time, and are left out\n".to_string())));
    files.push(("files.txt", on_disk.recv_timeout(COUNT_FOR + Duration::from_secs(4)).unwrap_or_else(|_| "the folders did not answer in time, and are left out\n".to_string())));
    files.push(("settings.txt", settings()));
    for (name, text) in [("health.log", health), ("memory.log", memory), ("hooks.log", hooks)] {
        if let Some(text) = text {
            files.push((name, text));
        }
    }
    // The log before this one, which the app put aside when it grew large:
    // a fault that began before then began in it
    if let Some(text) = log(logs.join("hooks.log.1"), LOG_LINES / 4) {
        files.push(("hooks.log.1", text));
    }
    // What this PC's resident process said: it is another process, and
    // writes in a folder of its own
    if let Some(text) = crate::localkeep::home().ok().and_then(|home| log(home.join("run").join("keep.log"), 600)) {
        files.push(("keep.log", text));
    }
    let state = |name: &str| crate::config::state_path(name);
    let sources: [(&str, std::path::PathBuf, usize); 6] = [
        ("crash.log", logs.join("crash.log"), CRASH_LINES),
        // The last time a part of the app stopped answering
        ("hang.json", logs.join("hang.json"), 50),
        ("running.json", state("running.json"), 50),
        ("short-runs.txt", state("short-runs"), 5),
        ("update-trial.json", state("update").join("trial.json"), 50),
        ("update-journal.json", state("update").join("journal.json"), 50),
    ];
    for (name, path, lines) in sources {
        if let Some(text) = tail(&path, lines) {
            files.push((name, text));
        }
    }
    if let Some(told) = crate::lastexit::told()
        && !told.ended.evidence.is_empty()
    {
        files.push(("last-run-ended.txt", told.ended.evidence.clone()));
    }
    // The dumps are named, not put in: they hold the memory of the
    // program's threads, which nothing here can look through for secrets.
    // The person hands one over by itself if asked for it
    let dumps: Vec<String> = crate::watchdog::tidy_dumps(&crate::watchdog::dumps_dir())
        .into_iter()
        .map(|(p, size)| format!("{} ({} KB)", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), size / 1024))
        .collect();
    if !dumps.is_empty() {
        files.push(("dumps.txt", dumps.join("\n")));
    }
    if let Some(text) = windows.recv_timeout(Duration::from_secs(25)).ok().filter(|t| !t.is_empty()) {
        files.push(("events.txt", text));
    }
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in files {
        let (text, n) = private(&text, &known, &names, kind_of(name));
        hid += n;
        zip.start_file(name, opts)?;
        zip.write_all(text.as_bytes())?;
    }
    let bytes = zip.finish()?.into_inner();
    let stamp = crate::hooks::local_stamp("%Y%m%d-%H%M%S");
    Ok((format!("shikisha-diagnostics-{stamp}.zip"), bytes, hid))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This app's own keys are plain lowercase hex, which nothing that goes
    /// by shape takes for a key: in a link, in a header, or by their value
    #[test]
    fn the_apps_own_keys_are_hidden_wherever_they_are() {
        let key = "3f9a0c7e1b2d4f6a8c0e1a3b5d7f9a1c3e5b7d9f1a3c5e7b";
        let log = format!(
            "Loaded settings: http://127.0.0.1:61234/?token={key}\n\
             phone opened /?t={key}&x=1\n\
             X-Token: {key}\n\
             the token file holds {key}\n"
        );
        let (out, n) = hidden(&log, &[key.to_string()]);
        assert!(!out.contains(key), "a key is left: {out}");
        assert!(n >= 4, "what was hidden is not counted: {n}");
        assert!(out.contains("127.0.0.1:61234"), "the loopback is not a secret: {out}");
        let (links, _) = hidden("push to https://github.com/acme/billing.git failed; update from https://github.com/styleio/ShikishaTerm/releases", &[]);
        assert!(!links.contains("acme"), "where the person's work lives went out: {links}");
        assert!(links.contains("github.com/styleio/ShikishaTerm/releases"), "this program's own home was hidden: {links}");
        assert!(out.contains("x=1"), "what came after the key went with it: {out}");
    }

    /// What an AI said and what was handed between tabs is somebody's work,
    /// and stays on this machine; the event it was part of goes
    #[test]
    fn what_was_said_is_left_out_and_the_event_kept() {
        let log = "[1.0] on_done fired tab8 [DONE]: response 2853 chars: the customer's billing screen is fixed\n\
                   [2.0] auto-send tab6 -> tab7 (depth 1): Folder D:\\acme-shop, fix the invoice\n\
                   [3.0] orchestration: t3 added to j2: rewrite the pricing page\n\
                   [4.0] git: {\"act\":\"pull\",\"ok\":true,\"data\":\"Updating 1..2 src/secret-plan.md\"}\n";
        let long = format!("[5.0] something new: {}\n", "x".repeat(LINE_KEPT * 2));
        let (out, n) = hidden(&(log.to_string() + &long), &[]);
        for gone in ["billing", "acme-shop", "pricing", "secret-plan", &"x".repeat(LINE_KEPT)] {
            assert!(!out.contains(gone), "{gone} is still in it: {out}");
        }
        for kept in ["on_done fired tab8 [DONE]: response 2853 chars", "auto-send tab6 -> tab7 (depth 1)", "\"act\":\"pull\",\"ok\":true"] {
            assert!(out.contains(kept), "the event went with the words: {kept}\n{out}");
        }
        assert_eq!(n, 5);
    }

    /// Who and where the person is goes too: addresses on their networks,
    /// their mail address. A version number is not an address
    #[test]
    fn addresses_and_mail_are_hidden_and_version_numbers_are_not() {
        let (out, n) = hidden("tailscale 100.101.102.103 and lan 192.168.1.20, mail me@example.com, v0.27.0.1, 999.1.1.1", &[]);
        assert!(!out.contains("100.101.102.103") && !out.contains("192.168.1.20"), "{out}");
        assert!(!out.contains("me@example.com"), "{out}");
        assert!(out.contains("0.27.0.1") || out.contains("v0.27.0.1"), "a version was taken for an address: {out}");
        assert!(out.contains("999.1.1.1"), "something that is not an address was hidden: {out}");
        assert_eq!(n, 3);
    }

    /// A real log, hidden, for looking at by eye:
    /// `SHIKISHA_DIAG_LOG=<copy of a hooks.log> cargo test -- --ignored hide_a_real_log`
    /// Prints what is left that has the shape of this app's keys
    #[test]
    #[ignore]
    fn hide_a_real_log_for_a_check() {
        let file = std::env::var("SHIKISHA_DIAG_LOG").expect("SHIKISHA_DIAG_LOG");
        let text = String::from_utf8_lossy(&std::fs::read(&file).unwrap()).into_owned();
        let (out, n) = hidden(&text, &known_keys());
        let left: Vec<&str> = Regex::new(r"\b[0-9a-f]{40,}\b").unwrap().find_iter(&out).map(|m| m.as_str()).collect();
        println!("hidden {n}; long hex left: {}", left.len());
        for l in left.iter().take(10) {
            println!("  {l}");
        }
        std::fs::write(format!("{file}.hidden"), out).unwrap();
    }

    /// A real report, saved for looking at by eye:
    /// `SHIKISHA_DIAG_OUT=<a folder> cargo test -- --ignored save_a_real_report`
    #[test]
    #[ignore]
    fn save_a_real_report_for_a_check() {
        let dir = std::env::var("SHIKISHA_DIAG_OUT").expect("SHIKISHA_DIAG_OUT");
        let began = std::time::Instant::now();
        let (name, bytes, hid) = bundle().unwrap();
        println!("{name}: {} KB, {hid} hidden, made in {:.1} s", bytes.len() / 1024, began.elapsed().as_secs_f64());
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
    }

    /// A folder's size is what is in it: what a link in it points to is
    /// somewhere else, and the counting stops where it is told to
    #[test]
    fn a_folder_is_counted_without_what_its_links_point_to() {
        let dir = std::env::temp_dir().join(format!("shikisha-diag-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("in/deeper")).unwrap();
        std::fs::create_dir_all(dir.join("elsewhere")).unwrap();
        std::fs::write(dir.join("in/a.txt"), [0u8; 100]).unwrap();
        std::fs::write(dir.join("in/deeper/b.txt"), [0u8; 50]).unwrap();
        std::fs::write(dir.join("elsewhere/big.bin"), [0u8; 4000]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("in/link")).unwrap();
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(dir.join("elsewhere"), dir.join("in/link"));
        let far = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut left = 100;
        assert_eq!(folder_size(&dir.join("in"), &mut left, far), (150, 2, false));
        assert_eq!(left, 98);
        let mut left = 1;
        let (_, files, more) = folder_size(&dir.join("in"), &mut left, far);
        assert!(more && files == 1, "the counting went on past what it was allowed");
        let mut left = 100;
        assert!(folder_size(&dir.join("in"), &mut left, std::time::Instant::now()).2, "the counting went on past its time");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The app's own record of the machine keeps its long lines, which name
    /// the programs; any other log's long line is somebody's words
    #[test]
    fn the_record_of_the_machine_keeps_its_long_lines() {
        let long = format!("[pid 1 at 2] cpu 93% | most memory: {}", "msedgewebview2.exe 2.6 GB, ".repeat(20));
        assert!(long.len() > LINE_KEPT);
        let (cut, n) = hidden(&long, &[]);
        assert!(cut.contains(LEFT_OUT) && n == 1, "{cut}");
        let (whole, n) = hidden_lines(&long, &[], false);
        assert_eq!(whole.trim_end(), long.trim_end());
        assert_eq!(n, 0);
    }

    /// A disk is named by its letter, a share only as a share
    #[test]
    fn a_disk_is_named_without_saying_where_a_share_is() {
        use std::path::Path;
        assert_eq!(disk_name(Path::new(r"d:\work\shop")), "D:");
        assert_eq!(disk_name(Path::new(r"\\?\C:\work")), "C:");
        for share in [r"\\nas01\projects\shop", r"\\?\UNC\nas01\projects\shop"] {
            let said = disk_name(Path::new(share));
            assert_eq!(said, "a network share", "{share}");
        }
        assert_eq!(disk_name(Path::new("/home/someone/work")), "this machine's disk");
    }

    /// What a log said over and over is said once, with how many more
    /// times and until when; what it said once stays as it is
    #[test]
    fn a_log_says_once_what_it_said_over_and_over() {
        let log = "[   1.000] started\n[   2.000] no line to it (gone)\n[   4.000] no line to it (gone)\n[   6.000] no line to it (gone)\n[   7.000] ready\n[   8.000] ready\n";
        assert_eq!(
            collapsed(log),
            "[   1.000] started\n[   2.000] no line to it (gone)\n    (the same 2 more times, the last at 6.000)\n[   7.000] ready\n    (the same 1 more times, the last at 8.000)\n"
        );
        assert_eq!(collapsed("one\ntwo\n"), "one\ntwo\n");
    }

    /// The settings are told by their shape: switches and numbers as they
    /// are set, a choice the app offers by its word, and what a person typed
    /// only as text of some length
    #[test]
    fn the_settings_are_told_without_what_was_typed() {
        let v = serde_json::json!({
            "//remote": "a note to the reader",
            "keep_terminals": false,
            "max_chain": 4,
            "appearance": {"theme": "dark", "font": "Cascadia Mono"},
            "desks": [{"name": "Acme shop", "file": "desks/acme.json", "auto": true}],
            "Acme Corp": {"token": "s3cr3t-value"},
            // A name a person gave in small letters, as the app's own are written
            "zorblatt_ledger": true,
            "#zorblatt": true,
            "capabilities": {"files": {"quuxley_books": {"read": true}}},
            "language": "A Sentence Somebody Typed",
            // Lists by name: what they are filed under is somebody's word,
            // small letters or not
            "providers": {"acme": {"kind": "openai", "stream": true}},
            "far_hooks": {"nas": {"claude": "yes"}},
        });
        let mut said = String::new();
        shape(&v, "settings", false, 0, &mut said);
        assert!(said.contains("providers:\n    (a name):\n      kind: openai\n      stream: true"), "{said}");
        assert!(said.contains("far_hooks:\n    (a name):\n      (a name): text, 3 letters"), "{said}");
        assert!(!said.contains("nas") && !said.contains("claude"), "{said}");
        for kept in ["keep_terminals: false", "max_chain: 4", "theme: text, 4 letters", "font: text, 13 letters", "desks: 1 of them", "#1:", "auto: true", "name: text, 9 letters", "language: text, 25 letters"] {
            assert!(said.contains(kept), "{kept} is not in:\n{said}");
        }
        assert!(said.contains("files:\n      (a name):\n        read: true"), "{said}");
        for gone in ["Acme", "acme", "Cascadia", "s3cr3t", "a note", "Somebody", "zorblatt", "quuxley"] {
            assert!(!said.contains(gone), "{gone} is in:\n{said}");
        }
        assert!(said.contains("(a name):"), "{said}");
    }

    /// A run of the same event is said once, with how many more there were
    #[test]
    fn a_run_of_the_same_event_is_said_once() {
        let one = |n: u32, id: u32| format!("Event[{n}]\n  Source: Microsoft-Windows-Kernel-Processor-Power\n  Date: 2026-11-1{n}\n  Event ID: {id}\n  Description:\nprocessor {n} is held back\n\n");
        // Two disks that failed are two events
        let disk = |n: u32| format!("Event[{n}]\n  Source: disk\n  Event ID: 7\n  Description:\nthe device Harddisk{n} has a bad block\n\n");
        let said = events_once(&format!("{}{}{}", disk(0), disk(1), disk(1)));
        assert!(said.contains("Harddisk0") && said.contains("Harddisk1") && said.contains("(the same event 1 more times before it)"), "{said}");
        let said = events_once(&format!("{}{}{}{}", one(0, 37), one(1, 37), one(2, 37), one(3, 41)));
        assert_eq!(said.matches("Event[").count(), 2, "{said}");
        assert!(said.contains("processor 0 is held back") && said.contains("(the same event 2 more times before it)") && said.contains("processor 3"), "{said}");
        // Two programs that crashed are two events, though the event has one number
        let crash = |who: &str| format!("Event[0]\n  Source: Application Error\n  Event ID: 1000\n  Description:\nFaulting application name: {who}\n\n");
        let said = events_once(&format!("{}{}", crash("SHIKISHA-TERM.exe"), crash("other.exe")));
        assert!(said.contains("SHIKISHA-TERM.exe") && said.contains("other.exe") && !said.contains("the same event"), "{said}");
        // ...and of what the Store did, only what names this app is kept
        let store = format!("{}{}", crash("the package SHIKISHA-TERM_0.31.0.0 could not be registered"), crash("the package of another app"));
        let ours = events_about(&store, "shikisha");
        assert!(ours.contains("could not be registered") && !ours.contains("another app"), "{ours}");
    }

    /// A path is told by no more than its file, and that only when the file
    /// is a program or the app's own: the folders above it are where
    /// somebody keeps their work, and so is the name of what they keep
    #[test]
    fn a_path_is_told_by_no_more_than_its_file() {
        for (path, told) in [
            (r"Faulting application path: D:\customers\acme\app.exe and more", r"Faulting application path: [a folder]\app.exe and more"),
            (r"in C:\Program Files\Some App\helper.dll, on \\nas01\projects\shop\tool.exe; version 1:2", r"in [a folder]\helper.dll, on [a folder]\tool.exe; version 1:2"),
            // The other way of writing it, a folder with a comma in its name, one in quotes
            ("at D:/customers/acme/app.exe now", "at [a folder]/app.exe now"),
            (r"at D:\Acme, Inc\repo\app.exe now", r"at [a folder]\app.exe now"),
            (r#"ran "D:\customers\acme corp\tool.exe" twice"#, r#"ran "[a folder]\tool.exe" twice"#),
            // A folder, and a document: nothing of them is left
            (r"could not remove D:\customers\acme\repo: it is in use", "could not remove [a folder]: it is in use"),
            (r"opened ~\work\acme\offer.docx", "opened [a folder]"),
            // The app's own file is told
            (r"~\AppData\Local\ShikishaTerm\keeper\ab12\run\keep.key is not there", r"[a folder]\keep.key is not there"),
            // A share whose name holds a space, and the long way of writing one
            (r"at \\nas01\Shared Tools\customer\app.exe now", r"at [a folder]\app.exe now"),
            (r"at \\?\UNC\nas01\share\customer\app.exe now", r"at [a folder]\app.exe now"),
            // ...and the other systems' places for a person's files, wherever they are
            ("left /home/someone/acme/repo behind", "left [a folder] behind"),
            ("left /workspace/customer/acme behind", "left [a folder] behind"),
            ("ran /usr/bin/git and read /proc/self/stat", "ran /usr/bin/git and read /proc/self/stat"),
            ("ran ~/work/acme/tool.so (twice)", "ran [a folder]/tool.so (twice)"),
            // What is not a path stays: a door, a link's own path, a share of something
            (r"listening on \\.\pipe\shikisha-14804 (Children)", r"listening on \\.\pipe\shikisha-14804 (Children)"),
            ("GET /api/diagnostics took 3 s, 2/3 done", "GET /api/diagnostics took 3 s, 2/3 done"),
        ] {
            assert_eq!(folders_out(path).0, told, "{path}");
        }
        assert_eq!(folders_out(r"D:\a\b.exe and D:\c\d").1, 2);
    }

    /// A log keeps the app's own words and the numbers, and nothing else:
    /// whatever line somebody's word is in, and however it is written there
    #[test]
    fn a_log_keeps_the_apps_own_words_and_no_others() {
        let names = vec![("Zorblatt".to_string(), "[desk 1]".to_string()), (r"D:\repo".to_string(), "[folder 1]".to_string())];
        let log = [
            "[   1.000] Startup: desk \"Zorblatt\" (first desk)",
            "[   2.000] \"claude\" starts clean: this desk remembers nothing for it in this folder",
            "[   3.000] tab1 \"quuxley ledger - frobnitz\" is running a helper",
            "[   4.000] scheduled prompt \"wibbly\" run 3 (run now) -> tab \"books\": \u{27e6}pay the Quuxley invoices and\u{2026}\u{27e7}",
            "[   5.000] could not remove D:\\repo: customers\\Quuxley Merger\\offer.xlsx could not be deleted: it is in use (os error 32)",
            "[   6.000] pr news QuuxleyCorp/wibbly-repo#123",
            "[   7.000] far terminal 2: the line to frobnitz@quuxhost01:22 went; waiting for it to come back",
            "[   8.000] write_file reports/customers/Quuxley/offer.txt (12 bytes)",
            "[   9.000] State tab2 WAIT->BUSY [Wibbly Profile] said=None prompted=false",
            "[  10.000] far terminal for 810668f8-e46b-409b-ac8b-afe984ff8d2f: to be opened on this PC once its line is up",
            "[  11.000] this PC's resident process: no line to it (~\\AppData\\Local\\ShikishaTerm\\keeper\\f81736d6f45330d2\\run\\keep.key is not there: (os error 3))",
            "[  12.000] \u{30c7}\u{30b9}\u{30af}\u{300c}\u{7d4c}\u{7406}\u{90e8}\u{300d}",
            // A name written as the numbers of its letters, and one written for a link
            "[  13.000] run lua result: {\"customer\":\"\\u5f35\\u4e09\"} and %E5%BC%B5%E4%B8%89",
            // What a library calls the way it failed is made of words the app knows
            "[  14.000] Update check: CheckFailed { message: \"invalid peer certificate: UnknownIssuer\" }",
        ]
        .join("\n");
        let (said, _) = private(&log, &[], &names, Kind::Log);
        let lines: Vec<&str> = said.lines().collect();
        for gone in ["zorblatt", "quux", "frobnitz", "wibbly", "\u{7d4c}\u{7406}"] {
            assert!(!said.to_lowercase().contains(gone), "{gone} is still in it:\n{said}");
        }
        // What the app itself said is all there
        assert_eq!(lines[0], "[   1.000] Startup: desk \"[desk 1]\" (first desk)");
        assert_eq!(lines[1], "[   2.000] \"claude\" starts clean: this desk remembers nothing for it in this folder");
        assert!(lines[2].starts_with("[   3.000] tab1 \"") && lines[2].ends_with("\" is running a helper"), "{}", lines[2]);
        assert!(lines[3].starts_with("[   4.000] scheduled prompt \"\u{25af}\" run 3 (run now) -> tab \"") && lines[3].ends_with(LEFT_OUT), "{}", lines[3]);
        // ...with the reason a folder stayed, and not the file that held it
        assert!(lines[4].starts_with("[   5.000] could not remove [folder 1]: ") && lines[4].ends_with(" could not be deleted: it is in use (os error 32)"), "{}", lines[4]);
        assert!(lines[5].starts_with("[   6.000] pr news \u{25af}/") && lines[5].ends_with("#123"), "{}", lines[5]);
        assert!(lines[6].contains("\u{25af}@\u{25af}01:22 went; waiting for it to come back"), "{}", lines[6]);
        assert!(lines[7].starts_with("[   8.000] write_file ") && lines[7].ends_with(" (12 bytes)") && lines[7].contains('\u{25af}'), "{}", lines[7]);
        assert!(lines[8].starts_with("[   9.000] State tab2 WAIT->BUSY [\u{25af} ") && lines[8].ends_with("said=None prompted=false"), "{}", lines[8]);
        assert!(!lines[12].contains("5f35") && !lines[12].contains("%E5") && !lines[12].contains('\u{5f35}'), "{}", lines[12]);
        assert_eq!(lines[13], "[  14.000] Update check: CheckFailed { message: \"invalid peer certificate: UnknownIssuer\" }");
        // A number written with letters is left whole
        assert_eq!(lines[9], "[  10.000] far terminal for 810668f8-e46b-409b-ac8b-afe984ff8d2f: to be opened on this PC once its line is up");
        assert_eq!(lines[10], "[  11.000] this PC's resident process: no line to it ([a folder]\\keep.key is not there: (os error 3))");
        // A name that is also a word the hiding looks for does not blind it
        let names = vec![("response".to_string(), "[desk 1]".to_string()), ("https".to_string(), "[a tab]".to_string())];
        let (said, _) = private("fired tab1 [done]: response 100 chars: the Quuxley plan\nopened https://github.com/quuxley/wibbly-repo\n", &[], &names, Kind::Log);
        assert!(!said.to_lowercase().contains("quux") && !said.contains("wibbly"), "{said}");
        // What this program wrote for the report is left as it is: a tab called node does not rename a program
        let names = vec![("node".to_string(), "[a tab]".to_string())];
        assert_eq!(private("most cpu: quuxleyd x6 41%\n", &[], &names, Kind::Record).0, "most cpu: quuxleyd x6 41%\n");
        // ...and where the app says how it ended, the words of what failed are kept
        assert!(private("thread 'main' panicked at quuxley: index out of bounds\n", &[], &[], Kind::Ending).0.contains("quuxley: index out of bounds"));
    }

    /// The person's own names are put as what stands for them, as words of
    /// their own, the longest first
    #[test]
    fn the_persons_own_names_are_put_as_numbers() {
        let names = vec![
            (r"D:\work\acme-payroll".to_string(), "[folder 1]".to_string()),
            ("acme-payroll".to_string(), "[folder 1]".to_string()),
            ("Acme".to_string(), "[desk 1]".to_string()),
            ("\u{7d4c}\u{7406}".to_string(), "[desk 2]".to_string()),
        ];
        let (said, n) = names_out("Sending state: 2 tabs, desk \"ACME\"; could not remove d:\\work\\Acme-Payroll\\target: in use; branch acme-payroll; desk \"\u{7d4c}\u{7406}\"; acmeish stays", &names);
        assert_eq!(said, "Sending state: 2 tabs, desk \"[desk 1]\"; could not remove [folder 1]\\...: in use; branch [folder 1]; desk \"[desk 2]\"; acmeish stays");
        assert_eq!(n, 4);
        // And through the whole of what a report may hold: nothing of the folder is left
        let (said, _) = private("left d:\\work\\acme-payroll\\target\\Acme Merger\\x.docx: in use", &[], &names, Kind::Log);
        assert_eq!(said.trim_end(), "left [folder 1]\\...: in use");
    }

    /// Every setting that is a list by name is known as one: a new one that
    /// is not would have its names -- a customer's, a machine's -- in the
    /// report whenever they are written the way the app writes its own
    #[test]
    fn every_setting_filed_by_name_is_known_as_one() {
        // (the settings, and the gates a person names in them)
        let source = [include_str!("config.rs"), include_str!("caps.rs")].concat();
        let filed = Regex::new(r"pub (\w+): (?:Option<)?(?:std::collections::)?(?:HashMap|BTreeMap)<String").unwrap();
        let found: Vec<&str> = filed.captures_iter(&source).map(|c| c.get(1).unwrap().as_str()).collect();
        assert!(found.len() >= 10, "the settings' lists by name were not found in their source: {found:?}");
        for name in found {
            assert!(BY_NAME.contains(&name), "the setting {name} is a list by name and is not in BY_NAME: its names would be in a report");
        }
    }

    /// What a console program printed is read in the letters it was printed in
    #[test]
    fn what_a_console_program_printed_is_read_as_text() {
        assert_eq!(console_text("Level: \u{30a8}\u{30e9}\u{30fc}\0\n".as_bytes()), "Level: \u{30a8}\u{30e9}\u{30fc}\n");
        // Not the web's letters: read in the console's own, and never refused
        let read = console_text(&[b'o', b'k', b' ', 0x83, 0x47, 0x83, 0x89, 0x81, 0x5b]);
        assert!(read.starts_with("ok "), "{read}");
    }

    /// A folder of the app's own is told thing by thing; in one a person
    /// also keeps things in, what is not the app's is counted and not named
    #[test]
    fn a_folder_is_listed_without_naming_what_is_not_the_apps() {
        let dir = std::env::temp_dir().join(format!("shikisha-diag-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::create_dir_all(dir.join("acme-shop")).unwrap();
        std::fs::write(dir.join("logs/hooks.log"), [0u8; 3000]).unwrap();
        std::fs::write(dir.join("acme-shop/offer.docx"), [0u8; 5000]).unwrap();
        std::fs::write(dir.join("SHIKISHA-TERM.exe"), [0u8; 10]).unwrap();
        // Beside a download unpacked into a folder of somebody's own, and a
        // key file of their own naming
        std::fs::write(dir.join("acme-merger-plan.txt"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("acme-secrets.json"), [0u8; 10]).unwrap();
        // ...one named after the app's own, and a tab's record in the logs
        std::fs::write(dir.join("secrets.json-acme.backup"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("acme-payroll-20261118.log"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("hooks.log"), [0u8; 10]).unwrap();
        // ...and a tab's record whose tab was called after the app
        std::fs::write(dir.join("shikisha-QuuxleyLedger-20261118.log"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("shikisha-desk-20261118.log"), [0u8; 10]).unwrap();
        let far = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let said = listing("a folder", &dir, Told::Own, &mut 1000, far);
        assert!(said.contains("logs/") && said.contains("in 1 files") && said.contains("SHIKISHA-TERM.exe"), "{said}");
        assert!(!said.contains("acme") && !said.contains("Quuxley") && said.contains("(7 more that are not the app's own") && said.contains("hooks.log") && !said.contains("shikisha-desk"), "{said}");
        let all = listing("a folder", &dir, Told::All, &mut 1000, far);
        assert!(all.contains("acme-shop/"), "{all}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bundle opens, holds what a report needs, and never a key
    #[test]
    fn the_bundle_is_a_zip_of_what_a_report_needs() {
        let (name, bytes, _) = bundle().unwrap();
        assert!(name.starts_with("shikisha-diagnostics-") && name.ends_with(".zip"));
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut about = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("about.txt").unwrap(), &mut about).unwrap();
        assert!(about.contains(env!("CARGO_PKG_VERSION")), "{about}");
        let mut storage = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("storage.txt").unwrap(), &mut storage).unwrap();
        assert!(storage.contains("disk ") && storage.contains(" free of "), "the disks are not in it: {storage}");
        // What the rest comes to is the first thing in it
        assert_eq!(zip.by_index(0).unwrap().name(), "findings.txt");
        let read = |zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str| {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut zip.by_name(name).unwrap_or_else(|_| panic!("{name} is not in it")), &mut text).unwrap();
            text
        };
        let first = read(&mut zip, "findings.txt");
        assert!(first.contains("At the moment this report was made") && first.contains("The week's record of the machine") && first.contains("The app's log"), "{first}");
        // Every program on the machine at that moment, this one among them and marked
        let now = read(&mut zip, "now.txt");
        assert!(now.contains("every program, the copies of one together") && now.contains("this app and what it started, one by one"), "{now}");
        let me = std::env::current_exe().unwrap().file_stem().unwrap().to_string_lossy().into_owned();
        assert!(now.lines().any(|l| l.starts_with("* ") && l.contains(&me[..me.len().min(20)])), "this program is not marked among them:\n{now}");
        let about = read(&mut zip, "about.txt");
        assert!(about.contains("the machine has been up: ") && about.contains("found on the PATH the app was started with: git "), "{about}");
        assert!(read(&mut zip, "files.txt").contains("== the app's folder =="));
        let _ = read(&mut zip, "settings.txt");
    }
}
