//! Shell mode — tmnl hosts a real pty and parses its output via the `vt100`
//! crate into our cell `Grid`. The companion to native mode: identical
//! renderer downstream, totally different source of cells.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, OwnedFd, RawFd};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::grid::Grid;
use crate::osc133;

const SCROLLBACK: usize = 4000;

const ATTR_BOLD: u32 = 1 << 0;
const ATTR_DIM: u32 = 1 << 1;
const ATTR_ITALIC: u32 = 1 << 2;
const ATTR_UNDERLINE: u32 = 1 << 3;

/// vt100 0.16 delivers the OSC window title through a [`vt100::Callbacks`]
/// implementation rather than storing it on `Screen`. This sink keeps
/// the latest title so `ShellSession::osc_title` can read it back.
#[derive(Default)]
struct TitleSink {
    title: String,
}

impl vt100::Callbacks for TitleSink {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }
}

pub struct ShellSession {
    parser: Arc<Mutex<vt100::Parser<TitleSink>>>,
    writer: Box<dyn Write + Send>,
    /// Cross-platform pty master — `Some` for sessions we spawned
    /// ourselves via portable-pty. `None` for sessions whose master fd
    /// we adopted via the pty-fd handoff (the original spawner's
    /// process owns the portable_pty::MasterPty; we just hold a dup
    /// of its raw fd in `adopted_fd`).
    master: Option<Box<dyn MasterPty + Send>>,
    /// Adopted pty master file descriptor — only `Some` when this
    /// session was constructed via [`Self::adopt_fd`]. We keep it as
    /// an [`std::fs::File`] so [`Drop`] on `ShellSession` closes the
    /// duplicated fd. Resize goes through `ioctl(TIOCSWINSZ)` on this
    /// instead of `MasterPty::resize` (we don't own a MasterPty).
    #[cfg(unix)]
    adopted_fd: Option<std::fs::File>,
    reader: Option<JoinHandle<()>>,
    /// `Some` for spawn-path sessions whose child we own. `None` for
    /// adopted sessions — the child still belongs to the sender's
    /// process group. [`Drop`] only kills the child for owned ones.
    child: Option<Box<dyn Child + Send + Sync>>,
    exited: Arc<Mutex<bool>>,
    last_size: (u16, u16),
    bytes_seen: Arc<AtomicU64>,
    last_applied_bytes: u64,
    default_bg: [f32; 4],
    default_fg: [f32; 4],
    /// Set by the reader thread when an OSC 1337 sequence is seen in
    /// the pty stream (iTerm2 convention for "process needs attention").
    /// Claude Code emits this when a turn finishes and it's waiting on
    /// the user. `take_attention()` reads + clears.
    attention_requested: Arc<AtomicBool>,
    /// Basename of the shell we launched (`zsh` / `bash` / `fish` / …)
    /// — used as the tab label fallback when no OSC title has been set
    /// by anything running in the shell. Matches Terminal.app / iTerm2
    /// / Kitty convention.
    shell_name: String,
    /// Cached foreground-process name (e.g. `vim`, `htop`, `less`) —
    /// refreshed at most every `FG_POLL_INTERVAL` to keep the `ps`
    /// invocation cheap. `None` ⇒ no foreground process distinct from
    /// the shell. Used by the tab-label fallback chain between OSC
    /// title and shell_name.
    fg_proc_cache: Option<String>,
    last_fg_poll: Option<std::time::Instant>,
    /// Live OSC 133 shell-integration state — whether the integration
    /// snippet is installed and whether a command is currently running.
    /// Updated by the reader thread; read by the render thread.
    integration: Arc<Mutex<osc133::State>>,
    /// Set when the scrollback view changes (mouse wheel / keys) so the
    /// next `apply_to_grid` re-renders even with no new pty output.
    scroll_dirty: bool,
}

/// How often to poll `ps` for the shell's foreground process. Shorter
/// → more responsive label updates; longer → fewer fork+exec spikes
/// when many shell tabs are open.
const FG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

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
        let shell_name = std::path::Path::new(&shell)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("shell")
            .to_string();
        let mut cmd = CommandBuilder::new(&shell);
        cmd.env("TERM", "xterm-256color");
        // Identify ourselves the way Terminal.app / iTerm2 do, so shell
        // integration snippets and other tools can detect tmnl.
        cmd.env("TERM_PROGRAM", "tmnl");
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

        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK,
            TitleSink::default(),
        )));
        let exited = Arc::new(Mutex::new(false));
        let bytes_seen = Arc::new(AtomicU64::new(0));

        let reader_handle = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("clone pty reader: {e}"))?;
        let attention_requested = Arc::new(AtomicBool::new(false));
        let integration = Arc::new(Mutex::new(osc133::State::default()));
        let reader = spawn_reader_thread(
            reader_handle,
            "tmnl-shell-reader",
            Arc::clone(&parser),
            Arc::clone(&exited),
            Arc::clone(&bytes_seen),
            Arc::clone(&attention_requested),
            Arc::clone(&integration),
        )?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("take pty writer: {e}"))?;

        Ok(ShellSession {
            parser,
            writer,
            master: Some(pair.master),
            #[cfg(unix)]
            adopted_fd: None,
            reader: Some(reader),
            child: Some(child),
            exited,
            last_size: (rows, cols),
            bytes_seen,
            last_applied_bytes: 0,
            default_bg,
            default_fg,
            attention_requested,
            integration,
            scroll_dirty: false,
            shell_name,
            fg_proc_cache: None,
            last_fg_poll: None,
        })
    }

    /// Construct a `ShellSession` that wraps an already-open pty
    /// master file descriptor (e.g. one transferred via SCM_RIGHTS
    /// from mnml's pty-pane handoff). The fd's ownership transfers
    /// to the returned session — its [`Drop`] closes the file.
    ///
    /// Unlike [`Self::spawn`], the caller does not own the spawned
    /// child process — that lives in whichever process originally
    /// opened the pty. tmnl just reads/writes to the master + treats
    /// EOF as the session ending.
    ///
    /// `label` is used as the tab name (typically the basename of
    /// the original program, e.g. `claude` / `htop` / `vim`). Same
    /// fallback chain as the spawn path: OSC title → fg-proc-name →
    /// this label.
    #[cfg(unix)]
    pub fn adopt_fd(
        fd: OwnedFd,
        rows: u16,
        cols: u16,
        default_fg: [f32; 4],
        default_bg: [f32; 4],
        label: &str,
    ) -> Result<Self, String> {
        let (rows, cols) = (rows.max(4), cols.max(20));
        // Wrap as `std::fs::File` — both Read + Write + Send, and its
        // Drop closes the fd. Resize is via ioctl on this fd, not
        // through portable-pty (which doesn't expose adopt-fd).
        let file: std::fs::File = fd.into();
        // Make sure the adopted pty matches tmnl's current grid size
        // before the reader thread starts — pre-emptive resize so the
        // first frame doesn't render at the sender's last-known size.
        let _ = ioctl_set_winsize(file.as_raw_fd(), rows, cols);

        let reader_file = file
            .try_clone()
            .map_err(|e| format!("clone adopted fd for reader: {e}"))?;
        let writer_file = file
            .try_clone()
            .map_err(|e| format!("clone adopted fd for writer: {e}"))?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK,
            TitleSink::default(),
        )));
        let exited = Arc::new(Mutex::new(false));
        let bytes_seen = Arc::new(AtomicU64::new(0));
        let attention_requested = Arc::new(AtomicBool::new(false));
        let integration = Arc::new(Mutex::new(osc133::State::default()));

        let reader = spawn_reader_thread(
            Box::new(reader_file),
            "tmnl-shell-adopted-reader",
            Arc::clone(&parser),
            Arc::clone(&exited),
            Arc::clone(&bytes_seen),
            Arc::clone(&attention_requested),
            Arc::clone(&integration),
        )?;

        Ok(ShellSession {
            parser,
            writer: Box::new(writer_file),
            master: None,
            adopted_fd: Some(file),
            reader: Some(reader),
            child: None,
            exited,
            last_size: (rows, cols),
            bytes_seen,
            last_applied_bytes: 0,
            default_bg,
            default_fg,
            attention_requested,
            integration,
            scroll_dirty: false,
            shell_name: label.to_string(),
            fg_proc_cache: None,
            last_fg_poll: None,
        })
    }

    /// Basename of `$SHELL` (e.g. `zsh`, `bash`, `fish`). Used as a
    /// last-resort tab label when nothing else (OSC title, spinner)
    /// supplies one.
    pub fn shell_name(&self) -> &str {
        &self.shell_name
    }

    /// Foreground process name running in this shell (e.g. `vim`,
    /// `htop`, `less`). Polls `ps` at most every `FG_POLL_INTERVAL` —
    /// the result is cached on `self` so per-tick calls are cheap.
    /// Returns `None` when the only process is the shell itself.
    pub fn fg_proc_name(&mut self) -> Option<&str> {
        // With OSC 133 integration installed the shell tells us exactly
        // when a command is running. If it isn't, there is no foreground
        // process distinct from the shell — skip the `ps` fork entirely.
        if self.shell_integration_active() && !self.command_running() {
            self.fg_proc_cache = None;
            return None;
        }
        let now = std::time::Instant::now();
        let due = self
            .last_fg_poll
            .map(|t| now.duration_since(t) >= FG_POLL_INTERVAL)
            .unwrap_or(true);
        if due {
            self.last_fg_poll = Some(now);
            // Spawn-owned sessions know the shell's pid. Adopted ones
            // don't (the child lives in another process); skip the
            // poll + leave fg_proc_cache at None.
            if let Some(shell_pid) = self.child.as_ref().and_then(|c| c.process_id()) {
                self.fg_proc_cache = poll_child_proc(shell_pid, &self.shell_name);
            }
        }
        self.fg_proc_cache.as_deref()
    }

    /// `true` when the OSC 133 integration snippet is installed in the
    /// running shell — i.e. any semantic-prompt mark has been seen.
    pub fn shell_integration_active(&self) -> bool {
        self.integration.lock().map(|s| s.active()).unwrap_or(false)
    }

    /// `true` while a foreground command is running (between OSC 133 `C`
    /// and `D`). Always `false` when the integration snippet isn't
    /// installed — gate on `shell_integration_active` first.
    pub fn command_running(&self) -> bool {
        self.integration
            .lock()
            .map(|s| s.running())
            .unwrap_or(false)
    }

    /// The text typed so far on the current command line, reconstructed
    /// from `grid` between the OSC 133 `B` anchor and the cursor. `None`
    /// when there's no anchor (integration snippet not installed, a
    /// command is running, or the anchor is stale relative to the
    /// cursor). For AI command completion this string is the prefix.
    pub fn current_command_line(
        &self,
        grid: &Grid,
        cursor_row: u16,
        cursor_col: u16,
    ) -> Option<String> {
        let (ar, ac) = self.integration.lock().ok()?.input_anchor()?;
        // The anchor must sit at or before the cursor for a valid span.
        if (cursor_row, cursor_col) < (ar, ac) {
            return None;
        }
        let mut s = String::new();
        for row in ar..=cursor_row {
            let start = if row == ar { ac } else { 0 };
            let end = if row == cursor_row {
                cursor_col
            } else {
                grid.cols as u16
            };
            for col in start..end {
                let idx = row as u32 * grid.cols + col as u32;
                if (idx as usize) < grid.cells.len() {
                    s.push(grid.cells[idx as usize].ch);
                }
            }
        }
        Some(s)
    }

    /// The OSC 133 `B`-mark cursor position `(row, col)` — where the
    /// current command line begins. `None` when there's no live prompt.
    pub fn input_anchor(&self) -> Option<(u16, u16)> {
        self.integration.lock().ok()?.input_anchor()
    }
}

/// Run `ps -ax -o ppid=,comm=` once and return the first child of
/// `parent_pid` whose comm differs from the shell's own name. The
/// Spawn the reader thread that feeds an incoming byte stream into a
/// shared vt100 parser + OSC scanners. Shared by both [`ShellSession::spawn`]
/// (cross-platform pty master) and [`ShellSession::adopt_fd`] (raw fd
/// adopted via SCM_RIGHTS handoff). The thread terminates on EOF /
/// read error and flips `exited` to `true`.
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    name: &str,
    parser: Arc<Mutex<vt100::Parser<TitleSink>>>,
    exited: Arc<Mutex<bool>>,
    bytes_seen: Arc<AtomicU64>,
    attention: Arc<AtomicBool>,
    integration: Arc<Mutex<osc133::State>>,
) -> Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            let mut osc_carry: Vec<u8> = Vec::new();
            let mut osc133_scanner = osc133::Scanner::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        if let Ok(mut e) = exited.lock() {
                            *e = true;
                        }
                        return;
                    }
                    Ok(n) => {
                        scan_osc_1337(&buf[..n], &mut osc_carry, &attention);
                        let marks = osc133_scanner.scan(&buf[..n]);
                        let mut events: Vec<(osc133::Mark, (u16, u16))> = Vec::new();
                        if let Ok(mut p) = parser.lock() {
                            if marks.is_empty() {
                                p.process(&buf[..n]);
                            } else {
                                let mut seg = 0;
                                for (off, mark) in marks {
                                    let off = off.min(n);
                                    p.process(&buf[seg..off]);
                                    seg = off;
                                    events.push((mark, p.screen().cursor_position()));
                                }
                                p.process(&buf[seg..n]);
                            }
                        }
                        if !events.is_empty()
                            && let Ok(mut st) = integration.lock()
                        {
                            for (mark, cursor) in events {
                                st.apply(mark, cursor);
                            }
                        }
                        bytes_seen.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
            }
        })
        .map_err(|e| format!("spawn reader thread: {e}"))
}

/// Resize an adopted pty by ioctl. The kernel propagates the resize
/// to the slave + delivers SIGWINCH to the foreground process group
/// — same effect as `MasterPty::resize` for the spawn path.
#[cfg(unix)]
fn ioctl_set_winsize(fd: RawFd, rows: u16, cols: u16) -> std::io::Result<()> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ is a tty ioctl that reads a `winsize`. `fd`
    // is borrowed from a live File, so it is valid for the call.
    let r = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// match is approximate — for most cases (one foreground program at a
/// time) it's accurate; multi-child backgrounds picks the first row,
/// which is usually-but-not-always the foreground. Acceptable for a
/// label hint.
fn poll_child_proc(parent_pid: u32, shell_name: &str) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-ax", "-o", "ppid=,comm="])
        .output()
        .ok()?;
    let s = std::str::from_utf8(&out.stdout).ok()?;
    let parent_str = parent_pid.to_string();
    for line in s.lines() {
        let mut parts = line.split_whitespace();
        let ppid = parts.next()?;
        if ppid != parent_str {
            continue;
        }
        let comm = parts.collect::<Vec<_>>().join(" ");
        // Strip absolute path + leading `-` (login-shell marker).
        let base = std::path::Path::new(&comm)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&comm)
            .trim_start_matches('-')
            .to_string();
        // Skip the shell process itself — we want a distinct child.
        if base == shell_name {
            continue;
        }
        if !base.is_empty() {
            return Some(base);
        }
    }
    None
}

impl ShellSession {
    /// Read + clear the OSC 1337 attention flag. Returns `true` if
    /// the shell emitted an attention-request since the last call —
    /// app uses this to badge background tabs with a notification
    /// indicator. Claude Code triggers this when waiting on input.
    pub fn take_attention(&self) -> bool {
        self.attention_requested.swap(false, Ordering::Relaxed)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(4), cols.max(20));
        if self.last_size == (rows, cols) {
            return;
        }
        self.last_size = (rows, cols);
        // Spawn-owned: route through portable-pty so MasterPty's
        // platform-specific size handling fires. Adopted: ioctl
        // straight on the raw fd (we don't own a MasterPty).
        if let Some(m) = self.master.as_ref() {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        #[cfg(unix)]
        if let Some(fd) = self.adopted_fd.as_ref() {
            let _ = ioctl_set_winsize(fd.as_raw_fd(), rows, cols);
        }
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Scroll the terminal's scrollback view. Positive `lines` moves
    /// back into history; negative moves toward the live bottom. vt100
    /// clamps to the available history.
    pub fn scroll(&mut self, lines: i32) {
        if let Ok(mut p) = self.parser.lock() {
            let cur = p.screen().scrollback() as i64;
            let next = (cur + lines as i64).max(0) as usize;
            // vt100 clamps to the available history internally.
            p.screen_mut().set_scrollback(next);
        }
        self.scroll_dirty = true;
    }

    /// Snap the scrollback view back to the live bottom.
    pub fn scroll_reset(&mut self) {
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_scrollback(0);
        }
        self.scroll_dirty = true;
    }

    /// Current scrollback offset in rows — 0 means at the live bottom.
    pub fn scrollback_offset(&self) -> usize {
        self.parser
            .lock()
            .map(|p| p.screen().scrollback())
            .unwrap_or(0)
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

    /// Current OSC title (set by `\033]0;<title>\007` or `\033]2;…\007`
    /// from any process running in the shell — claude / vim / etc. all
    /// emit one). Empty string if nothing has set a title yet. tmnl's
    /// App layer uses this to label the tab strip.
    pub fn osc_title(&self) -> String {
        match self.parser.lock() {
            Ok(p) => p.callbacks().title.clone(),
            Err(_) => String::new(),
        }
    }

    /// Scan the visible screen for Claude Code's status-line pattern
    /// (`✽ Wandering…`, `✶ Pondering…`, etc.) and return it if found.
    /// Lets tmnl mirror Claude's live spinner — the glyph cycles and
    /// the verb changes, so the tab label updates each tick. Returns
    /// `None` if no spinner-like line is visible (caller falls back
    /// to the OSC title).
    ///
    /// Pattern: any visible row that (a) contains one of the spinner
    /// glyphs Claude uses and (b) contains a Unicode ellipsis `…` or
    /// `...` (the trailing dots on "Wandering…" / "Pondering..." —
    /// pretty unique to spinner UIs). The match starts at the spinner
    /// glyph and extends to the end of the visible word + ellipsis,
    /// trimming Claude's trailing "(esc to interrupt)" / duration.
    /// Scans bottom-up since the spinner lives near the input box.
    pub fn detect_status_line(&self) -> Option<String> {
        const SPINNER_CHARS: &[char] = &[
            '✱', '✶', '✦', '✧', '⋆', '✽', '✻', '❋', '✿', '✺', '✷', '✸', '✹', '❉', '❅', '◐', '◓',
            '◑', '◒',
        ];
        const MAX_LABEL_CHARS: usize = 30;
        let p = self.parser.lock().ok()?;
        let screen = p.screen();
        let (rows, cols) = screen.size();
        // Scan the whole visible region from the bottom up. Claude
        // typically renders its spinner near the input prompt at the
        // bottom; vim, htop etc. that might also match a spinner-like
        // pattern usually do so on a status row also at the bottom.
        for row in (0..rows).rev() {
            // Build the row's text content into one string.
            let mut line = String::new();
            for col in 0..cols {
                if let Some(c) = screen.cell(row, col) {
                    line.push_str(c.contents());
                }
            }
            let line_trimmed = line.trim_end();
            // Must contain BOTH a spinner glyph AND an ellipsis
            // (`…` U+2026 or `...` three dots) — the two-signal
            // combo rejects unrelated lines that happen to start
            // with `*` etc.
            let glyph_pos = line_trimmed
                .chars()
                .position(|c| SPINNER_CHARS.contains(&c));
            let Some(glyph_pos) = glyph_pos else { continue };
            let has_ellipsis = line_trimmed.contains('…') || line_trimmed.contains("...");
            if !has_ellipsis {
                continue;
            }
            // Take from the glyph onward, capped at MAX_LABEL_CHARS.
            let from_glyph: String = line_trimmed
                .chars()
                .skip(glyph_pos)
                .take(MAX_LABEL_CHARS)
                .collect();
            // Trim Claude's trailing UI metadata. Common shapes:
            //   "✽ Wandering… (running stop hook · 2s · esc to interrupt)"
            //   "✻ Crunched for 2s · 1.2k tokens"
            //   "✶ Pondering… (3s · esc to interrupt)"
            // Stop at the first `(` (parenthetical context) or `·`
            // (Claude's bullet separator) — whichever comes first.
            let cut_at = from_glyph
                .char_indices()
                .find(|(_, c)| *c == '(' || *c == '·')
                .map(|(i, _)| i)
                .unwrap_or(from_glyph.len());
            let label = from_glyph[..cut_at].trim_end().to_string();
            if !label.is_empty() {
                return Some(label);
            }
        }
        None
    }

    /// Did new output arrive since the last `apply_to_grid`? Used to skip
    /// rebuilding instances when the shell's been idle.
    pub fn dirty(&self) -> bool {
        self.bytes_seen.load(Ordering::Relaxed) != self.last_applied_bytes || self.scroll_dirty
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
        self.scroll_dirty = false;
        (cc, cr, visible)
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        // Only kill the child we spawned ourselves. Adopted sessions
        // share the pty master with the original sender; killing the
        // child via this side would be wrong (we don't even know the
        // pid) and the kernel handles teardown when both fds close.
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

fn vt_color_to_rgba(c: vt100::Color, default: [f32; 4]) -> [f32; 4] {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
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

/// Scan a chunk of pty bytes for OSC 1337 sequences and flip
/// `attention` when one is seen. Doesn't consume the bytes — they
/// continue downstream to vt100 (which silently drops the unhandled
/// OSC). `carry` buffers partial sequences across read-boundary
/// splits so a `\x1b]1337` that lands at the end of one chunk and
/// `;Notify=...\x07` at the start of the next still resolves.
///
/// OSC format we recognize: `ESC ] 1337 ; <payload> (BEL | ESC \)`.
/// Any payload triggers attention — we don't parse the iTerm2
/// sub-grammar (`Notify=…`, `RequestAttention=…`, `StealFocus`).
/// Claude Code uses `RequestAttention=…` when a turn finishes; vim and
/// tmux occasionally emit other 1337 sequences (cursor color, etc.) and
/// harmlessly trigger an attention blip — an acceptable false-positive
/// cost.
fn scan_osc_1337(chunk: &[u8], carry: &mut Vec<u8>, attention: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering;
    const OSC_START: &[u8] = b"\x1b]1337;";
    const MAX_PAYLOAD: usize = 256; // longest plausible OSC 1337 args

    // Combine carried prefix + new chunk into one owned view, then
    // scan it. Avoids aliasing `carry` vs the borrow we'd need to
    // mutate `carry` mid-loop. Cost: one Vec copy per chunk when
    // there's nothing carried (the common path); cheaper than the
    // alternative of threading an offset through every branch.
    let view: Vec<u8> = if carry.is_empty() {
        chunk.to_vec()
    } else {
        let mut v = std::mem::take(carry);
        v.extend_from_slice(chunk);
        v
    };

    let mut i = 0;
    while i < view.len() {
        // Find next OSC_START (possibly partial at tail).
        if view[i] != 0x1b {
            i += 1;
            continue;
        }
        // Full OSC_START match?
        if i + OSC_START.len() <= view.len() && &view[i..i + OSC_START.len()] == OSC_START {
            // Look for terminator (BEL or ST = ESC \) up to MAX_PAYLOAD bytes.
            let scan_end = (i + OSC_START.len() + MAX_PAYLOAD).min(view.len());
            let mut term_at: Option<usize> = None;
            let mut j = i + OSC_START.len();
            while j < scan_end {
                if view[j] == 0x07 {
                    term_at = Some(j + 1);
                    break;
                }
                if view[j] == 0x1b && j + 1 < scan_end && view[j + 1] == b'\\' {
                    term_at = Some(j + 2);
                    break;
                }
                j += 1;
            }
            match term_at {
                Some(end) => {
                    attention.store(true, Ordering::Relaxed);
                    i = end;
                    continue;
                }
                None => {
                    // Incomplete payload — carry from `i` onward for the next read.
                    carry.clear();
                    carry.extend_from_slice(&view[i..]);
                    return;
                }
            }
        }
        // Tail might be a partial OSC_START prefix (e.g. ends in `\x1b]133`).
        // If everything from `i` to end of view matches a PREFIX of OSC_START,
        // carry it and bail — we'll match in full once the next chunk arrives.
        let tail = &view[i..];
        if tail.len() < OSC_START.len() && OSC_START.starts_with(tail) {
            carry.clear();
            carry.extend_from_slice(tail);
            return;
        }
        // Not OSC 1337 — move past this ESC.
        i += 1;
    }
    // Whole view consumed; nothing partial to carry.
    carry.clear();
}

#[cfg(test)]
mod scan_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn flag_after(chunks: &[&[u8]]) -> bool {
        let attention = AtomicBool::new(false);
        let mut carry = Vec::new();
        for c in chunks {
            scan_osc_1337(c, &mut carry, &attention);
        }
        attention.load(Ordering::Relaxed)
    }

    #[test]
    fn detects_bel_terminated_osc_1337() {
        assert!(flag_after(&[b"\x1b]1337;Notify=hello\x07"]));
    }

    #[test]
    fn detects_st_terminated_osc_1337() {
        assert!(flag_after(&[b"\x1b]1337;RequestAttention=yes\x1b\\"]));
    }

    #[test]
    fn ignores_other_osc() {
        assert!(!flag_after(&[b"\x1b]0;Window Title\x07"]));
        assert!(!flag_after(&[b"\x1b]2;Tab Title\x07"]));
    }

    #[test]
    fn handles_split_across_reads() {
        assert!(flag_after(&[
            b"plain bytes \x1b]133",
            b"7;Notify=split\x07 more",
        ]));
    }

    #[test]
    fn surrounded_by_normal_text() {
        assert!(flag_after(&[b"hello \x1b]1337;Notify=ok\x07 world",]));
    }
}

#[cfg(test)]
mod encode_tests {
    use super::*;

    fn ch(s: &str, mods: ModifiersState) -> Option<Vec<u8>> {
        winit_key_to_bytes(&Key::Character(s.into()), mods)
    }
    fn named(n: NamedKey) -> Option<Vec<u8>> {
        winit_key_to_bytes(&Key::Named(n), ModifiersState::empty())
    }

    #[test]
    fn plain_chars_pass_through_as_utf8() {
        assert_eq!(ch("a", ModifiersState::empty()), Some(b"a".to_vec()));
        // A non-ASCII char keeps its multi-byte UTF-8 encoding.
        assert_eq!(
            ch("é", ModifiersState::empty()),
            Some("é".as_bytes().to_vec())
        );
    }

    #[test]
    fn ctrl_letters_encode_as_control_bytes() {
        assert_eq!(ch("c", ModifiersState::CONTROL), Some(vec![0x03])); // ^C
        assert_eq!(ch("a", ModifiersState::CONTROL), Some(vec![0x01])); // ^A
        // Ctrl is case-insensitive — ^C and Ctrl+Shift+C both give 0x03.
        assert_eq!(
            ch("C", ModifiersState::CONTROL | ModifiersState::SHIFT),
            Some(vec![0x03])
        );
        // The non-letter control slots.
        assert_eq!(ch(" ", ModifiersState::CONTROL), Some(vec![0])); // ^Space
        assert_eq!(ch("[", ModifiersState::CONTROL), Some(vec![0x1b])); // ^[ == Esc
        assert_eq!(ch("\\", ModifiersState::CONTROL), Some(vec![0x1c]));
    }

    #[test]
    fn alt_prefixes_a_char_with_escape() {
        assert_eq!(ch("x", ModifiersState::ALT), Some(vec![0x1b, b'x']));
    }

    #[test]
    fn named_keys_encode_to_their_terminal_sequences() {
        assert_eq!(named(NamedKey::Enter), Some(b"\r".to_vec()));
        assert_eq!(named(NamedKey::Backspace), Some(b"\x7f".to_vec()));
        assert_eq!(named(NamedKey::Tab), Some(b"\t".to_vec()));
        assert_eq!(named(NamedKey::Escape), Some(b"\x1b".to_vec()));
        // Arrows — CSI cursor sequences.
        assert_eq!(named(NamedKey::ArrowUp), Some(b"\x1b[A".to_vec()));
        assert_eq!(named(NamedKey::ArrowDown), Some(b"\x1b[B".to_vec()));
        assert_eq!(named(NamedKey::ArrowRight), Some(b"\x1b[C".to_vec()));
        assert_eq!(named(NamedKey::ArrowLeft), Some(b"\x1b[D".to_vec()));
        // Navigation + editing.
        assert_eq!(named(NamedKey::Home), Some(b"\x1b[H".to_vec()));
        assert_eq!(named(NamedKey::PageUp), Some(b"\x1b[5~".to_vec()));
        assert_eq!(named(NamedKey::Delete), Some(b"\x1b[3~".to_vec()));
        // Function keys — F1–F4 use SS3, F5+ use CSI.
        assert_eq!(named(NamedKey::F1), Some(b"\x1bOP".to_vec()));
        assert_eq!(named(NamedKey::F5), Some(b"\x1b[15~".to_vec()));
        assert_eq!(named(NamedKey::F12), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn ansi_color_covers_the_three_palette_ranges() {
        // The 16 themed slots: index 15 is white.
        assert_eq!(ansi_color(15), [1.0, 1.0, 1.0, 1.0]);
        // The 6×6×6 cube: index 16 is its black corner.
        assert_eq!(ansi_color(16), [0.0, 0.0, 0.0, 1.0]);
        // The 24-step grayscale ramp: 232 is the darkest (value 8).
        let v = 8.0 / 255.0;
        assert_eq!(ansi_color(232), [v, v, v, 1.0]);
    }

    #[test]
    fn vt_color_to_rgba_handles_each_color_kind() {
        let default = [0.5, 0.5, 0.5, 1.0];
        // Default defers to the supplied fallback.
        assert_eq!(vt_color_to_rgba(vt100::Color::Default, default), default);
        // A true-color RGB maps straight through.
        assert_eq!(
            vt_color_to_rgba(vt100::Color::Rgb(255, 0, 0), default),
            [1.0, 0.0, 0.0, 1.0]
        );
        // An indexed color routes through the ANSI palette.
        assert_eq!(
            vt_color_to_rgba(vt100::Color::Idx(15), default),
            ansi_color(15)
        );
    }
}
