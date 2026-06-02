//! tmnl's help overlay — auto-generated from the command registry.
//!
//! Toggled by `view.help` (default chord `cmd+shift+/`, the macOS Help
//! convention). Walks [`crate::command::help_rows`] to render a
//! centered, scrollable list of `<chord>  <title>` entries grouped by
//! `Command.group`. The underlying tab keeps refreshing below the
//! overlay so closing it (Esc / `?` again) reveals the world
//! unchanged.
//!
//! Keys (while the overlay is up):
//!   Esc / ?           — dismiss
//!   ↑ / k             — scroll up one row
//!   ↓ / j             — scroll down one row
//!   PageUp / PageDown — scroll a page

use crate::command::help_rows;
use crate::grid::Grid;

const BG: [f32; 4] = [0.07, 0.08, 0.10, 1.0];
const FG: [f32; 4] = [0.86, 0.87, 0.92, 1.0];
const FG_DIM: [f32; 4] = [0.48, 0.50, 0.58, 1.0];
const ACCENT: [f32; 4] = [0.93, 0.73, 0.45, 1.0];
const KEY: [f32; 4] = [0.61, 0.69, 0.93, 1.0];

const TITLE: &str = " help ";
const HINT: &str = "↑↓ / j k scroll · PageUp/Down page · Esc / ? close";

/// State carried across renders while the help overlay is up.
#[derive(Debug, Clone, Default)]
pub struct HelpState {
    /// First visible row offset (after the row count is computed).
    pub scroll: usize,
}

impl HelpState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scroll(&mut self, delta: isize) {
        let new = (self.scroll as isize + delta).max(0) as usize;
        self.scroll = new;
    }
}

/// One displayed row in the overlay — either a group section header
/// or a binding.
enum Row {
    Section(String),
    Binding { keys: String, title: String },
}

/// Walk the registry and build the row list — section headers
/// between group changes, then each binding under its group.
fn build_rows() -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    let mut last_group: String = String::new();
    for (keys, title, group) in help_rows() {
        if group != last_group {
            if !last_group.is_empty() {
                out.push(Row::Section(String::new())); // blank spacer
            }
            out.push(Row::Section(format!("── {group} ──")));
            last_group = group.to_string();
        }
        out.push(Row::Binding {
            keys,
            title: title.to_string(),
        });
    }
    out
}

/// Compute the overlay rect — centered, sized to fit the list (with
/// reasonable caps so it doesn't blow out the window).
fn overlay_rect(grid_cols: u32, grid_rows: u32) -> (u32, u32, u32, u32) {
    let w: u32 = 80.min(grid_cols.saturating_sub(4)).max(40);
    let h: u32 = grid_rows.saturating_sub(6).clamp(10, 40);
    let x = (grid_cols.saturating_sub(w)) / 2;
    let y = grid_rows.saturating_sub(h) / 2;
    (x, y, w, h)
}

/// Paint the help overlay into `grid`. Skipped if the grid is too
/// small for a useful layout.
pub fn draw(grid: &mut Grid, st: &HelpState) {
    let cols = grid.cols;
    let rows = grid.rows;
    if cols < 40 || rows < 10 {
        return;
    }
    let (x0, y0, w, h) = overlay_rect(cols, rows);

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

    // Body — between body_top and (y0 + h - 2) (one row reserved for
    // the hint at the bottom). Two-line padding from the title.
    let body_top = y0 + 2;
    let body_h = (y0 + h).saturating_sub(body_top + 2) as usize;
    let rows_data = build_rows();

    // Clamp scroll so we can't end up past the last row.
    let max_scroll = rows_data.len().saturating_sub(body_h);
    let scroll = st.scroll.min(max_scroll);

    // Key column width — wide enough for the longest visible key
    // hint, capped at 24 to leave room for the title.
    let key_col_w: usize = rows_data
        .iter()
        .filter_map(|r| match r {
            Row::Binding { keys, .. } => Some(keys.chars().count()),
            _ => None,
        })
        .max()
        .unwrap_or(12)
        .clamp(8, 24);

    for (i, row) in rows_data.iter().enumerate().skip(scroll).take(body_h) {
        let row_y = body_top + (i - scroll) as u32;
        let inner_x = x0 + 2;
        let inner_w = (w as usize).saturating_sub(4);
        match row {
            Row::Section(label) => {
                grid.write(inner_x, row_y, label, FG_DIM, BG);
            }
            Row::Binding { keys, title } => {
                // Keys column (left-aligned).
                grid.write(inner_x, row_y, keys, KEY, BG);
                // Title (after the keys column + 2-space gutter).
                let title_x = inner_x + (key_col_w as u32) + 2;
                let max_title = inner_w.saturating_sub(key_col_w + 2);
                let s = if title.chars().count() <= max_title {
                    title.clone()
                } else {
                    let take = max_title.saturating_sub(1);
                    let truncated: String = title.chars().take(take).collect();
                    format!("{truncated}…")
                };
                grid.write(title_x, row_y, &s, FG, BG);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rows_emits_section_headers_and_bindings() {
        let rows = build_rows();
        let n_sections = rows.iter().filter(|r| matches!(r, Row::Section(_))).count();
        let n_bindings = rows
            .iter()
            .filter(|r| matches!(r, Row::Binding { .. }))
            .count();
        assert!(n_sections > 0, "expected at least one section header");
        assert!(
            n_bindings >= 30,
            "expected the command registry to have substantial bindings (got {n_bindings})"
        );
    }
}
