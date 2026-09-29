//! Finding keys to somebody's accounts in text that is about to leave.
//!
//! What a person picked on a page is pasted into an AI's input, and from there
//! into a conversation that is kept -- by the AI's company, in a history file.
//! A page can show a key: a settings screen with an API token on it, a
//! developer console, a pasted `.env`. So before anything picked is handed on,
//! the kinds of value that are keys by their shape are found here and hidden.
//!
//! Two ways, the ones secret scanners have long settled on: the shapes the
//! big issuers give their keys (a fixed prefix and a fixed alphabet), and,
//! for everything else, a long run of characters too random to be words.
//! The second is kept conservative on purpose -- a scanner that hides every
//! hash and every id teaches people to stop reading what it hid.
//!
//! This finds; it does not promise. What it hides is counted and said, so the
//! person looking at the draft knows to look.

use std::sync::LazyLock;

use regex::Regex;

/// What a hidden value is replaced with. Short, and plainly not the value
pub const HIDDEN: &str = "[hidden]";

/// The shapes issuers give their keys, each with why it is on the list
static SHAPES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // A private key in PEM, whole: the armour lines and what is between
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        // GitHub: personal, OAuth, user-to-server, server-to-server, refresh
        r"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
        // GitHub's fine-grained personal tokens
        r"\bgithub_pat_[A-Za-z0-9_]{22,}\b",
        // Anthropic, then OpenAI (and the many services that copied its prefix)
        r"\bsk-ant-[A-Za-z0-9_\-]{20,}",
        r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{20,}",
        // Stripe's secret, restricted and publishable keys
        r"\b[rsp]k_(?:live|test)_[A-Za-z0-9]{16,}\b",
        // AWS access key ids (long-lived and temporary)
        r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        // Slack's bot, user, app and refresh tokens
        r"\bxox[abprse]-[A-Za-z0-9\-]{10,}",
        // Google API keys
        r"\bAIza[0-9A-Za-z_\-]{35}\b",
        // A JSON Web Token: three base64url parts, the first a JSON header
        r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        // What follows "Bearer " in a header written out on a page
        r"(?i)\bbearer\s+[A-Za-z0-9._~+/\-]{16,}=*",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("a secret shape does not compile"))
    .collect()
});

/// A run of the characters keys are written in, long enough to be looked at
/// for randomness
static RUNS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/_\-]{32,}={0,2}").expect("the run pattern does not compile"));

/// Bits per character above which a run is too random to be words. Base64 of
/// random bytes sits near 6, hex near 4; English written without spaces sits
/// near 4 as well, which is why a run also has to mix letters and digits.
/// 4.3 is where the scanners that use this test put the line for base64-ish
/// strings, and it keeps a long lowercase hex hash (under 4) out
const RANDOM_BITS: f64 = 4.3;

fn bits_per_char(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let n = s.chars().count() as f64;
    counts.values().map(|&k| {
        let p = k as f64 / n;
        -p * p.log2()
    }).sum()
}

fn looks_random(run: &str) -> bool {
    let has_digit = run.chars().any(|c| c.is_ascii_digit());
    let has_upper = run.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = run.chars().any(|c| c.is_ascii_lowercase());
    has_digit && has_upper && has_lower && bits_per_char(run) >= RANDOM_BITS
}

/// `text` with every key-shaped value hidden, and how many were
pub fn hide(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut hidden = 0;
    for shape in SHAPES.iter() {
        let n = shape.find_iter(&out).count();
        if n > 0 {
            hidden += n;
            out = shape.replace_all(&out, HIDDEN).into_owned();
        }
    }
    let mut kept = String::with_capacity(out.len());
    let mut last = 0;
    for m in RUNS.find_iter(&out) {
        if looks_random(m.as_str()) {
            kept.push_str(&out[last..m.start()]);
            kept.push_str(HIDDEN);
            last = m.end();
            hidden += 1;
        }
    }
    kept.push_str(&out[last..]);
    (kept, hidden)
}

/// `text` with every one of `values` hidden where it appears whole. The values
/// this program itself holds -- a secret written down in its settings is known
/// exactly, whatever shape it has. Values shorter than four characters are not
/// looked for: a secret of three letters is in every word
pub fn hide_known<'a>(text: &str, values: impl IntoIterator<Item = &'a str>) -> (String, usize) {
    let mut out = text.to_string();
    let mut hidden = 0;
    for v in values {
        let v = v.trim();
        if v.chars().count() < 4 {
            continue;
        }
        let n = out.matches(v).count();
        if n > 0 {
            hidden += n;
            out = out.replace(v, HIDDEN);
        }
    }
    (out, hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made up here, in the shapes the issuers use. None of them is a real key
    fn fake(prefix: &str, alphabet: &str, n: usize) -> String {
        let a: Vec<char> = alphabet.chars().collect();
        let body: String = (0..n).map(|i| a[(i * 7 + 3) % a.len()]).collect();
        format!("{prefix}{body}")
    }
    const MIXED: &str = "aB3cD9eF1gH7iJ5kL0mN2oP4qR6sT8uVwXyZ";
    const UPPER_DIGITS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    fn hides(value: &str) {
        let (out, n) = hide(&format!("token: {value} end"));
        assert!(!out.contains(value), "{value} was not hidden: {out}");
        assert!(n >= 1, "{value} was not counted");
        assert!(out.starts_with("token: ") && out.ends_with(" end"), "the words around it went too: {out}");
    }

    #[test]
    fn github_tokens() {
        hides(&fake("ghp_", MIXED, 36));
        hides(&fake("gho_", MIXED, 36));
        hides(&fake("github_pat_", MIXED, 60));
    }

    #[test]
    fn ai_service_keys() {
        hides(&fake("sk-ant-api03-", MIXED, 40));
        hides(&fake("sk-proj-", MIXED, 40));
        hides(&fake("sk-", MIXED, 32));
    }

    #[test]
    fn cloud_and_chat_keys() {
        hides(&fake("AKIA", UPPER_DIGITS, 16));
        hides(&fake("xoxb-", "0123456789", 24));
        hides(&fake("AIza", MIXED, 35));
        hides(&fake("sk_live_", MIXED, 24));
    }

    #[test]
    fn a_json_web_token() {
        hides(&format!("eyJhbGciOiJIUzI1NiJ9.{}.{}", fake("", MIXED, 20), fake("", MIXED, 24)));
    }

    #[test]
    fn a_bearer_header_written_on_a_page() {
        let (out, n) = hide("Authorization: Bearer abcdef0123456789abcdef");
        assert_eq!(n, 1);
        assert!(!out.contains("abcdef0123456789abcdef"));
    }

    #[test]
    fn a_private_key_block_whole() {
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjE\nAAAA\n-----END OPENSSH PRIVATE KEY-----";
        let (out, n) = hide(&format!("<pre>{pem}</pre>"));
        assert_eq!(out, format!("<pre>{HIDDEN}</pre>"));
        assert_eq!(n, 1);
    }

    #[test]
    fn a_long_random_string_of_no_known_shape() {
        hides("Zq8Xv2Lm9Tp4Rk7Wn3Bs6Yd1Hf5Gc0JuQe");
    }

    #[test]
    fn what_is_not_a_key_is_left_alone() {
        for plain in [
            "<button class=\"btn btn-primary\">Save changes</button>",
            "background-color: rgb(34, 102, 221); font-family: \"Segoe UI\", sans-serif",
            // A lowercase hex hash, like a commit or a file's name on a CDN
            "d41d8cd98f00b204e9800998ecf8427e0123456789abcdef",
            "main > section.card > div.profile-settings-container-wrapper",
            "abcdefghijklmnopqrstuvwxyzabcdefghij",
            "sk-small",
        ] {
            let (out, n) = hide(plain);
            assert_eq!((out.as_str(), n), (plain, 0), "{plain} was taken for a key");
        }
    }

    #[test]
    fn a_value_this_program_holds_is_hidden_whatever_its_shape() {
        let (out, n) = hide_known("user hunter2go logged in as hunter2go", ["hunter2go", "ab", ""]);
        assert_eq!(out, format!("user {HIDDEN} logged in as {HIDDEN}"));
        assert_eq!(n, 2);
    }
}
