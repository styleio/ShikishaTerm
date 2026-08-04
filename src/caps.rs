//! 自動化に与える能力 (ファイル・HTTP)。DESIGN.md 8.5章。
//!
//! 既定では何も許可しない。設定ファイルに書いたものだけが使える。
//! GUIからは編集させない (玄人向け機能であり、誤操作の影響が大きいため)。
//!
//! 方式は「名前付きの窓口」。スクリプトはパスやURLを組み立てられず、
//! 登録済みの名前しか呼べないので、送信先のすり替えや資格情報の持ち出しが起きない。
//! 生パス・生URLのホワイトリストも用意するが既定は空。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use anyhow::{Result, bail};
use serde::Deserialize;

/// 許可ディレクトリ内であっても決して触れさせないファイル
/// (自己書き換えと資格情報の吸い出しを防ぐ)
fn is_forbidden(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(name.as_str(), "config.json" | "secrets.json" | ".env") {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 自動化スクリプト自身と暗号化ファイル
    matches!(ext.as_str(), "lua" | "enc")
}

fn rel_is_safe(rel: &str) -> bool {
    let p = Path::new(rel);
    !p.is_absolute()
        && !rel.is_empty()
        && !p
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CapabilitySpec {
    /// 名前付きのファイル窓口
    #[serde(default)]
    pub files: HashMap<String, FileCap>,
    /// 名前付きのHTTP窓口
    #[serde(default)]
    pub http: HashMap<String, HttpCap>,
    /// 玄人向け: 生パスを許可するディレクトリ (既定は空)
    #[serde(default)]
    pub allow_dirs: Vec<String>,
    /// 玄人向け: 生URLを許可するホスト。完全一致で照合する (既定は空)
    #[serde(default)]
    pub allow_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileCap {
    pub dir: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpCap {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    /// secretsの tokens から取り出して付与する認証情報の名前
    #[serde(default)]
    pub auth_from_secrets: Option<String>,
    /// 認証情報を載せるヘッダ名 (既定 Authorization)
    #[serde(default = "default_auth_header")]
    pub auth_header: String,
}

fn default_method() -> String {
    "POST".into()
}
fn default_auth_header() -> String {
    "Authorization".into()
}

struct HttpJob {
    url: String,
    method: String,
    body: String,
    auth: Option<(String, String)>,
}

pub struct Capabilities {
    spec: CapabilitySpec,
    base: PathBuf,
    tokens: HashMap<String, String>,
    tx: Option<mpsc::Sender<HttpJob>>,
}

impl Capabilities {
    /// 何も許可しない状態 (既定)
    pub fn disabled() -> Self {
        Self {
            spec: CapabilitySpec::default(),
            base: PathBuf::from("."),
            tokens: HashMap::new(),
            tx: None,
        }
    }

    pub fn new(spec: CapabilitySpec, base: PathBuf, tokens: HashMap<String, String>) -> Self {
        // 通信はUIをブロックしないよう専用スレッドで行う
        let tx = if spec.http.is_empty() && spec.allow_hosts.is_empty() {
            None
        } else {
            let (tx, rx) = mpsc::channel::<HttpJob>();
            std::thread::spawn(move || {
                let agent = ureq::Agent::config_builder()
                    .timeout_global(Some(std::time::Duration::from_secs(15)))
                    .build()
                    .new_agent();
                while let Ok(job) = rx.recv() {
                    // GETは本体を持たないので型が異なる (ureq)
                    let result = match job.method.to_ascii_uppercase().as_str() {
                        "GET" => {
                            let mut r = agent.get(&job.url);
                            if let Some((h, v)) = &job.auth {
                                r = r.header(h.as_str(), v.as_str());
                            }
                            r.call().map(|x| x.status().as_u16())
                        }
                        m => {
                            let mut r = if m == "PUT" {
                                agent.put(&job.url)
                            } else {
                                agent.post(&job.url)
                            };
                            if let Some((h, v)) = &job.auth {
                                r = r.header(h.as_str(), v.as_str());
                            }
                            r.header("Content-Type", "application/json")
                                .send(&job.body)
                                .map(|x| x.status().as_u16())
                        }
                    };
                    match result {
                        Ok(code) => crate::append_hook_log(&format!(
                            "http {} {} -> {code}",
                            job.method, job.url
                        )),
                        Err(e) => {
                            crate::append_hook_log(&format!("http {} 失敗: {e}", job.url))
                        }
                    }
                }
            });
            Some(tx)
        };
        Self {
            spec,
            base,
            tokens,
            tx,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spec.files.is_empty()
            && self.spec.http.is_empty()
            && self.spec.allow_dirs.is_empty()
            && self.spec.allow_hosts.is_empty()
    }

    /// 名前付き窓口のパスを解決する
    fn named_path(&self, name: &str, rel: &str, want_write: bool) -> Result<PathBuf> {
        let Some(cap) = self.spec.files.get(name) else {
            bail!("ファイル窓口 '{name}' は未登録です (config.json の capabilities.files)");
        };
        if want_write && !cap.write {
            bail!("'{name}' は書き込みが許可されていません");
        }
        if !want_write && !cap.read {
            bail!("'{name}' は読み取りが許可されていません");
        }
        if !rel_is_safe(rel) {
            bail!("ファイル名が不正です: {rel}");
        }
        let path = self.base.join(&cap.dir).join(rel);
        if is_forbidden(&path) {
            bail!("このファイルは自動化からは扱えません: {rel}");
        }
        Ok(path)
    }

    /// 生パス (allow_dirs 内に限る)
    fn raw_path(&self, p: &str) -> Result<PathBuf> {
        if self.spec.allow_dirs.is_empty() {
            bail!("生パスの利用は許可されていません (capabilities.allow_dirs が空)");
        }
        let target = self.base.join(p);
        let parent = target.parent().unwrap_or(Path::new("."));
        let canon_parent = parent
            .canonicalize()
            .map_err(|_| anyhow::anyhow!("フォルダが存在しません: {}", parent.display()))?;
        let ok = self.spec.allow_dirs.iter().any(|d| {
            self.base
                .join(d)
                .canonicalize()
                .map(|c| canon_parent.starts_with(c))
                .unwrap_or(false)
        });
        if !ok {
            bail!("許可されていない場所です: {p}");
        }
        if is_forbidden(&target) {
            bail!("このファイルは自動化からは扱えません: {p}");
        }
        Ok(target)
    }

    pub fn read(&self, name: &str, rel: &str) -> Result<String> {
        let p = self.named_path(name, rel, false)?;
        Ok(std::fs::read_to_string(&p)?)
    }

    pub fn write(&self, name: &str, rel: &str, data: &str) -> Result<()> {
        let p = self.named_path(name, rel, true)?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::crypto::write_atomic(&p, data)?;
        crate::append_hook_log(&format!("write_file {name}/{rel} ({} bytes)", data.len()));
        Ok(())
    }

    pub fn read_raw(&self, p: &str) -> Result<String> {
        Ok(std::fs::read_to_string(self.raw_path(p)?)?)
    }

    pub fn write_raw(&self, p: &str, data: &str) -> Result<()> {
        let path = self.raw_path(p)?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::crypto::write_atomic(&path, data)?;
        crate::append_hook_log(&format!("write_path {p} ({} bytes)", data.len()));
        Ok(())
    }

    /// 名前付きHTTP窓口へ送信する (送りっぱなし。応答はログに残す)
    pub fn http(&self, name: &str, body: &str) -> Result<()> {
        let Some(cap) = self.spec.http.get(name) else {
            bail!("HTTP窓口 '{name}' は未登録です (config.json の capabilities.http)");
        };
        let auth = cap.auth_from_secrets.as_ref().and_then(|key| {
            self.tokens
                .get(key)
                .map(|v| (cap.auth_header.clone(), v.clone()))
        });
        self.dispatch(HttpJob {
            url: cap.url.clone(),
            method: cap.method.clone(),
            body: body.to_string(),
            auth,
        })
    }

    /// 生URL (allow_hosts に完全一致するホストのみ)
    pub fn http_raw(&self, url: &str, body: &str) -> Result<()> {
        let host = host_of(url).ok_or_else(|| anyhow::anyhow!("URLが不正です: {url}"))?;
        if !url.starts_with("https://") {
            bail!("https:// のみ許可されています");
        }
        if !self.spec.allow_hosts.iter().any(|h| h == &host) {
            bail!("許可されていない接続先です: {host}");
        }
        self.dispatch(HttpJob {
            url: url.to_string(),
            method: "POST".into(),
            body: body.to_string(),
            auth: None,
        })
    }

    fn dispatch(&self, job: HttpJob) -> Result<()> {
        let Some(tx) = &self.tx else {
            bail!("HTTPは有効化されていません");
        };
        tx.send(job)
            .map_err(|_| anyhow::anyhow!("送信キューが閉じています"))
    }
}

/// URLからホスト部だけを取り出す (ポート・認証情報は除く)
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let hostport = rest.split(['/', '?', '#']).next()?;
    // user:pass@host のような形は認めない (すり替えを防ぐ)
    if hostport.contains('@') {
        return None;
    }
    let host = hostport.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(spec: CapabilitySpec, base: PathBuf) -> Capabilities {
        Capabilities::new(spec, base, HashMap::new())
    }

    #[test]
    fn nothing_is_allowed_by_default() {
        let c = caps(CapabilitySpec::default(), PathBuf::from("."));
        assert!(c.is_empty());
        assert!(c.write("reports", "a.md", "x").is_err());
        assert!(c.http("api", "{}").is_err());
        assert!(c.write_raw("a.md", "x").is_err());
    }

    #[test]
    fn named_file_window_confines_writes() {
        let dir = std::env::temp_dir().join("shikisha-caps");
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = CapabilitySpec::default();
        spec.files.insert(
            "reports".into(),
            FileCap {
                dir: "reports".into(),
                read: true,
                write: true,
            },
        );
        let c = caps(spec, dir.clone());

        assert!(c.write("reports", "ok.md", "hello").is_ok());
        assert_eq!(c.read("reports", "ok.md").unwrap(), "hello");
        // 外へ出ようとする指定は拒否
        assert!(c.write("reports", "../escape.md", "x").is_err());
        assert!(c.write("reports", "C:/windows/x.md", "x").is_err());
        // 未登録の窓口は使えない
        assert!(c.write("other", "a.md", "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crown_jewels_are_never_writable() {
        let dir = std::env::temp_dir().join("shikisha-caps2");
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = CapabilitySpec::default();
        spec.files.insert(
            "all".into(),
            FileCap {
                dir: ".".into(),
                read: true,
                write: true,
            },
        );
        let c = caps(spec, dir.clone());
        for f in ["config.json", "secrets.json", ".env", "hack.lua", "x.enc"] {
            assert!(c.write("all", f, "x").is_err(), "{f} は拒否されるはず");
            assert!(c.read("all", f).is_err(), "{f} は拒否されるはず");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_matching_is_exact() {
        assert_eq!(host_of("https://api.github.com/x"), Some("api.github.com".into()));
        // 前方一致のすり抜けを許さない
        assert_eq!(
            host_of("https://api.github.com.evil.com/x"),
            Some("api.github.com.evil.com".into())
        );
        // 認証情報つきURLでのすり替えを拒否
        assert_eq!(host_of("https://api.github.com@evil.com/x"), None);

        let mut spec = CapabilitySpec::default();
        spec.allow_hosts.push("api.github.com".into());
        let c = caps(spec, PathBuf::from("."));
        assert!(c.http_raw("https://api.github.com.evil.com/x", "{}").is_err());
        assert!(c.http_raw("http://api.github.com/x", "{}").is_err(), "httpは不可");
    }
}
