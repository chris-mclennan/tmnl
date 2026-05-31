//! tmnl's welcome screen — shown over the (otherwise-blank) shell view
//! on startup when [`crate::recents`] has entries. Numbered list of
//! recent native-tab launches (mnml workspaces, mixr, other blit-host apps, …)
//! so the user can re-open their familiar TUI with a single keypress.
//!
//! Keys (while the overlay is up):
//!   1-9        — open that recent entry as a new native tab
//!   ↑/↓ k/j    — move the selection
//!   Enter      — open the focused entry
//!   r          — drop the focused entry from recents
//!   Esc / n    — dismiss (keep the shell-mode pane underneath)
//!
//! Rendering: paints into tmnl's `Grid` directly (same approach as
//! [`crate::settings_ui`]), so the overlay sits on top of whatever
//! the shell-mode pane is showing.

use crate::grid::Grid;
use crate::recents::Entry;

const BG: [f32; 4] = [0.07, 0.08, 0.10, 1.0];
const FG: [f32; 4] = [0.86, 0.87, 0.92, 1.0];
const FG_DIM: [f32; 4] = [0.48, 0.50, 0.58, 1.0];
const SEL_BG: [f32; 4] = [0.18, 0.22, 0.30, 1.0];
const ACCENT: [f32; 4] = [0.93, 0.73, 0.45, 1.0];
const NUM: [f32; 4] = [0.61, 0.69, 0.93, 1.0];

const TITLE: &str = " welcome to tmnl ";
const HINT: &str = "1-9 open · ↑↓ move · ↵ open · r drop · esc dismiss";
const EMPTY_HINT: &str = "no recents yet — drop into the shell or `tmnl --mnml`";

/// State carried across renders while the welcome overlay is up.
pub struct WelcomeState {
    /// Latest snapshot of `~/.config/tmnl/recents.toml`. Reloaded on
    /// every `r` (drop) so a stale list doesn't linger.
    pub entries: Vec<Entry>,
    /// Selected row index, 0-based into `entries`.
    pub selected: usize,
}

impl WelcomeState {
    pub fn open(entries: Vec<Entry>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        let new = (self.selected as isize + delta).rem_euclid(n);
        self.selected = new as usize;
    }

    /// `1..=9` digit → 0-based index, only if in range of `entries`.
    pub fn pick_by_digit(&self, digit: u8) -> Option<usize> {
        if !(1..=9).contains(&digit) {
            return None;
        }
        let idx = (digit - 1) as usize;
        if idx < self.entries.len() {
            Some(idx)
        } else {
            None
        }
    }
}

/// Compute the overlay rect — centered, sized to fit the list.
fn overlay_rect(grid_cols: u32, grid_rows: u32, n_entries: usize) -> (u32, u32, u32, u32) {
    let lines = n_entries.max(1) as u32;
    let w: u32 = 70.min(grid_cols.saturating_sub(4));
    // 1 top border + 1 title row + 1 spacer + lines + 1 spacer + 1 hint + 1 bottom border
    let h: u32 = (6 + lines).min(grid_rows.saturating_sub(4));
    let x = (grid_cols.saturating_sub(w)) / 2;
    let y = (grid_rows.saturating_sub(h)) / 3;
    (x, y, w, h)
}

/// Paint the welcome overlay into `grid`. Skipped when the grid is
/// too small for a useful layout.
pub fn draw(grid: &mut Grid, st: &WelcomeState) {
    let cols = grid.cols;
    let rows = grid.rows;
    if cols < 40 || rows < 10 {
        return;
    }
    let (x0, y0, w, h) = overlay_rect(cols, rows, st.entries.len());

    // Solid background fill.
    for r in y0..y0 + h {
        for c in x0..x0 + w {
            grid.put(c, r, ' ', FG, BG);
        }
    }
    draw_border(grid, x0, y0, w, h);

    // Centered title in the top border.
    let t_x = x0 + (w.saturating_sub(TITLE.chars().count() as u32)) / 2;
    grid.write(t_x, y0, TITLE, ACCENT, BG);

    // Body — entries or the empty-recents hint.
    if st.entries.is_empty() {
        let mid = y0 + h / 2;
        let msg_x = x0 + (w.saturating_sub(EMPTY_HINT.chars().count() as u32)) / 2;
        grid.write(msg_x, mid, EMPTY_HINT, FG_DIM, BG);
    } else {
        // Rows start two lines below the title.
        let body_top = y0 + 2;
        // Visible-row count — clamped to whatever fits between body_top
        // and (y0 + h - 2) (one spacer + hint row).
        let body_h = (y0 + h).saturating_sub(body_top + 2);
        let scroll = if st.selected as u32 >= body_h {
            st.selected as u32 + 1 - body_h
        } else {
            0
        };
        for (i, entry) in st.entries.iter().enumerate().skip(scroll as usize) {
            let row_i = (i as u32).saturating_sub(scroll);
            if row_i >= body_h {
                break;
            }
            let row_y = body_top + row_i;
            let is_selected = i == st.selected;

            // Paint the row's background.
            let row_bg = if is_selected { SEL_BG } else { BG };
            for c in x0 + 1..x0 + w - 1 {
                grid.put(c, row_y, ' ', FG, row_bg);
            }
            // Marker / digit.
            let marker = if is_selected { "▸" } else { " " };
            grid.write(x0 + 2, row_y, marker, ACCENT, row_bg);
            // 1-9 keyboard-pickable digit (10+ shown but not pickable).
            let digit_str = if i < 9 {
                format!("{}", i + 1)
            } else {
                "·".to_string()
            };
            grid.write(x0 + 4, row_y, &digit_str, NUM, row_bg);
            // Entry summary.
            let summary = entry.summary();
            let max_len = (w as usize).saturating_sub(8);
            let s = if summary.chars().count() <= max_len {
                summary
            } else {
                let take = max_len.saturating_sub(1);
                let truncated: String = summary.chars().take(take).collect();
                format!("{truncated}…")
            };
            let row_fg = if is_selected { FG } else { FG_DIM };
            grid.write(x0 + 7, row_y, &s, row_fg, row_bg);
        }
    }

    // Hint footer along the row just inside the bottom border.
    let h_x = x0 + (w.saturating_sub(HINT.chars().count() as u32)) / 2;
    grid.write(h_x, y0 + h - 2, HINT, FG_DIM, BG);
}

fn draw_border(grid: &mut Grid, x: u32, y: u32, w: u32, h: u32) {
    grid.put(x, y, '╭', FG, BG);
    grid.put(x + w - 1, y, '╮', FG, BG);
    grid.put(x, y + h - 1, '╰', FG, BG);
    grid.put(x + w - 1, y + h - 1, '╯', FG, BG);
    for c in x + 1..x + w - 1 {
        grid.put(c, y, '─', FG, BG);
        grid.put(c, y + h - 1, '─', FG, BG);
    }
    for r in y + 1..y + h - 1 {
        grid.put(x, r, '│', FG, BG);
        grid.put(x + w - 1, r, '│', FG, BG);
    }
}
