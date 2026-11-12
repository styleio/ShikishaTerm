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
//! written, never read here. What is written is one card's value in one field.

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
    ... on ProjectV2SingleSelectField{options{id name color}} \
    ... on ProjectV2IterationField{configuration{iterations{id title startDate duration} completedIterations{id title startDate duration}}}";
const FIELD_ID: &str = "... on ProjectV2FieldCommon{id}";

/// A board, as one view of it shows it: its fields, its views, which view
/// this is, and its cards as that view's filter (or `filter`, when given)
/// picks them. `view` None is the address's view, else the first one
pub fn read(hub: &Hub, at: &GhRef, view: Option<u64>, filter: Option<&str>) -> Result<Value> {
    let org = match at.org {
        Some(o) => o,
        None => owner_is_org(hub, &at.owner)?,
    };
    let root = if org { "organization" } else { "user" };
    let shape = format!(
        "query($owner:String!,$number:Int!){{{root}(login:$owner){{projectV2(number:$number){{id number title url closed \
         fields(first:50){{nodes{{{FIELD}}}}} \
         views(first:30){{nodes{{number name layout filter fields(first:50){{nodes{{{FIELD_ID}}}}} \
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
    let cards = cards(hub, root, at, &filter)?;
    out["view"] = chosen["number"].clone();
    out["filter"] = json!(filter);
    out["columns"] = json!(column_field(&out["fields"], &chosen));
    out["cards"] = json!(cards);
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
            if let Some(options) = f.get("options").and_then(|o| o.as_array()) {
                field["options"] = json!(options.iter().map(|o| json!({"id": o["id"], "name": o["name"], "color": o["color"]})).collect::<Vec<_>>());
            }
            if let Some(c) = f.get("configuration") {
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
                "number": w["number"], "name": w["name"],
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
        "closed": node["closed"].as_bool().unwrap_or(false), "fields": fields, "views": views})
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

/// Every card the filter picks, in the board's own order. A view with more
/// than `MOST_CARDS` is refused rather than shown in part: a board missing
/// cards without saying so is worse than one that asks to be narrowed
fn cards(hub: &Hub, root: &str, at: &GhRef, filter: &str) -> Result<Vec<Value>> {
    let query = format!(
        "query($owner:String!,$number:Int!,$after:String,$filter:String){{{root}(login:$owner){{projectV2(number:$number){{\
         items(first:{CARDS_PER_PAGE},after:$after,query:$filter,orderBy:{{field:POSITION,direction:ASC}}){{totalCount pageInfo{{hasNextPage endCursor}} \
         nodes{{id type isArchived content{{__typename \
         ... on Issue{{number title url state stateReason updatedAt repository{{nameWithOwner}} assignees(first:5){{nodes{{login}}}} labels(first:8){{nodes{{name color}}}}}} \
         ... on PullRequest{{number title url state isDraft updatedAt repository{{nameWithOwner}} assignees(first:5){{nodes{{login}}}} labels(first:8){{nodes{{name color}}}}}} \
         ... on DraftIssue{{title updatedAt assignees(first:5){{nodes{{login}}}}}}}} \
         fieldValues(first:50){{nodes{{__typename \
         ... on ProjectV2ItemFieldTextValue{{text field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldNumberValue{{number field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldDateValue{{date field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldSingleSelectValue{{optionId name field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldIterationValue{{iterationId title startDate duration field{{{FIELD_ID}}}}} \
         ... on ProjectV2ItemFieldMilestoneValue{{milestone{{title}} field{{{FIELD_ID}}}}}}}}}}}}}}}}}}}"
    );
    let filter = (!filter.trim().is_empty()).then(|| filter.trim().to_string());
    let mut out = Vec::new();
    let mut after = Value::Null;
    loop {
        let v = hub.graphql_whole(&query, json!({"owner": at.owner, "number": at.number, "after": after, "filter": filter}))?;
        let Some(items) = v.pointer(&format!("/data/{root}/projectV2/items")).filter(|i| !i.is_null()) else {
            return Err(refused(&v));
        };
        let total = items["totalCount"].as_u64().unwrap_or(0);
        if total > MOST_CARDS {
            bail!(crate::i18n::tp("err.ghproject.too_many", &[("n", &total.to_string()), ("most", &MOST_CARDS.to_string())]));
        }
        out.extend(items.pointer("/nodes").and_then(|n| n.as_array()).into_iter().flatten().filter_map(read_card));
        match items.pointer("/pageInfo/endCursor") {
            Some(c) if items.pointer("/pageInfo/hasNextPage") == Some(&json!(true)) && out.len() < MOST_CARDS as usize => {
                after = c.clone()
            }
            _ => break,
        }
    }
    Ok(out)
}

/// One card: what it is about, and its value in each field it has one in.
/// A card for something in a repository the account cannot see is still a
/// card, and says so; an archived one is not on the board
pub fn read_card(node: &Value) -> Option<Value> {
    if node["isArchived"] == json!(true) {
        return None;
    }
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
    Some(json!({
        "id": node["id"], "kind": kind,
        "number": c["number"], "title": c["title"].as_str().unwrap_or_default(), "url": c["url"],
        "state": state, "repo": c.pointer("/repository/nameWithOwner"),
        "assignees": names("/assignees/nodes", "login"), "labels": labels,
        "updated": c["updatedAt"], "values": values,
    }))
}

/// A card's value in one field, set -- or taken off, for `value` null. The
/// value is said the way the field holds it: `{"option": id}`,
/// `{"iteration": id}`, `{"text": ..}`, `{"number": ..}` or `{"date": "YYYY-MM-DD"}`
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
            "content": {"__typename": "DraftIssue", "title": "Idea", "updatedAt": null,
                "assignees": {"nodes": [{"login": "lee"}]}}, "fieldValues": {"nodes": []}});
        let d = read_card(&draft).unwrap();
        assert_eq!(d["kind"], "draft");
        assert_eq!(d["assignees"], json!(["lee"]), "a draft card has people on it too");
        let archived = json!({"id": "PVTI_4", "isArchived": true});
        assert!(read_card(&archived).is_none());
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
