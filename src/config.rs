//! config.json: タブ構成の定義。DESIGN.md 7.4章のスキーマのサブセット (Phase 3時点)。
//! exe隣 (ポータブル配置) → カレント直下の順で探す。

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
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

impl TabConfig {
    /// 実行argvに正規化する。
    /// "user@example.com" のような user@host 単体はPuTTY流の
    /// SSH接続先指定とみなし、自動的に `ssh` を補う
    pub fn resolved_argv(&self) -> Vec<String> {
        let argv = self.command.argv();
        if argv.len() == 1 && looks_like_ssh_target(&argv[0]) {
            return vec!["ssh".to_string(), argv[0].clone()];
        }
        argv
    }
}

fn looks_like_ssh_target(s: &str) -> bool {
    s.contains('@') && !s.contains('/') && !s.contains('\\')
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(command: &str) -> TabConfig {
        TabConfig {
            name: None,
            command: CommandSpec::Line(command.to_string()),
            profile: None,
        }
    }

    #[test]
    fn user_at_host_becomes_ssh() {
        assert_eq!(
            tab("user@example.com").resolved_argv(),
            vec!["ssh", "user@example.com"]
        );
    }

    #[test]
    fn explicit_command_is_kept() {
        assert_eq!(
            tab("ssh user@example.com").resolved_argv(),
            vec!["ssh", "user@example.com"]
        );
        assert_eq!(tab("claude").resolved_argv(), vec!["claude"]);
        // パスに@を含んでもコマンドとして扱う
        assert_eq!(
            tab("C:/tools/foo@2/run.exe").resolved_argv(),
            vec!["C:/tools/foo@2/run.exe"]
        );
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
