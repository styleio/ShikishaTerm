//! The pull request a branch is on, when there is one.
//!
//! The third thing you would otherwise go and look up, after the branch and
//! the ports. With several agents each on their own branch, "has that one been
//! merged yet" is a question about every row at once, and answering it means
//! leaving the terminal entirely.
//!
//! **Nothing is set up for this.** The token is the one the person already
//! has: what `gh` stored when they logged in, or `GITHUB_TOKEN` if they keep
//! one in their environment. Asking someone to paste a token into a second
//! place, so a terminal can show them a number they can already see on a
//! website, is not a trade worth offering. Where there is no token there is no
//! PR line, and the settings say so rather than leaving it a mystery.
//!
//! The asking happens on a thread of its own and the answers are left where
//! the window can pick them up. A window that stops drawing because GitHub is
//! slow would be a poor way to learn that a branch has been merged.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long an answer is good for. A pull request opens, gets reviewed and
/// merges over hours; asking every minute is already generous
const FRESH: Duration = Duration::from_secs(60);

/// What a branch's pull request is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub state: State,
}

impl Pr {
    /// How it reads in a row eighteen columns wide.
    ///
    /// An open one says only its number: it is the ordinary case, and a word
    /// that appears on every row tells nobody anything. The other three are
    /// worth a word each, because each one means "stop waiting for this"
    pub fn short(&self) -> String {
        match self.state {
            State::Open => format!("#{}", self.number),
            State::Draft => format!("#{} draft", self.number),
            State::Merged => format!("#{} merged", self.number),
            State::Closed => format!("#{} closed", self.number),
        }
    }
}

/// One branch of one repository.
type Key = (String, String);

struct Slot {
    asked: Instant,
    pr: Option<Pr>,
}

/// Somewhere to ask, that never keeps the window waiting.
pub struct Watch {
    ask: Sender<Key>,
    known: Arc<Mutex<HashMap<Key, Slot>>>,
    /// Whether a token was found at all. Not the token -- nothing here hands
    /// that back out
    pub can_ask: bool,
}

impl Watch {
    pub fn start() -> Watch {
        let token = token();
        let known: Arc<Mutex<HashMap<Key, Slot>>> = Arc::new(Mutex::new(HashMap::new()));
        let (ask, inbox) = channel::<Key>();
        let watch = Watch { ask, known: Arc::clone(&known), can_ask: token.is_some() };
        if let Some(token) = token {
            std::thread::spawn(move || serve(inbox, known, token));
        }
        watch
    }

    /// What is known about this branch, and a nudge to find out if it is time.
    ///
    /// Returning what we have and asking in the background is the whole shape
    /// of this: the row draws now with whatever is known, including nothing
    pub fn of(&self, repo: &str, branch: &str) -> Option<Pr> {
        let key = (repo.to_string(), branch.to_string());
        let mut known = self.known.lock().ok()?;
        match known.get(&key) {
            Some(slot) if slot.asked.elapsed() < FRESH => slot.pr,
            found => {
                let had = found.and_then(|s| s.pr);
                // Marked as asked before the answer arrives, so a slow reply
                // does not turn into one request per frame
                known.insert(key.clone(), Slot { asked: Instant::now(), pr: had });
                drop(known);
                let _ = self.ask.send(key);
                had
            }
        }
    }
}

fn serve(inbox: Receiver<Key>, known: Arc<Mutex<HashMap<Key, Slot>>>, token: String) {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent();
    while let Ok((repo, branch)) = inbox.recv() {
        let pr = look_up(&agent, &token, &repo, &branch);
        if let Ok(mut k) = known.lock() {
            k.insert((repo, branch), Slot { asked: Instant::now(), pr });
        }
    }
}

fn look_up(agent: &ureq::Agent, token: &str, repo: &str, branch: &str) -> Option<Pr> {
    let (owner, _) = repo.split_once('/')?;
    // Newest first, and only one: a branch can have had several pull requests
    // over its life, and the one that matters is the last one
    let url = format!(
        "https://api.github.com/repos/{repo}/pulls\
         ?head={owner}:{branch}&state=all&sort=created&direction=desc&per_page=1"
    );
    let mut resp = agent
        .get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("shikisha-term/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    read_one(v.as_array()?.first()?)
}

/// One pull request, as GitHub describes it.
fn read_one(v: &serde_json::Value) -> Option<Pr> {
    let number = v.get("number")?.as_u64()?;
    // Merged is not one of the states GitHub reports; a merged one is closed
    // with a date on it. Saying "closed" to someone whose work went in would
    // be the most misleading thing this line could do
    let merged = v.get("merged_at").is_some_and(|m| !m.is_null());
    let draft = v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false);
    let closed = v.get("state").and_then(|s| s.as_str()) == Some("closed");
    Some(Pr {
        number,
        state: match (merged, closed, draft) {
            (true, _, _) => State::Merged,
            (_, true, _) => State::Closed,
            (_, _, true) => State::Draft,
            _ => State::Open,
        },
    })
}

/// Whether this machine has a GitHub sign-in at all.
///
/// Asked by the settings screen so that "why are there no pull request
/// numbers" has an answer inside the app, rather than being a silence someone
/// has to guess at. Says whether, never what
pub fn signed_in() -> bool {
    token().is_some()
}

/// The token this person already has.
///
/// Never written anywhere, never logged, and sent to nowhere but GitHub's own
/// API. Read in the order of how deliberate each one is: something they put in
/// their environment on purpose, then what their own GitHub tool stored when
/// they logged in
fn token() -> Option<String> {
    for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(name) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    from_gh()
}

/// What `gh` wrote down when the person logged in.
///
/// Its own file, read and never touched. Only github.com: the rest of that
/// file may describe an enterprise server this app knows nothing about, and
/// sending a token to the wrong host is not a small mistake
fn from_gh() -> Option<String> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    for tail in ["AppData/Roaming/GitHub CLI/hosts.yml", ".config/gh/hosts.yml"] {
        let p = std::path::PathBuf::from(&home).join(tail.replace('/', "\\"));
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        if let Some(t) = oauth_token_for_github(&text) {
            return Some(t);
        }
    }
    None
}

/// The `oauth_token:` under `github.com:` in gh's hosts file.
///
/// Read by hand rather than with a YAML parser: this is two levels of a file
/// with a known shape, and a whole dependency to find one line is a poor
/// trade. Indentation is what says which host a line belongs to
fn oauth_token_for_github(text: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        if indent == 0 {
            inside = trimmed.trim_end_matches(':').trim() == "github.com";
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(v) = trimmed.trim_start().strip_prefix("oauth_token:") {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_one_says_only_its_number() {
        // The ordinary case. A word on every row is a word that tells nobody
        // anything, and the row is eighteen columns wide
        assert_eq!(Pr { number: 12, state: State::Open }.short(), "#12");
        assert_eq!(Pr { number: 12, state: State::Draft }.short(), "#12 draft");
        assert_eq!(Pr { number: 12, state: State::Merged }.short(), "#12 merged");
        assert_eq!(Pr { number: 12, state: State::Closed }.short(), "#12 closed");
    }

    #[test]
    fn merged_is_not_reported_as_closed() {
        // GitHub has no "merged" state: a merged one is closed with a date on
        // it. Telling someone their work was closed when it went in would be
        // the most misleading thing this line could say
        let merged = serde_json::json!({
            "number": 7, "state": "closed", "draft": false,
            "merged_at": "2026-08-25T00:00:00Z"
        });
        assert_eq!(read_one(&merged).unwrap().state, State::Merged);
        let closed = serde_json::json!({
            "number": 8, "state": "closed", "draft": false, "merged_at": null
        });
        assert_eq!(read_one(&closed).unwrap().state, State::Closed);
        let draft = serde_json::json!({
            "number": 9, "state": "open", "draft": true, "merged_at": null
        });
        assert_eq!(read_one(&draft).unwrap().state, State::Draft);
    }

    #[test]
    fn a_reply_that_is_not_a_pull_request_is_not_guessed_at() {
        assert!(read_one(&serde_json::json!({})).is_none());
        assert!(read_one(&serde_json::json!({"state": "open"})).is_none());
    }

    #[test]
    fn the_token_is_taken_only_from_the_host_it_belongs_to() {
        // That file can describe an enterprise server this app knows nothing
        // about. Sending a token to the wrong host is not a small mistake
        let text = "\
github.com:
    user: someone
    oauth_token: gho_theRightOne
    git_protocol: https
git.internal.example:
    user: someone
    oauth_token: gho_theWrongOne
";
        assert_eq!(oauth_token_for_github(text).as_deref(), Some("gho_theRightOne"));

        let other_only = "\
git.internal.example:
    oauth_token: gho_theWrongOne
";
        assert_eq!(oauth_token_for_github(other_only), None);
        assert_eq!(oauth_token_for_github(""), None);
    }

    #[test]
    fn a_quoted_token_is_the_token_without_its_quotes() {
        let text = "github.com:\n    oauth_token: \"gho_quoted\"\n";
        assert_eq!(oauth_token_for_github(text).as_deref(), Some("gho_quoted"));
    }

    #[test]
    fn with_no_token_nothing_is_asked_and_nothing_pretends_to_know() {
        // Constructed without a token: no thread, no requests, and every
        // answer is "nothing known" rather than a made-up one
        let w = Watch { ask: channel().0, known: Arc::new(Mutex::new(HashMap::new())), can_ask: false };
        assert_eq!(w.of("owner/name", "main"), None);
        // Asking twice must not queue twice; the slot is marked before the
        // answer arrives so a slow reply is not one request per frame
        assert_eq!(w.of("owner/name", "main"), None);
        assert_eq!(w.known.lock().unwrap().len(), 1);
    }
}
