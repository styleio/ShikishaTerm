//! A new Markdown file, blank or from one of the folder's templates.
//!
//! The templates are files of the repository, in `.shikisha/templates/` at
//! the top of the working folder: a team writes its meeting notes, design
//! documents and reports there once, and everybody who has the repository has
//! them -- nothing about them is kept in the app's settings. A template is
//! any Markdown file there, in folders too; it is offered by the title its
//! front matter gives, else by its file name.
//!
//! What a template says is copied into the new file with a few words filled
//! in where it writes `{{ name }}`: `title` (from the new file's name),
//! `file` (the new file's name), `date` and `time` (now, here). A name the
//! app does not know is left as it was written, so a template can carry
//! braces of its own.
//!
//! Read and written through [`crate::disk::Disk`], so a folder on a server or
//! a MicroVM has its templates and gets its new files the same way.

use serde_json::{json, Value};

use crate::disk::{join, Disk};

/// Where a folder keeps its templates
pub const TEMPLATE_DIR: &str = ".shikisha/templates";

/// How deep into folders the templates are looked for, and how many are
/// offered: a team's kinds of document, sorted into a folder or two, not a
/// library -- and every one is read for its title on another machine too
const DEPTH: usize = 3;
const MOST: usize = 60;

/// A template bigger than this is not offered: what is copied is a page of
/// headings, not a document
const TEMPLATE_ROOM: usize = 256 * 1024;

fn is_markdown(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// The words a file's name stands for: without its ending, its dashes and
/// underscores as spaces
pub fn title_of(name: &str) -> String {
    let stem = name.rsplit('/').next().unwrap_or(name);
    let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
    let words: Vec<&str> = stem.split(['-', '_', ' ']).filter(|w| !w.is_empty()).collect();
    if words.is_empty() { stem.to_string() } else { words.join(" ") }
}

/// The title a template's front matter gives it (`title: …`), if it gives one
fn front_title(text: &str) -> Option<String> {
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    rest[..end].lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == "title").then(|| v.trim().trim_matches(['"', '\'']).to_string())
    }).filter(|t| !t.is_empty() && !t.contains("{{"))
}

/// The templates of a folder: (where, under the templates' folder; what it
/// is called), in the order they are offered
pub fn templates(disk: &dyn Disk) -> Result<Vec<(String, String)>, String> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut walk: Vec<(String, usize)> = vec![(String::new(), 0)];
    while let Some((rel, depth)) = walk.pop() {
        let dir = if rel.is_empty() { TEMPLATE_DIR.to_string() } else { join(TEMPLATE_DIR, &rel) };
        let Some(mut files) = disk.files(&dir)? else { continue };
        files.sort();
        for f in files.into_iter().filter(|f| is_markdown(f)) {
            if found.len() >= MOST {
                break;
            }
            let at = join(&rel, &f);
            let name = match disk.read(&join(TEMPLATE_DIR, &at))? {
                Some(b) if b.len() <= TEMPLATE_ROOM && !b.contains(&0) => {
                    front_title(&crate::charset::read(&b).text).unwrap_or_else(|| title_of(&f))
                }
                _ => continue,
            };
            found.push((at, name));
        }
        if depth + 1 < DEPTH {
            for d in disk.folders(&dir)? {
                if !d.starts_with('.') {
                    walk.push((join(&rel, &d), depth + 1));
                }
            }
        }
    }
    found.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    Ok(found)
}

/// A template's words with the app's names filled in
pub fn fill(text: &str, file: &str, date: &str, time: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let Some(len) = rest[open + 2..].find("}}") else { break };
        let key = rest[open + 2..open + 2 + len].trim();
        let value = match key {
            "title" => Some(title_of(file)),
            "file" => Some(file.to_string()),
            "date" => Some(date.to_string()),
            "time" => Some(time.to_string()),
            _ => None,
        };
        out.push_str(&rest[..open]);
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[open..open + 4 + len]),
        }
        rest = &rest[open + 4 + len..];
    }
    out.push_str(rest);
    out
}

/// One of the questions the file list asks about new Markdown files:
/// `templates` (what the folder offers) or `newmd` (make one: `at` the folder
/// it goes in, `name` its name, `template` the one it starts from, if any)
pub fn answer(disk: &dyn Disk, act: &str, args: &Value, panel: &str) -> String {
    let fail = |e: String| json!({"act": act, "panel": panel, "ok": false, "error": e}).to_string();
    let str_of = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    match act {
        "templates" => match templates(disk) {
            Ok(list) => json!({
                "act": act, "panel": panel, "ok": true, "dir": TEMPLATE_DIR,
                "templates": list.into_iter().map(|(path, name)| json!({"path": path, "name": name})).collect::<Vec<_>>(),
            })
            .to_string(),
            Err(e) => fail(e),
        },
        "newmd" => {
            let mut name = str_of("name");
            if !is_markdown(&name) {
                name.push_str(".md");
            }
            let Some(path) = crate::files::inside(&str_of("at"), &name) else {
                return fail(crate::i18n::t("err.files.bad_name"));
            };
            let template = str_of("template");
            let text = if template.is_empty() {
                String::new()
            } else {
                if template.split('/').any(|p| p == ".." || p.is_empty()) {
                    return fail(crate::i18n::t("err.sftp.outside"));
                }
                match disk.read(&join(TEMPLATE_DIR, &template)) {
                    Ok(Some(b)) => crate::charset::read(&b).text,
                    Ok(None) => return fail(crate::i18n::tp("err.mdnew.no_template", &[("name", &template)])),
                    Err(e) => return fail(e),
                }
            };
            let file = path.rsplit('/').next().unwrap_or(&path).to_string();
            let text = fill(&text, &file, &crate::hooks::local_stamp("%Y-%m-%d"), &crate::hooks::local_stamp("%H:%M"));
            let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            if let Err(e) = disk.make_dirs(dir).and_then(|()| disk.write(&path, text.as_bytes(), true)) {
                return fail(e);
            }
            json!({"act": act, "panel": panel, "ok": true, "path": path}).to_string()
        }
        _ => fail(format!("unknown act: {act}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Mem(RefCell<BTreeMap<String, Vec<u8>>>);

    impl Disk for Mem {
        fn files(&self, dir: &str) -> Result<Option<Vec<String>>, String> {
            let pre = format!("{dir}/");
            let map = self.0.borrow();
            let names: Vec<String> = map.keys().filter_map(|k| k.strip_prefix(&pre).filter(|r| !r.contains('/')).map(str::to_string)).collect();
            Ok((!names.is_empty() || map.keys().any(|k| k.starts_with(&pre))).then_some(names))
        }
        fn folders(&self, dir: &str) -> Result<Vec<String>, String> {
            let pre = format!("{dir}/");
            let mut out: Vec<String> = self.0.borrow().keys()
                .filter_map(|k| k.strip_prefix(&pre).and_then(|r| r.split_once('/')).map(|(d, _)| d.to_string())).collect();
            out.dedup();
            Ok(out)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.borrow().get(path).cloned())
        }
        fn write(&self, path: &str, bytes: &[u8], fresh: bool) -> Result<(), String> {
            let mut map = self.0.borrow_mut();
            if fresh && map.contains_key(path) {
                return Err("exists".into());
            }
            map.insert(path.into(), bytes.to_vec());
            Ok(())
        }
        fn make_dirs(&self, _dir: &str) -> Result<(), String> {
            Ok(())
        }
    }

    fn put(m: &Mem, path: &str, text: &str) {
        m.0.borrow_mut().insert(path.into(), text.as_bytes().to_vec());
    }

    #[test]
    fn the_templates_are_found_in_their_folders_and_named_by_their_title() {
        let m = Mem::default();
        put(&m, ".shikisha/templates/meeting-notes.md", "# {{title}}\n");
        put(&m, ".shikisha/templates/design/adr_short.md", "---\ntitle: 短い設計メモ\n---\n# x\n");
        put(&m, ".shikisha/templates/design/deep/deeper/too-deep.md", "x");
        put(&m, ".shikisha/templates/readme.txt", "not markdown");
        let list = templates(&m).unwrap();
        let names: Vec<&str> = list.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, ["meeting notes", "短い設計メモ"]);
        assert_eq!(list[1].0, "design/adr_short.md");
        assert!(templates(&Mem::default()).unwrap().is_empty(), "a folder with no templates offers none");
    }

    #[test]
    fn a_template_is_filled_in_and_the_names_it_does_not_know_are_left() {
        assert_eq!(
            fill("# {{ title }}\n{{date}} {{time}} {{file}} {{author}} {{", "weekly-sync.md", "2026-10-06", "09:30"),
            "# weekly sync\n2026-10-06 09:30 weekly-sync.md {{author}} {{"
        );
        assert_eq!(fill("{{title}}", "議事録.md", "", ""), "議事録");
    }

    #[test]
    fn a_new_file_is_made_where_it_was_asked_and_never_over_another() {
        let m = Mem::default();
        put(&m, ".shikisha/templates/notes.md", "# {{title}}\n\n{{date}}\n");
        let made: Value = serde_json::from_str(&answer(&m, "newmd", &json!({"at": "docs", "name": "定例会", "template": "notes.md"}), "p")).unwrap();
        assert_eq!(made["path"], "docs/定例会.md", "{made}");
        let text = String::from_utf8(m.0.borrow().get("docs/定例会.md").cloned().unwrap()).unwrap();
        assert!(text.starts_with("# 定例会\n\n20"), "{text}");
        let again: Value = serde_json::from_str(&answer(&m, "newmd", &json!({"at": "docs", "name": "定例会.md"}), "p")).unwrap();
        assert_eq!(again["ok"], false, "a second file of the same name was written over the first");
        let blank: Value = serde_json::from_str(&answer(&m, "newmd", &json!({"at": "", "name": "empty.markdown"}), "p")).unwrap();
        assert_eq!(blank["path"], "empty.markdown");
        let out: Value = serde_json::from_str(&answer(&m, "newmd", &json!({"at": "", "name": "x", "template": "../secret.md"}), "p")).unwrap();
        assert_eq!(out["ok"], false);
    }
}
