//! Read a tab's actual history without running a window or changing its DB.
//! cargo run -p shikisha-core --bin conversation_probe -- <db> <tab uid>
//!   <live record id> <cwd> <record glob> [cwd field]
//! Prints the panel's JSON to stdout; redirect it into a private file when
//! inspecting real conversations. No marks file is read or written.

use std::path::PathBuf;

use shikisha_core::{convo::read::{Target, answer}, reader::Record};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !(5..=6).contains(&args.len()) {
        return Err("usage: conversation_probe <db> <tab uid> <live record id> <cwd> <record glob> [cwd field]".into());
    }
    let target = Target {
        panel: args[1].clone(),
        tab: Some(args[1].clone()),
        live: Record::named(&args[4], &args[2], None),
        glob: args[4].clone(),
        machine: None,
        past: None,
        cwd: Some(PathBuf::from(&args[3])),
        cwd_field: args.get(5).cloned(),
    };
    let db = PathBuf::from(&args[0]);
    // A child of the DB file cannot be a real marks file. The reader treats
    // missing marks as empty and this probe only calls the read-only page.
    let marks = db.join("unused-marks.json");
    let found = answer(&target, "page", &serde_json::json!({"want": 200}), &db, &marks);
    println!("{}", serde_json::to_string(&found.answer)?);
    if found.answer["ok"] != true {
        return Err("the conversation could not be read".into());
    }
    Ok(())
}
