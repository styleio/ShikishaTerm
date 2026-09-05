//! What this machine already offers to open a tab on.
//!
//! The settings screen used to ask people to type these: a distribution name
//! into a box whose placeholder said "Ubuntu", a host name into a box next to
//! it. Both are already written down on the machine -- `wsl.exe` knows what is
//! installed and `~/.ssh/config` knows what has been connected to -- and a name
//! typed from memory is a tab that fails to start for a reason nobody can see
//! in the command line, because one letter is wrong.
//!
//! So this asks the machine instead. Nothing here is authoritative: a list that
//! comes back empty is not an error and never blocks anything. It fills a
//! suggestion list; typing is still allowed, for the distribution installed a
//! minute ago and the host that lives only in somebody's head.

use std::io::Read as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long `wsl.exe` gets before we stop waiting for it.
///
/// It can hang: a distribution mid-shutdown, a virtual machine platform that
/// is still starting. This runs while somebody is looking at a settings
/// screen, so the answer is bounded and the screen simply shows one list
/// shorter rather than freezing.
const WSL_TIMEOUT: Duration = Duration::from_millis(1500);

/// Distributions that exist to serve other software and are not for people to
/// open a shell in. Windows Terminal hides the same two.
const SERVICE_DISTROS: [&str; 2] = ["docker-desktop", "rancher-desktop"];

/// Biggest `~/.ssh/config` we will read, in bytes, per file. Generous for a
/// hand-written file; the point is only that a broken symlink to something
/// enormous cannot be read into memory here.
const SSH_CONFIG_MAX: u64 = 1_000_000;

/// How deep `Include` may go. Loops are the thing being stopped, and no real
/// configuration nests further than this.
const SSH_INCLUDE_DEPTH: usize = 4;

/// The installed WSL distributions, in the order `wsl.exe` lists them.
///
/// Empty means "nothing to suggest", and it stays that way whether WSL is
/// absent, broken, or merely slow -- none of which is this screen's business.
pub fn wsl_distros() -> Vec<String> {
    let Some(out) = run_briefly("wsl.exe", &["-l", "-q"], WSL_TIMEOUT) else {
        return Vec::new();
    };
    decode_utf16_or_utf8(&out)
        .lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\0'))
        .filter(|l| !l.is_empty())
        .filter(|l| {
            !SERVICE_DISTROS
                .iter()
                .any(|s| l.len() >= s.len() && l[..s.len()].eq_ignore_ascii_case(s))
        })
        .map(str::to_string)
        .collect()
}

/// Host aliases from `~/.ssh/config`, in the order they are written.
///
/// Only names somebody could actually connect to: a pattern (`*`, `?`) names a
/// rule rather than a host, and a negation (`!host`) exists to exclude one.
pub fn ssh_hosts() -> Vec<String> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    read_ssh_config(&home.join(".ssh").join("config"), &home, 0, &mut out);
    out.dedup();
    out
}

/// The code pages on which a Windows console still speaks something other than
/// Unicode, and the name of the encoding each one means.
///
/// This is the list of places where "it is not UTF-8" has an obvious next
/// answer. Everywhere else, output that is not UTF-8 is a program's own doing
/// and there is nothing sensible to suggest.
const LEGACY_CODE_PAGES: [(u32, &str); 5] = [
    (932, "Shift_JIS"),
    (936, "GBK"),
    (949, "EUC-KR"),
    (950, "Big5"),
    (1361, "Johab"),
];

/// The distributions installed here, learned once, off the drawing path.
///
/// [`is_wsl_distro`] is asked from the thread that reads a tab's output, and
/// that thread must never wait on a child process -- `wsl.exe` can take a
/// second and a half to answer, and every character in every tab would wait
/// with it. So the list is fetched once, in the background, and read after.
static KNOWN_DISTROS: OnceLock<Vec<String>> = OnceLock::new();

/// Start learning what is installed. Costs one short-lived thread, once.
pub fn learn_wsl_distros() {
    if KNOWN_DISTROS.get().is_some() {
        return;
    }
    std::thread::spawn(|| {
        let _ = KNOWN_DISTROS.set(wsl_distros());
    });
}

/// Is this the name of a WSL distribution on this machine?
///
/// False while the answer is still being fetched, and false on a machine with
/// no WSL -- both of which are the safe way to be wrong here. The question is
/// only ever asked about a name that arrived from a program, and treating an
/// unknown name as a local distribution would mean pointing a folder at
/// `\\wsl.localhost\<whatever a program said>`.
pub fn is_wsl_distro(name: &str) -> bool {
    KNOWN_DISTROS
        .get()
        .is_some_and(|list| list.iter().any(|d| d.eq_ignore_ascii_case(name)))
}

/// What this machine's console speaks when it is not speaking Unicode.
///
/// A Japanese Windows answers 932, and a program on it that writes its own
/// bytes rather than going through the console API writes Shift_JIS. That is
/// the one case where a terminal can say something more useful than "those
/// characters are wrong": it can name the encoding to switch to.
pub fn legacy_console_encoding() -> Option<(&'static str, u32)> {
    let cp = unsafe { windows_sys::Win32::Globalization::GetOEMCP() };
    let ansi = unsafe { windows_sys::Win32::Globalization::GetACP() };
    LEGACY_CODE_PAGES
        .iter()
        .find(|(n, _)| *n == cp || *n == ansi)
        .map(|(n, label)| (*label, *n))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn read_ssh_config(path: &std::path::Path, home: &std::path::Path, depth: usize, out: &mut Vec<String>) {
    if depth > SSH_INCLUDE_DEPTH {
        return;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if !meta.is_file() || meta.len() > SSH_CONFIG_MAX {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (keyword, rest) in text.lines().filter_map(directive) {
        match keyword.as_str() {
            "host" => out.extend(rest.split_whitespace().filter(|a| is_alias(a)).map(str::to_string)),
            // Include takes patterns, and expanding those means globbing. The
            // common shapes -- a plain name, and the `~/.ssh/config.d/*` a
            // person is told to write by every guide -- are worth following;
            // anything cleverer is left alone rather than half-understood.
            "include" => {
                for arg in rest.split_whitespace() {
                    for f in included_files(arg, home) {
                        read_ssh_config(&f, home, depth + 1, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// One configuration line as `(lowercased keyword, the rest)`.
///
/// ssh accepts `Host x`, `Host=x` and any amount of leading space, and treats
/// `#` as a comment wherever it appears.
fn directive(line: &str) -> Option<(String, &str)> {
    let line = line.split('#').next()?.trim();
    if line.is_empty() {
        return None;
    }
    let (k, rest) = match line.find(['=', ' ', '\t']) {
        Some(i) => (&line[..i], line[i + 1..].trim_start_matches(['=', ' ', '\t'])),
        None => (line, ""),
    };
    Some((k.to_ascii_lowercase(), rest))
}

fn is_alias(a: &str) -> bool {
    !a.is_empty() && !a.starts_with('!') && !a.contains('*') && !a.contains('?')
}

fn included_files(arg: &str, home: &std::path::Path) -> Vec<PathBuf> {
    let arg = arg.trim_matches('"');
    // ssh reads a bare relative path as relative to ~/.ssh
    let expanded = match arg.strip_prefix("~/").or_else(|| arg.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None => match std::path::Path::new(arg).is_absolute() {
            true => PathBuf::from(arg),
            false => home.join(".ssh").join(arg),
        },
    };
    let Some(name) = expanded.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    if !name.contains('*') {
        return vec![expanded];
    }
    let Some(dir) = expanded.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let (head, tail) = name.split_once('*').unwrap_or((name, ""));
    let mut hits: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.len() >= head.len() + tail.len() && n.starts_with(head) && n.ends_with(tail))
        })
        .collect();
    // read_dir order is the file system's; ssh reads a glob in sorted order and
    // so does this, or the same machine answers differently on two runs
    hits.sort();
    hits
}

/// Run a console program, take its output, and give up after `limit`.
///
/// Two things here are not optional on Windows. The output is drained on its
/// own thread, because a program that fills the pipe while we wait for it to
/// exit waits for us -- and neither ever moves again. And the window is
/// suppressed: this program has no console of its own, so a child console
/// would flash a black rectangle over whatever the person is reading.
fn run_briefly(exe: &str, args: &[&str], limit: Duration) -> Option<Vec<u8>> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::detach_console(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // The exit code is the part that says whether the words are a list or an
    // apology. `wsl.exe` exists on every Windows -- on a machine with no WSL
    // installed it is a stub that prints "install it first" and exits 1 -- so
    // reading its output without reading its code would offer that sentence as
    // the name of a distribution.
    if !status.success() {
        return None;
    }
    rx.recv_timeout(Duration::from_millis(200)).ok()
}

/// `wsl.exe` answers in UTF-16LE. Anything else that ends up here is read as
/// UTF-8, so this stays usable for a program that answers plainly.
fn decode_utf16_or_utf8(raw: &[u8]) -> String {
    let looks_utf16 = raw.len() >= 2
        && raw.len() % 2 == 0
        && (raw[..2] == [0xFF, 0xFE] || raw.iter().skip(1).step_by(2).take(8).any(|b| *b == 0));
    if !looks_utf16 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let mut units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    if units.first() == Some(&0xFEFF) {
        units.remove(0);
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_speaks_utf16_and_we_listen() {
        let utf16: Vec<u8> = "Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode_utf16_or_utf8(&utf16), "Ubuntu\r\nDebian\r\n");
        // With the byte order mark wsl sometimes writes
        let mut bom = vec![0xFF, 0xFE];
        bom.extend_from_slice(&utf16);
        assert_eq!(decode_utf16_or_utf8(&bom), "Ubuntu\r\nDebian\r\n");
        // Plain bytes are still plain bytes
        assert_eq!(decode_utf16_or_utf8(b"Ubuntu\n"), "Ubuntu\n");
        assert_eq!(decode_utf16_or_utf8(b""), "");
    }

    #[test]
    fn a_configuration_line_is_read_the_way_ssh_reads_it() {
        assert_eq!(directive("Host web"), Some(("host".into(), "web")));
        assert_eq!(directive("  Host=web  "), Some(("host".into(), "web")));
        assert_eq!(directive("HOST\tweb prod"), Some(("host".into(), "web prod")));
        assert_eq!(directive("Host web # the one"), Some(("host".into(), "web")));
        assert_eq!(directive("# Host web"), None);
        assert_eq!(directive("   "), None);
    }

    #[test]
    fn only_names_a_person_could_connect_to() {
        assert!(is_alias("web"));
        assert!(is_alias("10.0.0.2"));
        assert!(!is_alias("*"), "パターンはホスト名ではない");
        assert!(!is_alias("*.example.com"));
        assert!(!is_alias("web?"));
        assert!(!is_alias("!excluded"));
        assert!(!is_alias(""));
    }

    #[test]
    fn hosts_come_out_of_a_real_config_in_order() {
        let dir = std::env::temp_dir().join(format!("shikisha-ssh-{}", std::process::id()));
        let ssh = dir.join(".ssh");
        let _ = std::fs::create_dir_all(ssh.join("config.d"));
        std::fs::write(
            ssh.join("config"),
            "# comment\nHost web prod\n  User me\nHost *\n  ForwardAgent yes\n\
             Host !secret\nInclude config.d/*.conf\nHost web\n",
        )
        .unwrap();
        std::fs::write(ssh.join("config.d").join("extra.conf"), "Host db\n").unwrap();
        std::fs::write(ssh.join("config.d").join("notes.txt"), "Host nope\n").unwrap();

        let mut out = Vec::new();
        read_ssh_config(&ssh.join("config"), &dir, 0, &mut out);
        assert_eq!(out, vec!["web", "prod", "db", "web"], "順番も込みで {out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whatever this machine has, asking must not throw and must not hang.
    /// (This one is a smoke test on purpose: a build machine has no WSL and no
    /// ssh config, and neither absence is a failure.)
    #[test]
    fn asking_the_machine_is_always_safe() {
        let began = Instant::now();
        let _ = wsl_distros();
        let _ = ssh_hosts();
        assert!(began.elapsed() < Duration::from_secs(5), "答えは有界");
    }
}
