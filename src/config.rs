//! config.json: タブ構成の定義。DESIGN.md 7.4章のスキーマのサブセット (Phase 3時点)。
//! exe隣 (ポータブル配置) → カレント直下の順で探す。

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
    /// フックスクリプトのパス (例: "scripts/hooks.lua")
    #[serde(default)]
    pub lua: Option<String>,
    /// 自動送信チェーンの深度上限 (既定10)。
    /// 自動送信のたびに+1されてタブ間を受け継がれ、人間の手動入力で0に戻る
    #[serde(default)]
    pub max_chain: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct TabConfig {
    /// タブ名 (省略時はコマンド名から生成)
    pub name: Option<String>,
    /// 起動コマンド: "ssh root@host" または ["ssh", "root@host"]
    pub command: CommandSpec,
    /// 検出プロファイルの明示指定 (省略時はコマンド名から自動選択)
    pub profile: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CommandSpec {
    Line(String),
    Argv(Vec<String>),
}

impl CommandSpec {
    /// 空白区切り文字列 or 配列を argv に正規化。
    /// 空白を含むパスを使う場合は配列形式で書くこと
    pub fn argv(&self) -> Vec<String> {
        match self {
            CommandSpec::Line(s) => s.split_whitespace().map(str::to_string).collect(),
            CommandSpec::Argv(v) => v.clone(),
        }
    }
}

pub fn load() -> Option<Config> {
    let exe_side = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("config.json")));
    let candidates = [exe_side, Some(std::path::PathBuf::from("config.json"))];
    for path in candidates.into_iter().flatten() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(c) if !c.tabs.is_empty() => return Some(c),
            _ => continue,
        }
    }
    None
}
