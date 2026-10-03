//! Saved folder organization and on-demand, bounded capacity measurements.
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Usage {
    pub bytes: u64,
    pub partial: bool,
    pub error: String,
    pub measured: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Notice {
    pub busy: bool,
    pub done: bool,
    pub error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct View {
    pub measuring: bool,
    pub usage: BTreeMap<String, Usage>,
    pub results: BTreeMap<String, Notice>,
}

#[derive(Default)]
pub struct Manager {
    views: BTreeMap<String, View>,
    measuring: Option<(String, mpsc::Receiver<(String, Usage)>, Arc<AtomicBool>)>,
    /// A delayed remote preflight must never act on the next desk.
    pub pending: BTreeMap<String, String>,
    pub reviews: BTreeMap<String, String>,
    queue: VecDeque<String>,
    active: Option<String>,
}

impl Manager {
    pub fn enqueue(&mut self, desk: &str, key: String) {
        if self.pending.contains_key(&key) {
            return;
        }
        self.pending.insert(key.clone(), desk.to_string());
        self.note(desk, &key, true, String::new());
        self.queue.push_back(key);
    }

    /// One removal at a time bounds git preflights and remote connections.
    pub fn next_removal(&mut self, desk: &str) -> Option<String> {
        if self.active.is_some() {
            return None;
        }
        while let Some(key) = self.queue.pop_front() {
            if self.pending.get(&key).is_none_or(|d| d != desk) {
                self.finish(&key, crate::i18n::t("err.folders.changed").to_string());
                continue;
            }
            self.active = Some(key.clone());
            return Some(key);
        }
        None
    }
    pub fn view(&mut self, desk: &str) -> View {
        if let Some((id, rx, _)) = &self.measuring {
            loop {
                match rx.try_recv() {
                    Ok((key, usage)) => {
                        self.views
                            .entry(id.clone())
                            .or_default()
                            .usage
                            .insert(key, usage);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.views.entry(id.clone()).or_default().measuring = false;
                        self.measuring = None;
                        break;
                    }
                }
            }
        }
        self.views.get(desk).cloned().unwrap_or_default()
    }

    pub fn note(&mut self, desk: &str, key: &str, busy: bool, error: String) {
        self.views
            .entry(desk.to_string())
            .or_default()
            .results
            .insert(
                key.to_string(),
                Notice {
                    busy,
                    done: !busy && error.is_empty(),
                    error,
                },
            );
    }

    pub fn finish(&mut self, key: &str, error: String) {
        self.reviews.remove(key);
        if self.active.as_deref() == Some(key) {
            self.active = None;
        }
        if let Some(desk) = self.pending.remove(key) {
            self.note(&desk, key, false, error);
        }
    }

    pub fn measure(&mut self, desk: &str, folders: Vec<crate::config::Folder>) -> bool {
        if self.measuring.is_some() {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.views.entry(desk.to_string()).or_default().measuring = true;
        self.measuring = Some((desk.to_string(), rx, cancel.clone()));
        std::thread::spawn(move || {
            for folder in folders {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let (Some(path), Some(key)) = (folder.cwd.as_deref(), folder.place()) else {
                    continue;
                };
                let mut usage = match folder.host {
                    Some(ref host) => remote_usage(host, path),
                    None => local_usage(path, &cancel),
                };
                usage.measured = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if tx
                    .send((key.to_string_lossy().into_owned(), usage))
                    .is_err()
                {
                    break;
                }
            }
        });
        true
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        if let Some((_, _, cancel)) = &self.measuring {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Closing saved tabs must not discard a live turn or an editor's unsaved text.
/// Include child directories: a shell may have changed directory since launch.
pub fn blocking_work<'a>(
    folder: &crate::config::Folder,
    tabs: impl IntoIterator<Item = &'a crate::tab::Tab>,
    editors: &[crate::view::EditorOpen],
) -> Option<String> {
    if let Some(tab) = tabs.into_iter().find(|t| {
        t.cwd().is_some_and(|path| contains(folder, path, t.host()))
            && matches!(
                t.state,
                crate::detect::TabState::Busy | crate::detect::TabState::Question | crate::detect::TabState::Background
            )
    }) {
        return Some(crate::i18n::tp(
            "msg.folder.in_use",
            &[("name", &tab.title)],
        ));
    }
    if editors
        .iter()
        .any(|e| !e.read_only && e.dir.as_deref().is_some_and(|path| contains(folder, path, e.on.as_deref())))
    {
        return Some(crate::i18n::t("err.folders.editor").to_string());
    }
    None
}

/// The same machine, at this folder or below it. Remote paths must not be
/// canonicalized or case-folded using this PC's filesystem rules.
fn contains(folder: &crate::config::Folder, path: &Path, on: Option<&str>) -> bool {
    let Some(root) = folder.cwd.as_deref() else { return false };
    if folder.host.as_ref().map(|h| h.name.as_str()) != on { return false; }
    if on.is_none() {
        crate::worktree::inside_checkout(root, path)
    } else {
        let (path, root) = (path.to_string_lossy(), root.to_string_lossy());
        let root = root.trim_end_matches('/');
        path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
    }
}

pub fn deletion_guard(
    folder: &crate::config::Folder,
    desk: &crate::config::Desk,
    desks: &[crate::config::Desk],
) -> Option<&'static str> {
    if folder.keep_first {
        return Some("err.folders.pinned");
    }
    let Some(at) = folder.cwd.as_deref() else {
        return Some("err.folders.changed");
    };
    let primary = match &folder.host {
        None => at.exists() && !crate::repo::is_linked(at),
        Some(h) => desk
            .projects
            .iter()
            .filter_map(|p| p.home_on(&h.name))
            .any(|home| {
                if h.is_made() {
                    home.sandbox == h.instance
                } else {
                    crate::uistate::same_folder(Path::new(&home.at), at)
                }
            }),
    };
    if primary {
        return Some("err.worktree.not_a_branch");
    }
    if desks.iter().any(|other| {
        other.uid != desk.uid
            && other.folders.iter().any(|f| {
                f.cwd.as_deref().is_some_and(|p| contains(folder, p, f.host.as_ref().map(|h| h.name.as_str())))
            })
    }) {
        return Some("err.folders.shared");
    }
    None
}

// An interactive request gets half a minute per folder. Very large trees
// return an explicitly partial estimate instead of monopolizing a worker.
const MEASURE_WAIT: Duration = Duration::from_secs(30);

fn local_usage(root: &Path, cancel: &AtomicBool) -> Usage {
    let mut usage = Usage::default();
    let start = Instant::now();
    let result = (|| -> std::io::Result<()> {
        let metadata = std::fs::symlink_metadata(root)?;
        if link(&metadata) || !metadata.is_dir() {
            return Err(std::io::Error::other(crate::i18n::t(
                "err.folders.directory",
            )));
        }
        // Open iterators, rather than a list of every child path, bound memory
        // by directory depth. Do not follow directory links or Windows junctions.
        let mut stack = vec![std::fs::read_dir(root)?];
        while let Some(entries) = stack.last_mut() {
            if cancel.load(Ordering::Relaxed) || start.elapsed() >= MEASURE_WAIT {
                return Err(std::io::Error::other(crate::i18n::t(
                    "err.folders.measure_timeout",
                )));
            }
            let Some(entry) = entries.next() else {
                stack.pop();
                continue;
            };
            let path = entry?.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if link(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                stack.push(std::fs::read_dir(path)?);
            } else if metadata.is_file() {
                usage.bytes = usage.bytes.saturating_add(metadata.len());
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        usage.partial = true;
        usage.error = e.to_string();
    }
    usage
}

fn link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn remote_usage(host: &crate::config::HostSpec, path: &Path) -> Usage {
    let result = (|| -> anyhow::Result<u64> {
        let at = crate::elsewhere::Elsewhere::of(host)?;
        let dir = crate::worktree::for_a_shell(&[path.to_string_lossy().into_owned()]);
        // Apparent bytes, without following links. A failed/unsupported command
        // is an error, never a zero-sized folder. SSH and MicroVM share this path.
        let ran = crate::elsewhere::exec(
            &at,
            &format!("test ! -L {dir} && test -d {dir} && du -sb -- {dir}"),
            MEASURE_WAIT.as_millis() as u64,
        )?;
        anyhow::ensure!(ran.ok(), "{}: {}", crate::i18n::t("err.folders.measure"), ran.said());
        Ok(ran
            .out
            .split_whitespace()
            .next()
            .ok_or_else(|| anyhow::anyhow!("{}", crate::i18n::t("err.folders.measure")))?
            .parse()?)
    })();
    match result {
        Ok(bytes) => Usage {
            bytes,
            ..Default::default()
        },
        Err(e) => Usage {
            partial: true,
            error: format!("{e:#}"),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_counts_files_but_never_enters_links() {
        let root = std::env::temp_dir().join(format!("shikisha-capacity-{}", crate::random_hex(6)));
        let measured = root.join("measured");
        let outside = root.join("outside");
        std::fs::create_dir_all(measured.join("nested")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(measured.join("a"), b"123").unwrap();
        std::fs::write(measured.join("nested/b"), b"45678").unwrap();
        std::fs::write(outside.join("secret"), b"never count this").unwrap();
        let linked = measured.join("linked");
        #[cfg(windows)]
        junction::create(&outside, &linked).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        let cancel = AtomicBool::new(false);
        let usage = local_usage(&measured, &cancel);
        assert_eq!(usage.bytes, 8);
        assert!(!usage.partial, "{}", usage.error);
        assert!(
            local_usage(&linked, &cancel).partial,
            "a linked root was followed"
        );
        assert!(
            local_usage(&root.join("absent"), &cancel).partial,
            "missing was reported as empty"
        );
        cancel.store(true, Ordering::Relaxed);
        let stopped = local_usage(&measured, &cancel);
        assert!(stopped.partial && !stopped.error.is_empty());
        // The fixture stays in the temporary directory; no recursive removal
        // ever gets a chance to follow the deliberately constructed link.
    }

    #[test]
    fn deletion_results_stay_with_the_desk_that_requested_them() {
        let mut manager = Manager::default();
        manager.pending.insert("folder".into(), "first".into());
        manager.note("first", "folder", true, String::new());
        assert!(manager.view("second").results.is_empty());
        manager.finish("folder", "kept".into());
        assert_eq!(manager.view("first").results["folder"].error, "kept");
        assert!(!manager.view("first").results["folder"].busy);
        assert!(manager.view("second").results.is_empty());
    }

    #[test]
    fn bulk_removal_is_serial_and_queued_work_cannot_cross_desks() {
        let mut manager = Manager::default();
        manager.enqueue("first", "a".into());
        manager.enqueue("first", "b".into());
        manager.enqueue("first", "a".into());
        assert_eq!(manager.next_removal("first").as_deref(), Some("a"));
        assert!(manager.next_removal("first").is_none());
        manager.finish("a", String::new());
        assert!(manager.next_removal("second").is_none());
        assert!(manager.pending.is_empty());
        assert!(!manager.view("first").results["b"].error.is_empty());
        assert!(manager.view("second").results.is_empty());
    }

    #[test]
    fn a_child_folder_on_another_desk_blocks_parent_deletion() {
        let root = std::env::temp_dir().join(format!("shikisha-guard-{}", crate::random_hex(6)));
        let folder = crate::config::Folder { cwd: Some(root.clone()), ..Default::default() };
        let here = crate::config::Desk { uid: "here".into(), ..Default::default() };
        let mut other = crate::config::Desk { uid: "away".into(), folders: vec![crate::config::Folder {
            cwd: Some(root.join("src")), ..Default::default()
        }], ..Default::default() };
        assert_eq!(deletion_guard(&folder, &here, std::slice::from_ref(&other)), Some("err.folders.shared"));
        other.folders[0].cwd = Some(root.with_file_name("neighbour"));
        assert_eq!(deletion_guard(&folder, &here, &[other]), None);
    }

    #[test]
    fn folder_protection_distinguishes_machines_and_path_components() {
        let folder = crate::config::Folder {
            cwd: Some("/work/project".into()),
            host: Some(crate::config::HostSpec { name: "server-a".into(), ..Default::default() }),
            ..Default::default()
        };
        assert!(contains(&folder, Path::new("/work/project/src"), Some("server-a")));
        assert!(!contains(&folder, Path::new("/work/project-other"), Some("server-a")));
        assert!(!contains(&folder, Path::new("/work/Project/src"), Some("server-a")));
        assert!(!contains(&folder, Path::new("/work/project/src"), Some("server-b")));
        assert!(!contains(&folder, Path::new("/work/project/src"), None));
    }

    #[test]
    fn working_tabs_from_any_desk_protect_their_parent_folder() {
        use crate::{detect::TabState, tab::{Tab, TabOptions}};
        let root = std::env::temp_dir().join(format!("shikisha-busy-folder-{}", crate::random_hex(6)));
        let child = root.join("src");
        std::fs::create_dir_all(&child).unwrap();
        let folder = crate::config::Folder { cwd: Some(root.clone()), ..Default::default() };
        let mut tab = Tab::spawn("Background worker".into(), &[crate::test_shell()], None, 10, 40,
            TabOptions { cwd: Some(child), ..Default::default() }).unwrap();
        let foreground: Vec<Tab> = Vec::new();
        for state in [TabState::Busy, TabState::Question, TabState::Background] {
            tab.state = state;
            assert!(blocking_work(&folder, foreground.iter().chain(std::iter::once(&tab)), &[]).is_some());
        }
        tab.state = TabState::Wait;
        assert!(blocking_work(&folder, foreground.iter().chain(std::iter::once(&tab)), &[]).is_none());
        tab.kill();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_editor_in_a_child_directory_blocks_archiving_and_deletion() {
        let root = std::path::PathBuf::from(crate::local_path("D:/folder-guard"));
        let folder = crate::config::Folder {
            cwd: Some(root.clone()),
            ..Default::default()
        };
        let mut editor = crate::view::EditorOpen {
            key: "editor".into(),
            dir: Some(root.join("src")),
            showing: None,
            stamp: None,
            scratch: true,
            at: None,
            on: None,
            diff: None,
            read_only: false,
        };
        assert!(blocking_work(&folder, &[], &[editor.clone()]).is_some());
        editor.read_only = true;
        assert!(blocking_work(&folder, &[], &[editor.clone()]).is_none());
        editor.read_only = false;
        editor.dir = Some(root.with_file_name("folder-guard-other"));
        assert!(blocking_work(&folder, &[], &[editor]).is_none());
    }
}
