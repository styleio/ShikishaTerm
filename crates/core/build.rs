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
}
