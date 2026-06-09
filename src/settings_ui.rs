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
    ThemedPrompt,
}

const ROWS: &[RowKind] = &[RowKind::Inset, RowKind::TabLayout, RowKind::ThemedPrompt];

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
            RowKind::ThemedPrompt => {
                // Boolean toggle — any non-zero delta flips.
                self.cfg.themed_prompt = !self.cfg.themed_prompt;
            }
        }
    }

    /// `r` (and ⌫) — reset focused row to default.
    pub fn reset_row(&mut self) {
        match self.focused_row() {
            RowKind::Inset => self.cfg.inset = Config::default().inset,
            RowKind::TabLayout => self.cfg.tab_layout = Config::default().tab_layout,
            RowKind::ThemedPrompt => self.cfg.themed_prompt = Config::default().themed_prompt,
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

    fn themed_prompt_modified(&self) -> bool {
        self.cfg.themed_prompt != Config::default().themed_prompt
    }

    fn row_modified(&self, kind: RowKind) -> bool {
        match kind {
            RowKind::Inset => self.inset_modified(),
            RowKind::TabLayout => self.tab_layout_modified(),
            RowKind::ThemedPrompt => self.themed_prompt_modified(),
        }
    }
}

const TITLE: &str = " tmnl Settings ";
const HINT: &str = "↑↓ row  ←→ adjust  r reset  ↵ save  esc cancel";

fn row_label(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Inset => "Inset (px)",
        RowKind::TabLayout => "Tab layout",
        RowKind::ThemedPrompt => "Themed prompt",
    }
}

fn row_help(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Inset => "Padding around the shell prompt. TUIs always go edge-to-edge.",
        RowKind::TabLayout => {
            "Where tab chips render — horizontal row below the strip, or vertical sidebar."
        }
        RowKind::ThemedPrompt => {
            "Powerline-style prompt that color-matches tmnl's theme. First time you turn it on, tmnl appends a source line to ~/.zshrc."
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

    // Version chip in the top-right corner of the panel border —
    // shows what version of the app the user is running so they
    // can match it against the public release tag / changelog.
    // 2026-06-08 family-wide ask.
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let version_len = version.chars().count() as u32;
    if w > version_len + 4 {
        let v_x = x0 + w - version_len - 2;
        grid.write(v_x, y0, version, FG_DIM, BG);
    }

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
            RowKind::ThemedPrompt => render_bool_choices(st.cfg.themed_prompt),
        };
        let modified = st.row_modified(*kind);
        let suffix_w = if modified { 6 } else { 4 };
        let val_col = x0 + w - suffix_w - value_text.chars().count() as u32;
        grid.write(val_col, row_y, &value_text, row_fg, row_bg);
        if modified {
            grid.write(x0 + w - 3, row_y, "*", MODIFIED, row_bg);
        }
    }

    // Help line(s) for the focused row — sits under the row block with
    // a 1-row gap. Word-wraps to the panel's inner width so long
    // descriptions don't punch through the right border (the
    // `Where tab chips render — horizontal row below the strip, or
    // vertical sidebar.` line was 80 chars in a 60-cell panel).
    let help = row_help(st.focused_row());
    let help_inner_w = (w as usize).saturating_sub(4); // 2 cells of pad each side
    let help_y = first_row_y + ROWS.len() as u32 + 1;
    // Cap at how many rows we have available between help_y and the
    // hint footer (at y0 + h - 3) — leave 1 row of gap above the hint.
    let help_max_lines = (y0 + h - 3).saturating_sub(help_y + 1).max(1);
    for (i, line) in wrap_text(help, help_inner_w)
        .into_iter()
        .take(help_max_lines as usize)
        .enumerate()
    {
        let lx = x0 + (w.saturating_sub(line.chars().count() as u32)) / 2;
        grid.write(lx, help_y + i as u32, &line, FG_DIM, BG);
    }

    // Hint footer — one empty row above the bottom border so it
    // doesn't visually merge with the `─` line.
    let h_x = x0 + (w.saturating_sub(HINT.chars().count() as u32)) / 2;
    grid.write(h_x, y0 + h - 3, HINT, FG_DIM, BG);
}

/// Render the `[active]` / other format for a boolean field — same
/// `[off] / on` look the family UI gives any 2-value choice. Family
/// convention puts `off` first so the row reads left-to-right as
/// least-to-most-side-effect.
fn render_bool_choices(current: bool) -> String {
    if current {
        "off / [on]".to_string()
    } else {
        "[off] / on".to_string()
    }
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

/// Greedy word-wrap of `text` to lines of at most `width` chars.
/// Hard-splits any single word longer than `width`. Returns at least
/// one line (an empty input yields `[""]`). Width counted in chars,
/// not display cells — fine for ASCII / Latin descriptions; CJK +
/// emoji would over-estimate fit (not a concern for the current
/// fixed help strings).
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            out.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
        while cur.chars().count() > width {
            let head: String = cur.chars().take(width).collect();
            cur = cur.chars().skip(width).collect();
            out.push(head);
        }
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
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
