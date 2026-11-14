//! Issues and pull requests, one GitHub repository at a time.
//!
//! What the automation commands `github_*` do, and what the Issue tab shows by
//! calling the same functions. Every call signs in as the git account the
//! repository's project chose ([`crate::config::GitUse`]): its token, or -- for
//! "this PC's git settings" -- the GitHub credential git on this PC already
//! stores. Nothing is signed in as anybody nobody chose.
//!
//! Plain REST, one request per question, and answers shaped for a screen:
//! each function returns JSON whose field names are this module's, so a change
//! on GitHub's side is absorbed here rather than in every page and script.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

/// How many rows one page of a list holds
pub const PAGE: u32 = 36;

/// A repository on GitHub: `owner/name`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    pub fn parse(slug: &str) -> Option<Repo> {
        let (owner, name) = slug.trim().split_once('/')?;
        let ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        (ok(owner) && ok(name)).then(|| Repo {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// A refusal the project's account settings can answer: no account chosen, no
/// token, or a token GitHub will not let see the repository. Told apart from
/// the rest (a limit reached, GitHub out of reach) so only these offer the settings
#[derive(Debug)]
pub struct AccountTrouble(pub String);

impl std::fmt::Display for AccountTrouble {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AccountTrouble {}

/// Whether an address from an issue may be handed to this PC's browser: an
/// address on the web and nothing else. The call that opens it would as
/// happily start a program or open a file, so a path, another scheme, or
/// anything a shell could read as more than one word is refused
pub fn openable_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && url.len() > "https://".len()
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '<' | '>' | '`' | '\\' | '^' | '|'))
}

/// Whether the project's settings are where this error is put right
pub fn settings_fix(e: &anyhow::Error) -> bool {
    e.downcast_ref::<AccountTrouble>().is_some()
}

/// Which repository a folder is, and the token to ask as.
///
/// The folder's GitHub remote, and the account its project chose. Said in
/// words when either is missing, because both are things a person fixes: a
/// folder whose remote is somewhere else, an account nobody has chosen yet
pub fn target(
    dir: &Path,
    git: &crate::config::GitUse,
    look: &dyn Fn(&str) -> Option<String>,
) -> Result<(Repo, String)> {
    target_with(None, dir, git, look)
}

/// The same, with the repository already known (`owner/name`) when it is:
/// nothing is asked of the folder then
fn target_with(
    known: Option<String>,
    dir: &Path,
    git: &crate::config::GitUse,
    look: &dyn Fn(&str) -> Option<String>,
) -> Result<(Repo, String)> {
    use crate::config::GitUse;
    // A folder on another machine (noted on this thread, see `git::there`):
    // its remote is asked of git there, since there is nothing here to read
    let far = || {
        crate::git::is_far(dir)
            .then(|| crate::git::run(dir, &["remote", "get-url", "origin"]).ok())
            .flatten()
            .and_then(|u| crate::repo::github_path(u.trim()))
    };
    let slug = known.or_else(|| crate::repo::origin_of(dir)).or_else(far).ok_or_else(|| {
        anyhow!(crate::i18n::tp(
            "err.github.not_github",
            &[("p", &dir.display().to_string())]
        ))
    })?;
    let repo = Repo::parse(&slug)
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.github.not_github", &[("p", &slug)])))?;
    let token = match git {
        GitUse::Missing(name) => bail!(AccountTrouble(crate::i18n::tp(
            "err.git.account.missing",
            &[("name", name)]
        ))),
        // Nothing chosen reads GitHub as this PC's git, the same as it signs in
        GitUse::Unset | GitUse::Pc(_) => {
            let login = match git {
                GitUse::Pc(login) => login.as_deref(),
                _ => None,
            };
            crate::pr::pc_token(login).map_err(|why| {
                AccountTrouble(match why {
                    crate::pr::PcSignIn::None => crate::i18n::t("err.github.pc_none"),
                    crate::pr::PcSignIn::Many(names) => crate::i18n::tp(
                        "err.github.pc_many",
                        &[("names", &names.join(", "))],
                    ),
                    crate::pr::PcSignIn::Gone(login) => {
                        crate::i18n::tp("err.github.pc_gone", &[("login", &login)])
                    }
                })
            })?
        }
        GitUse::Gh { host, login } => {
            if host != crate::config::GITHUB_HOST {
                bail!(AccountTrouble(crate::i18n::tp(
                    "err.github.host",
                    &[("name", &git.written()), ("host", host)]
                )));
            }
            crate::pr::gh_token_of(host, login).ok_or_else(|| {
                AccountTrouble(crate::i18n::tp("err.git.gh_gone", &[("login", login), ("host", host)]))
            })?
        }
        GitUse::Account { spec } => {
            if spec.host() != crate::config::GITHUB_HOST {
                bail!(AccountTrouble(crate::i18n::tp(
                    "err.github.host",
                    &[("name", spec.shown()), ("host", &spec.host())]
                )));
            }
            spec.token(look).ok_or_else(|| AccountTrouble(spec.no_token_said()))?
        }
    };
    Ok((repo, token))
}

/// What a list is asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// `open`, `closed`, `merged` (pull requests only) or `all`. Empty is `open`
    pub state: String,
    /// Issues assigned to the account asking; pull requests it opened
    pub mine: bool,
    /// Pull requests waiting for the account's review
    pub review: bool,
    /// Words to search for, as GitHub's search reads them
    pub text: String,
    /// From 1
    pub page: u32,
}

impl Query {
    pub fn from_json(v: &Value) -> Query {
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        Query {
            state: s("state"),
            mine: b("mine"),
            review: b("review"),
            text: s("text"),
            page: v.get("page").and_then(|x| x.as_u64()).unwrap_or(1).max(1) as u32,
        }
    }
}

/// The search GitHub is asked for one list: the repository, the kind, the
/// state, whose, and the words.
pub fn search_q(repo: &Repo, pulls: bool, q: &Query) -> String {
    let mut parts = vec![
        format!("repo:{}", repo.slug()),
        (if pulls { "is:pr" } else { "is:issue" }).to_string(),
    ];
    match q.state.trim() {
        "all" => {}
        "closed" if pulls => parts.push("is:closed -is:merged".into()),
        "closed" => parts.push("is:closed".into()),
        "merged" if pulls => parts.push("is:merged".into()),
        _ => parts.push("is:open".into()),
    }
    if q.mine {
        parts.push((if pulls { "author:@me" } else { "assignee:@me" }).into());
    }
    if q.review && pulls {
        parts.push("review-requested:@me".into());
    }
    let text = q.text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }
    parts.join(" ")
}

/// A name for a worktree made to work on an issue or a pull request.
///
/// From the title, in the letters a branch and a folder can both hold. A title
/// with nothing of those in it -- a Japanese one, say -- is `issue-12` or
/// `pr-12`, so every one of them still has a name that says where it came from
pub fn workspace_name(pulls: bool, number: u64, title: &str) -> String {
    let lower = title.trim().to_lowercase();
    // "Issue 12: fix", "#12 fix" and "fix (#12)" all name the same subject
    let mut subject = lower.as_str();
    for head in ["issue", "pull request", "pr"] {
        if let Some(rest) = subject.strip_prefix(head) {
            let rest = rest.trim_start().trim_start_matches(['#', '!']);
            let digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            if digits.len() < rest.len() {
                subject = digits.trim_start_matches([' ', ':', '-']).trim_start();
            }
        }
    }
    let cleaned: String = subject.replace('\'', "");
    let mut slug = String::new();
    for c in cleaned.chars() {
        let keep = c.is_ascii_alphanumeric() || c == '.' || c == '_';
        match keep {
            true => slug.push(c),
            false if !slug.ends_with('-') => slug.push('-'),
            false => {}
        }
    }
    // "(#12)" and a bare "#12" are the number again, said in the title
    let words: Vec<&str> = slug.split('-').filter(|w| !w.is_empty()).collect();
    let words: Vec<&str> = words
        .iter()
        .copied()
        .filter(|w| {
            !(w.chars().all(|c| c.is_ascii_digit()) && w.parse::<u64>().ok() == Some(number))
        })
        .collect();
    let mut slug = words.join("-");
    while slug.contains("..") {
        slug = slug.replace("..", ".");
    }
    let whole = slug.trim_matches(['.', '-', '_']);
    let mut slug: String = whole.chars().take(48).collect();
    // Cut short between words, not inside one: "broken-table", not "broken-tab"
    if slug.len() < whole.len()
        && whole.as_bytes()[slug.len()] != b'-'
        && let Some(at) = slug.rfind('-')
    {
        slug.truncate(at);
    }
    while slug.ends_with(['.', '-', '_']) {
        slug.pop();
    }
    let kind = if pulls { "pr" } else { "issue" };
    match slug.is_empty() {
        true => format!("{kind}-{number}"),
        false => format!("{kind}-{number}-{slug}"),
    }
}

/// One conversation with GitHub, as one account.
pub struct Hub {
    agent: ureq::Agent,
    token: String,
}

/// The newest GitHub REST version this is written against
const API_VERSION: &str = "2022-11-28";

/// The longest one request to GitHub is waited for, from connecting to the
/// last byte of the answer
const REQUEST_LIMIT: Duration = Duration::from_secs(20);

/// The longest one file put on GitHub for an issue is waited for: a video at
/// GitHub's largest over a line of a few megabits a second
const UPLOAD_LIMIT: Duration = Duration::from_secs(600);

/// The largest image and video GitHub takes from a paste, as its own command
/// line tool counts them
pub const IMAGE_BYTES: usize = 10 << 20;
pub const VIDEO_BYTES: usize = 100 << 20;

/// What can be put on GitHub for an issue to show, by the kind a browser
/// names it, and how large each may be: the images and videos GitHub's page
/// takes from a paste. Handed to the page too, which says so before sending
pub const ATTACHABLE: &[(&str, usize)] = &[
    ("image/png", IMAGE_BYTES),
    ("image/jpeg", IMAGE_BYTES),
    ("image/gif", IMAGE_BYTES),
    ("image/webp", IMAGE_BYTES),
    ("image/svg+xml", IMAGE_BYTES),
    ("video/mp4", VIDEO_BYTES),
    ("video/quicktime", VIDEO_BYTES),
    ("video/webm", VIDEO_BYTES),
];

/// The largest a file of this kind may be, or `None` for a kind that cannot
/// be attached
pub fn attachable(kind: &str) -> Option<usize> {
    ATTACHABLE.iter().find(|(k, _)| *k == kind).map(|(_, most)| *most)
}

/// Where the pieces of a file being put on GitHub gather until the last one
/// is in: a page sends it a piece at a time, so a phone's request stays small
fn upload_place(id: &str) -> Option<std::path::PathBuf> {
    let ok = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| std::env::temp_dir().join("shikisha-issue-files").join(id))
}

/// Write one piece of a file being put on GitHub, at its place in the file
fn upload_piece(id: &str, at: u64, data: &str) -> Result<()> {
    use base64::Engine;
    use std::io::{Seek, SeekFrom, Write};
    let place = upload_place(id).ok_or_else(|| anyhow!("bad upload id"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| anyhow!("{e}"))?;
    if at as usize + bytes.len() > VIDEO_BYTES {
        bail!(crate::i18n::t("err.github.upload_size"));
    }
    if let Some(dir) = place.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&place)?;
    f.seek(SeekFrom::Start(at))?;
    f.write_all(&bytes)?;
    Ok(())
}

/// The longest a press that makes one thing on GitHub -- an issue, a pull
/// request -- can take before it is answered either way: the account's
/// sign-in read at its slowest, then the one request. A request that reached
/// GitHub is never sent again (a second would make a second issue), so this
/// is the whole of it. What the screen's bar fills toward
pub fn longest_make() -> Duration {
    crate::pr::slowest_sign_in() + REQUEST_LIMIT
}

impl Hub {
    pub fn new(token: String) -> Hub {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_LIMIT))
            // The answer to a refusal says why; read it rather than turning it
            // into a status code nobody can act on
            .http_status_as_error(false)
            .build()
            .new_agent();
        Hub { agent, token }
    }

    /// One request. The reply's JSON, or GitHub's own reason for saying no
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        match self.call_unless_absent(method, path, body)? {
            Some(v) => Ok(v),
            // Missing, to an account that is asked by name: the repository is
            // not there, or this account cannot see it
            None => {
                let (words, account) = refusal(404, "Not Found");
                if account {
                    bail!(AccountTrouble(words))
                }
                bail!(words)
            }
        }
    }

    /// One request, where "there is nothing there" is an answer rather than a
    /// failure: `None` when GitHub says it has no such thing
    fn call_unless_absent(&self, method: &str, path: &str, body: Option<Value>) -> Result<Option<Value>> {
        let url = format!("https://api.github.com/{}", path.trim_start_matches('/'));
        let auth = format!("Bearer {}", self.token);
        let ua = concat!("shikisha-term/", env!("CARGO_PKG_VERSION"));
        let mut resp = match (method, body) {
            ("GET", _) => self
                .agent
                .get(&url)
                .header("Authorization", &auth)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", API_VERSION)
                .header("User-Agent", ua)
                .call(),
            (m, body) => {
                let req = match m {
                    "POST" => self.agent.post(&url),
                    "PATCH" => self.agent.patch(&url),
                    "PUT" => self.agent.put(&url),
                    other => bail!("unsupported method {other}"),
                };
                req.header("Authorization", &auth)
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", API_VERSION)
                    .header("User-Agent", ua)
                    .send_json(body.unwrap_or_else(|| json!({})))
            }
        }
        .map_err(|e| {
            anyhow!(crate::i18n::tp(
                "err.github.unreachable",
                &[("error", &e.to_string())]
            ))
        })?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        note_allowance(&self.token, status, &|k| {
            headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string)
        });
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return Ok(Some(v));
        }
        if status == 404 {
            return Ok(None);
        }
        // GitHub's reason is its message and, for a refused search, the reasons
        // listed under it -- "cannot be searched ... permission" is the useful half
        let mut said = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        for e in v
            .get("errors")
            .and_then(|e| e.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(m) = e.get("message").and_then(|m| m.as_str()) {
                said.push(' ');
                said.push_str(m);
            }
        }
        let (words, account) = refusal(status, &said);
        if account {
            bail!(AccountTrouble(words))
        }
        bail!(words)
    }

    fn search(&self, repo: &Repo, pulls: bool, q: &Query) -> Result<Value> {
        let query = search_q(repo, pulls, q);
        let path = format!(
            "search/issues?q={}&sort=created&order=desc&per_page={PAGE}&page={}",
            encode(&query),
            q.page.max(1)
        );
        let v = self.call("GET", &path, None)?;
        let items: Vec<Value> = v
            .get("items")
            .and_then(|i| i.as_array())
            .into_iter()
            .flatten()
            .filter(|i| i.get("pull_request").is_some() == pulls)
            .map(|i| row(i, pulls))
            .collect();
        Ok(json!({
            "repo": repo.slug(),
            "total": v.get("total_count").and_then(|t| t.as_u64()).unwrap_or(0),
            "page": q.page.max(1),
            "per_page": PAGE,
            "items": items,
        }))
    }

    /// One page of a repository's issues
    pub fn issues(&self, repo: &Repo, q: &Query) -> Result<Value> {
        self.search(repo, false, q)
    }

    /// One page of a repository's pull requests
    pub fn pulls(&self, repo: &Repo, q: &Query) -> Result<Value> {
        self.search(repo, true, q)
    }

    /// An issue in full: what it says, who is on it, and what has happened to it
    pub fn issue(&self, repo: &Repo, number: u64) -> Result<Value> {
        let base = format!("repos/{}/issues/{number}", repo.slug());
        let item = self.call("GET", &base, None)?;
        let mut out = row(&item, item.get("pull_request").is_some());
        out["body"] = item.get("body").cloned().unwrap_or(Value::Null);
        out["assignees"] = logins(item.get("assignees"));
        out["created"] = item.get("created_at").cloned().unwrap_or(Value::Null);
        out["milestone"] = item.pointer("/milestone/title").cloned().unwrap_or(Value::Null);
        out["type"] = item.pointer("/type/name").cloned().unwrap_or(Value::Null);
        // The issue above it and those under it are worth having and not
        // worth failing over, the same as what happened to it
        if item.get("pull_request").is_none()
            && let Ok(f) = self.family(repo, number)
        {
            out["node"] = f["node"].clone();
            out["parent"] = f["parent"].clone();
            out["subs"] = f["subs"].clone();
        }
        out["comments"] = self.comments(repo, number)?;
        // What happened to it is worth having and not worth failing over: a
        // repository can refuse the timeline and still show the issue
        out["events"] = self.events(repo, number).unwrap_or_else(|_| json!([]));
        Ok(out)
    }

    fn comments(&self, repo: &Repo, number: u64) -> Result<Value> {
        let v = self.call(
            "GET",
            &format!(
                "repos/{}/issues/{number}/comments?per_page=100",
                repo.slug()
            ),
            None,
        )?;
        Ok(Value::Array(
            v.as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    json!({
                        "id": c.get("id"),
                        "author": c.pointer("/user/login"),
                        "bot": c.pointer("/user/type").and_then(|t| t.as_str()) == Some("Bot"),
                        "body": c.get("body"),
                        "created": c.get("created_at"),
                        "url": c.get("html_url"),
                    })
                })
                .collect(),
        ))
    }

    fn events(&self, repo: &Repo, number: u64) -> Result<Value> {
        let v = self.call(
            "GET",
            &format!(
                "repos/{}/issues/{number}/timeline?per_page=100",
                repo.slug()
            ),
            None,
        )?;
        const KEPT: [&str; 7] = [
            "assigned",
            "unassigned",
            "labeled",
            "unlabeled",
            "closed",
            "reopened",
            "cross-referenced",
        ];
        Ok(Value::Array(
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    let kind = e.get("event").and_then(|k| k.as_str())?;
                    KEPT.contains(&kind).then(|| {
                        json!({
                            "kind": kind,
                            "actor": e.pointer("/actor/login"),
                            "subject": e.pointer("/assignee/login").or_else(|| e.pointer("/label/name"))
                                .or_else(|| e.pointer("/source/issue/number")),
                            "reason": e.get("state_reason"),
                            "created": e.get("created_at"),
                        })
                    })
                })
                .collect(),
        ))
    }

    /// A pull request in full: its branches, whether it can go in, what the
    /// reviewers said, and how its checks stand
    pub fn pull(&self, repo: &Repo, number: u64) -> Result<Value> {
        let pr = self.call(
            "GET",
            &format!("repos/{}/pulls/{number}", repo.slug()),
            None,
        )?;
        let mut out = self.issue(repo, number)?;
        out["draft"] = pr.get("draft").cloned().unwrap_or(json!(false));
        out["merged"] = json!(pr.get("merged_at").is_some_and(|m| !m.is_null()));
        if out["merged"] == json!(true) {
            out["state"] = json!("merged");
        }
        out["head"] = pr.pointer("/head/ref").cloned().unwrap_or(Value::Null);
        out["head_repo"] = pr
            .pointer("/head/repo/full_name")
            .cloned()
            .unwrap_or(Value::Null);
        out["base"] = pr.pointer("/base/ref").cloned().unwrap_or(Value::Null);
        out["fork"] =
            json!(pr.pointer("/head/repo/full_name") != pr.pointer("/base/repo/full_name"));
        out["mergeable"] = pr.get("mergeable").cloned().unwrap_or(Value::Null);
        out["merge_state"] = pr.get("mergeable_state").cloned().unwrap_or(Value::Null);
        out["additions"] = pr.get("additions").cloned().unwrap_or(Value::Null);
        out["deletions"] = pr.get("deletions").cloned().unwrap_or(Value::Null);
        out["changed_files"] = pr.get("changed_files").cloned().unwrap_or(Value::Null);
        out["reviewers"] = logins(pr.get("requested_reviewers"));
        out["review"] = json!(self.review_decision(repo, number).unwrap_or_default());
        if let Some(sha) = pr.pointer("/head/sha").and_then(|s| s.as_str()) {
            out["checks"] = self.checks(repo, sha).unwrap_or(Value::Null);
        }
        Ok(out)
    }

    /// Every pull request made from `head` in this repository, newest first: one
    /// a base, as a branch can be sent to more than one. An open one says whether
    /// GitHub can merge it -- asked of each, since the list does not say
    pub fn branch_pulls(&self, repo: &Repo, head: &str) -> Result<Value> {
        let head = head.trim();
        if head.is_empty() {
            return Ok(json!([]));
        }
        let v = self.call(
            "GET",
            &format!("repos/{}/pulls?head={}:{}&state=all&per_page=20", repo.slug(), repo.owner, head),
            None,
        )?;
        let mut out = Vec::new();
        for p in v.as_array().into_iter().flatten() {
            let Some(number) = p.get("number").and_then(|n| n.as_u64()) else { continue };
            let merged = p.get("merged_at").is_some_and(|m| !m.is_null());
            let open = p.get("state").and_then(|s| s.as_str()) == Some("open");
            let merge_state = match open {
                true => self
                    .call("GET", &format!("repos/{}/pulls/{number}", repo.slug()), None)
                    .ok()
                    .and_then(|full| full.get("mergeable_state").cloned())
                    .unwrap_or(Value::Null),
                false => Value::Null,
            };
            out.push(json!({
                "number": number,
                "title": p.get("title"),
                "base": p.pointer("/base/ref"),
                "state": if merged { "merged" } else if open { "open" } else { "closed" },
                "draft": p.get("draft").cloned().unwrap_or(json!(false)),
                "sha": p.pointer("/head/sha"),
                "merge_state": merge_state,
                "url": p.get("html_url"),
            }));
        }
        Ok(Value::Array(out))
    }

    /// Each reviewer's latest word: changes asked for outweighs an approval
    fn review_decision(&self, repo: &Repo, number: u64) -> Result<String> {
        let v = self.call(
            "GET",
            &format!("repos/{}/pulls/{number}/reviews?per_page=100", repo.slug()),
            None,
        )?;
        let mut latest: Vec<(String, String)> = Vec::new();
        for r in v.as_array().into_iter().flatten() {
            let (Some(who), Some(state)) = (
                r.pointer("/user/login").and_then(|u| u.as_str()),
                r.get("state").and_then(|s| s.as_str()),
            ) else {
                continue;
            };
            if state == "COMMENTED" || state == "PENDING" {
                continue;
            }
            latest.retain(|(w, _)| w != who);
            latest.push((who.to_string(), state.to_string()));
        }
        Ok(match () {
            _ if latest.iter().any(|(_, s)| s == "CHANGES_REQUESTED") => "changes_requested".into(),
            _ if latest.iter().any(|(_, s)| s == "APPROVED") => "approved".into(),
            _ => String::new(),
        })
    }

    /// How the checks on a commit stand, counted
    fn checks(&self, repo: &Repo, sha: &str) -> Result<Value> {
        let runs = self.call(
            "GET",
            &format!(
                "repos/{}/commits/{sha}/check-runs?per_page=100",
                repo.slug()
            ),
            None,
        )?;
        let status = self
            .call(
                "GET",
                &format!("repos/{}/commits/{sha}/status", repo.slug()),
                None,
            )
            .unwrap_or(Value::Null);
        let mut items: Vec<Value> = Vec::new();
        let (mut failed, mut pending, mut passed) = (0, 0, 0);
        for r in runs
            .get("check_runs")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            let done = r.get("status").and_then(|s| s.as_str()) == Some("completed");
            let conclusion = r
                .get("conclusion")
                .and_then(|s| s.as_str())
                .unwrap_or_default();
            let verdict = check_verdict(done, conclusion);
            match verdict {
                "failed" => failed += 1,
                "pending" => pending += 1,
                "passed" => passed += 1,
                _ => {}
            }
            // A job GitHub Actions ran has a log that can be read; its check is the job
            let job = match r.pointer("/app/slug").and_then(|s| s.as_str()) {
                Some("github-actions") => r.get("id").cloned().unwrap_or(Value::Null),
                _ => Value::Null,
            };
            items.push(json!({"name": r.get("name"), "verdict": verdict, "url": r.get("html_url"), "job": job}));
        }
        for s in status
            .get("statuses")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            let verdict = status_verdict(s.get("state").and_then(|x| x.as_str()).unwrap_or_default());
            match verdict {
                "failed" => failed += 1,
                "pending" => pending += 1,
                _ => passed += 1,
            }
            items.push(
                json!({"name": s.get("context"), "verdict": verdict, "url": s.get("target_url")}),
            );
        }
        Ok(
            json!({"failed": failed, "pending": pending, "passed": passed, "total": items.len(), "items": items}),
        )
    }

    /// The end of a GitHub Actions job's log: where a failure says what failed
    pub fn job_log_tail(&self, repo: &Repo, job: u64) -> Result<String> {
        Ok(log_tail(&self.job_log(repo, job)?.text))
    }

    /// A GitHub Actions job's log, as GitHub keeps it -- its end, when it is
    /// bigger than an editor opens ([`crate::files::READ_LIMIT`]): a job fails
    /// at its end, and the start of a log without end is let go as it arrives
    /// rather than held whole first. GitHub answers with a redirect to the
    /// file, which is followed. A refusal is said the way every other one is:
    /// an expired log, a spent allowance and a missing sign-in each have
    /// their own words
    pub fn job_log(&self, repo: &Repo, job: u64) -> Result<crate::readview::Tail> {
        let url = format!("https://api.github.com/repos/{}/actions/jobs/{job}/logs", repo.slug());
        let mut resp = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("User-Agent", concat!("shikisha-term/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|e| anyhow!(crate::i18n::tp("err.github.unreachable", &[("error", &e.to_string())])))?;
        let status = resp.status().as_u16();
        // Not found, from a repository whose checks were just read with the
        // same account, is a job with no log yet: still running, or never
        // started. Said as that, not as a repository out of reach
        if status == 404 {
            bail!(crate::i18n::t("err.github.no_log"));
        }
        if !(200..300).contains(&status) {
            // A refusal is a sentence or two; more than this is not one
            let said = crate::readview::read_tail(resp.body_mut().as_reader(), REFUSAL_MOST).unwrap_or_default();
            bail!(refusal(status, said.text.trim()).0);
        }
        crate::readview::read_tail(resp.body_mut().as_reader(), crate::files::READ_LIMIT as usize)
            .map_err(|e| anyhow!(crate::i18n::tp("err.github.unreachable", &[("error", &e.to_string())])))
    }

    /// Open a new issue. Answers with its number and address
    pub fn create_issue(
        &self,
        repo: &Repo,
        title: &str,
        body: &str,
        labels: &[String],
        assignees: &[String],
    ) -> Result<Value> {
        let title = title.trim();
        if title.is_empty() {
            bail!(crate::i18n::t("err.github.no_title"));
        }
        let mut payload = json!({"title": title, "body": body});
        if !labels.is_empty() {
            payload["labels"] = json!(labels);
        }
        if !assignees.is_empty() {
            payload["assignees"] = json!(assignees);
        }
        let v = self.call(
            "POST",
            &format!("repos/{}/issues", repo.slug()),
            Some(payload),
        )?;
        Ok(json!({"number": v.get("number"), "url": v.get("html_url")}))
    }

    /// An issue or a pull request changed in place: any of its `title`, its
    /// `body`, who it is assigned to (`assignees`, all of them by name), its
    /// `labels` (all of them by name), its `milestone` (by title; null or
    /// empty for none) and -- an issue's -- its `type` (by name; null or
    /// empty for none: Lua has no null to put in a table). Only what
    /// `change` names is touched. Answers with what it then has.
    ///
    /// People and labels can also be said as what to add and what to take
    /// off -- `add_assignees`, `remove_assignees`, `add_labels`,
    /// `remove_labels` -- which is how a screen changes them: it may not
    /// have every label an issue carries, and a whole list from it would
    /// take the rest off. Those are worked out against what the issue has
    /// at the moment, read here
    pub fn edit_issue(&self, repo: &Repo, number: u64, change: &Value) -> Result<Value> {
        let mut payload = serde_json::Map::new();
        // A person may be written with an @; a label's name is as it is
        let names = |v: &Value, person: bool| -> Vec<String> {
            v.as_array().into_iter().flatten().filter_map(|x| x.as_str())
                .map(|s| if person { s.trim().trim_start_matches('@').to_string() } else { s.trim().to_string() })
                .filter(|s| !s.is_empty()).collect()
        };
        let path = format!("repos/{}/issues/{number}", repo.slug());
        // One at a time: a name put on or taken off is worked out against the
        // list the issue has, and two changes at once -- two screens, a screen
        // and a script -- would each start from the same list, the later
        // undoing the earlier
        static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let mut now: Option<Value> = None;
        for (key, person) in [("assignees", true), ("labels", false)] {
            let (add, remove) = (change.get(format!("add_{key}")), change.get(format!("remove_{key}")));
            if add.is_none() && remove.is_none() {
                continue;
            }
            if now.is_none() {
                now = Some(self.call("GET", &path, None)?);
            }
            let have = now.as_ref().and_then(|i| i.get(key)).and_then(|l| l.as_array()).cloned().unwrap_or_default();
            let mut list: Vec<String> = have.iter()
                .filter_map(|x| x.get(if person { "login" } else { "name" }).and_then(|n| n.as_str()).map(str::to_string)).collect();
            let same = |a: &str, b: &str| if person { a.eq_ignore_ascii_case(b) } else { a == b };
            for gone in remove.map(|r| names(r, person)).unwrap_or_default() {
                list.retain(|x| !same(x, &gone));
            }
            for more in add.map(|a| names(a, person)).unwrap_or_default() {
                if !list.iter().any(|x| same(x, &more)) {
                    list.push(more);
                }
            }
            payload.insert(key.into(), json!(list));
        }
        if let Some(t) = change.get("title").and_then(|t| t.as_str()) {
            if t.trim().is_empty() {
                bail!(crate::i18n::t("err.github.no_title"));
            }
            payload.insert("title".into(), json!(t.trim()));
        }
        if let Some(b) = change.get("body").and_then(|b| b.as_str()) {
            payload.insert("body".into(), json!(b));
        }
        for (key, person) in [("assignees", true), ("labels", false)] {
            if let Some(list) = change.get(key).filter(|l| l.is_array()) {
                payload.insert(key.into(), json!(names(list, person)));
            }
        }
        let none = |v: &Value| v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty());
        match change.get("milestone") {
            Some(v) if none(v) => {
                payload.insert("milestone".into(), Value::Null);
            }
            Some(Value::String(title)) => {
                let all = self.milestones(repo)?;
                let found = all.iter().find(|m| m["title"].as_str() == Some(title.trim()));
                let Some(m) = found else {
                    bail!(crate::i18n::tp("err.github.no_milestone", &[("title", title)]));
                };
                payload.insert("milestone".into(), m["number"].clone());
            }
            _ => {}
        }
        match change.get("type") {
            Some(v) if none(v) => {
                payload.insert("type".into(), Value::Null);
            }
            Some(Value::String(name)) => {
                payload.insert("type".into(), json!(name.trim()));
            }
            _ => {}
        }
        if payload.is_empty() {
            bail!(crate::i18n::t("err.github.no_change"));
        }
        let v = self.call("PATCH", &path, Some(Value::Object(payload)))?;
        Ok(json!({
            "number": v.get("number"), "title": v.get("title"),
            "assignees": logins(v.get("assignees")),
            "labels": v.get("labels").and_then(|l| l.as_array()).map(|l| l.iter().filter_map(|x| x.get("name").cloned()).collect::<Vec<_>>()).unwrap_or_default(),
            "milestone": v.pointer("/milestone/title"), "type": v.pointer("/type/name"),
        }))
    }

    /// The milestones a repository's issues can be given, the open ones:
    /// `[{number, title}]`
    pub fn milestones(&self, repo: &Repo) -> Result<Vec<Value>> {
        let v = self.call("GET", &format!("repos/{}/milestones?state=open&per_page=100", repo.slug()), None)?;
        Ok(v.as_array().into_iter().flatten().map(|m| json!({"number": m.get("number"), "title": m.get("title")})).collect())
    }

    /// The types an issue of the repository can be (Bug, Feature, ...), by
    /// name. A repository with none -- a person's own -- has an empty list
    pub fn issue_types(&self, repo: &Repo) -> Result<Vec<String>> {
        let v = self.graphql_whole(
            "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){issueTypes(first:50){nodes{name isEnabled}}}}",
            json!({"owner": repo.owner, "name": repo.name}),
        )?;
        Ok(v.pointer("/data/repository/issueTypes/nodes")
            .and_then(|n| n.as_array())
            .into_iter()
            .flatten()
            .filter(|t| t["isEnabled"].as_bool().unwrap_or(true))
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect())
    }

    /// The issue an issue is part of and the ones that are part of it, in
    /// their order: `{node, parent, subs}`, each of them `{id, number, title,
    /// state, url, repo}`. `node` and each `id` are what the changes below
    /// name an issue by
    pub fn family(&self, repo: &Repo, number: u64) -> Result<Value> {
        const ONE: &str = "id number title state url repository{nameWithOwner}";
        let v = self.graphql(
            &format!(
                "query($owner:String!,$name:String!,$number:Int!){{repository(owner:$owner,name:$name){{issue(number:$number){{id parent{{{ONE}}} subIssues(first:100){{nodes{{{ONE}}}}}}}}}}}"
            ),
            json!({"owner": repo.owner, "name": repo.name, "number": number}),
        )?;
        let issue = v.pointer("/data/repository/issue").filter(|i| !i.is_null()).ok_or_else(|| anyhow!(crate::i18n::t("err.github.number")))?;
        let one = |i: &Value| {
            json!({"id": i["id"], "number": i["number"], "title": i["title"], "url": i["url"],
                "state": if i["state"] == "OPEN" { "open" } else { "closed" }, "repo": i.pointer("/repository/nameWithOwner")})
        };
        Ok(json!({
            "node": issue["id"],
            "parent": issue.get("parent").filter(|p| p.is_object()).map(one),
            "subs": issue.pointer("/subIssues/nodes").and_then(|n| n.as_array()).into_iter().flatten().map(one).collect::<Vec<_>>(),
        }))
    }

    /// The id GitHub knows an issue by
    fn issue_node(&self, repo: &Repo, number: u64) -> Result<String> {
        let v = self.graphql(
            "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){issue(number:$number){id}}}",
            json!({"owner": repo.owner, "name": repo.name, "number": number}),
        )?;
        v.pointer("/data/repository/issue/id")
            .and_then(|i| i.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!(crate::i18n::t("err.github.number")))
    }

    /// GitHub's answer to a change of an issue's sub-issues, or why it refused
    fn sub_change(&self, query: &str, vars: Value, at: &str) -> Result<()> {
        let v = self.graphql_whole(query, vars)?;
        match v.pointer(at) {
            Some(x) if !x.is_null() => Ok(()),
            _ => bail!("{}", v.pointer("/errors/0/message").and_then(|m| m.as_str()).unwrap_or("GitHub did not answer")),
        }
    }

    /// An issue made part of another, as its sub-issue. `sub` is the issue's
    /// page, `owner/repo#n`, or `#n` / `n` for one of the same repository.
    /// One that is already part of another issue is moved only when `replace`
    pub fn sub_add(&self, repo: &Repo, number: u64, sub: &str, replace: bool) -> Result<()> {
        let sub = sub.trim();
        let same = sub.trim_start_matches('#');
        let url = if !same.is_empty() && same.chars().all(|c| c.is_ascii_digit()) {
            format!("https://github.com/{}/issues/{same}", repo.slug())
        } else {
            crate::ghproject::content_url(sub).ok_or_else(|| anyhow!(crate::i18n::t("err.ghproject.bad_content")))?
        };
        self.sub_change(
            "mutation($issue:ID!,$url:String!,$replace:Boolean){addSubIssue(input:{issueId:$issue,subIssueUrl:$url,replaceParent:$replace}){issue{id}}}",
            json!({"issue": self.issue_node(repo, number)?, "url": url, "replace": replace}),
            "/data/addSubIssue/issue/id",
        )
    }

    /// A sub-issue no longer part of the issue. `sub` is its `id`, as
    /// [`Hub::family`] answers it
    pub fn sub_remove(&self, repo: &Repo, number: u64, sub: &str) -> Result<()> {
        self.sub_change(
            "mutation($issue:ID!,$sub:ID!){removeSubIssue(input:{issueId:$issue,subIssueId:$sub}){issue{id}}}",
            json!({"issue": self.issue_node(repo, number)?, "sub": sub}),
            "/data/removeSubIssue/issue/id",
        )
    }

    /// A sub-issue put right after another of the same issue (`after`, its
    /// id), or first of them for None
    pub fn sub_move(&self, repo: &Repo, number: u64, sub: &str, after: Option<&str>) -> Result<()> {
        let issue = self.issue_node(repo, number)?;
        match after {
            Some(after) => self.sub_change(
                "mutation($issue:ID!,$sub:ID!,$after:ID!){reprioritizeSubIssue(input:{issueId:$issue,subIssueId:$sub,afterId:$after}){issue{id}}}",
                json!({"issue": issue, "sub": sub, "after": after}),
                "/data/reprioritizeSubIssue/issue/id",
            ),
            // First: before the one that is first now
            None => {
                let first = self.family(repo, number)?["subs"].as_array().and_then(|s| s.first()).and_then(|s| s["id"].as_str().map(str::to_string));
                match first.filter(|f| f != sub) {
                    Some(first) => self.sub_change(
                        "mutation($issue:ID!,$sub:ID!,$before:ID!){reprioritizeSubIssue(input:{issueId:$issue,subIssueId:$sub,beforeId:$before}){issue{id}}}",
                        json!({"issue": issue, "sub": sub, "before": first}),
                        "/data/reprioritizeSubIssue/issue/id",
                    ),
                    None => Ok(()),
                }
            }
        }
    }

    /// Open a pull request from `head` into `base`, as a draft when asked.
    /// Answers with its number and address
    pub fn create_pull(
        &self,
        repo: &Repo,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        draft: bool,
    ) -> Result<Value> {
        let title = title.trim();
        if title.is_empty() {
            bail!(crate::i18n::t("err.github.no_title"));
        }
        if head.trim().is_empty() || base.trim().is_empty() {
            bail!(crate::i18n::t("err.github.no_branches"));
        }
        let v = self.call(
            "POST",
            &format!("repos/{}/pulls", repo.slug()),
            Some(json!({"title": title, "body": body, "head": head.trim(), "base": base.trim(), "draft": draft})),
        )?;
        Ok(json!({"number": v.get("number"), "url": v.get("html_url"), "head": head.trim()}))
    }

    /// Say something on an issue or a pull request
    pub fn comment(&self, repo: &Repo, number: u64, body: &str) -> Result<Value> {
        if body.trim().is_empty() {
            bail!(crate::i18n::t("err.github.no_comment"));
        }
        let v = self.call(
            "POST",
            &format!("repos/{}/issues/{number}/comments", repo.slug()),
            Some(json!({"body": body})),
        )?;
        Ok(json!({"id": v.get("id"), "url": v.get("html_url")}))
    }

    /// Open an issue again, or close it as done, as not going to be done, or as
    /// a duplicate of another. A duplicate is said the way GitHub itself reads
    /// it -- a comment naming the other -- and then closed as one
    pub fn set_issue_state(
        &self,
        repo: &Repo,
        number: u64,
        state: &str,
        duplicate_of: Option<u64>,
    ) -> Result<()> {
        let path = format!("repos/{}/issues/{number}", repo.slug());
        let body = match state {
            "open" => json!({"state": "open"}),
            "completed" => json!({"state": "closed", "state_reason": "completed"}),
            "not_planned" => json!({"state": "closed", "state_reason": "not_planned"}),
            "duplicate" => {
                let Some(other) = duplicate_of.filter(|o| *o != number && *o > 0) else {
                    bail!(crate::i18n::t("err.github.duplicate_of"));
                };
                self.comment(repo, number, &format!("Duplicate of #{other}"))?;
                json!({"state": "closed", "state_reason": "duplicate"})
            }
            other => bail!(crate::i18n::tp("err.github.state", &[("state", other)])),
        };
        self.call("PATCH", &path, Some(body)).map(|_| ())
    }

    /// Open or close a pull request without merging it
    pub fn set_pull_state(&self, repo: &Repo, number: u64, open: bool) -> Result<()> {
        let state = if open { "open" } else { "closed" };
        self.call(
            "PATCH",
            &format!("repos/{}/pulls/{number}", repo.slug()),
            Some(json!({"state": state})),
        )
        .map(|_| ())
    }

    /// Merge a pull request: `squash`, `merge` or `rebase`. The branch is left
    /// where it is -- throwing it away is a separate decision
    pub fn merge_pull(&self, repo: &Repo, number: u64, method: &str) -> Result<Value> {
        let method = match method.trim() {
            "" | "squash" => "squash",
            "merge" => "merge",
            "rebase" => "rebase",
            other => bail!(crate::i18n::tp(
                "err.github.merge_method",
                &[("method", other)]
            )),
        };
        let v = self.call(
            "PUT",
            &format!("repos/{}/pulls/{number}/merge", repo.slug()),
            Some(json!({"merge_method": method})),
        )?;
        Ok(json!({"merged": v.get("merged"), "sha": v.get("sha")}))
    }

    /// The labels an issue can be given: all of them, a page at a time, so a
    /// template's label is found among a repository's hundred-and-first. Ten
    /// pages are a thousand labels, more than any repository has a use for
    pub fn labels(&self, repo: &Repo) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for page in 1..=10 {
            let v = self.call(
                "GET",
                &format!("repos/{}/labels?per_page=100&page={page}", repo.slug()),
                None,
            )?;
            let got: Vec<String> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l.get("name")?.as_str().map(str::to_string))
                .collect();
            let full = got.len() == 100;
            out.extend(got);
            if !full {
                break;
            }
        }
        Ok(out)
    }

    /// Put an image or a video on GitHub for an issue or a comment to show,
    /// the way pasting one into GitHub's own page does, and answer with its
    /// address. Sent where GitHub's own command line tool sends the files it
    /// attaches; an account that cannot write to the repository is refused
    pub fn upload_asset(&self, repo: &Repo, name: &str, kind: &str, bytes: &[u8]) -> Result<String> {
        let id = self
            .call("GET", &format!("repos/{}", repo.slug()), None)?
            .get("id")
            .and_then(|i| i.as_u64())
            .ok_or_else(|| anyhow!(crate::i18n::t("err.github.404")))?;
        let url = format!(
            "https://uploads.github.com/user-attachments/assets?name={}&content_type={}&repository_id={id}",
            encode(name),
            encode(kind)
        );
        // A file of a hundred megabytes over a slow line is not twenty seconds
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(UPLOAD_LIMIT))
            .http_status_as_error(false)
            .build()
            .new_agent();
        let mut resp = agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("Content-Type", "application/octet-stream")
            .header("User-Agent", concat!("shikisha-term/", env!("CARGO_PKG_VERSION")))
            .send(bytes)
            .map_err(|e| anyhow!(crate::i18n::tp("err.github.unreachable", &[("error", &e.to_string())])))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        match status {
            200..=299 => v
                .get("url")
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
                .map(str::to_string)
                .ok_or_else(|| anyhow!(crate::i18n::t("err.github.upload_no_url"))),
            // Refused for a lack of write access, said as a missing thing
            404 => bail!(crate::i18n::t("err.github.upload_write")),
            _ => {
                let said = v.get("message").and_then(|m| m.as_str()).unwrap_or_default();
                bail!(refusal(status, said).0)
            }
        }
    }

    /// The issue templates GitHub offers on the repository's own "New issue"
    /// page: its own, or when it has none, the ones its owner keeps for every
    /// repository in a repository named `.github`. Read from the default
    /// branch, which is the one GitHub's page reads
    pub fn issue_templates(&self, repo: &Repo) -> Result<crate::issueform::Templates> {
        let shared = Repo { owner: repo.owner.clone(), name: ".github".into() };
        let places = [
            (repo, crate::issueform::FOLDER),
            (&shared, crate::issueform::FOLDER),
            (&shared, "ISSUE_TEMPLATE"),
        ];
        for (at, folder) in places {
            // The repository itself is asked as any other request is; the
            // owner's shared one may simply not be there, or not be readable
            // by this account, and either means it has nothing to offer
            let listing = match self.call_unless_absent("GET", &format!("repos/{}/contents/{folder}", at.slug()), None) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) if std::ptr::eq(at, repo) => return Err(e),
                Err(_) => continue,
            };
            let mut files = Vec::new();
            for entry in listing.as_array().into_iter().flatten() {
                let name = entry.get("name").and_then(|n| n.as_str()).unwrap_or_default();
                let lower = name.to_lowercase();
                let wanted = entry.get("type").and_then(|t| t.as_str()) == Some("file")
                    && [".yml", ".yaml", ".md"].iter().any(|ext| lower.ends_with(ext));
                // A folder of a hundred files is not a folder of templates
                if !wanted || files.len() >= 50 {
                    continue;
                }
                let path = entry.get("path").and_then(|p| p.as_str()).unwrap_or(name);
                let file = self.call("GET", &format!("repos/{}/contents/{}", at.slug(), path_in_url(path)), None)?;
                files.push((name.to_string(), file_text(&file)?));
            }
            if !files.is_empty() {
                return Ok(crate::issueform::read_folder(&files));
            }
        }
        Ok(crate::issueform::Templates { blank: true, ..Default::default() })
    }

    /// The people an issue can be assigned to
    pub fn assignees(&self, repo: &Repo) -> Result<Vec<String>> {
        let v = self.call(
            "GET",
            &format!("repos/{}/assignees?per_page=100", repo.slug()),
            None,
        )?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|u| u.get("login")?.as_str().map(str::to_string))
            .collect())
    }
}

/// One repository the Issue tab shows: a project of the desk, where its
/// checkout is, and the account it chose.
#[derive(Debug, Clone)]
pub struct Source {
    /// The project's name, or its checkout's folder name when none is written
    pub name: String,
    /// The checkout the project's worktrees are cut from
    pub dir: std::path::PathBuf,
    /// The desk folder this project was found through, spelled the way the
    /// settings spell it.
    ///
    /// Not the same string as `dir`, and the difference is the whole point. A
    /// checkout is worked out by asking the disk, which answers with the place
    /// itself: a project opened as `P:\web\shop` comes back as
    /// `\\nas\projects\web\shop`, because that is where the
    /// mapped drive goes. That answer is right for running git and wrong for
    /// pointing at a line of the settings, which holds what the person typed --
    /// so the Issue tab's "open the settings" button led nowhere, and the
    /// branch dialog could not tell which project it was standing in
    pub at: std::path::PathBuf,
    /// `owner/name` on GitHub, when that is where it lives
    pub repo: Option<String>,
    pub git: crate::config::GitUse,
    /// For a project that is on another machine and not here: its folders
    /// there, each with its own machine (every worktree on a MicroVM is a
    /// machine of its own). Git about any of them runs there
    pub far: Vec<(std::path::PathBuf, crate::elsewhere::Elsewhere, String)>,
}

impl Source {
    /// Note, on this thread, which machine the project's folder `dir` is on,
    /// so the git asked about it next runs there. Nothing for a project here
    pub fn note_far(&self, dir: &std::path::Path) {
        let at = self
            .far
            .iter()
            .find(|(f, _, _)| crate::uistate::same_folder(f, dir))
            .map(|(_, at, _)| at);
        crate::git::there(dir, at);
    }

    /// The project's folder a place key names (see `uistate::place_key`):
    /// its path, and the machine it is on. A key naming a machine is only
    /// that machine's folder; a bare path is the first folder of that path
    pub fn far_at(&self, key: &std::path::Path) -> Option<(std::path::PathBuf, &crate::elsewhere::Elsewhere)> {
        let (on, path) = crate::uistate::place_of(key);
        self.far
            .iter()
            .find(|(f, _, name)| crate::uistate::same_folder(f, &path) && on.as_deref().is_none_or(|h| h.trim() == name.trim()))
            .map(|(f, at, _)| (f.clone(), at))
    }
}

/// The GitHub repository a project is, and a token for it.
///
/// A project on another machine: the repository as git there last said it
/// (`far_origin`, asked once and kept). Asked through its first folder only
/// until that answer is in -- every folder on a MicroVM is a machine of its
/// own, and asking the first on every call to GitHub woke the checkout's
/// machine for work done in another, and kept it from ever pausing
pub fn target_of(
    s: &Source,
    look: &dyn Fn(&str) -> Option<String>,
) -> Result<(Repo, String)> {
    let known = s.repo.clone().filter(|_| !s.far.is_empty());
    if known.is_none() {
        s.note_far(&s.dir);
    }
    target_with(known, &s.dir, &s.git, look)
}

/// The desk's repositories, one each: every folder on this machine, gathered
/// by the checkout it belongs to, named the way the settings name its project,
/// and every project whose folders are only on another machine
pub fn desk_sources(desk: &crate::config::Desk) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    // Which of them are named for their checkout's folder rather than by a
    // project written down: the names that can be another's (see the end)
    let mut guessed: Vec<usize> = Vec::new();
    for f in desk.folders.iter().filter(|f| f.host.is_none()) {
        let Some(cwd) = f.cwd.as_deref() else {
            continue;
        };
        let Some(main) = crate::repo::main_checkout(cwd) else {
            continue;
        };
        let Some(family) = crate::repo::family_of(cwd) else {
            continue;
        };
        if seen.contains(&family) {
            continue;
        }
        seen.push(family);
        let (git, project) = desk.git_use_of_folder(cwd);
        if project.is_none() {
            guessed.push(out.len());
        }
        out.push(Source {
            name: project.unwrap_or_else(|| {
                main.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            }),
            repo: crate::repo::origin_of(&main),
            dir: main,
            at: cwd.to_path_buf(),
            git,
            far: Vec::new(),
        });
    }
    // Projects on another machine, by the project their folders name. One
    // that is also here is already in the list, found through its checkout
    for f in desk.folders.iter() {
        let (Some(host), Some(cwd), Some(project)) = (f.host.as_ref(), f.cwd.as_deref(), f.project.as_deref()) else {
            continue;
        };
        let Ok(at) = crate::elsewhere::Elsewhere::of(host) else { continue };
        if let Some(s) = out.iter_mut().find(|s| s.name == project) {
            if !s.far.is_empty() {
                // Not known from the first folder (its machine paused): any
                // other folder of it that can say, says it
                if s.repo.is_none() {
                    s.repo = far_origin(host, cwd);
                }
                s.far.push((cwd.to_path_buf(), at, host.name.clone()));
            }
            continue;
        }
        let spec = desk.projects.iter().find(|p| p.name == project);
        out.push(Source {
            name: project.to_string(),
            // Where the project is fetched from, as it was written when its
            // checkout there was made: known without asking any machine,
            // which a paused one would be started for
            repo: spec
                .and_then(|p| p.origin.as_deref())
                .and_then(|o| crate::repo::github_path(o.trim()))
                .or_else(|| far_origin(host, cwd)),
            dir: cwd.to_path_buf(),
            at: cwd.to_path_buf(),
            git: desk.git_use(spec.and_then(|p| p.git_account.as_deref())),
            far: vec![(cwd.to_path_buf(), at, host.name.clone())],
        });
    }
    // A name is what the Issue page asks for a repository by, so no two may
    // share one. A checkout no project is written down for is named for its
    // folder, and that can be a project's name, or another such checkout's:
    // the page then asked for one and was handed the other. The written
    // names stand; a folder's name that is taken gets a number
    let mut taken: std::collections::HashSet<String> = desk.projects.iter().map(|p| p.name.clone()).collect();
    for (i, s) in out.iter().enumerate() {
        if !guessed.contains(&i) {
            taken.insert(s.name.clone());
        }
    }
    for i in guessed {
        let base = out[i].name.clone();
        let name = match taken.contains(&base) {
            false => base,
            true => (2..).map(|n| format!("{base} {n}")).find(|n| !taken.contains(n)).expect("endless"),
        };
        taken.insert(name.clone());
        out[i].name = name;
    }
    out
}

/// A folder on another machine's GitHub repository, as `owner/name`: asked of
/// git there on a thread and kept, since the list this goes into is asked for
/// on the board's own loop. `None` until it has been answered
pub fn far_origin(host: &crate::config::HostSpec, dir: &std::path::Path) -> Option<String> {
    type Known = std::collections::HashMap<String, Option<Option<String>>>;
    static KNOWN: std::sync::OnceLock<std::sync::Mutex<Known>> = std::sync::OnceLock::new();
    let key = format!("{}\u{1f}{}", host.name, dir.to_string_lossy());
    let known = KNOWN.get_or_init(Default::default);
    {
        let mut k = known.lock().unwrap_or_else(|e| e.into_inner());
        match k.get(&key) {
            Some(Some(found)) => return found.clone(),
            Some(None) => return None,
            // A paused MicroVM is not started to be asked: a panel put back
            // on screen at launch would start, and bill, a machine nobody is
            // using. Asked once something of this program has it running
            None if host.is_made() && host.instance.as_deref().is_some_and(crate::e2b::asleep) => return None,
            None => {
                k.insert(key.clone(), None);
            }
        }
    }
    let (host, dir) = (host.clone(), dir.to_path_buf());
    std::thread::spawn(move || {
        let found = crate::elsewhere::Elsewhere::of(&host).ok().and_then(|at| {
            crate::git::there(&dir, Some(&at));
            let url = crate::git::run(&dir, &["remote", "get-url", "origin"]).ok();
            crate::git::there(&dir, None);
            url.and_then(|u| crate::repo::github_path(u.trim()))
        });
        known.lock().unwrap_or_else(|e| e.into_inner()).insert(key, Some(found));
    });
    None
}

/// The folder the page named, when it is a folder of the project it named: the
/// checkout or a worktree cut from it. Anything else is not a place git is run
/// for a request from the page
pub fn project_folder(sources: &[Source], project: &str, folder: &str) -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(folder);
    // On another machine: one of the project's folders there, and git about
    // it runs there from here on this thread
    if let Some(s) = sources.iter().find(|s| s.name == project && !s.far.is_empty()) {
        // Named by place key: the folder on the machine the key names
        let found = s.far_at(&dir);
        if let Some((path, at)) = &found {
            crate::git::there(path, Some(at));
        }
        return found.map(|(path, _)| path);
    }
    let fam = crate::repo::family_of(&dir)?;
    sources
        .iter()
        .any(|s| s.name == project && crate::repo::family_of(&s.dir).as_deref() == Some(fam.as_path()))
        .then_some(dir)
}

/// The folder of a project on another machine that is on `head`, asked of git
/// in each of its folders there in turn. Waits on those machines, so it is for
/// a thread. On the thread that asked, git about the folder found runs there
/// from here on (see `git::there`).
///
/// The folder the board last heard is on that branch is asked first, and
/// alone when it still is: every folder on a MicroVM is a machine of its own,
/// and asking each in turn started every paused one to find the one wanted.
///
/// Answered with the machine it is on, by name: the same path can be a folder
/// on two machines, and work handed to the folder goes to this one
pub fn far_head_folder(s: &Source, head: &str) -> Option<(std::path::PathBuf, String)> {
    let head = head.trim();
    if head.is_empty() {
        return None;
    }
    let heard = |dir: &std::path::Path, at: &crate::elsewhere::Elsewhere| {
        crate::git::far_place(at, dir, false).0.as_deref() == Some(head)
    };
    let (first, rest): (Vec<_>, Vec<_>) = s.far.iter().partition(|(dir, at, _)| heard(dir, at));
    for (dir, at, host) in first.into_iter().chain(rest) {
        crate::git::there(dir, Some(at));
        let on = crate::git::run(dir, &["branch", "--show-current"]).ok();
        if on.as_deref().map(str::trim) == Some(head) {
            return Some((dir.clone(), host.clone()));
        }
    }
    crate::git::there(&s.dir, None);
    None
}

/// The folder on this PC where a pull request's branch is checked out: the
/// project's checkout itself, or any worktree cut from it. None when no folder
/// stands on that branch -- one is made for it first. A worktree an AI tool
/// made for its own helper is not somewhere to open somebody's work, even on
/// that branch (`repo::tool_scratch`; `bases` are the places projects chose)
pub fn head_folder(checkout: &std::path::Path, head: &str, bases: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    let head = head.trim();
    if head.is_empty() {
        return None;
    }
    let main = crate::repo::main_checkout(checkout).unwrap_or_else(|| checkout.to_path_buf());
    if crate::repo::branch_of(&main).as_deref() == Some(head) {
        return Some(main);
    }
    let family = crate::repo::family_of(&main)?;
    crate::repo::worktrees_of(&family)
        .into_iter()
        .find(|(f, b)| b.as_deref() == Some(head) && !crate::repo::tool_scratch(&main, f, bases))
        .map(|(folder, _)| folder)
}

/// What a pull request would carry, file by file: added and removed lines, from
/// `origin/<base>` to the branch in front. A file git cannot count lines in
/// (an image) is said to be binary
pub fn pr_files(dir: &std::path::Path, base: &str) -> Result<Value> {
    let out = crate::git::run(dir, &["diff", "--no-color", "--numstat", &format!("origin/{base}...HEAD")])?;
    let rows: Vec<Value> = out
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let (a, r, path) = (parts.next()?, parts.next()?, parts.next()?);
            let binary = a == "-" && r == "-";
            Some(json!({"path": path, "added": a.parse::<u64>().unwrap_or(0),
                        "removed": r.parse::<u64>().unwrap_or(0), "binary": binary}))
        })
        .collect();
    Ok(json!(rows))
}

/// One file's change in a pull request, in the pieces the git panel draws
pub fn pr_file(dir: &std::path::Path, base: &str, path: &str) -> Result<Value> {
    let text = crate::git::run(dir, &["diff", "--no-color", &format!("origin/{base}...HEAD"), "--", path])?;
    let binary = text.lines().any(|l| l.starts_with("Binary files ")) || text.contains("GIT binary patch");
    let hunks: Vec<Value> = crate::git::split_hunks(&text)
        .into_iter()
        .map(|h| json!({"start": h.start, "end": h.end, "patch": h.patch}))
        .collect();
    Ok(json!({"hunks": hunks, "binary": binary}))
}

/// The automation command a request from the Issue tab is the same as, so it
/// is allowed or refused by the same row of the permission table
pub fn command_for(act: &str, pulls: bool) -> Option<&'static str> {
    Some(match (act, pulls) {
        ("list", false) => "github_issues",
        ("list", true) | ("branch_prs", _) => "github_prs",
        // Reading a pull request's failed checks, or its review threads left
        // open, to hand them to an AI tab
        ("ci_fix", _) | ("review_fix", _) => "github_pr",
        // Reading one check's log, for a person
        ("ci_log", _) => "github_pr",
        ("detail", false) => "github_issue",
        ("detail", true) => "github_pr",
        ("options", _) => "github_labels",
        ("create", _) => "github_issue_create",
        // A file put on GitHub for the issue or comment being written
        ("upload_piece", _) | ("upload", _) => "github_issue_create",
        ("create_pr", _) => "github_pr_create",
        // The branches a pull request can go into are read off this PC's copy
        ("pr_bases", _) => "git_branches",
        // What it would carry is a diff on this PC
        ("pr_files", _) | ("pr_file", _) => "git_diff",
        ("comment", _) => "github_comment",
        // An issue or pull request changed in place, and an issue's sub-issues
        ("edit", _) => "github_issue_edit",
        ("sub_add", _) => "github_sub_issue_add",
        ("sub_remove", _) => "github_sub_issue_remove",
        ("sub_move", _) => "github_sub_issue_move",
        ("issue_state", _) => "github_issue_state",
        ("pr_state", _) => "github_pr_state",
        ("merge", _) => "github_pr_merge",
        // A GitHub Project: the ones there are, one read as a view shows it,
        // and one card's value in one field changed
        ("gh_projects", _) => "github_projects",
        ("gh_project", _) => "github_project",
        ("gh_project_set", _) => "github_project_set",
        // ...and the cards themselves: one added, put in its place, archived
        // or taken off, a draft's words, and the issue made of a draft
        ("gh_project_add", _) => "github_project_add",
        ("gh_project_move", _) => "github_project_move",
        ("gh_project_archive", _) => "github_project_archive",
        ("gh_project_remove", _) => "github_project_remove",
        ("gh_project_draft", _) => "github_project_draft",
        ("gh_project_to_issue", _) => "github_project_to_issue",
        // ...and the board itself: its views, its fields, its own words, the
        // reports of how it stands, what it is linked to and who may see it
        ("gh_project_view_add", _) => "github_project_view_add",
        ("gh_project_view_set", _) => "github_project_view_set",
        ("gh_project_view_remove", _) => "github_project_view_remove",
        ("gh_project_field_add", _) => "github_project_field_add",
        ("gh_project_field_set", _) => "github_project_field_set",
        ("gh_project_field_remove", _) => "github_project_field_remove",
        ("gh_project_about", _) => "github_project_about",
        ("gh_project_create", _) => "github_project_create",
        ("gh_project_copy", _) => "github_project_copy",
        ("gh_project_update", _) => "github_project_update",
        ("gh_project_delete", _) => "github_project_delete",
        ("gh_project_template", _) => "github_project_template",
        ("gh_project_status", _) => "github_project_status",
        ("gh_project_status_remove", _) => "github_project_status_remove",
        ("gh_project_link", _) => "github_project_link",
        ("gh_project_access", _) => "github_project_access",
        ("gh_project_workflow_remove", _) => "github_project_workflow_remove",
        // Bringing a pull request's branch here is a fetch, and is allowed as one
        ("prepare", _) => "git_fetch",
        _ => return None,
    })
}

/// Answer one request from the Issue tab. Runs on a thread of its own: every
/// part of it waits for GitHub. `look` reads the secrets the desk's accounts
/// keep, already copied out for this thread
pub fn answer(
    act: &str,
    args: &Value,
    sources: &[Source],
    look: &dyn Fn(&str) -> Option<String>,
) -> Value {
    let pulls = args.get("kind").and_then(|k| k.as_str()) == Some("pr");
    let wanted = args
        .get("project")
        .and_then(|p| p.as_str())
        .unwrap_or_default();
    // `seq` goes back as it came, so the screen can tell the answer to its last
    // question from one it has since stopped waiting for
    // `tag` goes back as it came too: what the screen marked its request
    // with, to know which of its own it is the answer to
    let base = json!({"act": act, "kind": if pulls { "pr" } else { "issue" }, "project": wanted,
        "at_repo": args.get("at_repo").cloned().unwrap_or(Value::Null),
        "tag": args.get("tag").cloned().unwrap_or(Value::Null),
        "in_repo": args.get("in_repo").cloned().unwrap_or(Value::Null),
        "seq": args.get("seq").cloned().unwrap_or(Value::Null)});
    let with = |mut v: Value, extra: Value| {
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        v
    };
    let hub_for = |s: &Source| -> Result<(Repo, Hub)> {
        let (repo, token) = target_of(s, look)?;
        Ok((repo, Hub::new(token)))
    };
    if act == "list" {
        // Every chosen project, each answering for itself: one that cannot be
        // read says why beside the others rather than emptying the list
        let q = Query::from_json(args);
        let mut items: Vec<Value> = Vec::new();
        let mut problems: Vec<Value> = Vec::new();
        let mut total = 0u64;
        for s in sources
            .iter()
            .filter(|s| wanted.is_empty() || s.name == wanted)
        {
            let got = hub_for(s).and_then(|(repo, hub)| {
                if pulls {
                    hub.pulls(&repo, &q)
                } else {
                    hub.issues(&repo, &q)
                }
            });
            match got {
                Ok(v) => {
                    total += v.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
                    for mut i in v.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default() {
                        i["project"] = json!(s.name);
                        i["repo"] = json!(s.repo);
                        items.push(i);
                    }
                }
                Err(e) => problems.push(json!({"project": s.name, "error": format!("{e:#}"), "settings": settings_fix(&e)})),
            }
        }
        items.sort_by(|a, b| {
            b["updated"]
                .as_str()
                .unwrap_or_default()
                .cmp(a["updated"].as_str().unwrap_or_default())
        });
        return with(
            base,
            json!({"ok": true, "items": items, "problems": problems, "total": total,
            "page": q.page, "per_page": PAGE}),
        );
    }
    let Some(source) = sources.iter().find(|s| s.name == wanted) else {
        return with(
            base,
            json!({"ok": false, "error": crate::i18n::t("err.github.no_project")}),
        );
    };
    let number = args.get("number").and_then(|n| n.as_u64()).unwrap_or(0);
    let s = |k: &str| {
        args.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let list = |k: &str| -> Vec<String> {
        args.get(k)
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    };
    // Who a draft is given to: the whole list when it is said whole, else
    // worked out from who is put on and taken off, against who it has now
    let draft_people = |hub: &Hub, draft: &str| -> Result<Option<Vec<String>>> {
        if args.get("add_assignees").is_some() || args.get("remove_assignees").is_some() {
            return crate::ghproject::draft_people(hub, draft, &list("add_assignees"), &list("remove_assignees")).map(Some);
        }
        Ok(args.get("assignees").filter(|a| a.is_array()).map(|_| list("assignees")))
    };
    // Read here, not asked of GitHub: the branches this PC knows the server
    // has, the one its server calls the default first
    if act == "pr_bases" {
        let mut bases: Vec<String> = Vec::new();
        // On another machine, the folder the form is for: each folder on a
        // MicroVM is a machine of its own, with its own copy of the
        // repository and its own last fetch -- and the one being worked in is
        // the one that is up. Any folder, for a form not written for one
        let folder = s("folder");
        let here = (!folder.is_empty())
            .then(|| source.far_at(std::path::Path::new(&folder)))
            .flatten()
            .or(source.far.first().map(|(d, at, _)| (d.clone(), at)));
        let found = match here.as_ref().map(|(d, at)| (d, *at)) {
            // On another machine: what git there knows the server has
            Some((dir, crate::elsewhere::Elsewhere::Cloud(host))) => {
                crate::worktree::far_bases_now(host, &dir.to_string_lossy().replace('\\', "/")).0
            }
            Some((dir, _)) => {
                source.note_far(dir);
                crate::git::run(dir, &["for-each-ref", "--format=%(refname:short)", "refs/remotes"])
                    .map(|t| t.lines().map(|l| l.trim().to_string()).collect())
                    .unwrap_or_default()
            }
            None => crate::worktree::bases(&source.dir),
        };
        for b in found {
            if let Some(name) = b.strip_prefix("origin/")
                && name != "HEAD"
                && !bases.iter().any(|x| x == name)
            {
                bases.push(name.to_string());
            }
        }
        return with(base, json!({"ok": true, "data": {"bases": bases}}));
    }
    // What a pull request would carry, read from the folder it is made from
    if act == "pr_files" || act == "pr_file" {
        let path = s("path");
        let read = match project_folder(sources, wanted, &s("folder")) {
            None => Err(anyhow!(crate::i18n::t("err.github.no_project"))),
            Some(dir) if act == "pr_files" => pr_files(&dir, &s("base")),
            Some(dir) => pr_file(&dir, &s("base"), &path),
        };
        return match read {
            Ok(data) => with(base, json!({"ok": true, "path": path, "data": data})),
            Err(e) => with(base, json!({"ok": false, "path": path, "error": format!("{e:#}")})),
        };
    }
    // A piece of a file on its way to GitHub, kept on this PC until the rest
    // is in. Nothing is asked of GitHub until then
    if act == "upload_piece" {
        let at = args.get("at").and_then(|a| a.as_u64()).unwrap_or(0);
        return match upload_piece(&s("id"), at, &s("data")) {
            Ok(()) => with(base, json!({"ok": true})),
            Err(e) => {
                if let Some(p) = upload_place(&s("id")) {
                    let _ = std::fs::remove_file(p);
                }
                with(base, json!({"ok": false, "error": format!("{e:#}")}))
            }
        };
    }
    // An issue from a template with something it requires left empty is not
    // sent, and the screen is told which, so it can point at them. Asked
    // before GitHub, whose own page would refuse it -- and whose API would not
    let template = args
        .get("template")
        .filter(|t| !t.is_null())
        .and_then(|t| serde_json::from_value::<crate::issueform::Template>(t.clone()).ok());
    let values = args.get("values").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if act == "create"
        && let Some(t) = &template
        && let Err(e) = crate::issueform::issue_body(t, &s("title"), &s("body"), &values)
    {
        return with(
            base,
            json!({"ok": false, "error": format!("{e:#}"), "missing": crate::issueform::missing(t, &s("title"), &values)}),
        );
    }
    // The file whose pieces are all in, read and let go of here, so they go
    // whether or not the account can be signed in as
    let mut uploaded = (act == "upload").then(|| {
        let place = upload_place(&s("id")).ok_or_else(|| anyhow!("bad upload id"))?;
        let read = std::fs::read(&place).map_err(|e| anyhow!("{e}"));
        let _ = std::fs::remove_file(&place);
        read
    });
    // A card of a GitHub Project can be about any repository the account
    // sees, not only the desk's: named, it is the one read and written, as
    // the account of the project the board is reached through
    let elsewhere = match s("at_repo") {
        r if r.is_empty() => None,
        r => match Repo::parse(&r) {
            Some(repo) => Some(repo),
            None => return with(base, json!({"ok": false, "number": number, "error": crate::i18n::t("err.ghproject.bad_repo")})),
        },
    };
    // The repository the screen saw the project stand for, when it says so:
    // a request made before the desk was changed for one whose project of the
    // same name is another repository is refused, not carried out there
    let seen = s("in_repo");
    let done = hub_for(source).and_then(|(repo, hub)| {
        if elsewhere.is_none() && !seen.is_empty() && !seen.eq_ignore_ascii_case(&repo.slug()) {
            bail!(crate::i18n::tp("err.github.other_repo", &[("repo", &seen)]));
        }
        Ok((elsewhere.unwrap_or(repo), hub))
    }).and_then(|(repo, hub)| match act {
        "detail" if pulls => hub.pull(&repo, number),
        "detail" => hub.issue(&repo, number),
        // With what CI says of the commit the branch is at on the server: the
        // same commit for every base the branch was sent to, and the one CI
        // ran on whether a pull request was ever made from it or not -- a
        // check that failed is worth knowing about before anybody opens one
        "branch_prs" => hub.branch_pulls(&repo, &s("head")).map(|prs| {
            let sha = prs
                .as_array()
                .and_then(|a| a.iter().find(|p| p["state"] == "open"))
                .and_then(|p| p["sha"].as_str())
                .map(str::to_string)
                .or_else(|| Some(s("sha")).filter(|sha| !sha.is_empty()));
            let checks = sha.as_deref().and_then(|sha| {
                hub.checks(&repo, sha).ok().map(|mut c| {
                    c["sha"] = json!(sha);
                    c
                })
            });
            json!({"prs": prs, "checks": checks})
        }),
        // With the repository's issue templates. Templates that cannot be
        // read leave the labels and people readable, and say why beside them
        "options" => {
            let labels = hub.labels(&repo)?;
            let assignees = hub.assignees(&repo)?;
            let templates = hub.issue_templates(&repo).unwrap_or_else(|e| crate::issueform::Templates {
                blank: true,
                problems: vec![crate::issueform::Problem { file: crate::issueform::FOLDER.into(), error: format!("{e:#}") }],
                ..Default::default()
            });
            // What an issue already there can be given besides: neither is
            // worth losing the labels and people over
            let milestones = hub.milestones(&repo).unwrap_or_default();
            let types = hub.issue_types(&repo).unwrap_or_default();
            Ok(json!({"labels": labels, "assignees": assignees, "templates": templates, "milestones": milestones, "types": types}))
        }
        "edit" => hub.edit_issue(&repo, number, args.get("change").unwrap_or(&Value::Null)),
        "sub_add" => hub
            .sub_add(&repo, number, &s("sub"), args.get("replace").and_then(|r| r.as_bool()).unwrap_or(false))
            .map(|()| json!(true)),
        "sub_remove" => hub.sub_remove(&repo, number, &s("sub")).map(|()| json!(true)),
        "sub_move" => hub.sub_move(&repo, number, &s("sub"), args.get("after").and_then(|a| a.as_str())).map(|()| json!(true)),
        // Made from a template: its description written the way GitHub writes
        // it, and nothing sent while what the template requires is missing
        "create" => {
            let title = s("title");
            let body = match &template {
                None => Ok(s("body")),
                Some(t) => crate::issueform::issue_body(t, &title, &s("body"), &values),
            };
            body.and_then(|body| hub.create_issue(&repo, &title, &body, &list("labels"), &list("assignees")))
        }
        "create_pr" => hub.create_pull(
            &repo,
            &s("title"),
            &s("body"),
            &s("head"),
            &s("base"),
            args.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        ),
        // The file whose pieces are all in: sent on, and the pieces let go of
        // whatever GitHub says
        "upload" => {
            let kind = s("type");
            let read = uploaded.take().unwrap_or_else(|| Err(anyhow!("bad upload id")));
            read.and_then(|bytes| match attachable(&kind) {
                None => bail!(crate::i18n::tp("err.github.upload_kind", &[("name", &s("name"))])),
                Some(most) if bytes.len() > most => bail!(crate::i18n::t("err.github.upload_size")),
                Some(_) if bytes.is_empty() => bail!(crate::i18n::t("err.github.upload_size")),
                Some(_) => hub.upload_asset(&repo, &s("name"), &kind, &bytes).map(|url| json!({"url": url})),
            })
        }
        "comment" => hub.comment(&repo, number, &s("body")),
        "issue_state" => hub
            .set_issue_state(
                &repo,
                number,
                &s("state"),
                args.get("duplicate_of").and_then(|d| d.as_u64()),
            )
            .map(|()| json!(true)),
        "pr_state" => hub
            .set_pull_state(&repo, number, s("state") == "open")
            .map(|()| json!(true)),
        "merge" => hub.merge_pull(&repo, number, &s("method")),
        "prepare" => prepare_pull(source, &hub, &repo, number, look),
        // A GitHub Project is read as the account of the project it is
        // reached through, wherever on GitHub it is
        "gh_projects" => crate::ghproject::list(&hub, &repo),
        "gh_project" => {
            let at = crate::ghproject::GhRef::parse(&s("ref")).ok_or_else(|| anyhow!(crate::i18n::t("err.ghproject.bad_ref")))?;
            let want = crate::ghproject::Want {
                view: args.get("view").and_then(|v| v.as_u64()),
                filter: args.get("filter").and_then(|f| f.as_str()),
                most: args.get("most").and_then(|m| m.as_u64()),
                archived: args.get("archived").and_then(|a| a.as_bool()).unwrap_or(false),
            };
            crate::ghproject::read(&hub, &at, want).map(|mut v| {
                v["ref"] = json!(s("ref"));
                v
            })
        }
        "gh_project_set" => crate::ghproject::set_value(
            &hub,
            &s("id"),
            &s("card"),
            &s("field"),
            args.get("value").unwrap_or(&Value::Null),
        )
        .map(|()| json!({"card": s("card"), "field": s("field")})),
        "gh_project_add" => crate::ghproject::add(
            &hub,
            &s("id"),
            args.get("what").unwrap_or(&Value::Null),
            args.get("values").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or_default(),
            args.get("after").and_then(|a| a.as_str()),
        ),
        "gh_project_move" => crate::ghproject::move_card(&hub, &s("id"), &s("card"), args.get("after").and_then(|a| a.as_str()))
            .map(|()| json!({"card": s("card")})),
        "gh_project_archive" => crate::ghproject::archive(
            &hub,
            &s("id"),
            &s("card"),
            args.get("restore").and_then(|r| r.as_bool()).unwrap_or(false),
        )
        .map(|()| json!({"card": s("card")})),
        "gh_project_remove" => crate::ghproject::remove(&hub, &s("id"), &s("card")).map(|()| json!({"card": s("card")})),
        "gh_project_draft" => {
            let _turn = crate::ghproject::turn();
            let people = draft_people(&hub, &s("draft"))?;
            let said = |k: &str| args.get(k).and_then(|x| x.as_str());
            crate::ghproject::draft(&hub, &s("draft"), said("title"), said("body"), people.as_deref()).map(|()| json!({"card": s("card")}))
        }
        "gh_project_view_add" => crate::ghproject::view_make(&hub, &s("id"), &s("name"), &s("layout")),
        "gh_project_view_set" => crate::ghproject::view_change(&hub, &s("view_id"), args.get("change").unwrap_or(&Value::Null)).map(|()| json!(true)),
        "gh_project_view_remove" => crate::ghproject::view_remove(&hub, &s("view_id")).map(|()| json!(true)),
        "gh_project_field_add" => crate::ghproject::field_make(&hub, &s("id"), args.get("field").unwrap_or(&Value::Null)),
        "gh_project_field_set" => crate::ghproject::field_change(&hub, &s("field"), args.get("change").unwrap_or(&Value::Null)).map(|()| json!(true)),
        "gh_project_field_remove" => crate::ghproject::field_remove(&hub, &s("field")).map(|()| json!(true)),
        "gh_project_about" => {
            let at = crate::ghproject::GhRef::parse(&s("ref")).ok_or_else(|| anyhow!(crate::i18n::t("err.ghproject.bad_ref")))?;
            crate::ghproject::about(&hub, &at)
        }
        "gh_project_create" => {
            let linked = s("link");
            crate::ghproject::make(&hub, &s("owner"), &s("title"), Some(linked.as_str()))
        }
        "gh_project_copy" => crate::ghproject::copy(
            &hub,
            &s("id"),
            &s("owner"),
            &s("title"),
            args.get("drafts").and_then(|d| d.as_bool()).unwrap_or(false),
        ),
        "gh_project_update" => crate::ghproject::change(&hub, &s("id"), args.get("change").unwrap_or(&Value::Null)).map(|()| json!(true)),
        "gh_project_delete" => crate::ghproject::delete(&hub, &s("id")).map(|()| json!(true)),
        "gh_project_template" => {
            crate::ghproject::template(&hub, &s("id"), args.get("on").and_then(|o| o.as_bool()).unwrap_or(true)).map(|()| json!(true))
        }
        "gh_project_status" => {
            let update = s("update");
            crate::ghproject::status_write(&hub, &s("id"), (!update.is_empty()).then_some(update.as_str()), args.get("status").unwrap_or(&Value::Null))
        }
        "gh_project_status_remove" => crate::ghproject::status_remove(&hub, &s("update")).map(|()| json!(true)),
        "gh_project_link" => {
            crate::ghproject::link(&hub, &s("id"), &s("link"), args.get("on").and_then(|o| o.as_bool()).unwrap_or(true)).map(|()| json!(true))
        }
        "gh_project_access" => crate::ghproject::access(
            &hub,
            &s("id"),
            args.get("people").and_then(|p| p.as_array()).map(|p| p.as_slice()).unwrap_or_default(),
        ),
        "gh_project_workflow_remove" => crate::ghproject::workflow_remove(&hub, &s("workflow")).map(|()| json!(true)),
        "gh_project_to_issue" => {
            let (draft, title, body) = (s("draft"), s("title"), s("body"));
            let words = (!draft.is_empty() && args.get("title").is_some()).then_some((draft.as_str(), title.as_str(), body.as_str()));
            let _turn = crate::ghproject::turn();
            let people = draft_people(&hub, &draft)?;
            crate::ghproject::to_issue(&hub, &s("card"), &s("repo"), words, people.as_deref())
        }
        other => Err(anyhow!("unknown request {other}")),
    });
    match done {
        Ok(data) => with(base, json!({"ok": true, "number": number, "data": data})),
        Err(e) => with(
            base,
            json!({"ok": false, "number": number, "error": format!("{e:#}"), "settings": settings_fix(&e)}),
        ),
    }
}

/// Bring a pull request's branch to this PC, so a worktree can be made on it.
///
/// From the repository itself when the branch is there, named as it is; from a
/// fork, through the pull request's own ref, as `pr-12`. Signed in as the
/// project's account, the same as any fetch. Answers with the branch to make
/// the worktree on and what it starts from
fn prepare_pull(
    source: &Source,
    hub: &Hub,
    repo: &Repo,
    number: u64,
    look: &dyn Fn(&str) -> Option<String>,
) -> Result<Value> {
    let pr = hub.call(
        "GET",
        &format!("repos/{}/pulls/{number}", repo.slug()),
        None,
    )?;
    let head = pr
        .pointer("/head/ref")
        .and_then(|h| h.as_str())
        .unwrap_or_default()
        .to_string();
    let fork = pr.pointer("/head/repo/full_name") != pr.pointer("/base/repo/full_name");
    let who = source.git.to_git(true, look).map_err(|e| anyhow!(e))?;
    let (branch, refspec, base) = match fork || head.is_empty() {
        false => (
            head.clone(),
            format!("+refs/heads/{head}:refs/remotes/origin/{head}"),
            format!("origin/{head}"),
        ),
        true => (
            format!("pr-{number}"),
            format!("+refs/pull/{number}/head:refs/remotes/origin/pr/{number}"),
            format!("origin/pr/{number}"),
        ),
    };
    crate::git::run_as(
        &source.dir,
        &["fetch", "--no-tags", "origin", &refspec],
        "",
        Duration::from_secs(180),
        &who,
    )?;
    Ok(json!({"branch": branch, "base": base, "fork": fork}))
}

/// One row of a list, in this module's words
fn row(i: &Value, pulls: bool) -> Value {
    let merged = i
        .pointer("/pull_request/merged_at")
        .is_some_and(|m| !m.is_null());
    let state = match (
        i.get("state").and_then(|s| s.as_str()).unwrap_or("open"),
        merged,
    ) {
        (_, true) => "merged",
        ("closed", _) => "closed",
        _ => "open",
    };
    json!({
        "kind": if pulls { "pr" } else { "issue" },
        "number": i.get("number"),
        "title": i.get("title"),
        "state": state,
        "reason": i.get("state_reason"),
        "draft": i.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        "author": i.pointer("/user/login"),
        "labels": i.get("labels").and_then(|l| l.as_array()).map(|l| {
            l.iter().filter_map(|x| x.get("name").cloned()).collect::<Vec<_>>()
        }).unwrap_or_default(),
        "assignees": logins(i.get("assignees")),
        "comments": i.get("comments"),
        "updated": i.get("updated_at"),
        "url": i.get("html_url"),
        "workspace": workspace_name(
            pulls,
            i.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
            i.get("title").and_then(|t| t.as_str()).unwrap_or_default(),
        ),
    })
}

fn logins(v: Option<&Value>) -> Value {
    Value::Array(
        v.and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .filter_map(|u| u.get("login").cloned())
            .collect(),
    )
}

/// A check run's verdict: failed, pending, passed, or neutral
/// How much of a refusal's body is read when a log is refused: GitHub's
/// answer then is a short JSON message, and a body larger than this is not
/// one worth holding
const REFUSAL_MOST: usize = 64 * 1024;

/// How much of a failed job's log is handed on: the lines before its last
/// error, where a failure says what failed -- not the clean-up after it
const LOG_TAIL_LINES: usize = 80;
const LOG_TAIL_CHARS: usize = 6000;

/// The part of a job's log worth reading: GitHub's time stamps taken off,
/// everything after the last error dropped, the last lines of what is left
pub fn log_tail(log: &str) -> String {
    let lines: Vec<&str> = log
        .trim_start_matches('\u{feff}')
        .lines()
        .map(|l| {
            // "2026-09-17T06:20:15.6543593Z text"
            match l.split_once(' ') {
                Some((stamp, rest)) if stamp.len() >= 20 && stamp.ends_with('Z') && stamp.as_bytes().get(10) == Some(&b'T') => rest,
                _ => l,
            }
        })
        .collect();
    let end = lines.iter().rposition(|l| l.contains("##[error]")).map(|i| i + 1).unwrap_or(lines.len());
    let start = end.saturating_sub(LOG_TAIL_LINES);
    let mut out = lines[start..end].join("\n");
    if out.len() > LOG_TAIL_CHARS {
        let cut = out.len() - LOG_TAIL_CHARS;
        let at = (cut..out.len()).find(|&i| out.is_char_boundary(i)).unwrap_or(out.len());
        out = out[at..].to_string();
    }
    out
}

/// The failed checks of a commit, each with the end of its log when GitHub
/// Actions ran it -- what an AI is given to find out why CI failed
pub fn ci_failures(
    sources: &[Source],
    project: &str,
    sha: &str,
    look: &dyn Fn(&str) -> Option<String>,
) -> Result<Value> {
    let source = sources
        .iter()
        .find(|s| s.name == project)
        .ok_or_else(|| anyhow!(crate::i18n::t("err.github.no_project")))?;
    let (repo, token) = target_of(source, look)?;
    let hub = Hub::new(token);
    let checks = hub.checks(&repo, sha)?;
    let mut out = Vec::new();
    for c in checks.get("items").and_then(|i| i.as_array()).into_iter().flatten() {
        if c.get("verdict").and_then(|v| v.as_str()) != Some("failed") {
            continue;
        }
        let mut row = json!({"name": c.get("name"), "url": c.get("url")});
        if let Some(job) = c.get("job").and_then(|j| j.as_u64())
            && let Ok(tail) = hub.job_log_tail(&repo, job)
        {
            row["log_tail"] = json!(tail);
        }
        out.push(row);
    }
    Ok(Value::Array(out))
}

/// The review threads of a pull request nobody has marked resolved: where
/// each is (the file, and its lines in the file as the pull request has it),
/// whether the code has moved on under it, and what was said in it, in
/// order. Asked of GitHub's GraphQL, which is the only place a thread's
/// "resolved" is kept -- a thread answered and resolved is not work left
pub fn review_threads(
    sources: &[Source],
    project: &str,
    number: u64,
    look: &dyn Fn(&str) -> Option<String>,
) -> Result<Value> {
    let source = sources
        .iter()
        .find(|s| s.name == project)
        .ok_or_else(|| anyhow!(crate::i18n::t("err.github.no_project")))?;
    let (repo, token) = target_of(source, look)?;
    Ok(Value::Array(Hub::new(token).open_review_threads(&repo, number)?))
}

impl Hub {
    /// One question to GitHub's GraphQL. Its answer whole -- `data`, and
    /// `errors` beside it when part of the question could not be answered --
    /// or the reason, when none of it could
    pub fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let v = self.call("POST", "graphql", Some(json!({"query": query, "variables": variables})))?;
        if v.get("data").is_none_or(|d| d.is_null())
            && let Some(e) = v.pointer("/errors/0/message").and_then(|m| m.as_str())
        {
            bail!("{e}");
        }
        Ok(v)
    }

    /// GitHub's answer to a query as it came, errors and all, even when it
    /// carries no data: for a caller that reads GitHub's reasons itself (an
    /// answer refused for a token's missing scope has no data at all)
    pub fn graphql_whole(&self, query: &str, variables: Value) -> Result<Value> {
        self.call("POST", "graphql", Some(json!({"query": query, "variables": variables})))
    }

    /// The review threads of a pull request nobody has marked resolved, as
    /// [`review_threads`] gives them
    pub fn open_review_threads(&self, repo: &Repo, number: u64) -> Result<Vec<Value>> {
        let query = "query($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){pullRequest(number:$number){commits(last:1){nodes{commit{committedDate}}} reactions(content:THUMBS_UP,last:30){nodes{createdAt user{login}}} reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor}nodes{isResolved isOutdated path line startLine originalLine comments(first:50){nodes{id author{login __typename} body}}}}}}}";
        let mut out = Vec::new();
        let mut after = Value::Null;
        // A pull request with more threads than one page holds is read on; ten
        // pages is a thousand threads, more than anybody hands to one AI tab
        for _ in 0..10 {
            let v = self.graphql(query, json!({"owner": repo.owner, "name": repo.name, "number": number, "after": after}))?;
            if let Some(e) = v.pointer("/errors/0/message").and_then(|m| m.as_str()) {
                bail!("{e}");
            }
            let pull = v.pointer("/data/repository/pullRequest").cloned().unwrap_or(Value::Null);
            let threads = pull.get("reviewThreads").cloned().unwrap_or(Value::Null);
            out.extend(open_threads(&threads, &clean_since_head(&pull)));
            match threads.pointer("/pageInfo/hasNextPage").and_then(|h| h.as_bool()) {
                Some(true) => after = threads.pointer("/pageInfo/endCursor").cloned().unwrap_or(Value::Null),
                _ => break,
            }
        }
        Ok(out)
    }
}

/// Who has put a 👍 on the pull request since its newest commit was made:
/// the way a bot that reviews every push says its round found nothing at all
/// -- nothing new, and nothing it said before still standing
fn clean_since_head(pull: &Value) -> Vec<String> {
    let since = pull
        .pointer("/commits/nodes/0/commit/committedDate")
        .and_then(|d| d.as_str())
        .map(crate::prnews::seconds)
        .unwrap_or(u64::MAX);
    let list = |p: &str| pull.pointer(p).and_then(|n| n.as_array()).cloned().unwrap_or_default();
    list("/reactions/nodes")
        .into_iter()
        .filter(|r| r.get("createdAt").and_then(|d| d.as_str()).map(crate::prnews::seconds).is_some_and(|at| at > since))
        .filter_map(|r| r.pointer("/user/login").and_then(|l| l.as_str()).map(crate::prnews::login))
        .collect()
}

/// The threads of one page of an answer that are still open, shaped for the
/// prompt: `path`, `line` (and `from`, for one about several lines),
/// `outdated` when the code under it has changed since, and `said` -- who
/// said what, in order.
///
/// A bot's thread on code that has changed since, when that bot has put a 👍
/// on the newest commit (`clean`), is not open work either: such a bot does
/// not mark its own threads resolved, and left in, a finding already fixed
/// would be counted and handed to an AI on every round after. Without its 👍
/// the thread stays, outdated or not -- a bot that finds the same thing again
/// does not post it a second time, so its old thread is all that says it. A
/// person's thread stays until the person resolves it
fn open_threads(threads: &Value, clean: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    for t in threads.get("nodes").and_then(|n| n.as_array()).into_iter().flatten() {
        if t.get("isResolved").and_then(|r| r.as_bool()) == Some(true) {
            continue;
        }
        let first = t.pointer("/comments/nodes/0/author");
        let named = first.and_then(|a| a.get("login")).and_then(|l| l.as_str()).unwrap_or_default();
        let by_bot = first.and_then(|a| a.get("__typename")).and_then(|k| k.as_str()) == Some("Bot") || named.ends_with("[bot]");
        if t.get("isOutdated").and_then(|o| o.as_bool()) == Some(true) && by_bot && clean.contains(&crate::prnews::login(named)) {
            continue;
        }
        let said: Vec<Value> = t
            .pointer("/comments/nodes")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .map(|c| json!({"who": c.pointer("/author/login"), "said": c.get("body")}))
            .collect();
        if said.is_empty() {
            continue;
        }
        let line = t.get("line").filter(|l| !l.is_null()).or_else(|| t.get("originalLine")).cloned().unwrap_or(Value::Null);
        // GitHub's id for each comment, beside what was said: what was handed
        // to an AI is remembered by them (`crate::handed`), and taken off
        // before the AI reads it
        let ids: Vec<Value> = t
            .pointer("/comments/nodes")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .map(|c| c.get("id").cloned().unwrap_or(Value::Null))
            .collect();
        let mut row = json!({"path": t.get("path"), "line": line, "said": said, "ids": ids});
        if let Some(from) = t.get("startLine").filter(|s| !s.is_null() && Some(*s) != Some(&line)) {
            row["from"] = from.clone();
        }
        if t.get("isOutdated").and_then(|o| o.as_bool()) == Some(true) {
            row["outdated"] = json!(true);
        }
        out.push(row);
    }
    out
}

/// One GitHub Actions job's log, for a person to read (its end, when it is
/// too big to hold): the project's repository and account found the way every
/// other request finds them
pub fn ci_log(
    sources: &[Source],
    project: &str,
    job: u64,
    look: &dyn Fn(&str) -> Option<String>,
) -> Result<crate::readview::Tail> {
    let source = sources
        .iter()
        .find(|s| s.name == project)
        .ok_or_else(|| anyhow!(crate::i18n::t("err.github.no_project")))?;
    let (repo, token) = target_of(source, look)?;
    Hub::new(token).job_log(&repo, job)
}

/// A job's log as an editor shows it, and the line of its first error
/// (1-based) when it has one.
///
/// The colour codes the runner wrote are taken off -- an editor draws no
/// colour, and `[31m` in front of a line is noise -- and the time stamp at the
/// head of each line is cut to the time of day: the date is the same all the
/// way down a job, and the stamp's seven fractional digits push the text off
/// to the right. The first error is where GitHub put its own mark
pub fn log_for_reading(log: &str) -> (String, Option<usize>) {
    let mut out = String::with_capacity(log.len());
    let mut first_error = None;
    // GitHub writes a job's log with a byte-order mark in front, which would
    // hide the first line's stamp from the check below
    for (i, line) in log.trim_start_matches('\u{feff}').lines().enumerate() {
        let plain = without_colour(line);
        let line = match plain.split_once(' ') {
            // "2026-09-17T06:20:15.6543593Z text" -> "06:20:15 text"
            Some((stamp, rest)) => match time_of_stamp(stamp) {
                Some(time) => format!("{time} {rest}"),
                None => plain,
            },
            None => plain,
        };
        if first_error.is_none() && line.contains("##[error]") {
            first_error = Some(i + 1);
        }
        out.push_str(&line);
        out.push('\n');
    }
    (out, first_error)
}

/// The time of day out of a runner's stamp (`2026-09-17T06:20:15.6543593Z`),
/// or `None` when the word is not one. Read character by character in its
/// fixed places, all of them ASCII: a log is anybody's text, and a word that
/// only resembles a stamp has to come back as `None` rather than be cut where
/// a character that takes several bytes happens to sit
fn time_of_stamp(stamp: &str) -> Option<&str> {
    let b = stamp.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    let shaped = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18].into_iter().all(digit)
        && at(4, b'-')
        && at(7, b'-')
        && at(10, b'T')
        && at(13, b':')
        && at(16, b':')
        && stamp.ends_with('Z')
        && stamp.is_ascii();
    // Every byte is ASCII, so byte places are character places
    shaped.then(|| &stamp[11..19])
}

/// A line with the terminal's escape sequences taken out: the colour codes
/// (`ESC [ ... m`) and any other `ESC [` sequence, which ends at its first
/// letter, and a lone `ESC`
fn without_colour(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// What one check run says, from whether it has finished and how. GitHub's
/// REST answer spells the conclusion in small letters and its GraphQL answer
/// in capitals; both are read here
pub(crate) fn check_verdict(done: bool, conclusion: &str) -> &'static str {
    match (done, conclusion.to_ascii_lowercase().as_str()) {
        (false, _) => "pending",
        (
            true,
            "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure" | "stale",
        ) => "failed",
        (true, "success" | "skipped") => "passed",
        _ => "neutral",
    }
}

/// What one commit status (the older kind of check, set by a service rather
/// than run by an app) says, from its state, in either spelling
pub(crate) fn status_verdict(state: &str) -> &'static str {
    match state.to_ascii_lowercase().as_str() {
        "success" => "passed",
        "pending" | "expected" => "pending",
        _ => "failed",
    }
}

/// How much of an account's GitHub allowance is left, by the kind of request
/// (`core` for REST, `graphql`), as the answers to its own requests said.
///
/// GitHub gives each account so many requests an hour of each kind, shared by
/// everything that signs in as it -- this program, `gh`, the AI tabs. Kept so
/// that what this program asks of its own accord, without anybody pressing
/// anything, stops well before the allowance runs out, and waits out a
/// refusal rather than asking into it
struct Allowance {
    remaining: u64,
    /// When GitHub fills it again, in seconds since 1970
    reset: u64,
    /// Asked to wait until then, in seconds since 1970 (a refusal, or GitHub
    /// saying to slow down)
    hold: u64,
}

static ALLOWANCE: std::sync::Mutex<Option<std::collections::HashMap<(u64, String), Allowance>>> =
    std::sync::Mutex::new(None);

/// What a token is known by here: never the token itself
fn token_mark(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

/// What is kept back for what people press: the requests this program makes
/// on its own stop while fewer than this are left of the hour's
const KEPT_FOR_PEOPLE: u64 = 500;

/// Write down what one answer said of the allowance
fn note_allowance(token: &str, status: u16, header: &dyn Fn(&str) -> Option<String>) {
    let num = |k: &str| header(k).and_then(|v| v.trim().parse::<u64>().ok());
    let Some(kind) = header("x-ratelimit-resource") else { return };
    let now = crate::prnews::now();
    let mut held = ALLOWANCE.lock().unwrap_or_else(|p| p.into_inner());
    let map = held.get_or_insert_with(Default::default);
    let a = map.entry((token_mark(token), kind)).or_insert(Allowance { remaining: u64::MAX, reset: 0, hold: 0 });
    if let Some(r) = num("x-ratelimit-remaining") {
        a.remaining = r;
    }
    if let Some(r) = num("x-ratelimit-reset") {
        a.reset = r;
    }
    // Refused for asking too much, or too fast: GitHub says how long to wait,
    // or failing that, until the allowance is filled again -- and at least a
    // minute, the shortest wait its own advice gives
    if status == 403 || status == 429 {
        let wait = num("retry-after").map(|s| now + s).unwrap_or(0);
        let refill = if a.remaining == 0 { a.reset } else { 0 };
        a.hold = wait.max(refill).max(now + 60);
    }
}

/// Whether this program may ask GitHub something of its own accord as this
/// account: not while it was told to wait, and not when what is left of the
/// hour is what people pressing buttons will need
pub fn spare(token: &str, kind: &str) -> bool {
    let now = crate::prnews::now();
    let held = ALLOWANCE.lock().unwrap_or_else(|p| p.into_inner());
    match held.as_ref().and_then(|m| m.get(&(token_mark(token), kind.to_string()))) {
        None => true,
        Some(a) if a.hold > now => false,
        Some(a) => a.remaining >= KEPT_FOR_PEOPLE || a.reset <= now,
    }
}

/// GitHub's refusal, in words a person can act on, and whether the account is
/// what to change
fn refusal(status: u16, said: &str) -> (String, bool) {
    let lower = said.to_lowercase();
    match status {
        401 => (crate::i18n::t("err.github.401"), true),
        403 | 429 if lower.contains("rate limit") => (crate::i18n::t("err.github.rate"), false),
        403 => (crate::i18n::tp("err.github.403", &[("said", said)]), true),
        404 => (crate::i18n::t("err.github.404"), true),
        // A search over a repository this account cannot see is refused as
        // invalid rather than as missing; it is the same answer to a person
        422 if lower.contains("cannot be searched") => (crate::i18n::t("err.github.404"), true),
        410 if lower.contains("disabled") => (crate::i18n::t("err.github.disabled"), false),
        // What GitHub no longer keeps: a job's log past the days it is kept
        410 => (crate::i18n::t("err.github.gone"), false),
        _ => (
            crate::i18n::tp(
                "err.github.other",
                &[("status", &status.to_string()), ("said", said)],
            ),
            false,
        ),
    }
}

/// A path in a repository written into an address: each part encoded, the
/// slashes between them kept
fn path_in_url(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// The text of a file as GitHub's contents answer carries it: base64, broken
/// into lines
fn file_text(file: &Value) -> Result<String> {
    use base64::Engine;
    let packed: String = file
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(packed)
        .map_err(|e| anyhow!("{e}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A search written into a query string
use crate::urlcodec::encode;

#[cfg(test)]
mod tests {
    /// A project on MicroVMs whose repository is known is not asked it again:
    /// asking went to the checkout's machine on every call to GitHub. Here the
    /// machine cannot be reached at all, and the answer still gets as far as
    /// the account -- the repository was never asked of it
    #[test]
    fn a_known_repository_is_not_asked_of_the_checkouts_machine() {
        let host = crate::config::HostSpec { name: "vm".into(), kind: Some("e2b".into()), ..Default::default() };
        let at = crate::elsewhere::Elsewhere::of(&host).unwrap();
        let s = super::Source {
            name: "site".into(),
            repo: Some("octocat/site".into()),
            dir: "/home/user/site".into(),
            at: "/home/user/site".into(),
            git: crate::config::GitUse::Missing("gone".into()),
            far: vec![("/home/user/site".into(), at, "vm".into())],
        };
        let err = super::target_of(&s, &|_| None).unwrap_err();
        assert!(err.downcast_ref::<super::AccountTrouble>().is_some(), "the repository was asked of the machine: {err:#}");
        let unknown = super::Source { repo: None, ..s };
        let err = super::target_of(&unknown, &|_| None).unwrap_err();
        assert!(err.downcast_ref::<super::AccountTrouble>().is_none(), "an unknown repository was not asked for: {err:#}");
    }

    /// What an AI is given of a failed job's log: the lines up to its last
    /// error, without GitHub's time stamps, and not the clean-up after it
    #[test]
    fn a_failed_logs_tail_ends_at_its_last_error() {
        let log = "2026-09-17T06:20:15.6543593Z running 3 tests\n\
                   2026-09-17T06:20:15.6545498Z test a ... FAILED\n\
                   2026-09-17T06:20:39.5426807Z ##[error]Process completed with exit code 1.\n\
                   2026-09-17T06:20:39.5667565Z Post job cleanup.\n\
                   2026-09-17T06:20:39.7690104Z [command]git version";
        let tail = super::log_tail(log);
        assert_eq!(tail, "running 3 tests\ntest a ... FAILED\n##[error]Process completed with exit code 1.");
        let long: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let tail = super::log_tail(&long);
        assert!(tail.lines().count() <= super::LOG_TAIL_LINES, "more lines than a tail");
        assert!(tail.ends_with("line 499"), "the end of a log with no error is not its end");
    }

    /// A log for a person keeps every line, without the runner's colours and
    /// with each stamp cut to the time of day, and knows where its first
    /// error is
    #[test]
    fn a_log_for_reading_keeps_its_lines_and_finds_the_first_error() {
        let log = "\u{feff}2026-09-17T06:20:15.6543593Z \u{1b}[36;1mcargo test\u{1b}[0m\n\
                   2026-09-17T06:20:16.0000000Z test a ... \u{1b}[31mFAILED\u{1b}[0m\n\
                   no stamp here\n\
                   2026-09-17T06:20:39.5426807Z ##[error]Process completed with exit code 1.\n\
                   2026-09-17T06:20:40.0000000Z ##[error]a second one";
        let (text, first) = super::log_for_reading(log);
        assert_eq!(
            text,
            "06:20:15 cargo test\n06:20:16 test a ... FAILED\nno stamp here\n\
             06:20:39 ##[error]Process completed with exit code 1.\n06:20:40 ##[error]a second one\n"
        );
        assert_eq!(first, Some(4), "the first error is the one opened on");
        assert_eq!(super::log_for_reading("all good\n").1, None);
        // A lone escape, and a sequence that is not a colour, go as well
        assert_eq!(super::without_colour("a\u{1b}[2Kb\u{1b}c"), "abc");
    }

    /// A line that only looks like it starts with a stamp is left as it was.
    /// The stamp's time is cut out by position, and a letter written in more
    /// than one byte where a digit should be put that position inside the
    /// letter -- which panicked the program instead of showing the log
    #[test]
    fn a_line_that_only_looks_stamped_is_kept_whole() {
        for odd in [
            "2026-09-17T06:20:1\u{e9}Z message",
            "2026-09-17T06:2\u{3042}:15.1Z message",
            "\u{3042}\u{3042}\u{3042}\u{3042}\u{3042}\u{3042}\u{3042}Z x",
            "2026-09-17T06-20-15.0000000Z message",
        ] {
            let (text, _) = super::log_for_reading(odd);
            assert_eq!(text, format!("{odd}\n"), "kept as it was: {odd}");
        }
        assert_eq!(super::log_for_reading("2026-09-17T06:20:15Z ok").0, "06:20:15 ok\n");
    }

    /// The branch a pull request comes from is found where it is checked out:
    /// in the checkout, or in a worktree cut from it, and nowhere else
    #[test]
    fn a_pull_requests_branch_is_found_in_the_folder_standing_on_it() {
        let git = |dir: &std::path::Path, args: &[&str]| {
            std::process::Command::new("git").current_dir(dir).args(args).output().ok().filter(|o| o.status.success())
        };
        let root = std::env::temp_dir().join(format!("shikisha-head-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        if git(&main, &["init", "-q", "-b", "main"]).is_none() {
            return;
        }
        git(&main, &["-c", "user.email=t@example.invalid", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "one"]).unwrap();
        let cut = root.join("cut");
        git(&main, &["worktree", "add", "-q", "-b", "feature", &cut.display().to_string()]).unwrap();
        // Compared as the disk resolves them: a temporary folder can be named
        // in its short form (`RUNNER~1`) and read back in its long one
        let real = |p: &std::path::Path| std::fs::canonicalize(p).ok();
        let is = |found: Option<std::path::PathBuf>, want: &std::path::Path| found.as_deref().and_then(real) == real(want);
        assert!(is(super::head_folder(&main, "main", &[]), &main), "the checkout was not found on its own branch");
        assert!(is(super::head_folder(&cut, "feature", &[]), &cut), "a worktree was not found from itself");
        assert!(is(super::head_folder(&main, "feature", &[]), &cut), "a worktree was not found from the checkout");
        assert_eq!(super::head_folder(&main, "elsewhere", &[]), None);
        // An AI tool's helper on the branch, inside the checkout: not where
        // somebody's work is opened -- unless the project put its worktrees there
        let helper = main.join(".claude").join("worktrees").join("agent-a1b2c3");
        git(&main, &["worktree", "add", "-q", "-b", "helper", &helper.display().to_string()]).unwrap();
        assert_eq!(super::head_folder(&main, "helper", &[]), None, "a helper's worktree was taken for the branch's folder");
        // The place as a project's settings would hold it: the long spelling
        let long = crate::repo::family_of(&main).and_then(|f| f.parent().map(|p| p.to_path_buf())).unwrap();
        let chosen = [long.join(".claude").join("worktrees")];
        assert!(is(super::head_folder(&main, "helper", &chosen), &helper), "a worktree where the project chose was not found");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project keeps both names of the one place it is in.
    ///
    /// The checkout is what the disk answers, because that is where git runs.
    /// The folder is what the settings hold, because that is what the settings
    /// can be searched for. The two are the same string for a folder on this
    /// PC and different ones as soon as the way in is a name for somewhere
    /// else -- a drive mapped to another machine's share answers as that
    /// machine (`P:\web\shop` -> `\\nas\projects\web\shop`),
    /// and the Issue tab's "open the settings" button, which looks the folder
    /// up in the desk's list, then found nothing and dropped the person on the
    /// desk's own page (2026-09-18, a user's share). Written here with a path
    /// that walks back up, because that is the one spelling every machine
    /// resolves the same way
    #[test]
    fn a_project_keeps_the_folder_the_settings_wrote() {
        let git = |dir: &std::path::Path, args: &[&str]| {
            std::process::Command::new("git").current_dir(dir).args(args).output().ok().filter(|o| o.status.success())
        };
        let root = std::env::temp_dir().join(format!("shikisha-source-at-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let main = root.join("proj");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(root.join("nest")).unwrap();
        if git(&main, &["init", "-q", "-b", "main"]).is_none() {
            return;
        }
        // The same folder, named the long way round
        let written = root.join("nest").join("..").join("proj");
        let desk = crate::config::Desk {
            folders: vec![crate::config::Folder { cwd: Some(written.clone()), ..Default::default() }],
            ..Default::default()
        };
        let found = super::desk_sources(&desk);
        assert_eq!(found.len(), 1, "the desk's one repository was not found");
        let s = &found[0];
        assert_eq!(s.at, written, "the folder the settings wrote was not kept");
        assert_eq!(
            std::fs::canonicalize(&s.dir).ok(),
            std::fs::canonicalize(&main).ok(),
            "the checkout is not the folder git would run in",
        );
        assert_ne!(s.dir, s.at, "the two spellings were not told apart, so this proves nothing");
        assert_eq!(s.name, "proj", "the project is not named after its checkout");
        let _ = std::fs::remove_dir_all(&root);
    }

    use super::*;

    fn repo() -> Repo {
        Repo::parse("styleio/ShikishaTerm").unwrap()
    }

    #[test]
    fn a_list_asks_github_for_exactly_this_repository() {
        let q = |state: &str, mine, review, text: &str| Query {
            state: state.into(),
            mine,
            review,
            text: text.into(),
            page: 1,
        };
        assert_eq!(
            search_q(&repo(), false, &q("", false, false, "")),
            "repo:styleio/ShikishaTerm is:issue is:open"
        );
        assert_eq!(
            search_q(&repo(), false, &q("all", true, false, "crash log")),
            "repo:styleio/ShikishaTerm is:issue assignee:@me crash log"
        );
        assert_eq!(
            search_q(&repo(), true, &q("closed", false, false, "")),
            "repo:styleio/ShikishaTerm is:pr is:closed -is:merged"
        );
        assert_eq!(
            search_q(&repo(), true, &q("merged", true, true, "")),
            "repo:styleio/ShikishaTerm is:pr is:merged author:@me review-requested:@me"
        );
        // An issue has no "merged": it is closed
        assert_eq!(
            search_q(&repo(), false, &q("merged", false, false, "")),
            "repo:styleio/ShikishaTerm is:issue is:open"
        );
        assert!(Repo::parse("no-slash").is_none());
        assert!(Repo::parse("a/b c").is_none());
    }

    #[test]
    fn a_worktree_is_named_after_what_it_is_for() {
        assert_eq!(
            workspace_name(false, 41, "Fix the tab that never starts"),
            "issue-41-fix-the-tab-that-never-starts"
        );
        assert_eq!(
            workspace_name(false, 41, "Issue #41: Fix it (#41)"),
            "issue-41-fix-it"
        );
        assert_eq!(
            workspace_name(true, 7, "feat: Don't/ break_the.build"),
            "pr-7-feat-dont-break_the.build"
        );
        // Nothing a branch can hold: the kind and the number are the name
        assert_eq!(
            workspace_name(false, 3, "起動できないタブの表示"),
            "issue-3"
        );
        assert!(workspace_name(false, 1, &"word ".repeat(40)).len() <= "issue-1-".len() + 48);
        assert_eq!(
            workspace_name(false, 288, "Empty content in base=binary produces broken table"),
            "issue-288-empty-content-in-base-binary-produces-broken",
            "a long title is cut between words"
        );
    }

    #[test]
    fn only_an_address_on_the_web_is_opened() {
        assert!(openable_link("https://github.com/owner/repo/issues/12"));
        assert!(openable_link("http://example.com/a?b=c&d=%20"));
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "C:\\Windows\\System32\\calc.exe",
            "javascript:alert(1)",
            "ms-settings:",
            "https://",
            "https://example.com/a b",
            "https://example.com/\"&calc",
            "https://example.com/\n",
        ] {
            assert!(!openable_link(bad), "{bad}");
        }
    }

    #[test]
    fn only_an_account_refusal_points_at_the_settings() {
        let (_, account) = refusal(401, "Bad credentials");
        assert!(account);
        let (_, account) = refusal(
            422,
            "Validation Failed The listed users and repositories cannot be searched",
        );
        assert!(account);
        let (_, account) = refusal(403, "API rate limit exceeded for user");
        assert!(!account, "waiting is the fix for a limit, not the settings");
        let (_, account) = refusal(502, "Server Error");
        assert!(!account);
        let e: anyhow::Error = AccountTrouble("no token".into()).into();
        assert!(
            settings_fix(&e.context("while listing")),
            "a reason wrapped on the way up is still the account's"
        );
        assert!(!settings_fix(&anyhow!("unreachable")));
    }

    #[test]
    fn only_the_review_threads_left_open_are_work() {
        let page = json!({"nodes": [
            {"isResolved": true, "isOutdated": false, "path": "a.rs", "line": 3, "startLine": null, "originalLine": 3,
             "comments": {"nodes": [{"author": {"login": "rev"}, "body": "done already"}]}},
            {"isResolved": false, "isOutdated": false, "path": "a.rs", "line": 12, "startLine": 10, "originalLine": 12,
             "comments": {"nodes": [{"author": {"login": "rev"}, "body": "name this better"}, {"author": {"login": "me"}, "body": "which name?"}]}},
            {"isResolved": false, "isOutdated": true, "path": "b.rs", "line": null, "startLine": null, "originalLine": 7,
             "comments": {"nodes": [{"author": {"login": "rev"}, "body": "off by one"}]}},
        ]});
        let open = open_threads(&page, &[]);
        assert_eq!(open.len(), 2, "a resolved thread was handed on: {open:?}");
        assert_eq!(open[0]["path"], "a.rs");
        assert_eq!((open[0]["from"].clone(), open[0]["line"].clone()), (json!(10), json!(12)), "the lines it is about");
        assert_eq!(open[0]["said"][1], json!({"who": "me", "said": "which name?"}), "the thread lost its order");
        assert_eq!(open[1]["line"], 7, "a thread whose line moved away says where it was");
        assert_eq!(open[1]["outdated"], true);
        assert!(open[0].get("outdated").is_none());
    }

    /// A bot that reviews every push does not resolve its own threads: one on
    /// code changed since is done with once the bot has put a 👍 on the newest
    /// commit. A person's thread, or one the bot has not said is done, stays
    #[test]
    fn a_bots_outdated_thread_is_done_once_its_thumb_is_on_the_newest_commit() {
        let thread = |outdated: bool, who: &str, kind: &str| {
            json!({"isResolved": false, "isOutdated": outdated, "path": "a.py", "line": 3, "startLine": null, "originalLine": 3,
                   "comments": {"nodes": [{"id": "c", "author": {"login": who, "__typename": kind}, "body": "x"}]}})
        };
        let page = json!({"nodes": [
            thread(true, "review-bot", "Bot"), thread(false, "review-bot", "Bot"), thread(true, "alice", "User"),
        ]});
        let left = open_threads(&page, &["review-bot".into()]);
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left[0].get("outdated").is_none() && left[1]["said"][0]["who"] == "alice");
        assert_eq!(open_threads(&page, &[]).len(), 3, "no 👍 from the bot: all of it is still open");
        let pull = |thumb_at: &str| {
            json!({"commits": {"nodes": [{"commit": {"committedDate": "2026-11-10T03:14:38Z"}}]},
                   "reactions": {"nodes": [{"createdAt": thumb_at, "user": {"login": "review-bot[bot]"}}]}})
        };
        assert_eq!(clean_since_head(&pull("2026-11-10T03:17:07Z")), ["review-bot"]);
        assert!(clean_since_head(&pull("2026-11-10T03:00:00Z")).is_empty(), "the 👍 was of the commit before");
    }

    #[test]
    fn a_check_counts_as_what_it_means() {
        assert_eq!(check_verdict(false, ""), "pending");
        assert_eq!(check_verdict(true, "failure"), "failed");
        assert_eq!(check_verdict(true, "skipped"), "passed");
        assert_eq!(check_verdict(true, "neutral"), "neutral");
    }

    #[test]
    fn a_row_says_merged_for_a_merged_pull_request() {
        let merged = json!({"number": 5, "state": "closed", "pull_request": {"merged_at": "2026-09-14T00:00:00Z"}});
        assert_eq!(row(&merged, true)["state"], "merged");
        let closed = json!({"number": 6, "state": "closed", "pull_request": {"merged_at": null}});
        assert_eq!(row(&closed, true)["state"], "closed");
        assert_eq!(encode("repo:a/b is:open"), "repo%3Aa%2Fb%20is%3Aopen");
    }
}
