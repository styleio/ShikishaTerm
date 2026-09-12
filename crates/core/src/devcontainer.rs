//! What a project says its environment should be.
//!
//! A folder cut on a machine that was made a second ago holds the source and
//! nothing else: no toolchain, no dependencies, none of the things a person
//! would have installed years ago on their own machine and forgotten about.
//! Something has to say what those are, and the format the world already uses
//! to say it is `devcontainer.json` -- read by VS Code, Codespaces, JetBrains
//! and others, and **already sitting in a great many repositories**.
//!
//! So this reads that, rather than inventing a third dialect. What is taken is
//! the part that survives without a container runtime here: the image, the
//! commands that finish the setup, the environment, and the ports. Features
//! are named but not resolved -- they need the devcontainer CLI, and a setup
//! that silently skipped half of itself is worse than one that says so.

use std::path::{Path, PathBuf};

/// A project's environment, as its own repository describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Env {
    /// The file this came from, so what is on screen can be checked
    pub from: String,
    /// What the machine should be built from. An image name, or the Dockerfile
    /// named by `build.dockerfile`
    pub image: Option<String>,
    /// The commands that finish the setup, in the order the spec runs them:
    /// onCreate, updateContent, postCreate, postStart
    pub setup: Vec<String>,
    /// Variables every process in there should see
    pub env: Vec<(String, String)>,
    /// Ports the project expects to be reachable
    pub ports: Vec<u16>,
    /// Where the source belongs
    pub folder: Option<String>,
    /// Who it runs as
    pub user: Option<String>,
    /// Parts this cannot put in place on its own. Named rather than dropped:
    /// a setup that skipped half of itself in silence is the worst of both
    pub unresolved: Vec<String>,
}

impl Env {
    /// Whether there is anything here worth doing.
    pub fn any(&self) -> bool {
        self.image.is_some() || !self.setup.is_empty() || !self.env.is_empty()
    }
}

/// Where the spec says the file may be, in the order it says to look.
fn candidates(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![
        root.join(".devcontainer").join("devcontainer.json"),
        root.join(".devcontainer.json"),
    ];
    // One folder deep under .devcontainer, which is how a repository carries
    // more than one of these
    if let Ok(entries) = std::fs::read_dir(root.join(".devcontainer")) {
        let mut deeper: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("devcontainer.json"))
            .filter(|p| p.is_file())
            .collect();
        deeper.sort();
        out.extend(deeper);
    }
    out
}

/// What this project says its environment is, or nothing if it does not say.
pub fn of(root: &Path) -> Option<Env> {
    let at = candidates(root).into_iter().find(|p| p.is_file())?;
    let text = std::fs::read_to_string(&at).ok()?;
    let mut env = read(&text)?;
    env.from = at.display().to_string();
    Some(env)
}

/// The same, from the text itself.
pub fn read(text: &str) -> Option<Env> {
    let v: serde_json::Value = serde_json::from_str(&plain(text)).ok()?;
    let mut env = Env::default();
    env.image = v
        .get("image")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .or_else(|| {
            v.get("build")
                .and_then(|b| b.get("dockerfile"))
                .and_then(|x| x.as_str())
                .map(str::to_string)
        });
    // The spec's own order. A setup run out of order is a setup that fails on
    // the machines where the order mattered, which is not every machine
    for key in ["onCreateCommand", "updateContentCommand", "postCreateCommand", "postStartCommand"] {
        if let Some(c) = v.get(key) {
            env.setup.extend(lines(c));
        }
    }
    if let Some(map) = v.get("containerEnv").and_then(|x| x.as_object()) {
        for (k, val) in map {
            if let Some(s) = val.as_str() {
                env.env.push((k.clone(), s.to_string()));
            }
        }
    }
    if let Some(ports) = v.get("forwardPorts").and_then(|x| x.as_array()) {
        for p in ports {
            // A port may be written as a number or as "host:port"
            if let Some(n) = p.as_u64() {
                env.ports.push(n as u16);
            } else if let Some(s) = p.as_str() {
                if let Ok(n) = s.rsplit(':').next().unwrap_or(s).parse::<u16>() {
                    env.ports.push(n);
                }
            }
        }
    }
    env.folder = v.get("workspaceFolder").and_then(|x| x.as_str()).map(str::to_string);
    env.user = v
        .get("remoteUser")
        .or_else(|| v.get("containerUser"))
        .and_then(|x| x.as_str())
        .map(str::to_string);
    if let Some(f) = v.get("features").and_then(|x| x.as_object()) {
        env.unresolved.extend(f.keys().cloned());
    }
    Some(env)
}

/// One command, however the spec lets it be written.
///
/// Three shapes: a line for a shell, a list that is a program and its
/// arguments, and a map of several to be run at once. The map is flattened in
/// the order it was written -- running them at once is an optimisation, and
/// one at a time is never wrong
fn lines(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) => vec![s.clone()],
        // A list is a program and its arguments, already apart. Joined with a
        // space is wrong the moment an argument holds one, so each is quoted
        serde_json::Value::Array(a) => {
            let words: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            match words.is_empty() {
                true => Vec::new(),
                false => vec![shell_words(&words)],
            }
        }
        serde_json::Value::Object(m) => m.values().flat_map(lines).collect(),
        _ => Vec::new(),
    }
}

/// A program and its arguments, as one line a shell will read back the same way.
fn shell_words(words: &[String]) -> String {
    words
        .iter()
        .map(|w| match w.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c)) {
            true => w.clone(),
            false => format!("'{}'", w.replace('\'', "'\\''")),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The file without the parts JSON does not allow.
///
/// These files are written with comments and trailing commas -- the tools that
/// read them all accept both, and a repository that has one is not wrong. A
/// reader that refused would refuse the majority of real ones.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let (mut in_string, mut escaped) = (false, false);
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
            }
            _ => out.push(c),
        }
    }
    // A comma with nothing after it but the end of its list
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            cleaned.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            cleaned.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let next = bytes[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                i += 1;
                continue;
            }
        }
        cleaned.push(c);
        i += 1;
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Comments and trailing commas are in real files, and a reader that
    /// refused them would refuse most of the ones that exist.
    #[test]
    fn a_file_written_the_way_people_write_them_is_read() {
        let e = read(
            r#"{
              // the image this project wants
              "image": "mcr.microsoft.com/devcontainers/rust:1",
              /* several ways to say a command */
              "postCreateCommand": "cargo fetch",
              "forwardPorts": [8080, "127.0.0.1:5432"],
              "containerEnv": { "RUST_LOG": "debug" },
              "remoteUser": "vscode",
            }"#,
        )
        .expect("読めない");
        assert_eq!(e.image.as_deref(), Some("mcr.microsoft.com/devcontainers/rust:1"));
        assert_eq!(e.setup, ["cargo fetch"]);
        assert_eq!(e.ports, [8080, 5432]);
        assert_eq!(e.env, [("RUST_LOG".to_string(), "debug".to_string())]);
        assert_eq!(e.user.as_deref(), Some("vscode"));
        assert!(e.any());
    }

    /// The spec's order is the order they run in. Out of order is a setup that
    /// works until it meets a project where the order mattered.
    #[test]
    fn the_commands_run_in_the_order_the_spec_gives() {
        let e = read(
            r#"{
              "postCreateCommand": "third",
              "onCreateCommand": "first",
              "updateContentCommand": ["npm", "ci"],
              "postStartCommand": { "a": "fourth", "b": "fifth" }
            }"#,
        )
        .expect("読めない");
        assert_eq!(e.setup[0], "first");
        assert_eq!(e.setup[1], "npm ci", "配列は1行になる");
        assert_eq!(e.setup[2], "third");
        assert_eq!(e.setup.len(), 5, "まとめて走らせる指定も1つずつ拾う: {:?}", e.setup);
    }

    /// A word with a space in it is one word, and stays one word.
    #[test]
    fn an_argument_with_a_space_survives_becoming_a_line() {
        let e = read(r#"{"onCreateCommand": ["echo", "two words", "it's"]}"#).expect("読めない");
        assert_eq!(e.setup, [r#"echo 'two words' 'it'\''s'"#]);
    }

    /// What cannot be put in place is named, never dropped.
    #[test]
    fn the_parts_that_need_another_tool_are_said_out_loud() {
        let e = read(
            r#"{"image":"x","features":{"ghcr.io/devcontainers/features/node:1":{},"ghcr.io/devcontainers/features/go:1":{}}}"#,
        )
        .expect("読めない");
        assert_eq!(e.unresolved.len(), 2, "{:?}", e.unresolved);
        assert!(e.unresolved.iter().any(|f| f.contains("node")));
    }

    /// A project that says nothing is not an error; most say nothing.
    #[test]
    fn a_project_with_nothing_to_say_says_nothing() {
        let none = read("{}").expect("空でも読める");
        assert!(!none.any());
        assert!(read("not json at all").is_none());
        let at = std::env::temp_dir().join("shikisha-dc-none");
        let _ = std::fs::create_dir_all(&at);
        assert!(of(&at).is_none(), "無いのに何か返している");
    }

    /// The build's Dockerfile stands in for an image when there is no image.
    #[test]
    fn a_dockerfile_is_what_it_is_built_from() {
        let e = read(r#"{"build":{"dockerfile":"Dockerfile","context":".."}}"#).expect("読めない");
        assert_eq!(e.image.as_deref(), Some("Dockerfile"));
    }
}
