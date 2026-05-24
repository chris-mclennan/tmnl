//! Settings modal — a centered bordered overlay painted into the grid
//! when the user picks `tmnl > Settings…` (⌘,). Keyboard-driven:
//!   ←→ adjust value · ↵ save · esc cancel · r / ⌫ reset.
//!
//! Family settings UI convention (see CLAUDE.md): tmnl's settings
//! modal follows the family idiom (`▸` focus marker, `*` modified
//! marker, `r` reset focused row, `R` reset all, Esc-revert via the
//! `original` snapshot). The full sectioned-list shape (section
//! headers, `[bracket]` choices) doesn't apply here yet — with one
//! numeric setting (`inset`) the modal stays a single-row layout.
//! Numeric-row support is a v2 convention extension; for now we paint
//! the value as a number and use `r` to snap it back to default.
//! When tmnl grows more settings (font size, cursor style, …) this
//! will graduate to the full sectioned list.

use crate::config::Config;
use crate::grid::Grid;

const BG: [f32; 4] = [0.07, 0.08, 0.10, 1.0];
const FG: [f32; 4] = [0.86, 0.87, 0.92, 1.0];
const FG_DIM: [f32; 4] = [0.48, 0.50, 0.58, 1.0];
const SEL_BG: [f32; 4] = [0.18, 0.22, 0.30, 1.0];
const ACCENT: [f32; 4] = [0.93, 0.73, 0.45, 1.0];
const MODIFIED: [f32; 4] = [0.95, 0.79, 0.30, 1.0];

pub struct SettingsState {
    pub cfg: Config,
    pub original: Config,
}

impl SettingsState {
    pub fn open(cfg: Config) -> Self {
        Self {
            original: cfg.clone(),
            cfg,
        }
    }

    pub fn nudge(&mut self, delta: f32) {
        self.cfg.inset = (self.cfg.inset + delta).clamp(0.0, 64.0);
    }

    /// `r` (and ⌫) — reset focused row to default. Only one row today,
    /// so this snaps `inset` back.
    pub fn reset_row(&mut self) {
        self.cfg.inset = Config::default().inset;
    }

    /// `R` — reset everything to defaults. Same as `reset_row` while
    /// there's only one setting; kept distinct so the keymap matches
    /// the family convention for when more settings land.
    pub fn reset_all(&mut self) {
        self.cfg = Config::default();
    }

    /// `true` when `cfg` differs from `Config::default()` — drives the
    /// `*` modified marker.
    pub fn inset_modified(&self) -> bool {
        (self.cfg.inset - Config::default().inset).abs() > f32::EPSILON
    }
}

const TITLE: &str = " tmnl Settings ";
const HINT: &str = "←→ adjust  r reset  ↵ save  esc cancel";
const HELP: &str = "Padding around the shell prompt. TUIs always go edge-to-edge.";

pub fn draw(grid: &mut Grid, st: &SettingsState) {
    let cols = grid.cols;
    let rows = grid.rows;
    if cols < 40 || rows < 10 {
        return;
    }
    let w: u32 = 60.min(cols.saturating_sub(4));
    // Box height bumped 10 → 12 (2026-05-24): the old layout
    // collapsed when `rows` was small enough to drop `h` below 10,
    // and the help line (`row + 2`) and hint line (`h - 2`) ended up
    // painting on the same row — text overlapping the bottom border
    // visually. With h=12 the rows are reliably spaced.
    let h: u32 = 12.min(rows.saturating_sub(4));
    if h < 10 {
        return;
    }
    let x0 = (cols - w) / 2;
    let y0 = (rows - h) / 2;

    for r in y0..y0 + h {
        for c in x0..x0 + w {
            grid.put(c, r, ' ', FG, BG);
        }
    }
    draw_border(grid, x0, y0, w, h);

    let t_x = x0 + (w.saturating_sub(TITLE.chars().count() as u32)) / 2;
    grid.write(t_x, y0, TITLE, ACCENT, BG);

    // Field row — fixed near the top of the box (not centered) so help
    // + hint have a stable gap above the bottom border. `▸` matches
    // the family convention (mnml + mixr).
    let row = y0 + 4;
    for c in x0 + 1..x0 + w - 1 {
        grid.put(c, row, ' ', FG, SEL_BG);
    }
    grid.write(x0 + 4, row, "▸ Inset (px)", ACCENT, SEL_BG);
    let val = format!("{:>3}", st.cfg.inset as i32);
    // `*` modified marker — appended after the value when the row
    // differs from `Config::default()`.
    let modified = st.inset_modified();
    let suffix_width = if modified { 6 } else { 4 }; // " *" adds 2 cells of margin
    let val_col = x0 + w - suffix_width - val.chars().count() as u32;
    grid.write(val_col, row, &val, FG, SEL_BG);
    if modified {
        grid.write(x0 + w - 3, row, "*", MODIFIED, SEL_BG);
    }

    // Help line just below the field (row + 2).
    let help_x = x0 + (w.saturating_sub(HELP.chars().count() as u32)) / 2;
    grid.write(help_x, row + 2, HELP, FG_DIM, BG);

    // Hint footer — one empty row above the bottom border so it
    // doesn't visually merge with the `─` line.
    let h_x = x0 + (w.saturating_sub(HINT.chars().count() as u32)) / 2;
    grid.write(h_x, y0 + h - 3, HINT, FG_DIM, BG);
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
