//! What became of a pull request since it was last looked at: whether its
//! review is being waited for, came back, or came back with nothing to do,
//! and whether the checks on its newest commit are running or have ended.
//!
//! After a push, the review (a person's, or a bot's that reviews every push)
//! and CI take minutes. Without this, knowing they are done means opening the
//! pull request's page again and again, or forgetting to. Worked out here
//! from what GitHub says of the pull request each time [`crate::pr`] asks,
//! and remembered in a file, so a restart in between neither forgets a wait
//! nor says the same thing twice.
//!
//! Nothing here asks the network: [`judge`] is given what was asked and what
//! was remembered, and answers with what to remember and what to tell.
//!
//! How a review is known to be over, from what reviewers actually leave:
//!
//! - **With findings**: a review (GitHub's own object) by somebody other than
//!   the author. Its threads are the findings.
//! - **With nothing found**: a bot that found nothing leaves no review at all.
//!   It puts a 👍 on the pull request, and edits the summary comment it keeps
//!   there. Either counts once it is newer than the commit being waited on: a
//!   👍 from an earlier round stays on the pull request after the next push.
//!   A comment edited counts only from somebody whose review is being waited
//!   for, since a deploy preview bot edits its comment on every push too.
//!
//! Who is waited for after a push: the reviewers GitHub was asked for, whoever
//! put 👀 on the pull request since the commit, and the bots that reviewed it
//! or gave it a 👍 before -- a bot that reviews every push is never asked to.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FILE: &str = "pr-news.json";
const VERSION: u32 = 1;

/// How long after a push a review is waited for. A bot's review takes a few
/// minutes; past this, whoever was expected is not coming for this commit,
/// and a "waiting" that never ends would be a line nobody believes
pub const WAIT_LIMIT: u64 = 30 * 60;

/// How many pull requests are remembered, the ones looked at most recently.
/// Far more than anyone has open, and the file stays a few kilobytes
const KEPT: usize = 200;

/// How many review ids are kept per pull request: the newest. GitHub is
/// asked for fewer than this each time, so an id that fell off here has
/// fallen off the answer too
const SEEN_KEPT: usize = 40;

/// What the checks on a commit say, all together
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ci {
    /// No checks on it
    #[default]
    None,
    Running,
    Passed,
    Failed,
}

/// Something one person or bot did on the pull request, and when
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Act {
    /// Their login, without the `[bot]` GitHub puts after a bot's in some
    /// places and not in others
    pub who: String,
    pub bot: bool,
    /// Seconds since 1970
    pub at: u64,
}

/// One review, as GitHub keeps it
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Review {
    pub id: String,
    pub by: Act,
    /// The commit it was of
    pub commit: String,
    /// Only an answer in a thread already there: GitHub files every reply
    /// as a review of its own, and a conversation is not a review coming back
    pub reply: bool,
}

/// What GitHub says of one pull request, read out of its answer by
/// [`crate::pr`]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// Open, draft or not. Merged and closed ones are not waited on
    pub open: bool,
    pub author: String,
    /// The newest commit, and when it was made: a push is never before it
    pub head: String,
    pub committed: u64,
    pub ci: Ci,
    /// The names of the checks that failed on it
    pub failed: Vec<String>,
    pub reviews: Vec<Review>,
    /// The reviewers GitHub was asked for and has not heard from
    pub requested: Vec<String>,
    /// 👍 on the pull request itself
    pub thumbs: Vec<Act>,
    /// 👀 on it
    pub eyes: Vec<Act>,
    /// Comments on it, each at when it was last written (made or edited)
    pub comments: Vec<Act>,
}

/// A login written the one way: GitHub says `name[bot]` for a bot's reaction
/// and `name` for the same bot's review
pub fn login(said: &str) -> String {
    said.trim().trim_end_matches("[bot]").to_string()
}

/// What is remembered of one pull request between looks
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mem {
    /// The newest commit when it was last looked at
    #[serde(default)]
    pub head: String,
    /// When the round on `head` began: its commit's time, before which
    /// nothing done on the pull request can be about it
    #[serde(default)]
    pub since: u64,
    /// When the push of `head` was noticed here: a wait is counted from it,
    /// since a commit can be made long before it is pushed
    #[serde(default)]
    pub pushed_at: u64,
    /// A review of `head` is expected and has not come
    #[serde(default)]
    pub waiting: bool,
    /// Bots that reviewed this pull request, or gave it a 👍, without being
    /// asked: who is waited for after the next push
    #[serde(default)]
    pub reviewers: Vec<String>,
    /// The reviews already told about (or there before anything was told)
    #[serde(default)]
    pub seen: Vec<String>,
    /// The newest 👍 or edited comment already counted as an end
    #[serde(default)]
    pub ended_at: u64,
    /// How the last review that came back ended, and on which commit
    #[serde(default)]
    pub ended: Ended,
    #[serde(default)]
    pub ended_on: String,
    /// The commit its checks were seen running on, and the commit whose
    /// checks ending was told
    #[serde(default)]
    pub ci_running: String,
    #[serde(default)]
    pub ci_told: String,
    /// The commit whose push was noticed here: its checks ending is news
    /// however soon they end, even before they were ever seen running
    #[serde(default)]
    pub ci_watch: String,
    /// When it was last written, to keep the newest
    #[serde(default)]
    pub at: u64,
}

/// How a review that came back ended
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    #[default]
    None,
    /// With a review, whose threads say what was found
    Reviewed,
    /// With nothing found
    Clean,
}

/// What came of one look
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Judged {
    pub mem: Mem,
    /// A review came back: its open threads are to be read, and the ones not
    /// yet handed to an AI counted, before anyone is told
    pub reviewed: bool,
    /// A review came back with nothing found, to be told as it is
    pub clean: bool,
    /// The checks on the newest commit have ended, to be told
    pub ci_ended: Option<Ci>,
}

/// What came of this look at a pull request, against what was remembered
/// (`mem`, `Mem::default()` the first time). `now` is seconds since 1970.
///
/// The first look tells nothing: what is already there was not news to
/// anybody when this program started, only to whoever was here when it
/// happened. It does start a wait for a commit pushed a moment ago
pub fn judge(mem: &Mem, s: &Snapshot, now: u64) -> Judged {
    let mut m = mem.clone();
    let first = m.head.is_empty();
    let author = login(&s.author);
    let others = |a: &&Act| login(&a.who) != author;
    // Bots that review this pull request of their own accord, learned as
    // they do it
    for a in s.reviews.iter().map(|r| &r.by).chain(s.thumbs.iter()).filter(others) {
        let who = login(&a.who);
        if a.bot && !m.reviewers.contains(&who) {
            m.reviewers.push(who);
        }
    }
    let pushed = !first && m.head != s.head;
    if first || pushed {
        m.head = s.head.clone();
        m.since = s.committed;
        m.pushed_at = if first { s.committed } else { now };
        m.waiting = false;
    }
    if pushed {
        m.ci_watch = s.head.clone();
    }
    let since = m.since;
    let mut awaited: Vec<String> = m.reviewers.clone();
    awaited.extend(s.requested.iter().map(|w| login(w)));
    awaited.extend(s.eyes.iter().filter(others).filter(|a| a.at >= since).map(|a| login(&a.who)));
    awaited.retain(|w| *w != author);
    // A 👍 from a bot or one waited for, or an edit by a bot waited for,
    // since the commit and newer than the last one counted. A person's 👍 on
    // the description is liking it, and a person's comment ("I will look
    // tomorrow") is not a review; a person reviewing leaves a review
    let edited = s
        .comments
        .iter()
        .filter(|a| a.bot && awaited.contains(&login(&a.who)))
        .map(|a| a.at)
        .filter(|at| *at > since)
        .max()
        .unwrap_or(0);
    let thumb = s
        .thumbs
        .iter()
        .filter(others)
        .filter(|a| a.bot || awaited.contains(&login(&a.who)))
        .map(|a| a.at)
        .filter(|at| *at > since)
        .max()
        .unwrap_or(0);
    let mut out = Judged::default();
    if first {
        m.seen = s.reviews.iter().map(|r| r.id.clone()).collect();
        m.ended_at = thumb.max(edited);
        // Pushed a moment ago, with somebody to wait for and nothing back yet
        let back = s.reviews.iter().any(|r| r.commit == s.head && login(&r.by.who) != author) || thumb > 0;
        m.waiting = s.open && !awaited.is_empty() && !back && now.saturating_sub(s.committed) < WAIT_LIMIT;
        if s.ci == Ci::Running {
            m.ci_running = s.head.clone();
        }
        m.at = now;
        out.mem = m;
        return out;
    }
    if pushed && s.open && !awaited.is_empty() {
        m.waiting = true;
    }
    let unseen: Vec<&Review> = s
        .reviews
        .iter()
        .filter(|r| login(&r.by.who) != author && !m.seen.contains(&r.id))
        .collect();
    m.seen.extend(unseen.iter().map(|r| r.id.clone()));
    // An answer in a thread is counted among what is still to be handed to
    // an AI, but told as nothing of its own; and a closed pull request's
    // review is not news to anybody waiting
    if unseen.iter().any(|r| !r.reply) && s.open {
        m.waiting = false;
        m.ended = Ended::Reviewed;
        m.ended_on = s.head.clone();
        m.ended_at = m.ended_at.max(thumb).max(edited);
        out.reviewed = true;
    } else {
        // An edit only ends a wait: one waited for keeps their comment up to
        // date on every push, and an edit when nothing is waited on is
        // somebody tidying up. A 👍 ends a round whether it was waited for or
        // not -- the first round of a new pull request is a review nobody
        // here knew was coming
        let ended = match m.waiting {
            true => thumb.max(edited),
            false => thumb,
        };
        if ended > m.ended_at && s.open {
            m.ended_at = ended;
            m.waiting = false;
            m.ended = Ended::Clean;
            m.ended_on = s.head.clone();
            out.clean = true;
        }
    }
    if m.seen.len() > SEEN_KEPT {
        let cut = m.seen.len() - SEEN_KEPT;
        m.seen.drain(..cut);
    }
    if m.waiting && (now.saturating_sub(since.max(m.pushed_at)) >= WAIT_LIMIT || !s.open) {
        m.waiting = false;
    }
    // The checks: their end is told once per commit, and only when it came
    // while somebody here could have been waiting for it -- seen running, or
    // on a commit whose push was noticed here. Checks that had already ended
    // when a commit was first looked at ended before anybody was waiting
    match s.ci {
        Ci::Running => m.ci_running = s.head.clone(),
        Ci::Passed | Ci::Failed
            if (m.ci_running == s.head || m.ci_watch == s.head) && m.ci_told != s.head && s.open =>
        {
            m.ci_told = s.head.clone();
            out.ci_ended = Some(s.ci);
        }
        _ => {}
    }
    m.at = now;
    out.mem = m;
    out
}

/// What a row says of the pull request's review, given how many of its open
/// threads have something not yet handed to an AI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Said {
    #[default]
    None,
    Waiting,
    /// Back, with findings not yet handed to an AI (how many is the
    /// caller's to say)
    Found,
    Clean,
}

/// The review as a row shows it: waited for; back with findings not yet
/// handed over; or back on the newest commit with nothing to do. Findings all
/// handed over say nothing: the AI has them
pub fn said(m: &Mem, fresh: usize) -> Said {
    match () {
        _ if m.waiting => Said::Waiting,
        _ if m.ended == Ended::Reviewed && fresh > 0 => Said::Found,
        _ if m.ended_on == m.head && !m.head.is_empty() && (m.ended == Ended::Clean || m.ended == Ended::Reviewed) => {
            Said::Clean
        }
        _ => Said::None,
    }
}

/// Whether a push noticed a moment ago is still settling: its review and its
/// checks may start any minute, so it is asked about as often as a wait is
pub fn settling(m: &Mem, now: u64) -> bool {
    !m.head.is_empty()
        && m.ci_watch == m.head
        && m.ci_told != m.head
        && now.saturating_sub(m.pushed_at) < WAIT_LIMIT
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// GitHub's time (`2026-11-09T05:39:55Z`) in seconds since 1970; 0 when it
/// is not one
pub fn seconds(stamp: &str) -> u64 {
    let s = stamp.trim();
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) =
        (num(0, 4), num(5, 7), num(8, 10), num(11, 13), num(14, 16), num(17, 19))
    else {
        return 0;
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return 0;
    }
    // Days from 1970 by the civil calendar (proleptic Gregorian)
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + h * 3_600 + mi * 60 + se).max(0) as u64
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    prs: BTreeMap<String, Mem>,
}

pub fn path() -> PathBuf {
    crate::config::root_dir().join("data").join(FILE)
}

/// What a pull request is remembered by: its repository and number
pub fn key(repo: &str, number: u64) -> String {
    format!("{}#{number}", repo.to_ascii_lowercase())
}

/// Everything remembered, by [`key`]
pub fn read(file: &Path) -> BTreeMap<String, Mem> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Store>(t.trim_start_matches('\u{feff}')).ok())
        .filter(|s| s.version <= VERSION)
        .map(|s| s.prs)
        .unwrap_or_default()
}

/// Remember these, the newest [`KEPT`] of them
pub fn write(file: &Path, mut prs: BTreeMap<String, Mem>) {
    if prs.len() > KEPT {
        let mut by_age: Vec<(u64, String)> = prs.iter().map(|(k, m)| (m.at, k.clone())).collect();
        by_age.sort();
        for (_, k) in by_age.iter().take(prs.len() - KEPT) {
            prs.remove(k);
        }
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&Store { version: VERSION, prs }) {
        let _ = crate::crypto::write_atomic(file, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_000_000;

    fn bot(who: &str, at: u64) -> Act {
        Act { who: who.into(), bot: true, at }
    }

    fn pr(head: &str, committed: u64) -> Snapshot {
        Snapshot {
            number: 7,
            title: "Fix".into(),
            url: "https://github.com/o/r/pull/7".into(),
            open: true,
            author: "me".into(),
            head: head.into(),
            committed,
            ..Default::default()
        }
    }

    fn review(id: &str, who: &str, commit: &str, at: u64) -> Review {
        Review { id: id.into(), by: bot(who, at), commit: commit.into(), reply: false }
    }

    #[test]
    fn the_first_look_tells_nothing_and_remembers_what_is_there() {
        let mut s = pr("a", T0);
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        s.ci = Ci::Passed;
        let j = judge(&Mem::default(), &s, T0 + 600);
        assert!(!j.reviewed && !j.clean && j.ci_ended.is_none());
        assert_eq!(j.mem.seen, ["r1"]);
        assert_eq!(j.mem.reviewers, ["reviewer-bot"]);
        // Its review of this commit is back: nothing is waited for
        assert!(!j.mem.waiting);
    }

    #[test]
    fn a_push_waits_for_the_bot_that_reviewed_before_and_its_review_is_told() {
        let mut s = pr("a", T0);
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        let m = judge(&Mem::default(), &s, T0 + 100).mem;
        // Pushed: the bot that reviewed of its own accord is waited for
        let mut s = pr("b", T0 + 1_000);
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        let j = judge(&m, &s, T0 + 1_030);
        assert!(j.mem.waiting);
        assert_eq!(said(&j.mem, 0), Said::Waiting);
        // Its review of the new commit comes back
        s.reviews.push(review("r2", "reviewer-bot", "b", T0 + 1_200));
        let j = judge(&j.mem, &s, T0 + 1_230);
        assert!(j.reviewed && !j.clean);
        assert!(!j.mem.waiting);
        assert_eq!(said(&j.mem, 2), Said::Found);
        // All of it handed to an AI: a review of the newest commit, nothing to do
        assert_eq!(said(&j.mem, 0), Said::Clean);
        // Looked at again: told once
        let again = judge(&j.mem, &s, T0 + 1_300);
        assert!(!again.reviewed);
    }

    #[test]
    fn a_thumb_newer_than_the_commit_is_a_review_that_found_nothing() {
        let mut s = pr("a", T0);
        s.thumbs = vec![bot("reviewer-bot[bot]", T0 + 100)];
        let m = judge(&Mem::default(), &s, T0 + 200).mem;
        assert_eq!(m.reviewers, ["reviewer-bot"]);
        // The 👍 from the round before stays on the pull request after a
        // push, and is not this commit's
        let mut s = pr("b", T0 + 1_000);
        s.thumbs = vec![bot("reviewer-bot[bot]", T0 + 100)];
        let j = judge(&m, &s, T0 + 1_010);
        assert!(j.mem.waiting && !j.clean);
        // Taken off and put back once it finds nothing in the new commit
        s.thumbs = vec![bot("reviewer-bot[bot]", T0 + 1_300)];
        let j = judge(&j.mem, &s, T0 + 1_320);
        assert!(j.clean && !j.mem.waiting);
        assert_eq!(said(&j.mem, 0), Said::Clean);
        assert!(!judge(&j.mem, &s, T0 + 1_400).clean, "told twice");
    }

    #[test]
    fn an_edited_comment_ends_a_wait_only_from_one_waited_for() {
        let mut s = pr("a", T0);
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        let m = judge(&Mem::default(), &s, T0 + 100).mem;
        let mut s = pr("b", T0 + 1_000);
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        // A deploy preview bot writes on every push: not a review
        s.comments = vec![bot("preview-bot", T0 + 1_050)];
        let j = judge(&m, &s, T0 + 1_060);
        assert!(j.mem.waiting && !j.clean);
        // The reviewing bot brings its summary up to date: done, nothing found
        s.comments.push(bot("reviewer-bot", T0 + 1_200));
        let j = judge(&j.mem, &s, T0 + 1_210);
        assert!(j.clean && !j.mem.waiting);
    }

    #[test]
    fn a_new_pull_request_with_a_thumb_from_nobody_expected_is_still_told() {
        let s = pr("a", T0);
        let m = judge(&Mem::default(), &s, T0 + 30).mem;
        assert!(!m.waiting, "nobody is known to review this one yet");
        let mut s = pr("a", T0);
        s.thumbs = vec![bot("reviewer-bot[bot]", T0 + 200)];
        assert!(judge(&m, &s, T0 + 230).clean);
    }

    #[test]
    fn the_author_answering_in_a_thread_is_not_a_review() {
        let s = pr("a", T0);
        let m = judge(&Mem::default(), &s, T0 + 30).mem;
        let mut s = pr("a", T0);
        s.reviews = vec![Review { id: "mine".into(), by: Act { who: "me".into(), bot: false, at: T0 + 90 }, commit: "a".into(), reply: false }];
        let j = judge(&m, &s, T0 + 100);
        assert!(!j.reviewed);
    }

    /// GitHub files each answer in a thread as a review of its own: counted
    /// among the findings, never told as a review coming back
    #[test]
    fn an_answer_in_a_thread_is_not_a_review_coming_back() {
        let s = pr("a", T0);
        let m = judge(&Mem::default(), &s, T0 + 30).mem;
        let mut s = pr("a", T0);
        s.reviews = vec![Review { reply: true, ..review("r9", "alice", "a", T0 + 90) }];
        let j = judge(&m, &s, T0 + 100);
        assert!(!j.reviewed && !j.clean);
        assert!(j.mem.seen.contains(&"r9".to_string()), "it would be news on every look");
    }

    /// A teammate liking the description is not a review that found nothing
    #[test]
    fn a_persons_thumb_is_not_a_review() {
        let s = pr("a", T0);
        let m = judge(&Mem::default(), &s, T0 + 30).mem;
        let mut s = pr("a", T0);
        s.thumbs = vec![Act { who: "teammate".into(), bot: false, at: T0 + 60 }];
        assert!(!judge(&m, &s, T0 + 90).clean);
        // ...unless that person was asked to review
        s.requested = vec!["teammate".into()];
        assert!(judge(&m, &s, T0 + 90).clean);
    }

    /// Nothing is told of a pull request that was merged or closed meanwhile
    #[test]
    fn a_closed_pull_request_is_not_told() {
        let s = pr("a", T0);
        let m = judge(&Mem::default(), &s, T0 + 30).mem;
        let mut s = pr("a", T0);
        s.open = false;
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        s.ci = Ci::Passed;
        let j = judge(&Mem { ci_running: "a".into(), ..m }, &s, T0 + 90);
        assert!(!j.reviewed && j.ci_ended.is_none());
    }

    /// A commit made long before it was pushed is waited on from the push
    #[test]
    fn a_wait_counts_from_the_push_not_the_commit() {
        let mut s = pr("a", T0);
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        let m = judge(&Mem::default(), &s, T0 + 100).mem;
        // Made an hour before it was pushed
        let mut s = pr("b", T0 + 200);
        s.reviews = vec![review("r1", "reviewer-bot", "a", T0 + 60)];
        let pushed = T0 + 200 + 3_600;
        let j = judge(&m, &s, pushed);
        assert!(j.mem.waiting);
        assert!(judge(&j.mem, &s, pushed + 60).mem.waiting, "the wait ended at once");
        assert!(!judge(&j.mem, &s, pushed + WAIT_LIMIT + 1).mem.waiting);
    }

    /// Checks that end before they were ever seen running are told all the
    /// same when the push they ran on was noticed here
    #[test]
    fn checks_that_end_quickly_after_a_push_are_told() {
        let s = pr("a", T0);
        let m = judge(&Mem::default(), &s, T0 + 10).mem;
        let mut s = pr("b", T0 + 100);
        s.ci = Ci::None;
        let j = judge(&m, &s, T0 + 110);
        assert!(settling(&j.mem, T0 + 110), "a push is asked about often while it settles");
        s.ci = Ci::Failed;
        let j = judge(&j.mem, &s, T0 + 200);
        assert_eq!(j.ci_ended, Some(Ci::Failed));
        assert!(!settling(&j.mem, T0 + 200));
    }

    #[test]
    fn a_wait_ends_by_itself() {
        let mut s = pr("a", T0);
        s.requested = vec!["alice".into()];
        let m = judge(&Mem::default(), &s, T0 + 10).mem;
        assert!(m.waiting, "a reviewer was asked for a moment ago");
        let j = judge(&m, &s, T0 + WAIT_LIMIT + 1);
        assert!(!j.mem.waiting && !j.clean && !j.reviewed);
        // Pushed long ago, first seen now: nothing to wait for
        assert!(!judge(&Mem::default(), &s, T0 + WAIT_LIMIT + 5).mem.waiting);
    }

    #[test]
    fn the_end_of_the_checks_is_told_once_and_only_when_seen_running() {
        let mut s = pr("a", T0);
        s.ci = Ci::Running;
        let m = judge(&Mem::default(), &s, T0 + 10).mem;
        s.ci = Ci::Failed;
        let j = judge(&m, &s, T0 + 100);
        assert_eq!(j.ci_ended, Some(Ci::Failed));
        assert_eq!(judge(&j.mem, &s, T0 + 200).ci_ended, None);
        // Run again on the same commit and passed: already told about this one
        let mut s2 = s.clone();
        s2.ci = Ci::Running;
        let m = judge(&j.mem, &s2, T0 + 300).mem;
        s2.ci = Ci::Passed;
        assert_eq!(judge(&m, &s2, T0 + 400).ci_ended, None);
        // A pull request whose run had already ended when this program first
        // looked at it: it ended before anybody here was waiting
        let mut s3 = pr("b", T0 + 500);
        s3.ci = Ci::Passed;
        assert_eq!(judge(&Mem::default(), &s3, T0 + 600).ci_ended, None);
    }

    #[test]
    fn github_times_are_read() {
        assert_eq!(seconds("1970-01-01T00:00:00Z"), 0);
        assert_eq!(seconds("2026-11-09T05:39:55Z"), 1_794_202_795);
        assert_eq!(seconds("2024-03-01T00:00:00Z") - seconds("2024-02-28T00:00:00Z"), 2 * 86_400);
        assert_eq!(seconds(""), 0);
        assert_eq!(seconds("not a time"), 0);
    }

    #[test]
    fn remembered_across_a_restart() {
        let dir = std::env::temp_dir().join(format!("shikisha-prnews-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join(FILE);
        let mut prs = BTreeMap::new();
        prs.insert(key("O/R", 7), Mem { head: "a".into(), waiting: true, at: 5, ..Default::default() });
        write(&file, prs);
        let back = read(&file);
        assert!(back.get("o/r#7").is_some_and(|m| m.waiting && m.head == "a"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
