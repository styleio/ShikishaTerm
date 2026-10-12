//! What a listening port is, and where to open it, for the ports panel.
//!
//! A port's number and the program holding it say little: a project that runs
//! ten servers is `node.exe` ten times over. And a server is not always
//! reached by opening `localhost` -- one that checks where a request came from
//! (its cookies, its CSRF token, its CORS list) works only at the address it
//! expects, and a browser takes `localhost` and `127.0.0.1` for two sites.
//! Three things say more, from the surest to the least:
//!
//! - **The address the server printed** when it started (`Local:
//!   http://127.0.0.1:5173/`), read off the tab's terminal. Nothing but the
//!   server says which address it expects, so a printed one is where the row
//!   opens -- when the address the port is bound on agrees ([`pick`])
//! - **The folder it works in and the command that started it**, asked of the
//!   system (`repo::Proc`) and said in a few words ([`describe`])
//! - **A name an AI writes** from those, in a folder that said yes to automatic
//!   names (`crate::labels`): the one of the three that costs anything, so it
//!   is asked once per server and kept ([`Names`])

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;

/// What the panel says about one port, beside its number and program.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct About {
    /// The folder the server works in and what it runs, in a few words:
    /// `apps/web · vite`
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub what: String,
    /// The command that started it, with what looks like a secret taken out
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Where it opens, as the server printed it: `scheme://host:port`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The name an AI gave it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The words printed on the line with its address, for the AI that names
    /// it. Not sent to the screen, which has the terminal for that
    #[serde(skip)]
    pub said: Option<String>,
    /// The folder the server works in, for what the AI is told about it
    #[serde(skip)]
    pub cwd: Option<std::path::PathBuf>,
}

// ── Addresses printed on a terminal ───────────────────────────────

/// A web address read off a terminal: where it points, and what else its
/// line said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// `scheme://host`, with `:port` when it is not the scheme's own
    pub origin: String,
    /// As written, an IPv6 one in its brackets, in lower case
    pub host: String,
    pub port: u16,
    /// The rest of the line, without the address and the lines of a table
    pub line: String,
}

/// The longest a line kept beside an address may be, in characters
const LINE_ROOM: usize = 160;

/// The web addresses in `text`, oldest first.
///
/// An address ends where a space, a quote, a bracket or any character outside
/// ASCII does -- a terminal line is often `http://127.0.0.1:5173を開く` or the
/// cell of a table -- and loses the punctuation a sentence puts after it. One
/// with a name and password in it is not taken: what opens it is not this
pub fn seen_in(text: &str) -> Vec<Seen> {
    let mut out = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let mut from = 0;
        let mut found: Vec<(usize, usize, Seen)> = Vec::new();
        while let Some(at) = next_scheme(&lower, from) {
            let end = line[at..]
                .char_indices()
                .find(|(_, c)| !c.is_ascii() || c.is_ascii_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '|' | '(' | ')' | '[' | ']' | '{' | '}'))
                .map_or(line.len(), |(i, _)| at + i);
            // ...except the brackets of an IPv6 host, which belong to it
            let end = ipv6_end(line, at).filter(|e| *e > end).map_or(end, |e| widen(line, e));
            let mut raw = &line[at..end];
            while let Some(c) = raw.chars().last() {
                if matches!(c, '.' | ',' | ';' | ':' | '!' | '?') {
                    raw = &raw[..raw.len() - 1];
                } else {
                    break;
                }
            }
            from = end.max(at + 1);
            if let Some(seen) = parse(raw) {
                found.push((at, at + raw.len(), seen));
            }
        }
        if found.is_empty() {
            continue;
        }
        // What the line says around its addresses, said once for all of them
        let mut rest = String::new();
        let mut last = 0;
        for (a, b, _) in &found {
            rest.push_str(&line[last..*a]);
            rest.push(' ');
            last = *b;
        }
        rest.push_str(&line[last..]);
        let words: String = rest
            .chars()
            .map(|c| if is_rule(c) { ' ' } else { c })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let words: String = words.chars().take(LINE_ROOM).collect();
        for (_, _, mut seen) in found {
            seen.line = words.clone();
            out.push(seen);
        }
    }
    out
}

/// Where the next `http://` or `https://` starts in a lowered line
fn next_scheme(lower: &str, from: usize) -> Option<usize> {
    let a = lower[from..].find("http://").map(|i| from + i);
    let b = lower[from..].find("https://").map(|i| from + i);
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// For an address whose host is in brackets, where the closing one is
fn ipv6_end(line: &str, at: usize) -> Option<usize> {
    let after = at + line[at..].find("://")? + 3;
    line[after..].starts_with('[').then(|| line[after..].find(']').map(|i| after + i + 1))?
}

/// From the end of a bracketed host, on to where the address ends
fn widen(line: &str, from: usize) -> usize {
    line[from..]
        .char_indices()
        .find(|(_, c)| !c.is_ascii() || c.is_ascii_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '|' | ')' | ']'))
        .map_or(line.len(), |(i, _)| from + i)
}

/// The lines a table or a box is drawn with, and the marks a list starts with
fn is_rule(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{257f}' | '|' | '•' | '●' | '➜' | '→' | '▶' | '›' | '»')
}

fn parse(raw: &str) -> Option<Seen> {
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let usual = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']')?;
        h.parse::<std::net::Ipv6Addr>().ok()?;
        (format!("[{}]", h.to_ascii_lowercase()), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_ascii_lowercase(), Some(p)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':')) {
        return None;
    }
    let port = match port {
        Some(p) => p.parse::<u16>().ok().filter(|p| *p != 0)?,
        None => usual,
    };
    let origin = if port == usual { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{port}") };
    Some(Seen { origin, host, port, line: String::new() })
}

/// Of the addresses printed for `port`, the one to open it at, given the
/// addresses the port is bound on; `seen` oldest first.
///
/// A port bound on one particular address answers there alone, so a printed
/// address that names another is not it -- the same number in a sentence
/// about some other machine, or an earlier run. Bound everywhere, any address
/// can be it, and the loopback is preferred: it is how this PC reaches it
/// without going out onto the network. Of the ones that fit as well, the
/// latest printed. An address of `0.0.0.0` is where a server listens, not one
/// a browser can open, and is never chosen
pub fn pick<'a>(seen: impl IntoIterator<Item = &'a Seen>, port: u16, bound: &[IpAddr]) -> Option<&'a Seen> {
    let mut best: Option<(u8, &Seen)> = None;
    for s in seen {
        if s.port != port {
            continue;
        }
        let r = rank(&s.host, bound);
        if r > 0 && best.is_none_or(|(b, _)| r >= b) {
            best = Some((r, s));
        }
    }
    best.map(|(_, s)| s)
}

/// How well a printed host fits a port bound on `bound`: 0 not at all
fn rank(host: &str, bound: &[IpAddr]) -> u8 {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let ip = bare.parse::<IpAddr>().ok();
    if ip.is_some_and(|i| i.is_unspecified()) {
        return 0;
    }
    let loopback_name = bare == "localhost" || bare.ends_with(".localhost");
    if bound.is_empty() || bound.iter().any(|a| a.is_unspecified()) {
        return if loopback_name || ip.is_some_and(|i| i.is_loopback()) { 2 } else { 1 };
    }
    match ip {
        Some(i) if bound.contains(&i) => 2,
        Some(_) => 0,
        // A name for the loopback reaches a port on it, but the address
        // itself is what the server was told to expect
        None if loopback_name && bound.iter().any(|a| a.is_loopback()) => 1,
        // Any other name: what it stands for cannot be checked from here
        None => 0,
    }
}

/// The addresses read off one terminal, kept past the point where they
/// scroll out of what is read: a server prints its address once, at the start
#[derive(Debug, Default)]
pub struct Memory {
    pub seen: Vec<Seen>,
    /// How much the terminal had printed when it was last read: a terminal
    /// that printed nothing since is not read again
    pub read_at: Option<u64>,
}

/// The most addresses one terminal's memory keeps, the oldest dropped first
const KEEP: usize = 64;

impl Memory {
    /// What a fresh read found, `now` oldest first. What was found before is
    /// kept behind it, so a printed address read again counts as the latest
    pub fn take(&mut self, now: Vec<Seen>) {
        self.seen.retain(|s| !now.iter().any(|n| n.origin == s.origin && n.line == s.line));
        self.seen.extend(now);
        let over = self.seen.len().saturating_sub(KEEP);
        self.seen.drain(..over);
    }
}

// ── What a process is, in a few words ─────────────────────────────

/// The longest the command shown beside a port may be, in characters
const COMMAND_ROOM: usize = 400;

/// What a process holding a port is, in a few words: the folder it works in,
/// said from the tab's folder `home`, and what it runs (`apps/web · vite`).
/// Either is left out when there is nothing to say
pub fn describe(home: Option<&Path>, proc: &crate::repo::Proc) -> String {
    let at = proc.cwd.as_deref().and_then(|cwd| where_from(home, cwd));
    let runs = runs(&proc.argv);
    [at.unwrap_or_default(), runs].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// A folder said from `home`: the way down when it is inside, nothing when it
/// is `home` itself, and only its own name when it is somewhere else
fn where_from(home: Option<&Path>, cwd: &Path) -> Option<String> {
    if let Some(home) = home
        && crate::worktree::inside_checkout(home, cwd)
    {
        if crate::uistate::same_folder(home, cwd) {
            return None;
        }
        let home = crate::repo::tidy(home.to_path_buf());
        let cwd = crate::repo::tidy(cwd.to_path_buf());
        let deep: Vec<String> = cwd
            .components()
            .skip(home.components().count())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if !deep.is_empty() {
            return Some(deep.join("/"));
        }
    }
    cwd.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Programs that run somebody else's script, where the script says what is
/// running and the program does not
const RUNNERS: &[&str] = &["node", "nodejs", "bun"];

/// What a command runs, in a word or three: the tool a script belongs to
/// (`vite` for `node .../node_modules/vite/bin/vite.js`), or the program and
/// the words after it that are not flags or paths (`pnpm dev`)
fn runs(argv: &[String]) -> String {
    let Some(first) = argv.first() else { return String::new() };
    let program = stem(first);
    let rest: Vec<&String> = argv.iter().skip(1).filter(|a| !a.starts_with('-')).collect();
    let (head, after) = if RUNNERS.contains(&program.as_str()) {
        match rest.split_first() {
            Some((script, after)) => (tool_of(script), after.to_vec()),
            None => (program, Vec::new()),
        }
    } else {
        (program, rest)
    };
    let mut words = vec![head];
    words.extend(after.iter().take(2).filter(|w| is_word(w)).map(|w| w.to_string()));
    let said = words.join(" ");
    said.chars().take(40).collect()
}

/// A program's name without its folder or the ending Windows gives it
fn stem(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
    let lower = name.to_ascii_lowercase();
    for end in [".exe", ".cmd", ".bat", ".ps1"] {
        if lower.ends_with(end) {
            return name[..name.len() - end.len()].to_string();
        }
    }
    name
}

/// The tool a script belongs to: the package it was installed as
/// (`node_modules/vite/...` is `vite`, `node_modules/@scope/x/...` is
/// `@scope/x`), the command a package put in `.bin`, or else the script's name
fn tool_of(script: &str) -> String {
    let parts: Vec<&str> = script.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if let Some(i) = parts.iter().rposition(|p| *p == "node_modules") {
        match parts.get(i + 1) {
            Some(&".bin") => {
                if let Some(cmd) = parts.get(i + 2) {
                    return stem(cmd);
                }
            }
            Some(scope) if scope.starts_with('@') => {
                if let Some(name) = parts.get(i + 2) {
                    return format!("{scope}/{name}");
                }
            }
            Some(name) => return name.to_string(),
            None => {}
        }
    }
    stem(parts.last().copied().unwrap_or(script))
}

/// A word worth saying after a program's name: a subcommand like `dev` or
/// `runserver`, not a path, a file, a setting or a number (the port a flag
/// was given, which the row already says)
fn is_word(w: &str) -> bool {
    !w.is_empty()
        && !w.chars().all(|c| c.is_ascii_digit())
        && w.chars().count() <= 20
        && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
}

/// The words of a command that look like they hold a secret
const SECRET: &[&str] = &["token", "secret", "password", "passwd", "pwd", "apikey", "api-key", "api_key", "auth", "credential", "key"];

fn secretish(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET.iter().any(|s| name.contains(s))
}

/// The command a process was started with, to be shown and read, with what
/// looks like a secret taken out: the value of a flag or a setting whose name
/// says token, password, key and the like, and the name and password in an
/// address. Shown in full it would put a key on a phone's screen and into
/// what an AI is sent
pub fn command_line(argv: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut hide_next = false;
    for a in argv {
        let shown = if hide_next && !a.starts_with('-') {
            "***".to_string()
        } else if let Some((name, _)) = a.split_once('=').filter(|(n, _)| secretish(n)) {
            format!("{name}=***")
        } else {
            without_password(a)
        };
        hide_next = a.starts_with('-') && !a.contains('=') && secretish(a);
        out.push(if shown.contains(' ') { format!("\"{shown}\"") } else { shown });
    }
    let line = out.join(" ");
    if line.chars().count() > COMMAND_ROOM {
        let cut: String = line.chars().take(COMMAND_ROOM).collect();
        return format!("{cut}…");
    }
    line
}

/// `scheme://user:password@host` with the part before the `@` hidden
fn without_password(word: &str) -> String {
    match word.find("://") {
        Some(i) => {
            let after = i + 3;
            let authority_end = word[after..].find(['/', '?', '#']).map_or(word.len(), |j| after + j);
            match word[after..authority_end].rfind('@') {
                Some(at) => format!("{}***{}", &word[..after], &word[after + at..]),
                None => word.to_string(),
            }
        }
        None => word.to_string(),
    }
}

/// The name a folder's `package.json` gives its project, when it has one
pub fn package_name(cwd: &Path) -> Option<String> {
    let file = cwd.join("package.json");
    if std::fs::metadata(&file).ok()?.len() > 256 * 1024 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    v.get("name").and_then(|n| n.as_str()).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string)
}

// ── Names an AI wrote ─────────────────────────────────────────────

/// Where the names are kept, beside the app's other state
const NAMES: &str = "port-names.json";

/// The most names kept; past it the ones written longest ago go
const NAMES_KEPT: usize = 500;

/// The names AIs gave ports, kept from one run to the next: a server started
/// again the same way, in the same folder, on the same port, is the same
/// server, and asking again would be paying twice for the one answer.
#[derive(Debug, Default)]
pub struct Names {
    /// By [`key`]: the name, and when it was written (seconds since 1970)
    all: BTreeMap<String, (String, u64)>,
}

/// What makes two servers the same one for a name: the folder they are in,
/// the port, and how they were started
pub fn key(folder: &Path, port: u16, about: &About) -> String {
    format!("{}|{port}|{}|{}", folder.display(), about.what, about.command)
}

impl Names {
    pub fn load() -> Self {
        let text = std::fs::read_to_string(crate::config::state_path(NAMES)).unwrap_or_default();
        let v: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
        let all = v
            .into_iter()
            .filter_map(|(k, v)| {
                let name = v.get("name")?.as_str()?.to_string();
                Some((k, (name, v.get("at").and_then(|a| a.as_u64()).unwrap_or(0))))
            })
            .collect();
        Self { all }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.all.get(key).map(|(n, _)| n.as_str())
    }

    /// Keep these names and write them down
    pub fn put(&mut self, named: impl IntoIterator<Item = (String, String)>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for (k, n) in named {
            self.all.insert(k, (n, now));
        }
        while self.all.len() > NAMES_KEPT {
            let Some(oldest) = self.all.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone()) else { break };
            self.all.remove(&oldest);
        }
        let v: serde_json::Map<String, serde_json::Value> = self
            .all
            .iter()
            .map(|(k, (n, at))| (k.clone(), serde_json::json!({"name": n, "at": at})))
            .collect();
        if let Ok(text) = serde_json::to_string_pretty(&v) {
            let _ = crate::crypto::write_atomic(&crate::config::state_path(NAMES), &text);
        }
    }
}

/// One port as an AI is told about it
#[derive(Debug, Clone, Default)]
pub struct Asked {
    pub port: u16,
    pub program: String,
    pub about: About,
    /// What the folder it works in calls its project (`package.json`)
    pub package: Option<String>,
}

/// The longest a name may be, in columns: what fits on a row of the panel
pub const NAME_ROOM: usize = 32;

/// What an AI is asked to name the ports `asked` with, in folder `folder`.
/// Only what the system and the terminal said about each server goes: its
/// program, where it runs, its command with secrets taken out, its project's
/// name and the line its address was printed on
pub fn prompt(folder: &str, asked: &[Asked]) -> String {
    let mut ports = String::new();
    for a in asked {
        ports.push_str(&format!("Port {}\n", a.port));
        let mut said = |what: &str, value: &str| {
            if !value.trim().is_empty() {
                ports.push_str(&format!("- {what}: {}\n", value.trim()));
            }
        };
        said("program", &a.program);
        said("runs as", &a.about.what);
        said("project", a.package.as_deref().unwrap_or_default());
        said("command", &a.about.command);
        said("address", a.about.url.as_deref().unwrap_or_default());
        said("printed beside its address", a.about.said.as_deref().unwrap_or_default());
        ports.push('\n');
    }
    crate::i18n::tp(
        "ai.ports.prompt",
        &[
            ("folder", folder),
            ("ports", ports.trim_end()),
            ("language", &crate::i18n::t("ai.label.language")),
        ],
    )
}

/// The names in an AI's answer, for the ports it was asked about and no
/// others, each cut to fit a row
pub fn names_in(answer: &str, asked: &[u16]) -> Result<BTreeMap<u16, String>, String> {
    let v: serde_json::Value = serde_json::from_str(answer).map_err(|e| e.to_string())?;
    let obj = v.as_object().ok_or("not a JSON object")?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        let Ok(port) = k.trim().trim_start_matches(':').parse::<u16>() else { continue };
        let Some(name) = v.as_str() else { continue };
        let name = crate::labels::fit(name, NAME_ROOM);
        if asked.contains(&port) && !name.is_empty() {
            out.insert(port, name);
        }
    }
    if out.is_empty() {
        return Err("no port was named".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn a_dev_servers_printed_address_is_read_with_its_port() {
        let seen = seen_in("  VITE v5.0.0  ready in 300 ms\n\n  ➜  Local:   http://127.0.0.1:5173/\n  ➜  Network: use --host to expose\n");
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].origin, "http://127.0.0.1:5173");
        assert_eq!(seen[0].port, 5173);
        assert_eq!(seen[0].line, "Local:");
    }

    /// The table an AI draws of what it started: each row's address, and the
    /// words in its other cells
    #[test]
    fn an_address_in_a_table_keeps_the_words_of_its_row() {
        let text = "│ 画面 │ URL │\n│ 利用企業コンソール │ http://127.0.0.1:5173 │\n│ 運営コンソール │ http://127.0.0.1:5175 │";
        let seen = seen_in(text);
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!((seen[0].port, seen[0].line.as_str()), (5173, "利用企業コンソール"));
        assert_eq!((seen[1].port, seen[1].line.as_str()), (5175, "運営コンソール"));
    }

    #[test]
    fn an_address_ends_where_the_sentence_goes_on() {
        let seen = seen_in("開くのは http://localhost:3000を見てください. Or https://[::1]:8443/x, then.");
        let origins: Vec<&str> = seen.iter().map(|s| s.origin.as_str()).collect();
        assert_eq!(origins, ["http://localhost:3000", "https://[::1]:8443"]);
        let seen = seen_in("(see http://example.test:8080).");
        assert_eq!(seen[0].origin, "http://example.test:8080");
        // A port of the scheme's own, said or not
        let seen = seen_in("http://site.test/ https://site.test:443/");
        assert_eq!((seen[0].port, seen[1].port), (80, 443));
        assert_eq!(seen[1].origin, "https://site.test");
        // Not an address: a password in it, a port that is not one
        assert!(seen_in("http://me:pw@localhost:3000 http://localhost:99999 ftp://x:21").is_empty());
    }

    /// The address a server expects is the one it printed, and the address it
    /// is bound on says which printed ones can be it
    #[test]
    fn the_printed_address_that_fits_the_bound_one_is_picked() {
        let seen = seen_in(
            "Local: http://127.0.0.1:5173/\nalso http://localhost:5173 and http://192.168.1.20:5173\nhttp://0.0.0.0:5173\nhttp://127.0.0.1:9999",
        );
        // Bound to 127.0.0.1 alone: that address, even with a later localhost
        let at = pick(&seen, 5173, &[ip("127.0.0.1")]).unwrap();
        assert_eq!(at.origin, "http://127.0.0.1:5173");
        // Bound everywhere: the loopback, the latest of the loopback ones
        let at = pick(&seen, 5173, &[ip("0.0.0.0")]).unwrap();
        assert_eq!(at.origin, "http://localhost:5173");
        // Bound on the LAN alone: only that address
        let at = pick(&seen, 5173, &[ip("192.168.1.20")]).unwrap();
        assert_eq!(at.origin, "http://192.168.1.20:5173");
        // Bound on the IPv6 loopback: the name for it
        let at = pick(&seen, 5173, &[ip("::1")]).unwrap();
        assert_eq!(at.origin, "http://localhost:5173");
        // Bound somewhere nothing printed names: none
        assert!(pick(&seen, 5173, &[ip("10.0.0.9")]).is_none());
        // Another port's address is not this one's
        assert!(pick(&seen, 4000, &[ip("0.0.0.0")]).is_none());
        // Where a server listens is not an address to open
        let only_any = seen_in("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/)");
        assert!(pick(&only_any, 8000, &[ip("0.0.0.0")]).is_none());
    }

    #[test]
    fn https_printed_is_opened_as_https() {
        let seen = seen_in("  ➜  Local:   https://localhost:5173/");
        assert_eq!(pick(&seen, 5173, &[ip("::1")]).unwrap().origin, "https://localhost:5173");
    }

    #[test]
    fn a_memory_keeps_what_scrolled_away_and_counts_a_reprint_as_latest() {
        let mut m = Memory::default();
        m.take(seen_in("http://127.0.0.1:5173\nhttp://localhost:5173"));
        m.take(seen_in("http://localhost:4000"));
        assert_eq!(m.seen.len(), 3);
        m.take(seen_in("http://127.0.0.1:5173"));
        assert_eq!(m.seen.last().unwrap().origin, "http://127.0.0.1:5173");
        assert_eq!(m.seen.len(), 3);
        for i in 0..100 {
            m.take(seen_in(&format!("http://localhost:{}", 6000 + i)));
        }
        assert_eq!(m.seen.len(), KEEP);
    }

    #[test]
    fn a_server_is_said_by_its_folder_and_what_it_runs() {
        let win = |p: &str| std::path::PathBuf::from(crate::local_path(&format!("D:/{p}")));
        let home = win("work/shop");
        let proc = |cwd: &str, argv: &[&str]| crate::repo::Proc {
            cwd: Some(win(cwd)),
            argv: argv.iter().map(|s| s.to_string()).collect(),
        };
        let vite = proc(
            "work/shop/apps/admin",
            &["C:\\Program Files\\nodejs\\node.exe", "D:\\work\\shop\\node_modules\\vite\\bin\\vite.js", "--port", "5173"],
        );
        assert_eq!(describe(Some(&home), &vite), "apps/admin · vite");
        let next = proc("work/shop", &["node", "/w/node_modules/next/dist/bin/next", "dev"]);
        assert_eq!(describe(Some(&home), &next), "next dev");
        let scoped = proc("work/shop/api", &["node", "/w/node_modules/@nestjs/cli/bin/nest.js", "start", "--watch"]);
        assert_eq!(describe(Some(&home), &scoped), "api · @nestjs/cli start");
        let bin = proc("work/shop", &["node", "/w/node_modules/.bin/tsx", "watch", "src/index.ts"]);
        assert_eq!(describe(Some(&home), &bin), "tsx watch");
        let pnpm = proc("work/other", &["pnpm.cmd", "run", "dev"]);
        assert_eq!(describe(Some(&home), &pnpm), "other · pnpm run dev");
        let py = proc("work/shop", &["python.exe", "manage.py", "runserver"]);
        assert_eq!(describe(Some(&home), &py), "python runserver");
        assert_eq!(describe(None, &crate::repo::Proc::default()), "");
    }

    #[test]
    fn a_command_is_shown_without_its_secrets() {
        let argv: Vec<String> = [
            "node",
            "server.js",
            "--token",
            "abc123",
            "--api-key=zzz",
            "DB_PASSWORD=hunter2",
            "--db",
            "postgres://app:s3cret@localhost:5432/app",
            "--port",
            "3000",
            "C:\\Program Files\\x",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let line = command_line(&argv);
        for secret in ["abc123", "zzz", "hunter2", "s3cret", "app:"] {
            assert!(!line.contains(secret), "{secret} is shown: {line}");
        }
        assert!(line.contains("--port 3000") && line.contains("postgres://***@localhost:5432/app"), "{line}");
        assert!(line.contains("\"C:\\Program Files\\x\""), "{line}");
    }

    #[test]
    fn an_answer_names_only_the_ports_asked_about() {
        let got = names_in(r#"{"5173": "利用企業コンソール", ":8721": "API", "9999": "nope", "x": "y"}"#, &[5173, 8721]).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[&5173], "利用企業コンソール");
        assert!(names_in("{}", &[1]).is_err());
        assert!(names_in("not json", &[1]).is_err());
    }
}
