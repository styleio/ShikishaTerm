//! What the runtime says about itself: when it was built, and from which commit.
//!
//! The same two values the app has always shown. They are stamped here because
//! this is the crate that reports them, and a build of the runtime alone -- no
//! window anywhere -- still has to be able to say what it is.

fn main() {
    let built = std::process::Command::new("cmd")
        .args(["/C", "echo %DATE% %TIME%"])
        .output()
        .ok()
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
