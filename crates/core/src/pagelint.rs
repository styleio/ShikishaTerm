//! Whether a page's script calls anything that is not there.
//!
//! The screens this app serves are one long script inside a Rust string, and a
//! string is not compiled. A function deleted with the feature that used it
//! leaves its other callers standing, and the day somebody opens that card the
//! browser throws `ReferenceError` in the middle of drawing -- which shows up
//! as a pane with nothing in it, no message, and nothing in the log the person
//! can see. That is how the "Automatic names" card came to draw nothing for
//! three days (`modelCandidates`, removed 2026-09-18 with the desk templates,
//! still called by two cards).
//!
//! So the page is read for that one thing before it ships: every name it calls
//! is a name something in it defines. Only what is called, because a name used
//! as a value is a bug of a different size and a rule that fires on prose is a
//! rule that gets switched off.

use std::collections::HashSet;

/// Words that come before a `(` without being a call.
const KEYWORDS: &[&str] = &[
    "if", "for", "while", "switch", "catch", "return", "typeof", "function", "new", "delete",
    "void", "in", "of", "do", "else", "try", "throw", "await", "yield", "case", "instanceof",
    "super", "this", "import", "export", "const", "let", "var", "class", "get", "set", "async",
    "static", "true", "false", "null", "undefined",
];

/// What the browser brings, which the page may call without defining.
///
/// A list rather than "anything capitalised": the point of the check is that a
/// name the page invented has to exist in the page, and a list is the only way
/// to say which names it did not invent.
const GLOBALS: &[&str] = &[
    "fetch", "setTimeout", "setInterval", "clearTimeout", "clearInterval", "requestAnimationFrame",
    "cancelAnimationFrame", "alert", "confirm", "prompt", "Number", "String", "Boolean", "Array",
    "Object", "JSON", "Math", "Date", "Promise", "Set", "Map", "WeakMap", "RegExp", "Error",
    "parseInt", "parseFloat", "isNaN", "isFinite", "encodeURIComponent", "decodeURIComponent",
    "encodeURI", "decodeURI", "structuredClone", "queueMicrotask", "atob", "btoa", "URL",
    "URLSearchParams", "Blob", "File", "FormData", "Image", "Audio", "Event", "CustomEvent",
    "MutationObserver", "ResizeObserver", "IntersectionObserver", "AbortController", "TextEncoder",
    "TextDecoder", "WebSocket", "Intl", "Symbol", "BigInt", "Proxy", "Reflect", "EventSource",
    "FileReader", "Uint8Array", "Int32Array", "Float32Array", "ArrayBuffer", "DOMParser",
    "XMLHttpRequest", "getComputedStyle", "matchMedia", "scrollTo", "scrollBy", "addEventListener",
    "removeEventListener", "postMessage", "dispatchEvent", "getSelection", "crypto", "Notification",
];

/// The names `code` calls and nothing in it defines, in the order they are
/// first called.
///
/// Give it code, not the page: [`code_only`] takes the prose out first.
pub fn dangling_calls(code: &str) -> Vec<String> {
    let known: HashSet<&str> = KEYWORDS.iter().chain(GLOBALS).copied().collect();
    let mut called: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut defined: HashSet<String> = HashSet::new();
    let src: Vec<char> = code.chars().collect();
    let mut i = 0;
    // The word and the character before the name being read: a name is defined
    // by what stands on either side of it, and `function x` and `(x, y)` are
    // both something standing on the left
    let mut before = ' ';
    let mut word = String::new();
    while i < src.len() {
        let c = src[i];
        // A name never starts with a digit, so a run that does is a number
        if !is_name_char(c) || c.is_ascii_digit() {
            if !c.is_whitespace() {
                before = c;
                word.clear();
            }
            i += 1;
            continue;
        }
        let start = i;
        while i < src.len() && is_name_char(src[i]) {
            i += 1;
        }
        let name: String = src[start..i].iter().collect();
        let at = next_code_at(&src, i);
        let after = at.map(|n| src[n]).unwrap_or(' ');
        // `x => ...`: one parameter, written without its brackets
        let arrow = after == '=' && at.and_then(|n| src.get(n + 1)) == Some(&'>');
        let is_property = before == '.';
        // `name(a, b) {` is a method written the short way: a definition that
        // reads exactly like a call, and the only one there is
        let is_method = after == '(' && takes_a_body(&src, i);
        if !is_property {
            // `typeof x === "function"` is how the script says a name may not
            // be here: the block that calls it is the page that has it
            if matches!(word.as_str(), "function" | "const" | "let" | "var" | "class" | "typeof")
                || after == ':'
                || arrow
                || is_method
                || (matches!(before, '(' | ',' | '{' | '[')
                    && matches!(after, ',' | ')' | '}' | ']' | '='))
            {
                defined.insert(name.clone());
            }
            if after == '('
                && !is_method
                && !known.contains(name.as_str())
                && seen.insert(name.clone())
            {
                called.push(name.clone());
            }
        }
        before = ' ';
        word = name;
    }
    called.into_iter().filter(|n| !defined.contains(n)).collect()
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Whether what follows `i` is a bracket pair with a body after it, which is
/// how a method is written when it is written the short way.
fn takes_a_body(src: &[char], i: usize) -> bool {
    let Some(open) = src[i..].iter().position(|c| !c.is_whitespace()).map(|n| i + n) else {
        return false;
    };
    let mut depth = 0usize;
    // A list of parameters is short. Anything longer is a call carrying
    // something big, and reading to the end of the page to find that out would
    // cost this check a second per call
    const FAR: usize = 400;
    let end = (open + FAR).min(src.len());
    for (n, c) in src[open..end].iter().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return next_code(src, open + n + 1) == '{';
                }
            }
            _ => {}
        }
    }
    false
}

/// The next character that is not a space, from `i` on.
fn next_code(src: &[char], i: usize) -> char {
    next_code_at(src, i).map(|n| src[n]).unwrap_or(' ')
}

/// Where that character is.
fn next_code_at(src: &[char], i: usize) -> Option<usize> {
    src.get(i..)?.iter().position(|c| !c.is_whitespace()).map(|n| i + n)
}

/// A page's script, with everything that is not script blanked out.
///
/// The style block is the reason this exists: `calc()`, `rgba()` and
/// `minmax()` are calls to nothing in every page ever written, and a check
/// that reads the whole file reports them forever.
pub fn scripts_of(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open) = rest.find("<script") {
        let Some(body) = rest[open..].find('>').map(|n| open + n + 1) else { break };
        let end = rest[body..].find("</script>").map(|n| body + n).unwrap_or(rest.len());
        blank(&rest[..body], &mut out);
        out.push_str(&rest[body..end]);
        rest = &rest[end..];
    }
    blank(rest, &mut out);
    out
}

/// The same text with only its newlines kept, so what follows keeps its line.
fn blank(text: &str, out: &mut String) {
    out.extend(text.chars().map(|c| match c {
        '\n' => '\n',
        _ => ' ',
    }));
}

/// The script with its comments, strings and regular expressions blanked out.
///
/// Newlines stay where they are, so a line number taken from the result is the
/// line number in the page. Everything else becomes a space: an English
/// sentence in a comment has brackets in it too, and read as code it says that
/// the page calls `shortcut` and `enough`.
pub fn code_only(js: &str) -> String {
    let src: Vec<char> = js.chars().collect();
    let mut out = String::with_capacity(js.len());
    let mut i = 0;
    // What stands before a `/`, which is the only way to tell a regular
    // expression from a division
    let mut before = ' ';
    let mut word = String::new();
    while i < src.len() {
        let c = src[i];
        let next = src.get(i + 1).copied().unwrap_or('\0');
        let ends = |from: usize, what: &str| -> usize {
            let pat: Vec<char> = what.chars().collect();
            let mut j = from;
            while j + pat.len() <= src.len() {
                if src[j..j + pat.len()] == pat[..] {
                    return j + pat.len();
                }
                j += 1;
            }
            src.len()
        };
        let stop = if c == '/' && next == '/' {
            src[i..].iter().position(|&c| c == '\n').map(|n| i + n).unwrap_or(src.len())
        } else if c == '/' && next == '*' {
            ends(i + 2, "*/")
        } else if c == '"' || c == '\'' || c == '`' {
            closing(&src, i, c)
        } else if c == '/' && starts_a_regex(before, &word) {
            closing_regex(&src, i)
        } else {
            out.push(c);
            if !c.is_whitespace() {
                before = c;
                match is_name_char(c) {
                    true => word.push(c),
                    false => word.clear(),
                }
            }
            i += 1;
            continue;
        };
        for c in &src[i..stop] {
            out.push(match c {
                '\n' => '\n',
                _ => ' ',
            });
        }
        // A string or a comment ends an expression the way a name does: what
        // follows a string's `/` is a division
        before = 'x';
        word.clear();
        i = stop;
    }
    out
}

/// Where the string opened at `at` ends, one past its closing quote.
fn closing(src: &[char], at: usize, quote: char) -> usize {
    let mut j = at + 1;
    while j < src.len() {
        match src[j] {
            '\\' => j += 2,
            c if c == quote => return j + 1,
            _ => j += 1,
        }
    }
    src.len()
}

/// The same for a regular expression, whose `/` inside `[...]` is not the end.
fn closing_regex(src: &[char], at: usize) -> usize {
    let mut j = at + 1;
    let mut in_class = false;
    while j < src.len() {
        match src[j] {
            '\\' => j += 2,
            '[' => {
                in_class = true;
                j += 1;
            }
            ']' => {
                in_class = false;
                j += 1;
            }
            '/' if !in_class => return j + 1,
            // A regular expression does not run over the end of a line. One
            // that appears to is a division, and stopping here keeps the
            // mistake to this line
            '\n' => return j,
            _ => j += 1,
        }
    }
    src.len()
}

/// Whether a `/` here opens a regular expression rather than dividing.
///
/// It divides only what can end an expression: a name, a number, a closing
/// bracket. Everything else -- an operator, a comma, the start of a line, and
/// the words that take an expression after them -- opens one
fn starts_a_regex(before: char, word: &str) -> bool {
    if matches!(
        word,
        "return" | "typeof" | "case" | "in" | "of" | "new" | "delete" | "void" | "do" | "else"
            | "yield" | "await" | "instanceof"
    ) {
        return true;
    }
    !(is_name_char(before) || matches!(before, ')' | ']' | '}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_with_nothing_to_call_is_named() {
        let js = "function a() { return b(1); }\nfunction c() { a(); }";
        assert_eq!(dangling_calls(&code_only(js)), vec!["b".to_string()]);
    }

    #[test]
    fn what_the_page_defines_in_every_way_it_defines_it() {
        let js = r#"
function one() {}
const two = () => {};
let three = function () {};
var four = 0;
class Five {}
const six = {seven(x) { return x; }};
[eight].forEach(eight => eight());
function nine(ten, eleven = 1) { return ten() + eleven(); }
one(); two(); three(); four(); new Five(); six.seven(); nine();
"#;
        assert!(dangling_calls(&code_only(js)).is_empty(), "{:?}", dangling_calls(&code_only(js)));
    }

    #[test]
    fn prose_and_strings_are_not_code() {
        // Every line here would read as a call if it were read as code
        let js = r#"
// the dialogs (all of them) are drawn by hand
/* a shortcut (Ctrl+K) opens it */
const s = "a summary (short)";
const t = `a name (${now()}) for it`;
const re = /nothing (here) matters/;
function now() { return 0; }
"#;
        assert!(dangling_calls(&code_only(js)).is_empty(), "{:?}", dangling_calls(&code_only(js)));
        assert!(code_only(js).lines().count() == js.lines().count(), "the lines moved");
    }

    #[test]
    fn a_quote_inside_a_regular_expression_does_not_open_a_string() {
        let js = "const q = /['\"]/; function after() {} after();";
        assert!(dangling_calls(&code_only(js)).is_empty(), "{:?}", dangling_calls(&code_only(js)));
    }

    #[test]
    fn a_division_is_not_a_regular_expression() {
        let js = "function half(n) { return n / 2; }\nfunction x() { return half(4) / 2; }\nx();";
        assert!(dangling_calls(&code_only(js)).is_empty(), "{:?}", dangling_calls(&code_only(js)));
    }

    #[test]
    fn what_the_browser_brings_is_not_missing() {
        let js = "fetch(\"/x\"); setTimeout(() => JSON.parse(\"{}\"), 0); document.getElementById(\"a\");";
        assert!(dangling_calls(&code_only(js)).is_empty(), "{:?}", dangling_calls(&code_only(js)));
    }
}
