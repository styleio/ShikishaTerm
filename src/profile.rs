//! エージェントプロファイル: ツール毎の状態検出ルールを宣言的に外部定義する。
//! DESIGN.md 4.3章。./profiles/*.json をユーザーが編集するだけで新ツールに対応できる。

use anyhow::{Context, Result};
use serde::Deserialize;

/// profiles/*.json の生の形
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileFile {
    pub name: String,
    /// コマンド名にこの文字列が含まれたら適用 (小文字比較)
    #[serde(default)]
    pub command_match: Vec<String>,
    /// 画面にマッチしたらBUSY (処理中) とみなす正規表現
    #[serde(default)]
    pub busy_patterns: Vec<String>,
    /// 画面にマッチしたらQUESTION (選択肢待ち) とみなす正規表現
    #[serde(default)]
    pub question_patterns: Vec<String>,
    /// この時間 (ms) 画面に変化が無ければ完了/待機とみなす
    #[serde(default = "default_silence_ms")]
    pub silence_ms: u64,
    /// 画面変化の判定から除外する最下部の行数。
    /// byobu/tmux等のステータスバー (時計が毎秒更新される) を無視するため
    #[serde(default = "default_ignore_bottom_rows")]
    pub ignore_bottom_rows: u16,
    /// DONE と見えてから、本当に終わったと確定するまでの待ち時間。
    ///
    /// AIの出力は途中で息継ぎをするので、静かになっただけでは終わりと言えない。
    /// 画面の表示はすぐ切り替えてよい (間違っても戻るだけ) が、
    /// 他のタブへ渡すのは取り消せないので、こちらだけ確証を待つ
    #[serde(default = "default_done_confirm_ms")]
    pub done_confirm_ms: u64,
}

fn default_silence_ms() -> u64 {
    2000
}

fn default_ignore_bottom_rows() -> u16 {
    2
}

fn default_done_confirm_ms() -> u64 {
    3000
}

/// コンパイル済みプロファイル
pub struct Profile {
    pub name: String,
    pub busy: Vec<regex::Regex>,
    pub question: Vec<regex::Regex>,
    pub silence_ms: u64,
    pub ignore_bottom_rows: u16,
    pub done_confirm_ms: u64,
}

impl Profile {
    /// どのプロファイルにもマッチしない場合の汎用フォールバック
    /// (沈黙タイマー・ベル・プロセス終了のみで判定)
    pub fn generic() -> Self {
        Self {
            name: "GENERIC".into(),
            busy: Vec::new(),
            question: Vec::new(),
            silence_ms: default_silence_ms(),
            ignore_bottom_rows: default_ignore_bottom_rows(),
            done_confirm_ms: default_done_confirm_ms(),
        }
    }

    pub fn compile(f: ProfileFile) -> Result<Self> {
        let compile_all = |patterns: &[String]| -> Result<Vec<regex::Regex>> {
            patterns
                .iter()
                .map(|p| regex::Regex::new(p).with_context(|| format!("正規表現が不正: {p}")))
                .collect()
        };
        Ok(Self {
            busy: compile_all(&f.busy_patterns)?,
            question: compile_all(&f.question_patterns)?,
            silence_ms: f.silence_ms,
            done_confirm_ms: f.done_confirm_ms,
            ignore_bottom_rows: f.ignore_bottom_rows,
            name: f.name,
        })
    }
}

/// exe隣 (ポータブル配置) → カレント直下の順で探す
fn candidate_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("profiles")))
    {
        dirs.push(d);
    }
    dirs.push(std::path::PathBuf::from("profiles"));
    dirs
}

fn find_profile<F>(pred: F) -> Option<Profile>
where
    F: Fn(&std::path::Path, &ProfileFile) -> bool,
{
    for dir in candidate_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(pf) = serde_json::from_str::<ProfileFile>(&text) else {
                continue;
            };
            if pred(&path, &pf) {
                if let Ok(p) = Profile::compile(pf) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// コマンド名にマッチするプロファイルを返す。無ければ汎用フォールバック
pub fn load_for_command(cmd: &str) -> Profile {
    let needle = std::path::Path::new(cmd)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(cmd)
        .to_lowercase();
    find_profile(|_, pf| {
        pf.command_match
            .iter()
            .any(|m| needle.contains(&m.to_lowercase()))
    })
    .unwrap_or_else(Profile::generic)
}

/// プロファイル名 (ファイル名 or name フィールド) の明示指定でロードする。
/// config.jsonのタブ定義 "profile": "claude" 用 (ssh先のAI等、コマンド名で判別できない場合)
pub fn load_by_name(name: &str) -> Profile {
    let needle = name.to_lowercase();
    find_profile(|path, pf| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.to_lowercase() == needle)
            || pf.name.to_lowercase() == needle
    })
    .unwrap_or_else(Profile::generic)
}
