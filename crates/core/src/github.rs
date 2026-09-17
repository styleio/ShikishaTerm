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
    use crate::config::GitUse;
    let slug = crate::repo::origin_of(dir).ok_or_else(|| {
        anyhow!(crate::i18n::tp(
            "err.github.not_github",
            &[("p", &dir.display().to_string())]
        ))
    })?;
    let repo = Repo::parse(&slug)
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.github.not_github", &[("p", &slug)])))?;
    let token = match git {
        GitUse::Unset => bail!(AccountTrouble(crate::i18n::t("err.github.account.unset"))),
        GitUse::Missing(name) => bail!(AccountTrouble(crate::i18n::tp(
            "err.git.account.missing",
            &[("name", name)]
        ))),
        GitUse::Pc(login) => crate::pr::pc_token(login.as_deref()).map_err(|why| {
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
        })?,
        GitUse::Account { desk, spec } => {
            if spec.host() != crate::config::GITHUB_HOST {
                bail!(AccountTrouble(crate::i18n::tp(
                    "err.github.host",
                    &[("name", &spec.name), ("host", &spec.host())]
                )));
            }
            spec.token(desk, look).ok_or_else(|| AccountTrouble(spec.no_token_said()))?
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

impl Hub {
    pub fn new(token: String) -> Hub {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            // The answer to a refusal says why; read it rather than turning it
            // into a status code nobody can act on
            .http_status_as_error(false)
            .build()
            .new_agent();
        Hub { agent, token }
    }

    /// One request. The reply's JSON, or GitHub's own reason for saying no
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
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
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return Ok(v);
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
            let verdict = match s.get("state").and_then(|x| x.as_str()).unwrap_or_default() {
                "success" => "passed",
                "pending" => "pending",
                _ => "failed",
            };
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

    /// The end of a GitHub Actions job's log: where a failure says what failed.
    /// GitHub answers with a redirect to the file, which is followed
    pub fn job_log_tail(&self, repo: &Repo, job: u64) -> Result<String> {
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
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            bail!(crate::i18n::tp("err.github.other", &[("status", &status.to_string()), ("said", text.trim())]));
        }
        Ok(log_tail(&text))
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
        Ok(json!({"number": v.get("number"), "url": v.get("html_url")}))
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

    /// The labels an issue can be given
    pub fn labels(&self, repo: &Repo) -> Result<Vec<String>> {
        let v = self.call(
            "GET",
            &format!("repos/{}/labels?per_page=100", repo.slug()),
            None,
        )?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.get("name")?.as_str().map(str::to_string))
            .collect())
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
    /// `owner/name` on GitHub, when that is where it lives
    pub repo: Option<String>,
    pub git: crate::config::GitUse,
}

/// The desk's repositories, one each: every folder on this machine, gathered
/// by the checkout it belongs to, named the way the settings name its project
pub fn desk_sources(desk: &crate::config::Desk) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
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
        out.push(Source {
            name: project.unwrap_or_else(|| {
                main.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            }),
            repo: crate::repo::origin_of(&main),
            dir: main,
            git,
        });
    }
    out
}

/// The folder the page named, when it is a folder of the project it named: the
/// checkout or a worktree cut from it. Anything else is not a place git is run
/// for a request from the page
pub fn project_folder(sources: &[Source], project: &str, folder: &str) -> Option<std::path::PathBuf> {
    let dir = std::path::PathBuf::from(folder);
    let fam = crate::repo::family_of(&dir)?;
    sources
        .iter()
        .any(|s| s.name == project && crate::repo::family_of(&s.dir).as_deref() == Some(fam.as_path()))
        .then_some(dir)
}

/// The folder on this PC where a pull request's branch is checked out: the
/// project's checkout itself, or any worktree cut from it. None when no folder
/// stands on that branch -- one is made for it first
pub fn head_folder(checkout: &std::path::Path, head: &str) -> Option<std::path::PathBuf> {
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
        .find(|(_, b)| b.as_deref() == Some(head))
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
        // Reading a pull request's failed checks, to hand them to an AI tab
        ("ci_fix", _) => "github_pr",
        ("detail", false) => "github_issue",
        ("detail", true) => "github_pr",
        ("options", _) => "github_labels",
        ("create", _) => "github_issue_create",
        ("create_pr", _) => "github_pr_create",
        // The branches a pull request can go into are read off this PC's copy
        ("pr_bases", _) => "git_branches",
        // What it would carry is a diff on this PC
        ("pr_files", _) | ("pr_file", _) => "git_diff",
        ("comment", _) => "github_comment",
        ("issue_state", _) => "github_issue_state",
        ("pr_state", _) => "github_pr_state",
        ("merge", _) => "github_pr_merge",
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
    let base = json!({"act": act, "kind": if pulls { "pr" } else { "issue" }, "project": wanted,
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
        let (repo, token) = target(&s.dir, &s.git, look)?;
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
    // Read here, not asked of GitHub: the branches this PC knows the server
    // has, the one its server calls the default first
    if act == "pr_bases" {
        let mut bases: Vec<String> = Vec::new();
        for b in crate::worktree::bases(&source.dir) {
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
    let done = hub_for(source).and_then(|(repo, hub)| match act {
        "detail" if pulls => hub.pull(&repo, number),
        "detail" => hub.issue(&repo, number),
        // With what CI says of the commit the newest open one is at: the same
        // commit for every base the branch was sent to
        "branch_prs" => hub.branch_pulls(&repo, &s("head")).map(|prs| {
            let sha = prs
                .as_array()
                .and_then(|a| a.iter().find(|p| p["state"] == "open"))
                .and_then(|p| p["sha"].as_str())
                .map(str::to_string);
            let checks = sha.as_deref().and_then(|sha| {
                hub.checks(&repo, sha).ok().map(|mut c| {
                    c["sha"] = json!(sha);
                    c
                })
            });
            json!({"prs": prs, "checks": checks})
        }),
        "options" => Ok(json!({"labels": hub.labels(&repo)?, "assignees": hub.assignees(&repo)?})),
        "create" => hub.create_issue(
            &repo,
            &s("title"),
            &s("body"),
            &list("labels"),
            &list("assignees"),
        ),
        "create_pr" => hub.create_pull(
            &repo,
            &s("title"),
            &s("body"),
            &s("head"),
            &s("base"),
            args.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        ),
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
        other => Err(anyhow!("unknown request {other}")),
    });
    match done {
        Ok(data) => with(base, json!({"ok": true, "number": number, "data": data})),
        Err(e) => with(
            base,
            json!({"ok": false, "number": number, "error": format!("{e:#}")}),
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
/// How much of a failed job's log is handed on: the lines before its last
/// error, where a failure says what failed -- not the clean-up after it
const LOG_TAIL_LINES: usize = 80;
const LOG_TAIL_CHARS: usize = 6000;

/// The part of a job's log worth reading: GitHub's time stamps taken off,
/// everything after the last error dropped, the last lines of what is left
pub fn log_tail(log: &str) -> String {
    let lines: Vec<&str> = log
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
    let (repo, token) = target(&source.dir, &source.git, look)?;
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

fn check_verdict(done: bool, conclusion: &str) -> &'static str {
    match (done, conclusion) {
        (false, _) => "pending",
        (
            true,
            "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure" | "stale",
        ) => "failed",
        (true, "success" | "skipped") => "passed",
        _ => "neutral",
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
        _ => (
            crate::i18n::tp(
                "err.github.other",
                &[("status", &status.to_string()), ("said", said)],
            ),
            false,
        ),
    }
}

/// A search written into a query string
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
        assert!(is(super::head_folder(&main, "main"), &main), "the checkout was not found on its own branch");
        assert!(is(super::head_folder(&cut, "feature"), &cut), "a worktree was not found from itself");
        assert!(is(super::head_folder(&main, "feature"), &cut), "a worktree was not found from the checkout");
        assert_eq!(super::head_folder(&main, "elsewhere"), None);
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
