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
    "Please carry out this task from the AI tab that assigned it to you, following the brief pasted below.";

/// What a worker is handed: who it works for, the four commands it needs, and
/// the task. `nest` adds how to hand work on, when the depth allows it
pub fn brief(dispatch: i64, task: i64, spec: &str, nest: bool) -> String {
    let nested = if nest {
        "\n\nIf part of this is better done by another AI tab, you may lead that part\n\
yourself: run `shikisha skill orchestration` and follow it. Report here only\n\
once everything you handed on has come back."
    } else {
        ""
    };
    format!(
        "You are a worker in SHIKISHA-TERM, on assignment d{dispatch} (task t{task}).
The tab that assigned it cannot see this terminal. Reach it only with these
commands; anything left only in this terminal never gets to it.

    # Report the outcome, exactly once. Use failed when it is not done.
    # The summary is three sentences: what you did, what you found, what is left.
    shikisha report succeeded \"<summary>\"

    # A question only the assigning tab can answer. Never ask the person here.
    shikisha ask_lead \"<question>\"

    # Follow-ups from the assigning tab: read them before starting a new file,
    # after a test run, and once more just before you report.
    shikisha inbox

After you report, stop and wait at the prompt. Do not poll.
A direct instruction from the person comes first and is new work.{nested}

=== TASK t{task} ===
{spec}"
    )
}

/// Typed into a tab that has mail and is not already waiting for it
pub fn pointer(count: usize) -> String {
    let noun = if count == 1 { "message" } else { "messages" };
    format!("[shikisha] You have {count} {noun}. Run: shikisha inbox")
}

/// The whole guide for a lead, printed by `shikisha skill orchestration`.
/// Kept in the program so the guide and the commands it describes are always
/// the same version
pub fn guide() -> String {
    r#"# Orchestration: handing work to other AI tabs and seeing it through

Use this when the person asks you to have other tabs do parts of a job and to
see it through ("have <@claude> implement it and <@codex> review it until
nothing is left", "split this across tabs"). You are the lead: you hand out
tasks, wait for reports, and decide what happens next. For a single question
to one tab, `shikisha ask_tab` is enough; this is for work that needs reports,
several tabs, or several rounds.

Every command answers with `next`: the exact command(s) to run next. Follow
them. `shikisha run_status` tells you where everything stands at any time --
use it whenever you are unsure, or after your conversation was summarised.

## The loop

    shikisha run_open "<the whole job, in one sentence>"
    shikisha task_add "<task>"                    # one per independent piece
    shikisha task_add "<task>" '{"deps":["t1"]}'  # one that waits on t1
    shikisha dispatch t1 <tab>                    # hand each ready task to a tab
    shikisha inbox wait                           # wait for reports and questions

`inbox wait` returns a batch of messages, or `NOTHING YET` after a while.
NOTHING YET is not a failure: run `shikisha inbox wait` again. A batch comes
back until you acknowledge it, so deal with every message in it, then run the
`next` it gives (it acknowledges the batch and waits for the next one).

For each message:

- **report** -- a task is done (succeeded or failed). Decide what follows: hand
  the tab its next task (`dispatch` to the same tab), keep it (`retain`), or
  close it (`release`). Failed means not done: read why, then dispatch again,
  add a task that fixes the cause, or ask the person.
- **question** -- a worker is waiting on you: `shikisha answer q<N> "<answer>"`.
- **escalation** -- something needs you: a tab stopped without reporting, is
  waiting for the person to approve something, or its program ended. It says
  what to do.
- **gate** -- a decision you asked for was made.

When the job is done: `shikisha run_close "<what was done>"`. It refuses while
anything is left open and lists what, with the command for each.

## Which tab

- A tab the person named with @ in what they asked you, or one this job opened.
- If the person named none, open one: `shikisha open_ai_tab codex` (or claude,
  gemini; add `'{"folder":"<folder>"}'` to work elsewhere). Its answer gives the
  tab to dispatch to.
- A separate working copy for a task, when two tabs would edit the same files:
  `shikisha worktree_add <branch>`, then open the tab in the folder it returns.

## Writing a task

The worker sees only the task, not this conversation. Each task says:

- **Target** -- the files, component or environment in scope.
- **Change** -- the concrete result to produce.
- **Constraints** -- what must not change, compatibility, style.
- **Ownership** -- what this worker may edit; what others are editing.
- **Done when** -- the test, output or evidence that proves it.

For a review, name the branch, commit or files, and what to report. A worker in
another folder cannot see uncommitted changes: commit first, or put the diff in
the task.

## Rules that are yours

- A review's findings are fixed by a worker you dispatch, not by you -- unless
  the person asked you to do the fixes yourself.
- Merge or push only when the person asked for it.
- A decision the person should make (merging to main, anything that cannot be
  undone): `shikisha gate_open t<N> "<question>" '{"options":["yes","no"],"to":"person"}'`,
  then wait; the answer arrives in your inbox.
- Tell a worker something while it works: `shikisha tell d<N> "<text>"`
  (`workers` for all of them). It reads it at its next checkpoint.
- Stop a worker with `shikisha stop d<N>`; stop all of them with `shikisha stop all`.
- Only positive evidence justifies stopping or retrying: a tab that is quiet is
  not a tab that is dead. The inbox tells you when a tab's program ended.

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
        assert!(b.contains("d5") && b.contains("t3"));
        assert!(b.trim_end().ends_with("Fix the parser."));
        assert!(!b.contains("skill orchestration"), "no nesting unless the depth allows it");
        assert!(brief(5, 3, "x", true).contains("skill orchestration"));
    }

    #[test]
    fn nothing_said_to_an_ai_names_a_tab_the_way_a_person_does() {
        // `<@ID>` in a person's request is what lets an AI drive a terminal or
        // a page; words this app types must never read as that
        for text in [brief(1, 1, "x", true), pointer(2), guide(), LEAD_LINE.to_string()] {
            assert!(!text.contains("<@") || text.contains("<@claude>"), "{text}");
        }
        assert!(!brief(1, 1, "x", true).contains("<@"));
        assert!(!pointer(3).contains("<@"));
    }
}
