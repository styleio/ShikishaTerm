//! Where a branch's folder really goes, asked from outside the library.
//!
//! The unit tests cannot answer this. They run with `cfg(test)` set, which is
//! exactly what sends them to a folder of their own so a test run never leaves
//! anything in somebody's home. An integration test compiles the library
//! without it, so this is the answer the program itself would give.
//!
//! Nothing is made here. The path is worked out and read.

use std::path::{Path, PathBuf};

fn home() -> Option<PathBuf> {
    for key in ["USERPROFILE", "HOME"] {
        let Ok(said) = std::env::var(key) else { continue };
        let at = PathBuf::from(said.trim());
        if at.is_dir() {
            return Some(at);
        }
    }
    None
}

#[test]
fn a_branch_lands_under_the_person_and_never_beside_the_project() {
    let project = Path::new("D:/somewhere/myproject");
    let at = shikisha_core::worktree::folder_for(project, "feature/login");

    // The project's own folder is left exactly as it was
    assert!(
        !at.starts_with(project.parent().unwrap()),
        "本体の隣に置いている: {at:?}"
    );
    // Under the project it belongs to, with the branch's shape kept
    assert!(at.ends_with("myproject/feature/login"), "{at:?}");

    let Some(home) = home() else { return };
    // A home being synced to the cloud is a real answer too, and on this
    // machine it may be either -- so what is checked is that it landed in one
    // of the two places the program is allowed to put it
    let ours = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    assert!(
        at.starts_with(home.join("SHIKISHA-TERM").join("branches"))
            || at.starts_with(ours.join("SHIKISHA-TERM").join("worktrees")),
        "自分のフォルダにも逃げ場にも入っていない: {at:?}"
    );
}
