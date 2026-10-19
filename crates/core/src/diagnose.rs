//! What a bug report carries: one file, made on this machine, looked at by
//! the person, and attached by them.
//!
//! A report that says "it stopped" and nothing else is one nobody can act
//! on, and the things that would say why -- the app's log, the crash log,
//! how the last run ended, what the update was doing -- are scattered over
//! folders a person has no reason to know. So they are put in one zip, here,
//! when the person asks for it.
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
        if line.chars().count() > LINE_KEPT {
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
    let text = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().collect();
    Some(lines[lines.len().saturating_sub(n)..].join("\n"))
}

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

/// The zip, its name, and how many things were hidden in it
pub fn bundle() -> Result<(String, Vec<u8>, usize)> {
    let known = known_keys();
    let mut hid = 0;
    let mut files: Vec<(&str, String)> = vec![("about.txt", about())];
    let logs = crate::config::logs_dir();
    let state = |name: &str| crate::config::state_path(name);
    let sources: [(&str, std::path::PathBuf, usize); 7] = [
        ("hooks.log", logs.join("hooks.log"), LOG_LINES),
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
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in files {
        let (text, n) = hidden(&text, &known);
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

    /// The bundle opens, holds what a report needs, and never a key
    #[test]
    fn the_bundle_is_a_zip_of_what_a_report_needs() {
        let (name, bytes, _) = bundle().unwrap();
        assert!(name.starts_with("shikisha-diagnostics-") && name.ends_with(".zip"));
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut about = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("about.txt").unwrap(), &mut about).unwrap();
        assert!(about.contains(env!("CARGO_PKG_VERSION")), "{about}");
    }
}
