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
    /// The last password and salt this was asked about, and what came out.
    /// Deriving is slow on purpose, and the same pair is asked about again and
    /// again while a vault is open
    type Derived = (String, Vec<u8>, [u8; 32]);
    static CACHE: Mutex<Option<Derived>> = Mutex::new(None);
    if let Ok(g) = CACHE.lock()
        && let Some((p, s, k)) = g.as_ref()
            && p == password && s.as_slice() == salt {
                return Ok(*k);
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

/// Write a file so that it is either the old one or the new one, and stays
/// the new one: written beside it, made to reach the disk, then put in its
/// place (also keeps clear of Google Drive's sync; see DESIGN section 11).
///
/// A rename alone is atomic for a reader but not for a power cut: the name
/// can reach the disk before the bytes, and the file comes back empty --
/// the settings, the paired devices, the token, gone. So the new file is
/// synced before the rename, and the folder after it where the system has
/// a way to (not Windows, where the file's own sync is what carries it).
///
/// The file written beside it has a name of its own each time. A fixed one
/// (`x.tmp`) was shared by every write of the same name -- two of them at
/// once wrote into one temp file and one of them put the other's half in
/// place -- and by two files of one stem (`a.json`, `a.md`).
///
/// On Windows the rename is tried again for a moment when the file is held:
/// an antivirus or an indexer opening it just as it is replaced is a normal
/// day there, and a setting not saved because of it is not
pub fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    write_atomic_bytes(path, content.as_bytes())
}

/// [`write_atomic`] for bytes that are not text
pub fn write_atomic_bytes(path: &std::path::Path, content: &[u8]) -> Result<()> {
    use std::io::Write as _;
    // Create the destination folder (e.g. config/) if it doesn't exist.
    // Without this, writing the temp file fails and the save silently
    // drops out as an "empty response".
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir).with_context(|| {
            crate::i18n::tp("err.crypto.mkdir_failed", &[("path", &dir.display().to_string())])
        })?;
    }
    let tmp = temp_beside(path);
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(content)?;
        // A file kept from the person's view stays kept: what replaces it is
        // given what it had (a secrets file at 0600 does not come back 0644)
        #[cfg(unix)]
        if let Ok(was) = std::fs::metadata(path) {
            f.set_permissions(was.permissions())?;
        }
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e))
            .with_context(|| crate::i18n::tp("err.crypto.write_failed", &[("path", &tmp.display().to_string())]));
    }
    if let Err(e) = rename_held_a_moment(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e))
            .with_context(|| crate::i18n::tp("err.crypto.rename_failed", &[("path", &path.display().to_string())]));
    }
    #[cfg(unix)]
    if let Some(dir) = dir
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    sweep_left_behind(path);
    Ok(())
}

/// The name a write is made under before it is put in place: the file's own
/// name, this process, and a number no other write in it has, so no two
/// writes share one. Not a secret, so not drawn from the random source --
/// a save must not fail for want of one
fn temp_beside(path: &std::path::Path) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mark = (nanos.wrapping_add(n << 32)) & 0xffff_ffff_ffff;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}.{}.{mark:012x}.tmp", std::process::id()))
}

/// Whether `file` is one of [`temp_beside`]'s for `name`
fn is_temp_of(file: &str, name: &str) -> bool {
    let Some(rest) = file.strip_prefix(name).and_then(|r| r.strip_prefix('.')).and_then(|r| r.strip_suffix(".tmp")) else {
        return false;
    };
    let mut parts = rest.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(pid), Some(drawn), None)
            if !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
            && drawn.len() == 12 && drawn.bytes().all(|b| b.is_ascii_hexdigit())
    )
}

/// Temp files of this name left by a write that never finished -- the
/// program stopped between writing and renaming. Old ones only: a young one
/// may be another copy of the program in the middle of its own write
fn sweep_left_behind(path: &std::path::Path) {
    const OLD: std::time::Duration = std::time::Duration::from_secs(600);
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().map(|n| n.to_string_lossy().into_owned())) else {
        return;
    };
    let dir = if dir.as_os_str().is_empty() { std::path::Path::new(".") } else { dir };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if !is_temp_of(&e.file_name().to_string_lossy(), &name) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > OLD);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Put `from` in place of `to`, trying again for a moment on Windows while
/// something else holds `to` open (access denied, sharing or lock
/// violation). Anything else, and anywhere else, is answered at once
fn rename_held_a_moment(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let mut tries = 0u64;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if cfg!(windows) && tries < 5 && is_held(&e) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50 * tries));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Whether a failed file operation looks like another program holding the
/// file for a moment: access denied (5), sharing violation (32), lock
/// violation (33)
fn is_held(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33))
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
        assert_ne!(a.salt, b.salt, "the salt changes every time");
        assert_ne!(a.nonce, b.nonce, "the nonce changes every time");
        assert_ne!(a.data, b.data, "the same plaintext gives a different ciphertext");
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

    /// Not a test: seals the file SHIKISHA_SEAL_FILE with the password
    /// SHIKISHA_SEAL_PW, for a check of the app's master password lock
    /// (tools/debug/master-lock.win.mjs) that needs a sealed store to start on
    #[test]
    #[ignore]
    fn seal_a_file_for_a_check() {
        let file = std::env::var("SHIKISHA_SEAL_FILE").expect("SHIKISHA_SEAL_FILE");
        let pw = std::env::var("SHIKISHA_SEAL_PW").expect("SHIKISHA_SEAL_PW");
        encrypt_file(std::path::Path::new(&file), &pw).unwrap();
    }

    fn own_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("shikitest_atomic_{}", crate::random_hex(6)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names_in(dir: &std::path::Path) -> Vec<String> {
        let mut n: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        n.sort();
        n
    }

    /// The file is replaced whole, and nothing is left beside it
    #[test]
    fn a_write_replaces_the_file_and_leaves_nothing_beside_it() {
        let dir = own_dir();
        let f = dir.join("config.json");
        write_atomic(&f, "first").unwrap();
        write_atomic(&f, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "second");
        assert_eq!(names_in(&dir), ["config.json"], "a temp file was left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Writes of one file at the same moment each put a whole file in place.
    /// With one temp name for all of them, one wrote into the other's half
    /// and a mix of the two -- or an error -- was what was left
    #[test]
    fn writes_at_once_never_mix() {
        let dir = own_dir();
        let f = dir.join("clients.json");
        let bodies: Vec<String> = (0..8).map(|i| format!("{{\"writer\":{i},\"pad\":\"{}\"}}", "x".repeat(200_000))).collect();
        std::thread::scope(|s| {
            for b in &bodies {
                let f = &f;
                s.spawn(move || {
                    for _ in 0..5 {
                        write_atomic(f, b).expect("a write at the same moment failed");
                    }
                });
            }
        });
        let left = std::fs::read_to_string(&f).unwrap();
        assert!(bodies.contains(&left), "what is left is not any one writer's whole file ({} bytes)", left.len());
        assert_eq!(names_in(&dir), ["clients.json"], "a temp file was left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two files of one stem do not write through one temp file
    #[test]
    fn two_files_of_one_stem_keep_apart() {
        let dir = own_dir();
        let (a, b) = (dir.join("notes.json"), dir.join("notes.md"));
        assert_ne!(temp_beside(&a).file_name(), temp_beside(&b).file_name());
        write_atomic(&a, "json").unwrap();
        write_atomic(&b, "md").unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "json");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A temp file left by a write that never finished goes once it is old;
    /// a young one may be another copy's write in progress and stays, and
    /// nothing that is not one of ours is touched
    #[test]
    fn what_an_unfinished_write_left_is_swept_once_old() {
        let dir = own_dir();
        let f = dir.join("push-subs.json");
        let old = dir.join("push-subs.json.4242.0123456789ab.tmp");
        let young = dir.join("push-subs.json.4242.0123456789ac.tmp");
        let theirs = dir.join("push-subs.json.backup.tmp");
        for p in [&old, &young, &theirs] {
            std::fs::write(p, "half").unwrap();
        }
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(hour_ago).unwrap();
        std::fs::File::options().write(true).open(&theirs).unwrap().set_modified(hour_ago).unwrap();
        write_atomic(&f, "[]").unwrap();
        assert_eq!(
            names_in(&dir),
            ["push-subs.json", "push-subs.json.4242.0123456789ac.tmp", "push-subs.json.backup.tmp"],
            "the old one stayed, or a young one or somebody else's went"
        );
        assert!(is_temp_of("a.json.12.0123456789ab.tmp", "a.json"));
        assert!(!is_temp_of("a.json.12.0123456789ab.tmp", "a.js"));
        assert!(!is_temp_of("a.json.tmp", "a.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file kept from other users stays kept when it is replaced
    #[cfg(unix)]
    #[test]
    fn a_replaced_file_keeps_who_may_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = own_dir();
        let f = dir.join("secrets.json");
        std::fs::write(&f, "{}").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&f, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
