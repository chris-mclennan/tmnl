//! Shell mode — tmnl hosts a real pty and parses its output via the `vt100`
//! crate into our cell `Grid`. The companion to native mode: identical
//! renderer downstream, totally different source of cells.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::grid::Grid;

const SCROLLBACK: usize = 4000;

const ATTR_BOLD: u32 = 1 << 0;
const ATTR_DIM: u32 = 1 << 1;
const ATTR_ITALIC: u32 = 1 << 2;
const ATTR_UNDERLINE: u32 = 1 << 3;

pub struct ShellSession {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    reader: Option<JoinHandle<()>>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<Mutex<bool>>,
    last_size: (u16, u16),
    bytes_seen: Arc<AtomicU64>,
    last_applied_bytes: u64,
    default_bg: [f32; 4],
    default_fg: [f32; 4],
}

impl ShellSession {
    pub fn spawn(
        rows: u16,
        cols: u16,
        default_fg: [f32; 4],
        default_bg: [f32; 4],
    ) -> Result<Self, String> {
        let (rows, cols) = (rows.max(4), cols.max(20));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("openpty: {e}"))?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", "xterm-256color");
        // Login shell so users' rc files load and the prompt is set up.
        cmd.arg("-l");
        if let Ok(home) = std::env::var("HOME") {
            cmd.cwd(&home);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("spawn {shell}: {e}"))?;
        drop(pair.slave);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
        let exited = Arc::new(Mutex::new(false));
        let bytes_seen = Arc::new(AtomicU64::new(0));

        let mut reader_handle = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("clone pty reader: {e}"))?;
        let r_parser = Arc::clone(&parser);
        let r_exited = Arc::clone(&exited);
        let r_bytes = Arc::clone(&bytes_seen);
        let reader = thread::Builder::new()
            .name("tmnl-shell-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader_handle.read(&mut buf) {
                        Ok(0) | Err(_) => {
                            if let Ok(mut e) = r_exited.lock() {
                                *e = true;
                            }
                            return;
                        }
                        Ok(n) => {
                            if let Ok(mut p) = r_parser.lock() {
                                p.process(&buf[..n]);
                            }
                            r_bytes.fetch_add(n as u64, Ordering::Relaxed);
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn reader thread: {e}"))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("take pty writer: {e}"))?;

        Ok(ShellSession {
            parser,
            writer,
            master: pair.master,
            reader: Some(reader),
            child,
            exited,
            last_size: (rows, cols),
            bytes_seen,
            last_applied_bytes: 0,
            default_bg,
            default_fg,
        })
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(4), cols.max(20));
        if self.last_size == (rows, cols) {
            return;
        }
        self.last_size = (rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            p.set_size(rows, cols);
        }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn exited(&self) -> bool {
        *self.exited.lock().unwrap()
    }

    /// True when the running shell program has flipped to the xterm
    /// alternate screen buffer (`\e[?1049h`) — i.e., a full-screen TUI
    /// like vim / mnml / mixr / htop has taken over. tmnl uses this to
    /// drop the configured shell-inset to 0 while the TUI is active and
    /// restore it on exit.
    pub fn altscreen_active(&self) -> bool {
        match self.parser.lock() {
            Ok(p) => p.screen().alternate_screen(),
            Err(_) => false,
        }
    }

    /// Did new output arrive since the last `apply_to_grid`? Used to skip
    /// rebuilding instances when the shell's been idle.
    pub fn dirty(&self) -> bool {
        self.bytes_seen.load(Ordering::Relaxed) != self.last_applied_bytes
    }

    /// Copy parsed cells into `grid`. Returns the cursor position +
    /// visibility as the host expects.
    pub fn apply_to_grid(&mut self, grid: &mut Grid) -> (u16, u16, bool) {
        let p = self.parser.lock().unwrap();
        let screen = p.screen();
        let (srows, scols) = screen.size();
        let rows = (grid.rows.min(srows as u32)) as u16;
        let cols = (grid.cols.min(scols as u32)) as u16;

        for row in 0..rows {
            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let g = cell.contents();
                let ch = g.chars().next().unwrap_or(' ');
                let mut fg = vt_color_to_rgba(cell.fgcolor(), self.default_fg);
                let mut bg = vt_color_to_rgba(cell.bgcolor(), self.default_bg);
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let mut attrs = 0u32;
                if cell.bold() {
                    attrs |= ATTR_BOLD;
                }
                if cell.italic() {
                    attrs |= ATTR_ITALIC;
                }
                if cell.underline() {
                    attrs |= ATTR_UNDERLINE;
                }
                let dim_via_intensity = false; // vt100 doesn't expose `dim` explicitly
                if dim_via_intensity {
                    attrs |= ATTR_DIM;
                }
                let i = (row as u32 * grid.cols + col as u32) as usize;
                grid.cells[i] = crate::grid::Cell { ch, fg, bg, attrs };
            }
        }

        let (cr, cc) = screen.cursor_position();
        let visible = !screen.hide_cursor();
        self.last_applied_bytes = self.bytes_seen.load(Ordering::Relaxed);
        (cc, cr, visible)
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

fn vt_color_to_rgba(c: vt100::Color, default: [f32; 4]) -> [f32; 4] {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            1.0,
        ],
        vt100::Color::Idx(i) => ansi_color(i),
    }
}

fn ansi_color(i: u8) -> [f32; 4] {
    if i < 16 {
        let palette: [[u8; 3]; 16] = [
            [0x10, 0x11, 0x1c],
            [0xe0, 0x60, 0x60],
            [0x84, 0xc8, 0x6f],
            [0xee, 0xbb, 0x57],
            [0x6e, 0xa2, 0xe7],
            [0xc9, 0x7a, 0xea],
            [0x5f, 0xb3, 0xa1],
            [0xab, 0xb2, 0xbf],
            [0x42, 0x46, 0x4e],
            [0xff, 0x82, 0x82],
            [0xa6, 0xe2, 0x8c],
            [0xff, 0xd7, 0x71],
            [0x82, 0xb3, 0xff],
            [0xdc, 0xa5, 0xff],
            [0x84, 0xd6, 0xc5],
            [0xff, 0xff, 0xff],
        ];
        let [r, g, b] = palette[i as usize];
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    } else if i < 232 {
        let n = i - 16;
        let r = (n / 36) * 51;
        let g = ((n / 6) % 6) * 51;
        let b = (n % 6) * 51;
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    } else {
        let v = 8 + (i - 232) * 10;
        [v as f32 / 255.0, v as f32 / 255.0, v as f32 / 255.0, 1.0]
    }
}

/// Translate a winit logical key + modifier state into the bytes a terminal
/// expects to receive. Handles ASCII chars (with Ctrl/Alt encoding), the
/// named navigation keys (arrows, Home/End/PageUp/PageDown/Delete), and F1–F12.
pub fn winit_key_to_bytes(key: &Key, mods: ModifiersState) -> Option<Vec<u8>> {
    match key {
        Key::Character(s) => {
            let ch = s.chars().next()?;
            Some(encode_char(ch, mods))
        }
        Key::Named(n) => Some(encode_named(*n, mods)?),
        _ => None,
    }
}

fn encode_char(ch: char, mods: ModifiersState) -> Vec<u8> {
    if mods.control_key() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() {
            return vec![(lower as u8) & 0x1f];
        }
        if ch == ' ' || ch == '@' {
            return vec![0];
        }
        if ch == '[' {
            return vec![0x1b];
        }
        if ch == ']' {
            return vec![0x1d];
        }
        if ch == '\\' {
            return vec![0x1c];
        }
        if ch == '_' {
            return vec![0x1f];
        }
    }
    let mut buf = [0u8; 4];
    let utf = ch.encode_utf8(&mut buf).as_bytes().to_vec();
    if mods.alt_key() {
        let mut out = vec![0x1b];
        out.extend_from_slice(&utf);
        out
    } else {
        utf
    }
}

fn encode_named(n: NamedKey, _mods: ModifiersState) -> Option<Vec<u8>> {
    Some(match n {
        NamedKey::Enter => b"\r".to_vec(),
        NamedKey::Backspace => b"\x7f".to_vec(),
        NamedKey::Tab => b"\t".to_vec(),
        NamedKey::Escape => b"\x1b".to_vec(),
        NamedKey::Space => b" ".to_vec(),
        NamedKey::ArrowLeft => b"\x1b[D".to_vec(),
        NamedKey::ArrowRight => b"\x1b[C".to_vec(),
        NamedKey::ArrowUp => b"\x1b[A".to_vec(),
        NamedKey::ArrowDown => b"\x1b[B".to_vec(),
        NamedKey::Home => b"\x1b[H".to_vec(),
        NamedKey::End => b"\x1b[F".to_vec(),
        NamedKey::PageUp => b"\x1b[5~".to_vec(),
        NamedKey::PageDown => b"\x1b[6~".to_vec(),
        NamedKey::Delete => b"\x1b[3~".to_vec(),
        NamedKey::Insert => b"\x1b[2~".to_vec(),
        NamedKey::F1 => b"\x1bOP".to_vec(),
        NamedKey::F2 => b"\x1bOQ".to_vec(),
        NamedKey::F3 => b"\x1bOR".to_vec(),
        NamedKey::F4 => b"\x1bOS".to_vec(),
        NamedKey::F5 => b"\x1b[15~".to_vec(),
        NamedKey::F6 => b"\x1b[17~".to_vec(),
        NamedKey::F7 => b"\x1b[18~".to_vec(),
        NamedKey::F8 => b"\x1b[19~".to_vec(),
        NamedKey::F9 => b"\x1b[20~".to_vec(),
        NamedKey::F10 => b"\x1b[21~".to_vec(),
        NamedKey::F11 => b"\x1b[23~".to_vec(),
        NamedKey::F12 => b"\x1b[24~".to_vec(),
        _ => return None,
    })
}
