//! Telling two texts apart.
//!
//! One job and nothing above it: two strings in, a diff out. It knows nothing
//! about files, servers or repositories, which is exactly what lets the same
//! command answer "what did the AI change in its second draft", "how does the
//! page on the server differ from the one here", and "what moved between two
//! runs of this script". A command that knew about any one of those would
//! answer only that one.
//!
//! The diff is written the way git writes one, down to the `diff --git` line,
//! so [`crate::git::split_hunks`] cuts it into the same pieces the git panel
//! is already drawn from. One shape for "a change", not two.

use similar::TextDiff;

/// Lines of unchanged text kept around a change when nobody says otherwise.
/// Three is what `diff -u` and git both settled on
pub const CONTEXT: usize = 3;

/// The most context there is any point asking for. Past this the context is
/// the whole of both texts, and a number far past it is somebody's mistake
/// rather than their intent
pub const CONTEXT_MAX: usize = 1000;

/// What the two sides are called when the caller does not say. A diff of two
/// strings is not about a file, but a unified diff has to write some name on
/// its `---` and `+++` lines, and a name that says what it is beats `a` and `b`
pub const UNNAMED: &str = "text";

/// A unified diff of `before` against `after`, empty when they are the same.
///
/// `name` goes on the header lines. `context` is how many unchanged lines are
/// kept either side of a change.
pub fn unified(before: &str, after: &str, name: &str, context: usize) -> String {
    let name = match name.trim() {
        "" => UNNAMED,
        n => n,
    };
    let body = TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(context.min(CONTEXT_MAX))
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string();
    // Nothing changed. Said as an empty string rather than a header with no
    // hunks under it, so that `if diff == ""` is the whole of the question --
    // the same answer `git_diff` gives for a folder with nothing in it
    if body.trim().is_empty() {
        return String::new();
    }
    format!("diff --git a/{name} b/{name}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_texts_that_are_the_same_have_no_diff() {
        assert_eq!(unified("one\ntwo\n", "one\ntwo\n", "x", CONTEXT), "");
        assert_eq!(unified("", "", "", CONTEXT), "");
    }

    #[test]
    fn a_changed_line_is_shown_going_out_and_coming_in() {
        let d = unified("one\ntwo\nthree\n", "one\nTWO\nthree\n", "notes.txt", CONTEXT);
        assert!(d.starts_with("diff --git a/notes.txt b/notes.txt\n"), "{d}");
        assert!(d.contains("--- a/notes.txt"), "{d}");
        assert!(d.contains("+++ b/notes.txt"), "{d}");
        assert!(d.contains("-two"), "{d}");
        assert!(d.contains("+TWO"), "{d}");
    }

    /// The whole reason it is written git's way: the cutter the git panel
    /// already uses has to be able to read it
    #[test]
    fn the_hunk_cutter_reads_what_this_writes() {
        let before = (1..=20).map(|n| format!("line {n}\n")).collect::<String>();
        let after = before.replace("line 10\n", "line ten\n");
        let hunks = crate::git::split_hunks(&unified(&before, &after, "big.txt", CONTEXT));
        assert_eq!(hunks.len(), 1, "one change is one hunk: {hunks:?}");
        assert_eq!(hunks[0].file, "big.txt");
        assert!(hunks[0].start <= 10 && hunks[0].end >= 10, "{:?}", hunks[0]);
        assert!(hunks[0].patch.contains("+line ten"), "{:?}", hunks[0]);
    }

    #[test]
    fn two_changes_far_apart_are_two_hunks() {
        let before = (1..=40).map(|n| format!("line {n}\n")).collect::<String>();
        let after = before.replace("line 5\n", "line five\n").replace("line 35\n", "line thirty-five\n");
        let hunks = crate::git::split_hunks(&unified(&before, &after, "big.txt", CONTEXT));
        assert_eq!(hunks.len(), 2, "{hunks:?}");
    }

    #[test]
    fn asking_for_more_context_gathers_the_changes_into_one() {
        let before = (1..=40).map(|n| format!("line {n}\n")).collect::<String>();
        let after = before.replace("line 5\n", "line five\n").replace("line 35\n", "line thirty-five\n");
        let hunks = crate::git::split_hunks(&unified(&before, &after, "big.txt", 40));
        assert_eq!(hunks.len(), 1, "one hunk once the context reaches across: {hunks:?}");
    }

    #[test]
    fn a_text_with_no_name_still_says_what_it_is() {
        let d = unified("a\n", "b\n", "   ", CONTEXT);
        assert!(d.starts_with(&format!("diff --git a/{UNNAMED} b/{UNNAMED}\n")), "{d}");
    }

    /// A number nobody meant is taken as "as much as there is" rather than
    /// being handed to the diff to allocate against
    #[test]
    fn a_silly_amount_of_context_is_brought_back_to_earth() {
        let d = unified("a\n", "b\n", "x", usize::MAX);
        assert!(d.contains("-a"), "{d}");
        assert!(d.contains("+b"), "{d}");
    }
}
