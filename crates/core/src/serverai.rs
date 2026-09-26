//! The AIs a server reached over SSH has.
//!
//! This app puts no AI on a server: whoever runs the server sets it up, and
//! then points this app at it. So what a folder there can hand its work to is
//! what that server has, never what this PC has -- a tab typing `claude` on a
//! server with only Codex is a `command not found` and nothing else.
//!
//! Asking takes a connection and a login shell there, far too long for the
//! loop that draws the board, so the answer is kept: asked on a thread the
//! first time anything wants it, and again once it is [`FRESH`] old, while
//! the old answer is still given. Until the first answer, [`known`] says
//! `None`, and whoever asked goes on as it would with no answer at all.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long an answer is trusted before the server is asked again. An AI is
/// installed on a server now and then, not from minute to minute
pub const FRESH: Duration = Duration::from_secs(600);

/// What each line naming an AI starts with, so nothing else a login shell
/// prints on its way in is read as one
const MARK: &str = "__SHIKISHA_AI__ ";

#[derive(Default)]
struct Kept {
    /// The commands of the AIs found there, in the profiles' order
    found: Option<Vec<String>>,
    asked: Option<Instant>,
    asking: bool,
}

fn kept() -> &'static Mutex<HashMap<String, Kept>> {
    static KEPT: OnceLock<Mutex<HashMap<String, Kept>>> = OnceLock::new();
    KEPT.get_or_init(Default::default)
}

/// A server by what reaches it: its entry's name and address, so an entry
/// pointed somewhere else is a server asked afresh
fn key_of(host: &crate::config::HostSpec) -> String {
    format!("{}\n{}", host.name.trim(), host.at.trim())
}

/// The AI commands this app knows, in the profiles' order: the first word
/// each profile matches on (`claude`, `codex`, ...)
pub fn known_commands() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for pf in crate::profile::files() {
        let Some(key) = pf.command_match.first().map(|k| k.trim().to_string()) else { continue };
        let plain = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if plain && !out.iter().any(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            out.push((key, pf.name));
        }
    }
    out
}

/// The AIs the server has, as last heard, asking it again on a thread when
/// it has not been asked or the answer is old. `None` until the first answer
pub fn known(host: &crate::config::HostSpec) -> Option<Vec<String>> {
    if host.is_made() {
        return None;
    }
    let key = key_of(host);
    let mut all = kept().lock().unwrap_or_else(|e| e.into_inner());
    let k = all.entry(key.clone()).or_default();
    let stale = k.asked.is_none_or(|at| at.elapsed() > FRESH);
    if stale && !k.asking {
        k.asking = true;
        let host = host.clone();
        std::thread::spawn(move || {
            let found = ask(&host);
            let mut all = kept().lock().unwrap_or_else(|e| e.into_inner());
            let k = all.entry(key).or_default();
            k.asking = false;
            k.asked = Some(Instant::now());
            // A server that could not be asked keeps what it last said: a
            // blip on the network is not an AI uninstalled
            if let Some(found) = found {
                k.found = Some(found);
            }
        });
    }
    k.found.clone()
}

/// Asked of the server: which of the AI commands a login shell there finds.
/// A login shell, because that is where an installer puts its folder on the
/// PATH (`~/.local/bin`, a Node version manager). `None` when it could not be
/// asked at all
fn ask(host: &crate::config::HostSpec) -> Option<Vec<String>> {
    let commands = known_commands();
    if commands.is_empty() {
        return Some(Vec::new());
    }
    let names = commands.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(" ");
    let inner = format!("for c in {names}; do command -v \"$c\" >/dev/null 2>&1 && echo \"{MARK}$c\"; done; true");
    let line = format!("${{SHELL:-sh}} -lic {} </dev/null 2>/dev/null", crate::ssh::sh_quote(&inner));
    let at = crate::elsewhere::Elsewhere::of(host).ok()?;
    let ran = crate::elsewhere::exec(&at, &line, 30_000).ok()?;
    Some(found_in(&ran.out, &commands))
}

/// The commands named in what the server said, in the profiles' order
fn found_in(said: &str, commands: &[(String, String)]) -> Vec<String> {
    let named: Vec<&str> = said.lines().filter_map(|l| l.trim_end_matches('\r').strip_prefix(MARK)).map(str::trim).collect();
    commands
        .iter()
        .filter(|(k, _)| named.iter().any(|n| n.eq_ignore_ascii_case(k)))
        .map(|(k, _)| k.clone())
        .collect()
}

/// Which of a server's AIs to use: the one the settings chose when the server
/// has it, else the first it has in `order`, else the first it has at all
pub fn choose(found: &[String], chosen: &str, order: &[&str]) -> Option<String> {
    let has = |k: &str| found.iter().find(|f| f.eq_ignore_ascii_case(k)).cloned();
    let chosen = chosen.split_whitespace().next().unwrap_or_default();
    (!chosen.is_empty())
        .then(|| has(chosen))
        .flatten()
        .or_else(|| order.iter().find_map(|k| has(k)))
        .or_else(|| found.first().cloned())
}

/// Written down as the answer for a server, as a test or a caller that asked
/// by some other road would
#[cfg(test)]
pub fn set_known(host: &crate::config::HostSpec, found: Vec<String>) {
    let mut all = kept().lock().unwrap_or_else(|e| e.into_inner());
    let k = all.entry(key_of(host)).or_default();
    k.found = Some(found);
    k.asked = Some(Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a login shell prints on its way in is not an AI; only the marked
    /// lines are, and they come back in the profiles' order
    #[test]
    fn a_servers_ais_are_read_from_the_marked_lines_only() {
        let commands = vec![("claude".to_string(), "Claude Code".to_string()), ("codex".to_string(), "Codex".to_string())];
        let said = format!("bash: no job control in this shell\r\nclaude\n{MARK}codex\r\n{MARK}aider\n");
        assert_eq!(found_in(&said, &commands), vec!["codex".to_string()]);
        assert!(found_in("", &commands).is_empty());
    }

    /// The settings' AI when the server has it, else the first the server has
    /// in the order the buttons use
    #[test]
    fn the_settings_ai_is_used_where_the_server_has_it() {
        let found = vec!["codex".to_string(), "claude".to_string()];
        let order = ["claude", "codex"];
        assert_eq!(choose(&found, "codex --yolo", &order).as_deref(), Some("codex"));
        assert_eq!(choose(&found, "gemini", &order).as_deref(), Some("claude"), "one the server lacks is typed there");
        assert_eq!(choose(&found, "", &order).as_deref(), Some("claude"));
        assert_eq!(choose(&["kimi".to_string()], "", &order).as_deref(), Some("kimi"));
        assert_eq!(choose(&[], "claude", &order), None, "a server with no AI is given one");
    }
}
