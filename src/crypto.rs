//! マスターパスワードによる秘密情報の暗号化。DESIGN.md 10.1章。
//!
//! Argon2id で鍵導出 → AES-256-GCM で暗号化。Google Drive等に置いても
//! パスワードなしには復号できない (端末に紐づかないのでポータブル性を維持)。
//! "encryption": "none" 相当として、平文のまま使う選択も許す (自己責任)。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const MAGIC: &str = "shikisha-enc-v1";

/// 暗号化ファイルの中身 (JSON)。平文JSONと区別できるよう magic を持つ
#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub magic: String,
    /// Argon2id のソルト (base64)
    pub salt: String,
    /// AES-GCM のnonce (base64)
    pub nonce: String,
    /// 暗号文 (base64)
    pub data: String,
}

/// パスワードとソルトから256bit鍵を導出する
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let mut key = [0u8; 32];
    // 対話ログイン用途の推奨値 (64MiB, 3パス)
    let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| anyhow::anyhow!("{e}"))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("鍵導出に失敗: {e}"))?;
    Ok(key)
}

pub fn encrypt(plaintext: &str, password: &str) -> Result<Envelope> {
    // OSのCSPRNGから直接取得する (ソルト・nonceは毎回新規)
    use rand::TryRng as _;
    let mut salt = [0u8; 16];
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::SysRng
        .try_fill_bytes(&mut salt)
        .map_err(|e| anyhow::anyhow!("乱数生成に失敗: {e}"))?;
    rand::rngs::SysRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|e| anyhow::anyhow!("乱数生成に失敗: {e}"))?;

    let mut key = derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key));
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("暗号化に失敗: {e}"))?;
    key.zeroize();

    Ok(Envelope {
        magic: MAGIC.to_string(),
        salt: B64.encode(salt),
        nonce: B64.encode(nonce_bytes),
        data: B64.encode(ciphertext),
    })
}

pub fn decrypt(env: &Envelope, password: &str) -> Result<String> {
    if env.magic != MAGIC {
        bail!("暗号化ファイルの形式が不明です: {}", env.magic);
    }
    let salt = B64.decode(&env.salt).context("saltが不正")?;
    let nonce = B64.decode(&env.nonce).context("nonceが不正")?;
    let data = B64.decode(&env.data).context("暗号文が不正")?;

    let nonce: [u8; 12] = nonce
        .as_slice()
        .try_into()
        .context("nonceの長さが不正です")?;
    let mut key = derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key));
    let plain = cipher
        .decrypt(&Nonce::from(nonce), data.as_ref())
        .map_err(|_| anyhow::anyhow!("復号できません (パスワードが違うか、ファイルが壊れています)"));
    key.zeroize();
    String::from_utf8(plain?).context("復号結果がUTF-8ではありません")
}

/// ファイルが暗号化されているか (平文JSONと共存させるため中身で判定する)
pub fn is_encrypted(text: &str) -> bool {
    serde_json::from_str::<Envelope>(text)
        .map(|e| e.magic == MAGIC)
        .unwrap_or(false)
}

/// 暗号化ファイルを読んで復号する。平文ならそのまま返す
pub fn read_maybe_encrypted(
    path: &std::path::Path,
    password: Option<&str>,
) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("読み込めません: {}", path.display()))?;
    if !is_encrypted(&text) {
        return Ok(text);
    }
    let env: Envelope = serde_json::from_str(&text).context("暗号化ファイルの形式が不正")?;
    let Some(pw) = password else {
        bail!("暗号化されています。マスターパスワードが必要です");
    };
    decrypt(&env, pw)
}

/// 平文JSONを暗号化して書き戻す (アトミック置換)
pub fn encrypt_file(path: &std::path::Path, password: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("読み込めません: {}", path.display()))?;
    if is_encrypted(&text) {
        bail!("すでに暗号化されています");
    }
    let env = encrypt(&text, password)?;
    write_atomic(path, &serde_json::to_string_pretty(&env)?)
}

/// 一時ファイル→リネームで書き込む (Google Drive同期との競合対策。DESIGN 11章)
pub fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    // 置き場のフォルダ (config/ など) が無ければ作る。無いまま書くと
    // 一時ファイルの作成で失敗し、保存が「空の応答」で黙って落ちる
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("フォルダを作れません: {}", dir.display()))?;
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).with_context(|| format!("書き込めません: {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("置換できません: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_correct_password() {
        let env = encrypt(r#"{"notify":{}}"#, "correct horse").unwrap();
        assert_eq!(decrypt(&env, "correct horse").unwrap(), r#"{"notify":{}}"#);
    }

    #[test]
    fn wrong_password_fails_clearly() {
        let env = encrypt("secret", "right").unwrap();
        let err = decrypt(&env, "wrong").unwrap_err().to_string();
        assert!(err.contains("復号できません"), "{err}");
    }

    #[test]
    fn each_encryption_uses_fresh_salt_and_nonce() {
        let a = encrypt("same", "pw").unwrap();
        let b = encrypt("same", "pw").unwrap();
        assert_ne!(a.salt, b.salt, "ソルトは毎回変わる");
        assert_ne!(a.nonce, b.nonce, "nonceは毎回変わる");
        assert_ne!(a.data, b.data, "同じ平文でも暗号文は変わる");
    }

    #[test]
    fn plaintext_json_is_not_mistaken_for_encrypted() {
        assert!(!is_encrypted(r#"{"notify":{"slack":{"type":"slack"}}}"#));
        let env = encrypt("x", "pw").unwrap();
        assert!(is_encrypted(&serde_json::to_string(&env).unwrap()));
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let mut env = encrypt("secret", "pw").unwrap();
        // AES-GCMは認証付きなので改竄を検出できる
        let mut raw = B64.decode(&env.data).unwrap();
        raw[0] ^= 0xff;
        env.data = B64.encode(raw);
        assert!(decrypt(&env, "pw").is_err());
    }
}
