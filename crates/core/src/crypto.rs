//! Encrypting secrets with a master password. DESIGN.md section 10.1.
//!
//! Key derivation via Argon2id -> encryption via AES-256-GCM. Even placed on
//! Google Drive etc., it can't be decrypted without the password (not tied
//! to a device, so portability is preserved).
//! Also allows using plaintext as-is, equivalent to "encryption": "none"
//! (at the user's own risk).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const MAGIC: &str = "shikisha-enc-v1";

/// Contents of an encrypted file (JSON). Carries a magic value so it can be
/// distinguished from plaintext JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub magic: String,
    /// Argon2id salt (base64)
    pub salt: String,
    /// AES-GCM nonce (base64)
    pub nonce: String,
    /// Ciphertext (base64)
    pub data: String,
}

/// Derive a 256-bit key from a password and salt.
///
/// Argon2id is deliberately slow (~hundreds of ms). The secrets file is one
/// envelope, but startup reads it several times (providers, tokens, notify,
/// remote token), and re-deriving each time added up to seconds of a black
/// screen after the password prompt. Memoize the last (password, salt) → key so
/// only the first derivation pays the cost and the rest are instant AES.
///
/// The cached key and password live for the process lifetime, which is no worse
/// than the master password we already hold resident in memory for the session.
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(String, Vec<u8>, [u8; 32])>> = Mutex::new(None);
    if let Ok(g) = CACHE.lock() {
        if let Some((p, s, k)) = g.as_ref() {
            if p == password && s.as_slice() == salt {
                return Ok(*k);
            }
        }
    }

    use argon2::{Algorithm, Argon2, Params, Version};
    let mut key = [0u8; 32];
    // Recommended values for interactive-login use (64MiB, 3 passes)
    let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| anyhow::anyhow!("{e}"))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| {
            anyhow::anyhow!(crate::i18n::tp(
                "err.crypto.key_derivation_failed",
                &[("e", &format!("{e}"))]
            ))
        })?;
    if let Ok(mut g) = CACHE.lock() {
        *g = Some((password.to_string(), salt.to_vec(), key));
    }
    Ok(key)
}

pub fn encrypt(plaintext: &str, password: &str) -> Result<Envelope> {
    // Obtained directly from the OS's CSPRNG (a fresh salt and nonce every time)
    use rand::TryRng as _;
    let mut salt = [0u8; 16];
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::SysRng.try_fill_bytes(&mut salt).map_err(|e| {
        anyhow::anyhow!(crate::i18n::tp(
            "err.crypto.random_failed",
            &[("e", &format!("{e}"))]
        ))
    })?;
    rand::rngs::SysRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|e| {
            anyhow::anyhow!(crate::i18n::tp(
                "err.crypto.random_failed",
                &[("e", &format!("{e}"))]
            ))
        })?;

    let mut key = derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key));
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), plaintext.as_bytes())
        .map_err(|e| {
            anyhow::anyhow!(crate::i18n::tp(
                "err.crypto.encrypt_failed",
                &[("e", &format!("{e}"))]
            ))
        })?;
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
        bail!(crate::i18n::tp(
            "err.crypto.unknown_format",
            &[("magic", &env.magic)]
        ));
    }
    let salt = B64
        .decode(&env.salt)
        .with_context(|| crate::i18n::t("err.crypto.bad_salt"))?;
    let nonce = B64
        .decode(&env.nonce)
        .with_context(|| crate::i18n::t("err.crypto.bad_nonce"))?;
    let data = B64
        .decode(&env.data)
        .with_context(|| crate::i18n::t("err.crypto.bad_ciphertext"))?;

    let nonce: [u8; 12] = nonce
        .as_slice()
        .try_into()
        .with_context(|| crate::i18n::t("err.crypto.bad_nonce_length"))?;
    let mut key = derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key));
    let plain = cipher
        .decrypt(&Nonce::from(nonce), data.as_ref())
        .map_err(|_| anyhow::anyhow!(crate::i18n::t("err.crypto.decrypt_failed")));
    key.zeroize();
    String::from_utf8(plain?).with_context(|| crate::i18n::t("err.crypto.not_utf8"))
}

/// Whether the file is encrypted (determined from its content, so it can
/// coexist with plaintext JSON).
pub fn is_encrypted(text: &str) -> bool {
    serde_json::from_str::<Envelope>(text)
        .map(|e| e.magic == MAGIC)
        .unwrap_or(false)
}

/// Read an encrypted file and decrypt it. If it's plaintext, return it
/// as-is.
pub fn read_maybe_encrypted(
    path: &std::path::Path,
    password: Option<&str>,
) -> Result<String> {
    let text = std::fs::read_to_string(path).with_context(|| {
        crate::i18n::tp(
            "err.crypto.read_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
    if !is_encrypted(&text) {
        return Ok(text);
    }
    let env: Envelope = serde_json::from_str(&text)
        .with_context(|| crate::i18n::t("err.crypto.bad_envelope"))?;
    let Some(pw) = password else {
        bail!(crate::i18n::t("err.crypto.password_required"));
    };
    decrypt(&env, pw)
}

/// Encrypt plaintext JSON and write it back (atomic replace).
pub fn encrypt_file(path: &std::path::Path, password: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| {
        crate::i18n::tp(
            "err.crypto.read_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
    if is_encrypted(&text) {
        bail!(crate::i18n::t("err.crypto.already_encrypted"));
    }
    let env = encrypt(&text, password)?;
    write_atomic(path, &serde_json::to_string_pretty(&env)?)
}

/// Constant-time comparison, for anything that gates access on a secret
/// string. Bailing out at the first differing byte tells whoever is guessing
/// how much of their guess was right, one byte at a time.
///
/// Lives here, in one place, because three doors need it now (the phone's
/// token, the settings server's, and the external API's) and three copies
/// would be three chances to write the fast, leaky version.
pub fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Write via a temp file then rename (avoids conflicts with Google Drive
/// sync; see DESIGN section 11).
pub fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    // Create the destination folder (e.g. config/) if it doesn't exist.
    // Without this, writing the temp file fails and the save silently
    // drops out as an "empty response".
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).with_context(|| {
                crate::i18n::tp(
                    "err.crypto.mkdir_failed",
                    &[("path", &dir.display().to_string())],
                )
            })?;
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).with_context(|| {
        crate::i18n::tp(
            "err.crypto.write_failed",
            &[("path", &tmp.display().to_string())],
        )
    })?;
    std::fs::rename(&tmp, path).with_context(|| {
        crate::i18n::tp(
            "err.crypto.rename_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
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
        assert!(err.contains("decrypt"), "{err}");
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
        // AES-GCM is authenticated, so tampering can be detected
        let mut raw = B64.decode(&env.data).unwrap();
        raw[0] ^= 0xff;
        env.data = B64.encode(raw);
        assert!(decrypt(&env, "pw").is_err());
    }
}
