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
//! expect contains <text>   settle, then assert the grid contains <text>
//! expect lacks <text>      …assert it does not
//! fim           reconstruct the command line, run an AI completion,
//!               and dump the grid with the ghost suggestion overlaid
//! gen           treat the command line as a description, generate a
//!               shell command, preview it on the row below
//! scroll <n>    scroll the scrollback view by <n> rows (+ into
//!               history, - toward the bottom), then dump
//! click <c> <r> [button] [mods]
//!               synthesize a Down+Up mouse-press at cell (c, r).
//!               button: left|middle|right (default left); mods:
//!               comma-separated ctrl,alt,shift,super. Only fires
//!               if the pty child enabled mouse tracking (DECSET
//!               1000/1002/1006) — otherwise silently drops.
//! hover <c> <r> synthesize a Moved event (no button). Only fires
//!               under `?1003h` mouse tracking; dropped otherwise.
//! wheel <dy> <c> <r>
//!               synthesize |dy| ticks of wheel scroll. dy > 0 ⇒
//!               wheel up; dy < 0 ⇒ wheel down. Forwarded as xterm
//!               wheel events (button 64/65) when the child has
//!               tracking on.
//! quit          stop (input EOF also stops)
//! ```
//!
//! A failed `expect` dumps the rendered grid and makes the process exit
//! non-zero — so a piped script doubles as a pass/fail test.
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

/// Entry point for `tmnl --headless --app` (full App-driving mode).
/// Builds a real `App` via `App::new_headless` and dispatches stdin
/// commands through the same code paths the winit loop uses. Lets
/// agents test multi-tab + chrome-click + state-machine flows
/// without spinning up a window.
///
/// Commands (one per line):
///   tab.new       — spawn a fresh shell tab
///   tab.close     — close active tab (quits if last tab)
///   tab.next      — focus next tab
///   tab.prev      — focus previous tab
///   click <px> <py> [left|middle|right] [mods]
///                 — fire a synthetic Down+Up mouse-press at pixel
///                   coords. Routes through the same handler the
///                   winit loop uses (chip rects, palette rects,
///                   splitter regions all reachable).
///   hover <px> <py>
///                 — fire a synthetic Moved event (no button).
///   wheel <dy> <px> <py>
///                 — fire a synthetic wheel scroll. dy > 0 ⇒ up.
///   state-json    — dump tabs + focused + chip layout + sidebar + strip_h
///   quit          — stop the loop
///
/// Future: palette open, key dispatch, native-tab spawn.
pub fn run_app() {
    let cols: u32 = env_dim("TMNL_COLS", 120);
    let rows: u32 = env_dim("TMNL_ROWS", 36);
    // Approximate window pixels for the headless Gpu (cell_w ≈ 8,
    // cell_h ≈ 16 — atlas decides the real values). Width / height
    // seed `config` so chip layout + grid_dims math runs.
    let width_px = cols * 8;
    let height_px = rows * 16 + 60;
    let cfg = crate::config::Config::load();
    let mut app = match crate::App::new_headless(width_px, height_px, cfg.inset, cfg) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("tmnl --headless --app: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("tmnl --headless --app: ready ({cols}x{rows}, 1 tab, gpu=fallback-adapter)");

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim_end_matches('\r');
        let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
        match cmd {
            "" => {}
            "tab.new" => {
                app.new_shell_tab();
            }
            "tab.close" => {
                app.close_active_tab();
            }
            "tab.next" => {
                if !app.tabs.is_empty() {
                    let next = (app.active + 1) % app.tabs.len();
                    app.switch_to_tab(next);
                }
            }
            "tab.prev" => {
                if !app.tabs.is_empty() {
                    let prev = if app.active == 0 {
                        app.tabs.len() - 1
                    } else {
                        app.active - 1
                    };
                    app.switch_to_tab(prev);
                }
            }
            "click" => match parse_pixel_click_arg(arg) {
                Some((px, py, button, mods)) => app.synthetic_click(px, py, button, mods),
                None => eprintln!(
                    "tmnl --headless --app: usage: click <px> <py> [left|middle|right] [mods]"
                ),
            },
            "hover" => match parse_pixel_pair(arg) {
                Some((px, py)) => app.synthetic_hover(px, py),
                None => eprintln!("tmnl --headless --app: usage: hover <px> <py>"),
            },
            "wheel" => match parse_pixel_wheel_arg(arg) {
                Some((dy, px, py)) => app.synthetic_wheel(px, py, dy),
                None => eprintln!("tmnl --headless --app: usage: wheel <dy> <px> <py>"),
            },
            "state-json" => {
                println!("{}", app_state_json(&app));
            }
            "quit" => break,
            other => eprintln!("tmnl --headless --app: unknown command '{other}'"),
        }
        if app.should_quit {
            break;
        }
    }
}

/// Parse `<px> <py> [button] [mods]` for the App-headless `click`
/// command. Returns `(px, py, winit MouseButton, winit ModifiersState)`.
fn parse_pixel_click_arg(
    arg: &str,
) -> Option<(
    f64,
    f64,
    winit::event::MouseButton,
    winit::keyboard::ModifiersState,
)> {
    use winit::event::MouseButton;
    use winit::keyboard::ModifiersState;
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let px: f64 = parts[0].parse().ok()?;
    let py: f64 = parts[1].parse().ok()?;
    let button = parts
        .get(2)
        .map(|s| s.to_ascii_lowercase())
        .map(|s| match s.as_str() {
            "middle" | "m" => MouseButton::Middle,
            "right" | "r" => MouseButton::Right,
            _ => MouseButton::Left,
        })
        .unwrap_or(MouseButton::Left);
    let mods = parts
        .get(3)
        .map(|s| parse_winit_mods(s))
        .unwrap_or(ModifiersState::empty());
    Some((px, py, button, mods))
}

fn parse_pixel_pair(arg: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
}

fn parse_pixel_wheel_arg(arg: &str) -> Option<(f32, f64, f64)> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let dy: f32 = parts[0].parse().ok()?;
    let px: f64 = parts[1].parse().ok()?;
    let py: f64 = parts[2].parse().ok()?;
    Some((dy, px, py))
}

/// Parse winit `ModifiersState` from `"ctrl,alt,shift,super"`.
fn parse_winit_mods(s: &str) -> winit::keyboard::ModifiersState {
    use winit::keyboard::ModifiersState;
    let mut out = ModifiersState::empty();
    for token in s.split(',') {
        match token.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => out |= ModifiersState::CONTROL,
            "alt" | "option" => out |= ModifiersState::ALT,
            "shift" => out |= ModifiersState::SHIFT,
            "super" | "cmd" | "meta" => out |= ModifiersState::SUPER,
            _ => {}
        }
    }
    out
}

/// Format the App's headline state as a single-line JSON object.
/// Schema (stable across versions; new fields are append-only):
///   {"tabs": <count>, "active": <idx>, "panes": [<panes-in-active-tab>],
///    "should_quit": bool, "tab_layout": "horizontal"|"vertical",
///    "altscreen": bool}
fn app_state_json(app: &crate::App) -> String {
    let active_tab = &app.tabs[app.active.min(app.tabs.len().saturating_sub(1))];
    let panes: Vec<&str> = active_tab
        .panes
        .iter()
        .map(|p| match &p.kind {
            crate::PaneKind::Shell { .. } => "Shell",
            crate::PaneKind::Native { .. } => "Native",
            crate::PaneKind::Browser { .. } => "Browser",
        })
        .collect();
    let panes_json = panes
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(",");
    let tab_layout = match app.cfg.tab_layout {
        crate::config::TabLayout::Horizontal => "horizontal",
        crate::config::TabLayout::Vertical => "vertical",
    };
    format!(
        r#"{{"tabs":{tabs},"active":{active},"panes":[{panes_json}],"should_quit":{should_quit},"tab_layout":"{tab_layout}","altscreen":{altscreen}}}"#,
        tabs = app.tabs.len(),
        active = app.active,
        should_quit = app.should_quit,
        altscreen = app.altscreen_active,
    )
}

/// Entry point for `tmnl --headless`. Runs until stdin EOF or `quit`.
pub fn run() {
    let cols: u32 = env_dim("TMNL_COLS", 80);
    let rows: u32 = env_dim("TMNL_ROWS", 24);

    let mut session = match ShellSession::spawn(
        rows as u16,
        cols as u16,
        crate::palette().text_fg,
        crate::palette().clear_bg,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("tmnl --headless: failed to start shell: {e}");
            std::process::exit(1);
        }
    };
    let mut grid = Grid::new(cols, rows, crate::palette().clear_bg);
    // Spawned lazily on the first `fim` command.
    let mut fim: Option<crate::fim::FimWorker> = None;

    // Let the shell load its rc files and print the first prompt.
    settle(&mut session, &mut grid);
    eprintln!("tmnl --headless: shell ready ({cols}x{rows})");

    // Count of failed `expect` checks — the process exits non-zero when
    // any failed, so a piped script works as a pass/fail test.
    let mut failures: usize = 0;
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
            "expect" => {
                settle(&mut session, &mut grid);
                let _ = session.apply_to_grid(&mut grid);
                if !run_expect(&grid, arg) {
                    failures += 1;
                }
            }
            "fim" => run_fim(&mut session, &mut grid, &mut fim),
            "gen" => run_gen(&mut session, &mut grid, &mut fim),
            "scroll" => {
                session.scroll(arg.parse().unwrap_or(0));
                print_dump(&mut session, &mut grid);
            }
            // `click <col> <row> [button] [mods]` — fires a Down+Up
            // mouse-press at the given cell coords through
            // `ShellSession::write_mouse`. Honors the pty child's
            // current vt100 mouse-protocol mode: drops the event
            // silently when the child hasn't enabled tracking, so a
            // bare shell prompt doesn't get garbage on stdin.
            //
            // `button` defaults to `left`; accepts `middle`/`right`
            // or `m`/`r`. `mods` is a comma-separated list of
            // `ctrl`/`alt`/`shift`/`super`. Examples:
            //   click 3 5
            //   click 10 12 right
            //   click 0 0 left ctrl,shift
            "click" => match parse_mouse_arg(arg) {
                Some((col, row, button, mods)) => {
                    session.write_mouse(col, row, button, true, mods);
                    session.write_mouse(col, row, button, false, mods);
                }
                None => eprintln!(
                    "tmnl --headless: usage: click <col> <row> [left|middle|right] [mods]"
                ),
            },
            // `hover <col> <row>` — Moved event (no button held).
            // Forwarded via `write_mouse_motion`; honors
            // `MouseProtocolMode::AnyMotion` (DECSET ?1003h). Drops
            // when the child hasn't requested motion tracking.
            "hover" => match parse_hover_arg(arg) {
                Some((col, row)) => {
                    session.write_mouse_motion(col, row, None, 0);
                }
                None => eprintln!("tmnl --headless: usage: hover <col> <row>"),
            },
            // `wheel <dy> <col> <row>` — `|dy|` ticks of wheel scroll
            // at the given cell. Positive `dy` ⇒ wheel up, negative
            // ⇒ wheel down. Forwarded as xterm wheel events (button
            // 64/65) via `write_mouse`; the body terminal's pty
            // child sees them only if it requested mouse tracking.
            // Note: in interactive mode this also routes through the
            // alt-screen check in `handle_mouse_wheel`; headless
            // doesn't have that distinction so we always forward.
            "wheel" => match parse_wheel_arg(arg) {
                Some((dy, col, row)) => {
                    const BUTTON_WHEEL_UP: u8 = 4;
                    const BUTTON_WHEEL_DOWN: u8 = 5;
                    let button = if dy > 0 {
                        BUTTON_WHEEL_UP
                    } else {
                        BUTTON_WHEEL_DOWN
                    };
                    for _ in 0..dy.unsigned_abs() {
                        session.write_mouse(col, row, button, true, 0);
                    }
                }
                None => eprintln!("tmnl --headless: usage: wheel <dy> <col> <row>"),
            },
            // `state-json` — dump session state as a single JSON line
            // for scripted assertion. Useful to verify pre-conditions
            // before sending a `click`/`hover`/`wheel` (was the child's
            // DECSET 1006 actually picked up?) + as oracle for the
            // post-condition (did the click reach where it should?).
            //
            // Schema (all fields always present):
            //   { "shell": "<shell-basename>", "title": "<osc>", ...
            //     "altscreen": true|false,
            //     "exited": true|false, "scrollback": <int>,
            //     "cursor": [<row>, <col>],
            //     "mouse_mode": "None|Press|PressRelease|ButtonMotion|AnyMotion",
            //     "mouse_encoding": "Default|Utf8|Sgr|Urxvt",
            //     "integration": {"active": ..., "running": ...} }
            "state-json" => {
                settle(&mut session, &mut grid);
                let _ = session.apply_to_grid(&mut grid);
                println!("{}", session_state_json(&session));
            }
            "quit" => break,
            other => eprintln!("tmnl --headless: unknown command '{other}'"),
        }
        if session.exited() {
            eprintln!("tmnl --headless: shell exited");
            break;
        }
    }
    if failures > 0 {
        eprintln!("tmnl --headless: {failures} expectation(s) FAILED");
        std::process::exit(1);
    }
}

/// Format the current shell session state as a single-line JSON
/// object for the `state-json` headless command. Manually-formatted
/// instead of `serde_json` so we don't pull in another dep just for
/// this — the schema is small + stable.
fn session_state_json(session: &ShellSession) -> String {
    use vt100::{MouseProtocolEncoding, MouseProtocolMode};
    let (mode, encoding) = session
        .mouse_protocol_state()
        .unwrap_or((MouseProtocolMode::None, MouseProtocolEncoding::default()));
    let mode_str = match mode {
        MouseProtocolMode::None => "None",
        MouseProtocolMode::Press => "Press",
        MouseProtocolMode::PressRelease => "PressRelease",
        MouseProtocolMode::ButtonMotion => "ButtonMotion",
        MouseProtocolMode::AnyMotion => "AnyMotion",
    };
    let encoding_str = match encoding {
        MouseProtocolEncoding::Default => "Default",
        MouseProtocolEncoding::Utf8 => "Utf8",
        MouseProtocolEncoding::Sgr => "Sgr",
    };
    let (cursor_row, cursor_col) = session.cursor_position();
    // Escape just the two things that can break a JSON string: `"` and `\`.
    let escape = |s: &str| -> String {
        let mut out = String::with_capacity(s.len() + 2);
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out
    };
    format!(
        concat!(
            r#"{{"shell":"{shell}","title":"{title}","altscreen":{altscreen},"#,
            r#""exited":{exited},"scrollback":{scrollback},"#,
            r#""cursor":[{cursor_row},{cursor_col}],"#,
            r#""mouse_mode":"{mode}","mouse_encoding":"{encoding}","#,
            r#""integration":{{"active":{integ_active},"running":{integ_running}}}}}"#
        ),
        shell = escape(session.shell_name()),
        title = escape(&session.osc_title()),
        altscreen = session.altscreen_active(),
        exited = session.exited(),
        scrollback = session.scrollback_offset(),
        cursor_row = cursor_row,
        cursor_col = cursor_col,
        mode = mode_str,
        encoding = encoding_str,
        integ_active = session.shell_integration_active(),
        integ_running = session.command_running(),
    )
}

/// Parse `<col> <row> [button] [mods]` into the tuple `write_mouse`
/// expects. tmnl-protocol button values: LEFT=0, RIGHT=1, MIDDLE=2.
/// mod bits: shift=1, ctrl=2, alt=4, super=8.
fn parse_mouse_arg(arg: &str) -> Option<(u16, u16, u8, u8)> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let col: u16 = parts[0].parse().ok()?;
    let row: u16 = parts[1].parse().ok()?;
    let button: u8 = parts
        .get(2)
        .map(|s| s.to_ascii_lowercase())
        .map(|s| match s.as_str() {
            "middle" | "m" => 2,
            "right" | "r" => 1,
            _ => 0,
        })
        .unwrap_or(0);
    let mods: u8 = parts.get(3).map(|s| parse_mods(s)).unwrap_or(0);
    Some((col, row, button, mods))
}

fn parse_hover_arg(arg: &str) -> Option<(u16, u16)> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let col: u16 = parts[0].parse().ok()?;
    let row: u16 = parts[1].parse().ok()?;
    Some((col, row))
}

fn parse_wheel_arg(arg: &str) -> Option<(i32, u16, u16)> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let dy: i32 = parts[0].parse().ok()?;
    let col: u16 = parts[1].parse().ok()?;
    let row: u16 = parts[2].parse().ok()?;
    Some((dy, col, row))
}

/// Parse a comma-separated `ctrl,alt,shift,super` string into the
/// tmnl-protocol mod bitmask (shift=1, ctrl=2, alt=4, super=8).
/// Unknown tokens are silently ignored.
fn parse_mods(s: &str) -> u8 {
    let mut out = 0u8;
    for token in s.split(',') {
        match token.trim().to_ascii_lowercase().as_str() {
            "shift" => out |= 1,
            "ctrl" | "control" => out |= 2,
            "alt" | "option" => out |= 4,
            "super" | "cmd" | "meta" => out |= 8,
            _ => {}
        }
    }
    out
}

/// Run an `expect contains|lacks <text>` check against the grid. Prints
/// `ok` / `FAIL` and, on failure, dumps the rendered grid. Returns
/// whether the check passed.
fn run_expect(grid: &Grid, arg: &str) -> bool {
    let (op, text) = arg.split_once(' ').unwrap_or((arg, ""));
    let screen = grid_text(grid);
    let pass = match op {
        "contains" => screen.contains(text),
        "lacks" => !screen.contains(text),
        _ => {
            eprintln!("tmnl --headless: expect <contains|lacks> <text>");
            return false;
        }
    };
    if pass {
        println!("ok: expect {op} {text:?}");
    } else {
        println!("FAIL: expect {op} {text:?}");
        dump_grid(grid, "expectation failed");
    }
    pass
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

/// Flatten the grid into a newline-joined string (rows right-trimmed) —
/// the form `expect` substring-checks against and `dump_grid` prints.
fn grid_text(grid: &Grid) -> String {
    let mut out = String::with_capacity((grid.cols + 1) as usize * grid.rows as usize);
    for row in 0..grid.rows {
        let mut line = String::with_capacity(grid.cols as usize);
        for col in 0..grid.cols {
            line.push(grid.cells[(row * grid.cols + col) as usize].ch);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
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
    let _ = write!(out, "{}", grid_text(grid));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_arg_defaults_to_left_with_no_mods() {
        assert_eq!(parse_mouse_arg("3 5"), Some((3, 5, 0, 0)));
    }

    #[test]
    fn click_arg_accepts_middle_right_and_short_aliases() {
        assert_eq!(parse_mouse_arg("0 0 middle"), Some((0, 0, 2, 0)));
        assert_eq!(parse_mouse_arg("0 0 m"), Some((0, 0, 2, 0)));
        assert_eq!(parse_mouse_arg("0 0 right"), Some((0, 0, 1, 0)));
        assert_eq!(parse_mouse_arg("0 0 r"), Some((0, 0, 1, 0)));
    }

    #[test]
    fn click_arg_parses_mods() {
        assert_eq!(parse_mouse_arg("1 2 left ctrl"), Some((1, 2, 0, 2)));
        assert_eq!(
            parse_mouse_arg("1 2 left ctrl,shift,alt"),
            Some((1, 2, 0, 7))
        );
        // Unknown bits silently dropped.
        assert_eq!(parse_mouse_arg("1 2 left ctrl,hyper"), Some((1, 2, 0, 2)));
        // Aliases.
        assert_eq!(parse_mouse_arg("1 2 left cmd"), Some((1, 2, 0, 8)));
        assert_eq!(parse_mouse_arg("1 2 left option"), Some((1, 2, 0, 4)));
    }

    #[test]
    fn click_arg_rejects_missing_coords() {
        assert_eq!(parse_mouse_arg(""), None);
        assert_eq!(parse_mouse_arg("3"), None);
        assert_eq!(parse_mouse_arg("a b"), None);
    }

    #[test]
    fn hover_arg_requires_two_ints() {
        assert_eq!(parse_hover_arg("4 6"), Some((4, 6)));
        assert_eq!(parse_hover_arg("4"), None);
        assert_eq!(parse_hover_arg(""), None);
    }

    #[test]
    fn wheel_arg_requires_dy_col_row() {
        assert_eq!(parse_wheel_arg("2 5 10"), Some((2, 5, 10)));
        assert_eq!(parse_wheel_arg("-3 0 0"), Some((-3, 0, 0)));
        assert_eq!(parse_wheel_arg("1 2"), None);
        assert_eq!(parse_wheel_arg("a b c"), None);
    }
}
