//! Web Push: a notification on a phone with nobody in the middle.
//!
//! The other destinations (src/notify.rs) reach a phone by way of an account
//! somebody else runs — a Slack workspace, a Telegram bot, a Discord webhook.
//! They work, and they mean the answer an AI gave on this machine travels
//! through a company's servers to get back to the person who asked for it.
//!
//! Web Push is the same buzz in a pocket without that. The phone's own browser
//! keeps a subscription; this program encrypts each message **for that phone**
//! and hands the sealed envelope to the browser vendor's push service, which
//! can carry it but cannot read it. No account is created, nothing is signed
//! up for, and there is no room in the middle to leave a copy.
//!
//! ## What it needs
//!
//! An HTTPS page. Browsers will not register a service worker anywhere else,
//! and the board is served over plain HTTP unless `tailscale serve` is in
//! front of it (src/tailscale.rs). That is the whole reason this arrives after
//! that file: the page has to be a secure context before any of this exists.
//!
//! This program also has to be **running** to send. Push does not queue work
//! for a machine that is off; what it removes is the need for the phone to
//! have the page open.
//!
//! ## What is here
//!
//! Two pieces of cryptography, both written out rather than reached for,
//! because the crates that would do it whole are large and this is small:
//!
//!   - **RFC 8291**, which encrypts the message. An ephemeral P-256 key is
//!     agreed with the subscriber's, and the result is stretched into an
//!     AES-128-GCM key and nonce. Held to the RFC's own worked example, down
//!     to the byte -- see the tests, which also say what that does and does
//!     not prove.
//!   - **RFC 8292** (VAPID), which signs each delivery so the push service can
//!     tell one sender from another. An ES256 JWT, and the public half of the
//!     same key travels with it.
//!
//! The VAPID key is made once and kept: every subscription a phone made is
//! bound to it, so losing it means every phone has to subscribe again.

use base64::Engine as _;
use std::path::PathBuf;

/// Base64url without padding, which is what every field in these two RFCs is.
const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Decodes base64url. Accepts the padded spelling too: a subscription is
/// copied out of a browser by way of JSON and JavaScript, and both spellings
/// turn up in the wild.
fn unb64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim().trim_end_matches('=');
    B64.decode(s).ok()
}

/// The service worker: the small piece of this program that runs inside the
/// phone's browser, with the page shut.
///
/// It does two things and deliberately nothing else. A woken worker has no
/// screen, no state and about ten seconds to live, so anything clever here is
/// something that fails silently on a phone in a pocket.
///
/// The `push` handler has to show a notification every time it runs -- the
/// subscription was made with `userVisibleOnly`, and a browser that catches
/// this handler staying quiet takes the permission away. So the fallbacks all
/// end in a notification, never in a `return`.
pub const SERVICE_WORKER: &str = r#"// SHIKISHA-TERM
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", e => e.waitUntil(self.clients.claim()));

self.addEventListener("push", e => {
  // Whatever arrives, something is shown. A message that will not parse is
  // still a message: the alternative is a browser quietly revoking the
  // permission for a worker that woke and said nothing.
  let d = {};
  try { d = e.data ? e.data.json() : {}; }
  catch (err) { d = { body: e.data ? e.data.text() : "" }; }
  e.waitUntil(self.registration.showNotification(d.title || "SHIKISHA-TERM", {
    body: d.body || "",
    icon: "/pwa/icon-192.png",
    badge: "/pwa/icon-192.png",
    data: { url: d.url || "/" },
  }));
});

self.addEventListener("notificationclick", e => {
  e.notification.close();
  const url = (e.notification.data && e.notification.data.url) || "/";
  // An open board is reused rather than a second one opened beside it: two
  // copies of this page fighting over one terminal is the thing to avoid.
  e.waitUntil((async () => {
    const open = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    for (const c of open) {
      if (c.url.indexOf(self.registration.scope) === 0) {
        if ("navigate" in c) { try { await c.navigate(url); } catch (err) {} }
        return c.focus();
      }
    }
    return self.clients.openWindow(url);
  })());
});
"#;

// ── Where the two files live ────────────────────────────────────────────────

fn vapid_path() -> PathBuf {
    crate::config::state_path("push-key.json")
}

fn subs_path() -> PathBuf {
    crate::config::state_path("push-subs.json")
}

/// One phone.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Sub {
    /// Where the push service wants the envelope posted.
    pub endpoint: String,
    /// The subscriber's public key, base64url, uncompressed (65 bytes).
    pub p256dh: String,
    /// The subscriber's authentication secret, base64url (16 bytes).
    pub auth: String,
    /// What to call this phone in the settings list. Whatever the browser says
    /// it is; a person with two phones needs to be able to tell them apart.
    #[serde(default)]
    pub name: String,
}

// ── The sender's own key ────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct Vapid {
    /// The private scalar, base64url (32 bytes).
    secret: String,
    /// The public point, base64url, uncompressed (65 bytes). Kept beside the
    /// secret rather than derived each time, because this is the string the
    /// page needs and it is nicer to read a file than to do arithmetic.
    public: String,
}

/// Read the sender's key, making one the first time.
///
/// Every subscription a phone holds names this key. Replacing it silently
/// would leave every phone subscribed to something that no longer exists and
/// no longer sends, so it is written once and then only read.
fn vapid() -> Result<(p256::SecretKey, String), String> {
    let path = vapid_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Vapid>(&text) {
            if let Some(key) = unb64(&v.secret).and_then(|b| p256::SecretKey::from_slice(&b).ok()) {
                return Ok((key, v.public));
            }
        }
        // A file that cannot be read is not quietly replaced: replacing it
        // breaks every phone, and the person deserves to be told which file to
        // look at rather than to wonder why notifications stopped.
        return Err(crate::i18n::tp(
            "err.push.bad_key",
            &[("path", &path.display().to_string())],
        ));
    }
    let key = new_key()?;
    let public = b64(&key.public_key().to_sec1_bytes());
    let v = Vapid {
        secret: b64(&key.to_bytes()),
        public: public.clone(),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    crate::append_hook_log(&format!("push: wrote a new sender key to {}", path.display()));
    Ok((key, public))
}

/// A fresh P-256 secret, from the system's own randomness.
///
/// Rolled here rather than through the curve crate's generator so that this
/// file draws from the same source as every other secret this program makes
/// (`crate::random_bytes`), and so that a version disagreement between two
/// randomness crates cannot quietly become a weaker key.
fn new_key() -> Result<p256::SecretKey, String> {
    for _ in 0..8 {
        let bytes = crate::random_bytes(32).ok_or_else(|| crate::i18n::t("err.push.no_random"))?;
        if let Ok(k) = p256::SecretKey::from_slice(&bytes) {
            return Ok(k);
        }
        // Only reachable for a scalar outside the curve's order, which is a
        // one-in-2^128 event. Drawing again is the whole remedy.
    }
    Err(crate::i18n::t("err.push.no_random"))
}

/// The public half of the sender's key, which the page hands to the browser
/// when it subscribes.
pub fn public_key() -> Result<String, String> {
    vapid().map(|(_, public)| public)
}

// ── The subscriptions ───────────────────────────────────────────────────────

pub fn subs() -> Vec<Sub> {
    std::fs::read_to_string(subs_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_subs(list: &[Sub]) {
    let path = subs_path();
    if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(list).unwrap_or_default()) {
        crate::append_hook_log(&format!("push: could not write {}: {e}", path.display()));
    }
}

/// Remember a phone. The endpoint is the identity: a browser that subscribes
/// twice hands back the same one, and storing it twice would mean two buzzes.
pub fn remember(sub: Sub) {
    let mut list = subs();
    match list.iter_mut().find(|s| s.endpoint == sub.endpoint) {
        Some(existing) => *existing = sub,
        None => list.push(sub),
    }
    write_subs(&list);
}

/// Forget one phone, by endpoint. Answers whether there was one.
pub fn forget(endpoint: &str) -> bool {
    let mut list = subs();
    let before = list.len();
    list.retain(|s| s.endpoint != endpoint);
    let gone = list.len() != before;
    if gone {
        write_subs(&list);
    }
    gone
}

// ── RFC 8291: the message, sealed for one phone ─────────────────────────────

/// The body of a push request: everything a phone needs to open it, and
/// nothing the push service can use.
///
/// `ephemeral` and `salt` are arguments rather than drawn inside, for one
/// reason: it is what lets the RFC's own test vector be run against this
/// function. They must be fresh for every real message, which `seal` sees to.
fn seal_with(
    ua_public: &[u8],
    auth: &[u8],
    plaintext: &[u8],
    ephemeral: &p256::SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use hkdf::Hkdf;
    use sha2::Sha256;

    let ua = p256::PublicKey::from_sec1_bytes(ua_public)
        .map_err(|_| crate::i18n::t("err.push.bad_sub"))?;
    let as_public = ephemeral.public_key().to_sec1_bytes();

    // The agreed secret. Everything below is a way of turning it into one key
    // and one nonce that only these two parties can arrive at.
    let shared = p256::elliptic_curve::ecdh::diffie_hellman(
        ephemeral.to_nonzero_scalar(),
        ua.as_affine(),
    );

    // "WebPush: info" ties the key to *these two* public keys, so a message
    // sealed for one phone cannot be replayed at another.
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(&as_public);

    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .map_err(|_| crate::i18n::t("err.push.encrypt"))?;

    let hk = Hkdf::<Sha256>::new(Some(&salt[..]), &ikm);
    let mut cek = [0u8; 16];
    hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| crate::i18n::t("err.push.encrypt"))?;
    let mut nonce = [0u8; 12];
    hk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(|_| crate::i18n::t("err.push.encrypt"))?;

    // One record, so the padding delimiter is the "last record" one (0x02).
    let mut padded = plaintext.to_vec();
    padded.push(0x02);
    let cipher = aes_gcm::Aes128Gcm::new_from_slice(&cek)
        .map_err(|_| crate::i18n::t("err.push.encrypt"))?;
    let sealed = cipher
        .encrypt(
            &aes_gcm::Nonce::<aes_gcm::aead::consts::U12>::from(nonce),
            Payload { msg: &padded, aad: b"" },
        )
        .map_err(|_| crate::i18n::t("err.push.encrypt"))?;

    // The header the RFC 8188 body carries in front of the ciphertext: the
    // salt, the record size, and the sender's ephemeral public key, so the
    // phone can do all of the above in reverse.
    let mut out = Vec::with_capacity(21 + as_public.len() + sealed.len());
    out.extend_from_slice(&salt[..]);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// The same, with a fresh key and salt — which is the only way a real message
/// may be sealed.
fn seal(ua_public: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let ephemeral = new_key()?;
    let bytes = crate::random_bytes(16).ok_or_else(|| crate::i18n::t("err.push.no_random"))?;
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&bytes);
    seal_with(ua_public, auth, plaintext, &ephemeral, &salt)
}

// ── RFC 8292: who is sending ────────────────────────────────────────────────

/// The origin of an endpoint, which is what a VAPID token is addressed to.
fn audience(endpoint: &str) -> Option<String> {
    let (scheme, rest) = endpoint.split_once("://")?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

/// A signed statement of who is sending this, for one push service, for a
/// while.
///
/// `sub` names the sender. The RFC wants a way to get in touch if this program
/// ever starts misbehaving as a sender, and the honest answer for a program
/// running on somebody's own PC is the project it came from -- not the address
/// of the person running it, which is theirs and no concern of a push service.
fn vapid_token(endpoint: &str, key: &p256::SecretKey, now: u64) -> Result<String, String> {
    use p256::ecdsa::{SigningKey, signature::Signer};
    let aud = audience(endpoint).ok_or_else(|| crate::i18n::t("err.push.bad_sub"))?;
    let header = b64(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = b64(
        serde_json::json!({
            "aud": aud,
            // Twelve hours. The ceiling the RFC allows is a day; a token that
            // outlives the message it carried has no reason to.
            "exp": now + 12 * 60 * 60,
            "sub": "https://github.com/styleio/ShikishaTerm",
        })
        .to_string()
        .as_bytes(),
    );
    let signing = format!("{header}.{claims}");
    let sk = SigningKey::from(key);
    let sig: p256::ecdsa::Signature = sk.sign(signing.as_bytes());
    Ok(format!("{signing}.{}", b64(&sig.to_bytes())))
}

// ── Sending ─────────────────────────────────────────────────────────────────

/// What a phone is told. Kept small on purpose: a push service guarantees only
/// 4KB, and a notification nobody can read at a glance is a notification that
/// did not need to interrupt anybody.
#[derive(serde::Serialize)]
struct Message<'a> {
    title: &'a str,
    body: &'a str,
    /// Where a tap should go. The reply page, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
}

/// Send to every phone that has subscribed. Answers how many were reached.
///
/// A phone that has gone (uninstalled the page, revoked the permission, or
/// simply not been seen for long enough) answers 404 or 410, and is forgotten
/// here rather than left to be retried forever. Nothing else is treated as
/// final: a push service having a bad minute is not a reason to lose a phone.
pub fn send(title: &str, body: &str, url: Option<&str>) -> Result<usize, String> {
    let list = subs();
    if list.is_empty() {
        return Err(crate::i18n::t("err.push.nobody"));
    }
    let (key, public) = vapid()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let payload = serde_json::to_vec(&Message { title, body, url })
        .map_err(|e| e.to_string())?;

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();

    let mut sent = 0usize;
    let mut gone: Vec<String> = Vec::new();
    let mut last_error: Option<String> = None;
    for s in &list {
        let (Some(ua), Some(auth)) = (unb64(&s.p256dh), unb64(&s.auth)) else {
            gone.push(s.endpoint.clone());
            continue;
        };
        let sealed = match seal(&ua, &auth, &payload) {
            Ok(b) => b,
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        let token = match vapid_token(&s.endpoint, &key, now) {
            Ok(t) => t,
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        let result = agent
            .post(&s.endpoint)
            .header("Authorization", &format!("vapid t={token}, k={public}"))
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            // How long the push service may hold this if the phone is off.
            // Four hours: an answer from this morning is not news tonight.
            .header("TTL", "14400")
            .send(&sealed[..]);
        match result {
            Ok(_) => sent += 1,
            Err(ureq::Error::StatusCode(404 | 410)) => {
                crate::append_hook_log(&format!("push: {} is gone; forgetting it", s.endpoint));
                gone.push(s.endpoint.clone());
            }
            Err(e) => last_error = Some(e.to_string()),
        }
    }
    for endpoint in &gone {
        forget(endpoint);
    }
    match (sent, last_error) {
        (0, Some(e)) => Err(e),
        (0, None) => Err(crate::i18n::t("err.push.nobody")),
        (n, _) => Ok(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 8291 section 5, verbatim. The subscriber's keys, the sender's fixed
    // ephemeral key, the fixed salt, the sentence, and the body that comes out.
    const RFC_UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const RFC_UA_SECRET: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const RFC_AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const RFC_EPHEMERAL: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const RFC_EPHEMERAL_PUBLIC: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const RFC_SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const RFC_PLAINTEXT: &[u8] = b"When I grow up, I want to be a watermelon";
    const RFC_BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    /// The worked example from RFC 8291 section 5, sealed with the same
    /// ephemeral key and salt the RFC fixes, so the answer is deterministic.
    ///
    /// Everything in this file is untestable by inspection: an encryption that
    /// is subtly wrong produces bytes that look exactly as random as correct
    /// ones, and the other way to find out is to hold a phone and wait.
    ///
    /// What backs the expected string below, in order of how much it is worth:
    ///
    ///   - its first 86 bytes are the RFC's own published values -- the salt,
    ///     the record size, and the ephemeral public key that the RFC's fixed
    ///     private key must produce. That the last of those matches is the
    ///     curve arithmetic checking out against a published answer.
    ///   - the rest of it opens, under the RFC's own subscriber private key,
    ///     to the RFC's own sentence. That is the next test down, and it is
    ///     what a phone will do.
    ///   - it was also opened by a separate implementation written against the
    ///     spec on a different crypto stack (OpenSSL, not this one), which is
    ///     as close to a second opinion as this gets without a phone.
    #[test]
    fn the_rfc_8291_example_comes_out_byte_for_byte() {
        let out = seal_with(
            &unb64(RFC_UA_PUBLIC).unwrap(),
            &unb64(RFC_AUTH).unwrap(),
            RFC_PLAINTEXT,
            &p256::SecretKey::from_slice(&unb64(RFC_EPHEMERAL).unwrap()).unwrap(),
            &{
                let mut salt = [0u8; 16];
                salt.copy_from_slice(&unb64(RFC_SALT).unwrap());
                salt
            },
        )
        .unwrap();
        assert_eq!(b64(&out), RFC_BODY);

        // The header, field by field, against what the RFC prints.
        assert_eq!(&out[..16], &unb64(RFC_SALT).unwrap()[..], "salt");
        assert_eq!(&out[16..20], &4096u32.to_be_bytes(), "record size");
        assert_eq!(out[20], 65, "key length");
        assert_eq!(b64(&out[21..86]), RFC_EPHEMERAL_PUBLIC, "ephemeral public key");
    }

    /// The receiving half, done here with the RFC's own subscriber key.
    ///
    /// This is the test that says the sealed thing is openable at all, by
    /// somebody holding only what a phone holds: their own private key and the
    /// bytes off the wire. It reads the record header the way a phone does --
    /// by the length byte, not by an offset written down twice -- so a body
    /// laid out wrongly fails here rather than on somebody's train.
    #[test]
    fn what_was_sealed_opens_with_the_subscribers_own_key() {
        use aes_gcm::aead::{Aead, KeyInit, Payload};
        use hkdf::Hkdf;
        use sha2::Sha256;

        let ua_public = unb64(RFC_UA_PUBLIC).unwrap();
        let ua_secret = p256::SecretKey::from_slice(&unb64(RFC_UA_SECRET).unwrap()).unwrap();
        let auth = unb64(RFC_AUTH).unwrap();
        let body = unb64(RFC_BODY).unwrap();

        let salt = &body[..16];
        let idlen = body[20] as usize;
        let as_public = &body[21..21 + idlen];
        let ciphertext = &body[21 + idlen..];

        let shared = p256::elliptic_curve::ecdh::diffie_hellman(
            ua_secret.to_nonzero_scalar(),
            p256::PublicKey::from_sec1_bytes(as_public).unwrap().as_affine(),
        );
        let mut key_info = Vec::new();
        key_info.extend_from_slice(b"WebPush: info\0");
        key_info.extend_from_slice(&ua_public);
        key_info.extend_from_slice(as_public);
        let mut ikm = [0u8; 32];
        Hkdf::<Sha256>::new(Some(&auth), shared.raw_secret_bytes())
            .expand(&key_info, &mut ikm)
            .unwrap();
        let hk = Hkdf::<Sha256>::new(Some(salt), &ikm);
        let mut cek = [0u8; 16];
        hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).unwrap();
        let mut nonce = [0u8; 12];
        hk.expand(b"Content-Encoding: nonce\0", &mut nonce).unwrap();

        let opened = aes_gcm::Aes128Gcm::new_from_slice(&cek)
            .unwrap()
            .decrypt(
                &aes_gcm::Nonce::<aes_gcm::aead::consts::U12>::from(nonce),
                Payload { msg: ciphertext, aad: b"" },
            )
            .expect("購読者の鍵で開けない");
        assert_eq!(*opened.last().unwrap(), 0x02, "最後のレコードの印が無い");
        assert_eq!(&opened[..opened.len() - 1], RFC_PLAINTEXT);
    }

    /// A real message draws its own key and salt, so no two are alike -- which
    /// is the whole reason `seal_with` exists to be tested at all.
    #[test]
    fn two_messages_are_never_sealed_the_same_way() {
        let ua = unb64(RFC_UA_PUBLIC).unwrap();
        let auth = unb64(RFC_AUTH).unwrap();
        let a = seal(&ua, &auth, b"hello").unwrap();
        let b = seal(&ua, &auth, b"hello").unwrap();
        assert_ne!(a[..16], b[..16], "同じ塩を二度使っている");
        assert_ne!(a[21..86], b[21..86], "同じ使い捨て鍵を二度使っている");
    }

    /// Sends one real notification to every device that has subscribed here.
    ///
    /// Ignored by default and run by hand (`cargo test -- --ignored
    /// really_sends_a_push --nocapture`). Nothing short of this exercises the
    /// part that cannot be checked on paper: whether a push service accepts
    /// the VAPID token, and whether the browser at the other end can open what
    /// was sealed for it.
    #[test]
    #[ignore]
    fn really_sends_a_push() {
        println!("subscriptions: {}", subs().len());
        match send("SHIKISHA-TERM", "\u{30c6}\u{30b9}\u{30c8}\u{901a}\u{77e5}\u{3067}\u{3059}", None) {
            Ok(n) => println!("sent to {n}"),
            Err(e) => println!("failed: {e}"),
        }
    }

    /// A token is addressed to the push service, not to the phone: the path is
    /// where the subscription is, and the origin is who is being asked.
    #[test]
    fn a_token_is_addressed_to_the_service_that_will_carry_it() {
        assert_eq!(
            audience("https://fcm.googleapis.com/fcm/send/abc123").as_deref(),
            Some("https://fcm.googleapis.com")
        );
        assert_eq!(
            audience("https://updates.push.services.mozilla.com/wpush/v2/gAA").as_deref(),
            Some("https://updates.push.services.mozilla.com")
        );
        assert_eq!(audience("not a url"), None);
        assert_eq!(audience("https:///nohost"), None);
    }

    /// Three dots' worth of JWT, signed with a key a push service can check
    /// against the one travelling beside it.
    #[test]
    fn a_token_is_a_signed_jwt_the_public_half_can_open() {
        use p256::ecdsa::signature::Verifier;
        let key = new_key().unwrap();
        let token = vapid_token("https://push.example/x/y", &key, 1_700_000_000).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT は3つの部分でできている");

        let header: serde_json::Value =
            serde_json::from_slice(&unb64(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        let claims: serde_json::Value =
            serde_json::from_slice(&unb64(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example");
        assert_eq!(claims["exp"], 1_700_000_000u64 + 12 * 60 * 60);
        // The sender is named as the project, never as the person running it
        assert!(claims["sub"].as_str().unwrap().starts_with("https://"));

        let signed = format!("{}.{}", parts[0], parts[1]);
        let sig = p256::ecdsa::Signature::from_slice(&unb64(parts[2]).unwrap()).unwrap();
        let verifying = p256::ecdsa::VerifyingKey::from(&p256::ecdsa::SigningKey::from(&key));
        assert!(
            verifying.verify(signed.as_bytes(), &sig).is_ok(),
            "自分の鍵で開けない署名を送っている"
        );
    }

    /// Base64url turns up in these subscriptions both padded and not.
    #[test]
    fn both_spellings_of_base64url_are_read() {
        assert_eq!(unb64("BTBZMqHH6r4Tts7J_aSIgg").unwrap().len(), 16);
        assert_eq!(unb64("BTBZMqHH6r4Tts7J_aSIgg==").unwrap().len(), 16);
        assert_eq!(unb64(" BTBZMqHH6r4Tts7J_aSIgg "), Some(vec![
            5, 48, 89, 50, 161, 199, 234, 190, 19, 182, 206, 201, 253, 164, 136, 130
        ]));
        assert_eq!(unb64("not base64!!"), None);
    }
}
