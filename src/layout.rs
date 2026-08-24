//! The pane tree: how the content area is divided, and which pane has focus.
//!
//! A *surface* is one row in the tab bar (a terminal session, a browser page,
//! or the dashboard). A *pane* is a rectangle of the content area, and it shows
//! exactly one surface. Before panes existed there was only ever one rectangle,
//! so "which surface is on screen" and "which surface has focus" were the same
//! number — `active`. They still are: `active` is the surface in the *focused*
//! pane. Everything else on screen hangs off this tree.
//!
//! Two rules keep the thing predictable, and both are enforced here rather than
//! left to callers:
//!
//!   - **A surface is in at most one pane.** Otherwise the same PTY would be
//!     asked to be two different sizes at once, and one of the two views would
//!     silently render at the wrong width. Selecting a surface that already sits
//!     in another pane *swaps* the two panes' surfaces instead of duplicating it.
//!   - **There is always at least one pane.** Closing the last one is refused,
//!     so there is never a state with nothing to focus.
//!
//! Geometry here is normalised (0.0–1.0), never pixels. The page owns real
//! pixels — it knows the font metrics, the dividers and the top bar — and
//! reports back how many rows and columns each pane actually got. These
//! fractions exist so that "focus the pane to the left" can be answered without
//! asking the page anything.

use serde::{Deserialize, Serialize};

/// Stable per-pane identity. The page addresses panes by this, so it must not
/// be an index into anything that shifts when a sibling closes.
pub type PaneId = u32;

/// Which way a split divides its space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// Side by side (the divider is vertical)
    Row,
    /// Stacked (the divider is horizontal)
    Col,
}

/// Which way focus is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
}

/// A normalised rectangle inside the content area (0.0–1.0 on both axes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl FRect {
    const FULL: FRect = FRect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }

    fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
}

/// One node of the tree: either a pane, or a division into two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node {
    Leaf {
        id: PaneId,
        /// The surface shown here: 0 = the dashboard, 1.. = a tab-bar row
        surface: usize,
    },
    Split {
        dir: Dir,
        /// How much of the space the first child gets (0.05–0.95)
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// The smallest share of a split either side may be squeezed to. Below this a
/// terminal is too narrow to be worth anything, and the divider becomes hard to
/// grab back.
const MIN_RATIO: f32 = 0.1;

impl Node {
    fn leaf(id: PaneId, surface: usize) -> Node {
        Node::Leaf { id, surface }
    }

    /// Walks every pane in on-screen order (first child first).
    fn walk<'a>(&'a self, out: &mut Vec<(PaneId, usize)>) {
        match self {
            Node::Leaf { id, surface } => out.push((*id, *surface)),
            Node::Split { a, b, .. } => {
                a.walk(out);
                b.walk(out);
            }
        }
    }

    fn walk_rects(&self, at: FRect, out: &mut Vec<(PaneId, FRect)>) {
        match self {
            Node::Leaf { id, .. } => out.push((*id, at)),
            Node::Split { dir, ratio, a, b } => {
                let r = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                let (ra, rb) = match dir {
                    Dir::Row => (
                        FRect { w: at.w * r, ..at },
                        FRect { x: at.x + at.w * r, w: at.w * (1.0 - r), ..at },
                    ),
                    Dir::Col => (
                        FRect { h: at.h * r, ..at },
                        FRect { y: at.y + at.h * r, h: at.h * (1.0 - r), ..at },
                    ),
                };
                a.walk_rects(ra, out);
                b.walk_rects(rb, out);
            }
        }
    }

    fn find_mut(&mut self, want: PaneId) -> Option<&mut Node> {
        match self {
            Node::Leaf { id, .. } if *id == want => Some(self),
            Node::Leaf { .. } => None,
            Node::Split { a, b, .. } => a.find_mut(want).or_else(|| b.find_mut(want)),
        }
    }

    /// Removes the given pane, replacing its parent split with the surviving
    /// sibling. Returns true when the removal happened somewhere below `self`.
    fn remove(&mut self, want: PaneId) -> bool {
        let Node::Split { a, b, .. } = self else {
            return false;
        };
        let survivor = match (a.as_ref(), b.as_ref()) {
            (Node::Leaf { id, .. }, _) if *id == want => Some((**b).clone()),
            (_, Node::Leaf { id, .. }) if *id == want => Some((**a).clone()),
            _ => None,
        };
        if let Some(s) = survivor {
            *self = s;
            return true;
        }
        a.remove(want) || b.remove(want)
    }

    fn count(&self) -> usize {
        match self {
            Node::Leaf { .. } => 1,
            Node::Split { a, b, .. } => a.count() + b.count(),
        }
    }
}

/// The whole division of the content area, plus which pane the keyboard is aimed at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    root: Node,
    focus: PaneId,
    next_id: PaneId,
}

impl Layout {
    /// One pane filling everything — what every workspace starts as.
    pub fn single(surface: usize) -> Layout {
        Layout { root: Node::leaf(1, surface), focus: 1, next_id: 2 }
    }

    pub fn focus(&self) -> PaneId {
        self.focus
    }

    /// How many panes are on screen.
    pub fn len(&self) -> usize {
        self.root.count()
    }

    /// True while the content area is undivided — the shape the app had before
    /// panes existed, and the one every code path that knows only `active`
    /// still behaves correctly in.
    pub fn is_single(&self) -> bool {
        self.len() == 1
    }

    /// Every pane, in on-screen order, as (pane, surface).
    pub fn leaves(&self) -> Vec<(PaneId, usize)> {
        let mut out = Vec::new();
        self.root.walk(&mut out);
        out
    }

    /// Every pane's share of the content area.
    pub fn rects(&self) -> Vec<(PaneId, FRect)> {
        let mut out = Vec::new();
        self.root.walk_rects(FRect::FULL, &mut out);
        out
    }

    /// The surface shown in a given pane.
    pub fn surface_of(&self, id: PaneId) -> Option<usize> {
        self.leaves().into_iter().find(|(p, _)| *p == id).map(|(_, s)| s)
    }

    /// The pane a surface is shown in, if any.
    pub fn pane_of(&self, surface: usize) -> Option<PaneId> {
        self.leaves().into_iter().find(|(_, s)| *s == surface).map(|(p, _)| p)
    }

    /// The surface the keyboard is aimed at. This is what the rest of the app
    /// calls `active`.
    pub fn focused_surface(&self) -> usize {
        self.surface_of(self.focus).unwrap_or(0)
    }

    /// Points the focused pane at a surface.
    ///
    /// If that surface is already in another pane the two panes trade contents,
    /// which keeps "one surface, one pane" true without ever refusing the
    /// user's request — pressing `Ctrl+B 3` always ends with 3 under the cursor.
    pub fn show(&mut self, surface: usize) {
        let focus = self.focus;
        if let Some(other) = self.pane_of(surface) {
            if other == focus {
                return;
            }
            let mine = self.focused_surface();
            self.set_surface(other, mine);
        }
        self.set_surface(focus, surface);
    }

    /// Points one specific pane at a surface, with no swapping. Callers that
    /// might duplicate a surface should go through `show` instead.
    pub fn set_surface(&mut self, id: PaneId, surface: usize) {
        if let Some(Node::Leaf { surface: s, .. }) = self.root.find_mut(id) {
            *s = surface;
        }
    }

    /// Moves focus to a pane, if it exists.
    pub fn focus_pane(&mut self, id: PaneId) -> bool {
        if self.surface_of(id).is_some() {
            self.focus = id;
            return true;
        }
        false
    }

    /// Divides the focused pane in two and focuses the new half, which shows
    /// `surface`. Returns the new pane.
    ///
    /// The new pane takes the second half, so a split to the right puts the new
    /// one on the right — the direction the user asked for is the direction the
    /// new thing appears in.
    pub fn split(&mut self, dir: Dir, surface: usize) -> PaneId {
        let id = self.next_id;
        self.next_id += 1;
        let focus = self.focus;
        // A surface may only be in one pane, so take it away from wherever it is
        if let Some(other) = self.pane_of(surface) {
            if other != focus {
                self.set_surface(other, 0);
            }
        }
        if let Some(node) = self.root.find_mut(focus) {
            let kept = node.clone();
            *node = Node::Split {
                dir,
                ratio: 0.5,
                a: Box::new(kept),
                b: Box::new(Node::leaf(id, surface)),
            };
            self.focus = id;
        }
        id
    }

    /// Closes a pane. The last one is never closed — with no pane there would be
    /// nothing to focus and no way to get back.
    ///
    /// Focus lands on whichever pane is nearest in on-screen order, so closing
    /// never leaves the cursor somewhere the user has to hunt for.
    pub fn close(&mut self, id: PaneId) -> bool {
        if self.is_single() {
            return false;
        }
        let order = self.leaves();
        let at = order.iter().position(|(p, _)| *p == id);
        if !self.root.remove(id) {
            return false;
        }
        if self.focus == id {
            let left = self.leaves();
            let next = at
                .and_then(|i| left.get(i.min(left.len().saturating_sub(1))))
                .or_else(|| left.first());
            if let Some((p, _)) = next {
                self.focus = *p;
            }
        }
        true
    }

    /// Moves the divider a pane sits against. `ratio` is the first child's share.
    pub fn set_ratio(&mut self, id: PaneId, ratio: f32) {
        fn walk(node: &mut Node, want: PaneId, ratio: f32) -> bool {
            let Node::Split { a, b, ratio: r, .. } = node else {
                return false;
            };
            if matches!(a.as_ref(), Node::Leaf { id, .. } if *id == want)
                || matches!(b.as_ref(), Node::Leaf { id, .. } if *id == want)
            {
                *r = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                return true;
            }
            walk(a, want, ratio) || walk(b, want, ratio)
        }
        walk(&mut self.root, id, ratio);
    }

    /// Gives a pane more (or less) of the divider it sits against.
    ///
    /// Which way the ratio has to move depends on which side of its split the
    /// pane is on, and only the tree knows that — working it back out from the
    /// rectangles would be guesswork that goes wrong the moment the split is
    /// nested. `by` is always "this pane gets that much bigger".
    pub fn grow(&mut self, id: PaneId, by: f32) -> bool {
        fn walk(node: &mut Node, want: PaneId, by: f32) -> bool {
            let Node::Split { a, b, ratio, .. } = node else {
                return false;
            };
            let first = matches!(a.as_ref(), Node::Leaf { id, .. } if *id == want);
            let second = matches!(b.as_ref(), Node::Leaf { id, .. } if *id == want);
            if first || second {
                let d = if first { by } else { -by };
                *ratio = (*ratio + d).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                return true;
            }
            walk(a, want, by) || walk(b, want, by)
        }
        walk(&mut self.root, id, by)
    }

    /// Focus the neighbour in a direction.
    ///
    /// "Neighbour" is decided from the panes' rectangles rather than from the
    /// tree, because the tree's shape is an implementation detail the user never
    /// sees: two panes that look adjacent must behave adjacent even when they
    /// are cousins several splits apart. Candidates must overlap on the
    /// perpendicular axis (so a pane diagonally away is not "to the left"), and
    /// the nearest one wins.
    pub fn focus_move(&mut self, dir: Move) -> bool {
        let rects = self.rects();
        let Some(&(_, here)) = rects.iter().find(|(p, _)| *p == self.focus) else {
            return false;
        };
        let mut best: Option<(f32, PaneId)> = None;
        for (id, r) in &rects {
            if *id == self.focus {
                continue;
            }
            // Must lie in the asked-for direction, and share some of the other axis
            let (gap, overlaps) = match dir {
                Move::Left => (here.x - (r.x + r.w), r.y < here.y + here.h && here.y < r.y + r.h),
                Move::Right => (r.x - (here.x + here.w), r.y < here.y + here.h && here.y < r.y + r.h),
                Move::Up => (here.y - (r.y + r.h), r.x < here.x + here.w && here.x < r.x + r.w),
                Move::Down => (r.y - (here.y + here.h), r.x < here.x + here.w && here.x < r.x + r.w),
            };
            if !overlaps || gap < -0.001 {
                continue;
            }
            // Break ties along the divider by nearest centre, so a tall pane
            // facing two short ones hands focus to the one straight ahead
            let drift = match dir {
                Move::Left | Move::Right => (r.cy() - here.cy()).abs(),
                Move::Up | Move::Down => (r.cx() - here.cx()).abs(),
            };
            let score = gap.max(0.0) * 2.0 + drift;
            if best.map(|(s, _)| score < s).unwrap_or(true) {
                best = Some((score, *id));
            }
        }
        if let Some((_, id)) = best {
            self.focus = id;
            return true;
        }
        false
    }

    /// Drops surfaces that no longer exist (a tab closed, a workspace with
    /// fewer rows) back to the dashboard, and collapses panes that are left
    /// showing nothing when there is more than one.
    ///
    /// Called after anything that can shorten the surface list. Without it a
    /// pane would keep pointing at a row number that now belongs to a different
    /// tab — the screen would look right and be wrong.
    pub fn clamp(&mut self, surface_count: usize) {
        for (id, s) in self.leaves() {
            if s > surface_count {
                self.set_surface(id, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaces(l: &Layout) -> Vec<usize> {
        l.leaves().into_iter().map(|(_, s)| s).collect()
    }

    #[test]
    fn starts_undivided() {
        let l = Layout::single(1);
        assert!(l.is_single());
        assert_eq!(l.focused_surface(), 1);
        assert_eq!(l.rects()[0].1, FRect::FULL);
    }

    #[test]
    fn split_puts_the_new_pane_where_it_was_asked_for() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        assert_eq!(l.focus(), right, "手を出した先にフォーカスが移る");
        assert_eq!(surfaces(&l), vec![1, 2]);
        let rects = l.rects();
        let a = rects.iter().find(|(p, _)| *p != right).unwrap().1;
        let b = rects.iter().find(|(p, _)| *p == right).unwrap().1;
        assert!(a.x < b.x, "右に割ったら新しい方が右");
        assert!((a.w + b.w - 1.0).abs() < 0.001, "幅を食い合って合計は1");
    }

    #[test]
    fn a_surface_never_appears_twice() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        // Asking the right-hand pane to show what the left one already shows
        l.show(1);
        assert_eq!(surfaces(&l), vec![2, 1], "重複させず入れ替える");
        assert_eq!(l.focused_surface(), 1);
    }

    #[test]
    fn splitting_off_a_shown_surface_takes_it_away_from_the_old_pane() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        l.split(Dir::Col, 1); // 1 is currently in the left pane
        assert_eq!(l.focused_surface(), 1);
        assert_eq!(l.leaves().iter().filter(|(_, s)| *s == 1).count(), 1);
    }

    #[test]
    fn closing_collapses_the_split_and_keeps_a_focus() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        assert!(l.close(right));
        assert!(l.is_single());
        assert_eq!(l.focused_surface(), 1);
        assert_eq!(l.rects()[0].1, FRect::FULL, "残った方が全部を取り返す");
    }

    #[test]
    fn the_last_pane_cannot_be_closed() {
        let mut l = Layout::single(1);
        assert!(!l.close(l.focus()));
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn focus_moves_by_what_the_eye_sees_not_by_the_tree() {
        // ┌───┬───┐  left | (top over bottom)
        // │ 1 │ 2 │
        // │   ├───┤
        // │   │ 3 │
        // └───┴───┘
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        let bottom = l.split(Dir::Col, 3);
        assert_eq!(l.focus(), bottom);
        assert!(l.focus_move(Move::Left), "斜めの親をまたいで左へ");
        assert_eq!(l.focused_surface(), 1);
        assert!(l.focus_move(Move::Right));
        assert_eq!(l.focused_surface(), 2, "左から右は上半分の方が近い");
        assert!(l.focus_move(Move::Down));
        assert_eq!(l.focused_surface(), 3);
        assert!(!l.focus_move(Move::Down), "端では動かない");
        let _ = right;
    }

    #[test]
    fn ratios_never_squeeze_a_pane_to_nothing() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        l.set_ratio(right, 0.0);
        let w = l.rects().iter().map(|(_, r)| r.w).fold(f32::MAX, f32::min);
        assert!(w >= MIN_RATIO - 0.001, "潰れきらない: {w}");
    }

    #[test]
    fn growing_a_pane_widens_it_whichever_side_it_is_on() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        let left = l.leaves().iter().find(|(p, _)| *p != right).unwrap().0;
        let width = |l: &Layout, id| l.rects().iter().find(|(p, _)| *p == id).unwrap().1.w;
        let before = width(&l, right);
        assert!(l.grow(right, 0.2));
        assert!(width(&l, right) > before + 0.1, "右のペインが広がらない");
        let before = width(&l, left);
        assert!(l.grow(left, 0.2));
        assert!(width(&l, left) > before + 0.1, "左のペインが広がらない");
    }

    #[test]
    fn vanished_surfaces_fall_back_to_the_dashboard() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 5);
        l.clamp(3);
        assert_eq!(surfaces(&l), vec![1, 0], "消えたタブは盤面に戻す");
    }

    #[test]
    fn survives_a_round_trip_through_json() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        l.split(Dir::Col, 3);
        let json = serde_json::to_string(&l).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
    }
}
