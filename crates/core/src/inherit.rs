//! Asking the assistant AI how each thing git ignores should reach a new
//! worktree.
//!
//! The program measures first -- what each line of the ignore files matches,
//! how large it is, which of those files name the checkout itself -- and the
//! AI judges from that. Its answer rests on the project as it is on disk, not
//! on a guess about what projects of its kind usually hold. What it says is
//! checked against what was offered before anybody sees it; the person reads
//! it on the settings screen and saves it, or does not. Nothing here writes a
//! setting.

use std::path::Path;

use anyhow::{bail, Result};

/// What the AI proposes for one line of an ignore file
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Proposal {
    /// The ignore file, as git names it
    pub source: String,
    /// The line as written
    pub pattern: String,
    /// `copy`, `replace`, `link` or `skip`
    pub how: String,
    /// For `replace`: what is written differently in the copy
    #[serde(default)]
    pub replace: Vec<crate::config::Replace>,
    /// Why, in one sentence, for the person deciding whether to save it
    #[serde(default)]
    pub reason: String,
}

/// What the question is about
pub struct Facts<'a> {
    /// The project's own checkout
    pub main: &'a Path,
    /// Where a new worktree of it would be made, following its placement
    pub example: &'a Path,
    /// What says the project is served where it stands, when something does
    pub served: Option<&'a str>,
    /// Whether a folder where worktrees go can be a second name for another,
    /// when that could be tried
    pub linkable: Option<bool>,
    /// What the person wrote about the project
    pub hint: &'a str,
}

/// How much a line's things hold before a link is worth suggesting in its
/// place: this much copied into every worktree is minutes and gigabytes each
pub const LARGE_BYTES: u64 = 1 << 30;
/// ...or this many files, which is slow to copy however little they weigh
pub const LARGE_FILES: u64 = 50_000;

/// How many files are counted in all when sizing what the lines match
pub const SIZE_MOST: u64 = 300_000;

/// How long the AI may take before the question is abandoned
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(240);

/// Whether a size is one to say something about
pub fn large(bytes: u64, files: u64) -> bool {
    bytes >= LARGE_BYTES || files >= LARGE_FILES
}

/// One line of an ignore file and what it matches
struct Line<'a> {
    source: String,
    pattern: String,
    matched: Vec<&'a crate::worktree::Ignored>,
}

impl Line<'_> {
    /// Whether any of what it matches is a file, which a replacement can be made in
    fn has_files(&self) -> bool {
        self.matched.iter().any(|m| !m.folder)
    }
}

/// The lines, each once, in the order their first match is listed
fn lines_of(found: &[crate::worktree::Ignored]) -> Vec<Line<'_>> {
    let mut out: Vec<Line> = Vec::new();
    for i in found {
        match out.iter_mut().find(|l| l.source == i.source && l.pattern == i.pattern) {
            Some(l) => l.matched.push(i),
            None => out.push(Line { source: i.source.clone(), pattern: i.pattern.clone(), matched: vec![i] }),
        }
    }
    out
}

/// Ask. The answer is the proposals that survived being checked -- at least
/// one, or the reason there are none
pub fn ask(facts: &Facts, engine: Option<&str>) -> Result<Vec<Proposal>> {
    let found = crate::worktree::ignored(facts.main);
    let lines = lines_of(&found);
    if lines.is_empty() {
        bail!(crate::i18n::t("err.inherit.nothing"));
    }
    let paths: Vec<String> = found.iter().map(|i| i.path.clone()).collect();
    let sizes = crate::worktree::sizes(facts.main, &paths, SIZE_MOST);
    let origin = name_of(facts.main);
    let prompt = crate::i18n::fill(
        crate::asking::BRING_ASK,
        &[
            ("origin_folder", &facts.main.display().to_string()),
            ("origin", &origin),
            ("example", &facts.example.display().to_string()),
            ("name", &name_of(facts.example)),
            ("served", &match facts.served {
                Some(why) => format!("yes ({why})"),
                None => "no".into(),
            }),
            ("linkable", match facts.linkable {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            }),
            ("lines", &describe_lines(&lines, &sizes)),
            ("mentions", &mentions(facts.main, &lines, &origin)),
            ("hint", match facts.hint.trim() {
                "" => "(nothing)",
                said => said,
            }),
        ],
    );
    let system = format!(
        "{}\n\n{}\n\n{}",
        crate::asking::BRING_WHO,
        crate::asking::BRING_HOW,
        crate::asking::answer_in(&crate::i18n::language_name())
    );
    let said = crate::webui::ask_local_ai_shaped(&prompt, &system, &shape().to_string(), engine, ASK_TIMEOUT)?;
    let kept = read_answer(&said, &lines, facts);
    if kept.is_empty() {
        bail!(crate::i18n::t("err.inherit.unreadable"));
    }
    Ok(kept)
}

/// The shape the answer is held to
fn shape() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "lines": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "source": {"type": "string"},
                        "pattern": {"type": "string"},
                        "how": {"type": "string", "enum": ["copy", "replace", "link", "skip"]},
                        "replace": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "find": {"type": "string"},
                                    "with": {"type": "string"},
                                    "regex": {"type": "boolean"}
                                },
                                "required": ["find", "with", "regex"],
                                "additionalProperties": false
                            }
                        },
                        "reason": {"type": "string"}
                    },
                    "required": ["source", "pattern", "how", "replace", "reason"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["lines"],
        "additionalProperties": false
    })
}

/// A folder's own name
fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// A size as a person reads it
fn said_size(bytes: u64, files: u64, more: bool) -> String {
    let amount = match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    };
    let at_least = if more { "at least " } else { "" };
    format!("{at_least}{amount}, {at_least}{files} files")
}

/// Each line, what it matches, and how large that is
fn describe_lines(lines: &[Line], sizes: &[crate::worktree::Size]) -> String {
    let mut out = String::new();
    for l in lines {
        let (mut bytes, mut files, mut more) = (0u64, 0u64, false);
        for m in &l.matched {
            if let Some(s) = sizes.iter().find(|s| s.path == m.path) {
                bytes += s.bytes;
                files += s.files;
                more |= s.more;
            }
        }
        let kind = match (l.matched.iter().all(|m| m.folder), l.has_files()) {
            (true, _) => "folders",
            (false, true) if l.matched.iter().any(|m| m.folder) => "files and folders",
            _ => "files",
        };
        let shown: Vec<&str> = l.matched.iter().take(8).map(|m| m.path.as_str()).collect();
        let rest = l.matched.len().saturating_sub(shown.len());
        let tail = if rest > 0 { format!(" and {rest} more") } else { String::new() };
        let big = if large(bytes, files) { " (large)" } else { "" };
        out.push_str(&format!(
            "- source {:?}, pattern {:?}: {} match(es), {kind}; {}{big}; e.g. {}{tail}\n",
            l.source,
            l.pattern,
            l.matched.len(),
            said_size(bytes, files, more),
            shown.join(", "),
        ));
    }
    out
}

/// Whether a line of a file looks like it holds a secret, whose values are
/// hidden before the line is shown to anybody
fn secret_line(line: &str) -> bool {
    let l = line.to_lowercase();
    ["pass", "secret", "token", "apikey", "api_key", "private", "credential"]
        .iter()
        .any(|w| l.contains(w))
}

/// A line with every quoted value in it hidden
fn hide_values(line: &str) -> String {
    static QUOTED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let quoted = QUOTED.get_or_init(|| regex::Regex::new(r#"'[^']*'|"[^"]*""#).expect("the pattern is fixed"));
    let hidden = quoted.replace_all(line, "'***'").to_string();
    // A value written without quotes (`KEY=value`) goes too
    match hidden.split_once('=') {
        Some((k, v)) if !v.contains("'***'") => format!("{k}=***"),
        _ => hidden,
    }
}

/// The files a line matches that name the checkout by its folder name, with
/// the lines that do. These are the files a copy would leave pointing back
/// at the checkout, which is the whole reason a replacement exists
fn mentions(main: &Path, lines: &[Line], origin: &str) -> String {
    const MOST_FILES: usize = 15;
    const MOST_LINES: usize = 20;
    const LARGEST: u64 = 256 * 1024;
    if origin.is_empty() {
        return "(none)".into();
    }
    let mut out = String::new();
    let mut files = 0;
    for l in lines {
        for m in l.matched.iter().filter(|m| !m.folder) {
            if files >= MOST_FILES {
                break;
            }
            let at = main.join(&m.path);
            if std::fs::metadata(&at).map(|x| x.len()).unwrap_or(u64::MAX) > LARGEST {
                continue;
            }
            // Text only: a file that is not text has no line to rewrite
            let Ok(text) = std::fs::read_to_string(&at) else { continue };
            let hits: Vec<(usize, &str)> =
                text.lines().enumerate().filter(|(_, t)| t.contains(origin)).take(MOST_LINES).collect();
            if hits.is_empty() {
                continue;
            }
            files += 1;
            let total = text.lines().filter(|t| t.contains(origin)).count();
            out.push_str(&format!("### {} (line {:?}, {total} line(s) mention it)\n", m.path, l.pattern));
            for (n, t) in hits {
                let t: String = t.chars().take(300).collect();
                let t = if secret_line(&t) { hide_values(&t) } else { t };
                out.push_str(&format!("{}: {}\n", n + 1, t.trim_end()));
            }
        }
    }
    if out.is_empty() {
        return "(none)".into();
    }
    out
}

/// What the AI said, kept where it holds up.
///
/// A proposal is dropped, not repaired, when it does not fit what was
/// offered: a line nobody asked about, a way that does not exist, a
/// replacement in something that is only folders, a link where the facts
/// said no link can be made or the project is served where it stands, or a
/// regular expression that does not compile. Each line once, the first time
fn read_answer(said: &str, lines: &[Line], facts: &Facts) -> Vec<Proposal> {
    #[derive(serde::Deserialize)]
    struct Answer {
        #[serde(default)]
        lines: Vec<Proposal>,
    }
    let body = said.trim();
    let body = match (body.find('{'), body.rfind('}')) {
        (Some(a), Some(b)) if b > a => &body[a..=b],
        _ => return Vec::new(),
    };
    let Ok(answer) = serde_json::from_str::<Answer>(body) else { return Vec::new() };
    let mut kept: Vec<Proposal> = Vec::new();
    for mut p in answer.lines {
        let source = match p.source.trim() {
            "" => ".gitignore".to_string(),
            s => s.to_string(),
        };
        let Some(line) = lines.iter().find(|l| l.source == source && l.pattern == p.pattern) else { continue };
        if kept.iter().any(|k| k.source == source && k.pattern == p.pattern) {
            continue;
        }
        if !crate::worktree::HOWS.contains(&p.how.as_str()) {
            continue;
        }
        match p.how.as_str() {
            "replace" => {
                p.replace.retain(|r| !r.find.is_empty());
                let compiles = p.replace.iter().all(|r| !r.regex || regex::Regex::new(&r.find).is_ok());
                if !line.has_files() || p.replace.is_empty() || !compiles {
                    continue;
                }
            }
            "link" if facts.served.is_some() || facts.linkable == Some(false) => continue,
            _ => p.replace.clear(),
        }
        p.source = source;
        p.reason = p.reason.trim().to_string();
        kept.push(p);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignored(path: &str, pattern: &str, folder: bool) -> crate::worktree::Ignored {
        crate::worktree::Ignored {
            path: path.into(),
            folder,
            source: ".gitignore".into(),
            line: 1,
            pattern: pattern.into(),
            secret: false,
        }
    }

    /// What the AI says is taken only where it fits what it was shown: a
    /// replacement in a folder, a link on a served project, a line nobody
    /// asked about and a pattern that does not compile are all left out, and
    /// the settings keep what they had for those lines
    #[test]
    fn an_answer_is_kept_only_where_it_fits_what_was_offered() {
        let found = vec![
            ignored("src/config/default/local.php", "src/config/default/*", false),
            ignored("vendor/", "vendor/", true),
            ignored("cache/", "cache/", true),
            ignored("logs/", "logs/", true),
        ];
        let lines = lines_of(&found);
        let main = Path::new("shop");
        let example = Path::new("shop-feature-x");
        let served = Facts { main, example, served: Some("webroot/.htaccess"), linkable: Some(true), hint: "" };
        let said = r#"Here you go: {"lines": [
            {"source": "", "pattern": "src/config/default/*", "how": "replace",
             "replace": [{"find": "shop", "with": "{name}", "regex": false}], "reason": "Names the checkout."},
            {"source": ".gitignore", "pattern": "vendor/", "how": "link", "replace": [], "reason": "Large."},
            {"source": ".gitignore", "pattern": "cache/", "how": "replace",
             "replace": [{"find": "x", "with": "y", "regex": false}], "reason": "?"},
            {"source": ".gitignore", "pattern": "logs/", "how": "skip", "replace": [], "reason": "Logs."},
            {"source": ".gitignore", "pattern": "nowhere/", "how": "copy", "replace": [], "reason": "?"},
            {"source": ".gitignore", "pattern": "logs/", "how": "copy", "replace": [], "reason": "again"}
        ]}"#;
        let kept = read_answer(said, &lines, &served);
        let got: Vec<(&str, &str)> = kept.iter().map(|p| (p.pattern.as_str(), p.how.as_str())).collect();
        assert_eq!(got, vec![("src/config/default/*", "replace"), ("logs/", "skip")]);
        assert_eq!(kept[0].source, ".gitignore", "an empty source is the project's own file");
        assert_eq!(kept[0].replace[0].with, "{name}");

        // Not served and linkable, the same link is kept
        let open = Facts { main, example, served: None, linkable: Some(true), hint: "" };
        let kept = read_answer(said, &lines, &open);
        assert!(kept.iter().any(|p| p.pattern == "vendor/" && p.how == "link"));

        // A pattern that does not compile is not kept
        let bad = r#"{"lines": [{"source": ".gitignore", "pattern": "src/config/default/*", "how": "replace",
            "replace": [{"find": "(", "with": "x", "regex": true}], "reason": ""}]}"#;
        assert!(read_answer(bad, &lines, &open).is_empty());
        // Nor is something that is not an answer at all
        assert!(read_answer("I could not decide.", &lines, &open).is_empty());
    }

    /// A secret on a line that names the checkout is not shown to the AI:
    /// the line is, with its values hidden
    #[test]
    fn a_secret_beside_the_folder_name_is_hidden() {
        assert_eq!(
            hide_values("define('_DB_PASS_', 'hunter2');"),
            "define('***', '***');"
        );
        assert_eq!(hide_values("DB_PASSWORD=hunter2"), "DB_PASSWORD=***");
        assert!(secret_line("define('_DB_PASS_','x')"));
        assert!(!secret_line("define('_HOME_','http://dev.local/www/shop/')"));
    }

    /// What the AI is shown is the files that name the checkout, with the
    /// lines that do -- a secret on one of them hidden -- and each line of the
    /// ignore files with what it matches and how large that is
    #[test]
    fn the_question_shows_what_names_the_checkout() {
        let main = crate::test_temp("inherit-facts").join("site");
        let _ = std::fs::remove_dir_all(&main);
        std::fs::create_dir_all(main.join("config")).unwrap();
        std::fs::create_dir_all(main.join("vendor")).unwrap();
        std::fs::write(
            main.join("config").join("local.php"),
            "<?php\ndefine('_ROOT_', '/srv/site/');\ndefine('_DB_PASS_', 'hunter2 site');\n$x = 1;\n",
        )
        .unwrap();
        std::fs::write(main.join("vendor").join("a.php"), "<?php // site\n").unwrap();
        let found = vec![
            ignored("config/local.php", "config/*", false),
            ignored("vendor/", "vendor/", true),
        ];
        let lines = lines_of(&found);
        let said = mentions(&main, &lines, "site");
        assert!(said.contains("### config/local.php"), "{said}");
        assert!(said.contains("2: define('_ROOT_', '/srv/site/');"), "{said}");
        assert!(!said.contains("hunter2"), "a secret reached the question: {said}");
        assert!(said.contains("3: define('***', '***');"), "{said}");
        assert!(!said.contains("$x"), "a line that does not name the checkout was shown: {said}");
        // A folder has no line to rewrite, so what is inside one is not shown
        assert!(!said.contains("vendor"), "{said}");

        let paths: Vec<String> = found.iter().map(|i| i.path.clone()).collect();
        let sizes = crate::worktree::sizes(&main, &paths, SIZE_MOST);
        let listed = describe_lines(&lines, &sizes);
        assert!(listed.contains("pattern \"config/*\": 1 match(es), files;"), "{listed}");
        assert!(listed.contains("pattern \"vendor/\": 1 match(es), folders;"), "{listed}");
        assert!(listed.contains("1 files"), "{listed}");
        assert_eq!(mentions(&main, &lines, "nowhere"), "(none)");
    }

    /// Sizes are said the way a person reads them, and "at least" when the
    /// count stopped before the end
    #[test]
    fn a_size_is_said_in_the_unit_that_fits() {
        assert_eq!(said_size(512, 1, false), "512 B, 1 files");
        assert_eq!(said_size(3 << 20, 40, false), "3.0 MB, 40 files");
        assert_eq!(said_size(5 << 30, 300_000, true), "at least 5.0 GB, at least 300000 files");
        assert!(large(LARGE_BYTES, 0) && large(0, LARGE_FILES) && !large(LARGE_BYTES - 1, LARGE_FILES - 1));
    }
}
