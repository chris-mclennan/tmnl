//! Settings modal — a centered bordered overlay painted into the grid
//! when the user picks `tmnl > Settings…` (⌘,). Keyboard-driven:
//!   ←→ adjust value · Enter save & close · Esc cancel · ⌫ reset.
//!
//! One setting today: pixel inset around the shell-prompt view. TUIs
//! (native mode + shell-with-altscreen) always get 0 — the user
//! shouldn't have to think about per-mode overrides.

use crate::config::Config;
use crate::grid::Grid;

const BG: [f32; 4] = [0.07, 0.08, 0.10, 1.0];
const FG: [f32; 4] = [0.86, 0.87, 0.92, 1.0];
const FG_DIM: [f32; 4] = [0.48, 0.50, 0.58, 1.0];
const SEL_BG: [f32; 4] = [0.18, 0.22, 0.30, 1.0];
const ACCENT: [f32; 4] = [0.93, 0.73, 0.45, 1.0];

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

    pub fn reset(&mut self) {
        self.cfg.inset = Config::default().inset;
    }
}

const TITLE: &str = " tmnl Settings ";
const HINT: &str = "←→ adjust  ⌫ reset  ↵ save  esc cancel";
const HELP: &str = "Padding around the shell prompt. TUIs always go edge-to-edge.";

pub fn draw(grid: &mut Grid, st: &SettingsState) {
    let cols = grid.cols;
    let rows = grid.rows;
    if cols < 40 || rows < 10 {
        return;
    }
    let w: u32 = 60.min(cols.saturating_sub(4));
    let h: u32 = 10.min(rows.saturating_sub(4));
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

    // Field row — centered vertically inside the box.
    let row = y0 + h / 2;
    for c in x0 + 1..x0 + w - 1 {
        grid.put(c, row, ' ', FG, SEL_BG);
    }
    grid.write(x0 + 4, row, "▶ Inset (px)", ACCENT, SEL_BG);
    let val = format!("{:>3}", st.cfg.inset as i32);
    let val_col = x0 + w - 4 - val.chars().count() as u32;
    grid.write(val_col, row, &val, FG, SEL_BG);

    // Help line just below the field.
    let help_x = x0 + (w.saturating_sub(HELP.chars().count() as u32)) / 2;
    grid.write(help_x, row + 2, HELP, FG_DIM, BG);

    // Hint footer along the bottom border row.
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
