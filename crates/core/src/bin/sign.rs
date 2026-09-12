//! Signs a release zip with the key the program checks downloads against.
//!
//! The program refuses to put a downloaded zip in place unless a signature
//! made with the matching private key comes with it (../update.rs). This
//! is the one thing that makes such a signature. It runs in CI, over the
//! zip that was just built, with the key from the repository's secrets;
//! it can also make a key pair, once, when the key is first set up.
//!
//!     sign keygen                 print a new seed and its public key, as hex
//!     sign pubkey                 print the public key for the seed in UPDATE_SIGNING_KEY
//!     sign sign <file>            write <file>.sig (hex) for the seed in UPDATE_SIGNING_KEY
//!     sign verify <file> <hex>    check <file>.sig against a public key
//!
//! The seed is 32 bytes of hex in the `UPDATE_SIGNING_KEY` environment
//! variable and nowhere else: never on the command line (visible to every
//! process) and never in a file this tool writes.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn seed_from_env() -> Result<SigningKey, String> {
    let s = std::env::var("UPDATE_SIGNING_KEY").map_err(|_| "UPDATE_SIGNING_KEY is not set".to_string())?;
    let bytes: [u8; 32] = from_hex(&s)?.try_into().map_err(|_| "the seed is not 32 bytes".to_string())?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen"] => {
            let seed: [u8; 32] = rand::random();
            let key = SigningKey::from_bytes(&seed);
            println!("UPDATE_SIGNING_KEY={}", hex(&seed));
            println!("public={}", hex(&key.verifying_key().to_bytes()));
            Ok(())
        }
        ["pubkey"] => {
            println!("{}", hex(&seed_from_env()?.verifying_key().to_bytes()));
            Ok(())
        }
        ["sign", file] => {
            let key = seed_from_env()?;
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let sig = key.sign(&bytes);
            let out = format!("{file}.sig");
            std::fs::write(&out, format!("{}\n", hex(&sig.to_bytes()))).map_err(|e| format!("{out}: {e}"))?;
            println!("{out}");
            Ok(())
        }
        ["verify", file, pubhex] => {
            let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
            let sig_hex = std::fs::read_to_string(format!("{file}.sig")).map_err(|e| format!("{file}.sig: {e}"))?;
            let key: [u8; 32] = from_hex(pubhex)?.try_into().map_err(|_| "the key is not 32 bytes".to_string())?;
            let sig: [u8; 64] = from_hex(&sig_hex)?.try_into().map_err(|_| "the signature is not 64 bytes".to_string())?;
            VerifyingKey::from_bytes(&key)
                .map_err(|e| e.to_string())?
                .verify(&bytes, &Signature::from_bytes(&sig))
                .map_err(|_| "the signature does not match".to_string())?;
            println!("ok");
            Ok(())
        }
        _ => Err("usage: sign keygen | pubkey | sign <file> | verify <file> <public-key-hex>".into()),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
