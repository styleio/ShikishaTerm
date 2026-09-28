//! What the bridge does on another machine when this PC asks, by name.
//!
//! The bridge (`shikisha-bridge`, see `farlink`) runs on a server or a MicroVM
//! a person agreed to put it on. It does two jobs: it carries the `shikisha`
//! command of the tabs there to this app, and it does small things on that
//! machine that would otherwise be one remote command after another -- reading
//! a conversation record, above all.
//!
//! **One table.** Each operation is one line in [`OPS`] and one function. The
//! functions are this crate's own, the same ones this PC uses on its own files,
//! so reading a record there and here is one piece of code and not two. Adding
//! an operation is a line and a function; nothing else has to learn its name.

use std::path::PathBuf;

use serde_json::{Value, json};

type Op = fn(&Value) -> Result<Value, String>;

/// Every operation, by the name this PC sends
pub const OPS: &[(&str, Op)] = &[
    ("ping", ping),
    ("read_page", read_page),
    ("put_key", put_key),
    ("drop_key", drop_key),
];

/// Carry out one operation
pub fn run(op: &str, params: &Value) -> Result<Value, String> {
    match OPS.iter().find(|(name, _)| *name == op) {
        Some((_, f)) => f(params),
        None => Err(format!("the bridge here has no operation called {op}; it may be an older version")),
    }
}

/// The folder the bridge keeps its things in on its machine. Set once, by the
/// bridge, when it starts
static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_home(dir: PathBuf) {
    let _ = HOME.set(dir);
}

fn home() -> Result<&'static PathBuf, String> {
    HOME.get().ok_or_else(|| "the bridge has no folder".to_string())
}

/// Where a tab's key is kept on the bridge's machine. Only a name this app
/// could have given a tab: nothing that climbs out of the folder
pub fn key_file(dir: &std::path::Path, tab: &str) -> Option<PathBuf> {
    let ok = !tab.is_empty() && tab.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !tab.starts_with('.');
    ok.then(|| dir.join("keys").join(tab))
}

fn text<'a>(p: &'a Value, key: &str) -> Result<&'a str, String> {
    p.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing {key}"))
}

fn ping(_: &Value) -> Result<Value, String> {
    Ok(json!({"version": env!("CARGO_PKG_VERSION"), "rev": crate::build_rev()}))
}

/// A page of a conversation record, found by the profile's pattern and the
/// conversation's id -- read here, with the reader this PC uses on its own
/// records. Null when the CLI has not written the record yet
fn read_page(p: &Value) -> Result<Value, String> {
    let glob = text(p, "glob")?;
    let id = text(p, "id")?;
    let before = p.get("before").and_then(Value::as_u64).unwrap_or(u64::MAX);
    let want = p.get("want").and_then(Value::as_u64).unwrap_or(6) as usize;
    let Some(path) = crate::sessionfind::locate(glob, id) else {
        return Ok(Value::Null);
    };
    let page = crate::reader::read_back(&path, before, want).map_err(|e| e.to_string())?;
    serde_json::to_value(page).map_err(|e| e.to_string())
}

/// A tab's key, for the `shikisha` command in that tab to show this app. In a
/// file only this account can read, and never on a command line
fn put_key(p: &Value) -> Result<Value, String> {
    let tab = text(p, "tab")?;
    let key = text(p, "key")?;
    let file = key_file(home()?, tab).ok_or_else(|| format!("{tab} is not a tab name"))?;
    write_private(&file, key)?;
    Ok(Value::Null)
}

fn drop_key(p: &Value) -> Result<Value, String> {
    let tab = text(p, "tab")?;
    if let Some(file) = key_file(home()?, tab) {
        let _ = std::fs::remove_file(file);
    }
    Ok(Value::Null)
}

/// Written so that only this account can read it, from the moment it exists
fn write_private(file: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(file)
            .map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(file, text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_file_stays_in_its_folder() {
        let d = std::path::Path::new("/home/u/.local/share/shikisha/bridge");
        assert_eq!(key_file(d, "coder"), Some(d.join("keys").join("coder")));
        for bad in ["", "../x", "a/b", ".hidden", "x y"] {
            assert_eq!(key_file(d, bad), None, "{bad}");
        }
    }

    #[test]
    fn an_unknown_operation_says_the_bridge_may_be_old() {
        let e = run("teleport", &Value::Null).unwrap_err();
        assert!(e.contains("older version"));
        assert!(run("ping", &Value::Null).unwrap()["version"].is_string());
    }

    #[test]
    fn a_record_is_read_there_with_the_reader_used_here() {
        let dir = std::env::temp_dir().join(format!("farops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n\
             {\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
        )
        .unwrap();
        let glob = format!("{}/{{id}}.jsonl", dir.display()).replace('\\', "/");
        let page = run("read_page", &json!({"glob": glob, "id": "abc", "want": 2})).unwrap();
        let here = serde_json::to_value(crate::reader::read_back(&file, u64::MAX, 2).unwrap()).unwrap();
        assert_eq!(page, here);
        assert_eq!(run("read_page", &json!({"glob": glob, "id": "nope"})).unwrap(), Value::Null);
        let _ = std::fs::remove_dir_all(dir);
    }
}
