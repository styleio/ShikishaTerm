//! ラリーのデータ受け渡し置き場 (exchange)。
//!
//! AI→ブラウザのLua手渡しファイルや、貼れば再生できる記録を、Drive同期されない
//! ローカル領域 (%LOCALAPPDATA%) に **run 単位のフォルダ** で置く。
//!
//! 設計の決定事項:
//! - 固定ファイル名は使わない (複数タスクの同時実行ができなくなるため)。
//!   run ごとにユニークなフォルダを作り、その中は単純名でよい。
//! - フォルダが肥大化しないよう掃除は必須:
//!   - 一時ファイル (in.lua / human.txt) は消費した時点で削除 (take が消す)。
//!   - 不正終了で残ったゴミは、起動時に古いフォルダを一掃して回収する。
//!
//! AIが書いた生Luaは「TUI画面を読む」のではなく、この置き場のファイルから
//! バイト正確に受け取る。描画によるフェンス消失・指示エコーの誤検知が原理的に起きない。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// exchange のルート。%LOCALAPPDATA%\ShikishaTerm\exchange (無ければ一時フォルダ)。
/// Drive同期下 (アプリ本体の隣) には置かない
pub fn root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShikishaTerm").join("exchange")
}

/// プロセス内での連番。同一ミリ秒に複数 run が始まっても衝突しないように付ける
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 新しい run 用フォルダを作って、そのパスを返す。
/// 名前は「エポックミリ秒-連番」でユニーク (固定名を避け同時実行を可能にする)
pub fn new_run() -> std::io::Result<PathBuf> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = root().join(format!("{ms:013}-{n:04}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 渡されたパスが exchange のルート配下かどうか (パストラバーサル防止)。
/// 未作成ファイルは canonicalize できないので、親フォルダで判定する
pub fn within_root(p: &Path) -> bool {
    let Ok(rr) = root().canonicalize() else {
        return false;
    };
    if let Ok(pp) = p.canonicalize() {
        return pp.starts_with(&rr);
    }
    p.parent()
        .and_then(|par| par.canonicalize().ok())
        .is_some_and(|par| par.starts_with(&rr))
}

/// ファイルを読んで削除し、中身を返す (無ければ None)。
/// 一時ファイル (in.lua / human.txt) の「消費」に使う。exchange 配下限定
pub fn take(path: &Path) -> Option<String> {
    if !within_root(path) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(text)
}

/// テキストを1行追記する (記録 record.lua 用)。exchange 配下限定
pub fn append(path: &Path, text: &str) -> std::io::Result<()> {
    if !within_root(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "exchange の外には書けません",
        ));
    }
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", text.trim_end())
}

/// 起動時の掃除。更新が `days` 日より古い run フォルダ／ファイルを丸ごと削除する。
/// 正常終了なら一時ファイルは既に消えているので、これは不正終了のゴミの救済
pub fn sweep_old(days: u64) {
    let Ok(rd) = std::fs::read_dir(root()) else {
        return;
    };
    let Some(cutoff) =
        std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(days * 86_400))
    else {
        return;
    };
    let mut removed = 0u32;
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if !old {
            continue;
        }
        let p = e.path();
        let ok = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        if ok.is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        crate::append_hook_log(&format!("exchange: 古いrunを{removed}件掃除しました"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_run_makes_unique_dirs_under_root() {
        let a = new_run().unwrap();
        let b = new_run().unwrap();
        assert_ne!(a, b, "run フォルダは毎回ユニーク");
        assert!(within_root(&a));
        assert!(a.starts_with(root()));
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn take_reads_then_deletes() {
        let run = new_run().unwrap();
        let f = run.join("in.lua");
        std::fs::write(&f, "browser_go(\"br\",\"reload\")").unwrap();
        let got = take(&f);
        assert_eq!(got.as_deref(), Some("browser_go(\"br\",\"reload\")"));
        assert!(!f.exists(), "消費後は削除されている");
        assert_eq!(take(&f), None, "二度目は None");
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn append_confined_to_root() {
        let run = new_run().unwrap();
        let rec = run.join("record.lua");
        append(&rec, "line1").unwrap();
        append(&rec, "line2\n").unwrap();
        let body = std::fs::read_to_string(&rec).unwrap();
        assert_eq!(body, "line1\nline2\n");
        // ルート外は拒否
        assert!(append(Path::new("C:/windows/x.lua"), "x").is_err());
        let _ = std::fs::remove_dir_all(&run);
    }
}
