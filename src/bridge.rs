//! model ブリッジ: OpenAI互換の `/chat/completions` を叩いて応答テキストを得る。
//!
//! DeepSeek(クラウド)・Ollama(ローカルQwen/DeepSeek)・OpenRouter 等、
//! **base_url と model を差し替えるだけ**で同じ経路で通る。SHIKISHA自身の正体は
//! 「既存AIを振る指揮者」なので、ここは "AI" ではなく「プロンプト→APIを叩いて
//! 指定先に書くだけ」の薄いパイプに徹する。
//!
//! 子プロセス (`--bridge`) は **env で接続先を受け取る**。鍵の復号や config/secrets の
//! 読み出しは親(本体)がやり、子は無知でよい (鍵をコマンドラインに出さない)。
//!   SHIKISHA_BRIDGE_URL     … base_url (例 https://api.deepseek.com/v1)
//!   SHIKISHA_BRIDGE_MODEL   … モデル名 (例 deepseek-chat / qwen2.5:7b)
//!   SHIKISHA_BRIDGE_HEADERS … 送信ヘッダ {"Authorization":"Bearer …"} 等 (JSON, 任意)
//!   SHIKISHA_BRIDGE_SYSTEM  … システムメッセージ (任意)

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::io::Read;

/// `--bridge` で子プロセスとして呼ばれたときの入口。
/// まずは一問一答: stdin を1回読み、応答本文を stdout に出して終わる。
pub fn run() -> Result<()> {
    let url = std::env::var("SHIKISHA_BRIDGE_URL").context("SHIKISHA_BRIDGE_URL 未設定")?;
    let model = std::env::var("SHIKISHA_BRIDGE_MODEL").context("SHIKISHA_BRIDGE_MODEL 未設定")?;
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
    let mut resp = req
        .send_json(&body)
        .map_err(|e| anyhow!("接続失敗 ({endpoint}): {e}"))?;
    let v: serde_json::Value = resp.body_mut().read_json().context("応答JSONが読めない")?;
    // OpenAI互換: choices[0].message.content。reasoning系の <think> は落とす
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow!("応答に content がありません: {v}"))?;
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
        // 既にフルパス/クエリ付きはそのまま
        let full = "https://x.openai.azure.com/openai/deployments/g/chat/completions?api-version=2024";
        assert_eq!(chat_endpoint(full), full);
    }

    #[test]
    fn strip_think_keeps_answer() {
        assert_eq!(strip_think("<think>reasoning</think>Answer"), "Answer");
        assert_eq!(strip_think("no think here"), "no think here");
    }
}
