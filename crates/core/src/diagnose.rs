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

/// Where the log carries what was said rather than what happened: an AI's
/// answer, a message handed from one tab to another, a job's words, what
/// git printed. The words are somebody's work -- a customer's project, a
/// file's contents -- and what is wrong with the program is in the events
/// around them, so only the event is kept
static SAID: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
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
        // Left for the record that is kept: taking it here would take it from there
        late: [0, 0],
        disk: crate::health::disk_room(&crate::config::root_dir()),
        kernel: rates.kernel,
        busy: rates.busy,
        queue: rates.queue,
        paged: rates.paged,
    };
    let top = crate::health::top_of(&procs);
    let mut s = format!("measured over {:.1} s at {}\n", over / 1000.0, crate::hooks::local_stamp("%Y-%m-%dT%H:%M:%S"));
    s.push_str(&format!("{}\n", crate::health::line(&look, None)));
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
    // Which part of the app itself was working
    let mut threads: Vec<(String, f64)> = after
        .iter()
        .map(|(id, (name, ms))| {
            let was = before.get(id).map_or(*ms, |(_, was)| *was);
            (if name.is_empty() { "(unnamed)".to_string() } else { name.clone() }, ms.saturating_sub(was) as f64 * 100.0 / over)
        })
        .collect();
    if !threads.is_empty() {
        threads.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        s.push_str(&format!("\nthe app's own threads ({}; the ones that used a hundredth of a processor or more)\n", threads.len()));
        for (name, share) in threads.iter().filter(|t| t.1 >= 1.0).take(20) {
            s.push_str(&format!("{name:<40} {share:>6.1} % of one processor\n"));
        }
    }
    (s, look, top)
}

/// What Windows itself wrote down over the week: programs that crashed or
/// stopped answering, the system saying it ran out of memory (and which
/// programs held it), a disk reporting errors, the processors being held
/// back, the machine waking from sleep. It is kept by Windows, not by this
/// app, so it is there for the days before this app kept any record, and
/// after the app was taken off and put on again.
///
/// Who was logged in and what the PC is called are left out
#[cfg(windows)]
fn events() -> String {
    const WEEK: &str = "TimeCreated[timediff(@SystemTime) <= 604800000]";
    /// The most of one log that goes in, the newest
    const EVENTS: &str = "/c:150";
    const ROOM: usize = 150 * 1024;
    let asks = [
        (
            "System",
            format!(
                "*[System[{WEEK} and (Level=1 or Level=2 or Provider[@Name='Microsoft-Windows-Resource-Exhaustion-Detector' or @Name='disk' or @Name='Ntfs' or @Name='volmgr' \
                 or @Name='stornvme' or @Name='storahci' or @Name='Microsoft-Windows-WHEA-Logger' or @Name='Microsoft-Windows-Power-Troubleshooter'] \
                 or (Provider[@Name='Microsoft-Windows-Kernel-Processor-Power'] and EventID=37))]]"
            ),
        ),
        ("Application", format!("*[System[{WEEK} and (Level=1 or Level=2 or Provider[@Name='Windows Error Reporting' or @Name='Application Hang'])]]")),
    ];
    let pc = std::env::var("COMPUTERNAME").ok().filter(|n| n.chars().count() >= 3);
    let mut out = String::new();
    for (log, ask) in asks {
        out.push_str(&format!("== {log}: the last week, the newest first. Its times are the world's (UTC); the app's own records are in this PC's time ==\n"));
        let Some(bytes) = said_by("wevtutil", &["qe", log, &format!("/q:{ask}"), "/f:text", EVENTS, "/rd:true"], std::time::Duration::from_secs(10)) else {
            out.push_str("Windows did not answer in time.\n\n");
            continue;
        };
        let mut text = String::new();
        for line in console_text(&bytes).lines() {
            let l = line.trim_start();
            if l.starts_with("User:") || l.starts_with("User Name:") || l.starts_with("Computer:") {
                continue;
            }
            if text.len() + line.len() > ROOM {
                text.push_str("(the rest is left out)\n");
                break;
            }
            text.push_str(line.trim_end());
            text.push('\n');
        }
        if let Some(pc) = &pc {
            text = replace_ignoring_case(&text, &format!(r"\b{}\b", regex::escape(pc)), "[this PC]");
        }
        let text = events_once(&text);
        out.push_str(if text.trim().is_empty() { "Nothing.\n" } else { &text });
        out.push('\n');
    }
    out
}

/// The events with a run of the same one said once: a processor held back
/// is written once for each processor, and thirty-two of them say no more
/// than one
#[cfg_attr(not(windows), allow(dead_code))]
fn events_once(text: &str) -> String {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        match blocks.last_mut() {
            Some(block) if !line.starts_with("Event[") => block.push(line),
            _ => blocks.push(vec![line]),
        }
    }
    let what = |block: &[&str]| -> String { block.iter().filter(|l| l.starts_with("  Source:") || l.starts_with("  Event ID:")).cloned().collect::<Vec<_>>().join("|") };
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

/// The folders of the app's own that are told by name, and the files told
/// by their ending: what the app put there. Anything else in a folder a
/// person can also keep things in is counted and not named
const OWN_FOLDERS: [&str; 12] = ["config", "data", "logs", "desks", "lang", "profiles", "bridge", "docs", "scripts", "debug", "run", "keys"];
const OWN_ENDINGS: [&str; 10] = ["exe", "dll", "old", "cmd", "md", "txt", "json", "log", "key", "db"];

/// What is in a folder: each thing, how much it holds and when it changed.
/// `all` names everything (a folder only the app writes into); otherwise
/// only what is the app's own by its name
fn listing(title: &str, dir: &std::path::Path, all: bool, left: &mut u64, until: std::time::Instant) -> String {
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
        let ending = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        let named = all || if is_dir { OWN_FOLDERS.contains(&name.as_str()) } else { OWN_ENDINGS.contains(&ending.as_str()) };
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
    let mut out = listing("the app's folder", &root, false, &mut left, until);
    for part in ["config", "data", "logs"] {
        out.push_str(&listing(&format!("the app's folder: {part}"), &root.join(part), true, &mut left, until));
    }
    let program = crate::config::exe_dir();
    if program != root {
        out.push_str(&listing("the program's folder", &program, false, &mut left, until));
    }
    if let Ok(home) = crate::localkeep::home() {
        out.push_str(&listing("the resident process's folder", &home, false, &mut left, until));
        out.push_str(&listing("the resident process's folder: run", &home.join("run"), false, &mut left, until));
    }
    out
}

/// The settings whose words are safe to say: a choice out of a few the app
/// offers, never something a person typed
const CHOICES: [&str; 11] = ["theme", "access", "language", "ai", "kind", "mode", "engine", "how", "shell", "type", "state"];

/// The shape of the settings: every setting by its name, a switch or a
/// number as it is set, and anything a person typed -- a name, a folder, a
/// prompt, a key -- only as "text" and how long it is. Which switches are on
/// is what a fault most often turns on, and what was typed is nobody's to read
fn shape(v: &serde_json::Value, key: &str, depth: usize, out: &mut String) {
    use serde_json::Value;
    let pad = "  ".repeat(depth);
    // A name the app gave is small letters and underscores; anything else
    // may be a name a person gave
    let plain = !key.is_empty() && key.len() <= 40 && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    let key = if plain || key.starts_with('#') { key } else { "(a name)" };
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
                        shape(item, &format!("#{}", i + 1), depth + 1, out);
                    }
                }
            }
        }
        Value::Object(o) => {
            out.push_str(&format!("{pad}{key}:\n"));
            if depth < 6 {
                // A line beginning `//` is a note to the person reading the file
                for (k, item) in o.iter().filter(|(k, _)| !k.starts_with("//")) {
                    shape(item, k, depth + 1, out);
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
    shape(&v, "settings", 0, &mut out);
    if out.len() > ROOM {
        let cut = (0..=ROOM).rev().find(|i| out.is_char_boundary(*i)).unwrap_or(0);
        out.truncate(cut);
        out.push_str("\n(the rest is left out)\n");
    }
    out
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
    }
    line("git", git.map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).unwrap_or_else(|| "did not answer".into()));
    // What is set around the app, by name only: a proxy changes every
    // connection it makes, and its address is not for a report
    let set: Vec<String> = std::env::vars_os()
        .map(|(k, _)| k.to_string_lossy().into_owned())
        .filter(|k| {
            let k = k.to_ascii_uppercase();
            k.starts_with("SHIKISHA") || ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "NODE_OPTIONS", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "WEBVIEW2_USER_DATA_FOLDER"].contains(&k.as_str())
        })
        .collect();
    line("set in the environment (names only)", if set.is_empty() { "none of the ones that matter".into() } else { set.join(", ") });
    s
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
    let known = known_keys();
    let mut hid = 0;
    // What takes a while is begun at once, side by side
    let storage = later("storage", storage);
    let moment = later("now", now);
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
    let hooks = log(logs.join("hooks.log"), LOG_LINES);
    let moment = moment.recv_timeout(NOW_OVER + Duration::from_secs(20)).ok();
    let mut first = String::from("What this report comes to, read for whoever opens it. The files beside this one are what it was read from.\n\n");
    if let Some((_, look, top)) = &moment {
        first.push_str(&crate::findings::of_now(look, top));
        first.push('\n');
    }
    first.push_str(&crate::findings::of_week(health.as_deref().unwrap_or_default()));
    first.push('\n');
    first.push_str(&crate::findings::of_log(hooks.as_deref().unwrap_or_default()));
    let about = about() + &machine(git.recv_timeout(Duration::from_secs(6)).ok().flatten());
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
    // What this program wrote a number and a file name at a time keeps its
    // long lines, which are the ones that name the programs
    // ...and what Windows wrote keeps its own, which say which programs it was
    const WHOLE: [&str; 4] = ["health.log", "findings.txt", "now.txt", "events.txt"];
    for (name, text) in files {
        let (text, n) = hidden_lines(&text, &known, !WHOLE.contains(&name));
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
            "language": "A Sentence Somebody Typed",
        });
        let mut said = String::new();
        shape(&v, "settings", 0, &mut said);
        for kept in ["keep_terminals: false", "max_chain: 4", "theme: dark", "font: text, 13 letters", "desks: 1 of them", "#1:", "auto: true", "name: text, 9 letters", "language: text, 25 letters"] {
            assert!(said.contains(kept), "{kept} is not in:\n{said}");
        }
        for gone in ["Acme", "acme", "Cascadia", "s3cr3t", "a note", "Somebody"] {
            assert!(!said.contains(gone), "{gone} is in:\n{said}");
        }
        assert!(said.contains("(a name):"), "{said}");
    }

    /// A run of the same event is said once, with how many more there were
    #[test]
    fn a_run_of_the_same_event_is_said_once() {
        let one = |n: u32, id: u32| format!("Event[{n}]\n  Source: Kernel-Processor-Power\n  Event ID: {id}\n  Description:\nprocessor {n} is held back\n\n");
        let said = events_once(&format!("{}{}{}{}", one(0, 37), one(1, 37), one(2, 37), one(3, 41)));
        assert_eq!(said.matches("Event[").count(), 2, "{said}");
        assert!(said.contains("processor 0 is held back") && said.contains("(the same event 2 more times before it)") && said.contains("processor 3"), "{said}");
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
        std::fs::write(dir.join("app.exe"), [0u8; 10]).unwrap();
        let far = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let said = listing("a folder", &dir, false, &mut 1000, far);
        assert!(said.contains("logs/") && said.contains("in 1 files") && said.contains("app.exe"), "{said}");
        assert!(!said.contains("acme") && said.contains("(1 more that are not the app's own"), "{said}");
        let all = listing("a folder", &dir, true, &mut 1000, far);
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
        assert!(read(&mut zip, "about.txt").contains("the machine has been up: "));
        assert!(read(&mut zip, "files.txt").contains("== the app's folder =="));
        let _ = read(&mut zip, "settings.txt");
    }
}
