#[derive(Clone, Copy, Debug, Default)]
pub struct Cell {
    pub ch: char,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub attrs: u32,
}

#[derive(Clone)]
pub struct Grid {
    pub cols: u32,
    pub rows: u32,
    pub cells: Vec<Cell>,
    pub default_bg: [f32; 4],
}

impl Grid {
    pub fn new(cols: u32, rows: u32, default_bg: [f32; 4]) -> Self {
        let blank = Cell {
            ch: ' ',
            fg: [1.0; 4],
            bg: default_bg,
            attrs: 0,
        };
        Self {
            cols,
            rows,
            cells: vec![blank; (cols * rows) as usize],
            default_bg,
        }
    }

    pub fn resize(&mut self, cols: u32, rows: u32) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        let blank = Cell {
            ch: ' ',
            fg: [1.0; 4],
            bg: self.default_bg,
            attrs: 0,
        };
        // Preserve overlapping cells so we don't flash to blank between the
        // window resize and the next Frame arriving from the client. The
        // overlap region keeps the prior content; cells outside it are blank.
        let old_cols = self.cols;
        let old_rows = self.rows;
        let old = std::mem::replace(&mut self.cells, vec![blank; (cols * rows) as usize]);
        let copy_cols = cols.min(old_cols);
        let copy_rows = rows.min(old_rows);
        for r in 0..copy_rows {
            let src = (r * old_cols) as usize;
            let dst = (r * cols) as usize;
            let n = copy_cols as usize;
            self.cells[dst..dst + n].copy_from_slice(&old[src..src + n]);
        }
        self.cols = cols;
        self.rows = rows;
    }

    pub fn clear(&mut self) {
        for c in &mut self.cells {
            c.ch = ' ';
            c.fg = [1.0; 4];
            c.bg = self.default_bg;
            c.attrs = 0;
        }
    }

    pub fn put(&mut self, col: u32, row: u32, ch: char, fg: [f32; 4], bg: [f32; 4]) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let i = (row * self.cols + col) as usize;
        self.cells[i] = Cell {
            ch,
            fg,
            bg,
            attrs: 0,
        };
    }

    pub fn write(&mut self, col: u32, row: u32, s: &str, fg: [f32; 4], bg: [f32; 4]) {
        for (c, ch) in (col..).zip(s.chars()) {
            if c >= self.cols {
                break;
            }
            self.put(c, row, ch, fg, bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: [f32; 4] = [0.1, 0.2, 0.3, 1.0];

    #[test]
    fn new_allocates_blank_cells() {
        let g = Grid::new(4, 3, BG);
        assert_eq!((g.cols, g.rows), (4, 3));
        assert_eq!(g.cells.len(), 12);
        assert!(
            g.cells
                .iter()
                .all(|c| c.ch == ' ' && c.bg == BG && c.attrs == 0)
        );
    }

    #[test]
    fn put_writes_a_cell_and_bounds_checks() {
        let mut g = Grid::new(4, 3, BG);
        g.put(1, 2, 'X', [1.0; 4], BG);
        assert_eq!(g.cells[2 * 4 + 1].ch, 'X');
        // Out of range on either axis — silently ignored, no panic.
        g.put(99, 0, 'Z', [1.0; 4], BG);
        g.put(0, 99, 'Z', [1.0; 4], BG);
        assert!(!g.cells.iter().any(|c| c.ch == 'Z'));
    }

    #[test]
    fn write_lays_a_string_and_clips_at_the_edge() {
        let mut g = Grid::new(5, 1, BG);
        g.write(3, 0, "abcd", [1.0; 4], BG);
        // Only "ab" fits in columns 3 and 4; "cd" is clipped off.
        assert_eq!(g.cells[3].ch, 'a');
        assert_eq!(g.cells[4].ch, 'b');
        assert_eq!(
            g.cells
                .iter()
                .filter(|c| c.ch == 'c' || c.ch == 'd')
                .count(),
            0
        );
    }

    #[test]
    fn clear_resets_every_cell() {
        let mut g = Grid::new(3, 2, BG);
        g.put(0, 0, 'X', [1.0; 4], [0.9; 4]);
        g.cells[1].attrs = 7;
        g.clear();
        assert!(
            g.cells
                .iter()
                .all(|c| c.ch == ' ' && c.bg == BG && c.attrs == 0)
        );
    }

    #[test]
    fn resize_to_same_dims_keeps_content() {
        let mut g = Grid::new(3, 2, BG);
        g.put(1, 1, 'X', [1.0; 4], BG);
        g.resize(3, 2);
        // Cell (row 1, col 1) — `row * cols + col` at stride 3.
        assert_eq!(g.cells[(g.cols + 1) as usize].ch, 'X');
    }

    #[test]
    fn resize_smaller_preserves_the_overlapping_region() {
        let mut g = Grid::new(4, 4, BG);
        g.put(0, 0, 'A', [1.0; 4], BG);
        g.put(2, 2, 'B', [1.0; 4], BG);
        g.put(3, 3, 'C', [1.0; 4], BG);
        // Shrink to 3×3 — A and B are inside the overlap, C is dropped.
        g.resize(3, 3);
        assert_eq!((g.cols, g.rows), (3, 3));
        assert_eq!(g.cells[0].ch, 'A');
        assert_eq!(g.cells[2 * 3 + 2].ch, 'B');
        assert!(!g.cells.iter().any(|c| c.ch == 'C'));
    }

    #[test]
    fn resize_larger_keeps_old_content_at_the_new_stride() {
        let mut g = Grid::new(2, 2, BG);
        g.put(1, 1, 'X', [1.0; 4], BG);
        g.resize(5, 4);
        assert_eq!((g.cols, g.rows), (5, 4));
        // X stays at cell (row 1, col 1) — re-indexed to the new stride.
        assert_eq!(g.cells[(g.cols + 1) as usize].ch, 'X');
        assert_eq!(g.cells.iter().filter(|c| c.ch != ' ').count(), 1);
    }
}
