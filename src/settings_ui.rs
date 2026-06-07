//! Settings modal — a centered bordered overlay painted into the grid
//! when the user picks `tmnl > Settings…` (⌘,). Keyboard-driven:
//!   ↑↓ move row · ←→ adjust value · ↵ save · esc cancel · r / ⌫ reset.
//!
//! Family settings UI convention (see CLAUDE.md): tmnl's settings
//! modal follows the family idiom (`▸` focus marker, `*` modified
//! marker, `r` reset focused row, `R` reset all, Esc-revert via the
//! `original` snapshot). Two row kinds are supported:
//!   * **Numeric** — `Inset (px)`. Arrow keys nudge by 1.
//!   * **Enum** — `Tab layout`. Arrow keys step through the choices.
//!
//! Section headers / per-section grouping ship when there are
//! enough rows to justify them.

use crate::config::{Config, TabLayout};
use crate::grid::Grid;

const BG: [f32; 4] = [0.07, 0.08, 0.10, 1.0];
const FG: [f32; 4] = [0.86, 0.87, 0.92, 1.0];
const FG_DIM: [f32; 4] = [0.48, 0.50, 0.58, 1.0];
const SEL_BG: [f32; 4] = [0.18, 0.22, 0.30, 1.0];
const ACCENT: [f32; 4] = [0.93, 0.73, 0.45, 1.0];
const MODIFIED: [f32; 4] = [0.95, 0.79, 0.30, 1.0];

/// Visible rows in the settings modal — the order they render top-
/// to-bottom. Used to translate `focused` index ↔ which field to
/// edit on a nudge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Inset,
    TabLayout,
}

const ROWS: &[RowKind] = &[RowKind::Inset, RowKind::TabLayout];

pub struct SettingsState {
    pub cfg: Config,
    pub original: Config,
    /// Index into [`ROWS`] — the currently focused row.
    pub focused: usize,
}

impl SettingsState {
    pub fn open(cfg: Config) -> Self {
        Self {
            original: cfg.clone(),
            cfg,
            focused: 0,
        }
    }

    fn focused_row(&self) -> RowKind {
        ROWS[self.focused.min(ROWS.len() - 1)]
    }

    /// ↑↓ — cycle through rows. Saturates at the ends rather than
    /// wrapping (less confusing for a tiny list).
    pub fn focus_prev(&mut self) {
        self.focused = self.focused.saturating_sub(1);
    }

    pub fn focus_next(&mut self) {
        self.focused = (self.focused + 1).min(ROWS.len() - 1);
    }

    /// ← / → — adjust the focused row's value by `delta`. For
    /// numeric rows this is a literal nudge; for enum rows it steps
    /// the choice list (delta sign = direction).
    pub fn nudge(&mut self, delta: f32) {
        match self.focused_row() {
            RowKind::Inset => {
                self.cfg.inset = (self.cfg.inset + delta).clamp(0.0, 64.0);
            }
            RowKind::TabLayout => {
                // Two values, so any non-zero delta toggles.
                self.cfg.tab_layout = match self.cfg.tab_layout {
                    TabLayout::Horizontal => TabLayout::Vertical,
                    TabLayout::Vertical => TabLayout::Horizontal,
                };
            }
        }
    }

    /// `r` (and ⌫) — reset focused row to default.
    pub fn reset_row(&mut self) {
        match self.focused_row() {
            RowKind::Inset => self.cfg.inset = Config::default().inset,
            RowKind::TabLayout => self.cfg.tab_layout = Config::default().tab_layout,
        }
    }

    /// `R` — reset everything to defaults.
    pub fn reset_all(&mut self) {
        self.cfg = Config::default();
    }

    fn inset_modified(&self) -> bool {
        (self.cfg.inset - Config::default().inset).abs() > f32::EPSILON
    }

    fn tab_layout_modified(&self) -> bool {
        self.cfg.tab_layout != Config::default().tab_layout
    }

    fn row_modified(&self, kind: RowKind) -> bool {
        match kind {
            RowKind::Inset => self.inset_modified(),
            RowKind::TabLayout => self.tab_layout_modified(),
        }
    }
}

const TITLE: &str = " tmnl Settings ";
const HINT: &str = "↑↓ row  ←→ adjust  r reset  ↵ save  esc cancel";

fn row_label(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Inset => "Inset (px)",
        RowKind::TabLayout => "Tab layout",
    }
}

fn row_help(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Inset => "Padding around the shell prompt. TUIs always go edge-to-edge.",
        RowKind::TabLayout => {
            "Where tab chips render — horizontal row below the strip, or vertical sidebar."
        }
    }
}

pub fn draw(grid: &mut Grid, st: &SettingsState) {
    let cols = grid.cols;
    let rows = grid.rows;
    if cols < 40 || rows < 14 {
        return;
    }
    let w: u32 = 60.min(cols.saturating_sub(4));
    // Box height bumped to fit two rows + a help line per focused
    // row + the hint footer. 14 rows total: 1 top border, 2 padding,
    // 1 title gap, N field rows, 1 gap, 1 help, 1 gap, 1 hint, 1 gap,
    // 1 bottom border.
    let h: u32 = 14.min(rows.saturating_sub(4));
    if h < 12 {
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

    // Field rows — start at y0+3 (just below title row + 1 gap). Each
    // row gets 1 line. Focused row's bg = SEL_BG so the cursor is
    // obvious; non-focused row's bg = BG (the box body).
    let first_row_y = y0 + 3;
    for (i, kind) in ROWS.iter().enumerate() {
        let row_y = first_row_y + i as u32;
        let is_focus = st.focused == i;
        let row_bg = if is_focus { SEL_BG } else { BG };
        let row_fg = if is_focus { ACCENT } else { FG };
        // Paint the row's bg edge-to-edge (inside the borders).
        for c in x0 + 1..x0 + w - 1 {
            grid.put(c, row_y, ' ', FG, row_bg);
        }
        let prefix = if is_focus { "▸ " } else { "  " };
        grid.write(x0 + 2, row_y, prefix, ACCENT, row_bg);
        grid.write(x0 + 4, row_y, row_label(*kind), row_fg, row_bg);
        // Right side: value + modified marker.
        let value_text = match kind {
            RowKind::Inset => format!("{:>3}", st.cfg.inset as i32),
            RowKind::TabLayout => render_enum_choices(st.cfg.tab_layout),
        };
        let modified = st.row_modified(*kind);
        let suffix_w = if modified { 6 } else { 4 };
        let val_col = x0 + w - suffix_w - value_text.chars().count() as u32;
        grid.write(val_col, row_y, &value_text, row_fg, row_bg);
        if modified {
            grid.write(x0 + w - 3, row_y, "*", MODIFIED, row_bg);
        }
    }

    // Help line for the focused row — sits under the row block with
    // a 1-row gap.
    let help = row_help(st.focused_row());
    let help_x = x0 + (w.saturating_sub(help.chars().count() as u32)) / 2;
    let help_y = first_row_y + ROWS.len() as u32 + 1;
    grid.write(help_x, help_y, help, FG_DIM, BG);

    // Hint footer — one empty row above the bottom border so it
    // doesn't visually merge with the `─` line.
    let h_x = x0 + (w.saturating_sub(HINT.chars().count() as u32)) / 2;
    grid.write(h_x, y0 + h - 3, HINT, FG_DIM, BG);
}

/// Render the `[active]` / other format for an enum field. Active
/// choice is in brackets, others are plain — matches the family UI
/// convention.
fn render_enum_choices(current: TabLayout) -> String {
    let mut out = String::new();
    let choices = [
        ("horizontal", TabLayout::Horizontal),
        ("vertical", TabLayout::Vertical),
    ];
    for (i, (label, value)) in choices.iter().enumerate() {
        if i > 0 {
            out.push_str(" / ");
        }
        if *value == current {
            out.push('[');
            out.push_str(label);
            out.push(']');
        } else {
            out.push_str(label);
        }
    }
    out
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
