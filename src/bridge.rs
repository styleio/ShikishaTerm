//! model ブリッジ: OpenAI互換の `/chat/completions` を叩いて応答テキストを得る。
//!
//! DeepSeek(クラウド)・Ollama(ローカルQwen/DeepSeek)・OpenRouter 等、
//! **base_url と model を差し替えるだけ**で同じ経路で通る。SHIKISHA自身の正体は
//! 「既存AIを振る指揮者」なので、ここは "AI" ではなく「プロンプト→APIを叩いて
//! 応答を返すだけ」の薄いパイプに徹する。
//!
//! 討論での使い方は **サブプロセスにしない**。本体はGUIサブシステムで ConPTY 子に
//! コンソールI/Oが付かないため、`Command::SendPrompt` がモデルペインに来たら、本体が
//! スレッドで `complete()` を直接叩き、応答をタブ画面へ注入＋say.txt へ書く (in-process)。
//!
//! `--bridge` 子プロセスは端末直実行(パイプ)用に残す。env で接続先を受け取り、
//! stdin を1回読んで応答を stdout に返す (テスト・単発利用)。

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;

/// 解決済みのモデル接続先 (タブに持たせ、手番ごとに complete() へ渡す)
#[derive(Debug, Clone)]
pub struct ModelConn {
    pub url: String,
    pub model: String,
    pub headers: HashMap<String, String>,
    /// 討論での立場・人格。ブリッジはステートレスなので、毎手番これを system として
    /// 添えないと立場を忘れて話題がぶれる (討論参加者のときだけ設定される)
    pub persona: Option<String>,
}

/// `--bridge` 子プロセス (端末直実行・パイプ用)。stdin を1回読んで応答を stdout に返す
pub fn run() -> Result<()> {
    let url = std::env::var("SHIKISHA_BRIDGE_URL")
        .with_context(|| crate::i18n::t("err.bridge.url_unset"))?;
    let model = std::env::var("SHIKISHA_BRIDGE_MODEL")
        .with_context(|| crate::i18n::t("err.bridge.model_unset"))?;
    let headers = std::env::var("SHIKISHA_BRIDGE_HEADERS")
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .unwrap_or_default();
    let system = std::env::var("SHIKISHA_BRIDGE_SYSTEM").ok();
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt)?;
    let out = complete(&url, &model, &headers, system.as_deref(), prompt.trim())?;
    print!("{out}");
    Ok(())
}

/// OpenAI互換の `/chat/completions` を1回叩いて、応答本文を返す
pub fn complete(
    base_url: &str,
    model: &str,
    headers: &HashMap<String, String>,
    system: Option<&str>,
    user: &str,
) -> Result<String> {
    let endpoint = chat_endpoint(base_url);
    let mut messages = Vec::new();
    if let Some(s) = system.filter(|s| !s.trim().is_empty()) {
        messages.push(serde_json::json!({"role": "system", "content": s}));
    }
    messages.push(serde_json::json!({"role": "user", "content": user}));
    let body = serde_json::json!({ "model": model, "messages": messages, "stream": false });

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(180)))
        .build()
        .new_agent();
    let mut req = agent.post(&endpoint);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req.send_json(&body).map_err(|e| {
        anyhow!(crate::i18n::tp(
            "err.bridge.connect_failed",
            &[("endpoint", &endpoint), ("e", &e.to_string())]
        ))
    })?;
    let v: serde_json::Value = resp
        .body_mut()
        .read_json()
        .with_context(|| crate::i18n::t("err.bridge.bad_response_json"))?;
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            anyhow!(crate::i18n::tp(
                "err.bridge.no_content",
                &[("v", &v.to_string())]
            ))
        })?;
    Ok(strip_think(content).trim().to_string())
}

/// base_url に `/chat/completions` を必要なら足す。
/// 既にフルパス (Azure等) やクエリ付きならそのまま使う
fn chat_endpoint(base: &str) -> String {
    let b = base.trim();
    if b.contains("/chat/completions") || b.contains('?') {
        return b.to_string();
    }
    format!("{}/chat/completions", b.trim_end_matches('/'))
}

/// reasoning系モデルが混ぜる `<think>…</think>` を除去する (最後の閉じタグ以降を採る)
fn strip_think(s: &str) -> String {
    match s.rfind("</think>") {
        Some(i) => s[i + "</think>".len()..].to_string(),
        None => s.to_string(),
    }
}

/// 討論プロンプト末尾の `SHIKISHA_SAY=<path>` を取り出す (最後の一致。パスに空白可)
pub fn extract_say(s: &str) -> Option<String> {
    s.lines().rev().find_map(|l| {
        l.trim()
            .strip_prefix("SHIKISHA_SAY=")
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
    })
}

/// 解決済みプロバイダのキャッシュ (名前 → (base_url, headers))。
/// 本体がパスワードを持つ起動時/設定リロード時に埋める (secretの復号はそこだけ)
static PROVIDERS: Mutex<Option<HashMap<String, (String, HashMap<String, String>)>>> =
    Mutex::new(None);

/// config の providers を解決してキャッシュする
pub fn set_providers(cfg: &crate::config::Config, password: Option<&str>) {
    let mut m = HashMap::new();
    for name in cfg.providers.keys() {
        if let Some(resolved) = cfg.resolve_provider(name, password) {
            m.insert(name.clone(), resolved);
        }
    }
    if let Ok(mut g) = PROVIDERS.lock() {
        *g = Some(m);
    }
}

/// `model <provider>/<model>` なら、解決済みの接続先を返す (無ければ None)。
/// model名は "/" を含みうる(Ollamaタグ)ので最初の "/" で割る
pub fn launch_for(argv: &[String]) -> Option<ModelConn> {
    if argv.first().map(String::as_str) != Some("model") {
        return None;
    }
    let (provider, model) = argv.get(1)?.trim().split_once('/')?;
    let (url, headers) = {
        let g = PROVIDERS.lock().ok()?;
        g.as_ref()?.get(provider)?.clone()
    };
    Some(ModelConn {
        url,
        model: model.to_string(),
        headers,
        persona: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_appends_or_keeps() {
        assert_eq!(
            chat_endpoint("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            chat_endpoint("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
        let full = "https://x.openai.azure.com/openai/deployments/g/chat/completions?api-version=2024";
        assert_eq!(chat_endpoint(full), full);
    }

    #[test]
    fn strip_think_keeps_answer() {
        assert_eq!(strip_think("<think>reasoning</think>Answer"), "Answer");
        assert_eq!(strip_think("no think here"), "no think here");
    }

    #[test]
    fn extract_say_finds_marker() {
        assert_eq!(
            extract_say("hello\nSHIKISHA_SAY=C:/a b/say.txt\n").as_deref(),
            Some("C:/a b/say.txt")
        );
        assert_eq!(extract_say("no marker"), None);
    }
}
