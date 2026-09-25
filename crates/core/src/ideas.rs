//! Ideas: short notes jotted down from the side column, each one a card.
//!
//! A card belongs to one project, or to none. It is written the moment it is
//! typed -- there is no Save -- ticked off when it is done, and kept in the
//! order somebody dragged it into.
//!
//! Kept in `config/ideas.json`, beside the settings: what somebody wrote is
//! theirs, and a folder the app may clear out (`data`) is no place for it. One
//! JSON file is plenty for what this holds -- a few thousand cards is a few
//! hundred kilobytes, rewritten whole in no time -- and it is a file a person
//! can open and read when the app is not there to show it.
//!
//! The file is read again for every request rather than held in memory. The
//! settings folder can be one that another PC writes to as well, and a copy
//! held here would put back, on its next change, whatever that PC had since
//! written.
//!
//! A project is a git repository: its checkout and every worktree cut from it
//! are one project, wherever those folders are, and a folder that is no
//! repository is no project. Which repository a folder is part of is asked of
//! git's own files. A card whose project is in no desk any more goes to "no
//! project" the next time the list is read -- but not while any folder of the
//! settings cannot be looked at (a drive that is not there today), since then
//! a missing project may only be a missing drive.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::config::Desk;
use crate::uistate::same_folder;

/// The file's shape, versioned so a later one can refuse to read this rather
/// than half-understand it
const VERSION: u32 = 1;
const FILE: &str = "ideas.json";

/// One card
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Idea {
    pub id: u64,
    /// The project it is about, by [`Project::key`]. None is no project
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub done: bool,
    /// When it was written and last changed, in seconds since 1970
    #[serde(default)]
    pub made: u64,
    #[serde(default)]
    pub changed: u64,
    /// The issue it became. Written when an issue sent from it was made, which
    /// is also when it was marked done
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueRef>,
}

/// An issue, by what a person reads and what a press opens
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IssueRef {
    pub number: u64,
    #[serde(default)]
    pub url: String,
}

/// Everything in the file. The order of `items` is the order on screen
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Store {
    version: u32,
    #[serde(default)]
    next: u64,
    #[serde(default)]
    items: Vec<Idea>,
}

impl Default for Store {
    fn default() -> Self {
        Store { version: VERSION, next: 1, items: Vec::new() }
    }
}

/// A project a card can belong to
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Project {
    /// Where the repository's own checkout is, with the machine in front for
    /// one reached over SSH
    pub key: String,
    /// What it is called on screen
    pub name: String,
    /// The folders of it the settings list, so the screen can tell which
    /// project the folder in front belongs to without working it out again
    pub folders: Vec<String>,
}

/// The projects of the settings, and whether every folder could be looked at.
/// Only a list that is `whole` may say a project has been deleted
#[derive(Debug, Clone, Default)]
pub struct Known {
    pub list: Vec<Project>,
    pub whole: bool,
}

/// Where the cards are kept
pub fn path() -> PathBuf {
    crate::config::root_dir().join("config").join(FILE)
}

/// The projects of every desk, each once.
///
/// Every desk and not only the one in front: a card written about a project
/// of another desk is not about a deleted project
pub fn projects(desks: &[Desk]) -> Known {
    let mut out = Known { list: Vec::new(), whole: !desks.is_empty() };
    for desk in desks {
        for f in &desk.folders {
            let Some(cwd) = f.cwd.as_deref() else { continue };
            let (key, checkout) = match &f.host {
                // On another machine nothing here can read its git folder; the
                // project says where it is checked out over there, and a folder
                // there that names no project with a checkout there is no project
                Some(h) => {
                    let home = f
                        .project
                        .as_deref()
                        .and_then(|n| desk.projects.iter().find(|p| p.name == n))
                        .and_then(|p| p.home_on(&h.name));
                    let Some(at) = home.map(|x| x.at.trim()).filter(|p| !p.is_empty()) else {
                        continue;
                    };
                    // Keyed by the address, as the cards already written are;
                    // a MicroVM has none, and is keyed by its entry's name
                    let machine = match h.at.trim() {
                        "" => h.name.as_str(),
                        at => at,
                    };
                    (format!("{machine}|{at}"), PathBuf::from(at))
                }
                None => {
                    if !cwd.exists() {
                        out.whole = false;
                        continue;
                    }
                    let Some(family) = crate::repo::family_of(cwd) else { continue };
                    // The git folder of a checkout is inside it; a bare
                    // repository is its own folder
                    let checkout = match family.file_name().is_some_and(|n| n.eq_ignore_ascii_case(".git")) {
                        true => family.parent().map(Path::to_path_buf).unwrap_or(family),
                        false => family,
                    };
                    (checkout.display().to_string(), checkout)
                }
            };
            let folder = cwd.display().to_string();
            if let Some(p) = out.list.iter_mut().find(|p| same_key(&p.key, &key)) {
                if !p.folders.iter().any(|x| same_folder(Path::new(x), cwd)) {
                    p.folders.push(folder);
                }
                continue;
            }
            // What the settings call it; else the repository's folder name
            let named = f
                .project
                .as_deref()
                .map(str::trim)
                .filter(|n| desk.projects.iter().any(|p| p.name == *n))
                .map(str::to_string);
            let name = named
                .or_else(|| {
                    checkout.file_name().map(|n| {
                        let n = n.to_string_lossy();
                        n.strip_suffix(".git").unwrap_or(&n).to_string()
                    })
                })
                .unwrap_or_else(|| key.clone());
            let name = match &f.host {
                Some(h) => format!("{name} ({})", h.name),
                None => name,
            };
            out.list.push(Project { key, name, folders: vec![folder] });
        }
    }
    out
}

/// Two keys for one project: the machine part exactly, the path the way
/// Windows compares paths
fn same_key(a: &str, b: &str) -> bool {
    let split = |k: &str| match k.rsplit_once('|') {
        Some((host, p)) => (host.to_string(), p.to_string()),
        None => (String::new(), k.to_string()),
    };
    let (ha, pa) = split(a);
    let (hb, pb) = split(b);
    ha == hb && same_folder(Path::new(&pa), Path::new(&pb))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read the file. None when it is from a later version: that is left alone
/// rather than overwritten, since the person may go back to the build that
/// wrote it
fn read(file: &Path) -> Result<Store, String> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Ok(Store::default());
    };
    match serde_json::from_str::<Store>(text.trim_start_matches('\u{feff}')) {
        Ok(s) if s.version <= VERSION => Ok(s),
        Ok(_) => Err(crate::i18n::t("err.ideas.newer")),
        // Not written over: what somebody wrote is still in there, and a file
        // that cannot be read today is one a person can still open and fix
        Err(e) => Err(crate::i18n::tp("err.ideas.unreadable", &[("why", &e.to_string())])),
    }
}

fn write(file: &Path, store: &Store) -> Result<(), String> {
    let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    crate::crypto::write_atomic(file, &text).map_err(|e| format!("{e:#}"))
}

/// Answer one request from the ideas window, and write what it changed.
///
/// Every answer carries the whole list and the projects, so a window and a
/// phone looking at the same cards both end up showing what is in the file.
/// `ref` goes back as it came, so the screen can tell which card it just asked
/// to be made
pub fn answer(file: &Path, act: &str, args: &Value, known: &Known) -> Value {
    let projects = &known.list;
    let reply = |ok: bool, store: Option<&Store>, extra: Value| {
        let mut v = json!({
            "act": act,
            "ok": ok,
            "ref": args.get("ref").cloned().unwrap_or(Value::Null),
            "projects": projects,
        });
        if let Some(s) = store {
            v["items"] = json!(s.items);
        }
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        v
    };
    let mut store = match read(file) {
        Ok(s) => s,
        Err(why) => return reply(false, None, json!({"error": why})),
    };
    // A project by its key, or by one of its folders: a card written when
    // its project was named by a folder finds the repository that folder is in
    let known_as = |key: &str| {
        projects
            .iter()
            .find(|p| same_key(&p.key, key) || p.folders.iter().any(|f| same_key(f, key)))
            .map(|p| p.key.clone())
    };
    let id = args.get("id").and_then(Value::as_u64).unwrap_or(0);
    let mut changed = false;
    let mut made = Value::Null;
    // Cards whose project has gone go to no project, and a card named by a
    // folder of its project is named by the project. Only from a whole list:
    // a folder that could not be looked at may be the project's only one
    for i in store.items.iter_mut() {
        let Some(k) = i.project.clone() else { continue };
        match known_as(&k) {
            Some(key) if key != k => {
                i.project = Some(key);
                changed = true;
            }
            Some(_) => {}
            None if known.whole => {
                i.project = None;
                changed = true;
            }
            None => {}
        }
    }
    match act {
        "list" => {}
        "add" => {
            let project = args
                .get("project")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .and_then(known_as);
            let at = now();
            let new_id = store.next.max(store.items.iter().map(|i| i.id + 1).max().unwrap_or(1));
            store.next = new_id + 1;
            let idea = Idea {
                id: new_id,
                project,
                text: args.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
                done: false,
                made: at,
                changed: at,
                issue: None,
            };
            // Straight after the card it was made from, or at the end
            let after = args.get("after").and_then(Value::as_u64);
            match after.and_then(|a| store.items.iter().position(|i| i.id == a)) {
                Some(p) => store.items.insert(p + 1, idea),
                None => store.items.push(idea),
            }
            made = json!(new_id);
            changed = true;
        }
        "edit" | "done" | "issued" => {
            let Some(at) = store.items.iter().position(|i| i.id == id) else {
                return reply(false, Some(&store), json!({"error": crate::i18n::t("err.ideas.gone")}));
            };
            let i = &mut store.items[at];
            if act == "edit" {
                let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
                if i.text != text {
                    i.text = text.to_string();
                    i.changed = now();
                    changed = true;
                }
            } else if act == "issued" {
                // Made into an issue: done, and which issue it became
                let number = args.get("number").and_then(Value::as_u64).unwrap_or(0);
                let url = args.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
                if number > 0 {
                    i.done = true;
                    i.issue = Some(IssueRef { number, url });
                    i.changed = now();
                    changed = true;
                }
            } else {
                let done = args.get("done").and_then(Value::as_bool).unwrap_or(true);
                if i.done != done {
                    i.done = done;
                    i.changed = now();
                    changed = true;
                }
            }
        }
        "drop" => {
            let before = store.items.len();
            store.items.retain(|i| i.id != id);
            changed |= store.items.len() != before;
        }
        // The cards on screen, in their new order. They take the places those
        // same cards held, so the cards of other projects stay where they were
        "order" => {
            let ids: Vec<u64> = args
                .get("ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect();
            let slots: Vec<usize> = store
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| ids.contains(&i.id))
                .map(|(n, _)| n)
                .collect();
            let wanted: Vec<Idea> = ids
                .iter()
                .filter_map(|want| store.items.iter().find(|i| i.id == *want).cloned())
                .collect();
            if wanted.len() == slots.len() {
                for (slot, idea) in slots.into_iter().zip(wanted) {
                    if store.items[slot].id != idea.id {
                        changed = true;
                    }
                    store.items[slot] = idea;
                }
            }
        }
        other => return reply(false, Some(&store), json!({"error": format!("unknown request {other}")})),
    }
    if changed {
        store.version = VERSION;
        if let Err(why) = write(file, &store) {
            return reply(false, Some(&store), json!({"error": crate::i18n::tp("err.ideas.write", &[("why", &why)])}));
        }
    }
    reply(true, Some(&store), json!({"made": made}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shikisha-ideas-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config").join(FILE)
    }

    fn project(key: &str) -> Project {
        Project { key: key.to_string(), name: key.to_string(), folders: vec![key.to_string()] }
    }

    fn known(ps: &[Project]) -> Known {
        Known { list: ps.to_vec(), whole: true }
    }

    fn ids(v: &Value) -> Vec<u64> {
        v["items"].as_array().unwrap().iter().map(|i| i["id"].as_u64().unwrap()).collect()
    }

    /// Written the moment it is asked for, and still there when read again:
    /// nothing is held in memory between two requests
    #[test]
    fn a_card_is_in_the_file_as_soon_as_it_is_written() {
        let f = temp("write");
        let ps = [project("D:\\app")];
        // Another case and a trailing separator: different on every system
        // this runs on in spelling only
        let a = answer(&f, "add", &json!({"project": "d:\\APP\\", "text": "first", "ref": "r1"}), &known(&ps));
        assert_eq!(a["ok"], true);
        assert_eq!(a["ref"], "r1", "the screen cannot tell which card it asked for");
        let id = a["made"].as_u64().unwrap();
        assert_eq!(a["items"][0]["project"], "D:\\app", "a project spelled another way was not recognised");
        answer(&f, "edit", &json!({"id": id, "text": "first, changed"}), &known(&ps));
        let again = answer(&f, "list", &json!({}), &known(&ps));
        assert_eq!(again["items"][0]["text"], "first, changed");
        let on_disk: Store = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(on_disk.items.len(), 1);
    }

    /// A new card goes straight after the one Enter was pressed in, and ids
    /// are never given twice, even after the newest card is deleted
    #[test]
    fn a_card_is_made_after_the_one_it_came_from() {
        let f = temp("after");
        let ps = [project("D:\\app")];
        let one = answer(&f, "add", &json!({"text": "1"}), &known(&ps))["made"].as_u64().unwrap();
        let two = answer(&f, "add", &json!({"text": "2"}), &known(&ps))["made"].as_u64().unwrap();
        let mid = answer(&f, "add", &json!({"text": "1.5", "after": one}), &known(&ps));
        let mid_id = mid["made"].as_u64().unwrap();
        assert_eq!(ids(&mid), vec![one, mid_id, two]);
        answer(&f, "drop", &json!({"id": two}), &known(&ps));
        let next = answer(&f, "add", &json!({"text": "3"}), &known(&ps))["made"].as_u64().unwrap();
        assert!(next > mid_id, "an id was given again");
    }

    /// Ticking a card off keeps it, marked done; ticking it again brings it back
    #[test]
    fn done_is_kept_not_deleted() {
        let f = temp("done");
        let ps = [project("D:\\app")];
        let id = answer(&f, "add", &json!({"text": "x"}), &known(&ps))["made"].as_u64().unwrap();
        let v = answer(&f, "done", &json!({"id": id, "done": true}), &known(&ps));
        assert_eq!(v["items"][0]["done"], true);
        let v = answer(&f, "done", &json!({"id": id, "done": false}), &known(&ps));
        assert_eq!(v["items"][0]["done"], false);
    }

    /// An idea made into an issue is done and says which issue, and keeps
    /// saying it when it is ticked back to not done
    #[test]
    fn an_idea_made_into_an_issue_is_done_and_names_it() {
        let f = temp("issued");
        let ps = [project("D:\\app")];
        let id = answer(&f, "add", &json!({"project": "D:\\app", "text": "x"}), &known(&ps))["made"].as_u64().unwrap();
        let v = answer(&f, "issued", &json!({"id": id, "number": 12, "url": "https://github.com/o/r/issues/12"}), &known(&ps));
        assert_eq!(v["items"][0]["done"], true);
        assert_eq!(v["items"][0]["issue"]["number"], 12);
        assert_eq!(v["items"][0]["issue"]["url"], "https://github.com/o/r/issues/12");
        let v = answer(&f, "done", &json!({"id": id, "done": false}), &known(&ps));
        assert_eq!(v["items"][0]["issue"]["number"], 12, "not done any more, it forgot its issue");
        // An answer with no number is no issue, and changes nothing
        let other = answer(&f, "add", &json!({"text": "y"}), &known(&ps))["made"].as_u64().unwrap();
        let v = answer(&f, "issued", &json!({"id": other}), &known(&ps));
        assert_eq!(v["items"][1]["done"], false);
        assert!(v["items"][1].get("issue").is_none());
    }

    /// Reordering one project's cards moves them among their own places and
    /// leaves the other project's cards exactly where they were
    #[test]
    fn reordering_one_project_leaves_the_others_in_place() {
        let f = temp("order");
        let ps = [project("D:\\a"), project("D:\\b")];
        let a1 = answer(&f, "add", &json!({"project": "D:\\a", "text": "a1"}), &known(&ps))["made"].as_u64().unwrap();
        let b1 = answer(&f, "add", &json!({"project": "D:\\b", "text": "b1"}), &known(&ps))["made"].as_u64().unwrap();
        let a2 = answer(&f, "add", &json!({"project": "D:\\a", "text": "a2"}), &known(&ps))["made"].as_u64().unwrap();
        let v = answer(&f, "order", &json!({"ids": [a2, a1]}), &known(&ps));
        assert_eq!(ids(&v), vec![a2, b1, a1]);
        // A list that names a card that is not there changes nothing
        let v = answer(&f, "order", &json!({"ids": [a1, 999]}), &known(&ps));
        assert_eq!(ids(&v), vec![a2, b1, a1]);
    }

    /// A card whose project is in no desk any more goes to no project. Settings
    /// that list no projects at all move nothing: that is a load that failed
    #[test]
    fn a_deleted_project_sends_its_cards_to_no_project() {
        let f = temp("gone");
        let both = [project("D:\\a"), project("D:\\b")];
        answer(&f, "add", &json!({"project": "D:\\a", "text": "keep"}), &known(&both));
        answer(&f, "add", &json!({"project": "D:\\b", "text": "orphan"}), &known(&both));
        let none = answer(&f, "list", &json!({}), &Known::default());
        assert_eq!(none["items"][1]["project"], "D:\\b", "a list that is not whole moved cards");
        let v = answer(&f, "list", &json!({}), &known(&[project("D:\\a")]));
        assert_eq!(v["items"][0]["project"], "D:\\a");
        assert!(v["items"][1]["project"].is_null(), "the card of a deleted project kept it");
        // And an unknown project asked for when adding is no project
        let v = answer(&f, "add", &json!({"project": "D:\\z", "text": "?"}), &known(&[project("D:\\a")]));
        assert!(v["items"][2]["project"].is_null());
    }

    /// A file that cannot be read is left as it is: writing over it would lose
    /// what somebody wrote
    #[test]
    fn an_unreadable_file_is_not_written_over() {
        let f = temp("broken");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "{ not json").unwrap();
        let v = answer(&f, "add", &json!({"text": "x"}), &Known::default());
        assert_eq!(v["ok"], false);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{ not json");
        std::fs::write(&f, r#"{"version": 99, "items": []}"#).unwrap();
        let v = answer(&f, "add", &json!({"text": "x"}), &Known::default());
        assert_eq!(v["ok"], false, "a file from a later version was written over");
    }

    /// A card named by a folder of its project comes to be named by the project
    #[test]
    fn a_card_named_by_a_folder_moves_to_its_project() {
        let f = temp("folder");
        let app = Project {
            key: "D:\\app".into(),
            name: "app".into(),
            folders: vec!["D:\\app".into(), "E:\\wt\\fix".into()],
        };
        let old = answer(&f, "add", &json!({"text": "x"}), &known(&[]));
        let id = old["made"].as_u64().unwrap();
        // Written by an earlier version, which named the worktree's folder
        let mut store: Store = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        store.items[0].project = Some("e:\\WT\\fix\\".into());
        std::fs::write(&f, serde_json::to_string(&store).unwrap()).unwrap();
        let v = answer(&f, "list", &json!({}), &known(&[app]));
        assert_eq!(v["items"][0]["id"], id);
        assert_eq!(v["items"][0]["project"], "D:\\app", "the worktree's card did not find its repository");
    }

    /// A project is a repository: its checkout and a worktree cut from it in
    /// another place entirely are one project, a folder that is no repository
    /// is none, and a folder that cannot be looked at makes the list not whole
    #[test]
    fn a_project_is_a_repository_however_its_folders_are_spread() {
        let root = std::env::temp_dir().join(format!("shikisha-ideas-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let app = root.join("app");
        let wt = root.join("elsewhere").join("fix");
        let notes = root.join("notes");
        std::fs::create_dir_all(app.join(".git").join("worktrees").join("fix")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&notes).unwrap();
        // What `git worktree add` leaves: a .git file naming the worktree's own
        // git folder, and in that folder the way back to the shared one
        let own = app.join(".git").join("worktrees").join("fix");
        std::fs::write(wt.join(".git"), format!("gitdir: {}", own.display())).unwrap();
        std::fs::write(own.join("commondir"), "../..").unwrap();
        let folder = |p: &Path| crate::config::Folder {
            name: None,
            id: None,
            host: None,
            cwd: Some(p.to_path_buf()),
            source: Default::default(),
            protect: Vec::new(),
            project: None,
            work_item: None,
            summary: None,
            auto_label: false,
            drawn: None,
        };
        let one = Desk { folders: vec![folder(&app), folder(&notes)], ..Default::default() };
        let two = Desk { folders: vec![folder(&wt)], ..Default::default() };
        let k = projects(&[one, two]);
        assert!(k.whole);
        assert_eq!(k.list.len(), 1, "{:?}", k.list);
        assert_eq!(k.list[0].name, "app");
        assert_eq!(k.list[0].folders.len(), 2, "the worktree elsewhere is not counted as the repository's");
        // Spelled the way git's files resolve, which may not be the way the
        // temporary folder was written (a short name, a link): named by its
        // last part rather than compared whole
        assert_eq!(Path::new(&k.list[0].key).file_name().and_then(|n| n.to_str()), Some("app"));
        let gone = Desk { folders: vec![folder(&app), folder(&root.join("not-here"))], ..Default::default() };
        assert!(!projects(&[gone]).whole, "a folder that is not there still let a project be called deleted");
        assert!(!projects(&[]).whole, "no desks at all is a whole list");
        let _ = std::fs::remove_dir_all(&root);
    }
}
