// Shared by the two build scripts that stamp which commit a build is
// (`build.rs` and `crates/core/build.rs`), through `include!`. A build script
// cannot depend on another crate's code, and two copies of this would drift --
// the core one had no copy at all, so its stamp kept the commit it was first
// built at.

/// One of git's directories, as git itself gives it. `--absolute-git-dir` is
/// this worktree's own (where its HEAD is); `--git-common-dir` is the one every
/// worktree shares (where the branches are).
fn git_dir(which: &str) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", which])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }
    // --git-common-dir answers with a relative path from some checkouts
    Some(std::path::PathBuf::from(path))
}

/// Take the label again after a new commit.
///
/// `.git/HEAD` still reads "ref: refs/heads/main", so watching only that file
/// means committing does not rerun the build script. The old hash and the "+"
/// stay, which defeats the whole point: telling whether what is running is the
/// latest. What HEAD points to (refs/heads/main) is watched as well.
///
/// The two live in different directories once a worktree is involved. A
/// worktree's git dir is `.git/worktrees/<name>`, which holds its HEAD, but its
/// branch is a file under the main checkout's `.git/refs`. Looking for the
/// branch beside the worktree's HEAD found nothing, watched nothing, and left
/// every build made in a worktree stamped with the commit it was first built
/// at -- which the deploy then turned away as "not built from HEAD", correctly
/// and unhelpfully (2026-09-20).
fn watch_git_head() {
    // Points at the right place in a worktree or a submodule as well
    let Some(git) = git_dir("--absolute-git-dir") else {
        return;
    };
    // Shared by every worktree; the same directory when there are none
    let common = git_dir("--git-common-dir").unwrap_or_else(|| git.clone());
    println!("cargo:rerun-if-changed={}", git.join("HEAD").display());
    let Ok(head) = std::fs::read_to_string(git.join("HEAD")) else {
        return;
    };
    // A detached HEAD rewrites HEAD itself, so there is nothing more to watch
    let Some(r) = head.strip_prefix("ref:") else {
        return;
    };
    let refpath = common.join(r.trim());
    if refpath.exists() {
        println!("cargo:rerun-if-changed={}", refpath.display());
    } else {
        // Packed refs leave no refs/heads/... file. Watching a path that does not
        // exist reruns this every time and slows incremental builds, so only real
        // files are handed over
        let packed = common.join("packed-refs");
        if packed.exists() {
            println!("cargo:rerun-if-changed={}", packed.display());
        }
    }
}
