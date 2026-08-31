//! Internationalization of displayed text.
//!
//! - English is the baseline. `lang/en.json` is embedded in the exe, so
//!   display always works even with zero language files present
//! - `lang/<code>.json` is layered on top. **Keys missing a translation
//!   stay in English**, so stale translations never break anything
//! - Contributors only need to copy en.json, translate the values, and drop
//!   it in as `lang/fr.json`

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// The baseline English. Embedded so it works even without a language file in the distribution
const EN: &str = include_str!("../lang/en.json");

static DICT: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static LANG: OnceLock<RwLock<String>> = OnceLock::new();

fn dict() -> &'static RwLock<HashMap<String, String>> {
    DICT.get_or_init(|| RwLock::new(parse(EN)))
}

fn parse(json: &str) -> HashMap<String, String> {
    serde_json::from_str::<HashMap<String, serde_json::Value>>(json)
        .map(|m| {
            m.into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Decides which language to use and loads it.
/// If `lang` is None or "auto", infers it from the OS setting
pub fn init(lang: Option<&str>, dirs: &[std::path::PathBuf]) {
    let code = match lang.map(str::trim).filter(|s| !s.is_empty() && *s != "auto") {
        Some(c) => c.to_string(),
        None => system_language(),
    };
    let mut map = parse(EN);
    // If not English, layer that language's file on top (anything missing stays English)
    if code != "en" {
        for dir in dirs {
            let path = dir.join("lang").join(format!("{code}.json"));
            if let Ok(text) = std::fs::read_to_string(&path) {
                for (k, v) in parse(&text) {
                    map.insert(k, v);
                }
                break;
            }
        }
    }
    let _ = dict().write().map(|mut d| *d = map);
    let _ = LANG.get_or_init(|| RwLock::new(code.clone())).write().map(|mut l| *l = code);
}

/// Whether restarting with this language setting would change the language
/// currently running. If it would, we can prompt "takes effect on restart."
/// The decision logic must match `init`
/// (otherwise the prompt would show when nothing actually changes, or vice versa)
pub fn would_change(lang: Option<&str>) -> bool {
    let next = match lang.map(str::trim).filter(|s| !s.is_empty() && *s != "auto") {
        Some(c) => c.to_string(),
        None => system_language(),
    };
    next != self::lang()
}

/// Current language code (e.g. "ja")
pub fn lang() -> String {
    LANG.get()
        .and_then(|l| l.read().ok().map(|s| s.clone()))
        .unwrap_or_else(|| "en".into())
}

/// Looks up a string. An unknown key returns the key itself (so a missing string is noticeable on screen)
pub fn t(key: &str) -> String {
    dict()
        .read()
        .ok()
        .and_then(|d| d.get(key).cloned())
        .unwrap_or_else(|| key.to_string())
}

/// Looks up a string with interpolation. Replaces `{name}`-style placeholders
pub fn tp(key: &str, args: &[(&str, &str)]) -> String {
    fill(&t(key), args)
}

/// Performs only the `{name}` substitution
fn fill(text: &str, args: &[(&str, &str)]) -> String {
    let mut s = text.to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Replaces every `{{key}}` in a template at once (for HTML pages)
pub fn render(template: &str) -> String {
    // For `<html lang="...">`. Not a dictionary key, so substitute it in first
    let template = template.replace("{{__lang__}}", &lang());
    match dict().read() {
        Ok(d) => render_with(&template, &d),
        Err(_) => template,
    }
}

fn render_with(template: &str, d: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(j) => {
                let key = &after[..j];
                out.push_str(d.get(key).map(String::as_str).unwrap_or(key));
                rest = &after[j + 2..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Dictionary handed to the screen's JavaScript
pub fn dict_json() -> String {
    dict()
        .read()
        .ok()
        .and_then(|d| serde_json::to_string(&*d).ok())
        .unwrap_or_else(|| "{}".into())
}

/// OS display language ("ja-JP" -> "ja")
fn system_language() -> String {
    let raw = std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(os_language)
        .unwrap_or_else(|| "en".into());
    raw.split(['-', '_', '.'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
}

#[cfg(windows)]
fn os_language() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..(n - 1) as usize]))
}

#[cfg(not(windows))]
fn os_language() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_english_is_valid_and_complete() {
        let en = parse(EN);
        assert!(!en.is_empty(), "英語の辞書が読める");
        assert!(en.contains_key("app.title"), "基本のキーがある");
        // Not allowed to have an empty value, or the screen would show blank
        assert!(en.values().all(|v| !v.trim().is_empty()), "空の文言が無い");
    }

    /// Confirms shipped translation files only use keys that exist in English (a typo'd key is otherwise silently ignored)
    #[test]
    fn shipped_translations_use_known_keys() {
        let en = parse(EN);
        // A relative path would break when running in parallel with another
        // test that changes the current directory (this actually happened)
        let lang_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lang");
        for entry in std::fs::read_dir(&lang_dir).expect("langフォルダ") {
            let path = entry.unwrap().path();
            if path.file_name().unwrap() == "en.json" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for key in parse(&text).keys() {
                assert!(
                    en.contains_key(key),
                    "{}: 英語に無いキー {key}",
                    path.display()
                );
            }
        }
    }

    /// Every word the pages ask for by name is one English has.
    ///
    /// The pages ask by name, look the name up in a table built at
    /// run time, and a key nobody put in `en.json` simply comes back as itself
    /// -- so the mistake ships as a settings row labelled `settings.group.where`
    /// and is only ever found by someone opening that screen. This is the check
    /// that would have caught it before it was written.
    #[test]
    fn every_word_a_page_asks_for_exists() {
        let en = parse(EN);
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut asked = 0;
        for entry in std::fs::read_dir(&src).expect("srcフォルダ") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (at, _) in text.match_indices("T[\"") {
                let rest = &text[at + 3..];
                let Some(end) = rest.find('"') else { continue };
                let key = &rest[..end];
                // A key is written out in full; a trailing dot means the page
                // builds the rest at run time, and only the page knows from
                // what -- the one in `shell.rs` checks its own list
                if !key.contains('.') || key.ends_with('.') {
                    continue;
                }
                asked += 1;
                assert!(
                    en.contains_key(key),
                    "{}: 画面が使う {key} が英語に無い",
                    path.file_name().unwrap().to_string_lossy()
                );
            }
        }
        assert!(asked > 100, "画面の文言を読めていない ({asked}件)");
    }

    #[test]
    fn missing_translation_falls_back_to_english() {
        let en = parse(EN);
        let mut ja = HashMap::new();
        ja.insert("app.title".to_string(), "訳あり".to_string());
        // Same procedure as the real layering
        let mut merged = en.clone();
        for (k, v) in ja {
            merged.insert(k, v);
        }
        assert_eq!(merged["app.title"], "訳あり");
        assert_eq!(merged["common.save"], en["common.save"], "訳が無ければ英語");
    }

    #[test]
    fn unknown_key_shows_the_key_itself() {
        // Shows the key as-is, so a missing entry is noticeable
        assert_eq!(t("no.such.key.exists"), "no.such.key.exists");
    }

    #[test]
    fn placeholders_are_substituted() {
        assert_eq!(fill("Hello, {name}!", &[("name", "世界")]), "Hello, 世界!");
        assert_eq!(fill("{a}+{b}", &[("a", "1"), ("b", "2")]), "1+2");
    }

    #[test]
    fn templates_are_substituted() {
        let mut d = HashMap::new();
        d.insert("a".to_string(), "A".to_string());
        assert_eq!(render_with("<p>{{a}}</p><i>{{missing}}</i>", &d), "<p>A</p><i>missing</i>");
        assert_eq!(render_with("no placeholders", &d), "no placeholders");
        // Doesn't break even if the closing braces are forgotten
        assert_eq!(render_with("{{unclosed", &d), "{{unclosed");
    }

    #[test]
    fn language_code_is_normalized() {
        assert_eq!(
            "ja-JP".split(['-', '_', '.']).next().unwrap().to_ascii_lowercase(),
            "ja"
        );
    }
}
