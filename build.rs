//! Makes a build tell you which build it is, on screen.
//!
//! More than once, "I fixed it and it is still broken" turned out to be an old
//! executable still running. With the date and time visible, "the latest is
//! MM/DD HH:MM" is enough to tell old from new. A hash on its own cannot say
//! which of two is the newer one.

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    // The date and time first, so old and new can be compared at a glance
    let built = run("powershell", &["-NoProfile", "-Command", "Get-Date -Format 'MM/dd HH:mm'"])
        .unwrap_or_else(|| "?".into());
    // Which commit, too (to tell apart several builds made in the same minute)
    let rev = run("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nogit".into());
    let dirty = run("git", &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    // The first thing someone who downloads this sees is its icon in Explorer.
    // Left as the generic console icon, it looks like something picked up off the street
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "SHIKISHA-TERM — conductor AI terminal");
        res.set("ProductName", "SHIKISHA-TERM");
        res.set("CompanyName", "styleio");
        // A copyright notice, not the name of a license. "MIT License" on this
        // field states no holder and no year, which is the one thing the field
        // exists to say
        res.set("LegalCopyright", "Copyright (c) 2026 styleio. MIT License.");
        // Both of these were empty. Windows shows OriginalFilename to say what
        // the file was called before anyone renamed it, and a blank one on a
        // downloaded executable is a small thing every reputation heuristic
        // notices -- and something Explorer simply has no answer for
        res.set("OriginalFilename", "SHIKISHA-TERM.exe");
        res.set("InternalName", "SHIKISHA-TERM");
        if let Err(e) = res.compile() {
            // The program runs without its icon. Not a reason to stop the whole build
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }

    // What goes beside the exe is written in dist.list. When each thing that hands
    // it out (this, stage.ps1, release.yml) kept its own list, they quietly disagreed
    for pattern in dist_patterns("beside-exe") {
        copy_beside_exe(&pattern, false);
    }
    for pattern in dist_patterns("beside-exe-flat") {
        copy_beside_exe(&pattern, true);
    }

    println!("cargo:rustc-env=BUILD_TIME={built}");
    println!(
        "cargo:rustc-env=BUILD_REV={}{}",
        rev,
        if dirty { "+" } else { "" }
    );
    // Stamp the date and time again on every build
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    watch_git_head();
    println!("cargo:rerun-if-changed=tools/build_git_head.rs");
    println!("cargo:rerun-if-changed=lang");
    println!("cargo:rerun-if-changed=docs");
    println!("cargo:rerun-if-changed=profiles");
    // tools/conpty.ps1 may put it there later. Unwatched, the build after it is
    // fetched would not carry it beside the exe, and the in-box ConPTY would
    // quietly stay in use
    println!("cargo:rerun-if-changed=vendor/conpty");
}

include!("tools/build_git_head.rs");

/// Reads the `dir/pattern` lines under one section of dist.list.
///
/// Kept deliberately plain. The PowerShell side (tools/stage.ps1) reads the
/// same file, and a format that needed a library on both sides would one day be
/// read two different ways
fn dist_patterns(section: &str) -> Vec<String> {
    println!("cargo:rerun-if-changed=dist.list");
    let Ok(text) = std::fs::read_to_string("dist.list") else {
        println!("cargo:warning=could not read dist.list, so nothing can be placed beside the exe");
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            in_section = name == section;
            continue;
        }
        if in_section {
            out.push(t.to_string());
        }
    }
    out
}

/// Places whatever matches `dir/pattern` beside the exe, as it is.
///
/// What sits beside the exe wins over what is embedded. Leave a stale copy
/// there and a fix never reaches the program that runs
///
/// `flat` places it at the exe's own level, with no folder around it. It exists
/// for the things Windows only looks for beside the exe (conpty.dll)
///
/// OUT_DIR is target/<profile>/build/<pkg>-<hash>/out, so three levels up is
/// where the exe lives
fn copy_beside_exe(pattern: &str, flat: bool) {
    let Ok(out) = std::env::var("OUT_DIR") else {
        return;
    };
    let mut dir = std::path::PathBuf::from(out);
    for _ in 0..3 {
        dir.pop();
    }
    let Some((dir_name, file_pat)) = pattern.rsplit_once('/') else {
        println!("cargo:warning=a dist.list line could not be read: {pattern}");
        return;
    };
    let dest = if flat { dir } else { dir.join(dir_name) };
    if std::fs::create_dir_all(&dest).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir_name) else {
        return;
    };
    for e in entries.flatten() {
        let from = e.path();
        let Some(name) = from.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !matches_pattern(name, file_pat) {
            continue;
        }
        // Not being able to place it does not stop anything. The embedded copy still works
        if let Err(err) = std::fs::copy(&from, dest.join(name)) {
            println!("cargo:warning=could not place {name} from {dir_name}: {err}");
        }
    }
}

/// Matching against a pattern with a single `*` (`*.json`, `AUTOMATION*.md`).
/// Nothing more is needed. If it ever is, add it then
fn matches_pattern(name: &str, pat: &str) -> bool {
    match pat.split_once('*') {
        Some((head, tail)) => {
            name.len() >= head.len() + tail.len() && name.starts_with(head) && name.ends_with(tail)
        }
        None => name == pat,
    }
}
