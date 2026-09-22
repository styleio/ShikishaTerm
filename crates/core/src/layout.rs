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
    fn walk(&self, out: &mut Vec<(PaneId, usize)>) {
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
                let (ra, rb) = Node::halves(at, *dir, *ratio);
                a.walk_rects(ra, out);
                b.walk_rects(rb, out);
            }
        }
    }

    fn halves(at: FRect, dir: Dir, ratio: f32) -> (FRect, FRect) {
        let r = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        match dir {
            Dir::Row => (
                FRect { w: at.w * r, ..at },
                FRect { x: at.x + at.w * r, w: at.w * (1.0 - r), ..at },
            ),
            Dir::Col => (
                FRect { h: at.h * r, ..at },
                FRect { y: at.y + at.h * r, h: at.h * (1.0 - r), ..at },
            ),
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

/// Visits every split, first child first, handing over the area it divides.
/// Stops early once `f` says it is finished.
///
/// Paired with `walk_splits_mut` below, and the pairing is the point: the page
/// names a divider by its position in this walk, so the two must agree on the
/// order forever. They are kept next to each other so a change to one is a
/// change in front of the other.
fn walk_splits(node: &Node, at: FRect, f: &mut impl FnMut(FRect, Dir, f32) -> bool) -> bool {
    let Node::Split { dir, ratio, a, b } = node else {
        return false;
    };
    if f(at, *dir, *ratio) {
        return true;
    }
    let (ra, rb) = Node::halves(at, *dir, *ratio);
    walk_splits(a, ra, f) || walk_splits(b, rb, f)
}

/// The same walk, for changing a ratio. No rectangles: whoever is changing one
/// already knows where it is
fn walk_splits_mut(node: &mut Node, f: &mut impl FnMut(&mut f32) -> bool) -> bool {
    let Node::Split { ratio, a, b, .. } = node else {
        return false;
    };
    if f(ratio) {
        return true;
    }
    walk_splits_mut(a, f) || walk_splits_mut(b, f)
}

/// The whole division of the content area, plus which pane the keyboard is aimed at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    root: Node,
    focus: PaneId,
    next_id: PaneId,
}

impl Default for Layout {
    /// One pane, showing the first tab: what a runtime with no window has, and
    /// what a window starts from before anything is split
    fn default() -> Self {
        Self::single(1)
    }
}

impl Layout {
    /// One pane filling everything — what every desk starts as.
    pub fn single(surface: usize) -> Layout {
        Layout { root: Node::leaf(1, surface), focus: 1, next_id: 2 }
    }

    /// One surface on its own, undivided, in place of this arrangement.
    ///
    /// Pane ids go on counting from this one, as `restore` does, so the page
    /// never sees an old id come back meaning a different pane
    pub fn alone(&self, surface: usize) -> Layout {
        let id = self.next_id;
        Layout { root: Node::leaf(id, surface), focus: id, next_id: id + 1 }
    }

    pub fn focus(&self) -> PaneId {
        self.focus
    }

    /// How many panes are on screen.
    pub fn len(&self) -> usize {
        self.root.count()
    }

    /// Never true: a layout is at least one pane, and there is no state of
    /// this app with none. Here because anything that says how many it has
    /// should be able to answer the shorter question
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    /// Every divider: the area it divides, which way, and where it currently
    /// sits in that area.
    ///
    /// Handed to the page so a drag can address a divider **by position in this
    /// list** rather than by the panes either side of it. Two panes can look
    /// adjacent while belonging to splits several levels apart, and a divider
    /// whose children are themselves splits has no pane touching it at all —
    /// working the answer back out from rectangles would move the wrong
    /// divider, on a screen where every divider looks the same. The tree knows;
    /// this says so. `set_divider` walks in exactly this order.
    pub fn dividers(&self) -> Vec<(FRect, Dir, f32)> {
        let mut out = Vec::new();
        walk_splits(&self.root, FRect::FULL, &mut |at, dir, ratio| {
            out.push((at, dir, ratio));
            false
        });
        out
    }

    /// Moves one divider, named by its position in `dividers()`.
    ///
    /// `ratio` is the first half's share — the left of a side-by-side split,
    /// the top of a stacked one
    pub fn set_divider(&mut self, index: usize, ratio: f32) -> bool {
        let mut seen = 0usize;
        let mut done = false;
        walk_splits_mut(&mut self.root, &mut |r| {
            if seen == index {
                *r = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                done = true;
            }
            seen += 1;
            done
        });
        done
    }

    /// Puts every divider back to even halves.
    ///
    /// After a few drags a layout drifts, and putting it right by hand means
    /// finding each divider in turn. This is the one gesture that undoes all
    /// of that, so it is worth having even though nothing else calls it.
    pub fn equalize(&mut self) {
        walk_splits_mut(&mut self.root, &mut |r| {
            *r = 0.5;
            false
        });
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

    /// Shows a surface, and aims at it.
    ///
    /// Already on screen in another pane? Then it is already shown, and the
    /// only thing missing is the aim: the focus moves there and not one pane
    /// changes what it holds. Nowhere on screen? Then the focused pane takes
    /// it. Either way it ends up under the cursor, and never in two panes at
    /// once.
    ///
    /// The two panes used to trade contents instead. Nothing was ever hidden by
    /// that, but the arrangement was the person's — they put the AI on the left
    /// and the browser on the right — and an automation walking back and forth
    /// between the two turned it into a flicker, the halves changing sides on
    /// every turn. A view that jumps is harder to read than one that sits still,
    /// and moving the eye is cheaper than moving the room.
    pub fn show(&mut self, surface: usize) {
        if let Some(other) = self.pane_of(surface) {
            self.focus = other;
            return;
        }
        self.set_surface(self.focus, surface);
    }

    /// Points one specific pane at a surface, with no swapping. Callers that
    /// might duplicate a surface should go through `show` instead.
    pub fn set_surface(&mut self, id: PaneId, surface: usize) {
        if let Some(Node::Leaf { surface: s, .. }) = self.root.find_mut(id) {
            *s = surface;
        }
    }

    /// Puts a surface in a pane and takes it out of wherever else it was.
    ///
    /// The rule this keeps is at the top of the file: a surface is in at most
    /// one pane, because a terminal has one size and two panes would ask it
    /// for two. `set_surface` does not keep it -- it is for callers that know
    /// the surface is nowhere else -- and a caller that knows wrongly gets the
    /// one failure nobody looks for: the same tab in two panes, one of them
    /// silently the wrong width.
    ///
    /// The pane it came from is left empty rather than closed, so the shape
    /// somebody arranged stays as they arranged it
    pub fn put(&mut self, id: PaneId, surface: usize) {
        if surface != 0
            && let Some(was) = self.pane_of(surface)
            && was != id
        {
            self.set_surface(was, 0);
        }
        self.set_surface(id, surface);
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
    /// Divides the focused pane, putting `surface` in the new half.
    ///
    /// The keyboard follows, except into an empty half. A pane with nothing in
    /// it is a pane nothing can be typed into, and everything that asks "what
    /// am I looking at" -- the folder's tab strip, the composer, the buttons
    /// over a page -- answers from the focused pane's row. Focused on an empty
    /// one they all answer "nothing", and the screen empties out around a
    /// division that was meant to add to it. The half is filled from its own
    /// +, which does not need the keyboard to be standing in it.
    pub fn split(&mut self, dir: Dir, surface: usize) -> PaneId {
        let id = self.next_id;
        self.next_id += 1;
        let focus = self.focus;
        // A surface may only be in one pane, so take it away from wherever it is
        if let Some(other) = self.pane_of(surface)
            && other != focus {
                self.set_surface(other, 0);
            }
        if let Some(node) = self.root.find_mut(focus) {
            let kept = node.clone();
            *node = Node::Split {
                dir,
                ratio: 0.5,
                a: Box::new(kept),
                b: Box::new(Node::leaf(id, surface)),
            };
            if surface != 0 {
                self.focus = id;
            }
        }
        id
    }

    /// Go on counting pane ids from where another arrangement had got to.
    ///
    /// The page keeps what it drew by pane id, so an id it has seen must never
    /// come back meaning a different pane. Two arrangements that were never on
    /// screen together count from 1 apiece, and the moment one replaces the
    /// other the page is handed two panes it thinks it knows
    pub fn renumber_from(&mut self, after: &Layout) {
        self.next_id = self.next_id.max(after.next_id);
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

    /// Drops surfaces that no longer exist (a tab closed, a desk with
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

    /// Keeps every pane on the thing it was showing, after the list of things
    /// changed underneath it.
    ///
    /// A pane holds a row number, and a row number belongs to whatever is in
    /// that row: close the second tab and the third one is now the second, so a
    /// pane that was showing the third would be showing the fourth without
    /// anything on screen saying so. `moves[n - 1]` is where row `n` went --
    /// its new number, or nothing when it is gone.
    ///
    /// A pane whose thing is gone closes, as `restore` closes one: the split
    /// was there for that tab. The last pane cannot close, so it moves on to
    /// the neighbour instead -- the next one along, or the one before when it
    /// was the last -- which is the tab a browser shows when the one in front
    /// of you is closed.
    pub fn follow(&mut self, moves: &[Option<usize>]) {
        let mut gone = Vec::new();
        for (id, s) in self.leaves() {
            let Some(to) = s.checked_sub(1).and_then(|i| moves.get(i)) else {
                continue;
            };
            match to {
                Some(n) => self.set_surface(id, *n),
                None => gone.push((id, s)),
            }
        }
        for (id, s) in gone {
            if !self.close(id) {
                let after = moves[s..].iter().flatten().next();
                let before = moves[..s - 1].iter().rev().flatten().next();
                self.set_surface(id, after.or(before).copied().unwrap_or(0));
            }
        }
    }

    /// Writes this arrangement down by what each pane shows, not by where that
    /// thing stands in the tab list.
    ///
    /// A working folder's view is put back later -- after tabs have been
    /// opened and closed elsewhere, which moves every row number below them.
    /// Kept by position it would come back pointing at somebody else's tabs;
    /// kept by name, a pane whose tab has gone simply isn't there any more.
    /// `key_of` names a surface (1..), or says it has no name worth keeping.
    pub fn keep(&self, key_of: impl Fn(usize) -> Option<String>) -> Kept {
        let keys: Vec<(PaneId, Option<String>)> = self
            .leaves()
            .into_iter()
            .map(|(id, s)| (id, if s == 0 { None } else { key_of(s) }))
            .collect();
        // The shape, and nothing about what is in it: the row numbers a pane
        // holds mean whatever is in that row today, and this is written into a
        // settings file that outlives today. `restore` fills every pane from
        // the names, so a number kept here would be a number nobody reads --
        // and one a person opening the file would read as meaning something
        let mut shape = self.clone();
        for (id, _) in &keys {
            shape.set_surface(*id, 0);
        }
        Kept { layout: shape, keys }
    }

    /// Puts a kept arrangement back, as it stands now.
    ///
    /// `index_of` finds a name in today's tab list. A pane whose tab is gone is
    /// left empty, and the arrangement keeps its shape whatever is missing --
    /// even when everything is. `after` is the arrangement being replaced: pane
    /// ids go on counting from wherever it had got to, so the page never sees
    /// an old id reused for a different pane.
    ///
    /// The shape is held on to because somebody made it. An arrangement that
    /// quietly reflows when a tab is closed is one nobody can learn the
    /// position of: what was bottom-right is suddenly the whole screen, and
    /// the next thing put there lands somewhere else again. An empty pane says
    /// what happened and offers its own + to fill it.
    pub fn restore(kept: &Kept, after: &Layout, index_of: impl Fn(&str) -> Option<usize>) -> Layout {
        let mut out = kept.layout.clone();
        out.next_id = out.next_id.max(after.next_id);
        for (id, key) in &kept.keys {
            out.set_surface(*id, key.as_deref().and_then(&index_of).unwrap_or(0));
        }
        out
    }
}

/// An arrangement of panes written down by name (see `Layout::keep`).
///
/// This is the form that is kept in a settings file, so it says only what a
/// person arranged: the shape, and which tab is in each pane by the name
/// automation calls it. Row numbers are not in it -- they mean whatever is in
/// that row this minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    layout: Layout,
    keys: Vec<(PaneId, Option<String>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaces(l: &Layout) -> Vec<usize> {
        l.leaves().into_iter().map(|(_, s)| s).collect()
    }

    /// A tab put into a pane leaves the pane it was in, because a terminal
    /// has one size and two panes would ask it for two. The pane it came from
    /// stays where it is, empty: the shape is somebody's arrangement
    #[test]
    fn a_tab_put_in_a_pane_is_in_no_other() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        assert_eq!(surfaces(&l), vec![1, 2]);
        // The one on the left, asked for on the right as well
        l.put(right, 1);
        assert_eq!(surfaces(&l), vec![0, 1], "it is in two panes at once");
        assert_eq!(l.pane_of(1), Some(right));

        // An empty pane filled from nowhere empties nothing
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 0);
        l.put(right, 2);
        assert_eq!(surfaces(&l), vec![1, 2]);

        // ...and putting a tab back where it already is changes nothing
        l.put(right, 2);
        assert_eq!(surfaces(&l), vec![1, 2]);
    }

    /// Dividing puts the keyboard where the thing is. Into an empty half it
    /// stays where it was: an empty pane is one nothing can be typed into, and
    /// everything that asks what is in front answers from the focused pane
    #[test]
    fn the_keyboard_does_not_follow_a_division_into_nothing() {
        let mut l = Layout::single(1);
        let empty = l.split(Dir::Row, 0);
        assert_ne!(l.focus(), empty, "the keyboard went to stand in an empty pane");
        assert_eq!(l.focused_surface(), 1, "what is in front became nothing");

        // ...and with something in it, it follows as it always did
        let mut l = Layout::single(1);
        let filled = l.split(Dir::Row, 2);
        assert_eq!(l.focus(), filled);
    }

    /// A folder opened on its own replaces the whole split, whichever side was
    /// in front, and its pane is one the page has not seen before
    #[test]
    fn a_surface_alone_replaces_the_whole_split() {
        let mut l = Layout::single(2);
        let right = l.split(Dir::Row, 5);
        l.focus_pane(right);
        let before: Vec<PaneId> = l.leaves().into_iter().map(|(id, _)| id).collect();

        let alone = l.alone(7);
        assert_eq!(surfaces(&alone), [7], "the other side of the split stayed");
        assert_eq!(alone.focused_surface(), 7);
        let id = alone.leaves()[0].0;
        assert!(!before.contains(&id), "a pane id came back meaning another pane");
    }

    #[test]
    fn a_kept_view_comes_back_to_the_same_tabs_after_the_list_moves() {
        // Tab "ai" on the left at row 2, "web" on the right at row 3, focus right
        let mut l = Layout::single(2);
        l.split(Dir::Row, 3);
        let names = ["", "x", "ai", "web"];
        let kept = l.keep(|s| names.get(s).map(|n| n.to_string()));
        // Meanwhile a tab was added above both: they are rows 3 and 4 now
        let now = ["", "x", "new", "ai", "web"];
        let other = Layout::single(1);
        let back = Layout::restore(&kept, &other, |k| now.iter().position(|n| *n == k));
        assert_eq!(surfaces(&back), vec![3, 4], "it comes back by name, not by number");
        assert_eq!(back.focused_surface(), 4, "it goes back to the pane last looked at");
    }

    /// An arrangement somebody made keeps its shape. A pane whose tab is gone
    /// goes empty and stays where it is, because the position is what the
    /// person learned: a split that reflows when a tab closes puts the next
    /// thing somewhere else again, and there is nothing on screen saying why
    #[test]
    fn a_pane_whose_tab_is_gone_goes_empty_and_the_shape_stays() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        let kept = l.keep(|s| Some(["", "ai", "web"][s].to_string()));
        let other = Layout::single(1);
        let back = Layout::restore(&kept, &other, |k| (k == "ai").then_some(1));
        assert!(!back.is_single(), "the arrangement lost a pane and moved everything else");
        assert_eq!(surfaces(&back), vec![1, 0], "the pane of a tab that is gone is not empty");

        // ...and with nothing left at all it is still that arrangement, two
        // empty panes waiting to be filled from their own +
        let nothing = Layout::restore(&kept, &other, |_| None);
        assert_eq!(surfaces(&nothing), vec![0, 0]);
    }

    /// What is kept is the shape and the names, never the row numbers: a row
    /// number means whatever is in that row this minute, and this is written
    /// into a settings file that outlives the minute
    #[test]
    fn what_is_kept_holds_no_row_numbers() {
        let mut l = Layout::single(2);
        l.split(Dir::Row, 3);
        let kept = l.keep(|s| Some(["", "x", "ai", "web"][s].to_string()));
        let written = serde_json::to_string(&kept).expect("it cannot be written down");
        assert!(written.contains("\"ai\"") && written.contains("\"web\""), "the names are not in it: {written}");
        assert_eq!(
            surfaces(&kept.layout),
            vec![0, 0],
            "row numbers were written into the settings: {written}"
        );
        let back: Kept = serde_json::from_str(&written).expect("it cannot be read back");
        assert_eq!(back, kept);
    }

    #[test]
    fn a_restored_view_never_hands_out_an_id_already_used() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        let kept = l.keep(|s| Some(s.to_string()));
        let mut busy = Layout::single(1);
        for s in 2..6 {
            busy.split(Dir::Row, s);
        }
        let mut back = Layout::restore(&kept, &busy, |k| k.parse().ok());
        let fresh = back.split(Dir::Col, 3);
        assert!(fresh > 5, "a new pane id collides with a number already used: {fresh}");
    }

    #[test]
    fn starts_undivided() {
        let l = Layout::single(1);
        assert!(l.is_single());
        assert_eq!(l.focused_surface(), 1);
        assert_eq!(l.rects()[0].1, FRect::FULL);
    }

    #[test]
    fn a_divider_can_be_moved_even_when_no_pane_touches_it() {
        // left | (top / bottom): the outer divider has a split on one side, so
        // no pane is its direct child. The mouse can still be on it, and
        // dragging it must move THAT divider — addressing it by the panes
        // either side would move the inner one instead, halfway across the
        // screen from where the hand is
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2); // focus is now the right half
        l.split(Dir::Col, 3); // ...which becomes a stack
        let dividers = l.dividers();
        assert_eq!(dividers.len(), 2, "two dividers");
        assert_eq!(dividers[0].1, Dir::Row, "the outer one first (the vertical divider)");
        assert_eq!(dividers[1].1, Dir::Col);

        assert!(l.set_divider(0, 0.25));
        let left = l.rects()[0].1;
        assert!((left.w - 0.25).abs() < 0.001, "the left became 1/4: {left:?}");
        // ...and the inner divider now lives in the area that is left over
        let inner = l.dividers()[1].0;
        assert!((inner.x - 0.25).abs() < 0.001, "the inner one divides what is left: {inner:?}");

        assert!(!l.set_divider(9, 0.5), "a divider that does not exist cannot be moved");
    }

    #[test]
    fn equalizing_puts_every_divider_back() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        l.split(Dir::Col, 3);
        l.set_divider(0, 0.8);
        l.set_divider(1, 0.15);
        l.equalize();
        for (r, _, ratio) in l.dividers() {
            assert!((ratio - 0.5).abs() < 0.001, "back to half and half: {r:?} {ratio}");
        }
    }

    #[test]
    fn a_divider_never_squeezes_a_pane_to_nothing() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        l.set_divider(0, 0.0);
        let w = l.rects()[0].1.w;
        assert!(w >= MIN_RATIO, "grabbing it and pulling to the edge does not make it disappear: {w}");
    }

    #[test]
    fn split_puts_the_new_pane_where_it_was_asked_for() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        assert_eq!(l.focus(), right, "focus moves to where it was reached for");
        assert_eq!(surfaces(&l), vec![1, 2]);
        let rects = l.rects();
        let a = rects.iter().find(|(p, _)| *p != right).unwrap().1;
        let b = rects.iter().find(|(p, _)| *p == right).unwrap().1;
        assert!(a.x < b.x, "split to the right, the new one is on the right");
        assert!((a.w + b.w - 1.0).abs() < 0.001, "they share the width, adding up to 1");
    }

    /// What is already on screen is not moved to another pane — the aim goes to
    /// it where it stands.
    ///
    /// An automation that hands work between two tabs says `show` on every
    /// turn. Trading the panes' contents there made the two halves swap sides
    /// again and again while the work ran: nothing was hidden, but the division
    /// the person arranged flickered under them. The one thing `show` must
    /// guarantee is that the surface ends up on screen and under the cursor.
    #[test]
    fn showing_what_is_already_on_screen_only_moves_the_aim() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2); // left: 1 (the AI), right: 2 (the browser), focus right
        assert_eq!(l.focused_surface(), 2);

        l.show(1);
        assert_eq!(surfaces(&l), vec![1, 2], "what is already visible is not swapped");
        assert_eq!(l.focused_surface(), 1, "the target moves there");

        // ...and back, as many turns as the automation takes
        l.show(2);
        assert_eq!(surfaces(&l), vec![1, 2], "going back and forth does not change positions");
        assert_eq!(l.focused_surface(), 2);
    }

    /// A surface nowhere on screen lands in the pane being looked at, and lands
    /// there only once.
    #[test]
    fn a_surface_never_appears_twice() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        l.show(3);
        assert_eq!(surfaces(&l), vec![1, 3], "what is not visible goes into the current pane");
        assert_eq!(l.focused_surface(), 3);
        assert_eq!(l.leaves().iter().filter(|(_, s)| *s == 3).count(), 1);
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
        assert_eq!(l.rects()[0].1, FRect::FULL, "the one left takes back the whole area");
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
        assert!(l.focus_move(Move::Left), "left, across a diagonal parent");
        assert_eq!(l.focused_surface(), 1);
        assert!(l.focus_move(Move::Right));
        assert_eq!(l.focused_surface(), 2, "from left, going right, the upper half is nearer");
        assert!(l.focus_move(Move::Down));
        assert_eq!(l.focused_surface(), 3);
        assert!(!l.focus_move(Move::Down), "it does not move at the edge");
        let _ = right;
    }

    #[test]
    fn ratios_never_squeeze_a_pane_to_nothing() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        l.set_ratio(right, 0.0);
        let w = l.rects().iter().map(|(_, r)| r.w).fold(f32::MAX, f32::min);
        assert!(w >= MIN_RATIO - 0.001, "it does not collapse completely: {w}");
    }

    #[test]
    fn growing_a_pane_widens_it_whichever_side_it_is_on() {
        let mut l = Layout::single(1);
        let right = l.split(Dir::Row, 2);
        let left = l.leaves().iter().find(|(p, _)| *p != right).unwrap().0;
        let width = |l: &Layout, id| l.rects().iter().find(|(p, _)| *p == id).unwrap().1.w;
        let before = width(&l, right);
        assert!(l.grow(right, 0.2));
        assert!(width(&l, right) > before + 0.1, "the right pane does not grow");
        let before = width(&l, left);
        assert!(l.grow(left, 0.2));
        assert!(width(&l, left) > before + 0.1, "the left pane does not grow");
    }

    #[test]
    fn vanished_surfaces_fall_back_to_the_dashboard() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 5);
        l.clamp(3);
        assert_eq!(surfaces(&l), vec![1, 0], "a tab that is gone goes back to the board");
    }

    /// Closing the second of three tabs moves the third into second place. A
    /// pane that was showing it has to go on showing it, under its new number
    #[test]
    fn a_pane_stays_on_its_tab_when_one_before_it_closes() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 3);
        l.follow(&[Some(1), None, Some(2)]);
        assert_eq!(surfaces(&l), vec![1, 2], "the pane was handed the tab next door");
    }

    /// The pane showing the closed tab goes with it, and the other one takes
    /// the room
    #[test]
    fn a_pane_whose_tab_closed_closes_too() {
        let mut l = Layout::single(1);
        l.split(Dir::Row, 2);
        l.follow(&[Some(1), None, Some(2)]);
        assert!(l.is_single(), "the pane of a closed tab is still there");
        assert_eq!(surfaces(&l), vec![1]);
    }

    /// With nothing divided there is no pane to close, so the screen moves on
    /// to the next tab along -- or the one before, when it was the last
    #[test]
    fn the_only_pane_moves_to_the_neighbour() {
        let mut l = Layout::single(2);
        l.follow(&[Some(1), None, Some(2)]);
        assert_eq!(surfaces(&l), vec![2], "it did not move on to the next tab");

        let mut l = Layout::single(3);
        l.follow(&[Some(1), Some(2), None]);
        assert_eq!(surfaces(&l), vec![2], "closing the last tab should show the one before");

        let mut l = Layout::single(1);
        l.follow(&[None]);
        assert_eq!(surfaces(&l), vec![0], "with nothing left it should show nothing");
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
