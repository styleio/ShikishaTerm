//! config.json: ワークスペース / タブ構成の定義。DESIGN.md 7.4章。
//! exe隣 (ポータブル配置) → カレント直下の順で探す。
//!
//! 用語: 「ワークスペース」= 切り替える単位 (仮想デスクトップ相当)。
//!       その中身を外部化したものが「ワークスペース定義ファイル」。
//!
//! 設定は役割で3種に分ける:
//!   config.json       … 全体設定 + ワークスペース一覧 (滅多に変えない)
//!   workspaces/*.json … ワークスペース定義ファイル (コピー・共有できる単位)
//!   secrets.json      … 資格情報 (暗号化可、共有厳禁)

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
    /// 全体共通の自動化 (例: "scripts/common" または "scripts/hooks.lua")
    #[serde(default)]
    pub automation: Option<String>,
    #[serde(default)]
    pub lua: Option<String>,
    /// 自動送信チェーンの深度上限 (既定10)。
    /// 自動送信のたびに+1されてタブ間を受け継がれ、人間の手動入力で0に戻る
    #[serde(default)]
    pub max_chain: Option<u32>,
    /// ボールが渡った先へ自動で画面を切り替えるか (既定: する)
    pub follow_ball: Option<bool>,
    /// 最後に開いていたワークスペースから始めるか (既定: する)
    pub restore_workspace: Option<bool>,
    /// ブラウザをターミナルに重ねるか (既定: 重ねる)。
    /// 切ると独立した窓になり、自分で動かせる代わりにタブらしくはならない
    pub browser_overlay: Option<bool>,
    /// 応答が終わったと確定するまでの待ち時間 (ms)。
    /// プロファイル側で指定があればそちらが優先される
    pub done_confirm_ms: Option<u64>,
    /// 左タブバーの幅 (桁)。省略時はタブ名に合わせて自動調整。
    /// 実行中は境界線のドラッグでも変更できる
    #[serde(default)]
    pub tab_bar_width: Option<u16>,
    /// 通知先の登録 (Luaはここに登録された宛先にしか送信できない)。
    /// トークン類は secrets.json (gitignore対象) に分離することを推奨
    #[serde(default)]
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// 通知先などの秘密情報を別ファイルから読み込む (例: "secrets.json")
    #[serde(default)]
    pub secrets: Option<String>,
    /// 自動化コードを書かせるAI ("claude" / "codex" / "gemini")。
    /// 空なら見つかったものを使う
    #[serde(default)]
    pub ai_engine: Option<String>,
    /// 自動化に与える能力 (ファイル・HTTP)。既定は空 = 何も許可しない。
    /// 玄人向け機能のためGUIからは編集しない
    #[serde(default)]
    pub capabilities: crate::caps::CapabilitySpec,
    /// スマホ等から見るリモートUI。既定は無効
    #[serde(default)]
    pub remote: RemoteSpec,
    /// 表示言語 ("ja" 等)。省略時はOSの設定に従う
    #[serde(default)]
    pub language: Option<String>,
}

/// リモートUIの設定。遠隔からAIを操作できる機能なので既定はオフ
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSpec {
    #[serde(default)]
    pub enabled: bool,
    /// "auto" (Tailscale→LANの順に探す) / "127.0.0.1" / 明示のIP
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_remote_port")]
    pub port: u16,
    /// プライベート網の外へ公開することを明示的に許可する
    #[serde(default)]
    pub allow_public: bool,
}

impl Default for RemoteSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_bind(),
            port: default_remote_port(),
            allow_public: false,
        }
    }
}

fn default_bind() -> String {
    "auto".into()
}
fn default_remote_port() -> u16 {
    8787
}

/// secrets.json: 資格情報だけを分離したファイル (共有厳禁)
#[derive(Debug, Deserialize, Default)]
pub struct Secrets {
    #[serde(default)]
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// HTTP窓口が使う認証情報 (スクリプトからは読めない)
    #[serde(default)]
    pub tokens: std::collections::HashMap<String, String>,
    /// リモートUIのトークン。設定するとURLが固定され、再ペアリングが不要になる
    #[serde(default)]
    pub remote_token: Option<String>,
}

impl Config {
    /// config.json の notify に secrets ファイルの内容をマージする
    /// (同名は secrets 側を優先)。secretsは暗号化されていることがある
    pub fn resolve_notify(
        &self,
        password: Option<&str>,
    ) -> (
        std::collections::HashMap<String, crate::notify::Destination>,
        Option<String>,
    ) {
        let mut map = self.notify.clone();
        let mut err = None;
        if let Some(path) = &self.secrets {
            let path = resolve_data_path(path);
            match crate::crypto::read_maybe_encrypted(&path, password)
                .and_then(|t| serde_json::from_str::<Secrets>(&t).context("secretsのJSONが不正"))
            {
                Ok(s) => map.extend(s.notify),
                Err(e) => err = Some(format!("secrets: {e:#}")),
            }
        }
        (map, err)
    }

    /// secretsファイルのパス (存在すれば)
    pub fn secrets_path(&self) -> Option<std::path::PathBuf> {
        self.secrets.as_deref().map(resolve_data_path)
    }

    /// リモートUIのトークン (secretsに書かれていれば使う)
    pub fn remote_token(&self, password: Option<&str>) -> Option<String> {
        let path = self.secrets_path()?;
        crate::crypto::read_maybe_encrypted(&path, password)
            .ok()
            .and_then(|t| serde_json::from_str::<Secrets>(&t).ok())
            .and_then(|s| s.remote_token)
            .filter(|t| t.len() >= 16)
    }

    /// HTTP窓口が使う認証情報を取り出す (スクリプトには渡さない)
    pub fn resolve_tokens(
        &self,
        password: Option<&str>,
    ) -> std::collections::HashMap<String, String> {
        let Some(path) = self.secrets_path() else {
            return Default::default();
        };
        crate::crypto::read_maybe_encrypted(&path, password)
            .ok()
            .and_then(|t| serde_json::from_str::<Secrets>(&t).ok())
            .map(|s| s.tokens)
            .unwrap_or_default()
    }
}

/// config.json 内のワークスペース項目。tabs直書き or 定義ファイル参照
#[derive(Debug, Deserialize)]
pub struct WorkspaceSpec {
    pub name: String,
    /// ワークスペース定義ファイルの参照 (例: "workspaces/projectx.json")
    #[serde(default)]
    pub file: Option<String>,
    /// インライン定義
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
    /// このワークスペース共通の自動化 (タブ指定が無い場合に使われる)
    #[serde(default)]
    pub automation: Option<String>,
    /// 一緒に開くブラウザ。自動化からは id で指す
    #[serde(default)]
    pub browsers: Vec<BrowserConfig>,
    #[serde(default)]
    pub lua: Option<String>,
}

/// ワークスペース定義ファイル (workspaces/*.json) の中身
#[derive(Debug, Deserialize)]
pub struct WorkspaceFile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
    /// このワークスペース共通の自動化
    #[serde(default)]
    pub automation: Option<String>,
    #[serde(default)]
    pub lua: Option<String>,
}

/// 一緒に開くブラウザ1台
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserConfig {
    /// 自動化から指す名前 (例: "br")
    pub id: String,
    /// 最初に開くURL。http/https のみ
    pub url: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct TabConfig {
    /// タブ名 (省略時はコマンド名から生成)
    pub name: Option<String>,
    /// 自動化から指す名前 (任意)。設定すると、タブ名を変えても
    /// スクリプトが壊れない。省略時はタブ名で指せる
    #[serde(default)]
    pub id: Option<String>,
    /// 起動コマンド: "ssh user@host" または ["ssh", "user@host"]
    pub command: CommandSpec,
    /// 検出プロファイルの明示指定 (省略時はコマンド名から自動選択)
    pub profile: Option<String>,
    /// 入力ロック (ソフトロック)。パイプラインの中間タブの誤操作防止。
    /// 実行時に Ctrl+B l / 🔒クリックで解除できる
    #[serde(default)]
    pub locked: bool,
    /// 子プロセスが終了したら自動で再起動する。
    /// SSH切断や、CLIツールの自己更新後の復帰に使う
    #[serde(default)]
    pub auto_restart: bool,
    /// 起動時の作業フォルダ。相対パスは設定ファイルの場所が基準。
    /// Docker/WSLの中のフォルダはこれでは指定できない (コマンド側の -w / --cd を使う)
    #[serde(default)]
    pub cwd: Option<String>,
    /// スクロールバック行数 (省略時5000)
    #[serde(default)]
    pub scrollback: Option<usize>,
    /// 文字コード ("shift_jis" 等)。省略時はUTF-8
    #[serde(default)]
    pub encoding: Option<String>,
    /// セッションログを logs/ に保存する
    #[serde(default)]
    pub log: bool,
    /// このタブ専用の自動化 (最優先で引き当てられる)。
    /// ディレクトリならイベント別ファイル方式、.lua なら関数定義方式
    #[serde(default)]
    pub automation: Option<String>,
    /// 旧称。automation が無いときに使われる
    #[serde(default)]
    pub lua: Option<String>,
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

impl Default for CommandSpec {
    /// 何も書かれていない状態。argv は空になる
    fn default() -> Self {
        CommandSpec::Argv(Vec::new())
    }
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
    /// ワークスペース階層の自動化
    pub automation: Option<String>,
    /// 一緒に開くブラウザ
    pub browsers: Vec<BrowserConfig>,
}

/// このタブはブラウザか。そうならURLを返す。
///
/// ssh/docker/wsl と同じで、コマンド文字列の頭で見分ける。
/// 設定画面の「種類」もこの規則を見ている
pub fn browser_url_of(argv: &[String]) -> Option<String> {
    let (head, rest) = argv.split_first()?;
    if !head.eq_ignore_ascii_case("browser") && !head.eq_ignore_ascii_case("web") {
        return None;
    }
    let url = rest.first()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

impl TabConfig {
    /// automation を優先し、旧称 lua にフォールバックする
    pub fn automation_path(&self) -> Option<String> {
        self.automation.clone().or_else(|| self.lua.clone())
    }
}

impl Config {
    pub fn automation_path(&self) -> Option<String> {
        self.automation.clone().or_else(|| self.lua.clone())
    }
}

pub struct FlatTab {
    pub cfg: TabConfig,
    /// 表示インデント段数 (0 = 親)
    pub depth: u16,
}

/// 自動化から一意に指せるかを確認する。
/// 同じ呼び名が複数あると送信先が定まらないため、起動時に知らせる
pub fn duplicate_keys(ws: &Workspace) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    for t in &ws.tabs {
        let key = t
            .cfg
            .id
            .clone()
            .or_else(|| t.cfg.name.clone())
            .unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        *seen.entry(key).or_insert(0) += 1;
    }
    let mut dups: Vec<String> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(k, _)| k)
        .collect();
    dups.sort();
    dups
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

/// exe隣 (ポータブル配置) を優先してデータファイルのパスを解決する。
/// 旧称の projects/ を指す設定も workspaces/ にフォールバックして読める (互換)
pub fn resolve_data_path(p: &str) -> std::path::PathBuf {
    let mut candidates = vec![p.to_string()];
    // projects/ ↔ workspaces/ を相互にフォールバック
    if let Some(rest) = p.strip_prefix("projects/") {
        candidates.push(format!("workspaces/{rest}"));
    } else if let Some(rest) = p.strip_prefix("workspaces/") {
        candidates.push(format!("projects/{rest}"));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf));
    for cand in &candidates {
        if let Some(dir) = &exe_dir {
            let full = dir.join(cand);
            if full.exists() {
                return full;
            }
        }
        let local = std::path::PathBuf::from(cand);
        if local.exists() {
            return local;
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
                    automation: None,
                    browsers: Vec::new(),
                });
            }
            return (out, errors);
        }
        for ws in &self.workspaces {
            let (tab_defs, file_name, file_lua): (Vec<TabConfig>, Option<String>, Option<String>) =
                match &ws.file {
                    Some(f) => match read_json::<WorkspaceFile>(&resolve_data_path(f)) {
                        Ok(p) => (p.tabs, p.name, p.automation.or(p.lua)),
                        Err(e) => {
                            errors.push(format!("{}: {e:#}", ws.name));
                            continue;
                        }
                    },
                    None => (ws.tabs.clone(), None, None),
                };
            let mut tabs = Vec::new();
            flatten(&tab_defs, 0, &mut tabs);
            out.push(Workspace {
                // 表示名はconfig側を優先し、空なら定義ファイルのnameを使う
                name: if ws.name.is_empty() {
                    file_name.unwrap_or_else(|| "UNNAMED".into())
                } else {
                    ws.name.clone()
                },
                tabs,
                // config側の指定を優先し、無ければ定義ファイル側を使う
                automation: ws.automation.clone().or_else(|| ws.lua.clone()).or(file_lua),
                browsers: ws.browsers.clone(),
            });
        }
        (out, errors)
    }
}

/// 設定ファイルの探索順 (exe隣 → カレント)
fn config_candidates() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("config.json")))
    {
        v.push(p);
    }
    v.push(std::path::PathBuf::from("config.json"));
    v
}

/// Web GUIの編集対象となる設定ファイルのパス。
/// 既存ファイルがあればそれを、無ければexe隣に新規作成する想定のパスを返す
/// 最後に開いていたワークスペース名の置き場。
///
/// config.json には書き戻さない。利用者が編集している最中に割り込むし、
/// 変更監視が自分の書き込みに反応して読み直しが走る
fn last_workspace_path() -> std::path::PathBuf {
    let mut p = config_file_path();
    p.set_file_name(".last-workspace");
    p
}

/// 最後に開いていたワークスペース名
pub fn load_last_workspace() -> Option<String> {
    let s = std::fs::read_to_string(last_workspace_path()).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// 開いているワークスペース名を覚える。失敗しても黙って諦める
/// (覚えられないことは、動かない理由にはならない)
pub fn save_last_workspace(name: &str) {
    let _ = crate::crypto::write_atomic(&last_workspace_path(), name);
}

pub fn config_file_path() -> std::path::PathBuf {
    let candidates = config_candidates();
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

pub fn load() -> Option<Config> {
    for path in config_candidates() {
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
    fn legacy_projects_path_falls_back_to_workspaces() {
        // 旧称 projects/ を指す既存設定でも workspaces/ 側を読めること
        let dir = std::env::temp_dir().join("shikisha-ws-compat");
        let ws_dir = dir.join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(ws_dir.join("x.json"), r#"{"tabs":[]}"#).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let resolved = resolve_data_path("projects/x.json");
        let ok = resolved.exists();

        std::env::set_current_dir(prev).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(ok, "projects/ 指定が workspaces/ にフォールバックする");
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
    fn missing_workspace_file_is_reported_not_fatal() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspaces":[
                {"name":"Bad","file":"workspaces/does-not-exist.json"},
                {"name":"Good","tabs":[{"name":"a","command":"a"}]}
            ]}"#,
        )
        .unwrap();
        let (ws, errs) = cfg.resolve_workspaces();
        assert_eq!(ws.len(), 1, "壊れた定義は飛ばして続行");
        assert_eq!(errs.len(), 1);
    }
}

#[cfg(test)]
mod browser_kind_tests {
    use super::browser_url_of;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// 「種類=ブラウザ」を、設定画面と本体が同じ規則で見分けること。
    ///
    /// 判定が2箇所に分かれると、画面ではブラウザに見えるのに
    /// 起動するとシェルが立つ、という食い違いが起きる
    #[test]
    fn a_browser_tab_is_told_apart_by_its_command() {
        assert_eq!(
            browser_url_of(&v(&["browser", "https://example.com/"])).as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            browser_url_of(&v(&["web", "http://127.0.0.1:8080/"])).as_deref(),
            Some("http://127.0.0.1:8080/"),
            "web という綴りも通す"
        );
        assert_eq!(
            browser_url_of(&v(&["BROWSER", "https://a.example/"])).as_deref(),
            Some("https://a.example/"),
            "大文字小文字は問わない"
        );

        // ブラウザではないもの
        assert!(browser_url_of(&v(&["cmd.exe"])).is_none());
        assert!(browser_url_of(&v(&["claude"])).is_none());
        assert!(browser_url_of(&v(&["browser"])).is_none(), "URLが無い");
        assert!(browser_url_of(&v(&["browser", "  "])).is_none(), "空白だけ");
        assert!(browser_url_of(&[]).is_none());
        // 「browser」で始まる別のコマンドを巻き込まない
        assert!(browser_url_of(&v(&["browserify", "x"])).is_none());
    }
}
