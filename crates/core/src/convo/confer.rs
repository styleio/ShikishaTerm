//! AIs conferring: the short lines they say to each other, shown as a chat.
//!
//! What an AI tab is sent and says back through `ask_tab` is long -- which
//! files to look at, what was found, the diff -- and read as it is, a desk of
//! AIs asking each other things is a wall of text. So a line is said beside
//! it: the asker writes one with every ask (`ask_tab ID "line" "text"`), the
//! one asked is asked for one as its turn ends (`asktab::Ask::hear_stop`),
//! and the panel shows the lines as a chat, with the whole text folded under
//! each. A tab says something on its own with `say`.
//!
//! Short is kept by the app, not asked for: a line over the length set in the
//! settings (`confer.line_max`) is refused with the reason, and the AI says it
//! again, shorter. A request that the AI keep it short would be read and
//! forgotten; a refusal is not.
//!
//! An answer with no line of its own -- what the tab said could not be taken,
//! twice, or it runs where nothing can hold its turn's end -- gets its first
//! sentence taken for it ([`first_sentence`]), and the panel shows that.

/// The marks a line may be given. A few, chosen so each says one thing: an
/// open set of emoji would make every mark a guess at what it meant
pub const MARKS: [&str; 6] = ["👍", "❤️", "🎉", "👀", "✅", "❓"];

/// What may be shared as a card
pub const KINDS: [&str; 4] = ["commit", "pr", "file", "url"];

/// How long a line may be when the settings say nothing. Long enough for one
/// full sentence in either English or Japanese (a Japanese sentence of this
/// length says as much as two English ones), short enough that a bubble never
/// needs more than two lines in the column
pub const LINE_MAX: u32 = 80;

/// The line as it is kept, or why it is refused. `what` names it in the
/// refusal the way the caller wrote it ("the line", "say")
pub fn check_line(text: &str, max: u32, what: &str) -> Result<String, String> {
    let line = text.trim();
    if line.is_empty() {
        return Err(format!(
            "{what} is empty: write one short line for the chat, as you would say it to a colleague"
        ));
    }
    if line.contains(['\n', '\r']) {
        return Err(format!("{what} has more than one line: say it in one line, with no line breaks"));
    }
    let n = line.chars().count();
    if max > 0 && n > max as usize {
        return Err(format!(
            "{what} is {n} characters; the most is {max}. Say it shorter -- the whole of it goes in the text"
        ));
    }
    Ok(line.to_string())
}

/// The first sentence of a reply, as a line: what is shown when the one who
/// answered said no line of its own. Markdown is taken off (a heading's `#`,
/// emphasis, code ticks, a list's bullet) so what is left reads as a sentence,
/// and it is cut to `max` characters with an ellipsis
pub fn first_sentence(reply: &str, max: u32) -> String {
    // A heading names what follows; the first thing said is under it. A
    // reply that is nothing but a heading is its heading
    let heading = |l: &&str| l.trim_start().starts_with('#');
    let first = reply
        .lines()
        .filter(|l| !heading(l))
        .map(plain)
        .find(|l| !l.is_empty())
        .or_else(|| reply.lines().map(plain).find(|l| !l.is_empty()))
        .unwrap_or_default();
    // A sentence ends at its stop, whichever language wrote it. A stop inside
    // a word (a version number, a file name) is followed by no space
    let mut end = first.len();
    let chars: Vec<(usize, char)> = first.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        let next = chars.get(i + 1).map(|&(_, n)| n);
        let wide = matches!(c, '。' | '！' | '？');
        let narrow = matches!(c, '.' | '!' | '?') && next.is_none_or(char::is_whitespace);
        if wide || narrow {
            end = at + c.len_utf8();
            break;
        }
    }
    let sentence = first[..end].trim();
    let max = if max == 0 { LINE_MAX } else { max } as usize;
    if sentence.chars().count() <= max {
        return sentence.to_string();
    }
    let cut: String = sentence.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// One line of markdown as plain words
fn plain(line: &str) -> String {
    let mut l = line.trim();
    l = l.trim_start_matches('#').trim_start();
    for bullet in ["- ", "* ", "+ ", "> "] {
        if let Some(rest) = l.strip_prefix(bullet) {
            l = rest.trim_start();
        }
    }
    // A numbered list's "1. "
    if let Some((n, rest)) = l.split_once(". ")
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
    {
        l = rest;
    }
    l.replace("**", "").replace("__", "").replace('`', "").trim().to_string()
}

/// A mark as it is kept, or why it is refused
pub fn check_mark(mark: &str) -> Result<&'static str, String> {
    let m = mark.trim();
    MARKS
        .iter()
        .find(|k| **k == m || k.trim_end_matches('\u{fe0f}') == m.trim_end_matches('\u{fe0f}'))
        .copied()
        .ok_or_else(|| format!("{m:?} is not one of the marks: {}", MARKS.join(" ")))
}

/// The number of a pull request in its address: GitHub's `/pull/12`,
/// GitLab's `/merge_requests/12`, Bitbucket's `/pull-requests/12`
pub fn pr_number(url: &str) -> Option<u64> {
    let path = url.split(['?', '#']).next()?;
    let mut parts = path.trim_end_matches('/').rsplit('/');
    let n = parts.next()?.parse().ok()?;
    matches!(parts.next()?, "pull" | "pulls" | "merge_requests" | "pull-requests").then_some(n)
}

/// An address the card may open: http or https, and nothing else. A card is
/// opened with one press, and `file:` or `javascript:` would be a press that
/// does something other than show a page
pub fn check_url(url: &str) -> Result<String, String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or_else(|| format!("{u:?} is not a web address: it has to start with https:// or http://"))?;
    if rest.is_empty() || rest.starts_with('/') || u.contains(char::is_whitespace) {
        return Err(format!("{u:?} is not a web address"));
    }
    Ok(u.to_string())
}

/// The host of an address, for the card's small print
pub fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or_default().to_string()
}

/// A card, checked: what it opens, what it is called, and what else it shows
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub kind: &'static str,
    pub target: String,
    pub title: String,
    pub detail: serde_json::Value,
}

/// Where the tab that shares is: its folder, and the machine it is on when
/// that is not this PC
#[derive(Debug, Clone)]
pub struct From {
    pub folder: Option<std::path::PathBuf>,
    pub machine: Option<crate::elsewhere::Elsewhere>,
}

/// `share(kind, target, title)`, checked. Run on a thread of its own: a
/// commit on another machine is asked about over the network.
///
/// What is checked is that the card points at something: a commit that is in
/// the tab's folder, a file inside it, an address a browser can open. What a
/// card cannot point at is refused with the reason, so a card never opens
/// onto nothing
pub fn card(kind: &str, target: &str, title: &str, from: &From) -> Result<Card, String> {
    let kind = KINDS
        .iter()
        .find(|k| **k == kind.trim())
        .copied()
        .ok_or_else(|| format!("{kind:?} cannot be shared: share a {}", KINDS.join(", a ")))?;
    let target = target.trim();
    if target.is_empty() {
        return Err(format!("share {kind} needs what to share second"));
    }
    let given = title.trim();
    match kind {
        "commit" => {
            let folder = from.folder.as_deref().ok_or("a commit is shared from a tab that is in a folder")?;
            // A name, never an option: git reads a leading dash as one
            if target.starts_with('-')
                || !target.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '~' | '^'))
            {
                return Err(format!("{target:?} is not a commit"));
            }
            crate::git::there(folder, from.machine.as_ref());
            let said = crate::git::run(folder, &["log", "-1", "--format=%H%x00%h%x00%s", target, "--"])
                .map_err(|e| format!("{target} is not a commit in this tab's folder ({e})"))?;
            let mut parts = said.trim().splitn(3, ' ');
            let (full, short, subject) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            if full.is_empty() {
                return Err(format!("{target} is not a commit in this tab's folder"));
            }
            crate::git::there(folder, from.machine.as_ref());
            let branch = crate::git::run(folder, &["branch", "--show-current"]).map(|b| b.trim().to_string()).unwrap_or_default();
            Ok(Card {
                kind,
                target: full.to_string(),
                title: if given.is_empty() { subject.to_string() } else { given.to_string() },
                detail: serde_json::json!({"short": short, "branch": branch}),
            })
        }
        "file" => {
            let folder = from.folder.as_deref().ok_or("a file is shared from a tab that is in a folder")?;
            if from.machine.is_some() {
                return Err("a file can be shared only from a tab on this PC; share the commit it is in instead".into());
            }
            let root = folder.canonicalize().map_err(|e| format!("this tab's folder cannot be read ({e})"))?;
            let path = std::path::Path::new(target);
            let path = if path.is_absolute() { path.to_path_buf() } else { folder.join(path) };
            let path = path.canonicalize().map_err(|_| format!("there is no file {target:?} in this tab's folder"))?;
            if !path.starts_with(&root) {
                return Err(format!("{target:?} is outside this tab's folder"));
            }
            if !path.is_file() {
                return Err(format!("{target:?} is not a file"));
            }
            let inside = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| inside.clone());
            let dir = inside.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
            Ok(Card {
                kind,
                target: inside,
                title: if given.is_empty() { name } else { given.to_string() },
                detail: serde_json::json!({"folder": dir}),
            })
        }
        _ => {
            let url = check_url(target)?;
            let number = pr_number(&url);
            if kind == "pr" && number.is_none() {
                return Err(format!("{url:?} is not the address of a pull request; share it as a url"));
            }
            let title = if !given.is_empty() {
                given.to_string()
            } else {
                url.clone()
            };
            Ok(Card { kind, detail: serde_json::json!({"host": host_of(&url), "number": number}), target: url, title })
        }
    }
}

/// How much of the conference one page holds: a long evening of AIs asking
/// each other things, and no more than the panel draws at once
pub const PAGE: usize = 80;

/// How many conversations the choice between them offers
pub const THREADS: usize = 30;

/// What the panel asked of the conference on `desk`, answered. Read on a
/// thread, over a connection that only reads. `act`:
///
/// * `confer_threads` -- the conversations, those `args.tab` takes part in
///   when it names one, the one something was said in last first;
/// * `confer` -- a page of the conversation `args.thread`: what was said
///   before `args.before` (the time of the oldest thing the panel has).
///
/// `args.req` is handed back so the panel knows which request this answers
pub fn page(desk: &str, act: &str, args: &serde_json::Value, path: &std::path::Path) -> serde_json::Value {
    let req = args.get("req").cloned().unwrap_or(serde_json::Value::Null);
    let answer = |body: Result<serde_json::Value, anyhow::Error>| match body {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.insert("panel".into(), "confer".into());
                o.insert("act".into(), act.into());
                o.insert("req".into(), req.clone());
                o.insert("ok".into(), true.into());
                o.insert("desk".into(), desk.into());
            }
            v
        }
        Err(e) => serde_json::json!({"panel": "confer", "act": act, "req": req, "ok": false,
            "error": format!("the conference could not be read: {e}")}),
    };
    let store = crate::convo::db::Store::open_read(path);
    if act == "confer_threads" {
        let tab = args.get("tab").and_then(serde_json::Value::as_str).filter(|t| !t.is_empty());
        return answer(store.and_then(|s| s.threads(desk, tab, THREADS)).map(|t| serde_json::json!({"tab": tab, "threads": t})));
    }
    let Some(thread) = args.get("thread").and_then(serde_json::Value::as_i64) else {
        return answer(Err(anyhow::anyhow!("no conversation was named")));
    };
    let before = args.get("before").and_then(serde_json::Value::as_i64).unwrap_or(i64::MAX);
    answer(store.and_then(|s| s.conference(desk, thread, before, PAGE + 1)).map(|mut said| {
        // One more than a page was read: whether there is anything before
        let more = said.len() > PAGE;
        if more {
            said.remove(0);
        }
        serde_json::json!({"thread": thread, "said": said, "more": more, "before": before != i64::MAX})
    }))
}

/// How many of a desk's recent conversations a new one is put beside, for the
/// deciding AI to say whether it belongs to one of them
pub const MERGE_CANDIDATES: usize = 5;

/// Whether the conversation just begun on `desk` with `line` / `text`
/// belongs to one of the desk's recent ones, asked of the deciding AI.
/// `Some(id)`: the one it belongs to. Run on a thread of its own: a model is
/// a round trip away, and the conversation is shown meanwhile as it was begun
pub fn belongs_to(
    who: &crate::bridge::Answerer,
    path: &std::path::Path,
    desk: &str,
    begun: i64,
    line: &str,
    text: &str,
) -> anyhow::Result<Option<i64>> {
    let store = crate::convo::db::Store::open_read(path)?;
    let mut criteria = serde_json::Map::new();
    let mut state = format!(
        "AI tabs in a terminal app ask each other for help, and each exchange belongs to a conversation. \
         A new conversation has just begun with this request.\nIts line: {line}\nThe request: {}\n\nRecent conversations on the same desk:",
        text.chars().take(1200).collect::<String>()
    );
    for t in store.threads(desk, None, MERGE_CANDIDATES + 1)?.into_iter().filter(|t| t.id != begun).take(MERGE_CANDIDATES) {
        let said: Vec<String> = store
            .conference(desk, t.id, i64::MAX, 4)?
            .into_iter()
            .filter_map(|s| match s {
                crate::convo::db::Said::Line { tab, text, .. } => Some(format!("  {}: {text}", tab.unwrap_or_else(|| "person".into()))),
                crate::convo::db::Said::Share { .. } => None,
            })
            .collect();
        let key = format!("t{}", t.id);
        state.push_str(&format!("\n\n[{key}] with {}; began: {}\n{}", t.tabs.join(", "), t.first, said.join("\n")));
        criteria.insert(key.clone(), format!("the new request continues conversation {key}: the same task or topic").into());
    }
    if criteria.is_empty() {
        return Ok(None);
    }
    criteria.insert("new".into(), "the new request is a separate matter: none of these conversations".into());
    let ask = serde_json::json!({
        "state": state,
        "questions": {"conversation": {
            "type": "choice",
            "criteria": criteria,
            "instructions": "Choose the conversation the new request continues. Choose new unless it is clearly the same task or topic.",
        }},
    });
    let answer = crate::bridge::choose(who, &ask)?;
    let choice = answer["conversation"]["choice"].as_str().unwrap_or("new");
    Ok(choice.strip_prefix('t').and_then(|n| n.parse().ok()))
}

/// What the stop hook tells an AI that has just answered another tab: in
/// English, as every instruction to an AI is, with the limit it has to keep.
/// The reply that follows is the line, whole: nothing is run to say it
pub fn stop_reason(max: u32) -> String {
    let max = if max == 0 { String::new() } else { format!("at most {max} characters, ") };
    format!(
        "Your answer has been passed on in full to the tab that asked you. The person follows the tabs as a chat: \
         reply now with only one short line about your answer, the way you would say it to a colleague -- \
         {max}one line, in the language the person uses. Nothing else: no quotes, no heading, no repeat of the answer."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_one_short_line_and_nothing_else() {
        assert_eq!(check_line("  Looks good, merging.  ", 80, "the line").unwrap(), "Looks good, merging.");
        assert!(check_line("   ", 80, "the line").unwrap_err().contains("empty"));
        assert!(check_line("one\ntwo", 80, "the line").unwrap_err().contains("one line"));
        let long = "あ".repeat(81);
        let e = check_line(&long, 80, "say").unwrap_err();
        assert!(e.starts_with("say is 81 characters; the most is 80"), "{e}");
        assert!(check_line(&"あ".repeat(80), 80, "say").is_ok(), "characters are counted, not bytes");
        assert!(check_line(&"a".repeat(500), 0, "say").is_ok(), "0 is no limit");
    }

    #[test]
    fn the_first_sentence_is_taken_as_plain_words() {
        assert_eq!(first_sentence("## Review\n\n**Two findings.** The parser drops the tail.", 80), "Two findings.");
        assert_eq!(first_sentence("- Fixed in v1.2.3 and tested! More later.", 80), "Fixed in v1.2.3 and tested!");
        assert_eq!(first_sentence("レビューしました。2点あります。", 80), "レビューしました。");
        assert_eq!(first_sentence("1. `cargo test` passes", 80), "cargo test passes");
        let cut = first_sentence(&"word ".repeat(40), 20);
        assert!(cut.ends_with('…') && cut.chars().count() <= 20, "{cut}");
        assert_eq!(first_sentence("", 80), "");
    }

    #[test]
    fn only_the_offered_marks_are_taken() {
        assert_eq!(check_mark("👍").unwrap(), "👍");
        assert_eq!(check_mark("❤").unwrap(), "❤️", "with or without the emoji selector");
        assert!(check_mark("💩").is_err());
    }

    #[test]
    fn a_card_points_at_something_or_is_refused() {
        let dir = std::env::temp_dir().join(format!("confer-card-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/p.rs"), "fn main() {}").unwrap();
        let here = From { folder: Some(dir.clone()), machine: None };
        let c = card("file", "src/p.rs", "", &here).unwrap();
        assert_eq!((c.target.as_str(), c.title.as_str()), ("src/p.rs", "p.rs"));
        assert_eq!(c.detail["folder"], "src");
        assert!(card("file", "../../etc/hosts", "", &here).is_err(), "outside the folder");
        assert!(card("file", "src/missing.rs", "", &here).unwrap_err().contains("no file"));
        assert!(card("file", "src", "", &here).unwrap_err().contains("not a file"));
        assert!(card("image", "x.png", "", &here).unwrap_err().contains("cannot be shared"));
        let pr = card("pr", "https://github.com/o/r/pull/7", "", &here).unwrap();
        assert_eq!((pr.detail["number"].as_u64(), pr.detail["host"].as_str()), (Some(7), Some("github.com")));
        assert!(card("pr", "https://github.com/o/r", "", &here).is_err(), "not a pull request");
        assert_eq!(card("url", "https://example.com/a", "Example", &here).unwrap().title, "Example");
        assert!(card("commit", "HEAD; rm -rf /", "", &here).unwrap_err().contains("not a commit"));
        assert!(card("commit", "--output=x", "", &here).unwrap_err().contains("not a commit"), "never an option");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_card_opens_only_a_web_address() {
        assert!(check_url("https://github.com/o/r/pull/12").is_ok());
        assert!(check_url("javascript:alert(1)").is_err());
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("https://").is_err());
        assert_eq!(pr_number("https://github.com/o/r/pull/12/files"), None, "the files page of a pull request is a page");
        assert_eq!(pr_number("https://github.com/o/r/pull/12"), Some(12));
        assert_eq!(pr_number("https://gitlab.com/g/p/-/merge_requests/7?x=1"), Some(7));
        assert_eq!(host_of("https://github.com/o/r"), "github.com");
    }
}
