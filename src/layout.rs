//! Split-pane layout — a binary tree of panes within one tmnl tab.
//!
//! Foundation for the splits feature (`docs/splits-plan.md`). This module
//! is the pure geometry: the [`Layout`] tree and the recursive
//! area-splitting in [`Layout::leaf_rects`]. Nothing here touches the
//! GPU, the shell, or input. Standalone + unit-tested so the tricky
//! rect-splitting math is pinned before anything depends on it.
//!
//! Phase 1 of the splits work wires `Layout` + [`PaneId`] into `Tab`
//! and `composite()` (always a single `Leaf`); the `Split` constructor,
//! [`Layout::pane_at`], [`Layout::leaf_ids`], and [`SplitDir`] stay
//! exercised only by the unit tests until the split / focus verbs land
//! in a later phase — hence the crate-level dead-code allowance.
#![allow(dead_code)]

/// Index of a pane within a tab's `panes` Vec.
pub type PaneId = usize;

/// Which way a [`Layout::Split`] divides its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// `first` left, `second` right — a vertical divider between them.
    Vertical,
    /// `first` top, `second` bottom — a horizontal divider.
    Horizontal,
}

/// A cell-coordinate rectangle (the unit the grid + renderer work in).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Rect { x, y, w, h }
    }
    /// True when cell `(cx, cy)` is inside the rectangle.
    pub fn contains(&self, cx: u32, cy: u32) -> bool {
        cx >= self.x && cx < self.x + self.w && cy >= self.y && cy < self.y + self.h
    }
}

/// A tab's pane layout — a binary split tree. Leaves carry a [`PaneId`].
#[derive(Debug, Clone)]
pub enum Layout {
    /// A single pane fills the area.
    Leaf(PaneId),
    /// Two child layouts, split `dir`-wise. `ratio` is `first`'s share
    /// of the splittable extent (0.0..=1.0, clamped on use).
    Split {
        dir: SplitDir,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

/// Divide `area` for a `Split` of direction `dir` + `ratio` into the
/// `first` child rect, the `second` child rect, and the 1-cell-wide
/// divider strip between them. The single source of the rect-splitting
/// math — both [`Layout::leaf_rects`] and [`Layout::divider_lines`]
/// route through here so they can never drift apart.
fn split_rects(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect, Rect) {
    let r = ratio.clamp(0.0, 1.0);
    match dir {
        SplitDir::Vertical => {
            // One column for the divider.
            let usable = area.w.saturating_sub(1);
            let fw = ((usable as f32 * r).round() as u32).min(usable);
            (
                Rect::new(area.x, area.y, fw, area.h),
                Rect::new(area.x + fw + 1, area.y, usable - fw, area.h),
                Rect::new(area.x + fw, area.y, 1, area.h),
            )
        }
        SplitDir::Horizontal => {
            let usable = area.h.saturating_sub(1);
            let fh = ((usable as f32 * r).round() as u32).min(usable);
            (
                Rect::new(area.x, area.y, area.w, fh),
                Rect::new(area.x, area.y + fh + 1, area.w, usable - fh),
                Rect::new(area.x, area.y + fh, area.w, 1),
            )
        }
    }
}

impl Layout {
    /// Assign each leaf a sub-rectangle of `area`. A split reserves one
    /// cell for the divider between its children; the remaining extent
    /// is shared by `ratio`. Order matches a left-to-right / top-to-
    /// bottom tree walk. An area too small to host a divider degrades
    /// gracefully — the second child just gets a zero-size rect.
    pub fn leaf_rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.collect(area, &mut out);
        out
    }

    fn collect(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Layout::Leaf(id) => out.push((*id, area)),
            Layout::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let (a, b, _div) = split_rects(area, *dir, *ratio);
                first.collect(a, out);
                second.collect(b, out);
            }
        }
    }

    /// The 1-cell divider strip of every `Split` in the tree — the gaps
    /// `leaf_rects` reserves between children. One `(Rect, SplitDir)`
    /// per split node; the renderer paints `│` / `─` glyphs into them.
    pub fn divider_lines(&self, area: Rect) -> Vec<(Rect, SplitDir)> {
        let mut out = Vec::new();
        self.collect_dividers(area, &mut out);
        out
    }

    fn collect_dividers(&self, area: Rect, out: &mut Vec<(Rect, SplitDir)>) {
        if let Layout::Split {
            dir,
            ratio,
            first,
            second,
        } = self
        {
            let (a, b, div) = split_rects(area, *dir, *ratio);
            out.push((div, *dir));
            first.collect_dividers(a, out);
            second.collect_dividers(b, out);
        }
    }

    /// Every `PaneId` in the tree, in tree order.
    pub fn leaf_ids(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_ids(&mut out);
        out
    }

    fn collect_ids(&self, out: &mut Vec<PaneId>) {
        match self {
            Layout::Leaf(id) => out.push(*id),
            Layout::Split { first, second, .. } => {
                first.collect_ids(out);
                second.collect_ids(out);
            }
        }
    }

    /// The pane whose rect contains cell `(cx, cy)`, if any — for routing
    /// a mouse event to the pane under the cursor.
    pub fn pane_at(&self, area: Rect, cx: u32, cy: u32) -> Option<PaneId> {
        self.leaf_rects(area)
            .into_iter()
            .find(|(_, r)| r.contains(cx, cy))
            .map(|(id, _)| id)
    }

    /// Replace the `Leaf(target)` node with a `Split` of `target` and
    /// `new_id` (`target` keeps the `first` slot). Returns `true` if
    /// `target` was found.
    pub fn split_leaf(&mut self, target: PaneId, dir: SplitDir, new_id: PaneId) -> bool {
        match self {
            Layout::Leaf(id) => {
                if *id == target {
                    *self = Layout::Split {
                        dir,
                        ratio: 0.5,
                        first: Box::new(Layout::Leaf(target)),
                        second: Box::new(Layout::Leaf(new_id)),
                    };
                    true
                } else {
                    false
                }
            }
            Layout::Split { first, second, .. } => {
                first.split_leaf(target, dir, new_id) || second.split_leaf(target, dir, new_id)
            }
        }
    }

    /// Remove `Leaf(target)`, collapsing its parent `Split` so the
    /// sibling subtree takes the parent's slot. Returns `true` if the
    /// target was removed. A `Leaf(target)` at the *root* can't be
    /// removed (a tab always keeps ≥ 1 pane) — returns `false`.
    pub fn remove_leaf(&mut self, target: PaneId) -> bool {
        match self {
            Layout::Leaf(_) => false,
            Layout::Split { first, second, .. } => {
                if matches!(**first, Layout::Leaf(id) if id == target) {
                    let kept = std::mem::replace(second.as_mut(), Layout::Leaf(0));
                    *self = kept;
                    true
                } else if matches!(**second, Layout::Leaf(id) if id == target) {
                    let kept = std::mem::replace(first.as_mut(), Layout::Leaf(0));
                    *self = kept;
                    true
                } else {
                    first.remove_leaf(target) || second.remove_leaf(target)
                }
            }
        }
    }

    /// The first leaf of `Leaf(target)`'s sibling subtree — the natural
    /// pane to focus once `target` is closed. `None` if `target` is the
    /// root leaf (no sibling).
    pub fn sibling_leaf(&self, target: PaneId) -> Option<PaneId> {
        match self {
            Layout::Leaf(_) => None,
            Layout::Split { first, second, .. } => {
                if matches!(**first, Layout::Leaf(id) if id == target) {
                    return second.leaf_ids().first().copied();
                }
                if matches!(**second, Layout::Leaf(id) if id == target) {
                    return first.leaf_ids().first().copied();
                }
                first
                    .sibling_leaf(target)
                    .or_else(|| second.sibling_leaf(target))
            }
        }
    }

    /// Decrement every leaf id strictly greater than `removed` — call
    /// after `panes.remove(removed)` so the tree still indexes the
    /// (now shorter) `panes` Vec correctly.
    pub fn shift_ids_after_removal(&mut self, removed: PaneId) {
        match self {
            Layout::Leaf(id) => {
                if *id > removed {
                    *id -= 1;
                }
            }
            Layout::Split { first, second, .. } => {
                first.shift_ids_after_removal(removed);
                second.shift_ids_after_removal(removed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_leaf_fills_the_area() {
        let l = Layout::Leaf(0);
        let area = Rect::new(0, 0, 80, 24);
        assert_eq!(l.leaf_rects(area), vec![(0, area)]);
    }

    #[test]
    fn vertical_split_halves_width_minus_a_divider() {
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        let rects = l.leaf_rects(Rect::new(0, 0, 81, 24));
        // 81 wide → 1 divider, 80 usable, 40/40.
        assert_eq!(rects[0], (0, Rect::new(0, 0, 40, 24)));
        assert_eq!(rects[1], (1, Rect::new(41, 0, 40, 24)));
    }

    #[test]
    fn horizontal_split_halves_height_minus_a_divider() {
        let l = Layout::Split {
            dir: SplitDir::Horizontal,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        let rects = l.leaf_rects(Rect::new(0, 0, 80, 25));
        // 25 tall → 1 divider, 24 usable, 12/12.
        assert_eq!(rects[0], (0, Rect::new(0, 0, 80, 12)));
        assert_eq!(rects[1], (1, Rect::new(0, 13, 80, 12)));
    }

    #[test]
    fn nested_splits_tile_without_overlap() {
        // Left pane, and a right column split top/bottom.
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(Layout::Leaf(1)),
                second: Box::new(Layout::Leaf(2)),
            }),
        };
        let rects = l.leaf_rects(Rect::new(0, 0, 81, 25));
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].0, 0);
        assert_eq!(rects[1].0, 1);
        assert_eq!(rects[2].0, 2);
        // No two leaf rects overlap.
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, b) = (rects[i].1, rects[j].1);
                let disjoint =
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
                assert!(disjoint, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn leaf_ids_walks_in_tree_order() {
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(7)),
            second: Box::new(Layout::Leaf(3)),
        };
        assert_eq!(l.leaf_ids(), vec![7, 3]);
    }

    #[test]
    fn pane_at_hit_tests_the_cursor_cell() {
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        let area = Rect::new(0, 0, 81, 24);
        assert_eq!(l.pane_at(area, 5, 5), Some(0));
        assert_eq!(l.pane_at(area, 60, 5), Some(1));
        // The divider column (40) belongs to neither pane.
        assert_eq!(l.pane_at(area, 40, 5), None);
        // Outside the whole area — nothing.
        assert_eq!(l.pane_at(area, 200, 200), None);
    }

    #[test]
    fn ratio_is_clamped_to_0_1() {
        let area = Rect::new(0, 0, 81, 24);
        // ratio > 1 → first takes all the usable extent, second is empty.
        let hi = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 9.0,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        let r = hi.leaf_rects(area);
        assert_eq!(r[0].1.w, 80);
        assert_eq!(r[1].1.w, 0);
        // ratio < 0 → first is empty.
        let lo = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: -3.0,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        let r = lo.leaf_rects(area);
        assert_eq!(r[0].1.w, 0);
        assert_eq!(r[1].1.w, 80);
    }

    #[test]
    fn tiny_area_too_small_for_a_divider_degrades_gracefully() {
        // 1-wide area: no room for the divider; both children collapse
        // rather than panicking (saturating_sub guards the arithmetic).
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        let rects = l.leaf_rects(Rect::new(0, 0, 1, 10));
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].1.w + rects[1].1.w, 0);
    }

    #[test]
    fn deep_nesting_assigns_every_leaf() {
        // A left-leaning chain of vertical splits — 4 leaves.
        let mut tree = Layout::Leaf(3);
        for id in (0..3).rev() {
            tree = Layout::Split {
                dir: SplitDir::Vertical,
                ratio: 0.5,
                first: Box::new(Layout::Leaf(id)),
                second: Box::new(tree),
            };
        }
        let rects = tree.leaf_rects(Rect::new(0, 0, 120, 40));
        let ids: Vec<_> = rects.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![0, 1, 2, 3]);
        assert_eq!(tree.leaf_ids(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn divider_lines_one_vertical_split() {
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        // 81 wide → 40/40 with the divider as the 1-wide column at 40.
        let d = l.divider_lines(Rect::new(0, 0, 81, 24));
        assert_eq!(d, vec![(Rect::new(40, 0, 1, 24), SplitDir::Vertical)]);
    }

    #[test]
    fn split_leaf_turns_a_leaf_into_a_split() {
        let mut l = Layout::Leaf(0);
        assert!(l.split_leaf(0, SplitDir::Vertical, 1));
        assert_eq!(l.leaf_ids(), vec![0, 1]);
        // Splitting a leaf that isn't in the tree is a no-op.
        assert!(!l.split_leaf(9, SplitDir::Vertical, 2));
        assert_eq!(l.leaf_ids(), vec![0, 1]);
    }

    #[test]
    fn remove_leaf_collapses_to_the_sibling() {
        // Split(0, Split(1, 2)) — removing 1 collapses the right side
        // to just leaf 2.
        let mut l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(Layout::Leaf(1)),
                second: Box::new(Layout::Leaf(2)),
            }),
        };
        assert!(l.remove_leaf(1));
        assert_eq!(l.leaf_ids(), vec![0, 2]);
        // The root leaf has no parent split to collapse — refused.
        let mut root = Layout::Leaf(5);
        assert!(!root.remove_leaf(5));
    }

    #[test]
    fn sibling_leaf_finds_the_other_child() {
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(1)),
        };
        assert_eq!(l.sibling_leaf(0), Some(1));
        assert_eq!(l.sibling_leaf(1), Some(0));
        assert_eq!(Layout::Leaf(0).sibling_leaf(0), None);
    }

    #[test]
    fn shift_ids_after_removal_renumbers_higher_leaves() {
        // Pane 1 was removed from the Vec — leaf 2 shifts down to 1,
        // leaf 0 is untouched.
        let mut l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Leaf(2)),
        };
        l.shift_ids_after_removal(1);
        assert_eq!(l.leaf_ids(), vec![0, 1]);
    }

    #[test]
    fn divider_lines_fill_every_gap_between_leaves() {
        // Left pane + a right column split top/bottom.
        let l = Layout::Split {
            dir: SplitDir::Vertical,
            ratio: 0.5,
            first: Box::new(Layout::Leaf(0)),
            second: Box::new(Layout::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(Layout::Leaf(1)),
                second: Box::new(Layout::Leaf(2)),
            }),
        };
        let area = Rect::new(0, 0, 81, 25);
        let leaves = l.leaf_rects(area);
        let dividers = l.divider_lines(area);
        // Two `Split` nodes ⇒ two divider strips.
        assert_eq!(dividers.len(), 2);
        // Every cell of `area` is covered exactly once — by either a
        // leaf rect or a divider strip. No gaps, no overlaps.
        for cy in 0..area.h {
            for cx in 0..area.w {
                let in_leaf = leaves.iter().filter(|(_, r)| r.contains(cx, cy)).count();
                let in_div = dividers.iter().filter(|(r, _)| r.contains(cx, cy)).count();
                assert_eq!(in_leaf + in_div, 1, "cell ({cx},{cy}) coverage");
            }
        }
    }
}
