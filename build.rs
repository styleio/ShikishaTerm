//! ビルドしたものを画面から見分けられるようにする。
//!
//! 「直したはずなのに直っていない」の原因が、古い実行ファイルを動かして
//! いただけ、ということが何度かあった。日時が見えていれば、
//! 「最新は MM/DD HH:MM です」と伝えるだけで新旧を照合できる。
//! ハッシュだけでは、どちらが新しいかが読み取れない。

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    // 新旧を見比べられるよう、まず日時
    let built = run("powershell", &["-NoProfile", "-Command", "Get-Date -Format 'MM/dd HH:mm'"])
        .unwrap_or_else(|| "?".into());
    // どのコミットかも添える (同じ分に複数ビルドしたときの区別用)
    let rev = run("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nogit".into());
    let dirty = run("git", &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    println!("cargo:rustc-env=BUILD_TIME={built}");
    println!(
        "cargo:rustc-env=BUILD_REV={}{}",
        rev,
        if dirty { "+" } else { "" }
    );
    // ビルドのたびに日時を入れ直す
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
