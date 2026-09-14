//! The pull request a branch is on, when there is one.
//!
//! The third thing you would otherwise go and look up, after the branch and
//! the ports. With several agents each on their own branch, "has that one been
//! merged yet" is a question about every row at once, and answering it means
//! leaving the terminal entirely.
//!
//! **It asks as the account somebody chose.** A folder's project chooses the git
//! account its column signs in with, and the pull request number is read with
//! that same account: a token it was given, or -- when the choice was "the way
//! git on this PC signs in" -- what git's own credential store hands out for
//! GitHub. Nothing is chosen for anybody, so a row with no number is a row
//! whose project has no account chosen, and the settings say which.
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

/// One branch of one repository, as one account sees it: (account, repo,
/// branch). The account is part of it because a repository one account can
/// see is one another may not
type Key = (String, String, String);

struct Slot {
    asked: Instant,
    pr: Option<Pr>,
}

/// How long what this PC's git hands out for GitHub is trusted before it is
/// asked again. Somebody signing in again should not wait for a restart
const PC_FRESH: Duration = Duration::from_secs(600);

/// Somewhere to ask, that never keeps the window waiting.
pub struct Watch {
    ask: Sender<Key>,
    known: Arc<Mutex<HashMap<Key, Slot>>>,
    /// The tokens of the desk on screen, by account name. Held here rather
    /// than handed to the thread once, because the thread outlives any one
    /// desk. Never handed back out
    tokens: Arc<Mutex<HashMap<String, String>>>,
}

impl Watch {
    pub fn start() -> Watch {
        let known: Arc<Mutex<HashMap<Key, Slot>>> = Arc::new(Mutex::new(HashMap::new()));
        let tokens: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let (ask, inbox) = channel::<Key>();
        std::thread::spawn({
            let (known, tokens) = (Arc::clone(&known), Arc::clone(&tokens));
            move || serve(inbox, known, tokens)
        });
        Watch { ask, known, tokens }
    }

    /// The tokens the desk now on screen asks with, by account.
    ///
    /// What is already known is thrown away when they change: the same account
    /// name in another desk is another account, and "no pull request" and "not
    /// allowed to look" would otherwise be the same empty line
    pub fn use_tokens(&self, tokens: HashMap<String, String>) {
        if let Ok(mut held) = self.tokens.lock() {
            if *held == tokens {
                return;
            }
            *held = tokens;
        }
        if let Ok(mut known) = self.known.lock() {
            known.clear();
        }
    }

    /// What is known about this branch as `account` sees it, and a nudge to
    /// find out if it is time.
    ///
    /// Returning what we have and asking in the background is the whole shape
    /// of this: the row draws now with whatever is known, including nothing
    pub fn of(&self, account: &str, repo: &str, branch: &str) -> Option<Pr> {
        let key = (account.to_string(), repo.to_string(), branch.to_string());
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

fn serve(
    inbox: Receiver<Key>,
    known: Arc<Mutex<HashMap<Key, Slot>>>,
    tokens: Arc<Mutex<HashMap<String, String>>>,
) {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent();
    let mut pc: Option<(Instant, Option<String>)> = None;
    while let Ok((account, repo, branch)) = inbox.recv() {
        // Read for each question rather than once: the desk, and with it the
        // accounts, can have changed since the last one
        let token = match account.as_str() {
            crate::config::THIS_PC => {
                if pc.as_ref().is_none_or(|(at, _)| at.elapsed() > PC_FRESH) {
                    pc = Some((Instant::now(), pc_token()));
                }
                pc.as_ref().and_then(|(_, t)| t.clone())
            }
            name => tokens.lock().ok().and_then(|t| t.get(name).cloned()),
        };
        let Some(token) = token else { continue };
        let pr = look_up(&agent, &token, &repo, &branch);
        if let Ok(mut k) = known.lock() {
            k.insert((account, repo, branch), Slot { asked: Instant::now(), pr });
        }
    }
}

/// What git on this PC hands out for GitHub: the credential a push from a
/// terminal here would sign in with. Asked of git's own credential store the
/// way git asks it, with nobody to prompt -- a store that has nothing says so
/// and this is None
pub fn pc_token() -> Option<String> {
    let said = crate::git::run_as(
        &std::env::temp_dir(),
        &["-c", "credential.interactive=never", "credential", "fill"],
        &format!("protocol=https\nhost={}\n\n", crate::config::GITHUB_HOST),
        Duration::from_secs(15),
        &crate::git::As::default(),
    )
    .ok()?;
    said.lines()
        .find_map(|l| l.strip_prefix("password="))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
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

/// The token to ask with: the one the account was given, and nothing else.
///
/// Never written anywhere, never logged, and sent to nowhere but GitHub's own
/// API. The machine's `GITHUB_TOKEN` is not read: it is one account for every
/// desk, and nobody chose it.
///
/// `own` is the account's token, already looked up by whoever knows which
/// desk is being asked about -- this module never reaches into the secrets
pub fn find(own: Option<String>) -> Option<String> {
    own.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// What GitHub says about a token: whether it still works, whose it is, and
/// when it stops working.
///
/// The state and the date, never the value. A token that has run out is said
/// out loud -- a row that quietly stops showing pull request numbers looks
/// exactly like a branch that has none
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// Where the token came from, or `None` when there is no token at all
    pub source: Option<&'static str>,
    /// Whether GitHub answered the way it answers a token it accepts
    pub ok: bool,
    /// The account the token speaks as
    pub login: Option<String>,
    /// When it stops working, as GitHub wrote it, and in days from today.
    /// Both absent means a token that does not expire
    pub expires_at: Option<String>,
    pub expires_in_days: Option<i64>,
    /// What GitHub answered with, for the one case worth telling apart: 401 is
    /// a token that has run out or been taken away
    pub status: u16,
}

/// Ask GitHub about the token this desk would use.
///
/// Its own call rather than a side effect of asking about a branch: the
/// settings screen has to be able to say "this one works and has eleven days
/// left" before anybody is looking at a branch at all
pub fn probe(own: Option<String>) -> Probe {
    let Some(token) = find(own) else {
        return Probe::default();
    };
    let mut said = Probe {
        source: Some("desk"),
        ..Default::default()
    };
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .new_agent();
    let answer = agent
        .get("https://api.github.com/user")
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("shikisha-term/", env!("CARGO_PKG_VERSION")),
        )
        .call();
    let mut resp = match answer {
        Ok(r) => r,
        // A refusal comes back as an error in this client, and the status is
        // the whole point of asking
        Err(ureq::Error::StatusCode(code)) => {
            said.status = code;
            return said;
        }
        Err(_) => return said,
    };
    said.status = resp.status().as_u16();
    said.ok = said.status == 200;
    // GitHub puts the end date of a fine-grained token in a header of its own,
    // on every answer. Absent means one that does not expire
    if let Some(when) = resp
        .headers()
        .get("github-authentication-token-expiration")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        said.expires_in_days = days_until(when);
        said.expires_at = Some(when.to_string());
    }
    if let Ok(v) = resp.body_mut().read_json::<serde_json::Value>() {
        said.login = v
            .get("login")
            .and_then(|l| l.as_str())
            .map(str::to_string);
    }
    said
}

/// How many days from today until a date GitHub wrote down.
///
/// Its header reads `2026-10-04 12:00:00 UTC`, and only the day matters: "8
/// days left" is what a person acts on. Worked out here rather than with a date
/// library, because the whole of the arithmetic is turning two calendar days
/// into two numbers
fn days_until(stamp: &str) -> Option<i64> {
    // Its header reads `2026-10-04 12:00:00 UTC` today. The date is taken off
    // the front either way a time can be joined to it, because a header's
    // spelling is GitHub's to change and the day is all of it that is read
    let day = stamp.split([' ', 'T']).next()?;
    let mut part = day.split('-');
    let y: i64 = part.next()?.parse().ok()?;
    let m: i64 = part.next()?.parse().ok()?;
    let d: i64 = part.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64
        / 86_400;
    Some(days_from_epoch(y, m, d) - now)
}

/// Days from 1970-01-01 to a calendar day (Howard Hinnant's algorithm).
fn days_from_epoch(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

    /// Only the desk's own token is asked with. One machine, two accounts:
    /// a token in the machine's environment belongs to every desk at once,
    /// so a desk given none has none
    #[test]
    fn only_the_desk_own_token_is_used() {
        // SAFETY: this process's own environment, in a test that puts it back
        unsafe {
            std::env::set_var("GITHUB_TOKEN", "from_the_environment");
        }
        assert_eq!(find(Some("  ours  ".into())).as_deref(), Some("ours"), "surrounding spaces got into the value");
        assert_eq!(find(None), None, "the machine's token was used for a desk");
        assert_eq!(find(Some("   ".into())), None, "an empty token was used");
        unsafe {
            std::env::remove_var("GITHUB_TOKEN");
        }
    }

    /// "Eleven days left" is what a person acts on, so the date has to become a
    /// number of days -- in GitHub's own spelling of a date, including the
    /// shapes that are not a date at all
    #[test]
    fn a_date_becomes_the_days_that_are_left() {
        // The day count itself, independent of today
        assert_eq!(days_from_epoch(1970, 1, 1), 0);
        assert_eq!(days_from_epoch(1970, 1, 2), 1);
        assert_eq!(days_from_epoch(2000, 3, 1), 11017);
        // A leap day is a day
        assert_eq!(days_from_epoch(2024, 3, 1) - days_from_epoch(2024, 2, 28), 2);
        assert_eq!(days_from_epoch(2023, 3, 1) - days_from_epoch(2023, 2, 28), 1);

        // Today, through the same door the header comes in by
        let today = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            / 86_400;
        let stamp = |days: i64| {
            let d = today + days;
            // Turned back into a date the long way round, so the test is not
            // checking the arithmetic against itself
            let (mut y, mut rest) = (1970, d);
            loop {
                let len = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
                if rest < len {
                    break;
                }
                rest -= len;
                y += 1;
            }
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let lens = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            let mut m = 1;
            for len in lens {
                if rest < len {
                    break;
                }
                rest -= len;
                m += 1;
            }
            format!("{y:04}-{m:02}-{:02} 12:00:00 UTC", rest + 1)
        };
        assert_eq!(days_until(&stamp(11)), Some(11));
        assert_eq!(days_until(&stamp(0)), Some(0));
        assert_eq!(days_until(&stamp(-3)), Some(-3), "an expired one comes out as negative days");

        // Not a date: nothing, rather than a number somebody would believe
        // The same day, with the time joined on the other way round
        assert_eq!(days_until(&stamp(11).replace(' ', "T")), Some(11));
        assert_eq!(days_until(""), None);
        assert_eq!(days_until("never"), None);
        assert_eq!(days_until("2026-13-01 00:00:00 UTC"), None);
    }

    #[test]
    fn with_no_token_nothing_is_asked_and_nothing_pretends_to_know() {
        // No token: every answer is "nothing known" rather than a made-up one
        let w = Watch::start();
        assert_eq!(w.of("work", "owner/name", "main"), None);
        // Asking twice must not queue twice; the slot is marked before the
        // answer arrives so a slow reply is not one request per frame
        assert_eq!(w.of("work", "owner/name", "main"), None);
        assert_eq!(w.known.lock().unwrap().len(), 1);
        // Another account asking about the same branch is another question
        assert_eq!(w.of("home", "owner/name", "main"), None);
        assert_eq!(w.known.lock().unwrap().len(), 2);
    }

    /// The real thing, against the real GitHub, with a real token.
    ///
    /// Ignored by default: it needs the network and a token, and neither belongs
    /// in an ordinary run. Run it by hand when the shape of GitHub's answer is
    /// what is in doubt -- the expiry header especially, which is the one thing
    /// here that no fixture can vouch for:
    ///
    /// ```text
    /// SHIKISHA_GITHUB_PROBE=<a fine-grained token> cargo test -p shikisha-core \
    ///     -- --ignored --nocapture the_real_github
    /// ```
    ///
    /// It prints the state, the account and the days left. Never the token
    #[test]
    #[ignore = "needs the network and a token of your own"]
    fn the_real_github_answers_the_way_this_reads_it() {
        let Ok(token) = std::env::var("SHIKISHA_GITHUB_PROBE") else {
            panic!("run it with a token in SHIKISHA_GITHUB_PROBE");
        };
        let said = probe(Some(token));
        println!(
            "source={:?} ok={} status={} login={:?} expires_at={:?} days={:?}",
            said.source, said.ok, said.status, said.login, said.expires_at, said.expires_in_days
        );
        assert_eq!(said.source, Some("desk"));
        assert!(said.ok, "GitHub did not accept it: status={}", said.status);
        assert!(said.login.is_some(), "the account name was not read");
        // An expiry is not guaranteed -- a token can be made without one -- but
        // when the header is there it has to become a number of days
        if said.expires_at.is_some() {
            assert!(said.expires_in_days.is_some(), "the expiry date does not become a number of days");
        }
    }

    /// A new token means the old answers are somebody else's.
    ///
    /// A repository one account can see is one the other may not, so keeping
    /// what was learned would make "there is no pull request" and "you are not
    /// allowed to look" the same empty line
    #[test]
    fn changing_the_token_forgets_what_the_other_one_saw() {
        let w = Watch::start();
        assert_eq!(w.of("work", "owner/name", "main"), None);
        assert_eq!(w.known.lock().unwrap().len(), 1);
        let ours = HashMap::from([("work".to_string(), "ours".to_string())]);
        w.use_tokens(ours.clone());
        assert!(w.known.lock().unwrap().is_empty(), "the previous account's answer is still there");
        // Saying the same thing twice is not a change, and must not throw away
        // answers that are still this token's
        assert_eq!(w.of("work", "owner/name", "main"), None);
        w.use_tokens(ours);
        assert_eq!(w.known.lock().unwrap().len(), 1);
    }
}
