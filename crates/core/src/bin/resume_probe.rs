//! Why a tab did or did not come back to the conversation it was having.
//!
//! The decision is made once, at launch, and it says nothing when the answer
//! is "start clean" -- which is the normal answer, and which is why a tab that
//! should have come back and did not leaves no trace at all. This asks the
//! same question the launch asks, against a real install's settings and a real
//! `last-session`, with nothing running, and prints every part of the answer.
//!
//!     cargo run -p shikisha-core --bin resume_probe -- <install folder>
//!
//! The install folder is the one the app runs out of: its `config/config.json`
//! and its `data/last-session`.

use shikisha_core::{config, desk, lastsession, tab, view};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [at] = args.as_slice() else {
        eprintln!("usage: resume_probe <install folder>");
        std::process::exit(2);
    };
    let root = std::path::Path::new(at);
    let cfg_at = root.join("config").join("config.json");
    let saved_at = root.join("data").join("last-session");

    let text = std::fs::read_to_string(&cfg_at).unwrap_or_else(|e| {
        eprintln!("{}: {e}", cfg_at.display());
        std::process::exit(2);
    });
    let cfg: config::Config = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .unwrap_or_else(|e| {
            eprintln!("{}: {e}", cfg_at.display());
            std::process::exit(2);
        });
    let text = std::fs::read_to_string(&saved_at).unwrap_or_else(|e| {
        eprintln!("{}: {e}", saved_at.display());
        std::process::exit(2);
    });
    let saved: lastsession::Saved = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .unwrap_or_else(|e| {
            eprintln!("{}: {e}", saved_at.display());
            std::process::exit(2);
        });

    println!("settings: {}", cfg_at.display());
    println!("last session: {}\n", saved_at.display());
    for w in &saved.desks {
        println!(
            "remembered desk {:?} (id {:?}): {} tab(s)",
            w.name,
            w.id.as_deref().unwrap_or("-"),
            w.tabs.len()
        );
        for t in &w.tabs {
            println!(
                "    title={:?} id={:?} program={:?} cwd={:?} session={} source={}",
                t.title,
                t.id.as_deref().unwrap_or("-"),
                t.program,
                t.cwd.as_deref().unwrap_or("-"),
                t.session,
                t.source
            );
        }
    }
    println!();

    let (desks, errors) = cfg.resolve_desks();
    for e in &errors {
        println!("settings problem: {e}");
    }
    for d in &desks {
        println!("desk {:?} (id {:?})", d.name, d.id);
        for ft in &d.tabs {
            let argv = ft.cfg.command.argv();
            if argv.is_empty() || config::is_app_panel(&argv) {
                continue;
            }
            let title = ft.cfg.name.clone().unwrap_or_else(|| view::title_of(&argv));
            let mut opts = desk::tab_options(&ft.cfg, d.folder_of(ft));
            let argv = desk::resolve_launch(argv, &mut opts, Some(d), &ft.cfg);
            let cwd = opts.cwd.clone();
            let far = opts.remote.is_some() || opts.cloud.is_some();
            let program = argv.first().cloned().unwrap_or_default();
            println!(
                "  tab title={title:?} id={:?} program={program:?} cwd={:?}",
                ft.cfg.id.as_deref().unwrap_or("-"),
                cwd.as_ref().map(|c| c.display().to_string()).unwrap_or_default()
            );
            if ft.cfg.restore_conversation == Some(false) {
                println!("    -> fresh: this tab is set to start clean");
                continue;
            }
            let found = saved.conversation_of(
                d,
                &program,
                cwd.as_ref().map(|c| c.display().to_string()).as_deref(),
                ft.cfg.id.as_deref(),
                &title,
            );
            match &found {
                None => {
                    let near = saved.remembered_here(
                        d,
                        &program,
                        cwd.as_ref().map(|c| c.display().to_string()).as_deref(),
                    );
                    match near {
                        0 => println!("    -> fresh: nothing remembered matches this tab"),
                        n => println!(
                            "    -> fresh: {n} conversation(s) remembered for {program} in this \
                             folder, and none of them could be told to be this tab's"
                        ),
                    }
                }
                Some(s) => {
                    // On another machine its record is there, looked for as
                    // the tab starts, and not on this PC
                    let ok = tab::resumable_at(&argv, &ft.cfg.profile, &s.id, far);
                    // The hash beside it is what the log calls this same
                    // conversation (`Session::short`). Printed together so a
                    // line in hooks.log can be matched to a record on the disk
                    // without anyone having to work out which is which
                    println!(
                        "    remembered {} (#{}, {:?}); its record is {}",
                        s.id,
                        s.digest(),
                        s.source,
                        match ok {
                            true => "still there",
                            false => "GONE (so the tab starts clean)",
                        }
                    );
                }
            }
            let carried = desk::carried_conversation(Some(&saved), d, &argv, &ft.cfg, &cwd, &title, far);
            // Whose conversation the tab would come up on. The CLI's own
            // records say where each one was had, and a tab carrying one that
            // was had somewhere else is the shape of a report written down
            // against the wrong tab -- two tabs on one conversation, and
            // another tab's gone
            let elsewhere = match (&carried.plan, cwd.as_deref()) {
                (tab::Resume::Id(s), Some(at)) => {
                    let prog = argv.first().map(String::as_str).unwrap_or_default();
                    !shikisha_core::vault::belongs(prog, at, &s.id)
                }
                _ => false,
            };
            match carried.plan {
                tab::Resume::Id(s) => println!("    -> carries {} (#{})", s.id, s.digest()),
                other => println!("    -> {other:?}"),
            }
            if elsewhere {
                println!("    -> but that conversation was had in ANOTHER FOLDER (somebody else's)");
            }
            if carried.lost || elsewhere {
                println!("    -> and says so: the tab keeps the offer of the way back");
            }
        }
    }
}
