//! Give an interactive CLI its own process when it offers that option.
//!
//! A shared daemon inherits the first terminal's environment. Other tabs
//! then report with that terminal's API key, and even their shell commands
//! speak as the wrong tab. Codex 0.160's --no-daemon avoids sharing that
//! environment. The option lives in its profile; older installs that do not
//! advertise it are left alone.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::profile::ResumeSpec;

fn flag<'a>(spec: Option<&'a ResumeSpec>, argv: &[String]) -> Option<&'a str> {
    let spec = spec?;
    let flag = spec.process_flag.as_deref()?;
    // Only a boolean long option, never a shell fragment or an option value.
    if argv.is_empty()
        || !flag.starts_with("--")
        || flag.len() <= 2
        || !flag[2..]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || argv
            .iter()
            .skip(1)
            .take_while(|a| a.as_str() != "--")
            .any(|a| {
                a == flag
                    || spec
                        .process_unless
                        .iter()
                        .any(|no| a == no || a.starts_with(&format!("{no}=")))
            })
    {
        return None;
    }
    Some(flag)
}

fn with_flag(argv: &[String], flag: &str) -> Vec<String> {
    let mut out = argv.to_vec();
    out.insert(1, flag.to_string());
    out
}

fn advertises(help: &str, flag: &str) -> bool {
    help.split(|c: char| c.is_whitespace() || matches!(c, ',' | '=' | '[' | ']'))
        .any(|word| word == flag)
}

struct Help {
    file: Option<(SystemTime, u64)>,
    checked: Instant,
    text: Option<String>,
}

fn supported(program: &str, flag: &str) -> bool {
    static HELP: OnceLock<Mutex<HashMap<String, Help>>> = OnceLock::new();
    let path = crate::tab::resolve_command(program).unwrap_or_else(|| program.into());
    let key = path.to_string_lossy().into_owned();
    let file = path
        .metadata()
        .ok()
        .and_then(|m| Some((m.modified().ok()?, m.len())));
    let cache = HELP.get_or_init(Mutex::default);
    if let Ok(cache) = cache.lock()
        && let Some(h) = cache.get(&key)
        && h.file == file
        // Also refresh unchanged npm shims: their package can be replaced
        // without replacing the shim. A failed probe is retried too.
        && h.checked.elapsed() < Duration::from_secs(60)
    {
        return h.text.as_deref().is_some_and(|s| advertises(s, flag));
    }
    let cmd = crate::tab::build_command(&[key.clone(), "--help".into()]);
    let args: Vec<_> = cmd
        .get_argv()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let words: Vec<_> = args.iter().skip(1).map(String::as_str).collect();
    // Reuse the bounded, hidden process runner. Never pass the person's
    // prompt or other arguments to this query, and never start a conversation.
    let text = crate::discover::run_briefly(&args[0], &words, Duration::from_secs(3))
        .map(|s| String::from_utf8_lossy(&s).into_owned());
    let yes = text.as_deref().is_some_and(|s| advertises(s, flag));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(
            key,
            Help {
                file,
                checked: Instant::now(),
                text,
            },
        );
    }
    yes
}

pub(crate) fn local(spec: Option<&ResumeSpec>, argv: &[String]) -> Vec<String> {
    match flag(spec, argv).filter(|flag| supported(&argv[0], flag)) {
        Some(flag) => with_flag(argv, flag),
        None => argv.to_vec(),
    }
}

/// The decision is made on the machine where the CLI runs. A local install's
/// options cannot answer for the version installed on a server or MicroVM.
pub(crate) fn far(
    spec: Option<&ResumeSpec>,
    argv: &[String],
    plain: String,
    make: impl FnOnce(&[String]) -> String,
) -> String {
    let Some(flag) = flag(spec, argv) else {
        return plain;
    };
    let program = crate::ssh::sh_quote(&argv[0]);
    let pattern = crate::ssh::sh_quote(&format!("(^|[[:space:],]){flag}([[:space:],=]|$)"));
    let own = make(&with_flag(argv, flag));
    format!("if {program} --help 2>/dev/null | grep -Eq -- {pattern}; then {own}; else {plain}; fi")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> ResumeSpec {
        ResumeSpec {
            process_flag: Some("--no-daemon".into()),
            process_unless: vec!["--remote".into()],
            ..Default::default()
        }
    }
    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| (*s).into()).collect()
    }

    #[test]
    fn only_an_advertised_whole_option_is_supported() {
        assert!(advertises(
            "Options:\n      --no-daemon\n          Use a private process",
            "--no-daemon"
        ));
        assert!(!advertises("--no-daemonize  --daemon", "--no-daemon"));
        assert!(!advertises("Usage: old-cli [OPTIONS]", "--no-daemon"));
    }

    #[test]
    fn explicit_connections_and_flags_are_kept() {
        for args in [
            argv(&["cli", "--no-daemon"]),
            argv(&["cli", "--remote", "unix://"]),
            argv(&["cli", "--remote=unix://"]),
        ] {
            assert_eq!(local(Some(&spec()), &args), args);
        }
        let bad = ResumeSpec {
            process_flag: Some("--flag;echo broken".into()),
            ..Default::default()
        };
        assert_eq!(flag(Some(&bad), &argv(&["cli"])), None);
        assert_eq!(
            flag(Some(&spec()), &argv(&["cli", "--", "--no-daemon"])),
            Some("--no-daemon")
        );
    }

    #[test]
    fn a_remote_launch_checks_there_and_quotes_the_program() {
        let args = argv(&["/a path/cli", "resume", "the-id", "a prompt"]);
        let line = far(
            Some(&spec()),
            &args,
            crate::worktree::as_written(&args),
            crate::worktree::as_written,
        );
        assert!(line.starts_with("if '/a path/cli' --help"), "{line}");
        assert!(line.contains("then '/a path/cli' --no-daemon resume the-id 'a prompt'; else '/a path/cli' resume the-id 'a prompt'; fi"), "{line}");
    }
}
