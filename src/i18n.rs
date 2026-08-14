//! 表示文言の多言語化。
//!
//! - 基準は英語。`lang/en.json` をexeに埋め込むので、言語ファイルが1つも
//!   無くても必ず表示できる
//! - `lang/<code>.json` を上に重ねる。**翻訳が欠けているキーは英語のまま**出るので、
//!   翻訳が古くなっても壊れない
//! - 有志は en.json をコピーして値を訳し、`lang/fr.json` として置くだけでよい

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// 基準となる英語。配布物に言語ファイルが無くても動くよう埋め込む
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

/// 使用言語を決めて読み込む。
/// `lang` が None か "auto" ならOSの設定から推定する
pub fn init(lang: Option<&str>, dirs: &[std::path::PathBuf]) {
    let code = match lang.map(str::trim).filter(|s| !s.is_empty() && *s != "auto") {
        Some(c) => c.to_string(),
        None => system_language(),
    };
    let mut map = parse(EN);
    // 英語以外なら、その言語のファイルを上書きで重ねる (欠けた分は英語のまま)
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

/// この言語指定で起動し直したら、いま動いている言語と変わるか。
/// 変わるなら「再起動で反映される」と促せる。決め方は `init` と揃える
/// (揃えないと、案内は出るのに実際は変わらない/その逆が起きる)
pub fn would_change(lang: Option<&str>) -> bool {
    let next = match lang.map(str::trim).filter(|s| !s.is_empty() && *s != "auto") {
        Some(c) => c.to_string(),
        None => system_language(),
    };
    next != self::lang()
}

/// 現在の言語コード ("ja" など)
pub fn lang() -> String {
    LANG.get()
        .and_then(|l| l.read().ok().map(|s| s.clone()))
        .unwrap_or_else(|| "en".into())
}

/// 文言を引く。未知のキーはキー自体を返す (画面上で欠落に気づけるように)
pub fn t(key: &str) -> String {
    dict()
        .read()
        .ok()
        .and_then(|d| d.get(key).cloned())
        .unwrap_or_else(|| key.to_string())
}

/// 差し込み付きで引く。`{name}` の形を置き換える
pub fn tp(key: &str, args: &[(&str, &str)]) -> String {
    fill(&t(key), args)
}

/// `{name}` の差し込みだけを行う
fn fill(text: &str, args: &[(&str, &str)]) -> String {
    let mut s = text.to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// テンプレート中の `{{key}}` をまとめて置き換える (HTMLページ用)
pub fn render(template: &str) -> String {
    // `<html lang="...">` 用。辞書のキーではないので先に差し込んでおく
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

/// 画面のJavaScriptへ渡す辞書
pub fn dict_json() -> String {
    dict()
        .read()
        .ok()
        .and_then(|d| serde_json::to_string(&*d).ok())
        .unwrap_or_else(|| "{}".into())
}

/// OSの表示言語 ("ja-JP" → "ja")
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
        // 値が空だと画面が空白になるので許さない
        assert!(en.values().all(|v| !v.trim().is_empty()), "空の文言が無い");
    }

    /// 翻訳ファイルは英語のキーの範囲内であること (綴り間違いは黙って無視されるため)
    #[test]
    fn shipped_translations_use_known_keys() {
        let en = parse(EN);
        // 相対パスにすると、カレントディレクトリを変える別のテストと
        // 並列に走ったときに読めなくなる (実際に落ちた)
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

    #[test]
    fn missing_translation_falls_back_to_english() {
        let en = parse(EN);
        let mut ja = HashMap::new();
        ja.insert("app.title".to_string(), "訳あり".to_string());
        // 実際の重ね方と同じ手順
        let mut merged = en.clone();
        for (k, v) in ja {
            merged.insert(k, v);
        }
        assert_eq!(merged["app.title"], "訳あり");
        assert_eq!(merged["common.save"], en["common.save"], "訳が無ければ英語");
    }

    #[test]
    fn unknown_key_shows_the_key_itself() {
        // 欠落に気づけるよう、キーをそのまま出す
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
        // 閉じ忘れても壊れない
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
