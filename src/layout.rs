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
                let r = ratio.clamp(0.0, 1.0);
                let (a, b) = match dir {
                    SplitDir::Vertical => {
                        // One column for the divider.
                        let usable = area.w.saturating_sub(1);
                        let fw = (usable as f32 * r).round() as u32;
                        let fw = fw.min(usable);
                        (
                            Rect::new(area.x, area.y, fw, area.h),
                            Rect::new(area.x + fw + 1, area.y, usable - fw, area.h),
                        )
                    }
                    SplitDir::Horizontal => {
                        let usable = area.h.saturating_sub(1);
                        let fh = (usable as f32 * r).round() as u32;
                        let fh = fh.min(usable);
                        (
                            Rect::new(area.x, area.y, area.w, fh),
                            Rect::new(area.x, area.y + fh + 1, area.w, usable - fh),
                        )
                    }
                };
                first.collect(a, out);
                second.collect(b, out);
            }
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
}
