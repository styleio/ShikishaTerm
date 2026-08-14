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

    // 訳語は exe の隣の lang/ から読む。手で置いたままだと、
    // 直しても動かしたものには届かない (「あなた」が YOU のままだった)
    copy_beside_exe("lang", "json");
    // 書き方の説明も同じ。こちらは人が読むだけでなく、
    // 自動化を書かせるAIへそのまま渡している。古いものが隣に残っていると、
    // AIは「その機能は仕様に無い」と正しく答えてしまう
    copy_beside_exe("docs", "md");
    // 状態検出プロファイル (profiles/*.json) も exe の隣から読む
    // (profile::candidate_dirs)。配らないと配布版が全AIをGENERIC判定にしてしまう
    copy_beside_exe("profiles", "json");

    println!("cargo:rustc-env=BUILD_TIME={built}");
    println!(
        "cargo:rustc-env=BUILD_REV={}{}",
        rev,
        if dirty { "+" } else { "" }
    );
    // ビルドのたびに日時を入れ直す
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=lang");
    println!("cargo:rerun-if-changed=docs");
    println!("cargo:rerun-if-changed=profiles");
}

/// リポジトリのフォルダを、そのまま exe の隣へ置く。
///
/// 隣に置かれたものは埋め込みより優先される。置きっぱなしにすると、
/// 直したはずのものが動かしたものへ届かない
///
/// OUT_DIR は target/<profile>/build/<pkg>-<hash>/out なので、
/// 3つ上が exe の置き場になる
fn copy_beside_exe(dir_name: &str, ext: &str) {
    let Ok(out) = std::env::var("OUT_DIR") else {
        return;
    };
    let mut dir = std::path::PathBuf::from(out);
    for _ in 0..3 {
        dir.pop();
    }
    let dest = dir.join(dir_name);
    if std::fs::create_dir_all(&dest).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir_name) else {
        return;
    };
    for e in entries.flatten() {
        let from = e.path();
        if from.extension().is_some_and(|x| x == ext) {
            if let Some(name) = from.file_name() {
                // 配れなくても止めない。埋め込んだもので動く
                if let Err(err) = std::fs::copy(&from, dest.join(name)) {
                    println!("cargo:warning={dir_name} を配れませんでした {name:?}: {err}");
                }
            }
        }
    }
}
