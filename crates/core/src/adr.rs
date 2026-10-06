//! Decision records (ADR): the Markdown files a project keeps to say what it
//! decided and why, in the shape MADR gives them.
//!
//! **The files are the record.** Nothing here is kept anywhere but in the
//! repository's own folder, written in the format anybody else's tools read:
//! a team where one person uses this app and the next uses an editor sees the
//! same records, and taking the app away takes nothing with it. What the app
//! adds is what a folder cannot do by itself: write a draft at the moment
//! something was decided, keep a record and the one it replaces saying the
//! same thing about each other, and list them where the work is.
//!
//! The form a new record is written in comes from the folder too: a template
//! there (`adr-template.md`, the name MADR hands out) is the team's choice of
//! sections, shared through git like the records themselves. A folder without
//! one is written in MADR's own sections ([`MADR`]).
//!
//! Every read and write goes through [`Disk`], so the same code works on a
//! folder of this PC and on one over SSH or on a MicroVM, behind the same
//! fence the file list uses.

use serde::Serialize;
use serde_json::{json, Value};

/// Where a project's records are, when nobody has said: MADR's own answer
pub const DEFAULT_DIR: &str = "docs/decisions";

/// Folders other tools and teams keep records in, looked for when a project
/// turns records on, so a repository that already has some is offered its
/// own folder rather than a second one beside it. MADR's first, then the
/// common spellings of the older tools
pub const KNOWN_DIRS: &[&str] = &[
    "docs/decisions",
    "docs/adr",
    "docs/adrs",
    "doc/adr",
    "doc/decisions",
    "docs/architecture/decisions",
    "doc/architecture/decisions",
    "docs/architecture-decisions",
    "adr",
    "decisions",
];

/// Names in the folder that are not records: the template, and the index a
/// person or a tool keeps beside them
const NOT_RECORDS: &[&str] = &["readme.md", "index.md", "adr-template.md", "template.md", "toc.md"];

/// The template files looked for, in order
const TEMPLATES: &[&str] = &["adr-template.md", "template.md"];

/// A record bigger than this is not read: nobody writes a decision that long,
/// and a folder with a generated file in it should not make the list slow
const RECORD_LIMIT: usize = 512 * 1024;

/// How much of each record is handed to the page for searching. The title and
/// the first pages are where the words somebody searches for are
const SEARCH_ROOM: usize = 12_000;

/// How many records a folder is read for. Far more than any project keeps;
/// the ceiling is for a folder that turned out to be something else
const MOST: usize = 2000;

/// Room for the words of a file name taken from a title, in characters
const SLUG_ROOM: usize = 60;

/// One section of the form: a heading of the template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    /// The heading, as written in the file
    pub heading: String,
    /// 2 for `##`, 3 for `###`
    pub level: u8,
    /// Whether the template says it can be left out
    pub optional: bool,
    /// What the template says goes here, with its placeholders taken out.
    /// Empty for MADR's own sections: the page has its own words for those
    pub hint: String,
}

/// MADR 4's sections, in its order. The headings are MADR's exactly: they are
/// what makes a file one every MADR tool reads
pub fn madr() -> Vec<Field> {
    let f = |heading: &str, level: u8, optional: bool| Field {
        heading: heading.to_string(),
        level,
        optional,
        hint: String::new(),
    };
    vec![
        f("Context and Problem Statement", 2, false),
        f("Decision Drivers", 2, true),
        f("Considered Options", 2, false),
        f("Decision Outcome", 2, false),
        f("Consequences", 3, true),
        f("Confirmation", 3, true),
        f("Pros and Cons of the Options", 2, true),
        f("More Information", 2, true),
    ]
}

/// The sections a template file asks for.
///
/// A heading with a placeholder in it (`### {title of option 1}`) is the
/// template showing what goes inside the section above it, not a section of
/// its own. A section is optional when the template says so in a comment,
/// the way MADR's does. `None` when the file has no sections at all, which is
/// not a template the form can be built from
pub fn template_fields(text: &str) -> Option<Vec<Field>> {
    let mut out: Vec<Field> = Vec::new();
    let mut body: Vec<String> = Vec::new();
    let finish = |out: &mut Vec<Field>, body: &mut Vec<String>| {
        if let Some(last) = out.last_mut() {
            let joined = body.join("\n");
            last.optional = joined.to_lowercase().contains("optional");
            last.hint = hint_of(&joined);
        }
        body.clear();
    };
    let mut fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
        }
        let heading = if fence { None } else { heading_of(line) };
        match heading {
            Some((level, words)) if (level == 2 || level == 3) && !words.contains('{') => {
                finish(&mut out, &mut body);
                out.push(Field { heading: words, level, optional: false, hint: String::new() });
            }
            _ => body.push(line.to_string()),
        }
    }
    finish(&mut out, &mut body);
    (!out.is_empty()).then_some(out)
}

/// What a template says goes in a section, as one line a person can read
/// under the box: its first words, without the comments and the braces
fn hint_of(body: &str) -> String {
    let mut text = String::new();
    let mut rest = body;
    while let Some(at) = rest.find("<!--") {
        text.push_str(&rest[..at]);
        rest = match rest[at..].find("-->") {
            Some(end) => &rest[at + end + 3..],
            None => "",
        };
    }
    text.push_str(rest);
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .trim_start_matches(['*', '-', ' '])
        .replace(['{', '}'], "");
    cut_chars(line.trim(), 200)
}

/// `## Words` as (2, "Words"); `None` for anything else
fn heading_of(line: &str) -> Option<(u8, String)> {
    let t = line.trim_end();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 || !t[hashes..].starts_with(' ') {
        return None;
    }
    Some((hashes as u8, t[hashes..].trim().to_string()))
}

/// A string cut to `room` characters -- characters, so a Japanese title is
/// never cut through the middle of one
fn cut_chars(s: &str, room: usize) -> String {
    s.chars().take(room).collect()
}

/// What a record says it is, as the list shows it
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Record {
    /// Its file name in the folder
    pub file: String,
    /// The number its name begins with, when it begins with one
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    pub title: String,
    /// One of `proposed`, `accepted`, `rejected`, `deprecated`,
    /// `superseded`, or whatever else the file wrote, in lower case.
    /// Empty when it says nothing
    pub status: String,
    /// The record that replaced this one: its file name when the link names
    /// one in the folder, else what was written
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub makers: String,
    /// Who was asked before it was decided -- people, and the AI the
    /// decision was talked through with (`Claude Code (AI)`)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub consulted: String,
    /// Whether the file is written in the form's sections, so the form can
    /// open it. A record in another format is edited in the editor
    pub fits: bool,
    /// The words, for the page to search in
    pub text: String,
}

/// The statuses MADR names, the ones the page draws in its own words
pub const STATUSES: &[&str] = &["proposed", "accepted", "rejected", "deprecated", "superseded"];

/// A record read: what the list shows of it.
///
/// Reads MADR's front matter, and the `## Status` section the older formats
/// (and MADR 2) write instead. `fields` are the form's sections, which decide
/// whether this record is one the form can open
pub fn parse(file: &str, text: &str, fields: &[Field]) -> Record {
    let doc = Doc::read(text, fields);
    let mut status_raw = doc.front_value("status").unwrap_or_default();
    if status_raw.is_empty()
        && let Some(s) = doc.sections.iter().find(|s| s.heading.eq_ignore_ascii_case("status"))
    {
        status_raw = s.body.iter().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or_default().to_string();
    }
    let (status, by) = status_words(&status_raw);
    let fits = doc.sections.iter().any(|s| fields.iter().any(|f| f.heading.eq_ignore_ascii_case(&s.heading)));
    Record {
        file: file.to_string(),
        number: number_of(file),
        title: display_title(&doc.title().unwrap_or_else(|| file.trim_end_matches(".md").to_string())),
        status,
        by,
        date: doc.front_value("date").unwrap_or_default(),
        makers: doc.front_value("decision-makers").or_else(|| doc.front_value("deciders")).unwrap_or_default(),
        consulted: doc.front_value("consulted").unwrap_or_default(),
        fits,
        text: cut_chars(text, SEARCH_ROOM),
    }
}

/// A status as written, read: the word, and for "superseded by", what by.
/// The link's target when there is one (`[ADR-0005](0005-x.md)`), else the
/// words after "by"
fn status_words(raw: &str) -> (String, Option<String>) {
    let t = raw.trim().trim_matches(['"', '\'']).trim();
    let lower = t.to_lowercase();
    if t.get(..10).is_some_and(|w| w.eq_ignore_ascii_case("superseded")) {
        let rest = t[10..].trim_start();
        let rest = rest.strip_prefix("by").or_else(|| rest.strip_prefix("By")).unwrap_or(rest).trim();
        let by = link_target(rest).unwrap_or_else(|| rest.to_string());
        return ("superseded".to_string(), Some(by).filter(|b| !b.is_empty()));
    }
    // "{proposed | rejected | ...}" left as the template wrote it is no status
    if lower.contains('{') || lower.contains('|') {
        return (String::new(), None);
    }
    (lower.split_whitespace().next().unwrap_or_default().to_string(), None)
}

/// The target of the first Markdown link in some words
fn link_target(words: &str) -> Option<String> {
    let open = words.find("](")?;
    let rest = &words[open + 2..];
    let close = rest.find(')')?;
    let target = rest[..close].trim();
    // Only the file's name: a record links to its neighbours by name
    Some(target.rsplit('/').next().unwrap_or(target).to_string()).filter(|t| !t.is_empty())
}

/// The number a record's file name begins with: `0012-x.md`, `adr-012-x.md`
pub fn number_of(file: &str) -> Option<u64> {
    let digits: String = file
        .trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '-' || c == '_')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// A title as a person reads it: without the number the older tools put in
/// front of it (`1. Record decisions`, `ADR-0001: Use X`)
fn display_title(raw: &str) -> String {
    let t = raw.trim();
    let after_adr = t
        .get(..3)
        .filter(|w| w.eq_ignore_ascii_case("adr"))
        .map(|_| 3)
        .filter(|&at| t[at..].trim_start_matches(['-', ' ']).starts_with(|c: char| c.is_ascii_digit()));
    let rest = match after_adr {
        Some(at) => t[at..].trim_start_matches(['-', ' ']),
        None => t,
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let after = rest[digits..].trim_start();
        if let Some(stripped) = after.strip_prefix(['.', ':']) {
            return stripped.trim().to_string();
        }
        if after_adr.is_some() {
            return after.trim_start_matches('-').trim().to_string();
        }
    }
    t.to_string()
}

/// A record's file, taken apart into what the form edits. Everything the
/// form does not know about is kept as it was written, so writing a record
/// back changes only what was changed
#[derive(Debug, Clone, PartialEq, Eq)]
struct Doc {
    /// The front matter's lines, without the `---` around them. `None` when
    /// the file has none
    front: Option<Vec<String>>,
    /// Everything between the front matter and the first section: the title
    /// and whatever was written under it
    head: Vec<String>,
    sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    heading: String,
    level: u8,
    /// The heading line as written
    line: String,
    body: Vec<String>,
}

impl Doc {
    /// `fields` decide where one section ends: every `##`, and a `###` the
    /// form has a box for. Any other `###` (an option under "Pros and Cons")
    /// is part of the section it is in
    fn read(text: &str, fields: &[Field]) -> Doc {
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        let mut front = None;
        if lines.first().is_some_and(|l| l.trim_end() == "---")
            && let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---")
        {
            front = Some(lines[1..=end].iter().map(|l| l.to_string()).collect());
            i = end + 2;
        }
        let mut head = Vec::new();
        let mut sections: Vec<Section> = Vec::new();
        let mut fence = false;
        for line in &lines[i.min(lines.len())..] {
            if line.trim_start().starts_with("```") {
                fence = !fence;
            }
            let starts = (!fence).then(|| heading_of(line)).flatten().filter(|(level, words)| {
                *level == 2 || (*level == 3 && fields.iter().any(|f| f.level == 3 && f.heading.eq_ignore_ascii_case(words)))
            });
            match starts {
                Some((level, heading)) => sections.push(Section { heading, level, line: line.to_string(), body: Vec::new() }),
                None => match sections.last_mut() {
                    Some(s) => s.body.push(line.to_string()),
                    None => head.push(line.to_string()),
                },
            }
        }
        Doc { front, head, sections }
    }

    fn front_value(&self, key: &str) -> Option<String> {
        self.front.as_ref()?.iter().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(key).then(|| unquote(v.trim()))
        })
    }

    /// The `# ` line under the front matter
    fn title(&self) -> Option<String> {
        self.head.iter().find_map(|l| match heading_of(l) {
            Some((1, words)) => Some(words),
            _ => None,
        })
    }

    /// Set a key of the front matter, making the front matter when there is
    /// none. An empty value takes the key out
    fn set_front(&mut self, key: &str, value: &str) {
        let front = self.front.get_or_insert_with(Vec::new);
        let at = front.iter().position(|l| l.split_once(':').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key)));
        let line = format!("{key}: {}", quote_if_needed(value));
        match (at, value.trim().is_empty()) {
            (Some(i), true) => {
                front.remove(i);
            }
            (Some(i), false) => front[i] = line,
            (None, false) => front.push(line),
            (None, true) => {}
        }
    }

    fn set_title(&mut self, title: &str) {
        match self.head.iter().position(|l| matches!(heading_of(l), Some((1, _)))) {
            Some(i) => self.head[i] = format!("# {title}"),
            None => {
                self.head.insert(0, String::new());
                self.head.insert(0, format!("# {title}"));
            }
        }
    }

    /// Put a section's words in. A section the file has gets its body
    /// replaced; one it does not have goes after the last section it has
    /// that the form puts before this one. Empty words take the section out
    fn set_section(&mut self, field: &Field, words: &str, order: &[Field]) {
        let at = self.sections.iter().position(|s| s.heading.eq_ignore_ascii_case(&field.heading));
        let body = section_body(words);
        match (at, body.is_empty()) {
            (Some(i), true) => {
                self.sections.remove(i);
            }
            (Some(i), false) => self.sections[i].body = body,
            (None, true) => {}
            (None, false) => {
                let mine = order.iter().position(|f| f.heading.eq_ignore_ascii_case(&field.heading)).unwrap_or(usize::MAX);
                let after = self.sections.iter().rposition(|s| {
                    order.iter().position(|f| f.heading.eq_ignore_ascii_case(&s.heading)).is_some_and(|p| p < mine)
                });
                let section = Section {
                    heading: field.heading.clone(),
                    level: field.level,
                    line: format!("{} {}", "#".repeat(field.level as usize), field.heading),
                    body,
                };
                match after {
                    Some(i) => self.sections.insert(i + 1, section),
                    None => {
                        // Before the first section the form puts after it
                        let before = self.sections.iter().position(|s| {
                            order.iter().position(|f| f.heading.eq_ignore_ascii_case(&s.heading)).is_some_and(|p| p > mine)
                        });
                        self.sections.insert(before.unwrap_or(self.sections.len()), section);
                    }
                }
            }
        }
    }

    /// A section's words as the form shows them: the body without the blank
    /// lines around it
    fn section_words(&self, heading: &str) -> String {
        self.sections
            .iter()
            .find(|s| s.heading.eq_ignore_ascii_case(heading))
            .map(|s| trim_blank(&s.body).join("\n"))
            .unwrap_or_default()
    }

    fn write(&self, newline: &str) -> String {
        let mut out: Vec<String> = Vec::new();
        if let Some(front) = &self.front {
            out.push("---".into());
            out.extend(front.iter().cloned());
            out.push("---".into());
            out.push(String::new());
        }
        out.extend(trim_blank(&self.head));
        for s in &self.sections {
            out.push(String::new());
            out.push(s.line.clone());
            let body = trim_blank(&s.body);
            if !body.is_empty() {
                out.push(String::new());
                out.extend(body);
            }
        }
        // Whatever leading blank lines the head left
        while out.first().is_some_and(|l| l.trim().is_empty()) {
            out.remove(0);
        }
        let mut text = out.join(newline);
        text.push_str(newline);
        text
    }
}

fn section_body(words: &str) -> Vec<String> {
    trim_blank(&words.lines().map(|l| l.trim_end().to_string()).collect::<Vec<_>>())
}

fn trim_blank(lines: &[String]) -> Vec<String> {
    let first = lines.iter().position(|l| !l.trim().is_empty());
    let last = lines.iter().rposition(|l| !l.trim().is_empty());
    match (first, last) {
        (Some(a), Some(b)) => lines[a..=b].to_vec(),
        _ => Vec::new(),
    }
}

fn unquote(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 2 && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\''))) {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// A value YAML would read as something other than the words: quoted
fn quote_if_needed(v: &str) -> String {
    let t = v.trim();
    let risky = t.starts_with(['[', '{', '&', '*', '!', '|', '>', '%', '@', '`', '#', '"', '\''])
        || t.contains(": ")
        || t.contains(" #");
    if risky && !t.starts_with('[') {
        format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        t.to_string()
    }
}

/// What the form gives back
#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub title: String,
    /// Front matter, by MADR's keys: `status`, `date`, `decision-makers`,
    /// `consulted`, `informed`
    pub front: Vec<(String, String)>,
    /// Each section's words, by heading
    pub sections: Vec<(String, String)>,
}

impl Edit {
    pub fn from_json(v: &Value) -> Edit {
        let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
        let pairs = |key: &str, a: &str, b: &str| -> Vec<(String, String)> {
            v.get(key)
                .and_then(Value::as_array)
                .map(|rows| rows.iter().map(|r| (s(&r[a]), s(&r[b]))).filter(|(k, _)| !k.trim().is_empty()).collect())
                .unwrap_or_default()
        };
        Edit {
            title: s(&v["title"]).trim().to_string(),
            front: pairs("front", "key", "value"),
            sections: pairs("sections", "heading", "body"),
        }
    }
}

/// The keys of the front matter the form writes, in MADR's order
pub const FRONT_KEYS: &[&str] = &["status", "date", "decision-makers", "consulted", "informed"];

/// A new record's text, in the order the form's sections come in
pub fn compose(edit: &Edit, fields: &[Field]) -> String {
    let mut doc = Doc { front: None, head: Vec::new(), sections: Vec::new() };
    for key in FRONT_KEYS {
        if let Some((_, v)) = edit.front.iter().find(|(k, _)| k == key) {
            doc.set_front(key, v);
        }
    }
    doc.set_title(&edit.title);
    for field in fields {
        if let Some((_, words)) = edit.sections.iter().find(|(h, _)| h.eq_ignore_ascii_case(&field.heading)) {
            doc.set_section(field, words, fields);
        }
    }
    doc.write("\n")
}

/// A record's text with the form's changes written into it, keeping
/// everything the form does not show where it was
pub fn rewrite(text: &str, edit: &Edit, fields: &[Field]) -> String {
    let mut doc = Doc::read(text, fields);
    for (k, v) in &edit.front {
        doc.set_front(k, v);
    }
    if !edit.title.is_empty() {
        doc.set_title(&edit.title);
    }
    for field in fields {
        if let Some((_, words)) = edit.sections.iter().find(|(h, _)| h.eq_ignore_ascii_case(&field.heading)) {
            doc.set_section(field, words, fields);
        }
    }
    doc.write(newline_of(text))
}

/// What the form opens with for a record: its title, front matter and each
/// of the form's sections
pub fn parts(text: &str, fields: &[Field]) -> Value {
    let doc = Doc::read(text, fields);
    json!({
        "title": doc.title().map(|t| display_title(&t)).unwrap_or_default(),
        "front": FRONT_KEYS.iter().map(|k| json!({"key": k, "value": doc.front_value(k).unwrap_or_default()})).collect::<Vec<_>>(),
        "sections": fields.iter().map(|f| json!({"heading": f.heading, "body": doc.section_words(&f.heading)})).collect::<Vec<_>>(),
    })
}

fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// A record's status changed. In the front matter when it has one; else in
/// the `## Status` section the older formats write; else a front matter is
/// made for it. `date` goes with it, since MADR's date is when the status
/// last changed
pub fn set_status(text: &str, status: &str, date: &str) -> String {
    let mut doc = Doc::read(text, &[]);
    let in_section = doc.front_value("status").is_none()
        && doc.sections.iter().any(|s| s.heading.eq_ignore_ascii_case("status"));
    if in_section {
        let s = doc.sections.iter_mut().find(|s| s.heading.eq_ignore_ascii_case("status")).expect("found above");
        let word = capitalised(status);
        match s.body.iter().position(|l| !l.trim().is_empty()) {
            Some(i) => s.body[i] = word,
            None => s.body = vec![String::new(), word],
        }
    } else {
        doc.set_front("status", status);
        if !date.is_empty() {
            doc.set_front("date", date);
        }
    }
    doc.write(newline_of(text))
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// How a record links to another one in the same folder: by its number when
/// it has one, the way MADR writes "superseded by ADR-0005"
pub fn link_to(file: &str) -> String {
    let name = match number_of(file) {
        Some(n) => format!("ADR-{n:04}"),
        None => file.trim_end_matches(".md").to_string(),
    };
    format!("[{name}]({file})")
}

/// The line a new record carries about the one it replaces, under "More
/// Information"
pub fn supersedes_line(old: &str) -> String {
    format!("Supersedes {}.", link_to(old))
}

/// A file name for a new record: the next number, written the way the
/// folder already writes them (`0012-`, `adr-012-`), then the title.
///
/// The title keeps its letters in whatever script it is written in, so a
/// Japanese title makes a Japanese name. Everything that is not a letter or a
/// digit becomes one `-` -- which takes out every character Windows refuses
/// in a name -- and ASCII is put in lower case, the way MADR names files
pub fn file_name(title: &str, taken: &[String]) -> String {
    let (prefix, width) = numbering(taken);
    let next = taken.iter().filter_map(|f| number_of(f)).max().unwrap_or(0) + 1;
    let mut n = next;
    loop {
        let name = format!("{prefix}{n:0width$}-{}.md", slug(title));
        if !taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
            return name;
        }
        n += 1;
    }
}

/// How the folder writes its numbers: what comes before them and how many
/// digits, from the record with the highest number. MADR's `0001-` when the
/// folder has none yet
fn numbering(taken: &[String]) -> (String, usize) {
    let last = taken.iter().filter(|f| number_of(f).is_some()).max_by_key(|f| number_of(f));
    match last {
        Some(f) => {
            let prefix: String = f.chars().take_while(|c| c.is_ascii_alphabetic() || *c == '-' || *c == '_').collect();
            let width = f[prefix.len()..].chars().take_while(char::is_ascii_digit).count();
            (prefix, width.max(1))
        }
        None => (String::new(), 4),
    }
}

/// The words of a file name taken from a title
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in title.chars() {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            if c.is_ascii() {
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        } else {
            gap = true;
        }
    }
    let mut cut: String = out.chars().take(SLUG_ROOM).collect();
    while cut.ends_with('-') {
        cut.pop();
    }
    if cut.is_empty() { "decision".to_string() } else { cut }
}

/// Today, as MADR writes a date
pub fn today() -> String {
    crate::hooks::local_stamp("%Y-%m-%d")
}

/// The folder a record lives in, on whichever machine
pub use crate::disk::Disk;

/// A folder a page named, as a path inside the working folder: forward
/// slashes, no `..`, no leading slash. `None` for one that would leave it
pub fn clean_dir(dir: &str) -> Option<String> {
    let parts: Vec<&str> = dir.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.iter().any(|p| *p == ".." || p.contains(':')) {
        return None;
    }
    Some(parts.join("/"))
}

use crate::disk::join;

/// A file name the page gave, as one name in the folder and nothing else
fn clean_file(name: &str) -> Option<String> {
    let t = name.trim();
    (!t.is_empty() && !t.contains(['/', '\\', ':']) && t != "." && t != "..").then(|| t.to_string())
}

/// The template the folder keeps, and what it was read from; MADR when it
/// keeps none
fn fields_of(disk: &dyn Disk, dir: &str) -> (Vec<Field>, Option<String>) {
    for name in TEMPLATES {
        if let Ok(Some(bytes)) = disk.read(&join(dir, name))
            && let Some(fields) = template_fields(&crate::charset::read(&bytes).text)
        {
            return (fields, Some((*name).to_string()));
        }
    }
    (madr(), None)
}

/// The records in a folder, read
pub fn records(disk: &dyn Disk, dir: &str, fields: &[Field]) -> Result<Option<Vec<Record>>, String> {
    let Some(mut names) = disk.files(dir)? else { return Ok(None) };
    names.retain(|n| {
        let lower = n.to_lowercase();
        lower.ends_with(".md") && !NOT_RECORDS.contains(&lower.as_str())
    });
    names.sort_by(|a, b| number_of(a).cmp(&number_of(b)).then_with(|| a.cmp(b)));
    names.truncate(MOST);
    let mut out = Vec::new();
    for name in names {
        let Some(bytes) = disk.read(&join(dir, &name))? else { continue };
        if bytes.len() > RECORD_LIMIT || bytes.contains(&0) {
            continue;
        }
        out.push(parse(&name, &crate::charset::read(&bytes).text, fields));
    }
    // "Superseded by ADR-0005" names a record by number: said as its file,
    // so the list can link the two
    let by_number: Vec<(u64, String)> = out.iter().filter_map(|r| r.number.map(|n| (n, r.file.clone()))).collect();
    for r in &mut out {
        if let Some(by) = r.by.clone().filter(|b| !b.ends_with(".md"))
            && let Some(n) = number_of(&by.to_lowercase().replace("adr", ""))
            && let Some((_, file)) = by_number.iter().find(|(k, _)| *k == n)
        {
            r.by = Some(file.clone());
        }
    }
    Ok(Some(out))
}

/// One of the folder questions the panel asks, answered. Every one names the
/// folder (`dir`, inside the working folder); what each needs beside it is
/// with it. The answer says the act it answers
pub fn answer(disk: &dyn Disk, act: &str, args: &Value) -> Value {
    let fail = |e: String| json!({"act": act, "ok": false, "error": e});
    let str_of = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let Some(dir) = clean_dir(&str_of("dir")) else {
        return fail(crate::i18n::t("err.sftp.outside"));
    };
    let (fields, template) = fields_of(disk, &dir);
    let today = today();
    match act {
        "list" => match records(disk, &dir, &fields) {
            Ok(found) => json!({
                "act": act, "ok": true, "dir": dir,
                "exists": found.is_some(),
                "records": found.unwrap_or_default(),
                "fields": fields,
                "template": template,
                "today": today,
            }),
            Err(e) => fail(e),
        },
        "read" => {
            let Some(file) = clean_file(&str_of("file")) else { return fail(crate::i18n::t("err.sftp.outside")) };
            match disk.read(&join(&dir, &file)) {
                Ok(Some(bytes)) => {
                    let text = crate::charset::read(&bytes).text;
                    json!({
                        "act": act, "ok": true, "dir": dir, "file": file,
                        "text": text,
                        "mark": crate::files::mark_of(&bytes),
                        "record": parse(&file, &text, &fields),
                        "parts": parts(&text, &fields),
                        "fields": fields,
                    })
                }
                Ok(None) => fail(crate::i18n::tp("err.adr.gone", &[("file", &file)])),
                Err(e) => fail(e),
            }
        }
        // A new record, or the form's changes to one. A new one gets the next
        // number and is never written over a file already there; a changed
        // one is refused if the file moved on since it was read, the way the
        // editor refuses
        "save" => {
            let edit = Edit::from_json(args);
            if edit.title.is_empty() {
                return fail(crate::i18n::t("err.adr.no_title"));
            }
            let existing = clean_file(&str_of("file"));
            let supersedes = clean_file(&str_of("supersedes"));
            if let Some(file) = existing {
                let path = join(&dir, &file);
                let bytes = match disk.read(&path) {
                    Ok(Some(b)) => b,
                    Ok(None) => return fail(crate::i18n::tp("err.adr.gone", &[("file", &file)])),
                    Err(e) => return fail(e),
                };
                if crate::files::mark_of(&bytes) != str_of("mark") {
                    return fail(crate::i18n::t("err.files.moved_on"));
                }
                let text = rewrite(&crate::charset::read(&bytes).text, &edit, &fields);
                return match disk.write(&path, text.as_bytes(), false) {
                    Ok(()) => json!({"act": act, "ok": true, "dir": dir, "file": file, "made": false}),
                    Err(e) => fail(e),
                };
            }
            let mut edit = edit;
            // The record it replaces is said under "More Information", where
            // MADR keeps links to other records
            if let Some(old) = &supersedes {
                let more = madr().into_iter().find(|f| f.heading == "More Information").expect("MADR has it");
                let heading = fields
                    .iter()
                    .find(|f| f.heading.eq_ignore_ascii_case(&more.heading))
                    .map(|f| f.heading.clone())
                    .unwrap_or(more.heading);
                let line = supersedes_line(old);
                match edit.sections.iter_mut().find(|(h, _)| h.eq_ignore_ascii_case(&heading)) {
                    Some((_, words)) if !words.contains(&line) => *words = format!("{line}\n\n{words}"),
                    Some(_) => {}
                    None => edit.sections.push((heading, line)),
                }
            }
            let mut order = fields.clone();
            if !order.iter().any(|f| f.heading.eq_ignore_ascii_case("More Information"))
                && supersedes.is_some()
            {
                order.push(Field { heading: "More Information".into(), level: 2, optional: true, hint: String::new() });
            }
            let text = compose(&edit, &order);
            if let Err(e) = disk.make_dirs(&dir) {
                return fail(e);
            }
            let taken = match disk.files(&dir) {
                Ok(names) => names.unwrap_or_default(),
                Err(e) => return fail(e),
            };
            let file = file_name(&edit.title, &taken);
            if let Err(e) = disk.write(&join(&dir, &file), text.as_bytes(), true) {
                return fail(e);
            }
            // The one it replaces says so, in the same breath: two records
            // that disagree about which one stands are worse than either alone
            if let Some(old) = supersedes {
                let path = join(&dir, &old);
                let said = match disk.read(&path) {
                    Ok(Some(b)) => {
                        let text = set_status(&crate::charset::read(&b).text, &format!("superseded by {}", link_to(&file)), &today);
                        disk.write(&path, text.as_bytes(), false)
                    }
                    Ok(None) => Err(crate::i18n::tp("err.adr.gone", &[("file", &old)])),
                    Err(e) => Err(e),
                };
                if let Err(e) = said {
                    return json!({"act": act, "ok": true, "dir": dir, "file": file, "made": true,
                        "warn": crate::i18n::tp("err.adr.old_unmarked", &[("file", &old), ("why", &e)])});
                }
            }
            json!({"act": act, "ok": true, "dir": dir, "file": file, "made": true})
        }
        // Accepted, rejected, deprecated: the status and the day it changed
        "status" => {
            let Some(file) = clean_file(&str_of("file")) else { return fail(crate::i18n::t("err.sftp.outside")) };
            let status = str_of("status").trim().to_lowercase();
            if !["proposed", "accepted", "rejected", "deprecated"].contains(&status.as_str()) {
                return fail(format!("unknown status: {status}"));
            }
            let path = join(&dir, &file);
            let bytes = match disk.read(&path) {
                Ok(Some(b)) => b,
                Ok(None) => return fail(crate::i18n::tp("err.adr.gone", &[("file", &file)])),
                Err(e) => return fail(e),
            };
            let mark = str_of("mark");
            if !mark.is_empty() && crate::files::mark_of(&bytes) != mark {
                return fail(crate::i18n::t("err.files.moved_on"));
            }
            let text = set_status(&crate::charset::read(&bytes).text, &status, &today);
            match disk.write(&path, text.as_bytes(), false) {
                Ok(()) => json!({"act": act, "ok": true, "dir": dir, "file": file, "status": status}),
                Err(e) => fail(e),
            }
        }
        _ => fail(format!("unknown act: {act}")),
    }
}

/// What an AI is told of a folder's records when it asks: each one's file,
/// title, status and date, and what replaced it -- not the words, which it
/// can read from the file itself
pub fn summary(records: &[Record]) -> Vec<Value> {
    records
        .iter()
        .map(|r| {
            json!({
                "file": r.file, "title": r.title, "status": r.status,
                "date": r.date, "superseded_by": r.by,
            })
        })
        .collect()
}

/// The records as the AI that answers a question about them reads them:
/// standing decisions first, each with its words, until `room` characters
pub fn for_question(records: &[Record], room: usize) -> String {
    let rank = |s: &str| match s {
        "accepted" => 0,
        "proposed" => 1,
        "" => 2,
        "deprecated" => 3,
        "superseded" => 4,
        _ => 5,
    };
    let mut ordered: Vec<&Record> = records.iter().collect();
    ordered.sort_by_key(|r| (rank(&r.status), std::cmp::Reverse(r.number)));
    let mut out = String::new();
    for r in ordered {
        let name = r.number.map(|n| format!("ADR-{n:04}")).unwrap_or_else(|| r.file.clone());
        let status = if r.status.is_empty() { "unknown".to_string() } else { r.status.clone() };
        let by = r.by.as_deref().map(|b| format!(" (superseded by {b})")).unwrap_or_default();
        let piece = format!(
            "=== {name}: {} [{status}{by}] file {} ===\n{}\n\n",
            r.title,
            r.file,
            cut_chars(&r.text, 3000)
        );
        if out.chars().count() + piece.chars().count() > room {
            break;
        }
        out.push_str(&piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// A folder in memory
    #[derive(Default)]
    struct Mem(RefCell<BTreeMap<String, Vec<u8>>>);

    impl Disk for Mem {
        fn files(&self, dir: &str) -> Result<Option<Vec<String>>, String> {
            let map = self.0.borrow();
            let pre = format!("{dir}/");
            let names: Vec<String> = map
                .keys()
                .filter_map(|k| k.strip_prefix(&pre).filter(|r| !r.contains('/')).map(str::to_string))
                .collect();
            Ok((!names.is_empty() || map.contains_key(&format!("{dir}/."))).then_some(names))
        }
        fn folders(&self, dir: &str) -> Result<Vec<String>, String> {
            let map = self.0.borrow();
            let pre = format!("{dir}/");
            let mut out: Vec<String> = map
                .keys()
                .filter_map(|k| k.strip_prefix(&pre).and_then(|r| r.split_once('/')).map(|(d, _)| d.to_string()))
                .collect();
            out.sort();
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
            map.insert(path.to_string(), bytes.to_vec());
            Ok(())
        }
        fn make_dirs(&self, dir: &str) -> Result<(), String> {
            self.0.borrow_mut().entry(format!("{dir}/.")).or_default();
            Ok(())
        }
    }

    impl Mem {
        fn put(&self, path: &str, text: &str) {
            self.0.borrow_mut().insert(path.into(), text.as_bytes().to_vec());
        }
        fn text(&self, path: &str) -> String {
            String::from_utf8(self.0.borrow().get(path).cloned().unwrap_or_default()).unwrap()
        }
    }

    const MADR_RECORD: &str = "---\nstatus: accepted\ndate: 2026-01-02\ndecision-makers: Aiko, Ben\n---\n\n# Use PostgreSQL\n\n## Context and Problem Statement\n\nWe need a database.\n\n## Considered Options\n\n* PostgreSQL\n* SQLite\n\n## Decision Outcome\n\nChosen option: \"PostgreSQL\".\n\n### Consequences\n\n* Good, because it scales.\n\n## Pros and Cons of the Options\n\n### PostgreSQL\n\n* Good, because mature.\n";

    #[test]
    fn a_madr_record_is_read_with_its_front_matter_and_sections() {
        let r = parse("0003-use-postgresql.md", MADR_RECORD, &madr());
        assert_eq!(r.number, Some(3));
        assert_eq!(r.title, "Use PostgreSQL");
        assert_eq!(r.status, "accepted");
        assert_eq!(r.date, "2026-01-02");
        assert_eq!(r.makers, "Aiko, Ben");
        assert_eq!(r.consulted, "", "nobody consulted is nothing written");
        assert!(r.fits);
        let p = parts(MADR_RECORD, &madr());
        assert_eq!(p["sections"][0]["body"], "We need a database.");
        // An option's own heading stays inside the section it is in
        assert!(p["sections"][6]["body"].as_str().unwrap().contains("### PostgreSQL"));
        assert_eq!(p["sections"][4]["body"], "* Good, because it scales.");
    }

    #[test]
    fn the_older_formats_are_read_for_their_status_and_number() {
        let nygard = "# 1. Record architecture decisions\n\nDate: 2020-01-01\n\n## Status\n\nSuperseded by [3. Use X](0003-use-x.md)\n\n## Context\n\nWords.\n";
        let r = parse("0001-record-architecture-decisions.md", nygard, &madr());
        assert_eq!(r.title, "Record architecture decisions");
        assert_eq!(r.status, "superseded");
        assert_eq!(r.by.as_deref(), Some("0003-use-x.md"));
        assert!(!r.fits, "a record in another format is not opened in the form");
        assert_eq!(parse("adr-012-x.md", "# ADR-012: Pick a queue\n", &madr()).title, "Pick a queue");
        assert_eq!(number_of("adr-012-x.md"), Some(12));
        assert_eq!(parse("x.md", "---\nstatus: \"{proposed | accepted}\"\n---\n# T\n", &madr()).status, "");
    }

    #[test]
    fn a_japanese_title_makes_a_japanese_file_name() {
        assert_eq!(slug("データベースに PostgreSQL を使う"), "データベースに-postgresql-を使う");
        assert_eq!(slug("Use <A>: \"B\" / C?"), "use-a-b-c");
        assert_eq!(slug("！？"), "decision");
        let long = "あ".repeat(100);
        assert_eq!(slug(&long).chars().count(), SLUG_ROOM);
        assert_eq!(file_name("決定", &[]), "0001-決定.md");
    }

    #[test]
    fn a_new_name_follows_the_numbering_the_folder_already_has() {
        let taken = vec!["adr-001-a.md".to_string(), "adr-007-b.md".to_string(), "README.md".to_string()];
        assert_eq!(file_name("Next one", &taken), "adr-008-next-one.md");
        let madr_style = vec!["0001-a.md".to_string(), "0002-b.md".to_string()];
        assert_eq!(file_name("C", &madr_style), "0003-c.md");
    }

    #[test]
    fn a_template_in_the_folder_decides_the_sections() {
        let template = "---\nstatus: \"{proposed | rejected}\"\n---\n\n# {short title}\n\n## Context and Problem Statement\n\n{Describe the context}\n\n## Decision Drivers\n\n<!-- This is an optional element. Feel free to remove. -->\n\n* {driver 1}\n\n## Pros and Cons of the Options\n\n### {title of option 1}\n\n* Good, because {argument a}\n\n## Our Own Section\n\nWho signs off.\n";
        let fields = template_fields(template).unwrap();
        let names: Vec<&str> = fields.iter().map(|f| f.heading.as_str()).collect();
        assert_eq!(names, ["Context and Problem Statement", "Decision Drivers", "Pros and Cons of the Options", "Our Own Section"]);
        assert!(!fields[0].optional && fields[1].optional);
        assert_eq!(fields[0].hint, "Describe the context");
        assert_eq!(fields[3].hint, "Who signs off.");
        assert!(template_fields("# Only a title\n").is_none());
    }

    #[test]
    fn a_new_record_is_written_in_madr_and_numbered() {
        let disk = Mem::default();
        let args = json!({
            "dir": "docs/decisions",
            "title": "キャッシュに Redis を使う",
            "front": [{"key": "status", "value": "proposed"}, {"key": "date", "value": "2026-10-06"},
                      {"key": "consulted", "value": "Claude Code (AI)"}, {"key": "informed", "value": ""}],
            "sections": [{"heading": "Context and Problem Statement", "body": "遅い。\n"},
                         {"heading": "Considered Options", "body": "* Redis\n* Memcached"},
                         {"heading": "Decision Drivers", "body": ""}],
        });
        let a = answer(&disk, "save", &args);
        assert_eq!(a["ok"], true, "{a}");
        assert_eq!(a["file"], "0001-キャッシュに-redis-を使う.md");
        let text = disk.text("docs/decisions/0001-キャッシュに-redis-を使う.md");
        assert_eq!(
            text,
            "---\nstatus: proposed\ndate: 2026-10-06\nconsulted: Claude Code (AI)\n---\n\n# キャッシュに Redis を使う\n\n## Context and Problem Statement\n\n遅い。\n\n## Considered Options\n\n* Redis\n* Memcached\n"
        );
        let list = answer(&disk, "list", &json!({"dir": "docs/decisions"}));
        assert_eq!(list["records"][0]["status"], "proposed");
        assert_eq!(list["records"][0]["title"], "キャッシュに Redis を使う");
        assert_eq!(list["records"][0]["consulted"], "Claude Code (AI)", "the AI asked is read back");
    }

    #[test]
    fn a_change_from_the_form_keeps_everything_it_does_not_show() {
        let disk = Mem::default();
        let path = "docs/decisions/0003-use-postgresql.md";
        let crlf = MADR_RECORD.replace('\n', "\r\n").replace("date: 2026-01-02", "date: 2026-01-02\r\ncustom: kept");
        disk.put(path, &crlf);
        let read = answer(&disk, "read", &json!({"dir": "docs/decisions", "file": "0003-use-postgresql.md"}));
        let args = json!({
            "dir": "docs/decisions", "file": "0003-use-postgresql.md", "mark": read["mark"],
            "title": "Use PostgreSQL 16",
            "front": [{"key": "status", "value": "accepted"}],
            "sections": [{"heading": "Context and Problem Statement", "body": "We need one database."},
                         {"heading": "Decision Drivers", "body": "* Cost"},
                         {"heading": "Consequences", "body": ""}],
        });
        assert_eq!(answer(&disk, "save", &args)["ok"], true);
        let text = disk.text(path);
        assert!(text.contains("custom: kept"), "{text}");
        assert!(text.contains("\r\n"), "the file's own line ends are kept");
        assert!(text.contains("# Use PostgreSQL 16"));
        assert!(text.contains("## Decision Drivers\r\n\r\n* Cost\r\n\r\n## Considered Options"), "{text}");
        assert!(!text.contains("### Consequences"), "an emptied section is taken out");
        assert!(text.contains("### PostgreSQL"), "{text}");
        // The same save again was read before this one: refused
        assert_eq!(answer(&disk, "save", &args)["ok"], false);
    }

    #[test]
    fn replacing_a_record_marks_both_of_them() {
        let disk = Mem::default();
        disk.put("docs/decisions/0003-use-postgresql.md", MADR_RECORD);
        let args = json!({
            "dir": "docs/decisions", "supersedes": "0003-use-postgresql.md",
            "title": "Use SQLite",
            "front": [{"key": "status", "value": "proposed"}],
            "sections": [{"heading": "Context and Problem Statement", "body": "Smaller."}],
        });
        let a = answer(&disk, "save", &args);
        assert_eq!(a["file"], "0004-use-sqlite.md", "{a}");
        let new = disk.text("docs/decisions/0004-use-sqlite.md");
        assert!(new.contains("## More Information\n\nSupersedes [ADR-0003](0003-use-postgresql.md)."), "{new}");
        let old = disk.text("docs/decisions/0003-use-postgresql.md");
        assert!(old.contains("status: superseded by [ADR-0004](0004-use-sqlite.md)"), "{old}");
        let list = answer(&disk, "list", &json!({"dir": "docs/decisions"}));
        assert_eq!(list["records"][0]["by"], "0004-use-sqlite.md");
    }

    #[test]
    fn a_status_is_changed_where_the_record_keeps_it() {
        let front = set_status("---\nstatus: proposed\n---\n\n# T\n", "accepted", "2026-10-06");
        assert_eq!(front, "---\nstatus: accepted\ndate: 2026-10-06\n---\n\n# T\n");
        let nygard = set_status("# T\n\n## Status\n\nProposed\n\n## Context\n\nx\n", "accepted", "2026-10-06");
        assert!(nygard.contains("## Status\n\nAccepted\n"), "{nygard}");
        let bare = set_status("# T\n", "rejected", "2026-10-06");
        assert!(bare.starts_with("---\nstatus: rejected\ndate: 2026-10-06\n---\n\n# T"), "{bare}");
    }

    #[test]
    fn a_folder_that_leaves_the_working_folder_is_refused() {
        let disk = Mem::default();
        assert_eq!(answer(&disk, "list", &json!({"dir": "../outside"}))["ok"], false);
        assert_eq!(answer(&disk, "read", &json!({"dir": "docs", "file": "../x.md"}))["ok"], false);
        assert_eq!(clean_dir("docs\\decisions\\").as_deref(), Some("docs/decisions"));
        assert!(clean_dir("C:/x").is_none());
        let none = answer(&disk, "list", &json!({"dir": "docs/decisions"}));
        assert_eq!(none["exists"], false);
    }

    #[test]
    fn a_question_reads_the_standing_decisions_first() {
        let rec = |n: u64, status: &str| Record {
            file: format!("{n:04}-x.md"), number: Some(n), title: format!("T{n}"),
            status: status.into(), text: "words".into(), ..Default::default()
        };
        let all = vec![rec(1, "superseded"), rec(2, "accepted"), rec(3, "proposed")];
        let text = for_question(&all, 10_000);
        let a = text.find("ADR-0002").unwrap();
        let p = text.find("ADR-0003").unwrap();
        let s = text.find("ADR-0001").unwrap();
        assert!(a < p && p < s, "{text}");
        assert!(for_question(&all, 10).is_empty());
    }
}
