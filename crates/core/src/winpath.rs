//! Turning what a shell says about where it is into a path this machine can use.
//!
//! A shell that has been set up for it announces its working directory as it
//! moves: `OSC 7` carries a `file://` URI, and ConEmu's `OSC 9;9` carries a
//! plain path. That is the only way to know where a tab is actually working --
//! the folder it was launched in is where it started, not where it is now, and
//! neither `cd` nor a program that changes directory tells us anything.
//!
//! Two things make the answer more than a string copy. It arrives as a URI, so
//! it is percent-encoded and carries the name of a machine. And under WSL it
//! arrives as a path in another operating system: `/mnt/d/work` is this
//! machine's `D:\work`, while `/home/me` is not on this machine at all -- it is
//! inside a distribution, reachable only through `\\wsl.localhost\`.

/// What a shell said about where it is.
///
/// The distinction that matters is whose machine the path belongs to. A shell
/// at the far end of an ssh session announces its directory exactly as eagerly
/// as one running here, and `/srv/app` on a build server is not a folder on
/// this disk. So the name it came with is kept, and whether that name is one of
/// this machine's WSL distributions is answered by [`crate::discover`], which
/// knows what is installed. Nothing in this module asks the machine anything,
/// which is what lets all of it be tested anywhere.
#[derive(Debug, PartialEq, Eq)]
pub enum Said {
    /// A path this machine can open exactly as it stands
    Here(String),
    /// A POSIX path, and the name of the host that claimed it. Usable only if
    /// that name turns out to be a WSL distribution on this machine
    Somewhere { host: String, path: String },
}

/// Where a shell says it is. `payload` is everything after `OSC 7;`.
pub fn from_osc7(payload: &str) -> Option<Said> {
    let payload = payload.trim();
    if payload.is_empty() {
        return None;
    }
    // A bare path (ConEmu's spelling, and some shells' idea of OSC 7)
    let Some(rest) = payload.strip_prefix("file://") else {
        return here(payload);
    };
    // file://<host>/<path>. The host is the machine the path belongs to -- and
    // under WSL it is the distribution's name, which is exactly what is needed
    // to reach the path from the Windows side.
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = percent_decode(host);
    let path = percent_decode(path);
    let local = host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case(&hostname());
    if let Some(said) = here(&path) {
        // A drive letter reads the same from anywhere and means something
        // different: another machine's D: is not this one's, and there is no
        // way to reach it from here. Ours is a path; theirs is nothing.
        return local.then_some(said);
    }
    if local || !path.starts_with('/') {
        return None;
    }
    Some(Said::Somewhere { host, path })
}

/// A path already in this machine's own terms, or nothing.
fn here(path: &str) -> Option<Said> {
    // "/C:/Users/me" -- a Windows path wearing the URI's leading slash
    let path = match drive_at(path.strip_prefix('/').unwrap_or("")) {
        true => &path[1..],
        false => path,
    };
    if drive_at(path) {
        return Some(Said::Here(path.replace('/', "\\")));
    }
    if path.starts_with("\\\\") {
        return Some(Said::Here(path.to_string()));
    }
    None
}

/// The Windows path for a directory inside one of this machine's WSL
/// distributions. The caller has already established that `distro` is one.
///
/// `/mnt/<letter>` is this machine's own disk seen from inside, and naming it
/// as the drive it is beats reaching it back through the network path: the
/// same folder, without the round trip.
pub fn in_distro(distro: &str, path: &str) -> Option<String> {
    if distro.is_empty() || distro.contains(['\\', '/']) || !path.starts_with('/') {
        return None;
    }
    if let Some(rest) = path.strip_prefix("/mnt/") {
        let mut parts = rest.splitn(2, '/');
        if let Some(letter) = parts.next().filter(|l| l.len() == 1) {
            let c = letter.chars().next()?;
            if c.is_ascii_alphabetic() {
                let tail = parts.next().unwrap_or("").replace('/', "\\");
                return Some(format!("{}:\\{}", c.to_ascii_uppercase(), tail));
            }
        }
    }
    Some(format!(
        "\\\\wsl.localhost\\{}{}",
        distro,
        path.replace('/', "\\")
    ))
}

/// "C:/..." or "C:\..." -- and not "C" alone, which is a folder name.
fn drive_at(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// %XX back into bytes, then read as UTF-8.
///
/// Anything that is not a complete escape is left as the characters it is: a
/// path with a bare `%` in its name is a real path, and refusing it would be
/// worse than passing it through.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1), b.get(i + 2)) {
            (b'%', Some(h), Some(l)) => match (hex(*h), hex(*l)) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 3;
                }
                _ => {
                    out.push(b[i]);
                    i += 1;
                }
            },
            _ => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here_str(payload: &str) -> Option<String> {
        match from_osc7(payload) {
            Some(Said::Here(p)) => Some(p),
            _ => None,
        }
    }

    #[test]
    fn a_windows_shell_says_where_it_is() {
        assert_eq!(
            here_str("file:///C:/Users/me/work"),
            Some(r"C:\Users\me\work".into())
        );
        // The host is this machine's name, spelled however it feels like
        assert_eq!(here_str("file://localhost/D:/proj"), Some(r"D:\proj".into()));
        // Spaces and Japanese survive the URI
        assert_eq!(
            here_str("file:///C:/Users/me/My%20Work"),
            Some(r"C:\Users\me\My Work".into())
        );
        assert_eq!(
            here_str("file:///D:/%E4%BB%95%E4%BA%8B"),
            Some(r"D:\仕事".into())
        );
        // A network path stays one
        assert_eq!(
            here_str(r"\\server\share\x"),
            Some(r"\\server\share\x".into())
        );
    }

    /// The name in a `file://` URI is a machine, and the only ones we can open
    /// are this machine's own WSL distributions. Which of those it is, this
    /// module does not decide -- it hands the name back for someone who knows.
    #[test]
    fn a_path_from_elsewhere_keeps_the_name_it_came_with() {
        assert_eq!(
            from_osc7("file://Ubuntu/mnt/d/work"),
            Some(Said::Somewhere {
                host: "Ubuntu".into(),
                path: "/mnt/d/work".into()
            })
        );
        assert_eq!(
            from_osc7("file://build-server/srv/app"),
            Some(Said::Somewhere {
                host: "build-server".into(),
                path: "/srv/app".into()
            })
        );
        // A drive letter announced by another machine is that machine's drive
        assert_eq!(from_osc7("file://build-server/D:/proj"), None);
        // Without a name there is nothing to resolve against: "/home/me" is a
        // folder on every Linux there has ever been
        assert_eq!(from_osc7("file:///home/me/src"), None);
        assert_eq!(from_osc7("/home/me/src"), None);
    }

    #[test]
    fn inside_a_distribution_the_drives_are_still_ours() {
        assert_eq!(
            in_distro("Ubuntu", "/mnt/d/work/proj"),
            Some(r"D:\work\proj".into())
        );
        assert_eq!(in_distro("Ubuntu", "/mnt/c"), Some(r"C:\".into()));
        // Everything else lives in the distribution
        assert_eq!(
            in_distro("Ubuntu-24.04", "/home/me/src"),
            Some(r"\\wsl.localhost\Ubuntu-24.04\home\me\src".into())
        );
        // A distribution name is one path element, never a route of its own
        assert_eq!(in_distro("a/../b", "/home/me"), None);
        assert_eq!(in_distro(r"a\b", "/home/me"), None);
        assert_eq!(in_distro("", "/home/me"), None);
        assert_eq!(in_distro("Ubuntu", "relative/path"), None);
    }

    #[test]
    fn what_cannot_be_turned_into_a_path_is_refused() {
        assert_eq!(from_osc7(""), None);
        assert_eq!(from_osc7("   "), None);
        assert_eq!(from_osc7("file://"), None);
        assert_eq!(from_osc7("nonsense"), None);
    }

    #[test]
    fn a_half_written_escape_is_left_alone() {
        assert_eq!(percent_decode("a%"), "a%");
        assert_eq!(percent_decode("a%zz"), "a%zz");
        assert_eq!(percent_decode("100%25"), "100%");
        assert_eq!(percent_decode(""), "");
    }

    #[test]
    fn a_drive_letter_is_two_characters_and_a_colon() {
        assert!(drive_at(r"C:\x"));
        assert!(drive_at("c:/x"));
        assert!(!drive_at("C"));
        assert!(!drive_at("/C:/x"), "先頭のスラッシュは剥がしてから");
        assert!(!drive_at("home/me"));
    }
}
