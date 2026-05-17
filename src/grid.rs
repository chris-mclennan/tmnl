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
