//! Files pages sent to be saved.
//!
//! Every browser that draws a page -- the window's WebView2, the server's
//! headless Chromium, the browser of somebody connected -- reports what it
//! saved as an `Ev::Download`, and this is where those reports become one list:
//! the one a person reads in the column beside the page, a script reads with
//! `shikisha.downloads()`, and a phone saves a file from.
//!
//! Nothing here is written to disk. The files are where the browser put them
//! and stay there; the list is what this run saw arrive, the way a browser's
//! own download bubble is. Somebody looking for an older file opens the folder.

use shikisha_shared::{Download, DownloadState};

/// The most lines kept. The oldest finished one goes first; one still going is
/// never dropped, since it is the line its Cancel is on
pub const KEPT: usize = 100;

/// The reasons a download stops by itself, as the short keys `Download::why`
/// carries. Each has its sentence under `msg.download.why.<key>`
pub const WHY: [&str; 7] = ["network", "server", "disk", "denied", "blocked", "toolarge", "unknown"];

/// Where this machine keeps downloads, for a browser that has to be told
/// (the server's) and for "open the folder" before anything was saved.
///
/// The window's WebView2 is not told: it saves where Windows says downloads
/// go, and that is the same folder this finds.
///
/// `SHIKISHA_DOWNLOADS` names another, for a server whose account has no
/// Downloads folder anybody looks in -- and for a test, which must not fill
/// the Downloads folder of whoever runs it
pub fn folder() -> std::path::PathBuf {
    if let Some(told) = std::env::var_os("SHIKISHA_DOWNLOADS").filter(|d| !d.is_empty()) {
        return std::path::PathBuf::from(told);
    }
    #[cfg(windows)]
    {
        // What Explorer calls Downloads, wherever the person moved it to
        let shell = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders");
        if let Ok(key) = shell
            && let Ok(at) = key.get_value::<String, _>("{374DE290-123F-4565-9164-39C4925E467B}")
            && !at.trim().is_empty()
        {
            return std::path::PathBuf::from(at);
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(at) = std::env::var_os("XDG_DOWNLOAD_DIR").filter(|d| !d.is_empty()) {
            return std::path::PathBuf::from(at);
        }
    }
    let home = crate::home_dir().unwrap_or_else(std::env::temp_dir);
    home.join("Downloads")
}

/// A name a server suggested, as one file name and nothing more: no folder in
/// it, nothing this file system refuses, never empty
pub fn safe_name(suggested: &str) -> String {
    let leaf = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let kept: String = leaf
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    // Windows drops a trailing dot or space without saying so, and a name of
    // only dots is a folder of its own
    let kept = kept.trim().trim_end_matches(['.', ' ']).to_string();
    if kept.is_empty() || kept.chars().all(|c| c == '.') { "download".to_string() } else { kept }
}

/// The place in `dir` a file called `name` can go without replacing anything:
/// `name.ext`, then `name (1).ext`, `name (2).ext`... -- what browsers do.
///
/// The place is taken as it is found (an empty file is made there), so two
/// downloads of the same name finishing together cannot both be handed it.
/// `None` when a thousand of them are taken, or the folder cannot be written
pub fn take_place(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let name = safe_name(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let _ = std::fs::create_dir_all(dir);
    for n in 0..1000 {
        let file = if n == 0 { name.clone() } else { format!("{stem} ({n}){ext}") };
        let at = dir.join(file);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&at) {
            Ok(_) => return Some(at),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Put a finished file where it was given a place by [`take_place`]. Across
/// two disks a move is a copy and a delete
pub fn move_into(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// How long one file may take to cross to another machine, or back
const CROSS_MS: u64 = 10 * 60 * 1000;

/// The machine each download sent away is on, by its id: what bringing it back
/// asks -- from the window's "Save to this PC" and from a phone's "Save to this
/// device" alike, the second of which is answered by a thread that has nothing
/// but the line it was asked for. The machine is the folder's own (a folder on
/// a MicroVM is a machine of its own), not the entry its name came from
static AWAY: std::sync::Mutex<Option<std::collections::HashMap<String, crate::elsewhere::Elsewhere>>> =
    std::sync::Mutex::new(None);

/// Remember where a download was sent
pub fn remember_away(id: &str, at: crate::elsewhere::Elsewhere) {
    let mut held = AWAY.lock().unwrap_or_else(|e| e.into_inner());
    held.get_or_insert_with(Default::default).insert(id.to_string(), at);
}

/// The machine a download was sent to, when it was
pub fn away(id: &str) -> Option<crate::elsewhere::Elsewhere> {
    AWAY.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(id).cloned())
}

/// Send a whole file to the Downloads folder of another machine, numbered
/// there the way a browser numbers a second file of a name, and take it off
/// this one. Answers where it is now, as that machine writes it.
///
/// For a page of a folder on an SSH server or a MicroVM: the page is drawn by
/// this machine's browser, so the file arrives here first -- and it belongs
/// with the folder, where the AI working in it can use it. The copy here is
/// removed only once the one there is whole
pub fn send_away(at: &crate::elsewhere::Elsewhere, from: &std::path::Path) -> anyhow::Result<String> {
    use crate::ssh::FileJob;
    let home = crate::elsewhere::exec(at, "printf %s \"$HOME\"", 60_000)?;
    let home = home.out.trim().trim_end_matches('/').to_string();
    if !home.starts_with('/') {
        anyhow::bail!(crate::i18n::t("msg.download.why.denied"));
    }
    let dir = format!("{home}/Downloads");
    let _ = crate::elsewhere::files(at, FileJob::MakeDir { path: dir.clone() }, 60_000);
    let name = safe_name(&from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 0..1000 {
        let file = if n == 0 { name.clone() } else { format!("{stem} ({n}){ext}") };
        let to = format!("{dir}/{file}");
        if crate::elsewhere::files(at, FileJob::Stat { path: to.clone() }, 60_000).is_ok() {
            continue;
        }
        crate::elsewhere::files(at, FileJob::Put { from: from.to_path_buf(), to: to.clone(), overwrite: false }, CROSS_MS)?;
        let _ = std::fs::remove_file(from);
        return Ok(to);
    }
    anyhow::bail!(crate::i18n::t("msg.download.why.disk"))
}

/// Bring a file from another machine into `dir` on this one, under its own
/// name (numbered if that is taken). Answers where it landed
pub fn fetch_here(at: &crate::elsewhere::Elsewhere, there: &str, dir: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let leaf = there.rsplit('/').next().unwrap_or("download");
    let to = take_place(dir, leaf).ok_or_else(|| anyhow::anyhow!(crate::i18n::t("msg.download.why.disk")))?;
    match crate::elsewhere::files(at, crate::ssh::FileJob::Get { from: there.to_string(), to: to.clone(), overwrite: true }, CROSS_MS) {
        Ok(_) => Ok(to),
        Err(e) => {
            let _ = std::fs::remove_file(&to);
            Err(e)
        }
    }
}

/// The site an address belongs to, as a person reads it (`example.com`)
pub fn site_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    host.to_string()
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub item: Download,
    /// The tab the page that saved it is in, by key. `None` when the browser
    /// could not say which page asked (a window the page opened)
    pub page: Option<String>,
    /// When it started, in ms since the epoch
    pub began: i64,
    /// The machine the file is on, by name, when that is not the one the
    /// browser saved it on: a page of a folder on an SSH server or a MicroVM
    /// saves there (see [`send_away`]). `None` is where the browser put it
    pub machine: Option<String>,
    /// Being sent to that machine right now
    pub sending: bool,
    /// The machine it could not be sent to, by name; the file stayed where
    /// the browser put it. Empty when nothing failed
    pub unsent: String,
}

/// What a report changed, for whoever says so to the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A download nobody had heard of
    Began,
    /// More bytes, nothing else
    Moved,
    /// It reached an end it had not reached before
    Ended(DownloadState),
    /// Nothing new (a report repeated, or one about a line already ended)
    Nothing,
}

/// Every download this run has seen, newest first.
#[derive(Debug, Default)]
pub struct List {
    rows: Vec<Row>,
}

impl List {
    /// Take in one report
    pub fn note(&mut self, page: Option<String>, item: Download, now_ms: i64) -> Change {
        if let Some(row) = self.rows.iter_mut().find(|r| r.item.id == item.id) {
            // An ended line stays ended: a late progress report arriving after
            // the end must not bring back a Cancel that does nothing
            if row.item.state != DownloadState::Going {
                return Change::Nothing;
            }
            let ended = item.state != DownloadState::Going;
            let changed = row.item != item;
            // What the first report knew and a later one left out is kept
            let keep_path = item.path.is_empty() && !row.item.path.is_empty();
            let path = if keep_path { row.item.path.clone() } else { item.path.clone() };
            row.item = Download { path, ..item };
            if row.page.is_none() {
                row.page = page;
            }
            return match (ended, changed) {
                (true, _) => Change::Ended(row.item.state),
                (false, true) => Change::Moved,
                (false, false) => Change::Nothing,
            };
        }
        let state = item.state;
        self.rows.insert(0, Row { item, page, began: now_ms, machine: None, sending: false, unsent: String::new() });
        self.trim();
        match state {
            DownloadState::Going => Change::Began,
            // Over before anybody heard it start (a small file): it still began
            other => Change::Ended(other),
        }
    }

    fn trim(&mut self) {
        while self.rows.len() > KEPT {
            match self.rows.iter().rposition(|r| r.item.state != DownloadState::Going) {
                Some(i) => {
                    self.rows.remove(i);
                }
                None => break,
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.item.id == id)
    }

    /// The line goes. The file, if there is one, stays where it is
    pub fn forget(&mut self, id: &str) {
        self.rows.retain(|r| r.item.id != id || r.item.state == DownloadState::Going);
    }

    /// Every line that has ended goes; the ones still going stay
    pub fn clear_ended(&mut self) {
        self.rows.retain(|r| r.item.state == DownloadState::Going);
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The file is on its way to the machine named
    pub fn sending(&mut self, id: &str, machine: &str) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.item.id == id) {
            row.machine = Some(machine.to_string());
            row.sending = true;
        }
    }

    /// It arrived there, at `path` (as that machine writes it)
    pub fn sent(&mut self, id: &str, path: &str) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.item.id == id) {
            row.sending = false;
            row.item.path = path.to_string();
            row.item.name = path.rsplit('/').next().unwrap_or(&row.item.name).to_string();
        }
    }

    /// It could not be sent to the machine named, and stays where the browser
    /// put it
    pub fn unsent(&mut self, id: &str, machine: &str) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.item.id == id) {
            row.machine = None;
            row.sending = false;
            row.unsent = machine.to_string();
        }
    }

    /// The folder the newest file is in: where "open the folder" goes, since
    /// that is where the person was just looking for something
    pub fn newest_folder(&self) -> Option<std::path::PathBuf> {
        self.rows
            .iter()
            .filter(|r| !r.item.far && r.machine.is_none() && !r.item.path.is_empty())
            .find_map(|r| std::path::Path::new(&r.item.path).parent().map(std::path::Path::to_path_buf))
    }
}

/// What a download is called in the list and to a script
pub fn state_word(s: DownloadState) -> &'static str {
    match s {
        DownloadState::Going => "going",
        DownloadState::Done => "done",
        DownloadState::Failed => "failed",
        DownloadState::Cancelled => "cancelled",
    }
}

/// One line as the board draws it and a script reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct View {
    pub id: String,
    pub name: String,
    /// The site it came from (`example.com`)
    pub site: String,
    pub url: String,
    pub path: String,
    pub got: u64,
    pub total: u64,
    /// `going` / `sending` (whole here, on its way to `machine`) / `done` /
    /// `failed` / `cancelled`
    pub state: String,
    /// The machine the file is on, by name; empty when it is where the
    /// browser saved it
    pub machine: String,
    /// The machine its folder is on, when it could not be sent there; it
    /// stayed where the browser saved it
    pub unsent: String,
    /// What stopped a failed one, as a key of `msg.download.why.*`
    pub why: String,
    /// The tab the page that saved it is in, by key; empty when not known
    pub page: String,
    /// Saved on the device of the person connected, not on this machine
    pub far: bool,
    /// Opening it would run it (a program, a script, an installer). The list
    /// offers its folder instead, the same rule a link in a terminal follows
    pub runs: bool,
    /// When it started, in ms since the epoch
    pub began: i64,
}

impl View {
    pub fn of(r: &Row) -> Self {
        let i = &r.item;
        Self {
            id: i.id.clone(),
            name: i.name.clone(),
            site: site_of(&i.url),
            url: i.url.clone(),
            path: i.path.clone(),
            got: i.got,
            total: i.total,
            state: if r.sending { "sending".to_string() } else { state_word(i.state).to_string() },
            machine: r.machine.clone().unwrap_or_default(),
            unsent: r.unsent.clone(),
            why: i.why.clone(),
            page: r.page.clone().unwrap_or_default(),
            far: i.far,
            runs: crate::termlink::runs_when_opened(std::path::Path::new(&i.name)),
            began: r.began,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, state: DownloadState, got: u64) -> Download {
        Download {
            id: id.into(),
            name: "report.pdf".into(),
            url: "https://example.com/files/report.pdf".into(),
            path: String::new(),
            got,
            total: 100,
            state,
            why: String::new(),
            far: false,
        }
    }

    /// A download is heard beginning, moving and ending once each, and the
    /// newest is first
    #[test]
    fn a_download_begins_moves_and_ends_once() {
        let mut l = List::default();
        assert_eq!(l.note(Some("web".into()), item("a", DownloadState::Going, 0), 1), Change::Began);
        assert_eq!(l.note(None, item("a", DownloadState::Going, 50), 2), Change::Moved);
        assert_eq!(l.note(None, item("a", DownloadState::Going, 50), 3), Change::Nothing);
        assert_eq!(l.note(None, item("a", DownloadState::Done, 100), 4), Change::Ended(DownloadState::Done));
        // A progress report that arrives after the end changes nothing
        assert_eq!(l.note(None, item("a", DownloadState::Going, 60), 5), Change::Nothing);
        assert_eq!(l.get("a").unwrap().item.state, DownloadState::Done);
        assert_eq!(l.get("a").unwrap().page.as_deref(), Some("web"), "the page it came from was lost");
        l.note(None, item("b", DownloadState::Going, 0), 6);
        assert_eq!(l.rows()[0].item.id, "b", "the newest is not first");
    }

    /// Forgetting a line never takes away a download still going -- its
    /// Cancel is on that line
    #[test]
    fn a_download_still_going_is_never_forgotten() {
        let mut l = List::default();
        l.note(None, item("a", DownloadState::Going, 0), 1);
        l.note(None, item("b", DownloadState::Done, 100), 2);
        l.forget("a");
        l.clear_ended();
        assert_eq!(l.rows().len(), 1);
        assert_eq!(l.rows()[0].item.id, "a");
    }

    /// The list stops growing at KEPT, dropping the oldest finished line
    #[test]
    fn the_list_drops_its_oldest_finished_line() {
        let mut l = List::default();
        l.note(None, item("going", DownloadState::Going, 0), 0);
        for n in 0..KEPT {
            l.note(None, item(&format!("d{n}"), DownloadState::Done, 100), n as i64 + 1);
        }
        assert_eq!(l.rows().len(), KEPT);
        assert!(l.get("going").is_some(), "a download still going was dropped");
        assert!(l.get("d0").is_none(), "the oldest finished line was kept");
    }

    /// A name from a server is one file name: no folder, nothing Windows
    /// refuses, never empty
    #[test]
    fn a_suggested_name_is_one_file_name() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(r"C:\Windows\evil.exe"), "evil.exe");
        assert_eq!(safe_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(safe_name("report. "), "report");
        assert_eq!(safe_name(".."), "download");
        assert_eq!(safe_name(""), "download");
    }

    /// A second file of the same name is given the next number, as browsers do
    #[test]
    fn a_second_file_of_the_same_name_is_numbered() {
        let dir = std::env::temp_dir().join(format!("shikisha-dl-{}", crate::random_hex(6)));
        let first = take_place(&dir, "report.pdf").unwrap();
        let second = take_place(&dir, "report.pdf").unwrap();
        let bare = take_place(&dir, "README").unwrap();
        let bare2 = take_place(&dir, "README").unwrap();
        assert_eq!(first.file_name().unwrap(), "report.pdf");
        assert_eq!(second.file_name().unwrap(), "report (1).pdf");
        assert_eq!(bare2.file_name().unwrap(), "README (1)");
        assert_eq!(bare.file_name().unwrap(), "README");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_site_is_the_host_a_person_reads() {
        assert_eq!(site_of("https://user:pw@files.example.com:8443/a/b?c#d"), "files.example.com:8443");
        assert_eq!(site_of("http://localhost:3000/x"), "localhost:3000");
    }

    /// A file of a far folder's page reads as on its way, then as on that
    /// machine under the name it got there; one that could not be sent stays
    /// here and says which machine it could not reach
    #[test]
    fn a_file_sent_away_says_where_it_is() {
        let mut l = List::default();
        l.note(Some("web".into()), item("a", DownloadState::Done, 100), 1);
        l.sending("a", "srv");
        assert_eq!(View::of(l.get("a").unwrap()).state, "sending");
        l.sent("a", "/home/me/Downloads/report (1).pdf");
        let v = View::of(l.get("a").unwrap());
        assert_eq!((v.state.as_str(), v.machine.as_str(), v.name.as_str()), ("done", "srv", "report (1).pdf"));
        assert!(l.newest_folder().is_none(), "the folder of a file on another machine is opened here");
        l.note(None, item("b", DownloadState::Done, 100), 2);
        l.sending("b", "srv");
        l.unsent("b", "srv");
        let v = View::of(l.get("b").unwrap());
        assert_eq!((v.machine.as_str(), v.unsent.as_str()), ("", "srv"));
    }

    /// A program is not opened from the list, the same rule a link follows
    #[test]
    fn a_program_is_offered_its_folder_not_opened() {
        let row = Row { item: Download { name: "setup.exe".into(), ..item("a", DownloadState::Done, 100) }, page: None, began: 0, machine: None, sending: false, unsent: String::new() };
        assert!(View::of(&row).runs);
        let row = Row { item: item("a", DownloadState::Done, 100), page: None, began: 0, machine: None, sending: false, unsent: String::new() };
        assert!(!View::of(&row).runs);
    }
}
