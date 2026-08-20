//! Saving a pasted/attached file next to the tab it is meant for.
//!
//! A terminal CLI can't take an attachment through the terminal; you hand it a
//! path and it reads the file. So a paste / drop / attach-button lands here: we
//! write the bytes into the tab's working folder (where the AI can reach them)
//! and hand back the path to type into the prompt.
//!
//! Safety posture (see `docs/design/convenience-bar.ja.md`): nothing here ever
//! *runs* the file — it is inert on disk, and only the AI the user chose to hand
//! the path to will read it. The checks are hygiene + mislabel correction, not an
//! antivirus:
//!   - refuse native-executable magic outright (unambiguous, so it catches an
//!     "image" that is really a `.exe` even when the extension lies),
//!   - name the file ourselves — random, ASCII — so there is no path traversal,
//!     no multibyte/encoding surprise, and no collision,
//!   - cap the size,
//!   - self-ignore the whole `.SHIKISHA/` folder from git regardless of the repo.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Bounds a single attachment. Built from the user's config by the caller.
pub struct Limits {
    pub max_bytes: usize,
    /// Extensions the user opted into (lowercase, no dot).
    pub allowed_ext: Vec<String>,
}

/// Leading-byte signatures of native executables / scripts, refused outright.
/// These are unambiguous, so the check holds even when the file is disguised
/// with an image extension. Nothing runs the file, so this is defense in depth,
/// not the primary guard — but it is cheap and catches the obvious disguise.
fn is_executable(bytes: &[u8]) -> bool {
    let starts = |sig: &[u8]| bytes.len() >= sig.len() && &bytes[..sig.len()] == sig;
    starts(b"MZ")                              // Windows PE (.exe/.dll)
        || starts(&[0x7f, b'E', b'L', b'F'])   // ELF
        || starts(&[0xfe, 0xed, 0xfa, 0xce])   // Mach-O 32-bit
        || starts(&[0xfe, 0xed, 0xfa, 0xcf])   // Mach-O 64-bit
        || starts(&[0xcf, 0xfa, 0xed, 0xfe])   // Mach-O 64-bit LE
        || starts(&[0xca, 0xfe, 0xba, 0xbe])   // Mach-O universal / Java class
        || starts(b"#!")                       // shebang script
}

/// Sniff a well-known content type from its leading bytes. `Some(ext)` only when
/// confident; otherwise `None` and we fall back to the declared extension.
fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    let s = |sig: &[u8]| bytes.len() >= sig.len() && &bytes[..sig.len()] == sig;
    if s(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("png");
    }
    if s(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    if s(b"GIF87a") || s(b"GIF89a") {
        return Some("gif");
    }
    if s(b"%PDF-") {
        return Some("pdf");
    }
    if s(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// jpg and jpeg are the same type; treat them as interchangeable when matching
/// against the allow-list.
fn ext_allowed(ext: &str, allowed: &[String]) -> bool {
    let same = |a: &str, b: &str| {
        a == b || matches!((a, b), ("jpg", "jpeg") | ("jpeg", "jpg"))
    };
    allowed.iter().any(|a| same(a, ext))
}

/// Choose the extension to save under: the sniffed type wins (correcting a
/// mislabel); otherwise the declared extension — but either way it must be one
/// the user opted into.
fn choose_ext(bytes: &[u8], declared_name: &str, allowed: &[String]) -> Result<String> {
    if let Some(sniffed) = sniff_ext(bytes) {
        if ext_allowed(sniffed, allowed) {
            return Ok(sniffed.to_string());
        }
        bail!(crate::i18n::tp("attach.err.type", &[("ext", sniffed)]));
    }
    match Path::new(declared_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
    {
        Some(ext) if ext_allowed(&ext, allowed) => Ok(ext),
        Some(ext) => bail!(crate::i18n::tp("attach.err.type", &[("ext", &ext)])),
        None => bail!(crate::i18n::t("attach.err.no_ext")),
    }
}

/// Write `bytes` (a file the user pasted/dropped, declaring `declared_name`) into
/// `<cwd>/.SHIKISHA/tmp/` under a fresh random name, and return the saved path.
pub fn save(cwd: &Path, declared_name: &str, bytes: &[u8], limits: &Limits) -> Result<PathBuf> {
    if bytes.is_empty() {
        bail!(crate::i18n::t("attach.err.empty"));
    }
    if bytes.len() > limits.max_bytes {
        bail!(crate::i18n::tp(
            "attach.err.too_large",
            &[
                ("size", &format!("{}", bytes.len() / (1024 * 1024))),
                ("max", &format!("{}", limits.max_bytes / (1024 * 1024))),
            ],
        ));
    }
    if is_executable(bytes) {
        bail!(crate::i18n::t("attach.err.executable"));
    }
    let ext = choose_ext(bytes, declared_name, &limits.allowed_ext)?;

    let base = cwd.join(".SHIKISHA");
    let dir = base.join("tmp");
    std::fs::create_dir_all(&dir)?;
    // Self-ignore the whole folder, so it never dirties the repo — whether or not
    // the working folder is even a git repo (a per-folder .gitignore works alone).
    let ignore = base.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "*\n");
    }

    // Our own random ASCII name: no traversal, no encoding surprise, no collision.
    let path = dir.join(format!("{}.{}", crate::random_hex(16), ext));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            max_bytes: 10 * 1024 * 1024,
            allowed_ext: ["jpg", "png", "gif", "pdf"].iter().map(|s| s.to_string()).collect(),
        }
    }

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];

    #[test]
    fn a_png_lands_in_shikisha_tmp_with_a_generated_name() {
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        // Declared name lies about the extension — the sniffed PNG wins.
        let p = save(&dir, "photo.bin", PNG, &limits()).unwrap();
        assert!(p.starts_with(dir.join(".SHIKISHA").join("tmp")), "wrong folder: {p:?}");
        assert_eq!(p.extension().unwrap(), "png", "sniffed type should win over declared");
        assert!(p.file_stem().unwrap().to_str().unwrap().chars().all(|c| c.is_ascii_hexdigit()));
        assert!(dir.join(".SHIKISHA").join(".gitignore").exists(), "folder must self-ignore from git");
        assert_eq!(std::fs::read(&p).unwrap(), PNG);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_executable_is_refused_even_disguised_as_an_image() {
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = b"MZ\x90\x00this is really a PE binary";
        let err = save(&dir, "totally_an_image.png", exe, &limits()).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("exec")
            || !err.to_string().is_empty(), "should refuse: {err}");
        // Nothing should have been written
        assert!(!dir.join(".SHIKISHA").join("tmp").exists() ||
            std::fs::read_dir(dir.join(".SHIKISHA").join("tmp")).map(|mut d| d.next().is_none()).unwrap_or(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unallowed_extension_with_no_known_magic_is_refused() {
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        // Unknown magic (plain text), declared .exe which isn't in the allow-list.
        let r = save(&dir, "notes.exe", b"just some bytes", &limits());
        assert!(r.is_err(), "an un-opted-in extension must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversize_is_refused() {
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let big = vec![0x89u8; 11 * 1024 * 1024]; // over the 10MB test cap
        let r = save(&dir, "big.png", &big, &limits());
        assert!(r.is_err(), "over the size cap must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
