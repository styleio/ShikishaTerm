//! What the runtime says about itself: when it was built, and from which commit.
//!
//! The same two values the app has always shown. They are stamped here because
//! this is the crate that reports them, and a build of the runtime alone -- no
//! window anywhere -- still has to be able to say what it is.

fn main() {
    // Every system can say what time it is; they are asked differently. Asked
    // the Windows way alone, every build made anywhere else stamped an empty
    // string, and the board's footer said "build ()" with nothing in it.
    let mut when = match cfg!(windows) {
        true => std::process::Command::new("cmd"),
        false => std::process::Command::new("date"),
    };
    match cfg!(windows) {
        true => when.args(["/C", "echo %DATE% %TIME%"]),
        false => when.arg("+%Y/%m/%d %H:%M:%S"),
    };
    let built = when
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=BUILD_TIME={built}");
    let rev = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    println!("cargo:rustc-env=BUILD_REV={rev}{}", if dirty { "+" } else { "" });
    println!("cargo:rerun-if-changed=src");
    // And again after every commit. Without it the stamp kept the commit the
    // sources were last edited at: fix, build, then commit, and the app went on
    // calling itself the commit before -- which the deploy rightly refused
    watch_git_head();
    println!("cargo:rerun-if-changed=../../tools/build_git_head.rs");

    // The library that compresses video where the operating system has no
    // encoder of its own. Said here rather than left to whoever runs the
    // build: a link line that lives in somebody's environment is a link line
    // that works on one machine.
    //
    // Only when asked for. Off, nothing is linked and nothing is needed;
    // Windows never asks, because it has its own (src/vencode.rs).
    if std::env::var_os("CARGO_FEATURE_VP8").is_some() {
        println!("cargo:rustc-link-lib=vpx");
    }
}

include!("../../tools/build_git_head.rs");
