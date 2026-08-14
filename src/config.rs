//! config/config.json: ワークスペース / タブ構成の定義。DESIGN.md 7.4章。
//! exe隣の config フォルダ → カレントの config フォルダの順で探す。
//!
//! 用語: 「ワークスペース」= 切り替える単位 (仮想デスクトップ相当)。
//!       その中身を外部化したものが「ワークスペース定義ファイル」。
//!
//! 設定は役割で3種に分ける (利用者の持ち物は config フォルダにまとめる):
//!   config/config.json  … 全体設定 + ワークスペース一覧 (滅多に変えない)
//!   workspaces/*.json   … ワークスペース定義ファイル (コピー・共有できる単位)
//!   config/secrets.json … 資格情報 (暗号化可、共有厳禁)

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
    /// ブラウザ (WebView2) のデータ置き場。キャッシュとログイン状態が入る。
    ///   "local" (既定) … 各PCの %LOCALAPPDATA% (Drive同期しない・軽い)
    ///   "portable"      … アプリ隣の data\webview2 (Driveで全PC共有・ログインも共有)
    ///   その他          … その文字列を絶対パスとして使う
    #[serde(default)]
    pub browser_data: Option<String>,
    /// 中継画面 (スマホ操縦) の補助キー列。左から順に並ぶ。
    /// 使える名前: esc tab space enter backspace delete
    ///   left up down right home end pageup pagedown
    ///   f1〜f12 ctrl alt (ctrl/alt は固定トグル)。
    /// 省略時は cast_keys_default() を使う
    #[serde(default)]
    pub cast_keys: Option<Vec<String>>,
    /// model ブリッジ (OpenAI互換API) の接続先。名前 → {base_url, api_key, headers}。
    /// 討論やブラウザ操作で cheap/local モデル(DeepSeek/Qwen/Ollama等)を使うための橋。
    /// `model <名前>/<モデル>` タブがここを参照して起動する
    #[serde(default)]
    pub providers: std::collections::HashMap<String, ProviderSpec>,
}

/// OpenAI互換APIの接続先 (DeepSeekクラウド / Ollamaローカル / OpenRouter / Azure 等)。
/// 「接続先(base_url＋認証) × モデル名」の2軸で、クラウド/ローカルもモデル種別も直交して扱える
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderSpec {
    /// OpenAI互換 base URL (例: https://api.deepseek.com/v1, http://localhost:11434/v1)。
    /// Azure等のフルパス/クエリ付きはそのまま使う
    #[serde(default)]
    pub base_url: String,
    /// 認証キー。"@名前" で secrets.json の tokens を参照する (直値も可)。
    /// headers 未指定なら Authorization: Bearer <解決値> ヘッダになる。
    /// ローカル(Ollama)等で不要なら省略
    #[serde(default)]
    pub api_key: Option<String>,
    /// 送信ヘッダの明示指定 (Azure の `api-key` や独自ゲートウェイ用)。値も "@名前" で
    /// secrets 参照可。指定すると api_key の既定 Bearer は使わずこちらを送る
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
}

/// 補助キー列の既定の並び。よく使う Enter/Space/⌫ と矢印を前に、
/// F1〜F12 や Ctrl/Alt は後ろへ (横スクロールで届く)。
/// 使う人が config の cast_keys で自由に差し替えられる
pub fn cast_keys_default() -> Vec<String> {
    [
        "esc", "tab", "left", "up", "down", "right", "space", "enter", "backspace", "ctrl", "alt",
        "home", "end", "pageup", "pagedown", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9",
        "f10", "f11", "f12",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 補助キー列の並びを設定から得る (未設定なら既定)。中継画面のクライアントへ渡す
pub fn cast_keys() -> Vec<String> {
    load()
        .and_then(|c| c.cast_keys)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(cast_keys_default)
}

/// WebView2 のデータ置き場を設定から決める。DriveのキャッシュチャーンやEBWebView
/// の同期通知を避けるため、既定は同期されないローカル (%LOCALAPPDATA%)
pub fn browser_data_dir() -> std::path::PathBuf {
    let mode = load()
        .and_then(|c| c.browser_data)
        .unwrap_or_default();
    match mode.trim() {
        "portable" => root_dir().join("data").join("webview2"),
        "" | "local" => local_appdata().join("ShikishaTerm").join("webview2"),
        other => std::path::PathBuf::from(other),
    }
}

fn local_appdata() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root_dir().join("data"))
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
    /// 各トークンの説明 (GUIの一覧に出す。値そのものは出さない)
    #[serde(default)]
    pub descriptions: std::collections::HashMap<String, String>,
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
        if let Some(path) = self.secrets_path().filter(|p| p.exists()) {
            match crate::crypto::read_maybe_encrypted(&path, password)
                .and_then(|t| serde_json::from_str::<Secrets>(&t).context("secretsのJSONが不正"))
            {
                Ok(s) => map.extend(s.notify),
                Err(e) => err = Some(format!("secrets: {e:#}")),
            }
        }
        (map, err)
    }

    /// secretsファイルのパス。相対パスは config.json の隣として解決する。
    ///
    /// 明示指定が無ければ既定で config/secrets.json を指す。こうしないと、
    /// 設定GUIが既定の場所に作った秘密を本体が読み込めない (登録したのに
    /// 使えない) ことになる。ファイルが無ければ読み手側が空として扱う
    pub fn secrets_path(&self) -> Option<std::path::PathBuf> {
        let p = self.secrets.as_deref().unwrap_or("secrets.json");
        if std::path::Path::new(p).is_absolute() {
            return Some(std::path::PathBuf::from(p));
        }
        let mut c = config_file_path();
        c.set_file_name(p);
        Some(c)
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

    /// provider名から接続先を解決する。返り値は (base_url, 送信ヘッダ)。
    /// 値中の "@名前" は secrets.json の tokens を展開する。
    /// headers 未指定で api_key があれば Authorization: Bearer を組み立てる。
    /// これを model ブリッジ子プロセスの env に渡す (鍵の復号は親=ここだけ)
    pub fn resolve_provider(
        &self,
        name: &str,
        password: Option<&str>,
    ) -> Option<(String, std::collections::HashMap<String, String>)> {
        let p = self.providers.get(name)?;
        if p.base_url.trim().is_empty() {
            return None;
        }
        let tokens = self.resolve_tokens(password);
        let deref = |v: &str| -> String {
            match v.strip_prefix('@') {
                Some(k) => tokens.get(k).cloned().unwrap_or_default(),
                None => v.to_string(),
            }
        };
        let mut headers = std::collections::HashMap::new();
        if !p.headers.is_empty() {
            for (k, v) in &p.headers {
                headers.insert(k.clone(), deref(v));
            }
        } else if let Some(key) = p.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            headers.insert("Authorization".into(), format!("Bearer {}", deref(key)));
        }
        Some((p.base_url.trim().to_string(), headers))
    }
}

// ── 秘密ストア (GitHub Secrets 相当) ─────────────────────────
// キー名で参照し、値そのものは決して返さない。書けば暗号化 (パスワードがあれば)。
// notify や remote_token など他の項目を壊さないよう、丸ごとの JSON を読み書きする

/// secrets ファイルを JSON として読む (無ければ空)
fn read_secrets_value(
    path: &std::path::Path,
    password: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let text = crate::crypto::read_maybe_encrypted(path, password)?;
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({})))
}

/// secrets ファイルを書き戻す (パスワードがあれば暗号化)
fn write_secrets_value(
    path: &std::path::Path,
    password: Option<&str>,
    root: &serde_json::Value,
) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(root)?;
    match password {
        Some(pw) if !pw.is_empty() => {
            let env = crate::crypto::encrypt(&json, pw)?;
            crate::crypto::write_atomic(path, &serde_json::to_string_pretty(&env)?)
        }
        _ => crate::crypto::write_atomic(path, &json),
    }
}

/// キー名として妥当か (英数字と _ - . のみ)。すり替えや変な文字を弾く
pub fn valid_secret_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// 秘密の一覧 (キーと説明のみ)。**値は決して返さない**
pub fn list_secrets(
    path: &std::path::Path,
    password: Option<&str>,
) -> anyhow::Result<Vec<(String, String)>> {
    let root = read_secrets_value(path, password)?;
    let descs = root.get("descriptions").and_then(|v| v.as_object());
    let mut out: Vec<(String, String)> = root
        .get("tokens")
        .and_then(|v| v.as_object())
        .map(|t| {
            t.keys()
                .map(|k| {
                    let d = descs
                        .and_then(|d| d.get(k))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (k.clone(), d)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// 秘密を追加/更新する (write-only。保存したら値は読み戻せない)
pub fn upsert_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
    desc: &str,
    value: &str,
) -> anyhow::Result<()> {
    if !valid_secret_key(key) {
        anyhow::bail!("キーは英数字と _ - . のみ使えます");
    }
    let mut root = read_secrets_value(path, password)?;
    if !root.get("tokens").map(|v| v.is_object()).unwrap_or(false) {
        root["tokens"] = serde_json::json!({});
    }
    root["tokens"][key] = serde_json::json!(value);
    if !root
        .get("descriptions")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        root["descriptions"] = serde_json::json!({});
    }
    root["descriptions"][key] = serde_json::json!(desc);
    write_secrets_value(path, password, &root)
}

/// 秘密を削除する
pub fn delete_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
) -> anyhow::Result<()> {
    let mut root = read_secrets_value(path, password)?;
    if let Some(t) = root.get_mut("tokens").and_then(|v| v.as_object_mut()) {
        t.remove(key);
    }
    if let Some(d) = root.get_mut("descriptions").and_then(|v| v.as_object_mut()) {
        d.remove(key);
    }
    write_secrets_value(path, password, &root)
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
    /// このワークスペースのラリーが使ってよい秘密のキー (既定は空 = 全拒否)
    #[serde(default)]
    pub secrets_allow: Vec<String>,
    /// 危険承知で全ての秘密を許可する
    #[serde(default)]
    pub secrets_allow_all: bool,
    /// 停止条件 (審判)。ブラウザ操作モード等の内蔵司令塔が読む
    #[serde(default)]
    pub stops: Vec<StopCond>,
    /// AI×AI 議論の設定 (あれば内蔵の議論オーケストレータを各AIタブに入れる)
    #[serde(default)]
    pub discuss: Option<DiscussSpec>,
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
    #[serde(default)]
    pub secrets_allow: Vec<String>,
    #[serde(default)]
    pub secrets_allow_all: bool,
    #[serde(default)]
    pub stops: Vec<StopCond>,
    #[serde(default)]
    pub discuss: Option<DiscussSpec>,
}

/// AI×AI(N者)の議論設定。ワークスペース単位。内蔵の議論オーケストレータが読む。
/// 参加者(agents)は手番の順。round-robinで回し、max_rounds でjudge(いれば)に裁定させる
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DiscussSpec {
    /// 参加AIタブの id (手番の順)
    #[serde(default)]
    pub agents: Vec<String>,
    /// 手番の回し方。今は "round-robin" のみ
    #[serde(default = "default_order")]
    pub order: String,
    /// 各参加者が話す最大周回数 (これを超えたら審判/終了へ)
    #[serde(default = "default_rounds")]
    pub max_rounds: u32,
    /// 審判(レフェリー)のタブ id。省略時は周回上限で「議論終了」として畳む
    #[serde(default)]
    pub judge: Option<String>,
    /// 審判の出し方: "winner"(勝敗) / "synthesis"(統合)。既定は winner
    #[serde(default = "default_verdict")]
    pub verdict: String,
    /// 司会(moderator)のタブ id。order="moderated" のとき、次の話者を指名する
    #[serde(default)]
    pub moderator: Option<String>,
    /// 各タブの立場・人格 (タブ id → ペルソナ文)。
    /// 例: {"bos":"あなたはブラザーフッド・オブ・スティール...", ...}。
    /// 開始時にそのAIへ伝える。空なら素のAI(ニュートラル)
    #[serde(default)]
    pub personas: std::collections::HashMap<String, String>,
}

fn default_order() -> String {
    "round-robin".into()
}
fn default_rounds() -> u32 {
    6
}
fn default_verdict() -> String {
    "winner".into()
}

/// 停止条件 (審判)。ワークスペース単位で持つ。上から評価し最初に成立したものが勝つ。
/// 「この共同作業はいつ終わりか (成功/失敗)」の定義。参加者(タブ)をまたいで指定できる
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct StopCond {
    /// 監視の種類: screen|css|xpath|console|rounds|time|tokens
    pub when: String,
    /// 監視するタブの id (screen/css/xpath/console 用。省略時は操作対象)
    #[serde(default)]
    pub tab: Option<String>,
    /// 文字列パターン (screen=ブラウザ本文, console=タブ出力)
    #[serde(default)]
    pub pattern: Option<String>,
    /// セレクタ (css/xpath 用。"#id" か xpath 文字列)
    #[serde(default)]
    pub sel: Option<String>,
    /// しきい値 (rounds=回数, tokens=概算)
    #[serde(default)]
    pub max: Option<i64>,
    /// 秒 (time 用)
    #[serde(default)]
    pub sec: Option<i64>,
    /// 判定: "success" | "fail"
    #[serde(default)]
    pub outcome: String,
    /// 終了コード
    #[serde(default)]
    pub code: i32,
    /// 理由 (人が読む・記録に残る)
    #[serde(default)]
    pub reason: Option<String>,
}

/// 停止条件の並びを、内蔵司令塔へ渡す Lua テーブルリテラルにする。
/// 文字列は %q 相当で安全に引用する (Lua の string.format ではなくRust側で)
pub fn stops_to_lua(stops: &[StopCond]) -> String {
    fn q(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }
    let mut b = String::from("{\n");
    for s in stops {
        if s.when.trim().is_empty() {
            continue;
        }
        b.push_str("  { when=");
        b.push_str(&q(&s.when));
        if let Some(t) = &s.tab {
            b.push_str(", tab=");
            b.push_str(&q(t));
        }
        if let Some(p) = &s.pattern {
            b.push_str(", pattern=");
            b.push_str(&q(p));
        }
        if let Some(sel) = &s.sel {
            b.push_str(", sel=");
            b.push_str(&q(sel));
        }
        if let Some(m) = s.max {
            b.push_str(&format!(", max={m}"));
        }
        if let Some(sec) = s.sec {
            b.push_str(&format!(", sec={sec}"));
        }
        let outcome = if s.outcome.is_empty() { "success" } else { &s.outcome };
        b.push_str(", outcome=");
        b.push_str(&q(outcome));
        b.push_str(&format!(", code={}", s.code));
        b.push_str(", reason=");
        b.push_str(&q(s.reason.as_deref().unwrap_or("")));
        b.push_str(" },\n");
    }
    b.push('}');
    b
}

/// ブラウザの上に出す操作。どれを出すかだけを持つ。
///
/// 既定はすべて false = 何も出さない。要らないプロジェクトの画面を
/// 勝手に狭めない。使う人が1つずつ選ぶ
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
pub struct NavSpec {
    #[serde(default)]
    pub back: bool,
    #[serde(default)]
    pub forward: bool,
    #[serde(default)]
    pub reload: bool,
    /// URL欄。人が任意のページへ移る手段
    #[serde(default)]
    pub url: bool,
}

impl NavSpec {
    /// 1つも出さないなら、バーそのものが要らない
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// 全部出す。`browser_nav(id)` のように指定を省いたとき用
    pub fn all() -> Self {
        Self { back: true, forward: true, reload: true, url: true }
    }
}

/// ページの下に出す帯。
///
/// 書いてあること自体が「出す」の意味。バーと違って文言が要るので、
/// 出す・出さないをまとめた真偽値では足りない
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct AskSpec {
    /// 帯の左に出る文言
    #[serde(default)]
    pub text: String,
    /// ボタンの字。空なら既定の言葉を使う
    #[serde(default)]
    pub label: String,
}

/// 一緒に開くブラウザ1台
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserConfig {
    /// 自動化から指す名前 (例: "br")
    pub id: String,
    /// 最初に開くURL。http/https のみ
    pub url: String,
    /// ブラウザのプロファイル名 (省略時 "default")。private が true なら無視
    #[serde(default)]
    pub browser_profile: Option<String>,
    /// プライベート(使い捨て)ブラウザ
    #[serde(default)]
    pub private: bool,
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
    /// ブラウザ操作モード。ここに操作対象のブラウザタブの id を書くと、
    /// このタブは「ブラウザを操作するエージェント」になる。内蔵の司令塔が動き、
    /// ゴールは入力欄に打つ (設定には書かない)。automation より優先
    #[serde(default)]
    pub drives: Option<String>,
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
    /// ブラウザのタブに出す操作 (戻る/進む/更新/URL欄)。
    /// 端末のタブでは意味がないので読まれない
    #[serde(default)]
    pub nav: Option<NavSpec>,
    /// ブラウザのタブの下に出す帯 (文言とボタンの字)
    #[serde(default)]
    pub ask: Option<AskSpec>,
    /// ブラウザのプロファイル名 (Cookie・ログインの箱)。省略時は "default"。
    /// 同じ名前のタブ同士でログイン状態を共有する。Chrome の「人物」と同じ発想。
    /// private が true のときは無視される
    #[serde(default)]
    pub browser_profile: Option<String>,
    /// プライベート(使い捨て)ブラウザ。true なら Cookie・履歴を残さない一時領域で
    /// 開き、閉じたら消す。このとき browser_profile は使われない
    #[serde(default)]
    pub private: bool,
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
    /// このワークスペースのラリーが使ってよい秘密のキー (既定は空 = 全拒否)
    pub secrets_allow: Vec<String>,
    /// 危険承知で全ての秘密を許可する
    pub secrets_allow_all: bool,
    /// 停止条件 (審判)
    pub stops: Vec<StopCond>,
    /// AI×AI 議論の設定
    pub discuss: Option<DiscussSpec>,
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
                    secrets_allow: Vec::new(),
                    secrets_allow_all: false,
                    stops: Vec::new(),
                    discuss: None,
                });
            }
            return (out, errors);
        }
        for ws in &self.workspaces {
            #[allow(clippy::type_complexity)]
            #[allow(clippy::type_complexity)]
            let (tab_defs, file_name, file_lua, file_secrets, file_stops, file_discuss): (
                Vec<TabConfig>,
                Option<String>,
                Option<String>,
                (Vec<String>, bool),
                Vec<StopCond>,
                Option<DiscussSpec>,
            ) = match &ws.file {
                Some(f) => match read_json::<WorkspaceFile>(&resolve_data_path(f)) {
                    Ok(p) => (
                        p.tabs,
                        p.name,
                        p.automation.or(p.lua),
                        (p.secrets_allow, p.secrets_allow_all),
                        p.stops,
                        p.discuss,
                    ),
                    Err(e) => {
                        errors.push(format!("{}: {e:#}", ws.name));
                        continue;
                    }
                },
                None => (ws.tabs.clone(), None, None, (Vec::new(), false), Vec::new(), None),
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
                // config側の指定を優先し、無ければ定義ファイル側を使う
                secrets_allow: if ws.secrets_allow.is_empty() {
                    file_secrets.0
                } else {
                    ws.secrets_allow.clone()
                },
                secrets_allow_all: ws.secrets_allow_all || file_secrets.1,
                // config側の指定を優先し、無ければ定義ファイル側を使う
                stops: if ws.stops.is_empty() { file_stops } else { ws.stops.clone() },
                discuss: ws.discuss.clone().or(file_discuss),
            });
        }
        (out, errors)
    }
}

/// ポータブル配置のルート (exe と各フォルダが並ぶ場所)。
/// data / logs / config はここの下に置く
pub fn root_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// 設定ファイルの探索順。新配置 (config フォルダ) を優先し、
/// 旧配置 (ルート直下の config.json) も読める。カレント基準も後ろに足す
fn config_candidates() -> Vec<std::path::PathBuf> {
    let root = root_dir();
    vec![
        root.join("config").join("config.json"), // 新: exe隣の config フォルダ
        root.join("config.json"),                // 旧: exe隣の直下 (移行対象)
        std::path::PathBuf::from("config/config.json"), // 新: カレント
        std::path::PathBuf::from("config.json"),        // 旧: カレント
    ]
}

/// 旧配置 (ルート直下の config.json / secrets.json) を新しい config フォルダへ移す。
/// 一度だけ。移せなくても読み込みは旧配置にフォールバックするので致命ではない
pub fn migrate_legacy_config() {
    let root = root_dir();
    let new_cfg = root.join("config").join("config.json");
    let old_cfg = root.join("config.json");
    if new_cfg.exists() || !old_cfg.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(new_cfg.parent().unwrap());
    // 同一ボリュームなら rename、駄目ならコピーして消す
    if std::fs::rename(&old_cfg, &new_cfg).is_err()
        && std::fs::copy(&old_cfg, &new_cfg).is_ok()
    {
        let _ = std::fs::remove_file(&old_cfg);
    }
    // secrets.json も隣にあれば一緒に移す
    let (old_s, new_s) = (root.join("secrets.json"), root.join("config").join("secrets.json"));
    if old_s.exists() && !new_s.exists() {
        if std::fs::rename(&old_s, &new_s).is_err() && std::fs::copy(&old_s, &new_s).is_ok() {
            let _ = std::fs::remove_file(&old_s);
        }
    }
}

/// Web GUIの編集対象となる設定ファイルのパス。
/// 既存ファイルがあればそれを、無ければexe隣に新規作成する想定のパスを返す
/// 状態ファイル (人が編集しないもの) の置き場。ルートの data フォルダにまとめる。
/// ルートのファイルは exe だけ (フォルダは config / data / logs / lang / workspaces / scripts)
pub fn state_path(name: &str) -> std::path::PathBuf {
    let p = root_dir().join("data");
    let _ = std::fs::create_dir_all(&p);
    p.join(name)
}

/// ログの置き場。カレントディレクトリではなくルートの logs フォルダに固定する
/// (起動のしかたでログの行き先が変わると、クラッシュの記録が迷子になる)
pub fn logs_dir() -> std::path::PathBuf {
    let p = root_dir().join("logs");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// 最後に開いていたワークスペース名の置き場。
///
/// config.json には書き戻さない。利用者が編集している最中に割り込むし、
/// 変更監視が自分の書き込みに反応して読み直しが走る
fn last_workspace_path() -> std::path::PathBuf {
    state_path("last-workspace")
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
    fn secret_store_roundtrips_and_never_reveals_values() {
        let dir = std::env::temp_dir().join("shikisha-secrets-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("secrets.json");

        // 平文で登録 → 一覧はキーと説明だけ (値は出ない)
        upsert_secret(&path, None, "diary_saas", "日記SaaSのログイン", "hunter2秘密").unwrap();
        let list = list_secrets(&path, None).unwrap();
        assert_eq!(list, vec![("diary_saas".into(), "日記SaaSのログイン".into())]);
        // 実際に値は保存されている (resolve_tokens 経由で取れる)
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("hunter2秘密"), "値が保存されていない");
        // だが一覧APIには値が一切現れない
        assert!(!format!("{list:?}").contains("hunter2"), "一覧に値が漏れている");

        // 更新・追加
        upsert_secret(&path, None, "diary_saas", "説明更新", "newpass").unwrap();
        upsert_secret(&path, None, "github", "PAT", "ghp_xxx").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["diary_saas".to_string(), "github".to_string()], "整列済み");

        // 削除
        delete_secret(&path, None, "github").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["diary_saas".to_string()]);

        // マスターパスワードありなら暗号化されて保存される
        upsert_secret(&path, Some("master"), "enc_key", "暗号化テスト", "topsecret").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(crate::crypto::is_encrypted(&raw), "パスワードありなら暗号化される");
        assert!(!raw.contains("topsecret"), "暗号化後は生値が見えない");
        // 正しいパスワードなら一覧が読め、値はやはり出ない
        let list = list_secrets(&path, Some("master")).unwrap();
        assert!(list.iter().any(|(k, _)| k == "enc_key"));
        // 不正なキーは弾く
        assert!(upsert_secret(&path, None, "../evil", "x", "y").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// ブラウザの見た目を、Luaを書かずに設定だけで決められること。
    ///
    /// 書いてある項目だけが出る。書いていないものは既定 (出さない) になる
    #[test]
    fn a_browser_tab_can_be_dressed_from_the_settings_alone() {
        let t: super::TabConfig = serde_json::from_str(
            r#"{
                "name": "解析",
                "command": "browser https://example.com/",
                "nav": { "reload": true, "url": true },
                "ask": { "text": "読み終わったら押してください", "label": "解析する" }
            }"#,
        )
        .unwrap();
        let nav = t.nav.expect("上のバーが読めていない");
        assert!(nav.reload && nav.url, "書いたものが出ない");
        assert!(!nav.back && !nav.forward, "書いていないものまで出る");
        let ask = t.ask.expect("帯が読めていない");
        assert_eq!(ask.label, "解析する");

        // 何も書かなければ、どちらも出さない
        let bare: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://example.com/"}"#).unwrap();
        assert!(bare.nav.is_none() && bare.ask.is_none());

        // 帯は書いてあること自体が「出す」の意味。中身が空でも出す
        let empty: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://x/", "ask": {}}"#).unwrap();
        assert!(empty.ask.is_some(), "空の帯を無かったことにしている");
    }
}
