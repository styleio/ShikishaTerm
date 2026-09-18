//! What this app would hear from a CLI's own record of a conversation.
//!
//! A folder names itself from the requests typed into its AI tabs, and those
//! requests are read out of the record each CLI already keeps (`crate::asks`).
//! A record holds far more than what was typed, and telling the two apart is
//! described per CLI in `profiles/<name>.json` under `resume.asks`. When a CLI
//! changes how it writes those files, this is what says so: it prints exactly
//! what would be heard, against a real record, with nothing running.
//!
//!     cargo run -p shikisha-core --bin asks_probe -- claude <record.jsonl>
//!     cargo run -p shikisha-core --bin asks_probe -- codex <record.jsonl>
//!
//! Every line printed should be something a person typed. One that is not is
//! the bug, and the fix is in that CLI's profile, not here.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [name, file] = args.as_slice() else {
        eprintln!("usage: asks_probe <profile name> <record file>");
        std::process::exit(2);
    };
    let profiles = shikisha_core::profile::all();
    let Some(p) = profiles.iter().find(|p| p.name.eq_ignore_ascii_case(name)) else {
        eprintln!(
            "no profile called {name:?}. There is: {}",
            profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
        );
        std::process::exit(2);
    };
    let Some(how) = p.resume.as_ref().and_then(|r| r.asks.as_ref()) else {
        eprintln!("{name} says nothing about reading requests (resume.asks)");
        std::process::exit(2);
    };
    let at = std::path::Path::new(file);
    // The whole record, however long. The app reads a stretch at a time and
    // comes back a second later; here there is no second later, so the looking
    // carries on until it stops moving
    let (mut said, mut read) = (Vec::new(), 0u64);
    loop {
        let (more, now_at) = shikisha_core::asks::read_from(at, how, read);
        said.extend(more);
        if now_at == read {
            break;
        }
        read = now_at;
    }
    println!("{} request(s) heard, {read} bytes read\n", said.len());
    for (i, ask) in said.iter().enumerate() {
        // As the AI writing the name would see it: already cut down
        println!("--- {} ---\n{ask}\n", i + 1);
    }
}
