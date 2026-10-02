//! A file on this PC, shown in a browser tab.
//!
//! A page cannot be opened from `file:` here: when the window's browser hears
//! from a page it reads that page's address as an `http::Uri`, a `file:` one
//! does not read, and the whole program goes down (see
//! `shikisha_shared::is_openable`). So a file is handed to the tab over HTTP
//! instead, from a small server of its own on 127.0.0.1.
//!
//! - **Its own port.** Not the settings server's: a page from the same origin
//!   could ask the settings of the app, and a file somebody downloaded is not
//!   to be trusted with that. A page's origin is its port as well as its host.
//! - **Public page assets only.** The selected page and ordinary web assets
//!   may be read. Data, configuration, hidden files and private keys are not
//!   published with a page. Sites needing data endpoints should use their own
//!   development server. Links are checked against the actual opened file.
//! - **Under a key nobody can guess.** Each shared folder is reached at
//!   `/<key>/...`, the key drawn at random when the folder is first shared,
//!   so another program on this PC cannot walk the disk through the port.
//! - **Read fresh every time.** Nothing is cached: a file changed and reloaded
//!   shows the change, which is why somebody opens their own HTML in a tab.
//!
//! The address written in the settings (`browser D:/site/index.html`) stays
//! what it is; the served address is worked out each time the page is opened,
//! so it is right after the program restarts on another port.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// What is shared, and where the server listens
struct Shared {
    port: u16,
    /// key -> the folder it opens on, as the disk names it (canonical)
    folders: Mutex<HashMap<String, PathBuf>>,
}

/// The file a typed or written address names on this PC, when it names one.
///
/// `file:///D:/a.html`, `file://server/share/a.html`, `D:/a.html`, `D:\a.html`,
/// `\\server\share\a.html` -- and on a machine with no drive letters, a path
/// from `/`. Nothing else: `example.com/a.html` is a web address, and a path
/// with no drive or root says nothing about where it is
pub fn local_file(text: &str) -> Option<PathBuf> {
    let s = text.trim();
    if let Some(rest) = s.get(..7).filter(|p| p.eq_ignore_ascii_case("file://")).map(|_| &s[7..]) {
        let rest = percent_decode(rest);
        // file:///D:/a -> D:/a ; file://server/share -> \\server\share ;
        // file:///home/a -> /home/a
        let path = match rest.strip_prefix('/') {
            Some(after) if has_drive(after) => after.to_string(),
            Some(_) => rest.clone(),
            None if has_drive(&rest) => rest.clone(),
            None => format!("//{rest}"),
        };
        return Some(PathBuf::from(path));
    }
    if has_drive(s) || s.starts_with("\\\\") || (cfg!(not(windows)) && s.starts_with('/') && !s.starts_with("//")) {
        return Some(PathBuf::from(s));
    }
    None
}

/// `D:/...` or `D:\...`: a letter, a colon, and a separator
fn has_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

/// Whether a page is the place to show this file: a page itself, a picture, a
/// PDF. What a terminal's link offers to open in a browser tab
pub fn shows_in_page(file: &Path) -> bool {
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(
        ext.as_str(),
        "html" | "htm" | "xhtml" | "svg" | "pdf" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif"
    )
}

/// The address a tab opens for `text`: None when it is not a file on this PC
/// (it is opened as it is), the served address when it is, and why not when
/// it names a file that cannot be shown
pub fn address(text: &str) -> Option<Result<String, String>> {
    let file = local_file(text)?;
    Some(serve(&file))
}

/// The served address of one file, sharing its folder
fn serve(file: &Path) -> Result<String, String> {
    let shown = file.display().to_string();
    let real = std::fs::canonicalize(file).map_err(|_| crate::i18n::tp("err.localpage.missing", &[("path", &shown)]))?;
    if real.is_dir() {
        return Err(crate::i18n::tp("err.localpage.folder", &[("path", &shown)]));
    }
    if !public_file(&real) || !public_file(file) {
        return Err(crate::i18n::tp("err.localpage.private", &[("path", &shown)]));
    }
    let folder = real.parent().ok_or_else(|| crate::i18n::tp("err.localpage.missing", &[("path", &shown)]))?;
    let shared = server().map_err(|e| crate::i18n::tp("err.localpage.server", &[("e", &e)]))?;
    let key = {
        let mut folders = shared.folders.lock().unwrap_or_else(|e| e.into_inner());
        match folders.iter().find(|(_, f)| f.as_path() == folder) {
            Some((k, _)) => k.clone(),
            None => {
                let k = crate::random_hex(16);
                folders.insert(k.clone(), folder.to_path_buf());
                k
            }
        }
    };
    let name = real.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    Ok(format!("http://127.0.0.1:{}/{key}/{}", shared.port, percent_encode(&name)))
}

/// The server, started the first time a file is asked for
fn server() -> Result<&'static Shared, String> {
    static SHARED: OnceLock<Result<Shared, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            let listening = tiny_http::Server::http_bounded("127.0.0.1:0", tiny_http::Limits::default()).map_err(|e| e.to_string())?;
            let port = listening.server_addr().to_ip().map(|a| a.port()).ok_or("no port")?;
            std::thread::spawn(move || {
                for req in listening.incoming_requests() {
                    // One at a time would make a page wait for its video
                    std::thread::spawn(move || answer(req));
                }
            });
            Ok(Shared { port, folders: Mutex::new(HashMap::new()) })
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn answer(req: tiny_http::Request) {
    if *req.method() != tiny_http::Method::Get && *req.method() != tiny_http::Method::Head {
        let _ = req.respond(tiny_http::Response::from_string("").with_status_code(405));
        return;
    }
    let found = server().ok().and_then(|s| {
        let folders = s.folders.lock().unwrap_or_else(|e| e.into_inner());
        found_at(&folders, req.url())
    });
    let Some((open, file)) = found else {
        let _ = req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        return;
    };
    let header = |k: &str, v: &str| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).ok();
    let mut resp = tiny_http::Response::from_file(open);
    for h in [header("Content-Type", kind_of(&file)), header("Cache-Control", "no-store"),
        header("X-Content-Type-Options", "nosniff"), header("Referrer-Policy", "no-referrer")].into_iter().flatten() {
        resp.add_header(h);
    }
    let _ = req.respond(resp);
}

/// The file a request names, opened: under a shared folder by its key, by a
/// path that stays inside it once every link in it is followed. A folder is
/// its `index.html`. None for anything else.
///
/// The test that it is inside is made on the file as opened, not on its name
/// beforehand. A name checked and then opened can be swapped for a link in
/// between, and the one added for a folder (`index.html`) was never checked
/// at all -- a link by that name in a shared folder served whatever it led to.
/// Asking the open file where it really is answers both: whatever the path
/// went through on the way, this is the file that will be sent
fn found_at(folders: &HashMap<String, PathBuf>, url: &str) -> Option<(std::fs::File, PathBuf)> {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let mut parts = path.trim_start_matches('/').splitn(2, '/');
    let root = folders.get(parts.next()?)?;
    let mut at = root.clone();
    for part in parts.next().unwrap_or("").split('/').filter(|p| !p.is_empty()) {
        let part = percent_decode(part);
        // Each part is one name: no climbing, no drive, no second separator,
        // no stream of a file (`a.html:x` names an alternate data stream on
        // NTFS), and no control character -- a NUL ends a name early wherever
        // the path reaches C
        if part.starts_with('.') || part.ends_with(['.', ' ']) || part.contains(['/', '\\', ':']) || part.chars().any(char::is_control) {
            return None;
        }
        at.push(part);
    }
    if std::fs::metadata(&at).ok()?.is_dir() {
        at.push("index.html");
    }
    if !public_file(&at) {
        return None;
    }
    let open = std::fs::File::open(&at).ok()?;
    let real = opened_at(&open)?;
    let relative = real.strip_prefix(root).ok()?;
    if relative.components().any(|p| p.as_os_str().to_string_lossy().starts_with('.')) {
        return None;
    }
    (public_file(&real) && open.metadata().ok()?.is_file()).then_some((open, real))
}

/// Opening HTML must not publish a project's credentials and configuration.
/// An allowlist also excludes backups, keys, source maps and arbitrary data.
fn public_file(file: &Path) -> bool {
    let name = file.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    if name.starts_with('.') || name.ends_with(['.', ' ']) || crate::caps::is_forbidden(file) {
        return false;
    }
    let ext = file.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    matches!(ext.as_str(), "html" | "htm" | "xhtml" | "css" | "js" | "mjs" | "svg" |
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "ico" | "woff" | "woff2" |
        "ttf" | "otf" | "wasm" | "pdf" | "mp4" | "webm" | "mp3" | "wav" | "ogg")
}

/// Where an open file really is: the path the system gives for the handle,
/// with every link and junction on the way already followed, in the same
/// spelling `std::fs::canonicalize` gives a shared folder (`\\?\D:\...` on
/// Windows) so the two compare part by part
#[cfg(windows)]
fn opened_at(file: &std::fs::File) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS};
    let handle = file.as_raw_handle();
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: the handle is the open file's and lives as long as `file`;
        // the buffer is as long as said
        let n = unsafe {
            GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), buf.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS)
        } as usize;
        if n == 0 {
            return None;
        }
        // Too small: the answer is the size needed, the end NUL included
        if n >= buf.len() {
            buf.resize(n + 1, 0);
            continue;
        }
        return Some(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
    }
}

/// On Linux the open descriptor names its file under /proc. A system without
/// that has no answer here, and a file whose place cannot be told is not
/// served: refusing is the safe side of not knowing
#[cfg(unix)]
fn opened_at(file: &std::fs::File) -> Option<PathBuf> {
    use std::os::unix::io::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()
}

#[cfg(not(any(windows, unix)))]
fn opened_at(_file: &std::fs::File) -> Option<PathBuf> {
    None
}

/// What a file is, by its ending, as a browser wants to be told. Text is not
/// given a character set: the page's own `<meta charset>` is the one to read
fn kind_of(file: &Path) -> &'static str {
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "xhtml" => "application/xhtml+xml",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "txt" | "md" | "log" | "csv" => "text/plain",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}

/// One part of an address, every byte outside the plain set as `%XX`
fn percent_encode(part: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for b in part.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(*b as char),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// `%XX` read back into bytes, and those as UTF-8
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_cannot_read_sibling_credentials_data_or_hidden_assets() {
        let root = crate::test_temp("localpage-secrets");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("index.html"), "page").unwrap();
        let files = [".env", ".env.local", "secrets.json", "config.json", "private.pem", "id_rsa", "backup.txt", "data.json", "code.js.map", ".git/private.js"];
        for file in files { std::fs::write(root.join(file), "dummy secret").unwrap(); }
        let folders = [("k".into(), root.canonicalize().unwrap())].into_iter().collect();
        assert!(found_at(&folders, "/k/index.html").is_some());
        for file in files {
            assert!(found_at(&folders, &format!("/k/{file}")).is_none(), "served {file}");
        }
        assert!(found_at(&folders, "/k/%2eenv").is_none());
        assert!(found_at(&folders, "/k/secrets.json%20").is_none());
        assert!(address(&root.join("secrets.json").display().to_string()).unwrap().is_err());
        // Check the handle's actual target too, not just its innocent alias.
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(root.join("secrets.json"), root.join("public.js"));
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(root.join("secrets.json"), root.join("public.js"));
        if linked.is_ok() { assert!(found_at(&folders, "/k/public.js").is_none()); }
        std::fs::remove_dir_all(root).unwrap();
    }

    /// What reads as a file here, and what is a web address or words
    #[test]
    fn a_path_on_this_pc_is_told_from_an_address() {
        let p = |s: &str| local_file(s).map(|p| p.display().to_string().replace('\\', "/"));
        assert_eq!(p("D:/cargo-target/tab-faces.html").as_deref(), Some("D:/cargo-target/tab-faces.html"));
        assert_eq!(p(" d:\\site\\a b.html ").as_deref(), Some("d:/site/a b.html"));
        assert_eq!(p("file:///D:/site/a%20b.html").as_deref(), Some("D:/site/a b.html"));
        assert_eq!(p("FILE:///C:/x.html").as_deref(), Some("C:/x.html"));
        assert_eq!(p("file://server/share/a.html").as_deref(), Some("//server/share/a.html"));
        assert_eq!(p("\\\\server\\share\\a.html").as_deref(), Some("//server/share/a.html"));
        for not in ["example.com/a.html", "https://example.com/", "D:", "D:a.html", "localhost:8000", "日本語"] {
            assert_eq!(p(not), None, "{not} was read as a file");
        }
        // What a terminal's link offers to show in a browser tab
        for shown in ["a.html", "B.HTM", "c.svg", "d.pdf", "e.png"] {
            assert!(shows_in_page(Path::new(shown)), "{shown} is not offered");
        }
        for not in ["a.rs", "b.exe", "c", "d.json"] {
            assert!(!shows_in_page(Path::new(not)), "{not} is offered");
        }
    }

    /// A file is served with its folder, and nothing outside that folder is
    #[test]
    fn a_file_is_served_with_its_folder_and_nothing_above_it() {
        let root = crate::test_temp("localpage");
        let _ = std::fs::remove_dir_all(&root);
        let site = root.join("site");
        std::fs::create_dir_all(site.join("img")).unwrap();
        std::fs::write(site.join("page one.html"), "<p>hi</p>").unwrap();
        std::fs::write(site.join("img").join("a.svg"), "<svg/>").unwrap();
        std::fs::write(site.join("index.html"), "home").unwrap();
        std::fs::write(root.join("secret.txt"), "no").unwrap();

        let url = address(&site.join("page one.html").display().to_string()).unwrap().unwrap();
        assert!(url.starts_with("http://127.0.0.1:") && url.ends_with("/page%20one.html"), "{url}");
        assert!(shikisha_shared::is_openable(&url), "the served address is not one a tab opens: {url}");
        let base = url.rsplit_once('/').unwrap().0.to_string();
        // The same folder is shared once, under the same key
        let again = address(&site.join("index.html").display().to_string()).unwrap().unwrap();
        assert!(again.starts_with(&format!("{base}/")), "{again} is not under {base}");

        let get = |u: &str| -> (u16, String, String) {
            let rest = u.strip_prefix("http://").unwrap();
            let (host, path) = rest.split_once('/').unwrap();
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(host).unwrap();
            write!(s, "GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
            let mut got = String::new();
            s.read_to_string(&mut got).unwrap();
            let status: u16 = got.split(' ').nth(1).unwrap().parse().unwrap();
            let kind = got.lines().find(|l| l.to_ascii_lowercase().starts_with("content-type:")).unwrap_or("").to_string();
            let body = got.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            (status, kind, body)
        };
        let (status, kind, body) = get(&url);
        assert_eq!((status, body.as_str()), (200, "<p>hi</p>"));
        assert!(kind.contains("text/html"), "{kind}");
        assert_eq!(get(&format!("{base}/img/a.svg")).0, 200, "a file below the folder was not served");
        assert_eq!(get(&format!("{base}/")).2, "home", "the folder is not its index.html");
        for out in ["/../secret.txt", "/%2e%2e/secret.txt", "/..%2fsecret.txt", "/img/..%5c..%5csecret.txt"] {
            assert_eq!(get(&format!("{base}{out}")).0, 404, "{out} reached above the folder");
        }
        assert_eq!(get(&url.replace(&base[base.rfind('/').unwrap() + 1..], "0000")).0, 404, "an unknown key was answered");

        // A file that is not there, and a folder, are said as such
        assert!(address(&site.join("gone.html").display().to_string()).unwrap().is_err());
        assert!(address(&site.display().to_string()).unwrap().is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A shared folder with links in it that lead out: none of them serves
    /// what is outside. The one by the name a folder is opened at
    /// (`index.html`) was the hole -- it was added after the check
    #[test]
    fn a_link_inside_the_folder_never_serves_what_is_outside_it() {
        let root = crate::test_temp("localpage-links");
        let _ = std::fs::remove_dir_all(&root);
        let site = root.join("site");
        let outside = root.join("outside");
        std::fs::create_dir_all(site.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(site.join("page.html"), "in").unwrap();
        std::fs::write(outside.join("index.html"), "OUTSIDE").unwrap();
        std::fs::write(outside.join("secret.txt"), "OUTSIDE").unwrap();
        let real_site = std::fs::canonicalize(&site).unwrap();
        let folders: HashMap<String, PathBuf> = [("k".to_string(), real_site)].into_iter().collect();
        let got = |url: &str| found_at(&folders, url).map(|(_, p)| p);

        assert!(got("/k/page.html").is_some(), "the page itself is not served");
        // Names that are not one plain name
        for bad in ["/k/page.html%3Ax", "/k/page.html:x", "/k/page%00.html", "/k/sub/%2e%2e/%2e%2e/outside/secret.txt"] {
            assert!(got(bad).is_none(), "{bad} was served");
        }

        let made = |made: std::io::Result<()>, what: &str| -> bool {
            if made.is_err() {
                eprintln!("skipped: this machine does not let a test make {what}");
            }
            made.is_ok()
        };
        // A link to a file outside, under the name a folder opens at
        #[cfg(windows)]
        let file_link = std::os::windows::fs::symlink_file(outside.join("index.html"), site.join("sub").join("index.html"));
        #[cfg(unix)]
        let file_link = std::os::unix::fs::symlink(outside.join("index.html"), site.join("sub").join("index.html"));
        if made(file_link, "a file link") {
            assert!(got("/k/sub/").is_none(), "a folder's index.html led outside and was served");
            assert!(got("/k/sub/index.html").is_none(), "a link to a file outside was served");
        }
        // A folder inside that leads outside: a junction on Windows, which
        // needs no privilege, so this half runs on every Windows machine
        #[cfg(windows)]
        let dir_link = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(site.join("away"))
            .arg(&outside)
            .output()
            .map_err(std::io::Error::other)
            .and_then(|o| if o.status.success() { Ok(()) } else { Err(std::io::Error::other("mklink")) });
        #[cfg(unix)]
        let dir_link = std::os::unix::fs::symlink(&outside, site.join("away"));
        if made(dir_link, "a folder link") {
            assert!(got("/k/away/").is_none(), "a folder link's index.html outside was served");
            assert!(got("/k/away/secret.txt").is_none(), "a file through a folder link was served");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
