//! config.json: ワークスペース / タブ構成の定義。DESIGN.md 7.4章。
//! exe隣 (ポータブル配置) → カレント直下の順で探す。
//!
//! 設定は役割で3ファイルに分ける方針:
//!   config.json     … 全体設定 + ワークスペース一覧 (滅多に変えない)
//!   projects/*.json … プロジェクト毎のタブ定義 (コピー・共有できる単位)
//!   secrets.enc     … 資格情報 (暗号化、共有厳禁 / Phase 5)

use anyhow::{Context as _, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// ワークスペース (プロジェクト) 一覧。仮想デスクトップのように切り替える
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSpec>,
    /// 後方互換: ワークスペースを使わない場合のタブ直書き
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

/// config.json 内のワークスペース定義。tabs直書き or 別ファイル参照
#[derive(Debug, Deserialize)]
pub struct WorkspaceSpec {
    pub name: String,
    /// 別ファイル参照 (例: "projects/projectx.json")
    #[serde(default)]
    pub file: Option<String>,
    /// インライン定義
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
}

/// projects/*.json の中身
#[derive(Debug, Deserialize)]
pub struct ProjectFile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TabConfig {
    /// タブ名 (省略時はコマンド名から生成)
    pub name: Option<String>,
    /// 起動コマンド: "ssh user@host" または ["ssh", "user@host"]
    pub command: CommandSpec,
    /// 検出プロファイルの明示指定 (省略時はコマンド名から自動選択)
    pub profile: Option<String>,
    /// 入力ロック (ソフトロック)。パイプラインの中間タブの誤操作防止。
    /// 実行時に Ctrl+B l / 🔒クリックで解除できる
    #[serde(default)]
    pub locked: bool,
    /// 表示上の子タブ (転送関係はLuaが決める。ここでは階層表示のみ)
    #[serde(default)]
    pub children: Vec<TabConfig>,
}

#[derive(Debug, Deserialize, Clone)]
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

/// 起動時に解決済みのワークスペース (タブは平坦化し、depthで階層を保持)
pub struct Workspace {
    pub name: String,
    pub tabs: Vec<FlatTab>,
}

pub struct FlatTab {
    pub cfg: TabConfig,
    /// 表示インデント段数 (0 = 親)
    pub depth: u16,
}

/// children を深さ優先で平坦化する (表示順とタブ番号を一致させる)
fn flatten(tabs: &[TabConfig], depth: u16, out: &mut Vec<FlatTab>) {
    for t in tabs {
        out.push(FlatTab {
            cfg: t.clone(),
            depth,
        });
        flatten(&t.children, depth + 1, out);
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("読み込めません: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("JSONが不正: {}", path.display()))
}

/// exe隣 (ポータブル配置) を優先してデータファイルのパスを解決する
pub fn resolve_data_path(p: &str) -> std::path::PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    {
        let cand = dir.join(p);
        if cand.exists() {
            return cand;
        }
    }
    std::path::PathBuf::from(p)
}

impl Config {
    /// ワークスペース定義を解決する。
    /// workspaces未定義なら tabs直書きを名前なしワークスペース1個として扱う
    pub fn resolve_workspaces(&self) -> (Vec<Workspace>, Vec<String>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        if self.workspaces.is_empty() {
            if !self.tabs.is_empty() {
                let mut tabs = Vec::new();
                flatten(&self.tabs, 0, &mut tabs);
                out.push(Workspace {
                    name: "DEFAULT".into(),
                    tabs,
                });
            }
            return (out, errors);
        }
        for ws in &self.workspaces {
            let (tab_defs, file_name): (Vec<TabConfig>, Option<String>) = match &ws.file {
                Some(f) => match read_json::<ProjectFile>(&resolve_data_path(f)) {
                    Ok(p) => (p.tabs, p.name),
                    Err(e) => {
                        errors.push(format!("{}: {e:#}", ws.name));
                        continue;
                    }
                },
                None => (ws.tabs.clone(), None),
            };
            let mut tabs = Vec::new();
            flatten(&tab_defs, 0, &mut tabs);
            out.push(Workspace {
                // 表示名はconfig側を優先し、空ならプロジェクトファイルのnameを使う
                name: if ws.name.is_empty() {
                    file_name.unwrap_or_else(|| "UNNAMED".into())
                } else {
                    ws.name.clone()
                },
                tabs,
            });
        }
        (out, errors)
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
            Ok(c) if !c.tabs.is_empty() || !c.workspaces.is_empty() => return Some(c),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_flatten_with_depth() {
        let cfg: Config = serde_json::from_str(
            r#"{"tabs":[
                {"name":"A","command":"a","children":[
                    {"name":"B","command":"b","locked":true},
                    {"name":"C","command":"c"}
                ]}
            ]}"#,
        )
        .unwrap();
        let (ws, errs) = cfg.resolve_workspaces();
        assert!(errs.is_empty());
        let tabs = &ws[0].tabs;
        assert_eq!(tabs.len(), 3, "親子が平坦化される");
        assert_eq!(tabs[0].depth, 0);
        assert_eq!(tabs[1].depth, 1);
        assert!(tabs[1].cfg.locked, "lockedが読める");
        assert_eq!(tabs[2].cfg.name.as_deref(), Some("C"));
    }

    #[test]
    fn inline_workspaces_are_resolved() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspaces":[
                {"name":"X","tabs":[{"name":"a","command":"a"}]},
                {"name":"Y","tabs":[{"name":"b","command":"b"}]}
            ]}"#,
        )
        .unwrap();
        let (ws, _) = cfg.resolve_workspaces();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[1].name, "Y");
    }

    #[test]
    fn missing_project_file_is_reported_not_fatal() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspaces":[
                {"name":"Bad","file":"projects/does-not-exist.json"},
                {"name":"Good","tabs":[{"name":"a","command":"a"}]}
            ]}"#,
        )
        .unwrap();
        let (ws, errs) = cfg.resolve_workspaces();
        assert_eq!(ws.len(), 1, "壊れた定義は飛ばして続行");
        assert_eq!(errs.len(), 1);
    }
}
