//! What the ? would answer, asked for real.
//!
//! The panel hands a question to the assistant AI with the index of this
//! program's own settings, and reads the answer back as a shape. Everything in
//! between is a real CLI on this machine: the flags that hold it to that shape
//! are its own, they differ per CLI, and a flag one of them stops accepting is
//! a panel that answers with a paragraph of JSON. Nothing in the test suite can
//! see that -- the tests do not start an AI -- so this does.
//!
//!     cargo run -p shikisha-core --bin guide_probe -- "how do I use my phone"
//!     cargo run -p shikisha-core --bin guide_probe -- --ja "スマホから使いたい"
//!     cargo run -p shikisha-core --bin guide_probe -- --ja --source "閉じたタブを戻したい"
//!
//! `--source` asks the second way: this version's source is fetched (into the
//! state folder beside this build, the first time) and read by the assistant
//! AI with tools that read and nothing else.
//!
//! It prints what was asked, what came back, and whether the answer named a
//! screen this program has. An answer with no `open` is not a failure -- not
//! every question is about one screen -- but a wrong one would be, and a screen
//! that does not exist is dropped before it is ever offered.

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let ja = args.first().is_some_and(|a| a == "--ja");
    if ja {
        args.remove(0);
    }
    let source = args.first().is_some_and(|a| a == "--source");
    if source {
        args.remove(0);
    }
    let question = args.join(" ");
    if question.trim().is_empty() {
        eprintln!("usage: guide_probe [--ja] [--source] <question>");
        std::process::exit(2);
    }
    // The language decides the words it is given and the words it answers in
    // The dictionary sits beside the exe, or two folders above it in a
    // checkout, which is where this is run from
    let here = std::env::current_dir().unwrap_or_default();
    shikisha_core::i18n::init(Some(if ja { "ja" } else { "en" }), &[here]);

    let idx = shikisha_core::guide::index(&shikisha_core::i18n::t);
    println!("screens: {}", idx.pages.len());
    println!("asked:   {question}");
    let began = std::time::Instant::now();
    if source {
        let at = match shikisha_core::source::ready() {
            Ok(at) => at,
            Err(e) => {
                eprintln!("the source could not be fetched: {e:#}");
                std::process::exit(1);
            }
        };
        println!("source:  {} at {}{} ({:.1}s)", at.folder.display(), at.commit, if at.instead { " (main, instead)" } else { "" }, began.elapsed().as_secs_f32());
        match shikisha_core::guide::ask_source(&question, &[], &at) {
            Ok(a) => {
                println!("took:    {:.1}s", began.elapsed().as_secs_f32());
                println!("found:   {}", a.found);
                println!("said:    {}", a.say);
                println!("issue:   {}", shikisha_core::guide::issue_url(&question, &a.say));
            }
            Err(e) => {
                eprintln!("nothing came back: {e:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    match shikisha_core::guide::ask(&question, &[], None) {
        Ok(a) => {
            println!("took:    {:.1}s", began.elapsed().as_secs_f32());
            println!("said:    {}", a.say);
            match a.open.is_empty() {
                true => println!("opens:   (no screen named)"),
                false => {
                    let at = shikisha_core::guide::screen_of(&a.open)
                        .map(shikisha_core::guide::where_it_is)
                        .unwrap_or_default();
                    println!("opens:   {} -- {at}", a.open);
                }
            }
            if !a.fill.is_empty() {
                println!("fills:   {}", a.fill);
            }
        }
        Err(e) => {
            println!("took:    {:.1}s", began.elapsed().as_secs_f32());
            eprintln!("nothing came back: {e:#}");
            std::process::exit(1);
        }
    }
}
