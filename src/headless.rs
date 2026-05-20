//! Headless mode — run a shell session with no window, for scripted
//! verification. `tmnl --headless` reads commands from stdin and dumps
//! the rendered cell `Grid` as text, so tests (and an agent) can "see"
//! what tmnl renders without a GPU surface. Mirrors mnml's `--headless`
//! smoke harness.
//!
//! It branches out of `main` before any winit / wgpu / AppKit setup, so
//! it is safe to run off a terminal with no display.
//!
//! Commands — one per line on stdin:
//!
//! ```text
//! type <text>   write literal text to the shell
//! key  <name>   send a named key: enter tab esc backspace space
//!               up down left right home end
//! wait <ms>     sleep for <ms> milliseconds
//! dump          settle pending output, then print the grid to stdout
//! fim           reconstruct the command line, run an AI completion,
//!               and dump the grid with the ghost suggestion overlaid
//! gen           treat the command line as a description, generate a
//!               shell command, preview it on the row below
//! scroll <n>    scroll the scrollback view by <n> rows (+ into
//!               history, - toward the bottom), then dump
//! quit          stop (input EOF also stops)
//! ```
//!
//! Grid size defaults to 80x24; override with `TMNL_COLS` / `TMNL_ROWS`.
//!
//! Example:
//!
//! ```text
//! printf 'type echo hi\nkey enter\ndump\nquit\n' | tmnl --headless
//! ```

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use crate::grid::Grid;
use crate::shell::ShellSession;

/// Entry point for `tmnl --headless`. Runs until stdin EOF or `quit`.
pub fn run() {
    let cols: u32 = env_dim("TMNL_COLS", 80);
    let rows: u32 = env_dim("TMNL_ROWS", 24);

    let mut session =
        match ShellSession::spawn(rows as u16, cols as u16, crate::TEXT_FG, crate::CLEAR_BG) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tmnl --headless: failed to start shell: {e}");
                std::process::exit(1);
            }
        };
    let mut grid = Grid::new(cols, rows, crate::CLEAR_BG);
    // Spawned lazily on the first `fim` command.
    let mut fim: Option<crate::fim::FimWorker> = None;

    // Let the shell load its rc files and print the first prompt.
    settle(&mut session, &mut grid);
    eprintln!("tmnl --headless: shell ready ({cols}x{rows})");

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim_end_matches('\r');
        let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
        match cmd {
            "" => {}
            "type" => session.write_bytes(arg.as_bytes()),
            "key" => match key_bytes(arg) {
                Some(bytes) => session.write_bytes(&bytes),
                None => eprintln!("tmnl --headless: unknown key '{arg}'"),
            },
            "wait" => {
                let ms: u64 = arg.parse().unwrap_or(0);
                std::thread::sleep(Duration::from_millis(ms));
            }
            "dump" => {
                settle(&mut session, &mut grid);
                print_dump(&mut session, &mut grid);
            }
            "fim" => run_fim(&mut session, &mut grid, &mut fim),
            "gen" => run_gen(&mut session, &mut grid, &mut fim),
            "scroll" => {
                session.scroll(arg.parse().unwrap_or(0));
                print_dump(&mut session, &mut grid);
            }
            "quit" => break,
            other => eprintln!("tmnl --headless: unknown command '{other}'"),
        }
        if session.exited() {
            eprintln!("tmnl --headless: shell exited");
            break;
        }
    }
}

fn env_dim(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// Apply pending shell output to the grid, waiting until output settles.
/// A 150 ms head start lets freshly-triggered output begin arriving;
/// the loop then drains until the byte stream goes quiet (capped at 2 s).
fn settle(session: &mut ShellSession, grid: &mut Grid) {
    std::thread::sleep(Duration::from_millis(150));
    let start = Instant::now();
    loop {
        let _ = session.apply_to_grid(grid);
        std::thread::sleep(Duration::from_millis(100));
        if !session.dirty() || start.elapsed() > Duration::from_millis(2000) {
            break;
        }
    }
    let _ = session.apply_to_grid(grid);
}

/// Re-apply the shell screen, then dump the grid as text.
fn print_dump(session: &mut ShellSession, grid: &mut Grid) {
    let (cc, cr, vis) = session.apply_to_grid(grid);
    let header = format!(
        "size: {}x{}  cursor: ({cr},{cc}) visible={vis}\n\
         integration: active={} running={}  scrollback={}  title={:?}",
        grid.cols,
        grid.rows,
        session.shell_integration_active(),
        session.command_running(),
        session.scrollback_offset(),
        session.osc_title(),
    );
    dump_grid(grid, &header);
}

/// Print the grid to stdout as plain text under `header` — one line per
/// row. Does not re-apply the shell screen, so an overlay already drawn
/// onto `grid` (e.g. the AI ghost suggestion) survives into the dump.
fn dump_grid(grid: &Grid, header: &str) {
    let out = std::io::stdout();
    let mut out = out.lock();
    let _ = writeln!(out, "=== tmnl headless dump ===");
    let _ = writeln!(out, "{header}");
    let _ = writeln!(out, "--- screen ---");
    for row in 0..grid.rows {
        let mut line = String::with_capacity(grid.cols as usize);
        for col in 0..grid.cols {
            line.push(grid.cells[(row * grid.cols + col) as usize].ch);
        }
        let _ = writeln!(out, "{}", line.trim_end());
    }
    let _ = writeln!(out, "=== end dump ===");
    let _ = out.flush();
}

/// Send one completion request and block for the reply (the first call
/// also loads the model — allow generous time). Returns the first line
/// of the suggestion.
fn complete_blocking(
    worker: &crate::fim::FimWorker,
    prefix: &str,
    suffix: &str,
) -> Result<String, String> {
    worker.request(0, prefix, suffix);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        for (id, result) in worker.poll() {
            if id == crate::fim::STATUS_ID {
                match result {
                    Ok(m) => eprintln!("fim: {m}"),
                    Err(e) => return Err(e),
                }
                continue;
            }
            return result.map(|t| t.lines().next().unwrap_or("").trim_end().to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("timed out waiting for completion".to_string())
}

/// `fim` — reconstruct the current command line, run an AI continuation,
/// and dump the grid with the ghost suggestion overlaid. Mirrors
/// `App::trigger_ai_completion`. Needs the OSC 133 snippet sourced first.
fn run_fim(session: &mut ShellSession, grid: &mut Grid, fim: &mut Option<crate::fim::FimWorker>) {
    settle(session, grid);
    let (cursor_col, cursor_row, _) = session.apply_to_grid(grid);
    let Some(prefix) = session.current_command_line(grid, cursor_row, cursor_col) else {
        println!("fim: no command-line anchor (OSC 133 integration not active)");
        return;
    };
    if prefix.trim().is_empty() {
        println!("fim: command line is empty — nothing to complete");
        return;
    }
    let worker = fim.get_or_insert_with(crate::fim::FimWorker::spawn);
    match complete_blocking(worker, &prefix, "") {
        Ok(s) => {
            println!("fim: prefix={prefix:?} suggestion={s:?}");
            let idx = cursor_row as usize * grid.cols as usize + cursor_col as usize;
            crate::draw_ghost(grid, idx, &s);
            crate::draw_ghost(grid, idx + s.chars().count() + 2, "[tab]");
            dump_grid(grid, &format!("fim ghost overlay — prefix={prefix:?}"));
        }
        Err(e) => println!("fim: {e}"),
    }
}

/// `gen` — treat the current command line as a natural-language
/// description and generate a shell command for it (Stage 2,
/// NL→command). Wraps the description in a shell-script-shaped FIM
/// prompt so the code model fills in a shell command.
fn run_gen(session: &mut ShellSession, grid: &mut Grid, fim: &mut Option<crate::fim::FimWorker>) {
    settle(session, grid);
    let (cursor_col, cursor_row, _) = session.apply_to_grid(grid);
    let Some(desc) = session.current_command_line(grid, cursor_row, cursor_col) else {
        println!("gen: no command-line anchor (OSC 133 integration not active)");
        return;
    };
    let desc = desc.trim();
    if desc.is_empty() {
        println!("gen: describe a command on the prompt first");
        return;
    }
    let anchor_col = session.input_anchor().map_or(0, |(_, c)| c);
    let worker = fim.get_or_insert_with(crate::fim::FimWorker::spawn);
    // A shebang + comment biases the code model toward a zsh one-liner.
    let prefix = format!("#!/bin/zsh\n# {desc}\n");
    match complete_blocking(worker, &prefix, "\n") {
        Ok(s) => {
            println!("gen: desc={desc:?} command={s:?}");
            // Preview on the row below, under the input start column —
            // the same placement as the App's ⌘K ghost.
            let idx = (cursor_row as usize + 1) * grid.cols as usize + anchor_col as usize;
            crate::draw_ghost(grid, idx, &s);
            crate::draw_ghost(grid, idx + s.chars().count() + 2, "[tab]");
            dump_grid(grid, &format!("gen preview (row below) — desc={desc:?}"));
        }
        Err(e) => println!("gen: {e}"),
    }
}

/// Bytes a terminal expects for a named key. Mirrors `encode_named` in
/// `shell.rs` for the subset the headless harness needs.
fn key_bytes(name: &str) -> Option<Vec<u8>> {
    Some(match name {
        "enter" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "esc" => b"\x1b".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "space" => b" ".to_vec(),
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        _ => return None,
    })
}
