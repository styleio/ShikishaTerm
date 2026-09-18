//! Whether git will touch a folder at all.
//!
//! Git refuses to work in a repository whose files belong to an account other
//! than the one running it -- "dubious ownership" -- and stops before it does
//! anything. A project kept on another machine's share is exactly that case:
//! the share hands out an owner of its own, so every git command in the
//! folder ends in the same refusal, ours and the person's alike.
//!
//! The way past it is a line in the person's own git settings naming the
//! folder as one they trust, and that line is theirs to decide on: git keeps
//! it deliberately out of reach of the repository itself, so that a folder
//! cannot vouch for itself. So this only finds out that it is needed, says
//! what would be written and where, and writes it when it is told to.
//!
//! What is offered is what git asked for. Git's refusal ends with the exact
//! command it wants -- the folder already spelled the way git spells it -- so
//! the offer is read out of git's own words rather than assembled here from a
//! guess about which spelling it would accept.

use std::path::{Path, PathBuf};

/// Git's own suggestion, taken out of what it said.
///
/// The refusal ends with the line it wants run, and only the folder in it
/// varies:
///
/// ```text
///     git config --global --add safe.directory '%(prefix)///192.168.0.35/projects/php7/te0_main'
/// ```
///
/// The words around it are translated when git speaks another language; the
/// setting's own name is not, which is what this looks for
pub fn asked_for(said: &str) -> Option<String> {
    let line = said.lines().find(|l| l.contains("safe.directory") && l.contains("git config"))?;
    let (_, value) = line.split_once("safe.directory")?;
    let value = value.trim();
    let value = unquoted(value, '\'').unwrap_or_else(|| unquoted(value, '"').unwrap_or(value));
    (!value.is_empty()).then(|| value.to_string())
}

fn unquoted(s: &str, q: char) -> Option<&str> {
    s.strip_prefix(q)?.strip_suffix(q)
}

/// Whether git refuses this folder, and the folder it wants written down.
///
/// Asked of git rather than worked out from who owns the files: the check is
/// git's own, its rules are git's own, and an answer of "it works" here that
/// git disagrees with would be worse than no answer at all
pub fn refused(folder: &Path) -> Option<String> {
    let mut ask = std::process::Command::new("git");
    ask.arg("-C").arg(folder).args(["rev-parse", "--git-dir"]);
    let out = crate::detach_console(&mut ask).output().ok()?;
    if out.status.success() {
        return None;
    }
    asked_for(&String::from_utf8_lossy(&out.stderr))
}

/// One line for every branch this app will ever cut, rather than one per
/// branch.
///
/// Git reads `/*` after a folder as everything below it, however deep, so the
/// place branches are made can be named once and answer for all of them. Only
/// that place: anywhere else, what git asked for is what is offered, because
/// widening somebody's settings beyond the folder in front of them is not
/// something an app should do quietly
pub fn spread(value: &str) -> String {
    let root = as_git_spells(&crate::worktree::branches_root());
    // A root of one part (`C:/`) would put every repository on the drive
    // inside it; the real one is several folders deep
    if root.matches('/').count() < 2 {
        return value.to_string();
    }
    let under = format!("{root}/");
    match same_start(value, &under) {
        true => format!("{root}/*"),
        false => value.to_string(),
    }
}

/// Whether a folder git named is inside the place branches are made. Case is
/// not what tells two Windows paths apart
fn same_start(value: &str, under: &str) -> bool {
    // By bytes rather than by characters, and asked for rather than cut out: a
    // path with a name in it that is not ASCII has no boundary there to cut on
    value.len() > under.len() && value.get(..under.len()).is_some_and(|head| head.eq_ignore_ascii_case(under))
}

/// A path the way git writes one: forward slashes, and a folder on another
/// machine keeps the two slashes that name it, written as git's own settings
/// write them
pub fn as_git_spells(p: &Path) -> String {
    let said = p.display().to_string().replace('\\', "/");
    match said.strip_prefix("//") {
        Some(rest) => format!("%(prefix)///{rest}"),
        None => said,
    }
}

/// The command that adds the line: what is shown, and what runs.
pub fn argv(value: &str) -> Vec<String> {
    vec![
        "git".into(),
        "config".into(),
        "--global".into(),
        "--add".into(),
        "safe.directory".into(),
        value.into(),
    ]
}

/// The line as it lands in the file, so that what is approved is what appears
/// there.
pub fn line(value: &str) -> String {
    format!("[safe]\n\tdirectory = {value}")
}

/// The file the line lands in.
///
/// Asked of git first, because git is the one that will read it. A settings
/// file with nothing in it yet has nothing to say about where it is, so the
/// places git looks are followed by hand for that one case.
///
/// Worked out once: the answer costs a git, the row that says it is drawn
/// sixty times a second, and where a person's git settings live does not move
/// while the program is running
pub fn file() -> PathBuf {
    static FILE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FILE.get_or_init(found_file).clone()
}

fn found_file() -> PathBuf {
    if let Some(said) = told_by_git() {
        return PathBuf::from(said);
    }
    if let Some(set) = std::env::var_os("GIT_CONFIG_GLOBAL").filter(|v| !v.is_empty()) {
        return PathBuf::from(set);
    }
    let home = home();
    let plain = home.join(".gitconfig");
    if plain.exists() {
        return plain;
    }
    // Git's other place, used only when it is the one that is there
    let xdg = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(x) => PathBuf::from(x).join("git").join("config"),
        None => home.join(".config").join("git").join("config"),
    };
    match xdg.exists() {
        true => xdg,
        false => plain,
    }
}

/// Where git says its own settings are, when it has any to show.
fn told_by_git() -> Option<String> {
    let mut ask = std::process::Command::new("git");
    ask.args(["config", "--global", "--list", "--show-origin", "--name-only"]);
    let out = crate::detach_console(&mut ask).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let said = String::from_utf8_lossy(&out.stdout).to_string();
    let first = said.lines().find_map(|l| l.strip_prefix("file:"))?;
    let path = unwrapped(first.split('\t').next()?.trim());
    (!path.is_empty()).then_some(path)
}

/// The path out of the way git writes one there.
///
/// Written plainly when nothing in it needs saying twice, and in quotes with
/// its backslashes doubled when something does -- which on Windows is any path
/// git was handed with backslashes in it
fn unwrapped(said: &str) -> String {
    let Some(inner) = unquoted(said, '"') else { return said.to_string() };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c == '\\' {
            true => out.extend(chars.next()),
            false => out.push(c),
        }
    }
    out
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Whether this exact folder is already written down, so that saying yes twice
/// does not leave the same line in the settings twice.
pub fn listed(value: &str) -> bool {
    let mut ask = std::process::Command::new("git");
    ask.args(["config", "--global", "--get-all", "safe.directory"]);
    let Ok(out) = crate::detach_console(&mut ask).output() else { return false };
    String::from_utf8_lossy(&out.stdout).lines().any(|l| l.trim() == value)
}

/// Adds the line. The same `argv` that was shown is the one that runs.
pub fn add(value: &str) -> anyhow::Result<()> {
    if listed(value) {
        return Ok(());
    }
    crate::worktree::run(&argv(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Word for word what git for Windows says about a project on a share,
    /// with the person's own settings out of the way (2026-09-18).
    const REFUSED: &str = "fatal: detected dubious ownership in repository at '//192.168.0.35/projects/php7/te0_main'\n\
'//192.168.0.35/projects/php7/te0_main' is owned by:\n\t(inconvertible) (S-1-5-21-2962977560-3957723904-1976646925-1000)\n\
but the current user is:\n\tNPC/style (S-1-5-21-3345742566-3279784867-2879849460-1001)\n\
To add an exception for this directory, call:\n\n\tgit config --global --add safe.directory '%(prefix)///192.168.0.35/projects/php7/te0_main'\n";

    #[test]
    fn what_git_asked_for_is_what_is_offered() {
        assert_eq!(
            asked_for(REFUSED).as_deref(),
            Some("%(prefix)///192.168.0.35/projects/php7/te0_main"),
        );
        // A branch's own folder, which is the second place git stops: the
        // folder is here, the git folder it belongs to is on the share
        let cut = "fatal: detected dubious ownership in repository at 'C:/Users/me/SHIKISHA-TERM/branches/te0_main/x'\n\
To add an exception for this directory, call:\n\n\tgit config --global --add safe.directory 'C:/Users/me/SHIKISHA-TERM/branches/te0_main/x'\n";
        assert_eq!(
            asked_for(cut).as_deref(),
            Some("C:/Users/me/SHIKISHA-TERM/branches/te0_main/x"),
        );
        // Without the quotes, which is how git writes a path with nothing in
        // it that would need them on a shell
        assert_eq!(
            asked_for("\tgit config --global --add safe.directory D:/work/proj").as_deref(),
            Some("D:/work/proj"),
        );
    }

    #[test]
    fn anything_that_is_not_that_refusal_asks_for_nothing() {
        assert_eq!(asked_for(""), None);
        assert_eq!(asked_for("fatal: not a git repository"), None);
        // The setting's name alone is not an instruction to write it down
        assert_eq!(asked_for("hint: see safe.directory in git-config(1)"), None);
    }

    #[test]
    fn a_branch_this_app_made_is_covered_by_one_line() {
        let root = as_git_spells(&crate::worktree::branches_root());
        let cut = format!("{root}/te0_main/tough-arachnid");
        assert_eq!(spread(&cut), format!("{root}/*"), "every branch would need its own line");
        // Spelled the other way by git, it is still the same place
        assert_eq!(spread(&cut.to_uppercase()), format!("{root}/*"));
        // The project itself is not in there, and is written down as itself
        let far = "%(prefix)///192.168.0.35/projects/php7/te0_main";
        assert_eq!(spread(far), far);
        assert_eq!(spread("D:/work/proj"), "D:/work/proj");
        // The place itself, not something inside it
        assert_eq!(spread(&root), root);
    }

    #[test]
    fn a_folder_on_another_machine_is_written_the_way_git_writes_one() {
        assert_eq!(
            as_git_spells(Path::new(r"\\192.168.0.35\projects\php7\te0_main")),
            "%(prefix)///192.168.0.35/projects/php7/te0_main",
        );
        assert_eq!(as_git_spells(Path::new(r"D:\work\proj")), "D:/work/proj");
    }

    #[test]
    fn the_command_and_the_line_say_the_same_thing() {
        let v = "C:/Users/me/SHIKISHA-TERM/branches/*";
        assert_eq!(argv(v).last().map(String::as_str), Some(v));
        assert!(line(v).ends_with(v), "{}", line(v));
        assert!(line(v).starts_with("[safe]"), "{}", line(v));
    }

    /// Both shapes git prints, taken from real runs (2026-09-18): plain when
    /// the path has nothing in it to escape, and in quotes with its
    /// backslashes doubled when it has. Read the second one raw, the dialog
    /// said the line went into a file whose name nobody has
    #[test]
    fn the_file_is_read_however_git_spells_it() {
        assert_eq!(unwrapped("C:/Users/style/.gitconfig"), "C:/Users/style/.gitconfig");
        assert_eq!(
            unwrapped(r#""C:\\Users\\style\\AppData\\Local\\Temp\\scratch-gitconfig""#),
            r"C:\Users\style\AppData\Local\Temp\scratch-gitconfig",
        );
        assert_eq!(unwrapped(r#""a \"quoted\" name""#), "a \"quoted\" name");
    }

    #[test]
    fn git_says_where_its_own_settings_are() {
        // Whatever the answer, it is a file inside a folder -- never empty,
        // which would leave the dialog saying a line goes nowhere
        let f = file();
        assert!(f.parent().is_some_and(|p| !p.as_os_str().is_empty()), "{f:?}");
        assert!(f.file_name().is_some(), "{f:?}");
    }

    #[test]
    fn a_folder_git_is_happy_with_is_not_asked_about() {
        // This very checkout: whatever owns it, git works here, and nothing is
        // offered for a folder nothing is wrong with
        let here = std::env::current_dir().unwrap();
        assert_eq!(refused(&here), None);
    }
}
