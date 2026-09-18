//! A working folder's name and summary, written from what its AIs are asked.
//!
//! Several folders open side by side, each with an AI working on something,
//! and a week later nobody remembers which branch was which. A folder that
//! asks for it (`auto_label`) is named and described from the requests typed
//! into its AI tabs -- the requests only, never what the AI answered: what
//! somebody asked for is what the folder is *for*, and it is a few lines where
//! the answers are pages.
//!
//! Two halves live here, both without a clock or a disk of their own so they
//! can be tested: what a request is cut down to before an AI reads it
//! ([`trim_ask`]), and when a folder is due to be described again ([`Board`]).
//! Asking the AI, and writing the answer into the settings, is the loop's.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A request lighter than this says nothing about the work: "yes", "go on",
/// "お願いします". Counted in columns, so a Japanese character weighs what it
/// takes on screen -- two -- and a short Japanese request is not taken for a
/// shorter English one
pub const SHORT: usize = 16;

/// The least time between two descriptions of one folder, after its first.
/// The first is written at once: a folder with no summary is the one that most
/// needs one
pub const EVERY: Duration = Duration::from_secs(10 * 60);

/// The requests kept per folder until they are read. The newest ones: what a
/// folder is for now is in the latest things asked of it
const KEEP: usize = 6;

/// How long one request may be once cut down, in characters. The opening says
/// what is wanted, and the end is usually the question; the middle of a long
/// request is the material it came with
const ONE_ROOM: usize = 1500;

/// A run of this many lines that read as code is taken out. One or two such
/// lines in a request are usually a name or a command being talked about, and
/// the request reads wrong without them
const CODE_RUN: usize = 3;

/// How heavy a request is (see [`SHORT`]).
pub fn weight(text: &str) -> usize {
    use unicode_width::UnicodeWidthChar as _;
    text.trim().chars().map(|c| c.width().unwrap_or(0).max(1)).sum()
}

/// A request as the AI writing the summary reads it.
///
/// Code is taken out and a mark left where it was, so "fix this: [code, 40
/// lines]" still says that something was handed over. Fenced blocks are code
/// by definition; a pasted log or stack trace without a fence is found by how
/// its lines look, and only when several of them stand together. What is left,
/// if still long, keeps its opening and its end
pub fn trim_ask(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // A fence: everything to the closing one, or to the end when it is
        // never closed
        if let Some(fence) = fence_of(line) {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim_start().starts_with(fence) {
                j += 1;
            }
            kept.push(code_mark(j.saturating_sub(i + 1)));
            i = (j + 1).min(lines.len());
            continue;
        }
        // A run of code-looking lines. A blank line inside the run belongs to
        // it when code carries on after it
        if looks_like_code(line) {
            let mut j = i;
            let mut code = 0;
            while j < lines.len() {
                if looks_like_code(lines[j]) {
                    code += 1;
                    j += 1;
                } else if lines[j].trim().is_empty() && lines.get(j + 1).is_some_and(|n| looks_like_code(n)) {
                    j += 1;
                } else {
                    break;
                }
            }
            if code >= CODE_RUN {
                kept.push(code_mark(j - i));
            } else {
                kept.extend(lines[i..j].iter().map(|l| l.to_string()));
            }
            i = j;
            continue;
        }
        kept.push(line.trim_end().to_string());
        i += 1;
    }
    // Blank lines left where things were taken out come to one at a time
    let mut out = String::new();
    let mut blank = false;
    for line in kept {
        if line.trim().is_empty() {
            blank = true;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        blank = false;
        out.push_str(&line);
    }
    squeeze(&out, ONE_ROOM)
}

/// The fence a line opens, when it opens one
fn fence_of(line: &str) -> Option<&'static str> {
    let t = line.trim_start();
    ["```", "~~~"].into_iter().find(|f| t.starts_with(f))
}

/// What stands where code was taken out. In English whatever the screen's
/// language: it is read by an AI, never by a person
fn code_mark(lines: usize) -> String {
    format!("[code, {lines} lines]")
}

/// Whether a line reads as code, a log or a stack trace rather than as words.
///
/// No single test is enough and none has to be: a line is only taken out in
/// the company of others like it ([`CODE_RUN`])
fn looks_like_code(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    const STARTS: &[&str] = &[
        "at ", "File \"", "Traceback", "@@", "diff --git", "+++ ", "--- ", "error[", "warning[",
        "#include", "import ", "from ", "use ", "fn ", "pub ", "def ", "class ", "function ", "const ",
        "let ", "var ", "return ", "if (", "} ", "</", "//", "/*", "* ", "$ ", "PS ", ">>> ", "at\t",
    ];
    if STARTS.iter().any(|s| t.starts_with(s)) {
        return true;
    }
    if [';', '{', '}', '(', ')', ',', '[', ']'].iter().any(|c| t.ends_with(*c)) && t.is_ascii() {
        return true;
    }
    // A place in a file, the way compilers, logs and stack traces point:
    // `config.rs:120`, `app.py:31`
    if t.is_ascii() && names_a_line(t) {
        return true;
    }
    let symbols = t.chars().filter(|c| "{}[]()<>=;:|&$\\/*#\"'`+-_.@%".contains(*c)).count();
    let letters = t.chars().filter(|c| !c.is_whitespace()).count();
    let indented = line.starts_with("    ") || line.starts_with('\t');
    // Mostly punctuation: a path, a JSON line, a line of a log
    symbols * 10 >= letters * 3 || (indented && t.is_ascii() && symbols > 0)
}

/// Whether a line holds `name.ext:123`
fn names_a_line(t: &str) -> bool {
    let b = t.as_bytes();
    (0..b.len()).any(|i| {
        if b[i] != b'.' {
            return false;
        }
        let ext = b[i + 1..].iter().take_while(|c| c.is_ascii_alphanumeric()).count();
        let colon = i + 1 + ext;
        (1..=5).contains(&ext) && b.get(colon) == Some(&b':') && b.get(colon + 1).is_some_and(|c| c.is_ascii_digit())
    })
}

/// `text` no longer than `room` characters: its opening and its end, with a
/// mark between them
fn squeeze(text: &str, room: usize) -> String {
    let count = text.chars().count();
    if count <= room {
        return text.to_string();
    }
    let head: String = text.chars().take(room * 2 / 3).collect();
    let tail: String = text.chars().skip(count - room / 3).collect();
    format!("{}\n[...]\n{}", head.trim_end(), tail.trim_start())
}

/// One folder's requests since it was last described.
#[derive(Debug, Default)]
struct Heard {
    at: PathBuf,
    /// Cut down already, the newest last
    asks: Vec<String>,
    /// The last request heard, as it arrived. The same request can arrive
    /// twice -- from the input bar that sent it, and from the AI's own report
    /// of having been asked -- and is one request
    last: String,
    /// An AI in the folder finished a turn since the requests arrived
    ended: bool,
    /// Being described right now
    running: bool,
    /// When it was last described, or last tried
    tried: Option<Instant>,
}

/// Every folder's requests, and whether it is time to describe it again.
#[derive(Debug, Default)]
pub struct Board {
    heard: Vec<Heard>,
}

impl Board {
    fn find(&mut self, at: &Path) -> &mut Heard {
        let i = match self.heard.iter().position(|h| crate::uistate::same_folder(&h.at, at)) {
            Some(i) => i,
            None => {
                self.heard.push(Heard { at: at.to_path_buf(), ..Default::default() });
                self.heard.len() - 1
            }
        };
        &mut self.heard[i]
    }

    /// Somebody asked an AI in the folder `at` something. Answers whether it
    /// was kept: a request too short to say anything, or one already heard,
    /// is not
    pub fn hear(&mut self, at: &Path, text: &str) -> bool {
        let text = text.trim();
        let h = self.find(at);
        if text.is_empty() || text == h.last {
            return false;
        }
        h.last = text.to_string();
        if weight(text) < SHORT {
            return false;
        }
        // The same request can arrive by more than one road -- the input bar
        // knows what it handed over, the CLI's own record has it written down,
        // and a hook can report it -- and one asked for twice is not two
        // things the folder is for
        let cut = trim_ask(text);
        if h.asks.iter().any(|a| *a == cut) {
            return false;
        }
        h.asks.push(cut);
        if h.asks.len() > KEEP {
            let over = h.asks.len() - KEEP;
            h.asks.drain(..over);
        }
        // What an AI finished before this was asked was not this
        h.ended = false;
        true
    }

    /// An AI working in the folder `at` finished its turn
    pub fn turn_ended(&mut self, at: &Path) {
        if let Some(h) = self.heard.iter_mut().find(|h| crate::uistate::same_folder(&h.at, at))
            && !h.asks.is_empty()
        {
            h.ended = true;
        }
    }

    /// The folders it is time to describe, of `wanted`: each folder that asks
    /// for it, with whether it has a summary already.
    ///
    /// The first summary is written as soon as there is something to write it
    /// from. After that a folder waits for an AI in it to finish a turn, and
    /// for [`EVERY`] since the last one -- asking again while the work is still
    /// being asked for would describe half a request
    pub fn due(&self, now: Instant, wanted: &[(PathBuf, bool)]) -> Vec<PathBuf> {
        self.heard
            .iter()
            .filter(|h| !h.asks.is_empty() && !h.running)
            .filter_map(|h| {
                let described = wanted.iter().find(|(at, _)| crate::uistate::same_folder(at, &h.at))?.1;
                let first = !described && h.tried.is_none();
                let rested = h.tried.is_none_or(|t| now.duration_since(t) >= EVERY);
                (first || (h.ended && rested)).then(|| h.at.clone())
            })
            .collect()
    }

    /// The requests to describe the folder `at` from, taken: they are the
    /// AI's now, and whatever is asked from here on waits for the next time
    pub fn take(&mut self, at: &Path) -> Vec<String> {
        let h = self.find(at);
        h.running = true;
        h.ended = false;
        std::mem::take(&mut h.asks)
    }

    /// The description of `at` is written, or failed. A failure hands its
    /// requests back, ahead of anything asked since, so the next try reads
    /// them all. Either way the next one waits its turn: a failing AI is not
    /// asked again on every frame
    pub fn finished(&mut self, at: &Path, now: Instant, failed: Option<Vec<String>>) {
        let h = self.find(at);
        h.running = false;
        h.tried = Some(now);
        if let Some(mut back) = failed {
            back.append(&mut h.asks);
            let over = back.len().saturating_sub(KEEP);
            back.drain(..over);
            h.asks = back;
            h.ended = true;
        }
    }

    /// The folder stopped asking to be described: what it heard goes
    pub fn forget(&mut self, at: &Path) {
        self.heard.retain(|h| h.running || !crate::uistate::same_folder(&h.at, at));
    }
}

/// Where the last failure to describe each folder is kept, beside the app.
/// Read by the settings page, which runs apart from the loop that failed
const OUTCOMES: &str = "folder-labels.json";

/// Says how describing the folder `at` went: `failed` with the reason, or
/// `None` once it worked. Only failures are kept -- a folder that is fine has
/// nothing to say about it
pub fn note_outcome(at: &Path, failed: Option<&str>) {
    let path = crate::config::state_path(OUTCOMES);
    let mut all = read_outcomes(&path);
    let key = at.display().to_string();
    let changed = match failed {
        Some(why) => all.insert(key, serde_json::json!(why)).as_ref() != Some(&serde_json::json!(why)),
        None => all.remove(&key).is_some(),
    };
    if changed && let Ok(text) = serde_json::to_string_pretty(&all) {
        let _ = crate::crypto::write_atomic(&path, &text);
    }
}

/// Why each folder could not be described, the last time it was tried: the
/// folder's path, and the reason
pub fn outcomes() -> serde_json::Map<String, serde_json::Value> {
    read_outcomes(&crate::config::state_path(OUTCOMES))
}

fn read_outcomes(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The longest a written name may be, in columns: what fits on a card
pub const NAME_ROOM: usize = 40;

/// The longest a written summary may be, in columns: three lines of a tooltip
pub const SUMMARY_ROOM: usize = 240;

/// `text` on one line and no wider than `room` columns
pub fn fit(text: &str, room: usize) -> String {
    use unicode_width::UnicodeWidthChar as _;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    let mut used = 0;
    for c in flat.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > room {
            out.push('…');
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_name_fits_a_card() {
        assert_eq!(fit("  Fix the\nlogin  page ", NAME_ROOM), "Fix the login page");
        let long = fit(&"あ".repeat(30), NAME_ROOM);
        assert_eq!(long, format!("{}…", "あ".repeat(20)));
    }

    #[test]
    fn a_short_request_is_not_heard() {
        let mut b = Board::default();
        let at = Path::new(r"D:\work\a");
        assert!(!b.hear(at, "yes"));
        assert!(!b.hear(at, "お願いします"));
        assert!(!b.hear(at, "continue please"));
        assert!(b.hear(at, "ログイン画面を直して"), "eight Japanese characters are a request");
        assert!(b.hear(at, "add a dark mode toggle"));
    }

    #[test]
    fn the_same_request_twice_is_one() {
        let mut b = Board::default();
        let at = Path::new(r"D:\work\a");
        assert!(b.hear(at, "fix the failing login test"));
        assert!(!b.hear(at, "fix the failing login test\n"), "the AI's own report of it was counted again");
        assert_eq!(b.take(at).len(), 1);
    }

    #[test]
    fn the_first_summary_is_written_at_once_and_the_next_waits() {
        let mut b = Board::default();
        let at = PathBuf::from(r"D:\work\a");
        let t0 = Instant::now();
        b.hear(&at, "make the settings page remember the last tab");
        assert_eq!(b.due(t0, &[(at.clone(), false)]), vec![at.clone()], "a folder with no summary waited");
        // One the settings do not ask about is never due
        assert!(b.due(t0, &[]).is_empty());
        b.take(&at);
        assert!(b.due(t0, &[(at.clone(), false)]).is_empty(), "nothing is left to read");
        b.finished(&at, t0, None);
        b.hear(&at, "and put the tab in the page address as well");
        assert!(b.due(t0, &[(at.clone(), true)]).is_empty(), "described again before any AI finished");
        b.turn_ended(&at);
        assert!(b.due(t0 + Duration::from_secs(60), &[(at.clone(), true)]).is_empty(), "described again within ten minutes");
        assert_eq!(b.due(t0 + EVERY, &[(at.clone(), true)]), vec![at.clone()]);
    }

    #[test]
    fn a_summary_kept_from_before_waits_for_a_turn_to_end() {
        let mut b = Board::default();
        let at = PathBuf::from(r"D:\work\a");
        let t0 = Instant::now();
        b.hear(&at, "make the settings page remember the last tab");
        assert!(b.due(t0, &[(at.clone(), true)]).is_empty());
        b.turn_ended(&at);
        assert_eq!(b.due(t0, &[(at.clone(), true)]), vec![at]);
    }

    #[test]
    fn a_failure_keeps_its_requests_and_does_not_ask_again_at_once() {
        let mut b = Board::default();
        let at = PathBuf::from(r"D:\work\a");
        let t0 = Instant::now();
        b.hear(&at, "make the settings page remember the last tab");
        let asks = b.take(&at);
        b.hear(&at, "and put the tab in the page address as well");
        b.finished(&at, t0, Some(asks));
        assert!(b.due(t0, &[(at.clone(), false)]).is_empty(), "a failing AI would be asked on every frame");
        assert_eq!(b.due(t0 + EVERY, &[(at.clone(), false)]), vec![at.clone()]);
        let again = b.take(&at);
        assert_eq!(again.len(), 2);
        assert!(again[0].starts_with("make"), "the older request lost its place");
    }

    #[test]
    fn a_fenced_block_leaves_a_mark() {
        let asked = "Why does this panic?\n```rust\nfn main() {\n    let v: Vec<u8> = vec![];\n    v[0];\n}\n```\nIt happens on start.";
        assert_eq!(trim_ask(asked), "Why does this panic?\n[code, 4 lines]\nIt happens on start.");
    }

    #[test]
    fn a_pasted_stack_trace_is_taken_out_but_a_word_of_code_stays() {
        let asked = "テストが落ちます。\nthread 'main' panicked at src/config.rs:120:5:\n    at shikisha::config::load (src/config.rs:120)\n    at shikisha::main (src/main.rs:40)\n    at std::rt::lang_start (library/std/src/rt.rs:1)\n`cargo test` を通してください";
        let out = trim_ask(asked);
        assert!(out.contains("[code, 4 lines]"), "{out}");
        assert!(out.contains("`cargo test` を通してください"), "a sentence mentioning a command was cut: {out}");
        assert!(out.starts_with("テストが落ちます。"));
    }

    #[test]
    fn plain_words_are_left_alone() {
        let asked = "ワークツリーの一覧で、ホバーしたときに名前と概要を出したい。\nパスは出さなくていい。\nスマホでも見られるようにしてほしい。";
        assert_eq!(trim_ask(asked), asked);
        let english = "Make the sidebar remember which projects were folded.\nIt should survive a restart.\nThe phone should do the same.";
        assert_eq!(trim_ask(english), english);
    }

    #[test]
    fn a_long_request_keeps_its_opening_and_its_end() {
        let asked = format!("最初に言いたいこと。{}最後の質問はこれ？", "あ".repeat(5000));
        let out = trim_ask(&asked);
        assert!(out.chars().count() <= ONE_ROOM + 10);
        assert!(out.starts_with("最初に言いたいこと。"));
        assert!(out.ends_with("最後の質問はこれ？"));
        assert!(out.contains("[...]"));
    }
}
