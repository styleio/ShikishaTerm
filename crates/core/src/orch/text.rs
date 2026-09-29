//! What is said to the AIs taking part: the brief a worker is handed, the
//! line that tells a tab it has mail, and the guide a lead reads.
//!
//! English, as every instruction this app gives an AI is; the AI answers the
//! person in the person's own language. Each is written in one place and
//! nowhere else, so what a worker is told and what the commands accept cannot
//! drift apart.
//!
//! **Short on rules.** What must hold is held by the commands themselves (see
//! `orch`): a second report is answered "already", a tab already at work is
//! refused, and every answer carries the command to run next. The words here
//! explain; they do not police. A rule an AI has to remember is a rule it
//! will one day forget.

/// Typed before the brief, for a CLI that follows pasted text only when the
/// person's own typed words ask it to (the profile's `paste_needs_typed_request`)
pub const LEAD_LINE: &str =
    "Another AI tab in SHIKISHA-TERM has handed you the task pasted below. Please do it as the brief says.";

/// What a worker is handed: whose work it is, how to answer, and the task.
/// `nest` adds how to hand parts on, when the depth allows it
pub fn brief(assignment: i64, task: i64, body: &str, nest: bool) -> String {
    let nested = if nest {
        "\n\nPart of this better done by another AI tab? You may hand it on and lead\n\
that part yourself: `shikisha skill orchestration` explains how. Report here\n\
once everything you handed on is back."
    } else {
        ""
    };
    format!(
        "SHIKISHA-TERM: task t{task}, handed to this tab as assignment a{assignment}.
The AI tab that handed it to you does not watch this screen. It hears from
you only through the commands below.

When you finish -- or cannot -- report once:
    shikisha report done \"<what you did>\" \"<what you found>\" \"<what is left, or: nothing>\"
    (failed in place of done when it is not done)

Stuck on something only that tab can settle? Ask it, not the person:
    shikisha ask_lead \"<question>\"

It may send you more while you work. Look now and then, and once more
before you report:
    shikisha inbox

Once you have reported, stop and wait at the prompt.
If the person tells you something directly, that comes first.{nested}

--- t{task} ---
{body}"
    )
}

/// Typed into a tab that has mail and is not already waiting for it
pub fn mail_line(count: usize) -> String {
    format!("[shikisha] New mail for this tab ({count}). Read it with: shikisha inbox")
}

/// The whole guide for a lead, printed by `shikisha skill orchestration`.
/// Kept in the program so the guide and the commands it describes are always
/// the same version
pub fn guide() -> String {
    r#"# Orchestration: handing parts of a job to other AI tabs and seeing it through

For when the person asks you to have other tabs do parts of a job and to see
it through ("have <@claude> implement it and <@codex> review it until nothing is
left", "split this across tabs"). You lead the job: you cut it into tasks,
hand each to a tab, read what comes back, and decide what happens next. One
question to one tab needs none of this: `shikisha ask_tab` does that.

Every command answers with `next`: what to do after it, as the exact command
wherever there is one. Do it. A refusal says what to run instead.
Lost track, or your conversation was summarised? `shikisha job_status` shows
where everything stands.

## The round

    shikisha job_open "<the whole job, in one sentence>"
    shikisha task_add "<task>"                         # one per piece
    shikisha task_add "<task>" '{"waits_on":["t1"]}'   # a piece that needs t1 done first
    shikisha assign t1 <tab>                           # hand an open task to a tab
    shikisha inbox wait                                # wait for what comes back

`inbox wait` answers with mail, or with `NOTHING YET` after a while. NOTHING
YET only means nothing came yet: wait again. The same mail keeps coming back
until you say you have dealt with it, so act on every piece of it first, then
run the `next` it gives -- that says so and waits for more.

What the mail can be:

- **report** -- a tab finished its task (done) or could not (failed). Decide
  what follows: give the same tab its next task (`assign`), keep it open
  (`keep`), or let it go (`let_go`). Failed means not done: read why, then
  assign it again, add a task that removes the cause, or ask the person.
- **question** -- a worker waits on you: `shikisha answer q<N> "<answer>"`.
- **alert** -- something needs you: a tab went quiet without reporting, waits
  for the person's approval, or its program ended. It says what to do.
- **decision** -- a decision you asked for was made.

When everything is done: `shikisha job_close "<what was done>"`. While
anything is still open -- a task not done among them -- it refuses, and
lists each thing with the command that settles it. A task that turned out
not to be needed is taken out on purpose, with why:
`shikisha task_drop t<N> "<why>"`. A failed or stopped task is tried again
by assigning it again.

## Which tabs

- Tabs the person named with @ when they asked you, and tabs this job opened.
- Named none? Open one: `shikisha open_ai_tab codex` (or claude, gemini; add
  `'{"folder":"<folder>"}'` for another folder). Its answer names the tab.
- Two tabs about to edit the same files? Give one its own working copy:
  `shikisha worktree_add <branch>`, then open the tab in the folder it names.

## Writing a task

The tab you hand it to has not seen this conversation. Put in the task:

1. **Where** -- the repository, folder, branch and files it is about.
2. **What to end up with** -- the change or the answer, concretely.
3. **What must not change** -- files others are editing, behaviour to keep,
   style to follow.
4. **How to tell it is done** -- the test to pass, the output to show, what
   to put in the report.

A review task names the branch, commit or files to review. A tab in another
folder does not see uncommitted changes: commit first, or paste the diff into
the task.

## What stays with you

- Findings from a review are fixed by a tab you assign, not by you -- unless
  the person asked you to fix them yourself.
- Merge or push only when the person asked for it.
- Something the person should decide (merging to main, anything that cannot
  be undone): `shikisha decision_open t<N> "<question>" '{"choices":["yes","no"],"to":"person"}'`,
  then wait; the decision arrives in your mail.
- To tell a working tab something: `shikisha tell a<N> "<text>"` (`workers`
  reaches all of them). It reads it the next time it looks.
- To stop one: `shikisha stop a<N>`; every one: `shikisha stop all`.
- A quiet tab is not a dead one. Retry or stop only on evidence; your mail
  tells you when a tab's program has ended.

Talk to the person in their own language.
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brief_names_its_assignment_and_ends_with_the_task() {
        let b = brief(5, 3, "Fix the parser.", false);
        assert!(b.contains("a5") && b.contains("t3"));
        assert!(b.trim_end().ends_with("Fix the parser."));
        assert!(!b.contains("skill orchestration"), "no nesting unless the depth allows it");
        assert!(brief(5, 3, "x", true).contains("skill orchestration"));
    }

    #[test]
    fn nothing_said_to_an_ai_names_a_tab_the_way_a_person_does() {
        // `<@ID>` in a person's request is what lets an AI drive a terminal or
        // a page; words this app types must never read as that
        for text in [brief(1, 1, "x", true), mail_line(2), guide(), LEAD_LINE.to_string()] {
            assert!(!text.contains("<@") || text.contains("<@claude>"), "{text}");
        }
        assert!(!brief(1, 1, "x", true).contains("<@"));
        assert!(!mail_line(3).contains("<@"));
    }

    #[test]
    fn the_brief_and_the_guide_use_the_commands_that_exist() {
        let said = format!("{}\n{}", brief(1, 1, "x", true), guide());
        for word in said.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).collect::<Vec<_>>().windows(2) {
            if word[0] == "shikisha" && !word[1].is_empty() && word[1].chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                let known = super::super::METHODS.contains(&word[1])
                    || ["skill", "ask_tab", "open_ai_tab", "worktree_add", "tab_conversation"].contains(&word[1]);
                assert!(known, "`shikisha {}` is not a command", word[1]);
            }
        }
    }
}
