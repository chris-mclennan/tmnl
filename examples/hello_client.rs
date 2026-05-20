//! hello_client — the minimal tmnl backing app.
//!
//! A "backing app" renders into tmnl's window without speaking ANSI: it
//! connects over a Unix socket, sends `Frame`s of typed cells, and
//! receives `Input` / `Resize` events back. This example is an echo
//! demo — type and the text appears; Enter clears; Esc quits.
//!
//! Run it against a real tmnl, or against the `fake_server` stub:
//!
//!   $ cargo run --example fake_server  -- /tmp/dev.sock   # terminal A
//!   $ cargo run --example hello_client -- /tmp/dev.sock   # terminal B
//!
//! Copy this file as the starting point for your own client. The full
//! protocol walkthrough is in `docs/sdk-guide.md`.

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use tmnl_protocol::{
    DiffRun, Frame, InputEvent, KeyCode, Message, PROTOCOL_VERSION, WireCell, pack_rgba,
    read_message, write_message,
};

fn main() -> std::io::Result<()> {
    let socket = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: hello_client <socket-path>  (the path tmnl prints on startup)");

    // ── 1. Connect and handshake ────────────────────────────────────
    // tmnl is the server (it bound the socket); we connect to it.
    let stream = UnixStream::connect(&socket)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    write_message(
        &mut writer,
        &Message::Hello {
            version: PROTOCOL_VERSION,
        },
    )?;
    // Optional, but without it the tab falls back to a default label.
    write_message(&mut writer, &Message::Title("hello".to_string()))?;

    // ── 2. Wait for the first Resize so we know the grid size ───────
    // Don't render before this — we wouldn't know cols/rows.
    let (mut cols, mut rows) = loop {
        match read_message(&mut reader)? {
            Message::Hello { version } => eprintln!("connected — tmnl protocol v{version}"),
            Message::Resize(r) => break (r.cols, r.rows),
            Message::Quit => return Ok(()),
            _ => {}
        }
    };

    // ── 3. App state — just the text typed so far ───────────────────
    let mut typed = String::new();
    let mut seq: u64 = 0;
    render(&mut writer, cols, rows, &typed, &mut seq)?;

    // ── 4. Event loop — react to input, then redraw ─────────────────
    // The loop ends when read_message errors (tmnl disconnected).
    while let Ok(msg) = read_message(&mut reader) {
        match msg {
            Message::Resize(r) => {
                cols = r.cols;
                rows = r.rows;
            }
            Message::Input(InputEvent::Key(k)) if k.press => match k.code {
                KeyCode::Esc => break,
                KeyCode::Enter => typed.clear(),
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Char(c) => typed.push(c),
                _ => {}
            },
            Message::Quit => break,
            // Key-up, mouse, etc. — nothing changed, skip the redraw.
            _ => continue,
        }
        render(&mut writer, cols, rows, &typed, &mut seq)?;
    }

    Ok(())
}

/// Draw one full frame: a centered title, the typed text, and a hint.
///
/// This sends the whole grid as a single run every time. That's the
/// simplest correct approach; once it works you can switch to partial
/// `DiffRun`s covering only the rows that changed.
fn render(
    w: &mut UnixStream,
    cols: u16,
    rows: u16,
    typed: &str,
    seq: &mut u64,
) -> std::io::Result<()> {
    let bg = pack_rgba(0.06, 0.07, 0.09, 1.0);
    let fg = pack_rgba(0.90, 0.91, 0.93, 1.0);
    let dim = pack_rgba(0.45, 0.47, 0.52, 1.0);

    let (w_cells, h_cells) = (cols as usize, rows as usize);

    // Begin with a grid of blank, background-colored cells.
    let blank = WireCell {
        ch: ' ' as u32,
        fg: dim,
        bg,
        attrs: 0,
    };
    let mut cells = vec![blank; w_cells * h_cells];

    let title = "hello from a tmnl backing app";
    let hint = "type something  ·  Enter clears  ·  Esc quits";
    let prompt = format!("> {typed}");

    let center = |s: &str| (w_cells.saturating_sub(s.chars().count())) / 2;
    let mid = h_cells / 2;

    put(
        &mut cells,
        w_cells,
        h_cells,
        mid.saturating_sub(2),
        center(title),
        title,
        fg,
        bg,
    );
    put(&mut cells, w_cells, h_cells, mid, 2, &prompt, fg, bg);
    put(
        &mut cells,
        w_cells,
        h_cells,
        h_cells.saturating_sub(2),
        center(hint),
        hint,
        dim,
        bg,
    );

    let frame = Frame {
        seq: *seq,
        cols,
        rows,
        // Park the cursor just after the typed text.
        cursor_col: (2 + prompt.chars().count()).min(w_cells.saturating_sub(1)) as u16,
        cursor_row: mid as u16,
        cursor_shape: 0,
        cursor_visible: 1,
        runs: vec![DiffRun { start: 0, cells }],
    };
    *seq += 1;
    write_message(w, &Message::Frame(frame))
}

/// Write `s` into `cells` starting at `(col, row)`, clipping at the grid
/// edges. The grid is row-major: index = row * cols + col.
#[allow(clippy::too_many_arguments)]
fn put(
    cells: &mut [WireCell],
    cols: usize,
    rows: usize,
    row: usize,
    col: usize,
    s: &str,
    fg: u32,
    bg: u32,
) {
    if row >= rows {
        return;
    }
    for (i, ch) in s.chars().enumerate() {
        let c = col + i;
        if c >= cols {
            break;
        }
        cells[row * cols + c] = WireCell {
            ch: ch as u32,
            fg,
            bg,
            attrs: 0,
        };
    }
}
