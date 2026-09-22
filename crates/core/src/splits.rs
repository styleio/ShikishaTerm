//! The arrangements the split rows own, while the program runs.
//!
//! A split row shows several other rows at once, divided ([`view::Surface::Split`]).
//! The division is that row's own: it is written in the settings beside the
//! row, by the names of the tabs in it, and only one arrangement is ever on
//! screen -- the one belonging to whichever row is in front.
//!
//! Before this, the division belonged to the desk. There was one of them, every
//! folder on the desk looked at it, and nothing on screen said whose it was: a
//! split made while working in one folder was waiting in the next one, showing
//! tabs from a folder nobody had opened. Closing it in the second folder closed
//! it in the first. That is what a shared piece of state with no owner does,
//! and giving it an owner is the whole of the fix.
//!
//! What is here is the parking: the arrangement of a row that is not in front
//! has to be somewhere, and this is where. The tree itself is
//! [`crate::layout::Layout`], and what is written down is
//! [`crate::layout::Kept`] -- shape and names, never row numbers.

use std::collections::HashMap;

use crate::layout::{Kept, Layout};

/// Every split row's arrangement, by the name automation calls that row.
#[derive(Debug, Default)]
pub struct Splits {
    parked: HashMap<String, Layout>,
}

impl Splits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put a row's arrangement away while it is not in front
    pub fn park(&mut self, key: &str, layout: Layout) {
        self.parked.insert(key.to_string(), layout);
    }

    /// The arrangement for a row coming into front.
    ///
    /// The one parked when it was last looked at, and failing that the one
    /// written down in the settings, put back by name. A row that has neither
    /// -- one just made -- starts as a single empty pane, which is what the
    /// caller then divides.
    ///
    /// `after` is the arrangement going off screen: pane ids go on counting
    /// from wherever it had got to, so the page never sees an id it already
    /// knows used for a different pane.
    pub fn take(
        &mut self,
        key: &str,
        written: Option<&Kept>,
        after: &Layout,
        index_of: impl Fn(&str) -> Option<usize>,
    ) -> Layout {
        if let Some(mut held) = self.parked.remove(key) {
            held.renumber_from(after);
            return held;
        }
        match written {
            Some(kept) => Layout::restore(kept, after, index_of),
            None => {
                let mut fresh = Layout::single(0);
                fresh.renumber_from(after);
                fresh
            }
        }
    }

    /// What to write down for the row in front, so the next run finds it as it
    /// was left. `None` where nothing of it is worth keeping
    pub fn written(layout: &Layout, key_of: impl Fn(usize) -> Option<String>) -> Option<Kept> {
        (!layout.is_single()).then(|| layout.keep(key_of))
    }

    /// A row that is gone takes its arrangement with it
    pub fn forget(&mut self, key: &str) {
        self.parked.remove(key);
    }

    /// Rows that are no longer there at all: their arrangements go too, so a
    /// long run does not keep the shape of every split ever closed
    pub fn keep_only(&mut self, alive: &[String]) {
        self.parked.retain(|k, _| alive.iter().any(|a| a == k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Dir;

    fn two(a: usize, b: usize) -> Layout {
        let mut l = Layout::single(a);
        l.split(Dir::Row, b);
        l
    }

    /// A row put away comes back as it was, and only to the row it belongs to
    #[test]
    fn an_arrangement_comes_back_to_the_row_that_owns_it() {
        let mut s = Splits::new();
        s.park("compare", two(1, 2));
        let after = Layout::single(3);
        let back = s.take("compare", None, &after, |_| None);
        assert_eq!(back.leaves().len(), 2, "it came back undivided");

        // ...and a different row gets its own, not that one
        let other = s.take("elsewhere", None, &after, |_| None);
        assert!(other.is_single(), "another row was handed somebody else's arrangement");
        assert_eq!(other.focused_surface(), 0, "a row just made starts on nothing");
    }

    /// Taken twice, the second time is not the first time's arrangement all
    /// over again: it was parked on the way out or it was not
    #[test]
    fn what_was_parked_is_handed_over_once() {
        let mut s = Splits::new();
        s.park("compare", two(1, 2));
        let after = Layout::single(3);
        assert_eq!(s.take("compare", None, &after, |_| None).leaves().len(), 2);
        assert!(s.take("compare", None, &after, |_| None).is_single(), "it was handed out twice");
    }

    /// Nothing parked, but something written down: the settings decide, by name
    #[test]
    fn what_was_written_down_comes_back_by_name() {
        let names = ["", "ai", "web"];
        let kept = two(1, 2).keep(|s| names.get(s).map(|n| n.to_string()));
        let mut s = Splits::new();
        let back = s.take("compare", Some(&kept), &Layout::single(1), |k| {
            names.iter().position(|n| *n == k)
        });
        assert_eq!(back.leaves().into_iter().map(|(_, n)| n).collect::<Vec<_>>(), vec![1, 2]);
    }

    /// An undivided row has no arrangement worth writing down: it is what
    /// every row is
    #[test]
    fn only_a_real_division_is_written_down() {
        assert!(Splits::written(&Layout::single(1), |_| Some("ai".into())).is_none());
        assert!(Splits::written(&two(1, 2), |_| Some("ai".into())).is_some());
    }

    /// A row that is gone does not keep its shape waiting for ever
    #[test]
    fn the_shape_of_a_row_that_is_gone_goes_with_it() {
        let mut s = Splits::new();
        s.park("a", two(1, 2));
        s.park("b", two(1, 2));
        s.keep_only(&["a".to_string()]);
        assert!(s.take("b", None, &Layout::single(1), |_| None).is_single());
        assert_eq!(s.take("a", None, &Layout::single(1), |_| None).leaves().len(), 2);
    }
}
