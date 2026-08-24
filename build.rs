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

    // ダウンロードした人が最初に見るのは Explorer のアイコン。
    // 汎用のコンソールアイコンのままだと、そこで「拾い物」に見える
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "SHIKISHA-TERM — conductor AI terminal");
        res.set("ProductName", "SHIKISHA-TERM");
        res.set("CompanyName", "SHIKISHA-TERM");
        res.set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            // アイコンが無くてもソフトは動く。ビルドごと止める理由にはならない
            println!("cargo:warning=アイコンを埋め込めませんでした: {e}");
        }
    }

    // exe の隣に置くものは dist.list に書いてある。配る側 (ここ・Deploy.cmd・
    // deploy-hook・release.yml) が各自リストを持っていた頃は、静かに食い違った
    for pattern in beside_exe_patterns() {
        copy_beside_exe(&pattern);
    }

    println!("cargo:rustc-env=BUILD_TIME={built}");
    println!(
        "cargo:rustc-env=BUILD_REV={}{}",
        rev,
        if dirty { "+" } else { "" }
    );
    // ビルドのたびに日時を入れ直す
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    watch_git_head();
    println!("cargo:rerun-if-changed=lang");
    println!("cargo:rerun-if-changed=docs");
    println!("cargo:rerun-if-changed=profiles");
}

/// コミットし直したらラベルも取り直す。
///
/// `.git/HEAD` の中身は "ref: refs/heads/main" のままなので、そこだけを見張って
/// いるとコミットしても build.rs が再実行されない。結果、古いハッシュと "+" が
/// 残り続け、「動かしているものが最新かを見分ける」という目的そのものが崩れる。
/// HEAD が指している先 (refs/heads/main) も一緒に見張る。
fn watch_git_head() {
    // worktree / submodule でも正しい場所を指す
    let Some(git) = run("git", &["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    let git = std::path::Path::new(&git);
    println!("cargo:rerun-if-changed={}", git.join("HEAD").display());
    let Ok(head) = std::fs::read_to_string(git.join("HEAD")) else {
        return;
    };
    // detached HEAD なら HEAD 自体が書き換わるので、これ以上見張るものは無い
    let Some(r) = head.strip_prefix("ref:") else {
        return;
    };
    let refpath = git.join(r.trim());
    if refpath.exists() {
        println!("cargo:rerun-if-changed={}", refpath.display());
    } else {
        // 束ねられていると refs/heads/... のファイルは無い。無いパスを見張らせると
        // 毎回再実行されて増分ビルドが遅くなるので、実在するものだけを渡す
        let packed = git.join("packed-refs");
        if packed.exists() {
            println!("cargo:rerun-if-changed={}", packed.display());
        }
    }
}

/// dist.list の [beside-exe] に並んだ `dir/pattern` を読む。
///
/// わざと素朴な形式にしてある。同じファイルを PowerShell 側 (tools/stage.ps1)
/// も読むので、両方にライブラリが要る形式にすると、いつか解釈がずれる
fn beside_exe_patterns() -> Vec<String> {
    println!("cargo:rerun-if-changed=dist.list");
    let Ok(text) = std::fs::read_to_string("dist.list") else {
        println!("cargo:warning=dist.list が読めません。exe の隣に何も配れません");
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
            in_section = name == "beside-exe";
            continue;
        }
        if in_section {
            out.push(t.to_string());
        }
    }
    out
}

/// `dir/pattern` にあたるものを、そのまま exe の隣へ置く。
///
/// 隣に置かれたものは埋め込みより優先される。置きっぱなしにすると、
/// 直したはずのものが動かしたものへ届かない
///
/// OUT_DIR は target/<profile>/build/<pkg>-<hash>/out なので、
/// 3つ上が exe の置き場になる
fn copy_beside_exe(pattern: &str) {
    let Ok(out) = std::env::var("OUT_DIR") else {
        return;
    };
    let mut dir = std::path::PathBuf::from(out);
    for _ in 0..3 {
        dir.pop();
    }
    let Some((dir_name, file_pat)) = pattern.rsplit_once('/') else {
        println!("cargo:warning=dist.list の書き方が読めません: {pattern}");
        return;
    };
    let dest = dir.join(dir_name);
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
        // 配れなくても止めない。埋め込んだもので動く
        if let Err(err) = std::fs::copy(&from, dest.join(name)) {
            println!("cargo:warning={dir_name} を配れませんでした {name}: {err}");
        }
    }
}

/// `*` を1つだけ含む形 (`*.json`, `AUTOMATION*.md`) に対する照合。
/// それ以上は要らない。要るようになったら、その時に足す
fn matches_pattern(name: &str, pat: &str) -> bool {
    match pat.split_once('*') {
        Some((head, tail)) => {
            name.len() >= head.len() + tail.len() && name.starts_with(head) && name.ends_with(tail)
        }
        None => name == pat,
    }
}
