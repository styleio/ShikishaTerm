//! A GitHub Project: the board a team keeps its issues and pull requests on.
//!
//! Named `ghproject` here, since "project" alone is already the desk's own
//! word for a repository it works in (and "board" is the desk's own screen).
//! A GitHub Project belongs to a person or an organization, not
//! to a repository, and holds cards from any number of repositories; it is
//! reached through a project of the desk all the same, read as that project's
//! account -- the account the desk already chose for the repository the board
//! is about.
//!
//! What is read is what GitHub keeps: the board's fields, its saved views and
//! its cards. A view's filter is GitHub's own search and is handed to GitHub as
//! written, never read here. What is written is a card: its value in a field,
//! its place among the others, a new one (a draft, or an issue or pull request
//! put on the board), a draft's words or the issue made of it, and a card put
//! away (archived, or taken off the board) -- and the board itself: its
//! views, its fields and their choices, its name and words, the reports of
//! how it stands, who may see it and what it is linked to.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::github::{AccountTrouble, Hub, Repo};

/// Where a board is: whose it is and its number there. `org` is None while
/// that is not known yet (written as `owner/number`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhRef {
    pub owner: String,
    pub org: Option<bool>,
    pub number: u64,
    /// The view the address named, if it named one
    pub view: Option<u64>,
}

impl GhRef {
    /// A board's address as GitHub's page shows it
    /// (`https://github.com/orgs/<owner>/projects/<n>`, `/users/` for a
    /// person's, with or without `/views/<v>`), or `owner/number`
    pub fn parse(text: &str) -> Option<GhRef> {
        let text = text.trim().trim_end_matches('/');
        let owner_ok = |s: &str| {
            !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        let number = |s: &str| s.parse::<u64>().ok().filter(|n| *n > 0);
        if let Some(path) = text.strip_prefix("https://github.com/").or_else(|| text.strip_prefix("github.com/")) {
            // Only the address itself: what follows a `?` or `#` is the page's own
            let path = path.split(['?', '#']).next().unwrap_or_default();
            let parts: Vec<&str> = path.split('/').collect();
            let (kind, owner, n, view) = match parts.as_slice() {
                [kind, owner, "projects", n] => (*kind, *owner, *n, None),
                [kind, owner, "projects", n, "views", v] => (*kind, *owner, *n, Some(number(v)?)),
                _ => return None,
            };
            let org = match kind {
                "orgs" => true,
                "users" => false,
                _ => return None,
            };
            return owner_ok(owner).then_some(())
                .and(number(n))
                .map(|number| GhRef { owner: owner.to_string(), org: Some(org), number, view });
        }
        let (owner, n) = text.split_once('/')?;
        (owner_ok(owner) && !n.contains('/'))
            .then_some(())
            .and(number(n))
            .map(|number| GhRef { owner: owner.to_string(), org: None, number, view: None })
    }

    /// The board's page on GitHub
    pub fn url(&self) -> String {
        let kind = if self.org == Some(false) { "users" } else { "orgs" };
        format!("https://github.com/{kind}/{}/projects/{}", self.owner, self.number)
    }
}

/// How many cards one look at a view reads at most. GitHub hands out a
/// hundred at a time; ten of those is a board a team reads by its views, not
/// card by card, and stays well inside the hour's allowance of the account
/// (each page costs about one of its 5000 points)
pub const MOST_CARDS: u64 = 1000;
const CARDS_PER_PAGE: u64 = 100;
/// The most cards a read can be asked to go on to, a thousand at a time: two
/// hundred requests, which is as far as an hour's allowance at GitHub is
/// worth spending on one board
pub const CARDS_CEILING: u64 = 20_000;

/// What a read of a board is asked for beyond its address
#[derive(Debug, Clone, Copy, Default)]
pub struct Want<'a> {
    /// The view's number; None is the address's view, else the first
    pub view: Option<u64>,
    /// Instead of the view's own filter
    pub filter: Option<&'a str>,
    /// How many cards to read up to (`MOST_CARDS` when None)
    pub most: Option<u64>,
    /// The cards in the board's archive, rather than those on it
    pub archived: bool,
}

/// What is shown of a board in a list of them
const SUMMARY: &str = "id number title url closed owner{__typename ... on Organization{login} ... on User{login}} items{totalCount}";

/// How many boards each place is asked for. Thirty is more than a team keeps
/// open at once; a board past them is reached by its address
const LISTED_EACH: u32 = 30;

/// The boards a repository's account can see that matter to it: those linked
/// to the repository first, then its owner's, then the account's own. Each
/// once, in that order
pub fn list(hub: &Hub, repo: &Repo) -> Result<Value> {
    let each = format!("projectsV2(first:{LISTED_EACH},orderBy:{{field:UPDATED_AT,direction:DESC}}){{nodes{{{SUMMARY}}}}}");
    let query = format!(
        "query($owner:String!,$name:String!){{repository(owner:$owner,name:$name){{{each}}} \
         repositoryOwner(login:$owner){{__typename ... on Organization{{{each}}} ... on User{{{each}}}}} viewer{{login {each}}}}}"
    );
    let v = hub.graphql_whole(&query, json!({"owner": repo.owner, "name": repo.name}))?;
    read_list(&v)
}

/// The list read out of GitHub's answer. A board the token may not see comes
/// back as nothing, with GitHub saying why beside it: when that is every one
/// of them, the reason is the answer
pub fn read_list(v: &Value) -> Result<Value> {
    // Refused whole -- a token without the scope for it gets no data at all
    if v.get("data").is_none_or(|d| d.is_null()) {
        return Err(refused(v));
    }
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let places = [
        ("/data/repository/projectsV2/nodes", "repository"),
        ("/data/repositoryOwner/projectsV2/nodes", "owner"),
        ("/data/viewer/projectsV2/nodes", "mine"),
    ];
    let mut hidden = 0usize;
    for (at, from) in places {
        for node in v.pointer(at).and_then(|n| n.as_array()).into_iter().flatten() {
            if node.is_null() {
                hidden += 1;
                continue;
            }
            let id = node["id"].as_str().unwrap_or_default().to_string();
            if id.is_empty() || seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            let owner = node.pointer("/owner/login").and_then(|o| o.as_str()).unwrap_or_default();
            let org = node.pointer("/owner/__typename").and_then(|t| t.as_str()) == Some("Organization");
            let r = GhRef { owner: owner.to_string(), org: Some(org), number: node["number"].as_u64().unwrap_or(0), view: None };
            out.push(json!({
                "id": id, "number": r.number, "title": node["title"], "owner": owner, "org": org,
                "url": node["url"].as_str().map(str::to_string).unwrap_or_else(|| r.url()),
                "closed": node["closed"].as_bool().unwrap_or(false),
                "cards": node.pointer("/items/totalCount").cloned().unwrap_or(json!(0)),
                "from": from,
            }));
        }
    }
    if out.is_empty() && hidden > 0 {
        return Err(refused(v));
    }
    Ok(json!({"projects": out, "hidden": hidden}))
}

/// Why GitHub would not show a board, in words a person can act on. A token
/// without the Projects permission is the account's to fix, and the settings
/// are offered for it; an organization's own rule is said in GitHub's words
fn refused(v: &Value) -> anyhow::Error {
    let errors: Vec<&Value> = v.get("errors").and_then(|e| e.as_array()).into_iter().flatten().collect();
    let said = |e: &Value| e["message"].as_str().unwrap_or_default().to_string();
    // An organization that turns tokens away by their own rule says which
    if let Some(e) = errors.iter().find(|e| said(e).contains("organization forbids")) {
        return anyhow!(AccountTrouble(said(e)));
    }
    if errors.iter().any(|e| {
        let kind = e["type"].as_str().unwrap_or_default();
        kind == "INSUFFICIENT_SCOPES" || (kind == "FORBIDDEN" && said(e).contains("not accessible"))
    }) {
        return anyhow!(AccountTrouble(crate::i18n::t("err.ghproject.denied")));
    }
    if errors.iter().any(|e| e["type"] == "NOT_FOUND") {
        return anyhow!(crate::i18n::t("err.ghproject.not_found"));
    }
    match errors.first() {
        Some(e) => anyhow!(said(e)),
        None => anyhow!(crate::i18n::t("err.ghproject.not_found")),
    }
}

/// Whether the owner is an organization or a person, for an address that did
/// not say (`owner/number`)
fn owner_is_org(hub: &Hub, owner: &str) -> Result<bool> {
    let v = hub.graphql_whole("query($owner:String!){repositoryOwner(login:$owner){__typename}}", json!({"owner": owner}))?;
    match v.pointer("/data/repositoryOwner/__typename").and_then(|t| t.as_str()) {
        Some("Organization") => Ok(true),
        Some(_) => Ok(false),
        None => Err(refused(&v)),
    }
}

const FIELD: &str = "__typename ... on ProjectV2FieldCommon{id name dataType} \
    ... on ProjectV2SingleSelectField{options{id name color description}} \
    ... on ProjectV2MultiSelectField{multiSelectOptions{id name color description}} \
    ... on ProjectV2IterationField{configuration{duration iterations{id title startDate duration} completedIterations{id title startDate duration}}}";
const FIELD_ID: &str = "... on ProjectV2FieldCommon{id}";

/// A board, as one view of it shows it: its fields, its views, which view
/// this is, and its cards as that view's filter (or `want.filter`, when
/// given) picks them -- the first thousand, or as many as `want.most` says,
/// with `total` saying how many there are and `more` whether some are left
pub fn read(hub: &Hub, at: &GhRef, want: Want) -> Result<Value> {
    let (view, filter) = (want.view, want.filter);
    let org = match at.org {
        Some(o) => o,
        None => owner_is_org(hub, &at.owner)?,
    };
    let root = if org { "organization" } else { "user" };
    let shape = format!(
        "query($owner:String!,$number:Int!){{{root}(login:$owner){{projectV2(number:$number){{id number title url closed public shortDescription viewerCanUpdate \
         fields(first:50){{nodes{{{FIELD}}}}} \
         views(first:30){{nodes{{id number name layout filter fields(first:50){{nodes{{{FIELD_ID}}}}} \
         groupByFields(first:1){{nodes{{{FIELD_ID}}}}} verticalGroupByFields(first:1){{nodes{{{FIELD_ID}}}}} \
         sortByFields(first:5){{nodes{{direction field{{{FIELD_ID}}}}}}}}}}}}}}}}}"
    );
    let v = hub.graphql_whole(&shape, json!({"owner": at.owner, "number": at.number}))?;
    let Some(node) = v.pointer(&format!("/data/{root}/projectV2")).filter(|n| !n.is_null()) else {
        return Err(refused(&v));
    };
    let mut out = read_shape(node);
    let views = out["views"].as_array().cloned().unwrap_or_default();
    let wanted = view.or(at.view);
    let chosen = views
        .iter()
        .find(|w| wanted.is_some() && w["number"].as_u64() == wanted)
        .or(views.first())
        .cloned()
        .unwrap_or(Value::Null);
    let filter = filter.map(str::to_string).unwrap_or_else(|| chosen["filter"].as_str().unwrap_or_default().to_string());
    let most = want.most.unwrap_or(MOST_CARDS).clamp(1, CARDS_CEILING);
    let (cards, total, more) = cards(hub, root, at, &filter, most, want.archived)?;
    out["view"] = chosen["number"].clone();
    out["filter"] = json!(filter);
    out["columns"] = json!(column_field(&out["fields"], &chosen));
    out["cards"] = json!(cards);
    out["total"] = json!(total);
    out["more"] = json!(more);
    out["archived"] = json!(want.archived);
    out["org"] = json!(org);
    out["owner"] = json!(at.owner);
    Ok(out)
}

/// The board's fields and views, read out of GitHub's answer
pub fn read_shape(node: &Value) -> Value {
    let fields: Vec<Value> = node
        .pointer("/fields/nodes")
        .and_then(|n| n.as_array())
        .into_iter()
        .flatten()
        .filter(|f| f["id"].is_string())
        .map(|f| {
            let mut field = json!({"id": f["id"], "name": f["name"], "type": f["dataType"]});
            // The choices of a field that takes one of them, or several
            if let Some(options) = f.get("options").or_else(|| f.get("multiSelectOptions")).and_then(|o| o.as_array()) {
                field["options"] = json!(options.iter().map(|o| json!({"id": o["id"], "name": o["name"], "color": o["color"],
                    "description": o["description"].as_str().unwrap_or_default()})).collect::<Vec<_>>());
            }
            if let Some(c) = f.get("configuration") {
                // How long a new iteration of it runs
                field["days"] = c["duration"].clone();
                // Those over first, in the order they ran, then the one now
                // and those to come: the order a board's columns are in
                let mut iterations: Vec<Value> = Vec::new();
                for (at, done) in [("completedIterations", true), ("iterations", false)] {
                    let mut these: Vec<Value> = c.get(at).and_then(|i| i.as_array()).cloned().unwrap_or_default();
                    these.sort_by(|a, b| a["startDate"].as_str().cmp(&b["startDate"].as_str()));
                    for i in these {
                        iterations.push(json!({"id": i["id"], "title": i["title"], "start": i["startDate"],
                            "days": i["duration"], "done": done}));
                    }
                }
                field["iterations"] = json!(iterations);
            }
            field
        })
        .collect();
    let ids = |w: &Value, at: &str| -> Vec<Value> {
        w.pointer(at).and_then(|n| n.as_array()).into_iter().flatten().filter_map(|f| f.get("id").cloned()).collect()
    };
    let views: Vec<Value> = node
        .pointer("/views/nodes")
        .and_then(|n| n.as_array())
        .into_iter()
        .flatten()
        .map(|w| {
            let sort: Vec<Value> = w
                .pointer("/sortByFields/nodes")
                .and_then(|n| n.as_array())
                .into_iter()
                .flatten()
                .filter_map(|s| Some(json!({"field": s.pointer("/field/id")?, "desc": s["direction"] == "DESC"})))
                .collect();
            json!({
                "id": w["id"], "number": w["number"], "name": w["name"],
                "layout": match w["layout"].as_str() {
                    Some("BOARD_LAYOUT") => "board",
                    Some("ROADMAP_LAYOUT") => "roadmap",
                    _ => "table",
                },
                "filter": w["filter"].as_str().unwrap_or_default(),
                "fields": ids(w, "/fields/nodes"),
                "group": ids(w, "/groupByFields/nodes").first(),
                "lanes": ids(w, "/verticalGroupByFields/nodes").first(),
                "sort": sort,
            })
        })
        .collect();
    json!({"id": node["id"], "number": node["number"], "title": node["title"], "url": node["url"],
        "closed": node["closed"].as_bool().unwrap_or(false),
        "public": node["public"].as_bool().unwrap_or(false),
        "description": words(&node["shortDescription"]),
        // Whether this account may change the board itself (its views, its fields)
        "can_update": node["viewerCanUpdate"].as_bool().unwrap_or(false),
        "fields": fields, "views": views})
}

/// The field a board view's columns are: the one the view itself puts its
/// cards in columns by, when that is a choice or an iteration; else the field
/// named Status, which every new board has; else its first choice field. None
/// when the board has no field cards can be put in columns by
pub fn column_field(fields: &Value, view: &Value) -> Option<String> {
    let all: Vec<&Value> = fields.as_array().into_iter().flatten().collect();
    let groups = |f: &&Value| matches!(f["type"].as_str(), Some("SINGLE_SELECT") | Some("ITERATION"));
    let by_id = |id: &Value| all.iter().find(|f| f["id"] == *id).copied();
    view.get("lanes")
        .filter(|l| !l.is_null())
        .and_then(by_id)
        .filter(groups)
        .or_else(|| {
            all.iter()
                .find(|f| f["type"] == "SINGLE_SELECT" && f["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("status")))
                .copied()
        })
        .or_else(|| all.iter().find(|f| f["type"] == "SINGLE_SELECT").copied())
        .and_then(|f| f["id"].as_str().map(str::to_string))
}

/// The cards query. `rich` asks for what only newer parts of GitHub know (an
/// issue's type, its parent and how far its sub-issues are, a field with
/// several choices, the people asked to review, the pull requests linked):
/// asked first, and left out when GitHub will not answer with them.
///
/// The pull requests linked to an issue and the people asked to review a
/// pull request are asked of the issue and the pull request themselves, not
/// of the board's fields that show them: GitHub prices a question by the
/// lists inside lists it could have to walk, and a list inside each of fifty
/// field values of each of a hundred cards made one page of cards cost a
/// fiftieth of an hour's allowance (measured 2026-11-14), where this costs
/// a few hundredths of that
fn cards_query(root: &str, rich: bool) -> String {
    let people = "assignees(first:10){nodes{login}}";
    let labels = "labels(first:20){nodes{name color}}";
    let (issue, pull, more) = if rich {
        (
            "issueType{name} parent{number title url repository{nameWithOwner}} subIssuesSummary{total completed percentCompleted} \
             closedByPullRequestsReferences(first:5,includeClosedPrs:true){nodes{number url state isDraft repository{nameWithOwner}}}",
            "reviewRequests(first:10){nodes{requestedReviewer{__typename ... on User{login} ... on Bot{login} ... on Mannequin{login}}}} \
             latestReviews(first:10){nodes{author{login}}}",
            format!(
                "... on ProjectV2ItemFieldMultiSelectValue{{options{{id name}} field{{{FIELD_ID}}}}} \
                 ... on ProjectV2ItemIssueFieldValue{{field{{{FIELD_ID}}} issueFieldValue{{__typename ... on IssueFieldTextValue{{value}} ... on IssueFieldDateValue{{value}} \
                 ... on IssueFieldNumberValue{{amount:value}} ... on IssueFieldSingleSelectValue{{name}} ... on IssueFieldMultiSelectValue{{options{{name}}}}}}}}"
            ),
        )
    } else {
        ("", "", String::new())
    };
    format!(
        "query($owner:String!,$number:Int!,$after:String,$filter:String,$states:[ProjectV2ItemArchivedState!]){{{root}(login:$owner){{projectV2(number:$number){{\
         items(first:{CARDS_PER_PAGE},after:$after,query:$filter,archivedStates:$states,orderBy:{{field:POSITION,direction:ASC}}){{totalCount pageInfo{{hasNextPage endCursor}} \
         nodes{{id type isArchived content{{__typename \
         ... on Issue{{id number title url state stateReason updatedAt createdAt closedAt repository{{nameWithOwner}} {people} {labels} milestone{{title}} {issue}}} \
         ... on PullRequest{{id number title url state isDraft updatedAt createdAt closedAt repository{{nameWithOwner}} {people} {labels} milestone{{title}} {pull}}} \
         ... on DraftIssue{{id title body updatedAt createdAt {people}}}}} \
         fieldValues(first:50){{nodes{{__typename \
         ... on ProjectV2ItemFieldTextValue{{text field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldNumberValue{{number field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldDateValue{{date field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldSingleSelectValue{{optionId name field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldIterationValue{{iterationId title startDate duration field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldMilestoneValue{{milestone{{title}} field{{{FIELD_ID}}}}} {more}}}}}}}}}}}}}}}"
    )
}

/// The cards the filter picks, in the board's own order, up to `most` of
/// them: with how many there are in all, and whether some were left unread.
/// `archived` reads the board's archive instead
fn cards(hub: &Hub, root: &str, at: &GhRef, filter: &str, most: u64, archived: bool) -> Result<(Vec<Value>, u64, bool)> {
    let filter = (!filter.trim().is_empty()).then(|| filter.trim().to_string());
    let states = if archived { json!(["ARCHIVED"]) } else { json!(["NOT_ARCHIVED"]) };
    let mut rich = true;
    let mut query = cards_query(root, rich);
    let mut out = Vec::new();
    let mut after = Value::Null;
    let mut total;
    loop {
        let vars = json!({"owner": at.owner, "number": at.number, "after": after, "filter": filter, "states": states});
        let mut v = hub.graphql_whole(&query, vars.clone())?;
        // Refused whole on the first page: once more without the newer parts,
        // whose word decides (a token that may not see the board is refused
        // either way, and said to be)
        if rich && out.is_empty() && v.get("data").is_none_or(|d| d.is_null()) {
            rich = false;
            query = cards_query(root, rich);
            v = hub.graphql_whole(&query, vars)?;
        }
        let Some(items) = v.pointer(&format!("/data/{root}/projectV2/items")).filter(|i| !i.is_null()) else {
            return Err(refused(&v));
        };
        total = items["totalCount"].as_u64().unwrap_or(0);
        out.extend(
            items.pointer("/nodes").and_then(|n| n.as_array()).into_iter().flatten().filter_map(read_card)
                .filter(|c| c["archived"].as_bool().unwrap_or(false) == archived),
        );
        match items.pointer("/pageInfo/endCursor") {
            Some(c) if items.pointer("/pageInfo/hasNextPage") == Some(&json!(true)) => {
                if out.len() as u64 >= most {
                    return Ok((out, total, true));
                }
                after = c.clone()
            }
            _ => break,
        }
    }
    Ok((out, total, false))
}

/// One card: what it is about, and its value in each field it has one in.
/// A card for something in a repository the account cannot see is still a
/// card, and says so; an archived one is not on the board
pub fn read_card(node: &Value) -> Option<Value> {
    node["id"].as_str()?;
    let c = &node["content"];
    let kind = match (node["type"].as_str(), c["__typename"].as_str()) {
        (_, Some("Issue")) => "issue",
        (_, Some("PullRequest")) => "pr",
        (_, Some("DraftIssue")) | (Some("DRAFT_ISSUE"), _) => "draft",
        _ => "hidden",
    };
    let names = |at: &str, key: &str| -> Vec<Value> {
        c.pointer(at).and_then(|n| n.as_array()).into_iter().flatten().filter_map(|x| x.get(key).cloned()).collect()
    };
    let labels: Vec<Value> = c
        .pointer("/labels/nodes")
        .and_then(|n| n.as_array())
        .into_iter()
        .flatten()
        .map(|l| json!({"name": l["name"], "color": l["color"]}))
        .collect();
    let mut values = serde_json::Map::new();
    for f in node.pointer("/fieldValues/nodes").and_then(|n| n.as_array()).into_iter().flatten() {
        let Some(field) = f.pointer("/field/id").and_then(|i| i.as_str()) else { continue };
        let value = match f["__typename"].as_str() {
            Some("ProjectV2ItemFieldTextValue") => json!({"text": f["text"]}),
            Some("ProjectV2ItemFieldNumberValue") => json!({"number": f["number"]}),
            Some("ProjectV2ItemFieldDateValue") => json!({"date": f["date"]}),
            Some("ProjectV2ItemFieldSingleSelectValue") => json!({"option": f["optionId"], "name": f["name"]}),
            Some("ProjectV2ItemFieldMilestoneValue") => json!({"milestone": f.pointer("/milestone/title")}),
            // Several choices of one field
            Some("ProjectV2ItemFieldMultiSelectValue") => json!({"options": f["options"].as_array().into_iter().flatten()
                .map(|o| json!({"id": o["id"], "name": o["name"]})).collect::<Vec<_>>()}),

            // A field the organization gives its issues, shown as it reads
            Some("ProjectV2ItemIssueFieldValue") => {
                let x = &f["issueFieldValue"];
                let shown = x["value"].as_str().map(str::to_string)
                    .or_else(|| x["amount"].as_f64().map(|n| n.to_string()))
                    .or_else(|| x["name"].as_str().map(str::to_string))
                    .or_else(|| x["options"].as_array().map(|o| {
                        o.iter().filter_map(|o| o["name"].as_str()).collect::<Vec<_>>().join(", ")
                    }));
                match shown {
                    Some(s) => json!({"shown": s}),
                    None => continue,
                }
            }
            Some("ProjectV2ItemFieldIterationValue") => {
                json!({"iteration": f["iterationId"], "title": f["title"], "start": f["startDate"], "days": f["duration"]})
            }
            _ => continue,
        };
        values.insert(field.to_string(), value);
    }
    let state = match (kind, c["state"].as_str(), c["isDraft"].as_bool()) {
        ("pr", Some("OPEN"), Some(true)) => "draft",
        (_, Some(s), _) => match s {
            "OPEN" => "open",
            "MERGED" => "merged",
            _ => "closed",
        },
        _ => "",
    };
    let mut card = json!({
        "id": node["id"], "kind": kind,
        "number": c["number"], "title": c["title"].as_str().unwrap_or_default(), "url": c["url"],
        "state": state, "repo": c.pointer("/repository/nameWithOwner"),
        "assignees": names("/assignees/nodes", "login"), "labels": labels,
        "updated": c["updatedAt"], "created": c["createdAt"], "closed_at": c["closedAt"], "values": values,
        "archived": node["isArchived"].as_bool().unwrap_or(false),
    });
    // What the issue or pull request itself says, which a board shows as
    // fields of its own: by its own id, which changes to it are made with
    if kind == "issue" || kind == "pr" {
        card["node"] = c["id"].clone();
        card["milestone"] = c.pointer("/milestone/title").cloned().unwrap_or(Value::Null);
    }
    // Who a pull request waits on for review and who has reviewed it, each
    // once; a team is not named to a token that may not read the
    // organization, and is said to be one
    if kind == "pr" {
        let mut people: Vec<Value> = Vec::new();
        for r in c.pointer("/reviewRequests/nodes").and_then(|n| n.as_array()).into_iter().flatten() {
            let who = match r.pointer("/requestedReviewer/login").and_then(|l| l.as_str()) {
                Some(login) => json!(login),
                None if r["requestedReviewer"].is_object() => json!({"team": true}),
                None => continue,
            };
            if !people.contains(&who) {
                people.push(who);
            }
        }
        for login in c.pointer("/latestReviews/nodes").and_then(|n| n.as_array()).into_iter().flatten().filter_map(|r| r.pointer("/author/login")) {
            if !people.contains(login) {
                people.push(login.clone());
            }
        }
        card["reviewers"] = json!(people);
    }
    if kind == "issue" {
        // The pull requests that would close it, as its page links them
        card["prs"] = json!(c.pointer("/closedByPullRequestsReferences/nodes").and_then(|n| n.as_array()).into_iter().flatten()
            .map(|p| json!({"number": p["number"], "url": p["url"], "repo": p.pointer("/repository/nameWithOwner"),
                "state": match (p["state"].as_str(), p["isDraft"].as_bool()) {
                    (Some("OPEN"), Some(true)) => "draft",
                    (Some("OPEN"), _) => "open",
                    (Some("MERGED"), _) => "merged",
                    _ => "closed",
                }})).collect::<Vec<_>>());
        card["type"] = c.pointer("/issueType/name").cloned().unwrap_or(Value::Null);
        if let Some(p) = c.get("parent").filter(|p| p.is_object()) {
            card["parent"] = json!({"number": p["number"], "title": p["title"], "url": p["url"], "repo": p.pointer("/repository/nameWithOwner")});
        }
        if let Some(s) = c.get("subIssuesSummary").filter(|s| s["total"].as_u64().unwrap_or(0) > 0) {
            card["sub"] = json!({"total": s["total"], "done": s["completed"], "percent": s["percentCompleted"]});
        }
    }
    // A draft is written here, not on GitHub's page of an issue: what it says,
    // and the draft's own id it is written to by
    if kind == "draft" {
        card["draft"] = c["id"].clone();
        card["body"] = json!(c["body"].as_str().unwrap_or_default());
    }
    Some(card)
}

/// A card's value in one field, set -- or taken off, for `value` null. The
/// value is said the way the field holds it: `{"option": id}`, `{"options":
/// [id, ..]}` (a field of several choices), `{"iteration": id}`, `{"text":
/// ..}`, `{"number": ..}` or `{"date": "YYYY-MM-DD"}`
pub fn set_value(hub: &Hub, board: &str, card: &str, field: &str, value: &Value) -> Result<()> {
    let ids = json!({"project": board, "item": card, "field": field});
    let v = if value.is_null() {
        hub.graphql_whole(
            "mutation($project:ID!,$item:ID!,$field:ID!){clearProjectV2ItemFieldValue(input:{projectId:$project,itemId:$item,fieldId:$field}){projectV2Item{id}}}",
            ids,
        )?
    } else {
        let mut vars = ids;
        vars["value"] = field_value(value)?;
        hub.graphql_whole(
            "mutation($project:ID!,$item:ID!,$field:ID!,$value:ProjectV2FieldValue!){updateProjectV2ItemFieldValue(input:{projectId:$project,itemId:$item,fieldId:$field,value:$value}){projectV2Item{id}}}",
            vars,
        )?
    };
    let done = v.pointer("/data/updateProjectV2ItemFieldValue/projectV2Item/id")
        .or_else(|| v.pointer("/data/clearProjectV2ItemFieldValue/projectV2Item/id"))
        .is_some_and(|i| !i.is_null());
    if !done {
        return Err(refused(&v));
    }
    Ok(())
}

/// The value as GitHub takes it. Anything else is refused here, before
/// GitHub is asked
pub fn field_value(value: &Value) -> Result<Value> {
    let bad = || anyhow!(crate::i18n::t("err.ghproject.bad_value"));
    let o = value.as_object().ok_or_else(bad)?;
    if let Some(id) = o.get("option").and_then(|i| i.as_str()) {
        return Ok(json!({"singleSelectOptionId": id}));
    }
    if let Some(ids) = o.get("options").and_then(|i| i.as_array()) {
        let ids: Vec<&str> = ids.iter().map(|i| i.as_str().or_else(|| i["id"].as_str()).ok_or_else(bad)).collect::<Result<_>>()?;
        return Ok(json!({"multiSelectOptionIds": ids}));
    }
    if let Some(id) = o.get("iteration").and_then(|i| i.as_str()) {
        return Ok(json!({"iterationId": id}));
    }
    if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
        return Ok(json!({"text": t}));
    }
    if let Some(n) = o.get("number") {
        let n = n.as_f64().or_else(|| n.as_str().and_then(|s| s.trim().parse().ok())).filter(|n| n.is_finite()).ok_or_else(bad)?;
        return Ok(json!({"number": n}));
    }
    if let Some(d) = o.get("date").and_then(|d| d.as_str()) {
        let ok = d.len() == 10
            && d.char_indices().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
        if !ok {
            return Err(bad());
        }
        return Ok(json!({"date": d}));
    }
    Err(bad())
}

/// What GitHub answered at `at`, or why it refused: a mutation GitHub did not
/// carry out has nothing there, and says why in its errors
fn carried<'a>(v: &'a Value, at: &str) -> Result<&'a Value> {
    match v.pointer(at) {
        Some(x) if !x.is_null() => Ok(x),
        _ => Err(refused(v)),
    }
}

/// An issue or pull request named the ways people name one: its page on
/// GitHub, or `owner/repo#12`. As the page's address, which GitHub can find
/// it by whichever it turns out to be
pub fn content_url(text: &str) -> Option<String> {
    let text = text.trim();
    let name_ok = |s: &str| !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let number_ok = |s: &str| !s.is_empty() && s.len() <= 10 && s.chars().all(|c| c.is_ascii_digit()) && s != "0" && !s.starts_with('0');
    if let Some(path) = text.strip_prefix("https://github.com/").or_else(|| text.strip_prefix("github.com/")) {
        let path = path.split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
        return match path.split('/').collect::<Vec<_>>().as_slice() {
            [owner, repo, "issues" | "pull", n] if name_ok(owner) && name_ok(repo) && number_ok(n) => {
                Some(format!("https://github.com/{path}"))
            }
            _ => None,
        };
    }
    let (repo, n) = text.split_once('#')?;
    let (owner, name) = repo.split_once('/')?;
    (name_ok(owner) && name_ok(name) && number_ok(n)).then(|| format!("https://github.com/{owner}/{name}/issues/{n}"))
}

/// A new card on a board. `what` is a draft -- `{"title": .., "body": ..}` --
/// or an issue or pull request already on GitHub -- `{"content": <its page or
/// owner/repo#n>}` -- or an issue made for it there and then -- `{"issue":
/// {"repo": "owner/name", "title": .., "body": ..}}`, answered with its
/// `number` and `url` too (and when the issue is made but the board will not
/// take it, with `card` null and `incomplete` saying why). Then, as one press
/// does on GitHub's page, the values it
/// is given (the column or group it was added in: `[{"field": id, "value":
/// ..}]`) and its place: after the card `after`, else where GitHub puts a new
/// one (last). Answers with the new card's id -- and, when the card is made
/// but a value or its place could not be given, `incomplete` with why: the
/// card is there all the same, and adding it again would make a second one
pub fn add(hub: &Hub, board: &str, what: &Value, values: &[Value], after: Option<&str>) -> Result<Value> {
    // An issue made for the board: made first, then put on it as any other
    let made = match what.get("issue").filter(|i| i.is_object()) {
        Some(i) => {
            let text = |k: &str| i.get(k).and_then(|x| x.as_str()).unwrap_or_default();
            let repo = Repo::parse(text("repo")).ok_or_else(|| anyhow!(crate::i18n::t("err.ghproject.bad_repo")))?;
            Some(hub.create_issue(&repo, text("title"), text("body"), &[], &[])?)
        }
        None => None,
    };
    let content = made.as_ref().and_then(|m| m["url"].as_str()).or_else(|| what.get("content").and_then(|c| c.as_str()));
    let card = if let Some(content) = content {
        let url = content_url(content).ok_or_else(|| anyhow!(crate::i18n::t("err.ghproject.bad_content")))?;
        let found = hub.graphql_whole(
            "query($url:URI!){resource(url:$url){__typename ... on Issue{id} ... on PullRequest{id}}}",
            json!({"url": url}),
        )?;
        let id = found
            .pointer("/data/resource/id")
            .and_then(|i| i.as_str())
            .ok_or_else(|| match found.pointer("/data/resource") {
                // Found, but not something a board holds (a discussion, say)
                Some(r) if !r.is_null() => anyhow!(crate::i18n::t("err.ghproject.bad_content")),
                _ => anyhow!(crate::i18n::tp("err.ghproject.no_content", &[("what", content.trim())])),
            })?
            .to_string();
        let v = hub.graphql_whole(
            "mutation($project:ID!,$content:ID!){addProjectV2ItemById(input:{projectId:$project,contentId:$content}){item{id}}}",
            json!({"project": board, "content": id}),
        )?;
        match carried(&v, "/data/addProjectV2ItemById/item/id") {
            Ok(card) => card.clone(),
            // The issue is made and the board would not take it: said with
            // the issue, which is there -- making it again would make a second
            Err(e) => match &made {
                Some(m) => return Ok(json!({"card": null, "number": m["number"], "url": m["url"], "incomplete": format!("{e:#}")})),
                None => return Err(e),
            },
        }
    } else {
        let title = what.get("title").and_then(|t| t.as_str()).unwrap_or_default().trim().to_string();
        if title.is_empty() {
            bail!(crate::i18n::t("err.ghproject.no_title"));
        }
        let body = what.get("body").and_then(|b| b.as_str()).unwrap_or_default();
        let v = hub.graphql_whole(
            "mutation($project:ID!,$title:String!,$body:String){addProjectV2DraftIssue(input:{projectId:$project,title:$title,body:$body}){projectItem{id}}}",
            json!({"project": board, "title": title, "body": body}),
        )?;
        carried(&v, "/data/addProjectV2DraftIssue/projectItem/id")?.clone()
    };
    let id = card.as_str().unwrap_or_default().to_string();
    let then = || -> Result<()> {
        for v in values {
            let field = v.get("field").and_then(|f| f.as_str()).unwrap_or_default();
            if !field.is_empty() {
                set_value(hub, board, &id, field, v.get("value").unwrap_or(&Value::Null))?;
            }
        }
        if let Some(after) = after {
            move_card(hub, board, &id, Some(after))?;
        }
        Ok(())
    };
    let mut out = match then() {
        Ok(()) => json!({"card": id}),
        Err(e) => json!({"card": id, "incomplete": format!("{e:#}")}),
    };
    if let Some(m) = made {
        out["number"] = m["number"].clone();
        out["url"] = m["url"].clone();
    }
    Ok(out)
}

/// A card's place among the others: right after `after`, or first of all for
/// None. The order every view without its own sorting shows them in
pub fn move_card(hub: &Hub, board: &str, card: &str, after: Option<&str>) -> Result<()> {
    let v = hub.graphql_whole(
        "mutation($project:ID!,$item:ID!,$after:ID){updateProjectV2ItemPosition(input:{projectId:$project,itemId:$item,afterId:$after}){items(first:1){totalCount}}}",
        json!({"project": board, "item": card, "after": after}),
    )?;
    carried(&v, "/data/updateProjectV2ItemPosition/items").map(|_| ())
}

/// A card put away in the board's archive, where GitHub keeps it to be
/// brought back -- or, `restore`, brought back from there
pub fn archive(hub: &Hub, board: &str, card: &str, restore: bool) -> Result<()> {
    let (q, at) = if restore {
        ("mutation($project:ID!,$item:ID!){unarchiveProjectV2Item(input:{projectId:$project,itemId:$item}){item{id}}}",
            "/data/unarchiveProjectV2Item/item/id")
    } else {
        ("mutation($project:ID!,$item:ID!){archiveProjectV2Item(input:{projectId:$project,itemId:$item}){item{id}}}",
            "/data/archiveProjectV2Item/item/id")
    };
    let v = hub.graphql_whole(q, json!({"project": board, "item": card}))?;
    carried(&v, at).map(|_| ())
}

/// A card taken off the board. The issue or pull request stays where it is;
/// a draft, which is nowhere else, is gone
pub fn remove(hub: &Hub, board: &str, card: &str) -> Result<()> {
    let v = hub.graphql_whole(
        "mutation($project:ID!,$item:ID!){deleteProjectV2Item(input:{projectId:$project,itemId:$item}){deletedItemId}}",
        json!({"project": board, "item": card}),
    )?;
    carried(&v, "/data/deleteProjectV2Item/deletedItemId").map(|_| ())
}

/// The ids GitHub knows people by, for their names there, in the order given.
/// A name nobody has is said, rather than left out
fn user_ids(hub: &Hub, logins: &[String]) -> Result<Vec<String>> {
    if logins.is_empty() {
        return Ok(Vec::new());
    }
    let vars: Vec<String> = (0..logins.len()).map(|i| format!("$l{i}:String!")).collect();
    let asks: Vec<String> = (0..logins.len()).map(|i| format!("u{i}:user(login:$l{i}){{id}}")).collect();
    let mut given = serde_json::Map::new();
    for (i, l) in logins.iter().enumerate() {
        given.insert(format!("l{i}"), json!(l.trim().trim_start_matches('@')));
    }
    let v = hub.graphql_whole(&format!("query({}){{{}}}", vars.join(","), asks.join(" ")), Value::Object(given))?;
    logins
        .iter()
        .enumerate()
        .map(|(i, l)| {
            v.pointer(&format!("/data/u{i}/id"))
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .ok_or_else(|| anyhow!(crate::i18n::tp("err.ghproject.no_user", &[("login", l)])))
        })
        .collect()
}

/// A draft's title, its words, and who it is assigned to, by their names on
/// GitHub (all of them: an empty list assigns it to nobody) -- each written
/// only when it is given, so what is not being changed stays as GitHub has
/// it, whatever a screen read some time ago says. A draft must keep a title
pub fn draft(hub: &Hub, draft: &str, title: Option<&str>, body: Option<&str>, people: Option<&[String]>) -> Result<()> {
    let title = title.map(str::trim);
    if title == Some("") {
        bail!(crate::i18n::t("err.ghproject.no_title"));
    }
    if title.is_none() && body.is_none() && people.is_none() {
        bail!(crate::i18n::t("err.github.no_change"));
    }
    let (mut takes, mut gives, mut vars) = (String::from("$draft:ID!"), String::from("draftIssueId:$draft"), json!({"draft": draft}));
    if let Some(title) = title {
        takes.push_str(",$title:String!");
        gives.push_str(",title:$title");
        vars["title"] = json!(title);
    }
    if let Some(body) = body {
        takes.push_str(",$body:String!");
        gives.push_str(",body:$body");
        vars["body"] = json!(body);
    }
    if let Some(people) = people {
        takes.push_str(",$people:[ID!]");
        gives.push_str(",assigneeIds:$people");
        vars["people"] = json!(user_ids(hub, people)?);
    }
    let v = hub.graphql_whole(&format!("mutation({takes}){{updateProjectV2DraftIssue(input:{{{gives}}}){{draftIssue{{id}}}}}}"), vars)?;
    carried(&v, "/data/updateProjectV2DraftIssue/draftIssue/id").map(|_| ())
}

/// A draft made an issue of `repo` (`owner/name`), with its title and words;
/// the card stays where it was and is the issue's from then on. Answers with
/// the issue's number and page. `words`, the draft's id with a title and
/// words, are written to the draft first -- and nothing is made unless they
/// are, so the issue never carries words older than those on screen. `people`
/// are who it is given to, written with the words: the issue has them
pub fn to_issue(hub: &Hub, card: &str, repo: &str, words: Option<(&str, &str, &str)>, people: Option<&[String]>) -> Result<Value> {
    if let Some((id, title, body)) = words {
        draft(hub, id, Some(title), Some(body), people)?;
    }
    let (owner, name) = repo.split_once('/').filter(|(o, n)| !o.is_empty() && !n.is_empty() && !n.contains('/'))
        .ok_or_else(|| anyhow!(crate::i18n::t("err.ghproject.bad_repo")))?;
    let found = hub.graphql_whole("query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id}}", json!({"owner": owner, "name": name}))?;
    let repo_id = carried(&found, "/data/repository/id")?.clone();
    let v = hub.graphql_whole(
        "mutation($item:ID!,$repo:ID!){convertProjectV2DraftIssueItemToIssue(input:{itemId:$item,repositoryId:$repo}){item{id content{... on Issue{number url}}}}}",
        json!({"item": card, "repo": repo_id}),
    )?;
    let issue = carried(&v, "/data/convertProjectV2DraftIssueItemToIssue/item/content")?;
    Ok(json!({"card": card, "number": issue["number"], "url": issue["url"]}))
}

// ── The board itself ─────────────────────────────────────────────────────

/// A text of `change`, when it names one
fn said<'a>(change: &'a Value, key: &str) -> Option<&'a str> {
    change.get(key).and_then(|x| x.as_str())
}

/// A view's shape, in GitHub's word for it
fn layout_word(layout: &str) -> Result<&'static str> {
    Ok(match layout.trim().to_ascii_lowercase().as_str() {
        "table" | "table_layout" => "TABLE_LAYOUT",
        "board" | "board_layout" => "BOARD_LAYOUT",
        "roadmap" | "roadmap_layout" => "ROADMAP_LAYOUT",
        _ => bail!(crate::i18n::tp("err.ghproject.bad_layout", &[("layout", layout)])),
    })
}

/// A new view of a board: a table, a board or a roadmap, by a name. Answers
/// with its `id` and its `view` number
pub fn view_make(hub: &Hub, board: &str, name: &str, layout: &str) -> Result<Value> {
    let name = name.trim();
    if name.is_empty() {
        bail!(crate::i18n::t("err.ghproject.no_name"));
    }
    let v = hub.graphql_whole(
        "mutation($project:ID!,$name:String!,$layout:ProjectV2ViewLayout!){createProjectV2View(input:{projectId:$project,name:$name,layout:$layout}){projectV2View{id number}}}",
        json!({"project": board, "name": name, "layout": layout_word(layout)?}),
    )?;
    let made = carried(&v, "/data/createProjectV2View/projectV2View")?;
    Ok(json!({"id": made["id"], "view": made["number"]}))
}

/// A view changed: its `name`, its `layout`, the `filter` it keeps (GitHub's
/// search words; empty for none) and the `fields` it shows, by their ids, in
/// order. Only what `change` names is touched. How a view groups and sorts
/// its cards GitHub keeps to its own page, and takes no word of here
pub fn view_change(hub: &Hub, view: &str, change: &Value) -> Result<()> {
    let mut vars = json!({"view": view});
    let mut sig = vec!["$view:ID!"];
    let mut input = vec!["viewId:$view"];
    if let Some(name) = said(change, "name") {
        if name.trim().is_empty() {
            bail!(crate::i18n::t("err.ghproject.no_name"));
        }
        vars["name"] = json!(name.trim());
        sig.push("$name:String");
        input.push("name:$name");
    }
    if let Some(layout) = said(change, "layout") {
        vars["layout"] = json!(layout_word(layout)?);
        sig.push("$layout:ProjectV2ViewLayout");
        input.push("layout:$layout");
    }
    if let Some(filter) = said(change, "filter") {
        vars["filter"] = json!(filter.trim());
        sig.push("$filter:String");
        input.push("filter:$filter");
    }
    if let Some(fields) = change.get("fields").and_then(|f| f.as_array()) {
        vars["fields"] = json!(fields.iter().filter_map(|f| f.as_str()).collect::<Vec<_>>());
        sig.push("$fields:[ID!]");
        input.push("configuration:{visibleFieldIds:$fields}");
    }
    if input.len() == 1 {
        bail!(crate::i18n::t("err.ghproject.no_change"));
    }
    let v = hub.graphql_whole(
        &format!("mutation({}){{updateProjectV2View(input:{{{}}}){{projectV2View{{id}}}}}}", sig.join(","), input.join(",")),
        vars,
    )?;
    carried(&v, "/data/updateProjectV2View/projectV2View/id").map(|_| ())
}

/// A view taken off its board. The cards stay: a view is only a way of looking
pub fn view_remove(hub: &Hub, view: &str) -> Result<()> {
    let v = hub.graphql_whole(
        "mutation($view:ID!){deleteProjectV2View(input:{viewId:$view}){projectV2View{id}}}",
        json!({"view": view}),
    )?;
    carried(&v, "/data/deleteProjectV2View/projectV2View/id").map(|_| ())
}

/// The colours GitHub paints a choice in
const OPTION_COLORS: [&str; 8] = ["GRAY", "BLUE", "GREEN", "YELLOW", "ORANGE", "RED", "PINK", "PURPLE"];

/// A field's choices as GitHub takes them: every one of them, in order, each
/// with the `id` it already has (one without is new; one left out is taken
/// off, and the cards that had it lose it)
fn options_given(options: &[Value]) -> Result<Vec<Value>> {
    options
        .iter()
        .map(|o| {
            let name = o.as_str().or_else(|| o["name"].as_str()).unwrap_or_default().trim();
            if name.is_empty() {
                bail!(crate::i18n::t("err.ghproject.no_name"));
            }
            let color = o["color"].as_str().unwrap_or("GRAY").to_ascii_uppercase();
            if !OPTION_COLORS.contains(&color.as_str()) {
                bail!(crate::i18n::tp("err.ghproject.bad_color", &[("color", &color), ("colors", &OPTION_COLORS.join(", "))]));
            }
            let mut out = json!({"name": name, "color": color, "description": o["description"].as_str().unwrap_or_default()});
            if let Some(id) = o["id"].as_str().filter(|i| !i.is_empty()) {
                out["id"] = json!(id);
            }
            Ok(out)
        })
        .collect()
}

/// A date as GitHub writes one, or refused
fn day(text: &str) -> Result<&str> {
    let ok = text.len() == 10 && text.char_indices().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
    if ok { Ok(text) } else { Err(anyhow!(crate::i18n::t("err.ghproject.bad_value"))) }
}

/// An iteration field's plan as GitHub takes it: `{"start": "YYYY-MM-DD",
/// "days": n, "iterations": [{"id"?, "title", "start", "days"}]}` -- every
/// iteration it is to have, each with the id it already has
fn iterations_given(plan: &Value) -> Result<Value> {
    let bad = || anyhow!(crate::i18n::t("err.ghproject.bad_value"));
    let days = |v: &Value| v.as_u64().filter(|d| (1..=366).contains(d)).ok_or_else(bad);
    let all: Vec<Value> = plan["iterations"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            let title = i["title"].as_str().unwrap_or_default().trim();
            if title.is_empty() {
                bail!(crate::i18n::t("err.ghproject.no_name"));
            }
            let mut out = json!({"title": title, "startDate": day(i["start"].as_str().unwrap_or_default())?, "duration": days(&i["days"])?});
            if let Some(id) = i["id"].as_str().filter(|x| !x.is_empty()) {
                out["id"] = json!(id);
            }
            Ok(out)
        })
        .collect::<Result<_>>()?;
    let first = all.first().and_then(|i| i["startDate"].as_str()).unwrap_or_default().to_string();
    let start = plan["start"].as_str().map(str::to_string).unwrap_or(first);
    Ok(json!({"startDate": day(&start)?, "duration": days(&plan["days"])?, "iterations": all}))
}

/// The kinds of field a person can add to a board
fn field_kind(kind: &str) -> Result<&'static str> {
    Ok(match kind.trim().to_ascii_uppercase().as_str() {
        "TEXT" => "TEXT",
        "NUMBER" => "NUMBER",
        "DATE" => "DATE",
        "SINGLE_SELECT" => "SINGLE_SELECT",
        "MULTI_SELECT" => "MULTI_SELECT",
        "ITERATION" => "ITERATION",
        _ => bail!(crate::i18n::tp("err.ghproject.bad_kind", &[("kind", kind)])),
    })
}

/// A new field on a board: `{"name", "type"}` -- text, number, date,
/// single_select, multi_select or iteration -- with its `options` (a field of
/// choices must start with one) or its `iteration` plan. Answers with its `field` id
pub fn field_make(hub: &Hub, board: &str, spec: &Value) -> Result<Value> {
    let name = said(spec, "name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        bail!(crate::i18n::t("err.ghproject.no_name"));
    }
    let kind = field_kind(said(spec, "type").unwrap_or_default())?;
    let mut vars = json!({"project": board, "name": name, "kind": kind});
    let (sig, input) = match kind {
        "SINGLE_SELECT" | "MULTI_SELECT" => {
            let options = options_given(spec["options"].as_array().map(|o| o.as_slice()).unwrap_or_default())?;
            if options.is_empty() {
                bail!(crate::i18n::t("err.ghproject.no_options"));
            }
            vars["options"] = json!(options);
            if kind == "SINGLE_SELECT" {
                (",$options:[ProjectV2SingleSelectFieldOptionInput!]", ",singleSelectOptions:$options")
            } else {
                (",$options:[ProjectV2MultiSelectFieldOptionInput!]", ",multiSelectOptions:$options")
            }
        }
        "ITERATION" => {
            vars["plan"] = iterations_given(&spec["iteration"])?;
            (",$plan:ProjectV2IterationFieldConfigurationInput", ",iterationConfiguration:$plan")
        }
        _ => ("", ""),
    };
    let v = hub.graphql_whole(
        &format!(
            "mutation($project:ID!,$name:String!,$kind:ProjectV2CustomFieldType!{sig}){{createProjectV2Field(input:{{projectId:$project,name:$name,dataType:$kind{input}}}){{projectV2Field{{... on ProjectV2FieldCommon{{id}}}}}}}}"
        ),
        vars,
    )?;
    Ok(json!({"field": carried(&v, "/data/createProjectV2Field/projectV2Field/id")?}))
}

/// A field changed: its `name`, its `options` (all of them, as
/// [`options_given`] reads them; `type` says "multi_select" for a field of
/// several choices) or its `iteration` plan. Only what `change` names is touched
pub fn field_change(hub: &Hub, field: &str, change: &Value) -> Result<()> {
    let mut vars = json!({"field": field});
    let mut sig = vec!["$field:ID!".to_string()];
    let mut input = vec!["fieldId:$field".to_string()];
    if let Some(name) = said(change, "name") {
        if name.trim().is_empty() {
            bail!(crate::i18n::t("err.ghproject.no_name"));
        }
        vars["name"] = json!(name.trim());
        sig.push("$name:String".into());
        input.push("name:$name".into());
    }
    if let Some(options) = change.get("options").and_then(|o| o.as_array()) {
        let options = options_given(options)?;
        if options.is_empty() {
            bail!(crate::i18n::t("err.ghproject.no_options"));
        }
        vars["options"] = json!(options);
        let multi = said(change, "type").is_some_and(|t| t.eq_ignore_ascii_case("multi_select"));
        if multi {
            sig.push("$options:[ProjectV2MultiSelectFieldOptionInput!]".into());
            input.push("multiSelectOptions:$options".into());
        } else {
            sig.push("$options:[ProjectV2SingleSelectFieldOptionInput!]".into());
            input.push("singleSelectOptions:$options".into());
        }
    }
    if change.get("iteration").is_some_and(|i| i.is_object()) {
        vars["plan"] = iterations_given(&change["iteration"])?;
        sig.push("$plan:ProjectV2IterationFieldConfigurationInput".into());
        input.push("iterationConfiguration:$plan".into());
    }
    if input.len() == 1 {
        bail!(crate::i18n::t("err.ghproject.no_change"));
    }
    let v = hub.graphql_whole(
        &format!(
            "mutation({}){{updateProjectV2Field(input:{{{}}}){{projectV2Field{{... on ProjectV2FieldCommon{{id}}}}}}}}",
            sig.join(","),
            input.join(",")
        ),
        vars,
    )?;
    carried(&v, "/data/updateProjectV2Field/projectV2Field/id").map(|_| ())
}

/// A field taken off its board, with every card's value in it
pub fn field_remove(hub: &Hub, field: &str) -> Result<()> {
    let v = hub.graphql_whole(
        "mutation($field:ID!){deleteProjectV2Field(input:{fieldId:$field}){projectV2Field{... on ProjectV2FieldCommon{id}}}}",
        json!({"field": field}),
    )?;
    carried(&v, "/data/deleteProjectV2Field/projectV2Field/id").map(|_| ())
}

/// The id GitHub knows a person or an organization by
fn owner_id(hub: &Hub, owner: &str) -> Result<Value> {
    let v = hub.graphql_whole("query($owner:String!){repositoryOwner(login:$owner){id}}", json!({"owner": owner.trim()}))?;
    carried(&v, "/data/repositoryOwner/id").cloned()
}

/// The id GitHub knows a repository by, named `owner/name`
fn repo_id(hub: &Hub, repo: &str) -> Result<Value> {
    let r = Repo::parse(repo).ok_or_else(|| anyhow!(crate::i18n::t("err.ghproject.bad_repo")))?;
    let v = hub.graphql_whole("query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id}}", json!({"owner": r.owner, "name": r.name}))?;
    carried(&v, "/data/repository/id").cloned()
}

/// What a board just made or copied is, for whoever asked
fn made_board(node: &Value) -> Value {
    json!({"id": node["id"], "number": node["number"], "title": node["title"], "url": node["url"]})
}

/// A new board of `owner` (a person or an organization), by a title --
/// linked to the repository `repo` (`owner/name`) when one is named, so it is
/// found from there. Answers `{id, number, title, url}`
pub fn make(hub: &Hub, owner: &str, title: &str, repo: Option<&str>) -> Result<Value> {
    let title = title.trim();
    if title.is_empty() {
        bail!(crate::i18n::t("err.ghproject.no_title"));
    }
    let repo = match repo.filter(|r| !r.trim().is_empty()) {
        Some(r) => repo_id(hub, r)?,
        None => Value::Null,
    };
    let v = hub.graphql_whole(
        "mutation($owner:ID!,$title:String!,$repo:ID){createProjectV2(input:{ownerId:$owner,title:$title,repositoryId:$repo}){projectV2{id number title url}}}",
        json!({"owner": owner_id(hub, owner)?, "title": title, "repo": repo}),
    )?;
    carried(&v, "/data/createProjectV2/projectV2").map(made_board)
}

/// A board copied -- its views, its fields, its workflows; its drafts too
/// when `drafts` -- as a new board of `owner`. Answers as [`make`] does
pub fn copy(hub: &Hub, board: &str, owner: &str, title: &str, drafts: bool) -> Result<Value> {
    let title = title.trim();
    if title.is_empty() {
        bail!(crate::i18n::t("err.ghproject.no_title"));
    }
    let v = hub.graphql_whole(
        "mutation($project:ID!,$owner:ID!,$title:String!,$drafts:Boolean){copyProjectV2(input:{projectId:$project,ownerId:$owner,title:$title,includeDraftIssues:$drafts}){projectV2{id number title url}}}",
        json!({"project": board, "owner": owner_id(hub, owner)?, "title": title, "drafts": drafts}),
    )?;
    carried(&v, "/data/copyProjectV2/projectV2").map(made_board)
}

/// What an emptied text of a board is written as (a zero-width space), and
/// read back as nothing: see [`change`]
const EMPTIED: &str = "\u{200b}";

/// A text of a board as a person reads it: nothing, when it is only the mark
/// an emptied one is written as
fn words(v: &Value) -> &str {
    let text = v.as_str().unwrap_or_default();
    if text.chars().all(|c| c.is_whitespace() || c == '\u{200b}') { "" } else { text }
}

/// The board's own words changed: its `title`, its short `description`, its
/// `readme`, whether it is `closed`, and whether it is `public`. Only what
/// `change` names is touched
pub fn change(hub: &Hub, board: &str, change: &Value) -> Result<()> {
    let mut vars = json!({"project": board});
    let mut sig = vec!["$project:ID!".to_string()];
    let mut input = vec!["projectId:$project".to_string()];
    // As this module names them, and as GitHub does
    for (key, name) in [("title", "title"), ("description", "shortDescription"), ("readme", "readme")] {
        if let Some(text) = said(change, key) {
            if key == "title" && text.trim().is_empty() {
                bail!(crate::i18n::t("err.ghproject.no_title"));
            }
            // GitHub reads an empty text as "leave it as it is": one that is
            // emptied is written as a mark that shows as nothing
            let text = if key == "readme" { text } else { text.trim() };
            vars[name] = json!(if text.trim().is_empty() { EMPTIED } else { text });
            sig.push(format!("${name}:String"));
            input.push(format!("{name}:${name}"));
        }
    }
    for key in ["closed", "public"] {
        if let Some(b) = change.get(key).and_then(|b| b.as_bool()) {
            vars[key] = json!(b);
            sig.push(format!("${key}:Boolean"));
            input.push(format!("{key}:${key}"));
        }
    }
    if input.len() == 1 {
        bail!(crate::i18n::t("err.ghproject.no_change"));
    }
    let v = hub.graphql_whole(
        &format!("mutation({}){{updateProjectV2(input:{{{}}}){{projectV2{{id}}}}}}", sig.join(","), input.join(",")),
        vars,
    )?;
    carried(&v, "/data/updateProjectV2/projectV2/id").map(|_| ())
}

/// A board deleted, with its views, its fields and its drafts. The issues and
/// pull requests that were on it stay where they are. It cannot be brought back
pub fn delete(hub: &Hub, board: &str) -> Result<()> {
    let v = hub.graphql_whole(
        "mutation($project:ID!){deleteProjectV2(input:{projectId:$project}){projectV2{id}}}",
        json!({"project": board}),
    )?;
    carried(&v, "/data/deleteProjectV2/projectV2/id").map(|_| ())
}

/// An organization's board made a template new boards can be started from,
/// or no longer one
pub fn template(hub: &Hub, board: &str, on: bool) -> Result<()> {
    let (q, at) = if on {
        ("mutation($project:ID!){markProjectV2AsTemplate(input:{projectId:$project}){projectV2{id}}}", "/data/markProjectV2AsTemplate/projectV2/id")
    } else {
        ("mutation($project:ID!){unmarkProjectV2AsTemplate(input:{projectId:$project}){projectV2{id}}}", "/data/unmarkProjectV2AsTemplate/projectV2/id")
    };
    let v = hub.graphql_whole(q, json!({"project": board}))?;
    carried(&v, at).map(|_| ())
}

/// A report of how the board stands, as a person reads it
fn status_shape(s: &Value) -> Value {
    json!({"id": s["id"], "body": s["body"].as_str().unwrap_or_default(),
        "status": s["status"].as_str().map(|x| x.to_ascii_lowercase()),
        "start": s["startDate"], "target": s["targetDate"], "created": s["createdAt"], "author": s.pointer("/creator/login")})
}

/// What the board says of itself beyond its cards: its words (`description`,
/// `readme`), whether it is `public`, `closed`, a `template`, whether this
/// account may change it (`can_update`), the repositories it is linked to
/// (`repos`), the reports of how it stands, newest first (`updates`), and its
/// `workflows`. A part this account's token may not read is null, not an
/// error: the rest is still worth having
pub fn about(hub: &Hub, at: &GhRef) -> Result<Value> {
    let org = match at.org {
        Some(o) => o,
        None => owner_is_org(hub, &at.owner)?,
    };
    let root = if org { "organization" } else { "user" };
    let ask = |part: &str| -> Result<Value> {
        hub.graphql_whole(
            &format!("query($owner:String!,$number:Int!){{{root}(login:$owner){{projectV2(number:$number){{{part}}}}}}}"),
            json!({"owner": at.owner, "number": at.number}),
        )
    };
    let at_board = format!("/data/{root}/projectV2");
    let v = ask(
        "id number title url closed public template readme shortDescription viewerCanUpdate \
         repositories(first:30){nodes{nameWithOwner}} \
         statusUpdates(first:20,orderBy:{field:CREATED_AT,direction:DESC}){nodes{id body status startDate targetDate createdAt creator{login}}}",
    )?;
    let Some(b) = v.pointer(&at_board).filter(|n| !n.is_null()) else {
        return Err(refused(&v));
    };
    let list = |x: Option<&Value>| -> Vec<Value> { x.and_then(|n| n.as_array()).cloned().unwrap_or_default() };
    // Asked apart: a token that may not read them is refused the whole of
    // what they are asked with
    let workflows = ask("workflows(first:30,orderBy:{field:NAME,direction:ASC}){nodes{id name number enabled}}").ok().and_then(|w| {
        let nodes = w.pointer(&format!("{at_board}/workflows/nodes"))?.as_array()?;
        let seen: Vec<Value> = nodes.iter().filter(|n| n.is_object())
            .map(|n| json!({"id": n["id"], "name": n["name"], "number": n["number"], "enabled": n["enabled"]})).collect();
        // Every one of them refused is "may not read", not "has none"
        (seen.len() == nodes.len() || !seen.is_empty()).then(|| json!(seen))
    });
    Ok(json!({
        "id": b["id"], "number": b["number"], "title": b["title"], "url": b["url"], "org": org, "owner": at.owner,
        "closed": b["closed"].as_bool().unwrap_or(false), "public": b["public"].as_bool().unwrap_or(false),
        "template": b["template"].as_bool().unwrap_or(false), "can_update": b["viewerCanUpdate"].as_bool().unwrap_or(false),
        "description": words(&b["shortDescription"]), "readme": words(&b["readme"]),
        "repos": list(b.pointer("/repositories/nodes")).iter().filter_map(|r| r["nameWithOwner"].as_str().map(str::to_string)).collect::<Vec<_>>(),
        "updates": list(b.pointer("/statusUpdates/nodes")).iter().map(status_shape).collect::<Vec<_>>(),
        "workflows": workflows,
    }))
}

/// How a board stands, in GitHub's words for it
fn status_word(status: &str) -> Result<&'static str> {
    Ok(match status.trim().to_ascii_lowercase().as_str() {
        "inactive" => "INACTIVE",
        "on_track" => "ON_TRACK",
        "at_risk" => "AT_RISK",
        "off_track" => "OFF_TRACK",
        "complete" => "COMPLETE",
        _ => bail!(crate::i18n::tp("err.ghproject.bad_status", &[("status", status)])),
    })
}

/// A report of how the board stands, written -- or, with `id`, the one there
/// rewritten: `{"status": inactive|on_track|at_risk|off_track|complete,
/// "body", "start", "target"}` (dates `YYYY-MM-DD`; any of them may be left
/// out). Answers with the report as it is kept
pub fn status_write(hub: &Hub, board: &str, id: Option<&str>, what: &Value) -> Result<Value> {
    let status = match said(what, "status").filter(|s| !s.trim().is_empty()) {
        Some(s) => json!(status_word(s)?),
        None => Value::Null,
    };
    let date = |key: &str| -> Result<Value> {
        match said(what, key).filter(|d| !d.trim().is_empty()) {
            Some(d) => Ok(json!(day(d.trim())?)),
            None => Ok(Value::Null),
        }
    };
    let vars = json!({"id": id.unwrap_or(board), "status": status, "body": said(what, "body"), "start": date("start")?, "target": date("target")?});
    const SHAPE: &str = "statusUpdate{id body status startDate targetDate createdAt creator{login}}";
    let (name, key) = if id.is_some() { ("updateProjectV2StatusUpdate", "statusUpdateId") } else { ("createProjectV2StatusUpdate", "projectId") };
    let v = hub.graphql_whole(
        &format!(
            "mutation($id:ID!,$status:ProjectV2StatusUpdateStatus,$body:String,$start:Date,$target:Date){{{name}(input:{{{key}:$id,status:$status,body:$body,startDate:$start,targetDate:$target}}){{{SHAPE}}}}}"
        ),
        vars,
    )?;
    carried(&v, &format!("/data/{name}/statusUpdate")).map(status_shape)
}

/// A report taken off the board
pub fn status_remove(hub: &Hub, id: &str) -> Result<()> {
    let v = hub.graphql_whole(
        "mutation($id:ID!){deleteProjectV2StatusUpdate(input:{statusUpdateId:$id}){deletedStatusUpdateId}}",
        json!({"id": id}),
    )?;
    carried(&v, "/data/deleteProjectV2StatusUpdate/deletedStatusUpdateId").map(|_| ())
}

/// A board linked to a repository (`owner/name`), so it is listed there -- or,
/// `on` false, no longer linked
pub fn link(hub: &Hub, board: &str, repo: &str, on: bool) -> Result<()> {
    let name = if on { "linkProjectV2ToRepository" } else { "unlinkProjectV2FromRepository" };
    let v = hub.graphql_whole(
        &format!("mutation($project:ID!,$repo:ID!){{{name}(input:{{projectId:$project,repositoryId:$repo}}){{repository{{id}}}}}}"),
        json!({"project": board, "repo": repo_id(hub, repo)?}),
    )?;
    carried(&v, &format!("/data/{name}/repository/id")).map(|_| ())
}

/// What people may do on a board, set for each of `people`: `[{"user":
/// <name on GitHub>, "role": reader|writer|admin|none}]` -- none takes the
/// person off. Answers with everybody who has a part in it after, by name.
/// GitHub tells who they are only in answer to a change
pub fn access(hub: &Hub, board: &str, people: &[Value]) -> Result<Value> {
    if people.is_empty() {
        bail!(crate::i18n::t("err.ghproject.no_change"));
    }
    let logins: Vec<String> = people.iter().map(|p| p["user"].as_str().unwrap_or_default().to_string()).collect();
    let ids = user_ids(hub, &logins)?;
    let given: Vec<Value> = people
        .iter()
        .zip(ids)
        .map(|(p, id)| {
            let role = match p["role"].as_str().unwrap_or_default().to_ascii_lowercase().as_str() {
                "reader" => "READER",
                "writer" => "WRITER",
                "admin" => "ADMIN",
                "none" => "NONE",
                other => bail!(crate::i18n::tp("err.ghproject.bad_role", &[("role", other)])),
            };
            Ok(json!({"userId": id, "role": role}))
        })
        .collect::<Result<_>>()?;
    let v = hub.graphql_whole(
        "mutation($project:ID!,$people:[ProjectV2Collaborator!]!){updateProjectV2Collaborators(input:{projectId:$project,collaborators:$people}){collaborators(first:100){nodes{__typename ... on User{login}}}}}",
        json!({"project": board, "people": given}),
    )?;
    let nodes = carried(&v, "/data/updateProjectV2Collaborators/collaborators/nodes")?;
    Ok(json!({"people": nodes.as_array().into_iter().flatten().filter_map(|n| n["login"].as_str().map(str::to_string)).collect::<Vec<_>>()}))
}

/// One of a board's workflows taken off it. GitHub lets a workflow be made
/// and changed only on its own page
pub fn workflow_remove(hub: &Hub, id: &str) -> Result<()> {
    let v = hub.graphql_whole(
        "mutation($id:ID!){deleteProjectV2Workflow(input:{workflowId:$id}){deletedWorkflowId}}",
        json!({"id": id}),
    )?;
    carried(&v, "/data/deleteProjectV2Workflow/deletedWorkflowId").map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_project_is_found_by_its_address_or_by_owner_and_number() {
        let org = GhRef::parse("https://github.com/orgs/acme-co/projects/7").unwrap();
        assert_eq!(org, GhRef { owner: "acme-co".into(), org: Some(true), number: 7, view: None });
        let user = GhRef::parse("https://github.com/users/someone/projects/2/views/3/").unwrap();
        assert_eq!(user, GhRef { owner: "someone".into(), org: Some(false), number: 2, view: Some(3) });
        // What the page adds after the address is not part of it
        assert_eq!(GhRef::parse("https://github.com/orgs/acme-co/projects/7?pane=issue&itemId=5").unwrap().number, 7);
        assert_eq!(GhRef::parse("acme-co/7").unwrap(), GhRef { owner: "acme-co".into(), org: None, number: 7, view: None });
        for bad in ["https://github.com/acme-co/repo", "https://github.com/orgs/acme-co/projects/7x",
            "https://github.com/orgs/acme-co/projects/0", "https://example.com/orgs/a/projects/1", "a/b/1", "acme/", ""] {
            assert_eq!(GhRef::parse(bad), None, "{bad}");
        }
        assert_eq!(org.url(), "https://github.com/orgs/acme-co/projects/7");
        assert_eq!(user.url(), "https://github.com/users/someone/projects/2");
    }

    #[test]
    fn a_list_of_github_projects_is_each_one_once_and_a_token_that_sees_none_is_told_why() {
        let node = |id: &str, n: u64| json!({"id": id, "number": n, "title": format!("B{n}"), "url": null, "closed": false,
            "owner": {"__typename": "Organization", "login": "acme-co"}, "items": {"totalCount": 4}});
        let v = json!({"data": {
            "repository": {"projectsV2": {"nodes": [node("P1", 1)]}},
            "repositoryOwner": {"__typename": "Organization", "projectsV2": {"nodes": [node("P1", 1), node("P2", 2), null]}},
            "viewer": {"login": "me", "projectsV2": {"nodes": []}}}});
        let got = read_list(&v).unwrap();
        let listed = got["projects"].as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!((listed[0]["from"].as_str(), listed[1]["from"].as_str()), (Some("repository"), Some("owner")));
        assert_eq!(listed[0]["url"], "https://github.com/orgs/acme-co/projects/1");
        assert_eq!(got["hidden"], 1);

        // As GitHub answers a token without the Projects permission
        let denied = json!({"data": {"repository": {"projectsV2": {"nodes": [null]}},
            "repositoryOwner": {"__typename": "Organization", "projectsV2": {"nodes": [null]}},
            "viewer": {"login": "me", "projectsV2": {"nodes": []}}},
            "errors": [{"type": "FORBIDDEN", "message": "Resource not accessible by personal access token"}]});
        let e = read_list(&denied).unwrap_err();
        assert!(e.downcast_ref::<AccountTrouble>().is_some(), "the settings are offered for it");
        assert_eq!(e.to_string(), crate::i18n::t("err.ghproject.denied"));

        // A classic token or GitHub CLI's without the scope: refused whole
        let scope = json!({"errors": [{"type": "INSUFFICIENT_SCOPES",
            "message": "Your token has not been granted the required scopes to execute this query."}]});
        let e = read_list(&scope).unwrap_err();
        assert!(e.downcast_ref::<AccountTrouble>().is_some());
        assert_eq!(e.to_string(), crate::i18n::t("err.ghproject.denied"));

        // An organization's own rule is said in its own words
        let rule = json!({"data": {"repositoryOwner": null}, "errors": [{"type": "FORBIDDEN",
            "message": "The 'acme-co' organization forbids access via a fine-grained personal access tokens if the token's lifetime is greater than 366 days."}]});
        assert!(refused(&rule).to_string().contains("366 days"));
    }

    fn shape() -> Value {
        json!({"id": "PVT_1", "number": 3, "title": "Work", "url": "u", "closed": false,
            "fields": {"nodes": [
                {"__typename": "ProjectV2Field", "id": "F_title", "name": "Title", "dataType": "TITLE"},
                {"__typename": "ProjectV2SingleSelectField", "id": "F_pri", "name": "Priority", "dataType": "SINGLE_SELECT",
                 "options": [{"id": "o1", "name": "High", "color": "RED"}]},
                {"__typename": "ProjectV2SingleSelectField", "id": "F_status", "name": "Status", "dataType": "SINGLE_SELECT",
                 "options": [{"id": "s1", "name": "Todo", "color": "GRAY"}, {"id": "s2", "name": "Done", "color": "GREEN"}]},
                {"__typename": "ProjectV2IterationField", "id": "F_sprint", "name": "Sprint", "dataType": "ITERATION",
                 "configuration": {"iterations": [{"id": "i3", "title": "S3", "startDate": "2026-11-15", "duration": 14},
                                                  {"id": "i2", "title": "S2", "startDate": "2026-11-01", "duration": 14}],
                                   "completedIterations": [{"id": "i1", "title": "S1", "startDate": "2026-10-18", "duration": 14}]}},
                {}]},
            "views": {"nodes": [
                {"number": 1, "name": "Table", "layout": "TABLE_LAYOUT", "filter": null, "fields": {"nodes": [{"id": "F_title"}]},
                 "groupByFields": {"nodes": []}, "verticalGroupByFields": {"nodes": []},
                 "sortByFields": {"nodes": [{"direction": "DESC", "field": {"id": "F_pri"}}]}},
                {"number": 2, "name": "Sprint board", "layout": "BOARD_LAYOUT", "filter": "is:open", "fields": {"nodes": []},
                 "groupByFields": {"nodes": []}, "verticalGroupByFields": {"nodes": [{"id": "F_sprint"}]},
                 "sortByFields": {"nodes": []}}]}})
    }

    #[test]
    fn a_github_projects_fields_and_views_are_read_in_the_order_they_are_shown() {
        let got = read_shape(&shape());
        let fields = got["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4, "a field GitHub said nothing of is left out");
        let sprint = &fields[3]["iterations"];
        assert_eq!(sprint.as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect::<Vec<_>>(), ["i1", "i2", "i3"]);
        assert_eq!(sprint[0]["done"], true);
        let views = got["views"].as_array().unwrap();
        assert_eq!((views[0]["layout"].as_str(), views[1]["layout"].as_str()), (Some("table"), Some("board")));
        assert_eq!(views[0]["filter"], "");
        assert_eq!(views[0]["sort"], json!([{"field": "F_pri", "desc": true}]));
        assert_eq!(views[1]["lanes"], "F_sprint");
    }

    #[test]
    fn a_board_view_is_in_columns_by_its_views_own_field_else_by_status() {
        let got = read_shape(&shape());
        let views = got["views"].as_array().unwrap();
        assert_eq!(column_field(&got["fields"], &views[1]).as_deref(), Some("F_sprint"));
        // No field of the view's own: Status, though it is not the first choice field
        assert_eq!(column_field(&got["fields"], &views[0]).as_deref(), Some("F_status"));
        // A field the view names that cannot be columns is passed over
        let odd = json!({"lanes": "F_title"});
        assert_eq!(column_field(&got["fields"], &odd).as_deref(), Some("F_status"));
        let no_status = json!([{"id": "a", "name": "Size", "type": "SINGLE_SELECT"}]);
        assert_eq!(column_field(&no_status, &json!({})).as_deref(), Some("a"));
        assert_eq!(column_field(&json!([{"id": "t", "name": "Title", "type": "TITLE"}]), &json!({})), None);
    }

    #[test]
    fn a_card_says_what_it_is_about_and_its_value_in_each_field() {
        let node = json!({"id": "PVTI_1", "type": "PULL_REQUEST", "isArchived": false,
            "content": {"__typename": "PullRequest", "number": 12, "title": "Speed up", "url": "https://github.com/a/b/pull/12",
                "state": "OPEN", "isDraft": true, "updatedAt": "2026-11-10T00:00:00Z", "repository": {"nameWithOwner": "a/b"},
                "assignees": {"nodes": [{"login": "kim"}]}, "labels": {"nodes": [{"name": "perf", "color": "ff0000"}]}},
            "fieldValues": {"nodes": [
                {"__typename": "ProjectV2ItemFieldSingleSelectValue", "optionId": "s1", "name": "Todo", "field": {"id": "F_status"}},
                {"__typename": "ProjectV2ItemFieldDateValue", "date": "2026-11-20", "field": {"id": "F_due"}},
                {"__typename": "ProjectV2ItemFieldIterationValue", "iterationId": "i2", "title": "S2", "startDate": "2026-11-01", "duration": 14, "field": {"id": "F_sprint"}},
                {"__typename": "ProjectV2ItemFieldRepositoryValue", "field": {"id": "F_repo"}},
                {"__typename": "ProjectV2ItemFieldMilestoneValue", "milestone": {"title": "v2"}, "field": {"id": "F_ms"}},
                {}]}});
        let c = read_card(&node).unwrap();
        assert_eq!((c["kind"].as_str(), c["state"].as_str(), c["number"].as_u64()), (Some("pr"), Some("draft"), Some(12)));
        assert_eq!(c["repo"], "a/b");
        assert_eq!(c["assignees"], json!(["kim"]));
        assert_eq!(c["values"]["F_status"], json!({"option": "s1", "name": "Todo"}));
        assert_eq!(c["values"]["F_due"], json!({"date": "2026-11-20"}));
        assert_eq!(c["values"]["F_sprint"]["iteration"], "i2");
        assert!(c["values"].get("F_repo").is_none(), "a value not written here is not read as one");
        assert_eq!(c["values"]["F_ms"], json!({"milestone": "v2"}));

        let hidden = json!({"id": "PVTI_2", "type": "REDACTED", "isArchived": false, "content": null, "fieldValues": {"nodes": []}});
        assert_eq!(read_card(&hidden).unwrap()["kind"], "hidden");
        let draft = json!({"id": "PVTI_3", "type": "DRAFT_ISSUE", "isArchived": false,
            "content": {"__typename": "DraftIssue", "id": "DI_1", "title": "Idea", "body": "Why and how", "updatedAt": null,
                "assignees": {"nodes": [{"login": "lee"}]}}, "fieldValues": {"nodes": []}});
        let d = read_card(&draft).unwrap();
        assert_eq!(d["kind"], "draft");
        assert_eq!(d["assignees"], json!(["lee"]), "a draft card has people on it too");
        assert_eq!((d["draft"].as_str(), d["body"].as_str()), (Some("DI_1"), Some("Why and how")), "a draft is written to by its own id");
        let issue = read_card(&json!({"id": "PVTI_5", "isArchived": false, "fieldValues": {"nodes": []},
            "content": {"__typename": "Issue", "id": "I_1", "number": 4, "title": "x", "body": "not read"}})).unwrap();
        assert!(issue.get("draft").is_none() && issue.get("body").is_none(), "an issue is written to on its own page");
        // A card in the archive says so: which of the two is read is the asker's choice
        let archived = json!({"id": "PVTI_4", "isArchived": true});
        assert_eq!(read_card(&archived).unwrap()["archived"], true);
        assert_eq!(c["archived"], false);
        assert!(read_card(&json!({"isArchived": false})).is_none(), "a card GitHub gave no id is not one");

        // What only the issue itself says, which a board shows as fields of its own
        let rich = read_card(&json!({"id": "PVTI_6", "isArchived": false,
            "content": {"__typename": "Issue", "id": "I_6", "number": 6, "title": "Ship", "state": "CLOSED",
                "createdAt": "2026-10-01T00:00:00Z", "closedAt": "2026-10-09T00:00:00Z", "milestone": {"title": "v2"},
                "issueType": {"name": "Bug"}, "parent": {"number": 2, "title": "Epic", "url": "u", "repository": {"nameWithOwner": "a/b"}},
                "subIssuesSummary": {"total": 4, "completed": 1, "percentCompleted": 25},
                "closedByPullRequestsReferences": {"nodes": [
                    {"number": 9, "url": "p", "state": "OPEN", "isDraft": true, "repository": {"nameWithOwner": "a/b"}},
                    {"number": 8, "url": "q", "state": "MERGED", "isDraft": false, "repository": {"nameWithOwner": "a/b"}}]}},
            "fieldValues": {"nodes": [
                {"__typename": "ProjectV2ItemFieldMultiSelectValue", "options": [{"id": "m1", "name": "Web"}, {"id": "m2", "name": "App"}], "field": {"id": "F_area"}},
                {"__typename": "ProjectV2ItemIssueFieldValue", "field": {"id": "F_org"}, "issueFieldValue": {"__typename": "IssueFieldNumberValue", "amount": 3.0}}]}})).unwrap();
        assert_eq!((rich["node"].as_str(), rich["milestone"].as_str(), rich["type"].as_str()), (Some("I_6"), Some("v2"), Some("Bug")));
        assert_eq!(rich["parent"], json!({"number": 2, "title": "Epic", "url": "u", "repo": "a/b"}));
        assert_eq!(rich["sub"], json!({"total": 4, "done": 1, "percent": 25}));
        assert_eq!((rich["created"].as_str(), rich["closed_at"].as_str()), (Some("2026-10-01T00:00:00Z"), Some("2026-10-09T00:00:00Z")));
        assert_eq!(rich["values"]["F_area"], json!({"options": [{"id": "m1", "name": "Web"}, {"id": "m2", "name": "App"}]}));
        assert_eq!((rich["prs"][0]["state"].as_str(), rich["prs"][1]["state"].as_str()), (Some("draft"), Some("merged")));
        let reviewed = read_card(&json!({"id": "PVTI_7", "isArchived": false, "fieldValues": {"nodes": []},
            "content": {"__typename": "PullRequest", "id": "PR_7", "number": 7, "title": "x", "state": "OPEN",
                "reviewRequests": {"nodes": [{"requestedReviewer": {"__typename": "User", "login": "kim"}}, {"requestedReviewer": {"__typename": "Team"}}, {"requestedReviewer": null}]},
                "latestReviews": {"nodes": [{"author": {"login": "lee"}}, {"author": {"login": "kim"}}]}}})).unwrap();
        assert_eq!(reviewed["reviewers"], json!(["kim", {"team": true}, "lee"]), "each once; a team is not named, and is said to be one");
        assert_eq!(rich["values"]["F_org"], json!({"shown": "3"}));
        assert!(c.get("sub").is_none() && c.get("type").is_none(), "a pull request has neither");
    }

    #[test]
    fn what_changes_a_board_itself_is_said_the_way_github_takes_it() {
        assert_eq!(field_value(&json!({"options": ["a", {"id": "b"}]})).unwrap(), json!({"multiSelectOptionIds": ["a", "b"]}));
        assert!(field_value(&json!({"options": [1]})).is_err());
        assert_eq!((layout_word("Board").unwrap(), layout_word("roadmap_layout").unwrap()), ("BOARD_LAYOUT", "ROADMAP_LAYOUT"));
        assert!(layout_word("kanban").is_err());
        assert_eq!((field_kind("multi_select").unwrap(), field_kind("DATE").unwrap()), ("MULTI_SELECT", "DATE"));
        assert!(field_kind("assignees").is_err(), "a field GitHub fills in itself cannot be added");
        assert_eq!(status_word("At_Risk").unwrap(), "AT_RISK");
        // An emptied text is written as a mark, and the mark is read as nothing
        assert_eq!((words(&json!(EMPTIED)), words(&json!(" \n")), words(&json!(null)), words(&json!("Plan"))), ("", "", "", "Plan"));
        assert!(status_word("late").is_err());

        // Every choice goes, each with the id it has; a new one has none
        let given = options_given(&[json!({"id": "o1", "name": " High ", "color": "red", "description": "first"}), json!("Low")]).unwrap();
        assert_eq!(given, vec![json!({"id": "o1", "name": "High", "color": "RED", "description": "first"}),
            json!({"name": "Low", "color": "GRAY", "description": ""})]);
        assert!(options_given(&[json!({"name": ""})]).is_err());
        assert!(options_given(&[json!({"name": "x", "color": "teal"})]).is_err());

        // An iteration plan starts where its first iteration does, unless told
        let plan = iterations_given(&json!({"days": 14, "iterations": [
            {"id": "i1", "title": "S1", "start": "2026-11-02", "days": 14}, {"title": "S2", "start": "2026-11-16", "days": 7}]})).unwrap();
        assert_eq!(plan["startDate"], "2026-11-02");
        assert_eq!(plan["iterations"][1], json!({"title": "S2", "startDate": "2026-11-16", "duration": 7}));
        assert!(iterations_given(&json!({"days": 14, "iterations": [{"title": "S", "start": "2026-13-40x", "days": 14}]})).is_err());
        assert!(iterations_given(&json!({"days": 0, "iterations": []})).is_err());

        // The cards are asked for with the newer parts of GitHub first, and without them when refused
        let (rich, plain) = (cards_query("user", true), cards_query("user", false));
        assert!(rich.contains("subIssuesSummary") && rich.contains("ProjectV2ItemFieldMultiSelectValue"));
        assert!(!plain.contains("subIssuesSummary") && !plain.contains("MultiSelect") && plain.contains("archivedStates:$states"));
        // No list is asked for inside the list of a card's field values: that is what a page of cards is priced by
        let values = rich.split("fieldValues(first:50)").nth(1).unwrap();
        assert!(!values.contains("(first:"), "{values}");
    }

    #[test]
    fn an_issue_or_pull_request_to_put_on_a_board_is_named_by_its_page_or_owner_repo_and_number() {
        assert_eq!(content_url("https://github.com/acme/shop/issues/12").as_deref(), Some("https://github.com/acme/shop/issues/12"));
        assert_eq!(content_url(" https://github.com/acme/shop/pull/7/?x=1#top ").as_deref(), Some("https://github.com/acme/shop/pull/7"));
        assert_eq!(content_url("acme/shop.web#40").as_deref(), Some("https://github.com/acme/shop.web/issues/40"));
        for bad in ["https://github.com/acme/shop", "https://github.com/acme/shop/issues/0", "https://github.com/acme/shop/tree/main",
            "https://example.com/acme/shop/issues/1", "acme/shop#", "acme#3", "a/b/c#3", "acme/shop#01", "acme/shop#1x", "Fix the thing", ""] {
            assert_eq!(content_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_value_is_sent_the_way_github_takes_it_and_nothing_else_is_sent() {
        assert_eq!(field_value(&json!({"option": "s2"})).unwrap(), json!({"singleSelectOptionId": "s2"}));
        assert_eq!(field_value(&json!({"iteration": "i1"})).unwrap(), json!({"iterationId": "i1"}));
        assert_eq!(field_value(&json!({"text": "x"})).unwrap(), json!({"text": "x"}));
        assert_eq!(field_value(&json!({"number": "2.5"})).unwrap(), json!({"number": 2.5}));
        assert_eq!(field_value(&json!({"date": "2026-11-20"})).unwrap(), json!({"date": "2026-11-20"}));
        for bad in [json!({"date": "2026/11/20"}), json!({"number": "many"}), json!({"color": "red"}), json!("s2")] {
            assert!(field_value(&bad).is_err(), "{bad}");
        }
    }
}
