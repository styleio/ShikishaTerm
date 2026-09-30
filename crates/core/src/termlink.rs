//! Web addresses and file paths in what a terminal is showing.
//!
//! The screen is turned into HTML in one place (`shell::screen_rows`), and the
//! window, a phone and the pictures of the panes nobody is looking at are all
//! handed that HTML. So this is where a place worth pressing is recognised: once,
//! on the text of a line, with no idea of which folder or which machine it is
//! in. Whether a path really names a file is asked when it is pressed -- the
//! screen is drawn several times a second, and a question to the disk per row
//! per frame is a cost nobody asked for.
//!
//! Because nothing is checked here, the rules lean towards saying nothing: a
//! word that merely has a slash in it (`and/or`, `2026/09/29`, a slash command
//! like `/help`) is left as words, and an underline appears only on what reads
//! as a path to a person too.

/// What was found
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An `http` or `https` address
    Web,
    /// A path on the machine the terminal runs on, perhaps with a line and column
    File,
}

/// One place in a line, as character positions `[from, to)` of what was handed in
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub from: usize,
    pub to: usize,
    pub kind: Kind,
    /// Exactly the characters it covers
    pub text: String,
}

/// The longest address this will call one. A signed download link or a sign-in
/// redirect carries a query of one or two thousand characters; twice that
/// leaves room for the longest of those, and anything longer on a terminal is
/// an encoded blob that happens to start with a scheme -- and scanning a screen
/// full of one for its end is time spent on nothing
const WEB_LONGEST: usize = 4096;

/// Every place worth pressing in one line of text, in order. Addresses first:
/// a path is looked for only where no address already is, so the `/path` of
/// an address is never offered as a file of its own
pub fn find(line: &[char]) -> Vec<Found> {
    let mut out = webs(line);
    let mut taken = vec![false; line.len()];
    for f in &out {
        for t in &mut taken[f.from..f.to] {
            *t = true;
        }
    }
    out.extend(files(line, &taken));
    out.sort_by_key(|f| f.from);
    out
}

fn starts_with_at(line: &[char], at: usize, word: &str) -> bool {
    let mut i = at;
    for w in word.chars() {
        match line.get(i) {
            Some(c) if c.eq_ignore_ascii_case(&w) => i += 1,
            _ => return false,
        }
    }
    true
}

/// A character that cannot be inside an address as it is written on a screen:
/// the ones RFC 3986 never lets through unescaped, and -- outside ASCII --
/// anything that is not a letter or a digit. Unescaped Japanese in a path is
/// common and kept; a full-width bracket, comma or space ends the address,
/// because that is the sentence around it starting again
fn web_stops_at(c: char) -> bool {
    c.is_whitespace()
        || matches!(c, '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}')
        || (!c.is_ascii() && !c.is_alphanumeric())
}

/// What a sentence puts after an address and is not part of it
fn web_leaves(c: char) -> bool {
    matches!(c, '.' | ',' | ':' | ';' | '?' | '!' | '\'' | '*')
}

fn webs(line: &[char]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let scheme = if starts_with_at(line, i, "https://") {
            8
        } else if starts_with_at(line, i, "http://") {
            7
        } else {
            i += 1;
            continue;
        };
        // `xhttp://` is a word, not an address
        if i > 0 && (line[i - 1].is_ascii_alphanumeric() || line[i - 1] == '_') {
            i += 1;
            continue;
        }
        let mut end = i + scheme;
        while end < line.len() && end - i <= WEB_LONGEST && !web_stops_at(line[end]) {
            end += 1;
        }
        let scanned = end;
        end = trim_closers(line, i, end, web_leaves);
        let host = line.get(i + scheme).copied();
        let hosted = host.is_some_and(|h| h.is_alphanumeric() || h == '[') && end > i + scheme;
        if hosted && end - i <= WEB_LONGEST {
            out.push(Found { from: i, to: end, kind: Kind::Web, text: line[i..end].iter().collect() });
        }
        i = scanned.max(i + 1);
    }
    out
}

/// Takes off what a sentence left at the end of `line[from..to]`: the
/// characters `leaves` names, and a closing bracket whose opener is not inside.
/// `(see https://x.org/a_(b))` keeps the address's own pair and gives the
/// sentence back its last one
fn trim_closers(line: &[char], from: usize, mut to: usize, leaves: fn(char) -> bool) -> usize {
    loop {
        let Some(&last) = line.get(to.wrapping_sub(1)).filter(|_| to > from) else { return to };
        let opener = match last {
            ')' => Some('('),
            ']' => Some('['),
            _ => None,
        };
        if let Some(open) = opener {
            let opens = line[from..to].iter().filter(|&&c| c == open).count();
            let closes = line[from..to].iter().filter(|&&c| c == last).count();
            if closes > opens {
                to -= 1;
                continue;
            }
            return to;
        }
        if leaves(last) {
            to -= 1;
            continue;
        }
        return to;
    }
}

/// A character a path on a screen is made of. Letters and digits of any
/// script (a folder called 資料 is a folder), the punctuation file names
/// actually carry, both separators, and the colon -- which is only allowed
/// where a drive or a line number puts it, and is checked afterwards.
///
/// The middle dots and wave dashes are here because Japanese names use them
/// (`議事録・2026〜.md`); leaving them out cut such a name in two and offered
/// half of it
fn path_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c, '.' | '_' | '-' | '~' | '/' | '\\' | '%' | '+' | '@' | '(' | ')' | '[' | ']' | ':' | '#' | '$' | '=')
        || matches!(c, '・' | '･' | '〜' | '～')
}

/// Whether the last character of a full row and the first of the next can be
/// the two sides of one address or path broken by the edge of the screen.
///
/// A pseudo console that draws a screen again places each row itself, with
/// a line break of its own, and the terminal is never told that a row ran on
/// past the edge. So a row that is full to its last column, ending in
/// something an address or a path is made of, followed by a row that starts
/// with the same, is read as one line. What a sentence ends with (a full stop,
/// a comma, a closing bracket) is not taken for a break in the middle of one
pub fn runs_on(last: char, first: char) -> bool {
    let inside = |c: char| !c.is_whitespace() && (!web_stops_at(c) || path_char(c));
    inside(last) && inside(first) && !matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '\'' | '"')
}

fn path_leaves(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':')
}

fn files(line: &[char], taken: &[bool]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        if !path_char(line[i]) || taken[i] {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < line.len() && path_char(line[end]) && !taken[end] {
            end += 1;
        }
        if let Some((from, to)) = as_path(line, i, end) {
            out.push(Found { from, to, kind: Kind::File, text: line[from..to].iter().collect() });
        }
        i = end;
    }
    out
}

/// The part of a run of path characters that is a path, if any
fn as_path(line: &[char], mut from: usize, to: usize) -> Option<(usize, usize)> {
    // A bracket in front is the sentence's: a path does not start with one
    // (`app/(shop)/page.tsx` has its pair inside). What it opened is closed at
    // the end, and `trim_closers` hands that back too
    while from < to && matches!(line[from], '(' | '[') {
        from += 1;
    }
    let to = trim_closers(line, from, to, path_leaves);
    if to <= from {
        return None;
    }
    let text: String = line[from..to].iter().collect();
    let place = place_of(&text);
    reads_as_path(&place.path, place.line.is_some()).then_some((from, to))
}

/// A found path taken apart: the file, and where in it, when the text said
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// Splits `src/main.rs:12:5` into the file and where in it. The numbers are
/// only taken off the end, one or two of them, so a drive's colon is left alone
pub fn place_of(text: &str) -> Place {
    let tail = |s: &str| -> Option<(String, u32)> {
        let (head, n) = s.rsplit_once(':')?;
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || head.is_empty() {
            return None;
        }
        // `C:5` is a drive, not line five of a file called C
        if head.len() == 1 && head.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        Some((head.to_string(), n.parse().ok()?))
    };
    match tail(text) {
        Some((head, last)) => match tail(&head) {
            Some((path, line)) => Place { path, line: Some(line), column: Some(last) },
            None => Place { path: head, line: Some(last), column: None },
        },
        None => Place { path: text.to_string(), line: None, column: None },
    }
}

/// A `file://` address, read: which machine it names and the path there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileUrl {
    /// The machine the address names. `None` when it names none -- an empty
    /// host or `localhost`, which both mean "the machine this was said on"
    pub host: Option<String>,
    /// The path, decoded and written the way that machine writes one:
    /// `/C:/x%20y.txt` is `C:/x y.txt`, `/home/me/a` stays as it is
    pub path: String,
}

/// A program's `file://` hyperlink, or a shell's `OSC 7` address, read.
/// Anything that is not a file address is `None`
pub fn file_url(uri: &str) -> Option<FileUrl> {
    let rest = uri.get(..7).filter(|s| s.eq_ignore_ascii_case("file://")).map(|_| &uri[7..])?;
    // RFC 8089: the host is everything before the path's first slash
    let slash = rest.find('/')?;
    let host = unescape(&rest[..slash])?;
    let decoded = unescape(&rest[slash..])?;
    // `/C:/x` is how a drive is written in an address
    let d = decoded.as_bytes();
    let path = if d.len() >= 3 && d[0] == b'/' && d[1].is_ascii_alphabetic() && d[2] == b':' {
        decoded[1..].to_string()
    } else {
        decoded
    };
    if path.is_empty() || path.chars().any(char::is_control) || host.chars().any(char::is_control) {
        return None;
    }
    let host = (!host.is_empty() && !host.eq_ignore_ascii_case("localhost")).then_some(host);
    Some(FileUrl { host, path })
}

/// `%XX` back into bytes, read as UTF-8
fn unescape(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            raw.push(v);
            i += 3;
            continue;
        }
        raw.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(raw).ok()
}

/// Where a pressed `file://` place is, as seen from the tab it was pressed on
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileHome {
    /// On the tab's own machine, at this path
    Here(String),
    /// On another Windows computer on the network: the UNC path
    /// (`\\server\share\file`) this PC opens it by
    Share(String),
    /// On another machine this tab cannot reach it on; the machine's name
    Away(String),
}

/// Where a `file://` place is. A host that names no machine, or names the
/// tab's own (`names`: every name that machine is known by here), is the
/// tab's own machine -- programs such as `ls --hyperlink` write their own
/// machine's name there. Any other host is another computer: from a tab on
/// this PC that is a file share, the standard Windows meaning of a host in a
/// file address; from a tab on another machine, somewhere it cannot reach.
/// Another computer's drive letter is not reachable from anywhere here
pub fn file_home(url: &FileUrl, names: &[String], far: bool) -> FileHome {
    let Some(host) = &url.host else { return FileHome::Here(url.path.clone()) };
    if names.iter().any(|n| same_machine(host, n)) {
        return FileHome::Here(url.path.clone());
    }
    let drive = url.path.as_bytes().get(1) == Some(&b':');
    let parts: Vec<&str> = url.path.split('/').filter(|p| !p.is_empty()).collect();
    if far || drive || parts.is_empty() {
        return FileHome::Away(host.clone());
    }
    FileHome::Share(format!("\\\\{host}\\{}", parts.join("\\")))
}

/// Whether two names are one machine: the same name in any case, or the
/// same first part when one of them is written out in full with its domain
/// (`build-01` and `build-01.corp.example`)
fn same_machine(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let first = |s: &str| s.split('.').next().unwrap_or(s).to_ascii_lowercase();
    a.eq_ignore_ascii_case(b) || ((a.contains('.') || b.contains('.')) && first(a) == first(b))
}

/// A path from a screen on this PC, made whole: `~` is the home folder, a
/// relative path is under `base` (where the terminal is), and Git Bash's
/// `/c/Users/...` is the drive it names. `None` when there is no telling
/// where it is -- a relative path with no `base`, or `/usr/bin` on a PC that
/// has no such folder of its own
pub fn resolve_here(path: &str, base: Option<&std::path::Path>, home: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    use std::path::{Component, Path, PathBuf};
    let rest_of = |p: &str| p.get(2..).map(str::to_string);
    let joined: PathBuf = if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        home?.join(rest_of(path).unwrap_or_default())
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else if cfg!(windows) && (path.starts_with('/') || path.starts_with('\\')) && !path.starts_with("\\\\") {
        // `/c/work` is how Git Bash and MSYS write C:\work. Any other path
        // from the root is a folder of another system (WSL, a container), not
        // one this PC can open
        let b = path.as_bytes();
        if b.len() >= 3 && b[1].is_ascii_alphabetic() && (b[2] == b'/' || b[2] == b'\\') {
            PathBuf::from(format!("{}:\\{}", (b[1] as char).to_ascii_uppercase(), &path[3..]))
        } else if b.len() == 2 && b[1].is_ascii_alphabetic() {
            PathBuf::from(format!("{}:\\", (b[1] as char).to_ascii_uppercase()))
        } else {
            return None;
        }
    } else {
        base?.join(path)
    };
    // `..` worked out on the words, not the disk: a path to a file that is not
    // there yet is still somewhere, and saying where is the useful answer
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// Whether Windows would run this file rather than show it. "Open with the
/// program for it" is offered on a file somebody clicked in a terminal; for
/// these that program is the file itself, and a press on a name should never
/// be what starts a script. The PC's own list (`PATHEXT`) and the kinds that
/// run without being on it
pub fn runs_when_opened(path: &std::path::Path) -> bool {
    let Some(kind) = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) else {
        return false;
    };
    let listed = std::env::var("PATHEXT").unwrap_or_else(|_| ".com;.exe;.bat;.cmd".to_string());
    if listed.split(';').any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&kind)) {
        return true;
    }
    matches!(
        kind.as_str(),
        "exe" | "com" | "bat" | "cmd" | "ps1" | "psm1" | "vbs" | "vbe" | "js" | "jse" | "wsf" | "wsh" | "msi"
            | "msp" | "msc" | "lnk" | "url" | "reg" | "hta" | "scr" | "cpl" | "pif" | "jar" | "appref-ms"
            | "application" | "inf" | "sct" | "settingcontent-ms"
    )
}

/// A path from a screen on another machine, made whole against where the
/// terminal there is (always `/`-separated)
pub fn resolve_there(path: &str, base: &str) -> String {
    let full = if path.starts_with('/') { path.to_string() } else { format!("{}/{path}", base.trim_end_matches('/')) };
    let mut out: Vec<&str> = Vec::new();
    for part in full.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            one => out.push(one),
        }
    }
    format!("/{}", out.join("/"))
}

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Whether a file name ends in something that looks like a kind of file:
/// `main.rs`, `README.md`, `.gitignore` is not one (a leading dot is a hidden
/// file's name, not its kind), and a "kind" of more than a dozen characters is
/// the rest of a sentence
fn has_kind(name: &str) -> bool {
    match name.rfind('.') {
        Some(0) | None => false,
        Some(dot) => {
            let kind = &name[dot + 1..];
            (1..=12).contains(&kind.chars().count()) && kind.chars().all(|c| c.is_alphanumeric() || c == '_')
        }
    }
}

/// Whether text (with any line number already taken off) is a path a person
/// would recognise as one
fn reads_as_path(path: &str, numbered: bool) -> bool {
    let chars: Vec<char> = path.chars().collect();
    if !chars.iter().any(|&c| is_sep(c)) || !chars.iter().any(|c| c.is_alphanumeric()) {
        return false;
    }
    let drive = chars.len() >= 3 && chars[0].is_ascii_alphabetic() && chars[1] == ':' && is_sep(chars[2]);
    // The only colon a path may keep is its drive's
    if chars.iter().skip(if drive { 2 } else { 0 }).any(|&c| c == ':') {
        return false;
    }
    let parts: Vec<&str> = path
        .split(is_sep)
        .skip(if drive { 1 } else { 0 })
        .filter(|p| !p.is_empty() && *p != "." && *p != ".." && *p != "~")
        .collect();
    if parts.is_empty() {
        return false;
    }
    // Dates, fractions and version counters, not files
    if parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())) {
        return false;
    }
    let last = parts[parts.len() - 1];
    if drive || path.starts_with("\\\\") {
        return true;
    }
    if path.starts_with("//") {
        // A comment marker or an address without its scheme
        return false;
    }
    if ["~/", "~\\", "./", ".\\", "../", "..\\"].iter().any(|p| path.starts_with(p)) {
        return true;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        // `/help` and `/compact` are what an AI tool is typed; a real
        // absolute path has a folder in it, or is a file with a kind
        return parts.len() >= 2 || has_kind(last);
    }
    // Relative with nothing to anchor it: it needs one more thing a person
    // would read as a file -- a kind, a line number, or a folder in a folder
    has_kind(last) || numbered || parts.len() >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(s: &str) -> Vec<(Kind, String)> {
        let chars: Vec<char> = s.chars().collect();
        find(&chars).into_iter().map(|f| (f.kind, f.text)).collect()
    }
    fn web(s: &str) -> (Kind, String) {
        (Kind::Web, s.to_string())
    }
    fn file(s: &str) -> (Kind, String) {
        (Kind::File, s.to_string())
    }

    #[test]
    fn an_address_in_a_sentence_leaves_the_sentence_its_punctuation() {
        assert_eq!(found("see https://example.com/a?b=1."), vec![web("https://example.com/a?b=1")]);
        assert_eq!(found("(https://example.com/x)"), vec![web("https://example.com/x")]);
        assert_eq!(found("\"http://localhost:3000/\""), vec![web("http://localhost:3000/")]);
        assert_eq!(found("<https://a.io>"), vec![web("https://a.io")]);
    }

    #[test]
    fn an_address_keeps_brackets_of_its_own() {
        assert_eq!(
            found("read https://en.wikipedia.org/wiki/Rust_(language) now"),
            vec![web("https://en.wikipedia.org/wiki/Rust_(language)")]
        );
    }

    #[test]
    fn japanese_in_an_address_stays_and_japanese_punctuation_ends_it() {
        assert_eq!(found("https://ja.wikipedia.org/wiki/端末、次へ"), vec![web("https://ja.wikipedia.org/wiki/端末")]);
        assert_eq!(found("「https://example.jp/a」"), vec![web("https://example.jp/a")]);
    }

    #[test]
    fn a_scheme_glued_to_a_word_is_not_an_address() {
        assert_eq!(found("xhttps://example.com"), vec![]);
        assert_eq!(found("https://"), vec![]);
        assert_eq!(found("https:// nothing"), vec![]);
    }

    #[test]
    fn a_path_of_an_address_is_not_offered_again_as_a_file() {
        assert_eq!(found("https://github.com/a/b/blob/main/src/x.rs"), vec![web("https://github.com/a/b/blob/main/src/x.rs")]);
    }

    #[test]
    fn compiler_places_are_files_with_where_in_them() {
        assert_eq!(found("  --> src/main.rs:12:5"), vec![file("src/main.rs:12:5")]);
        assert_eq!(found("at Object.<anonymous> (lib/a.js:3:9)"), vec![file("lib/a.js:3:9")]);
        assert_eq!(found("error in C:\\work\\app\\main.go:40."), vec![file("C:\\work\\app\\main.go:40")]);
        let p = place_of("src/main.rs:12:5");
        assert_eq!(p, Place { path: "src/main.rs".into(), line: Some(12), column: Some(5) });
        let p = place_of("C:\\a\\b.txt:7");
        assert_eq!(p, Place { path: "C:\\a\\b.txt".into(), line: Some(7), column: None });
        assert_eq!(place_of("C:\\a").path, "C:\\a");
    }

    #[test]
    fn anchored_paths_are_files_however_they_start() {
        assert_eq!(found("cd ~/projects/site"), vec![file("~/projects/site")]);
        assert_eq!(found("run ./build.sh"), vec![file("./build.sh")]);
        assert_eq!(found("up ../x"), vec![file("../x")]);
        assert_eq!(found("open D:/work/notes"), vec![file("D:/work/notes")]);
        assert_eq!(found("share \\\\nas\\team\\doc"), vec![file("\\\\nas\\team\\doc")]);
        assert_eq!(found("log at /var/log/syslog"), vec![file("/var/log/syslog")]);
    }

    #[test]
    fn japanese_names_are_kept_whole() {
        assert_eq!(found("保存: docs/議事録・2026〜秋.md を見て"), vec![file("docs/議事録・2026〜秋.md")]);
        assert_eq!(found("C:\\Users\\山田\\資料\\見積～最終.xlsx"), vec![file("C:\\Users\\山田\\資料\\見積～最終.xlsx")]);
    }

    #[test]
    fn words_with_a_slash_in_them_are_left_as_words() {
        assert_eq!(found("this and/or that"), vec![]);
        assert_eq!(found("on 2026/09/29 at 1/2 speed"), vec![]);
        assert_eq!(found("type /help or /compact"), vec![]);
        assert_eq!(found("// a comment"), vec![]);
        assert_eq!(found("a\\nb"), vec![]);
        assert_eq!(found("ratio: a:b/c"), vec![]);
    }

    #[test]
    fn relative_paths_need_one_more_sign_of_being_a_file() {
        assert_eq!(found("edit src/lib.rs"), vec![file("src/lib.rs")]);
        assert_eq!(found("see crates/core/src"), vec![file("crates/core/src")]);
        assert_eq!(found("see src/lib"), vec![]);
        assert_eq!(found("see src/lib:9"), vec![file("src/lib:9")]);
        assert_eq!(found("app/(shop)/products/[id]/page.tsx"), vec![file("app/(shop)/products/[id]/page.tsx")]);
        assert_eq!(found("(see src/a.rs)"), vec![file("src/a.rs")]);
    }

    #[test]
    fn an_address_and_a_path_side_by_side_are_both_found() {
        assert_eq!(
            found("fetched https://a.io/x into ./out/x.json."),
            vec![web("https://a.io/x"), file("./out/x.json")]
        );
    }

    #[test]
    fn a_file_address_is_read_with_the_machine_it_names() {
        let read = |u: &str| file_url(u).map(|f| (f.host, f.path));
        assert_eq!(read("file:///C:/work/a%20b.txt"), Some((None, "C:/work/a b.txt".into())));
        assert_eq!(read("file://localhost/C:/x"), Some((None, "C:/x".into())));
        assert_eq!(read("file://LOCALHOST/tmp/a"), Some((None, "/tmp/a".into())));
        assert_eq!(read("file:///home/me/%E8%B3%87%E6%96%99"), Some((None, "/home/me/資料".into())));
        assert_eq!(read("file://pc-1/C:/x"), Some((Some("pc-1".into()), "C:/x".into())));
        assert_eq!(read("file://nas/share/a%20b.txt"), Some((Some("nas".into()), "/share/a b.txt".into())));
        assert_eq!(read("https://a.io/"), None);
        assert_eq!(read("file://"), None);
        assert_eq!(read("file://host-only"), None);
    }

    /// A host that is nobody, or the tab's own machine, is the tab's machine;
    /// any other is somewhere else -- a share from this PC, out of reach from
    /// a machine over there
    #[test]
    fn where_a_file_address_is_depends_on_the_machine_it_names() {
        let at = |u: &str, names: &[&str], far: bool| {
            let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
            file_home(&file_url(u).unwrap(), &names, far)
        };
        use FileHome::*;
        // No host, or localhost: the tab's own machine, here or there
        assert_eq!(at("file:///C:/w/a.txt", &["DESK-7"], false), Here("C:/w/a.txt".into()));
        assert_eq!(at("file://localhost/srv/a", &[], true), Here("/srv/a".into()));
        // `ls --hyperlink` names its own machine, in whatever case
        assert_eq!(at("file://desk-7/C:/w/a.txt", &["DESK-7"], false), Here("C:/w/a.txt".into()));
        assert_eq!(at("file://build-01/home/me/a", &["", "build-01"], true), Here("/home/me/a".into()));
        assert_eq!(at("file://build-01.corp.example/home/me/a", &["build-01"], true), Here("/home/me/a".into()));
        // Another computer, from this PC: its share, by the UNC path
        assert_eq!(at("file://nas/share/dir/a%20b.txt", &["DESK-7"], false), Share(r"\\nas\share\dir\a b.txt".into()));
        // ... but not its drive, and not the bare machine
        assert_eq!(at("file://pc-1/C:/x", &["DESK-7"], false), Away("pc-1".into()));
        assert_eq!(at("file://nas/", &["DESK-7"], false), Away("nas".into()));
        // Another computer, from a machine over there: out of reach, whatever
        // its name is -- including when that machine's own name is not known
        assert_eq!(at("file://nas/share/a", &["build-01"], true), Away("nas".into()));
        assert_eq!(at("file://build-01/home/me/a", &["", ""], true), Away("build-01".into()));
        // A name that only starts the same is another machine
        assert_eq!(at("file://desk-70/C:/w", &["DESK-7"], false), Away("desk-70".into()));
    }

    #[test]
    fn a_path_is_found_from_where_the_terminal_is() {
        let base = std::path::PathBuf::from(crate::local_path("D:/work/app"));
        let home = std::path::PathBuf::from(crate::local_path("C:/Users/me"));
        let got = resolve_here("src/../lib/a.rs", Some(&base), Some(&home)).unwrap();
        assert_eq!(got, base.join("lib").join("a.rs"));
        assert_eq!(resolve_here("~/notes.md", Some(&base), Some(&home)).unwrap(), home.join("notes.md"));
        assert_eq!(resolve_here("x.rs", None, Some(&home)), None, "nowhere to put a relative path");
        assert_eq!(resolve_there("../lib/a.rs", "/home/u/app/src"), "/home/u/app/lib/a.rs");
        assert_eq!(resolve_there("/etc/hosts", "/home/u"), "/etc/hosts");
    }

    #[cfg(windows)]
    #[test]
    fn git_bash_drive_paths_are_this_pcs_drives() {
        assert_eq!(resolve_here("/c/work/a.txt", None, None).unwrap(), std::path::PathBuf::from("C:\\work\\a.txt"));
        assert_eq!(resolve_here("/usr/bin/env", None, None), None);
    }

    #[test]
    fn a_script_is_never_opened_by_pressing_its_name() {
        for f in ["build.bat", "x.PS1", "go.cmd", "setup.exe", "a.lnk", "b.reg"] {
            assert!(runs_when_opened(std::path::Path::new(f)), "{f}");
        }
        for f in ["main.rs", "README.md", "photo.png", "Makefile"] {
            assert!(!runs_when_opened(std::path::Path::new(f)), "{f}");
        }
    }

    #[test]
    fn an_endless_address_is_not_scanned_for_ever() {
        let long: String = "https://a.io/".chars().chain(std::iter::repeat_n('a', WEB_LONGEST * 2)).collect();
        assert_eq!(found(&long), vec![]);
    }
}
