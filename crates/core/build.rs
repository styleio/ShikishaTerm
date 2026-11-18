//! What the runtime says about itself: when it was built, and from which commit.
//!
//! The same two values the app has always shown. They are stamped here because
//! this is the crate that reports them, and a build of the runtime alone -- no
//! window anywhere -- still has to be able to say what it is.

fn main() {
    // Every system can say what time it is; they are asked differently. Asked
    // the Windows way alone, every build made anywhere else stamped an empty
    // string, and the board's footer said "build ()" with nothing in it.
    let mut when = match cfg!(windows) {
        true => std::process::Command::new("cmd"),
        false => std::process::Command::new("date"),
    };
    match cfg!(windows) {
        true => when.args(["/C", "echo %DATE% %TIME%"]),
        false => when.arg("+%Y/%m/%d %H:%M:%S"),
    };
    let built = when
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=BUILD_TIME={built}");
    let rev = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    println!("cargo:rustc-env=BUILD_REV={rev}{}", if dirty { "+" } else { "" });
    // The whole commit, for fetching this very version of the source from the
    // public repository (the ? searching the code). A short one cannot be
    // fetched by
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=BUILD_SHA={sha}");
    println!("cargo:rerun-if-changed=src");
    // And again after every commit. Without it the stamp kept the commit the
    // sources were last edited at: fix, build, then commit, and the app went on
    // calling itself the commit before -- which the deploy rightly refused
    watch_git_head();
    println!("cargo:rerun-if-changed=../../tools/build_git_head.rs");
    // The words the app is written in, for what a saved report keeps of a log
    vocabulary();

    // The library that compresses video where the operating system has no
    // encoder of its own. Said here rather than left to whoever runs the
    // build: a link line that lives in somebody's environment is a link line
    // that works on one machine.
    //
    // Only when asked for. Off, nothing is linked and nothing is needed;
    // Windows never asks, because it has its own (src/vencode.rs).
    if std::env::var_os("CARGO_FEATURE_VP8").is_some() {
        println!("cargo:rustc-link-lib=vpx");
    }
}

/// The words this program is written in, put beside the build for the
/// program to read (`src/diagnose.rs`).
///
/// A report somebody saves carries the app's logs, and a log line is the
/// app's words with somebody's own among them: the name of a desk, a
/// branch, a machine, a file. Which words are the app's cannot be told from
/// the line. It can be told from the source: every word the app itself can
/// write is in it. So the words of the source are listed here -- of what is
/// built, not of its tests, which are full of made-up names, nor of what is
/// written about the code -- with the words of the two languages and of the
/// AI profiles, and a report keeps those and leaves out the rest.
///
/// Read again when this crate's sources change and after every commit. A
/// word added to another crate is known from the next commit on; until then
/// a report leaves it out, which is the safe way to be wrong
fn vocabulary() {
    use std::path::{Path, PathBuf};
    fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let at = entry.path();
            match at.is_dir() {
                true => walk(&at, into),
                false => into.push(at),
            }
        }
    }
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let top = here.join("..").join("..");
    let mut files = Vec::new();
    for dir in ["src", "lang", "profiles"] {
        walk(&top.join(dir), &mut files);
    }
    if let Ok(crates) = std::fs::read_dir(top.join("crates")) {
        for one in crates.flatten() {
            walk(&one.path().join("src"), &mut files);
        }
    }
    // A file of tests is one a parent names under a mark for tests, or one
    // called so: none of what it says is what the app says
    let texts: Vec<(PathBuf, String)> = files.into_iter().filter_map(|f| Some((f.clone(), std::fs::read_to_string(&f).ok()?))).collect();
    let mut tests: Vec<PathBuf> = Vec::new();
    for (file, text) in texts.iter().filter(|(f, _)| f.extension().is_some_and(|e| e == "rs")) {
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        for (i, line) in lines.iter().enumerate() {
            if !for_tests(line) {
                continue;
            }
            // The module it marks, past any other marks on the way
            let Some(named) = lines[i + 1..].iter().find(|l| !l.starts_with("#[")) else { continue };
            let named = named.trim_start_matches("pub(crate) ").trim_start_matches("pub ");
            if let Some(name) = named.strip_prefix("mod ").and_then(|m| m.strip_suffix(';')) {
                let beside = file.parent().unwrap_or(Path::new("."));
                tests.push(beside.join(format!("{name}.rs")));
                tests.push(beside.join(name));
            }
        }
    }
    let of_tests = |file: &Path| {
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        stem == "tests" || stem.ends_with("_tests") || stem.ends_with("_test") || tests.iter().any(|t| file.starts_with(t))
    };
    let mut words = std::collections::BTreeSet::new();
    for (file, text) in texts {
        let source = file.extension().is_some_and(|e| e == "rs");
        if (!source && file.extension().is_none_or(|e| e != "json")) || (source && of_tests(&file)) {
            continue;
        }
        let text = if source { built(&text) } else { text };
        let mut run = String::new();
        for c in text.chars().chain(std::iter::once(' ')) {
            if c.is_alphabetic() {
                run.extend(c.to_lowercase());
            } else if !run.is_empty() {
                // A word of one or two plain letters says nothing of anybody, and is kept without being listed
                if !run.is_ascii() || run.len() > 2 {
                    words.insert(std::mem::take(&mut run));
                }
                run.clear();
            }
        }
    }
    // What fails underneath the app says why in words of its own: a
    // certificate nobody vouches for, a name that does not resolve, a
    // connection refused. They are no one's name, and without them a report
    // says only that something failed
    for word in UNDERNEATH.split_whitespace() {
        words.insert(word.to_string());
    }
    // ...and a name the app's own texts use as an example stands for a
    // person wherever else it is said
    for name in ["alice", "bob", "carol", "dave", "ada", "acme"] {
        words.remove(name);
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("vocabulary.txt");
    let _ = std::fs::write(out, words.into_iter().collect::<Vec<_>>().join("\n"));
}

/// The words of the systems and the libraries under the app, for when they fail
const UNDERNEATH: &str = "\
    access accepted address aborted alert algorithm already authentication authority bad broken buffer busy cancelled canceled certificate chain cipher closed \
    connect connection corrupt corrupted crl decode decrypt denied digest dns eof encode encrypt encrypted error exhausted expired failed failure fatal forbidden gateway \
    handshake host hostname illegal incomplete incorrect insufficient interrupted invalid issuer kex key length limit locked mismatch missing name network \
    nonce not ocsp operation overflow peer permission pipe port protocol proxy quota read refused rejected remote renegotiation reset resolve resolved revoked \
    route schannel secure server signature socket ssl status stream temporarily terminated timed timeout tls token trust trusted truncated unauthorized \
    unavailable unexpected unknown unreachable unsupported untrusted valid verify verification version would write yet";

/// Whether a mark says what follows is only there when the tests are built:
/// `#[cfg(test)]`, or one that asks for tests among other things
fn for_tests(mark: &str) -> bool {
    mark.strip_prefix("#[cfg(").is_some_and(|rest| {
        let asked = rest.split(")]").next().unwrap_or(rest);
        asked.split(|c: char| !c.is_alphanumeric() && c != '_').any(|w| w == "test") && !asked.contains("not(test")
    })
}

/// A source file as what is built of it: its code and what its code says,
/// without what is written about the code and without whatever is marked
/// `#[cfg(test)]` -- a module of tests, wherever in the file it stands, or
/// one item. Read the way the language is: a brace inside a string or a
/// comment opens nothing
fn built(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut depth) = (0usize, 0i32);
    // While leaving something out: the depth it began at, and whether its body has opened
    let mut leaving: Option<(i32, bool)> = None;
    let at = |i: usize, s: &str| s.chars().enumerate().all(|(k, ch)| c.get(i + k) == Some(&ch));
    while i < c.len() {
        // What is written about the code
        if at(i, "//") {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if at(i, "/*") {
            let mut open = 1;
            i += 2;
            while i < c.len() && open > 0 {
                if at(i, "/*") {
                    open += 1;
                    i += 2;
                } else if at(i, "*/") {
                    open -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // What the code says: a string, written plainly or raw
        let word_before = i > 0 && (c[i - 1].is_alphanumeric() || c[i - 1] == '_');
        let raw = (!word_before && (at(i, "r\"") || at(i, "r#"))).then_some(i + 1).or((!word_before && (at(i, "br\"") || at(i, "br#"))).then_some(i + 2));
        if let Some(mut j) = raw {
            let mut hashes = 0;
            while c.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if c.get(j) == Some(&'"') {
                j += 1;
                let from = j;
                let closes = |j: usize| c.get(j) == Some(&'"') && (1..=hashes).all(|k| c.get(j + k) == Some(&'#'));
                while j < c.len() && !closes(j) {
                    j += 1;
                }
                if leaving.is_none() {
                    out.push(' ');
                    out.extend(&c[from..j.min(c.len())]);
                    out.push(' ');
                }
                i = j + 1 + hashes;
                continue;
            }
        }
        if c[i] == '"' {
            let mut j = i + 1;
            let mut said = String::new();
            while j < c.len() && c[j] != '"' {
                if c[j] != '\\' {
                    said.push(c[j]);
                    j += 1;
                    continue;
                }
                // What stands for a mark or a letter is read as what it stands for:
                // "one\ntwo" says one and two, not one and ntwo
                match c.get(j + 1) {
                    Some('u') if c.get(j + 2) == Some(&'{') => {
                        let end = (j + 3..c.len()).find(|k| c[*k] == '}').unwrap_or(c.len());
                        let number: String = c[j + 3..end].iter().collect();
                        said.push(u32::from_str_radix(&number, 16).ok().and_then(char::from_u32).unwrap_or(' '));
                        j = end + 1;
                    }
                    Some('"') => {
                        said.push('"');
                        j += 2;
                    }
                    _ => {
                        said.push(' ');
                        j += 2;
                    }
                }
            }
            if leaving.is_none() {
                out.push(' ');
                out.push_str(&said);
                out.push(' ');
            }
            i = j + 1;
            continue;
        }
        // One letter between marks -- and not a name for how long something lives, which begins the same way
        if c[i] == '\'' {
            if c.get(i + 1) == Some(&'\\') {
                let mut j = i + 2;
                while j < c.len() && c[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            if c.get(i + 2) == Some(&'\'') {
                i += 3;
                continue;
            }
        }
        // What is only there when the tests are built
        if leaving.is_none() && at(i, "#[cfg(") {
            let mark: String = c[i..].iter().take(200).take_while(|ch| **ch != '\n').collect();
            if for_tests(&mark) {
                leaving = Some((depth, false));
                i += mark.find(")]").map_or(mark.chars().count(), |end| mark[..end].chars().count() + 2);
                continue;
            }
        }
        match c[i] {
            '{' => {
                depth += 1;
                if let Some((_, opened)) = &mut leaving {
                    *opened = true;
                }
            }
            '}' => {
                depth -= 1;
                match leaving {
                    // Its body has closed, or what it stood in has
                    Some((began, opened)) if (opened && depth == began) || depth < began => leaving = None,
                    _ => {}
                }
                i += 1;
                continue;
            }
            // An item with no body ends where it says so
            ';' if leaving.is_some_and(|(began, opened)| !opened && depth == began) => {
                leaving = None;
                i += 1;
                continue;
            }
            _ => {}
        }
        if leaving.is_none() {
            out.push(c[i]);
        }
        i += 1;
    }
    out
}

include!("../../tools/build_git_head.rs");
