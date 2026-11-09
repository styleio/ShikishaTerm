//! A repository's issue templates: the ones GitHub offers on its own "New
//! issue" page, read so an issue made here has the same shape as one made
//! there.
//!
//! GitHub has two kinds, both kept in `.github/ISSUE_TEMPLATE/`:
//!
//! - **forms** (`.yml` / `.yaml`): a list of fields -- one line, several
//!   lines, a choice, checkboxes -- each answered on its own, and turned into
//!   the issue's description by GitHub when the issue is made. Here the same
//!   turning is done by [`render`], so the description is the one GitHub's
//!   page would have written
//! - **Markdown** (`.md`): a description that starts out written, headings
//!   and all, with its title, labels and people in a front matter
//!
//! Both may put a title's start (`[Bug] `), labels and assignees on the issue.
//! `config.yml` beside them says whether an issue may also be made from
//! nothing (`blank_issues_enabled`).
//!
//! Nothing here talks to GitHub: the files are handed in, read, and what comes
//! back is plain data the issue form draws, the AI is asked from and the issue
//! is made from.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The folder GitHub reads issue templates from
pub const FOLDER: &str = ".github/ISSUE_TEMPLATE";

/// What GitHub writes for a field nobody answered
const NO_RESPONSE: &str = "_No response_";

/// The templates a repository offers, in the order GitHub lists them
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Templates {
    /// Whether an issue may also be made without one (`blank_issues_enabled`)
    pub blank: bool,
    pub items: Vec<Template>,
    /// Template files that could not be read, each with why, so a broken one
    /// is said rather than silently missing from the list
    #[serde(default)]
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Problem {
    pub file: String,
    pub error: String,
}

/// One template
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Template {
    /// Its file name, which is also how it is told apart from the others
    pub file: String,
    pub name: String,
    /// The one line under its name on GitHub's list (`description`, or `about`
    /// in a Markdown template)
    #[serde(default)]
    pub about: String,
    /// The start of every title made from it (`[Bug] `)
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    /// A form's fields, in order; `None` for a Markdown template
    #[serde(default)]
    pub fields: Option<Vec<Field>>,
    /// A Markdown template's description as it starts out
    #[serde(default)]
    pub body: String,
    /// What a form's fields hold before anybody writes anything
    /// ([`start_values`]), handed to the screen with the template so it does
    /// not work that out a second time
    #[serde(default)]
    pub start: Vec<Value>,
}

/// One element of a form
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Field {
    /// `markdown`, `input`, `textarea`, `dropdown`, `checkboxes` or `upload`
    pub kind: String,
    /// What it is called in an answer: its `id`, or its place in the form when
    /// it has none
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub placeholder: String,
    /// What a text field starts out holding, or a `markdown` element's text
    #[serde(default)]
    pub value: String,
    /// A language: what is written is put in a code block of it
    #[serde(default)]
    pub render: String,
    #[serde(default)]
    pub multiple: bool,
    /// A choice's options, or the checkboxes' labels
    #[serde(default)]
    pub options: Vec<String>,
    /// Which checkboxes must be ticked, beside `options`
    #[serde(default)]
    pub ticks_needed: Vec<bool>,
    /// The option a choice starts out on
    #[serde(default)]
    pub default: Option<usize>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub min_length: Option<usize>,
}

impl Field {
    /// Whether it is part of what the issue says. A `markdown` element is
    /// guidance for the person filling the form and is not sent
    pub fn answered(&self) -> bool {
        self.kind != "markdown"
    }

    fn text_like(&self) -> bool {
        matches!(self.kind.as_str(), "input" | "textarea" | "upload")
    }
}

/// Read every template file of a repository's template folder: `files` are
/// the names and texts found there. `config.yml` says whether a blank issue is
/// offered; any other file that is not a template is passed over, and one that
/// cannot be read is listed with why
pub fn read_folder(files: &[(String, String)]) -> Templates {
    let mut out = Templates { blank: true, ..Templates::default() };
    let mut files: Vec<&(String, String)> = files.iter().collect();
    // GitHub lists them by file name
    files.sort_by_key(|(name, _)| name.to_lowercase());
    for (name, text) in files {
        let lower = name.to_lowercase();
        if lower == "config.yml" || lower == "config.yaml" {
            if let Ok(v) = yaml_value(text) {
                if v.get("blank_issues_enabled").and_then(|b| b.as_bool()) == Some(false) {
                    out.blank = false;
                }
            }
            continue;
        }
        let read = if lower.ends_with(".yml") || lower.ends_with(".yaml") {
            read_form(name, text)
        } else if lower.ends_with(".md") {
            read_markdown(name, text)
        } else {
            continue;
        };
        match read {
            Ok(mut t) => {
                t.start = start_values(&t);
                out.items.push(t);
            }
            Err(e) => out.problems.push(Problem { file: name.clone(), error: format!("{e:#}") }),
        }
    }
    out
}

/// A form template (`.yml`)
pub fn read_form(file: &str, text: &str) -> Result<Template> {
    let v = yaml_value(text)?;
    let name = string_of(v.get("name"));
    if name.trim().is_empty() {
        return Err(anyhow!(crate::i18n::t("err.issue_template.no_name")));
    }
    let body = v
        .get("body")
        .and_then(|b| b.as_array())
        .ok_or_else(|| anyhow!(crate::i18n::t("err.issue_template.no_body")))?;
    let mut fields = Vec::new();
    for (i, e) in body.iter().enumerate() {
        let kind = string_of(e.get("type"));
        let a = e.get("attributes").cloned().unwrap_or(Value::Null);
        let checks = e.get("validations").cloned().unwrap_or(Value::Null);
        let id = string_of(e.get("id"));
        let mut f = Field {
            key: if id.trim().is_empty() { format!("field_{}", i + 1) } else { id },
            label: string_of(a.get("label")),
            description: string_of(a.get("description")),
            placeholder: string_of(a.get("placeholder")),
            value: string_of(a.get("value")),
            render: string_of(a.get("render")),
            multiple: a.get("multiple").and_then(|m| m.as_bool()).unwrap_or(false),
            required: checks.get("required").and_then(|r| r.as_bool()).unwrap_or(false),
            min_length: checks.get("min_length").and_then(|n| n.as_u64()).map(|n| n as usize),
            ..Field::default()
        };
        match kind.as_str() {
            "markdown" | "input" | "textarea" | "upload" => {}
            "dropdown" => {
                f.options = a
                    .get("options")
                    .and_then(|o| o.as_array())
                    .into_iter()
                    .flatten()
                    .map(|o| string_of(Some(o)))
                    .collect();
                f.default = a
                    .get("default")
                    .and_then(|d| d.as_u64())
                    .map(|d| d as usize)
                    .filter(|d| *d < f.options.len());
            }
            "checkboxes" => {
                for o in a.get("options").and_then(|o| o.as_array()).into_iter().flatten() {
                    f.options.push(string_of(o.get("label")));
                    f.ticks_needed.push(o.get("required").and_then(|r| r.as_bool()).unwrap_or(false));
                }
            }
            other => {
                return Err(anyhow!(crate::i18n::tp(
                    "err.issue_template.kind",
                    &[("kind", other), ("n", &(i + 1).to_string())]
                )));
            }
        }
        f.kind = kind;
        fields.push(f);
    }
    Ok(Template {
        file: file.to_string(),
        name,
        about: string_of(v.get("description")),
        title: string_of(v.get("title")),
        labels: names_of(v.get("labels")),
        assignees: names_of(v.get("assignees")),
        fields: Some(fields),
        body: String::new(),
        start: Vec::new(),
    })
}

/// A Markdown template (`.md`): its front matter, then the description it
/// starts out with
pub fn read_markdown(file: &str, text: &str) -> Result<Template> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let (head, body) = match text.strip_prefix("---\n") {
        Some(rest) => match rest.find("\n---") {
            Some(end) => {
                let after = &rest[end + 4..];
                (rest[..end].to_string(), after.strip_prefix('\n').unwrap_or(after).to_string())
            }
            None => (String::new(), text.clone()),
        },
        None => (String::new(), text.clone()),
    };
    let v = if head.trim().is_empty() { Value::Null } else { yaml_value(&head)? };
    let mut name = string_of(v.get("name"));
    if name.trim().is_empty() {
        name = file.rsplit_once('.').map(|(n, _)| n).unwrap_or(file).to_string();
    }
    Ok(Template {
        file: file.to_string(),
        name,
        about: string_of(v.get("about")),
        title: string_of(v.get("title")),
        labels: names_of(v.get("labels")),
        assignees: names_of(v.get("assignees")),
        fields: None,
        body,
        start: Vec::new(),
    })
}

/// What each field of a form holds before anybody writes anything: the
/// text it is given, the option it starts on, no ticks. In the form's order,
/// one per field (`null` for guidance)
pub fn start_values(t: &Template) -> Vec<Value> {
    t.fields
        .iter()
        .flatten()
        .map(|f| match f.kind.as_str() {
            "markdown" => Value::Null,
            "dropdown" => json!(f.default.map(|d| vec![d]).unwrap_or_default()),
            "checkboxes" => json!(vec![false; f.options.len()]),
            _ => json!(f.value),
        })
        .collect()
}

fn text_at(values: &[Value], i: usize) -> String {
    values.get(i).and_then(|v| v.as_str()).unwrap_or_default().trim().to_string()
}

/// The options chosen in a choice, in the order they are listed
fn chosen_at(f: &Field, values: &[Value], i: usize) -> Vec<usize> {
    let mut at: Vec<usize> = values
        .get(i)
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_u64().map(|n| n as usize))
        .filter(|n| *n < f.options.len())
        .collect();
    at.sort_unstable();
    at.dedup();
    if !f.multiple {
        at.truncate(1);
    }
    at
}

fn ticks_at(f: &Field, values: &[Value], i: usize) -> Vec<bool> {
    let got = values.get(i).and_then(|v| v.as_array());
    (0..f.options.len())
        .map(|n| got.and_then(|g| g.get(n)).and_then(|b| b.as_bool()).unwrap_or(false))
        .collect()
}

/// The description GitHub writes from a form's answers: each field under its
/// label as a third-level heading, an unanswered one as `_No response_`, a
/// field with a language in a code block of it, a choice as its options
/// separated by commas, checkboxes as a task list
pub fn render(fields: &[Field], values: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        if !f.answered() {
            continue;
        }
        let said = match f.kind.as_str() {
            "dropdown" => {
                let chosen: Vec<&str> = chosen_at(f, values, i).iter().map(|n| f.options[*n].as_str()).collect();
                if chosen.is_empty() { NO_RESPONSE.to_string() } else { chosen.join(", ") }
            }
            "checkboxes" => ticks_at(f, values, i)
                .iter()
                .zip(&f.options)
                .map(|(on, label)| format!("- [{}] {label}", if *on { "x" } else { " " }))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => {
                let text = text_at(values, i);
                if !f.render.trim().is_empty() {
                    // Written or not, GitHub puts it in the block
                    format!("```{}\n{text}\n```", f.render.trim())
                } else if text.is_empty() {
                    NO_RESPONSE.to_string()
                } else {
                    text
                }
            }
        };
        let label = f.label.trim();
        parts.push(if label.is_empty() { said } else { format!("### {label}\n\n{said}") });
    }
    parts.join("\n\n")
}

/// The start a template puts on a title, put on one that does not already
/// begin with it
pub fn with_prefix(prefix: &str, title: &str) -> String {
    let title = title.trim();
    let start = prefix.trim();
    if start.is_empty() || title.starts_with(start) {
        return title.to_string();
    }
    format!("{prefix}{title}")
}

/// What has to be written before an issue can be made from the template:
/// each as `{"at": <field's place, or -1 for the title>, "label": ...}`. The
/// same things GitHub's page will not make an issue without -- a required
/// field left empty, a box that must be ticked, a text shorter than its least
/// length -- and a title that is nothing but the template's start, which
/// GitHub's page lets through and nobody can tell apart from the next one
pub fn missing(t: &Template, title: &str, values: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    let title = title.trim();
    let start = t.title.trim();
    if title.is_empty() || (!start.is_empty() && title.strip_prefix(start).unwrap_or(title).trim().is_empty()) {
        out.push(json!({"at": -1, "label": crate::i18n::t("issues.new.title")}));
    }
    for (i, f) in t.fields.iter().flatten().enumerate() {
        let name = if f.label.trim().is_empty() { f.key.clone() } else { f.label.trim().to_string() };
        let short = match f.kind.as_str() {
            "markdown" => None,
            "dropdown" => (f.required && chosen_at(f, values, i).is_empty()).then(|| name.clone()),
            "checkboxes" => {
                let ticks = ticks_at(f, values, i);
                let unticked: Vec<&String> = ticks
                    .iter()
                    .zip(&f.options)
                    .zip(&f.ticks_needed)
                    .filter(|((on, _), need)| **need && !**on)
                    .map(|((_, label), _)| label)
                    .collect();
                if let Some(first) = unticked.first() {
                    Some(format!("{name}: {first}"))
                } else {
                    (f.required && !ticks.iter().any(|on| *on)).then(|| name.clone())
                }
            }
            _ => {
                let text = text_at(values, i);
                let len = text.chars().count();
                match f.min_length {
                    Some(n) if n > 0 && (len > 0 || f.required) && len < n => Some(crate::i18n::tp(
                        "issues.field.min_length",
                        &[("label", &name), ("n", &n.to_string())],
                    )),
                    _ => (f.required && text.is_empty()).then(|| name.clone()),
                }
            }
        };
        if let Some(label) = short {
            out.push(json!({"at": i, "label": label}));
        }
    }
    out
}

/// The shape of the AI's answer when the issue follows a template: the JSON
/// it must give, and the template it must follow
pub fn ai_shape(t: &Template, labels: &str, assignees: &str) -> String {
    let start = t.title.trim();
    let prefix = if start.is_empty() {
        String::new()
    } else {
        crate::i18n::tp("ai.issue.shape.prefix", &[("prefix", start)])
    };
    match &t.fields {
        Some(fields) => {
            let listed: Vec<Value> = fields
                .iter()
                .filter(|f| f.answered())
                .map(|f| {
                    let mut o = json!({"key": f.key, "kind": f.kind, "label": f.label});
                    if !f.description.trim().is_empty() {
                        o["description"] = json!(f.description.trim());
                    }
                    if !f.placeholder.trim().is_empty() {
                        o["example_of_the_form"] = json!(f.placeholder.trim());
                    }
                    if !f.options.is_empty() {
                        o["options"] = json!(f.options);
                    }
                    if f.kind == "dropdown" && f.multiple {
                        o["several_may_be_chosen"] = json!(true);
                    }
                    if f.required {
                        o["required"] = json!(true);
                    }
                    o
                })
                .collect();
            crate::i18n::tp(
                "ai.issue.shape.form",
                &[
                    ("prefix", &prefix),
                    ("labels", labels),
                    ("assignees", assignees),
                    ("form", &serde_json::to_string_pretty(&listed).unwrap_or_default()),
                ],
            )
        }
        None => crate::i18n::tp(
            "ai.issue.shape.markdown",
            &[("prefix", &prefix), ("labels", labels), ("assignees", assignees), ("template", &t.body)],
        ),
    }
}

/// The option an AI named, by its text: as written, then without the spaces
/// around it, then without regard to case. `None` for anything else -- an
/// option the template does not have is never taken
fn option_named(options: &[String], said: &str) -> Option<usize> {
    let said = said.trim();
    options
        .iter()
        .position(|o| o == said)
        .or_else(|| options.iter().position(|o| o.trim() == said))
        .or_else(|| options.iter().position(|o| o.trim().to_lowercase() == said.to_lowercase()))
}

/// The AI's answer for a template, read into what the issue form holds.
/// `title_now` and `values` are what the form held when the AI was asked.
/// The AI only fills in: a field somebody wrote in keeps what they wrote, and
/// only one still holding what the template started it with (or nothing) is
/// written; a ticked box stays ticked; only an option the template has can be
/// chosen; and a title somebody wrote stays. Answers with the title (with the
/// template's start), the description (a Markdown template's) or the fields
/// (a form's), the labels and person it named, and what is still missing
pub fn read_answer(t: &Template, answer: &str, title_now: &str, values: &[Value]) -> Result<Value> {
    let got: Value = serde_json::from_str(answer).map_err(|e| anyhow!("{e}"))?;
    let start = t.title.trim();
    let written = title_now.trim();
    let title = if written.strip_prefix(start).unwrap_or(written).trim().is_empty() {
        with_prefix(&t.title, got.get("title").and_then(|v| v.as_str()).unwrap_or_default())
    } else {
        written.to_string()
    };
    let mut out = json!({
        "file": t.file,
        "title": title,
        "labels": got.get("labels").cloned().filter(|l| l.is_array()).unwrap_or(json!([])),
        "assignee": got.get("assignee").cloned().unwrap_or(Value::Null),
    });
    let Some(fields) = &t.fields else {
        let body = got.get("body").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        out["body"] = json!(body);
        out["missing"] = json!(missing(t, &title, &[]));
        return Ok(out);
    };
    let first: Vec<Value> = start_values(t);
    let mut now = first.clone();
    for (i, v) in values.iter().enumerate().take(now.len()) {
        if !v.is_null() {
            now[i] = v.clone();
        }
    }
    let said = got.get("fields").cloned().unwrap_or(Value::Null);
    for (i, f) in fields.iter().enumerate() {
        let Some(answer) = said.get(&f.key).filter(|v| !v.is_null()) else { continue };
        // Somebody wrote here: theirs stays. Boxes are only ever ticked, so
        // they are open to it either way
        let open = now[i] == first[i]
            || match f.kind.as_str() {
                "dropdown" => chosen_at(f, &now, i).is_empty(),
                "checkboxes" => true,
                _ => text_at(&now, i).is_empty(),
            };
        if !open {
            continue;
        }
        match f.kind.as_str() {
            "markdown" => {}
            "dropdown" => {
                let names: Vec<String> = match answer {
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                    Value::String(s) => vec![s.clone()],
                    _ => Vec::new(),
                };
                let mut at: Vec<usize> = names.iter().filter_map(|n| option_named(&f.options, n)).collect();
                at.dedup();
                if !f.multiple {
                    at.truncate(1);
                }
                if !at.is_empty() {
                    now[i] = json!(at);
                }
            }
            "checkboxes" => {
                let mut ticks = ticks_at(f, &now, i);
                for n in answer.as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                    if let Some(at) = option_named(&f.options, n) {
                        ticks[at] = true;
                    }
                }
                now[i] = json!(ticks);
            }
            _ if f.text_like() => {
                let text = match answer {
                    Value::String(s) => s.trim().to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => String::new(),
                };
                if !text.is_empty() {
                    now[i] = json!(text);
                }
            }
            _ => {}
        }
    }
    out["missing"] = json!(missing(t, &title, &now));
    out["values"] = json!(now);
    Ok(out)
}

/// The description of an issue made from a template, or why it cannot be
/// made yet. A form's is written from its fields, the way GitHub writes it; a
/// Markdown template's is what was written
pub fn issue_body(t: &Template, title: &str, body: &str, values: &[Value]) -> Result<String> {
    let short = missing(t, title, values);
    if !short.is_empty() {
        let names: Vec<&str> = short.iter().filter_map(|m| m["label"].as_str()).collect();
        return Err(anyhow!(crate::i18n::tp("err.issue.missing", &[("items", &names.join(crate::i18n::t("list.sep").as_str()))])));
    }
    Ok(match &t.fields {
        Some(fields) => render(fields, values),
        None => body.to_string(),
    })
}

/// A YAML document as JSON, the shape the rest of this reads
fn yaml_value(text: &str) -> Result<Value> {
    let docs = yaml_rust2::YamlLoader::load_from_str(text).map_err(|e| anyhow!("{e}"))?;
    Ok(docs.into_iter().next().map(|d| json_of(&d)).unwrap_or(Value::Null))
}

fn json_of(y: &yaml_rust2::Yaml) -> Value {
    use yaml_rust2::Yaml;
    match y {
        Yaml::Real(s) => s.parse::<f64>().map(|f| json!(f)).unwrap_or_else(|_| json!(s)),
        Yaml::Integer(n) => json!(n),
        Yaml::String(s) => json!(s),
        Yaml::Boolean(b) => json!(b),
        Yaml::Array(a) => Value::Array(a.iter().map(json_of).collect()),
        Yaml::Hash(h) => {
            let mut o = serde_json::Map::new();
            for (k, v) in h {
                let key = match k {
                    Yaml::String(s) => s.clone(),
                    Yaml::Integer(n) => n.to_string(),
                    Yaml::Boolean(b) => b.to_string(),
                    Yaml::Real(s) => s.clone(),
                    _ => continue,
                };
                o.insert(key, json_of(v));
            }
            Value::Object(o)
        }
        _ => Value::Null,
    }
}

/// A YAML scalar as text: `version: 1.0` is the words `1.0`, not a number
fn string_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// Labels or people: a list, or one line with commas between them
fn names_of(v: Option<&Value>) -> Vec<String> {
    let all: Vec<String> = match v {
        Some(Value::Array(a)) => a.iter().map(|x| string_of(Some(x))).collect(),
        Some(Value::String(s)) => s.split(',').map(str::to_string).collect(),
        _ => Vec::new(),
    };
    all.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form with one of each kind, written the way repositories write them
    const FORM: &str = r#"
name: Bug report
description: Something does not work
title: "[Bug] "
labels: ["bug", "triage"]
assignees: octocat
body:
  - type: markdown
    attributes:
      value: "Thanks for taking the time."
  - type: input
    id: version
    attributes:
      label: Version
      placeholder: "1.2.3"
    validations:
      required: true
  - type: dropdown
    id: os
    attributes:
      label: Operating system
      multiple: true
      options:
        - Windows
        - macOS
        - Linux
  - type: dropdown
    id: severity
    attributes:
      label: How bad is it?
      options: [Low, High]
      default: 0
  - type: textarea
    id: what
    attributes:
      label: What happened?
    validations:
      required: true
  - type: textarea
    id: log
    attributes:
      label: Log
      render: shell
  - type: checkboxes
    id: terms
    attributes:
      label: Checklist
      options:
        - label: I searched the issues
          required: true
        - label: I can help fix it
  - type: input
    attributes:
      label: Extra
"#;

    fn form() -> Template {
        read_form("bug.yml", FORM).unwrap()
    }

    #[test]
    fn a_form_is_read_with_its_start_labels_people_and_fields() {
        let t = form();
        assert_eq!(t.name, "Bug report");
        assert_eq!(t.about, "Something does not work");
        assert_eq!(t.title, "[Bug] ");
        assert_eq!(t.labels, vec!["bug", "triage"]);
        assert_eq!(t.assignees, vec!["octocat"]);
        let f = t.fields.unwrap();
        assert_eq!(f.len(), 8);
        assert_eq!(f[1].key, "version");
        assert!(f[1].required);
        assert_eq!(f[2].options, vec!["Windows", "macOS", "Linux"]);
        assert!(f[2].multiple);
        assert_eq!(f[3].default, Some(0));
        assert_eq!(f[5].render, "shell");
        assert_eq!(f[6].ticks_needed, vec![true, false]);
        // A field without an id is called by its place
        assert_eq!(f[7].key, "field_8");
    }

    #[test]
    fn a_form_is_written_out_the_way_github_writes_it() {
        let t = form();
        let fields = t.fields.clone().unwrap();
        let mut v = start_values(&t);
        v[1] = json!("1.4.0");
        v[2] = json!([2, 0]);
        v[4] = json!("It crashes.\n\nEvery time.");
        v[6] = json!([true, false]);
        let body = render(&fields, &v);
        assert_eq!(
            body,
            "### Version\n\n1.4.0\n\n\
             ### Operating system\n\nWindows, Linux\n\n\
             ### How bad is it?\n\nLow\n\n\
             ### What happened?\n\nIt crashes.\n\nEvery time.\n\n\
             ### Log\n\n```shell\n\n```\n\n\
             ### Checklist\n\n- [x] I searched the issues\n- [ ] I can help fix it\n\n\
             ### Extra\n\n_No response_"
        );
    }

    #[test]
    fn a_log_is_written_inside_its_block() {
        let t = form();
        let mut v = start_values(&t);
        v[5] = json!("panic at main.rs:3\n");
        assert!(render(t.fields.as_ref().unwrap(), &v).contains("### Log\n\n```shell\npanic at main.rs:3\n```"));
    }

    #[test]
    fn what_is_required_and_empty_is_named_before_the_issue_is_made() {
        let t = form();
        let v = start_values(&t);
        let short: Vec<String> = missing(&t, "[Bug] ", &v).iter().map(|m| m["label"].as_str().unwrap().to_string()).collect();
        assert_eq!(short.len(), 4);
        assert!(short[1..].contains(&"Version".to_string()));
        assert!(short[1..].contains(&"What happened?".to_string()));
        assert!(short[1..].contains(&"Checklist: I searched the issues".to_string()));
        // A title that is only the template's start is no title
        assert_eq!(missing(&t, "[Bug] ", &v)[0]["at"], -1);
        let mut v = v;
        v[1] = json!("2");
        v[4] = json!("x");
        v[6] = json!([true, false]);
        assert!(missing(&t, "[Bug] It crashes", &v).is_empty());
        assert!(issue_body(&t, "[Bug] It crashes", "", &v).is_ok());
        assert!(issue_body(&t, "[Bug]", "", &v).is_err());
    }

    #[test]
    fn the_ai_can_only_choose_options_the_template_has_and_keeps_what_was_written() {
        let t = form();
        let mut before = start_values(&t);
        before[7] = json!("written by hand");
        let answer = json!({
            "title": "It crashes on start",
            "fields": {
                "version": "1.4.0",
                "os": ["linux", "BeOS"],
                "severity": "Catastrophic",
                "what": "It crashes.",
                "log": null,
                "terms": ["I searched the issues", "I own the company"],
                "field_8": null
            },
            "labels": ["bug"],
            "assignee": null
        })
        .to_string();
        let got = read_answer(&t, &answer, "", &before).unwrap();
        assert_eq!(got["title"], "[Bug] It crashes on start");
        let v = got["values"].as_array().unwrap();
        assert_eq!(v[1], "1.4.0");
        assert_eq!(v[2], json!([2]));
        // Not an option: the default it started on stays
        assert_eq!(v[3], json!([0]));
        assert_eq!(v[6], json!([true, false]));
        assert_eq!(v[7], "written by hand");
        assert!(got["missing"].as_array().unwrap().is_empty());
    }

    #[test]
    fn what_somebody_wrote_is_never_written_over_by_the_ai() {
        let t = form();
        let mut before = start_values(&t);
        before[1] = json!("9.9");
        before[2] = json!([1]);
        // Still the default it started on: the AI may choose
        let answer = json!({
            "title": "Something the AI thought of",
            "fields": {"version": "1.4.0", "os": ["Linux"], "severity": "High", "what": "It crashes."}
        })
        .to_string();
        let got = read_answer(&t, &answer, "[Bug] Crash on start", &before).unwrap();
        assert_eq!(got["title"], "[Bug] Crash on start");
        let v = got["values"].as_array().unwrap();
        assert_eq!(v[1], "9.9");
        assert_eq!(v[2], json!([1]));
        assert_eq!(v[3], json!([1]));
        assert_eq!(v[4], "It crashes.");
    }

    #[test]
    fn a_field_the_notes_did_not_cover_is_reported_missing() {
        let t = form();
        let answer = json!({"title": "Crash", "fields": {"what": "It crashes."}}).to_string();
        let got = read_answer(&t, &answer, "[Bug] ", &start_values(&t)).unwrap();
        let short: Vec<&str> = got["missing"].as_array().unwrap().iter().map(|m| m["label"].as_str().unwrap()).collect();
        assert!(short.contains(&"Version"));
        assert!(!short.contains(&"What happened?"));
    }

    #[test]
    fn a_markdown_template_keeps_its_front_matter_apart_from_its_body() {
        let t = read_markdown(
            "feature.md",
            "---\nname: Feature request\nabout: Suggest an idea\ntitle: '[Idea] '\nlabels: enhancement, ui\nassignees: ''\n---\n\n## Problem\n\n## Proposal\n",
        )
        .unwrap();
        assert_eq!(t.name, "Feature request");
        assert_eq!(t.about, "Suggest an idea");
        assert_eq!(t.title, "[Idea] ");
        assert_eq!(t.labels, vec!["enhancement", "ui"]);
        assert!(t.assignees.is_empty());
        assert_eq!(t.body, "\n## Problem\n\n## Proposal\n");
        assert!(t.fields.is_none());
        let got = read_answer(&t, &json!({"title": "Dark mode", "body": "## Problem\n\nToo bright"}).to_string(), "", &[]).unwrap();
        assert_eq!(got["title"], "[Idea] Dark mode");
        assert_eq!(got["body"], "## Problem\n\nToo bright");
    }

    #[test]
    fn the_folder_is_listed_by_file_name_and_its_config_is_obeyed() {
        let files = vec![
            ("z_request.yml".to_string(), "name: Request\ndescription: d\nbody:\n  - type: textarea\n    attributes:\n      label: What\n".to_string()),
            ("config.yml".to_string(), "blank_issues_enabled: false\n".to_string()),
            ("a_bug.md".to_string(), "---\nname: Bug\n---\nbody".to_string()),
            ("broken.yml".to_string(), "name: [unclosed".to_string()),
            ("README.txt".to_string(), "not a template".to_string()),
        ];
        let got = read_folder(&files);
        assert!(!got.blank);
        let names: Vec<&str> = got.items.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Bug", "Request"]);
        assert_eq!(got.problems.len(), 1);
        assert_eq!(got.problems[0].file, "broken.yml");
    }

    #[test]
    fn a_title_is_given_the_templates_start_once() {
        assert_eq!(with_prefix("[Bug] ", "Crash"), "[Bug] Crash");
        assert_eq!(with_prefix("[Bug] ", "[Bug] Crash"), "[Bug] Crash");
        assert_eq!(with_prefix("", "Crash"), "Crash");
    }
}
