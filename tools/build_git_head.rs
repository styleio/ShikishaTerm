// Shared by the two build scripts that stamp which commit a build is
// (`build.rs` and `crates/core/build.rs`), through `include!`. A build script
// cannot depend on another crate's code, and two copies of this would drift --
// the core one had no copy at all, so its stamp kept the commit it was first
// built at.

/// Take the label again after a new commit.
///
/// `.git/HEAD` still reads "ref: refs/heads/main", so watching only that file
/// means committing does not rerun the build script. The old hash and the "+"
/// stay, which defeats the whole point: telling whether what is running is the
/// latest. What HEAD points to (refs/heads/main) is watched as well.
fn watch_git_head() {
    let git = std::process::Command::new("git")
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    // Points at the right place in a worktree or a submodule as well
    let Some(git) = git else {
        return;
    };
    let git = std::path::Path::new(&git);
    println!("cargo:rerun-if-changed={}", git.join("HEAD").display());
    let Ok(head) = std::fs::read_to_string(git.join("HEAD")) else {
        return;
    };
    // A detached HEAD rewrites HEAD itself, so there is nothing more to watch
    let Some(r) = head.strip_prefix("ref:") else {
        return;
    };
    let refpath = git.join(r.trim());
    if refpath.exists() {
        println!("cargo:rerun-if-changed={}", refpath.display());
    } else {
        // Packed refs leave no refs/heads/... file. Watching a path that does not
        // exist reruns this every time and slows incremental builds, so only real
        // files are handed over
        let packed = git.join("packed-refs");
        if packed.exists() {
            println!("cargo:rerun-if-changed={}", packed.display());
        }
    }
}
