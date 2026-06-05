//! tmnl's standalone command palette — VS Code-style fuzzy picker
//! over the entries in [`crate::command::registry`]. Sibling to
//! `help.rs`; same overlay-rect math, same wgpu cell-grid render,
//! but with a typed substring filter and Enter-dispatches semantics.
//!
//! Activated by `view.palette` (default chord `cmd+shift+p` when the
//! focused pane is *not* a Native one — Native panes keep the
//! existing Cmd→Ctrl forward so mnml's own palette opens). The
//! command also accepts `f1` as the terminal-proof alias.
//!
//! Keys (while the overlay is up):
//!   Esc                — dismiss
//!   Enter              — run the highlighted command via
//!                        `command::dispatch_by_id`
//!   ↑ / k              — move highlight up
//!   ↓ / j              — move highlight down
//!   Backspace          — drop last filter char
//!   any printable      — append to filter; reset highlight to top

use crate::command::registry;
use crate::grid::Grid;

const BG: [f32; 4] = [0.07, 0.08, 0.10, 1.0];
const FG: [f32; 4] = [0.86, 0.87, 0.92, 1.0];
const FG_DIM: [f32; 4] = [0.48, 0.50, 0.58, 1.0];
const ACCENT: [f32; 4] = [0.93, 0.73, 0.45, 1.0];
const SEL_BG: [f32; 4] = [0.21, 0.55, 0.78, 1.0];
const SEL_FG: [f32; 4] = [0.04, 0.05, 0.08, 1.0];
const KEY: [f32; 4] = [0.61, 0.69, 0.93, 1.0];

const HINT: &str = "type to filter · ↑↓ move · Enter run · Esc close";

#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    pub filter: String,
    pub cursor: usize,
    /// Combined-list index of the highlighted entry. Local commands
    /// come first (0..`local_len`), then remote (local_len..end).
    pub selected: usize,
    /// Commands aggregated from the focused Native pane via
    /// `Message::ClientCommands`. Populated lazily after the palette
    /// opens; an empty Vec means we've sent the request but haven't
    /// received the response yet (or there's no Native pane focused).
    /// v1: single source — the focused pane only.
    pub remote_commands: Vec<tmnl_protocol::CommandInfo>,
}

/// One row's identity. Returned by `current_entry` so the dispatcher
/// can route Local entries through the local registry and Remote
/// entries back to the focused Native pane via `send_run_client_command`.
#[derive(Debug, Clone)]
pub enum PaletteEntry {
    Local(&'static str),
    Remote(tmnl_protocol::CommandInfo),
}

impl PaletteState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Length of the local segment in the combined list — local
    /// commands come first; remote commands are indices >= this.
    pub fn local_len(&self) -> usize {
        registry().all().len()
    }

    pub fn total_len(&self) -> usize {
        self.local_len() + self.remote_commands.len()
    }

    /// Indices into the combined (local + remote) list matching the
    /// current filter (case-insensitive substring against title +
    /// id). Empty filter ⇒ every entry.
    pub fn visible_indices(&self) -> Vec<usize> {
        let local_len = self.local_len();
        let total = self.total_len();
        if self.filter.is_empty() {
            return (0..total).collect();
        }
        let needle = self.filter.to_ascii_lowercase();
        let mut out = Vec::with_capacity(total);
        for (i, c) in registry().all().iter().enumerate() {
            let hay = format!("{} {}", c.title, c.id).to_ascii_lowercase();
            if hay.contains(&needle) {
                out.push(i);
            }
        }
        for (j, info) in self.remote_commands.iter().enumerate() {
            let hay = format!("{} {} {}", info.title, info.id, info.group).to_ascii_lowercase();
            if hay.contains(&needle) {
                out.push(local_len + j);
            }
        }
        out
    }

    /// Resolve a combined-list index to a Local or Remote entry.
    pub fn entry_at(&self, idx: usize) -> Option<PaletteEntry> {
        let local_len = self.local_len();
        if idx < local_len {
            registry().all().get(idx).map(|c| PaletteEntry::Local(c.id))
        } else {
            self.remote_commands
                .get(idx - local_len)
                .cloned()
                .map(PaletteEntry::Remote)
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0) as isize;
        let new_pos = (pos + delta).clamp(0, visible.len() as isize - 1) as usize;
        self.selected = visible[new_pos];
    }

    pub fn insert_char(&mut self, c: char) {
        let byte = self
            .filter
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or_else(|| self.filter.len());
        self.filter.insert(byte, c);
        self.cursor += 1;
        // Reset to first match — the filtered set just changed.
        let visible = self.visible_indices();
        self.selected = visible.first().copied().unwrap_or(0);
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self
            .filter
            .char_indices()
            .nth(self.cursor - 1)
            .map(|(b, _)| b)
            .unwrap_or(0);
        let end = self
            .filter
            .char_indices()
            .nth(self.cursor)
            .map(|(b, _)| b)
            .unwrap_or_else(|| self.filter.len());
        self.filter.replace_range(start..end, "");
        self.cursor -= 1;
        // Clamp selection into the new (larger) visible set.
        let visible = self.visible_indices();
        if !visible.is_empty() && !visible.contains(&self.selected) {
            self.selected = visible[0];
        }
    }

    /// Issue id of the currently-highlighted command, or `None` if
    /// the filtered set is empty. Local entries return the static id;
    /// remote entries fall through `current_entry` for routing. Kept
    /// for callers that only care about the static-id case; the Enter
    /// dispatcher in `app.rs` uses `current_entry` directly.
    #[allow(dead_code)]
    pub fn current_id(&self) -> Option<&'static str> {
        match self.current_entry()? {
            PaletteEntry::Local(id) => Some(id),
            PaletteEntry::Remote(_) => None,
        }
    }

    /// Highlighted entry (Local or Remote). Drives the Enter dispatch
    /// in `app.rs`.
    pub fn current_entry(&self) -> Option<PaletteEntry> {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return None;
        }
        let pos = visible.iter().position(|&i| i == self.selected)?;
        self.entry_at(visible[pos])
    }
}

fn overlay_rect(grid_cols: u32, grid_rows: u32) -> (u32, u32, u32, u32) {
    let w: u32 = 80.min(grid_cols.saturating_sub(4)).max(40);
    let h: u32 = grid_rows.saturating_sub(6).clamp(10, 40);
    let x = (grid_cols.saturating_sub(w)) / 2;
    let y = grid_rows.saturating_sub(h) / 2;
    (x, y, w, h)
}

/// Paint the palette overlay into `grid`. Skipped if the grid is too
/// small for a useful layout.
pub fn draw(grid: &mut Grid, st: &PaletteState) {
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

    let visible = st.visible_indices();
    let total = st.total_len();
    let title = format!(" command palette ({}/{}) ", visible.len(), total);
    let t_x = x0 + (w.saturating_sub(title.chars().count() as u32)) / 2;
    grid.write(t_x, y0, &title, ACCENT, BG);

    // Filter line right under the top border. `/<filter>│` with a
    // cyan cursor block.
    let inner_x = x0 + 2;
    let inner_w = (w as usize).saturating_sub(4);
    let filter_y = y0 + 2;
    {
        let chars: Vec<char> = st.filter.chars().collect();
        let cursor = st.cursor.min(chars.len());
        let avail = inner_w.saturating_sub(2);
        let start = if cursor >= avail {
            cursor - avail + 1
        } else {
            0
        };
        let end = (start + avail).min(chars.len());
        let head: String = chars[start..cursor].iter().collect();
        let tail: String = chars[cursor..end].iter().collect();
        grid.write(inner_x, filter_y, "/", ACCENT, BG);
        grid.write(inner_x + 1, filter_y, &head, FG, BG);
        let bar_x = inner_x + 1 + (cursor - start) as u32;
        grid.write(bar_x, filter_y, "│", ACCENT, BG);
        grid.write(bar_x + 1, filter_y, &tail, FG_DIM, BG);
    }

    // Body — between filter_y+2 and (y0 + h - 2) (one row reserved
    // for the hint at the bottom).
    let body_top = filter_y + 2;
    let body_h = (y0 + h).saturating_sub(body_top + 2) as usize;

    if visible.is_empty() {
        grid.write(inner_x, body_top, "(no commands match)", FG_DIM, BG);
    } else {
        // Window list around the selected row.
        let sel_pos = visible.iter().position(|&i| i == st.selected).unwrap_or(0);
        let start_pos = sel_pos.saturating_sub(body_h / 2);
        let end_pos = (start_pos + body_h).min(visible.len());

        // Pre-resolve every visible entry once — local entries use the
        // command's key hint as the left column; remote entries use a
        // dim "mnml" / "<group>" prefix instead since they don't have
        // keybindings on tmnl's side.
        let resolved: Vec<(usize, Option<PaletteEntry>)> = visible[start_pos..end_pos]
            .iter()
            .map(|&i| (i, st.entry_at(i)))
            .collect();

        // Key column width — capped at 22 to leave room for titles.
        let key_col_w: usize = resolved
            .iter()
            .map(|(_, e)| match e {
                Some(PaletteEntry::Local(id)) => registry()
                    .all()
                    .iter()
                    .find(|c| c.id == *id)
                    .map(|c| c.key_hint().chars().count())
                    .unwrap_or(0),
                Some(PaletteEntry::Remote(info)) => {
                    // Remote source label: lowercase group, capped.
                    info.group.chars().count() + 2 // brackets
                }
                None => 0,
            })
            .max()
            .unwrap_or(0)
            .clamp(0, 22);

        for (i, (cmd_idx, entry)) in resolved.iter().enumerate() {
            let row_y = body_top + i as u32;
            let is_sel = *cmd_idx == st.selected;
            let (row_fg, row_bg) = if is_sel { (SEL_FG, SEL_BG) } else { (FG, BG) };
            // Highlight the full row by painting a space first.
            if is_sel {
                for c in inner_x..inner_x + inner_w as u32 {
                    grid.put(c, row_y, ' ', row_fg, row_bg);
                }
            }
            let (keys, title, is_remote) = match entry {
                Some(PaletteEntry::Local(id)) => {
                    match registry().all().iter().find(|c| c.id == *id) {
                        Some(cmd) => (cmd.key_hint().to_string(), cmd.title.to_string(), false),
                        None => continue,
                    }
                }
                Some(PaletteEntry::Remote(info)) => {
                    let group = if info.group.is_empty() {
                        "remote".to_string()
                    } else {
                        info.group.clone()
                    };
                    (format!("[{group}]"), info.title.clone(), true)
                }
                None => continue,
            };
            let keys_color = if is_sel {
                SEL_FG
            } else if is_remote {
                FG_DIM
            } else {
                KEY
            };
            grid.write(inner_x, row_y, &keys, keys_color, row_bg);
            let title_x = inner_x + (key_col_w as u32) + 2;
            let max_title = inner_w.saturating_sub(key_col_w + 2);
            let s = if title.chars().count() <= max_title {
                title
            } else {
                let take = max_title.saturating_sub(1);
                let truncated: String = title.chars().take(take).collect();
                format!("{truncated}…")
            };
            grid.write(title_x, row_y, &s, row_fg, row_bg);
        }
    }

    // Hint footer.
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
    fn visible_indices_empty_filter_returns_all() {
        let st = PaletteState::default();
        let all_len = registry().all().len();
        assert_eq!(st.visible_indices().len(), all_len);
    }

    #[test]
    fn visible_indices_filter_narrows_set() {
        let st = PaletteState {
            filter: "tab".to_string(),
            ..PaletteState::default()
        };
        let visible = st.visible_indices();
        assert!(
            !visible.is_empty(),
            "expected `tab` to match tab.* commands"
        );
        // Every match should contain "tab" in title or id.
        let all = registry().all();
        for &i in &visible {
            let hay = format!("{} {}", all[i].title, all[i].id).to_ascii_lowercase();
            assert!(
                hay.contains("tab"),
                "match {} didn't contain 'tab'",
                all[i].id
            );
        }
    }

    #[test]
    fn insert_char_appends_and_resets_selection() {
        let mut st = PaletteState {
            selected: 999_999,
            ..PaletteState::default()
        };
        st.insert_char('z');
        // After insert, selected should be a valid index from
        // visible_indices().
        let visible = st.visible_indices();
        if visible.is_empty() {
            // Filter matched nothing — selected stays at 0.
            assert_eq!(st.selected, 0);
        } else {
            assert_eq!(st.selected, visible[0]);
        }
    }

    #[test]
    fn backspace_clamps_selection_into_new_visible_set() {
        let mut st = PaletteState::default();
        st.insert_char('t');
        st.insert_char('a');
        st.insert_char('b');
        // Now we're in `tab` matches.
        st.backspace();
        st.backspace();
        st.backspace();
        // Back to empty filter — selected should still be valid.
        let visible = st.visible_indices();
        assert!(visible.contains(&st.selected));
    }
}
