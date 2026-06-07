//! `App` impls — the winit `ApplicationHandler` body and `App`'s
//! intrinsic methods. The struct definition + supporting types
//! (`Tab`, `Pane`, `RenameState`, `Ghost`, …) remain in `src/main.rs`
//! because they're tightly woven into the file's other free fns.
//!
//! Extracted from `main.rs` in the file-split refactor
//! (`.local/PLAN.md` refactor Phase 1). Pure non-destructive move.

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::Key;
use winit::window::{Window, WindowId};

use crate::*;

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("tmnl")
            .with_inner_size(winit::dpi::LogicalSize::new(1400.0, 900.0));
        // Ghostty / Warp style title bar: drop the "tmnl" text + the
        // chrome strip but keep the traffic-light buttons floating, and
        // let the surface extend behind where the titlebar was so the
        // cell grid runs flush against the window's top edge. Crucial
        // distinction — `titlebar_hidden` would drop the buttons too;
        // `title_hidden + transparent + fullsize_content_view` keeps
        // them. System menu bar at the top of the screen is unaffected.
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_title_hidden(true)
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        // Install the native menu bar once NSApp is alive (winit has
        // bootstrapped it by the time `resumed` fires).
        if self.app_menu.is_none() {
            self.app_menu = Some(AppMenu::build_and_install());
        }
        let gpu = pollster::block_on(Gpu::new(window.clone(), self.inset_px));
        let (cols, rows) = (gpu.grid.cols, gpu.grid.rows);
        // Panes were created before the GPU existed (placeholder grid
        // size); bring every pane grid up to the real window dims.
        for tab in &mut self.tabs {
            for pane in &mut tab.panes {
                pane.grid.resize(cols, rows);
            }
        }
        let focused = self.tabs[self.active].focused;
        let Pane { kind, grid, .. } = &mut self.tabs[self.active].panes[focused];
        match kind {
            PaneKind::Shell { session } => {
                match ShellSession::spawn(
                    rows as u16,
                    cols as u16,
                    palette().text_fg,
                    palette().clear_bg,
                ) {
                    Ok(s) => *session = Some(s),
                    Err(e) => {
                        eprintln!("tmnl: failed to start shell: {e}");
                        self.should_quit = true;
                        return;
                    }
                }
            }
            PaneKind::Native { server, conn, .. } => {
                paint_idle(grid, *conn, &server.socket_path);
            }
            PaneKind::Browser { url, .. } => {
                paint_browser_placeholder(grid, url);
            }
        }
        window.request_redraw();
        self.window = Some(window);
        self.gpu = Some(gpu);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.shutdown_gracefully();
                self.should_quit = true;
            }
            WindowEvent::Resized(size) => {
                self.handle_resized(size);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.scale = scale_factor as f32;
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
            }
            WindowEvent::KeyboardInput { event: ke, .. } => {
                self.handle_keyboard_input(event_loop, ke);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.handle_mouse_wheel(delta);
            }
            WindowEvent::RedrawRequested => {
                self.handle_redraw_requested(event_loop);
            }
            _ => {}
        }
        // Any handler above may have set `should_quit` (last tab
        // closed, ⌘Q, blit channel reported its child exited, …).
        // The flag is the single source of truth for "ready to
        // exit" so headless mode and winit mode share the same
        // signal — headless reads it after each stdin command.
        if self.should_quit {
            event_loop.exit();
        }
    }
}

impl App {
    /// Construct an App suitable for headless mode — no window, no
    /// winit. Builds a real wgpu fallback-adapter Gpu via
    /// [`Gpu::new_headless`], wires a single empty shell-mode tab,
    /// and leaves every Optional / state field at its default. The
    /// returned App is immediately driveable through the
    /// `dispatch_*` methods (Phase B/1 cleanup ensured those don't
    /// require `&ActiveEventLoop` anymore).
    ///
    /// Bug-hunt agents + tests can use this to exercise multi-tab
    /// flows, chip layouts, chrome hit-testing without spinning up
    /// a window. Rendering paths short-circuit (no Surface), but
    /// every other code path runs identically to real mode — so
    /// "headless test passed but real mode renders differently" is
    /// avoided structurally.
    pub fn new_headless(
        width: u32,
        height: u32,
        inset_px: f32,
        cfg: crate::config::Config,
    ) -> Result<Self, String> {
        let gpu = pollster::block_on(Gpu::new_headless(width, height, inset_px))?;
        let initial_pane = Pane {
            kind: PaneKind::Shell { session: None },
            grid: grid::Grid::new(80, 24, palette().clear_bg),
            last_cursor: None,
            label: String::new(),
            attention: false,
            last_status: None,
        };
        let initial_tab = Tab {
            layout: Layout::Leaf(0),
            panes: vec![initial_pane],
            focused: 0,
            label: String::new(),
            custom_name: None,
        };
        Ok(App {
            window: None,
            gpu: Some(gpu),
            mods: winit::keyboard::ModifiersState::empty(),
            should_quit: false,
            keymap: crate::keymap::Keymap::build(),
            help: None,
            palette: None,
            cursor_cell: (0, 0),
            cursor_px: (0.0, 0.0),
            buttons_down: 0,
            tabs: vec![initial_tab],
            active: 0,
            inset_px,
            cfg,
            altscreen_active: false,
            app_menu: None,
            settings: None,
            welcome: None,
            editor_template: None,
            native_tab_nonce: 1,
            dragging_tab: None,
            renaming_tab: None,
            dragging_divider: None,
            fim: None,
            fim_pending: None,
            fim_next_id: 0,
            ghost: None,
            fim_redraw: false,
            transfer_listener: None,
        })
    }

    /// Spawn a new Native (editor) tab — fresh socket, fresh Server,
    /// fresh Launcher pointing at the same `editor_template.command`
    /// the original tab used. No-op when shell mode is active
    /// (`editor_template == None` ⇒ falls back to a shell tab so ⌘T
    /// still does something sensible). The new tab's pane owns its
    /// own grid, sized to the current window.
    pub(crate) fn new_native_tab(&mut self) {
        let Some(tmpl) = self.editor_template.clone() else {
            // Fall back to a shell tab — gives ⌘T a sensible behavior
            // in shell-mode windows.
            self.new_shell_tab();
            return;
        };
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        // Allocate a unique socket path for this tab.
        let socket_path = native_tab_socket_path(self.native_tab_nonce);
        self.native_tab_nonce = self.native_tab_nonce.saturating_add(1);
        let server = match Server::start(socket_path.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tmnl: new native tab failed (server start): {e}");
                return;
            }
        };
        let cfg = LauncherConfig {
            command: tmpl.command.clone(),
            workspace: tmpl.workspace.clone(),
            socket: socket_path.clone(),
            extra_args: tmpl.extra_args.clone(),
        };
        let mut l = Launcher::new(cfg);
        let launcher = match l.spawn() {
            Ok(()) => Some(l),
            Err(e) => {
                eprintln!(
                    "tmnl: new native tab launch failed ({e}); start mnml manually with --blit {}",
                    socket_path.display()
                );
                None
            }
        };
        let mut grid = grid::Grid::new(cols, rows, palette().clear_bg);
        paint_idle(&mut grid, ConnState::Waiting, &socket_path);
        let pane = Pane {
            kind: PaneKind::Native {
                server,
                conn: ConnState::Waiting,
                launcher,
                client_title: None,
            },
            grid,
            last_cursor: None,
            label: "mnml".to_string(),
            attention: false,
            last_status: None,
        };
        self.tabs.push(Tab {
            layout: Layout::Leaf(0),
            panes: vec![pane],
            focused: 0,
            label: "mnml".to_string(),
            custom_name: None,
        });
        self.active = self.tabs.len() - 1;
    }

    /// Drain pending pty-fd handoffs from the transfer listener (task
    /// #50). Each handoff lights up a new adopted-shell tab — same
    /// rendering pipeline as a spawned shell, just adopting the master
    /// fd handed to us via SCM_RIGHTS instead of opening one ourselves.
    /// No-op when the listener failed to start at boot.
    fn drain_transfer_events(&mut self) {
        let Some(listener) = self.transfer_listener.as_ref() else {
            return;
        };
        // Collect events first so we don't hold an &self borrow across
        // the mutating `adopt_pty_into_new_tab` calls.
        let mut events: Vec<transfer::TransferEvent> = Vec::new();
        while let Ok(ev) = listener.events.try_recv() {
            events.push(ev);
        }
        for ev in events {
            match ev {
                #[cfg(unix)]
                transfer::TransferEvent::OpenPaneTransfer { command, args, fd } => {
                    self.adopt_pty_into_new_tab(command, args, fd);
                }
                transfer::TransferEvent::PromoteToNative { command, args } => {
                    self.spawn_native_in_new_tab(command, args);
                }
                #[cfg(not(unix))]
                _ => {}
            }
        }
    }

    /// Append a new top-level tab running `command [args…]` as a
    /// native blit client. The companion to [`Self::open_pane_with_command`]
    /// (which splits inside the focused tab) for outside-in promotion
    /// requests arriving over the transfer socket — e.g. mnml at
    /// startup detecting tmnl's env var and asking to be relaunched
    /// natively.
    fn spawn_native_in_new_tab(&mut self, command: String, mut args: Vec<String>) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        let socket_path = native_tab_socket_path(self.native_tab_nonce);
        self.native_tab_nonce = self.native_tab_nonce.saturating_add(1);
        let server = match Server::start(socket_path.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tmnl: promote-to-native server start failed: {e}");
                return;
            }
        };
        // Launcher::spawn unconditionally prepends `cfg.workspace` as
        // the child's first positional, then appends `extra_args`. For
        // PromoteToNative, mnml's `build_promote_args` already put the
        // workspace first in `args` — peel it off here so the child
        // doesn't get two positional workspaces (which would make
        // mnml's arg parser error out with "unexpected extra argument").
        // No args at all ⇒ fall back to cwd as a sensible default.
        let workspace = if args.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| ".".into())
        } else {
            std::path::PathBuf::from(args.remove(0))
        };
        let label = std::path::Path::new(&command)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string();
        let cfg = LauncherConfig {
            command: std::path::PathBuf::from(&command),
            workspace,
            socket: socket_path.clone(),
            extra_args: args,
        };
        let mut l = Launcher::new(cfg);
        let launcher = match l.spawn() {
            Ok(()) => Some(l),
            Err(e) => {
                eprintln!(
                    "tmnl: promote-to-native launch failed ({e}); start it manually with --blit {}",
                    socket_path.display()
                );
                None
            }
        };
        let mut grid = grid::Grid::new(cols, rows, palette().clear_bg);
        paint_idle(&mut grid, ConnState::Waiting, &socket_path);
        let pane = Pane {
            kind: PaneKind::Native {
                server,
                conn: ConnState::Waiting,
                launcher,
                client_title: None,
            },
            grid,
            last_cursor: None,
            label: label.clone(),
            attention: false,
            last_status: None,
        };
        self.tabs.push(Tab {
            layout: Layout::Leaf(0),
            panes: vec![pane],
            focused: 0,
            label,
            custom_name: None,
        });
        self.active = self.tabs.len() - 1;
        self.on_tab_focused();
        self.relayout_all_panes();
    }

    /// Create a new tab whose pane owns an adopted pty master fd —
    /// produced by mnml's pop-out path (task #49). The pane uses the
    /// shell pipeline (`PaneKind::Shell`) since the cell-grid renderer
    /// is identical whether tmnl spawned the child or merely adopted
    /// its master fd from a sibling process.
    ///
    /// Label preference: basename of `command` (e.g. `claude` from
    /// `/usr/local/bin/claude`). If somehow empty, falls back to
    /// `"adopted"`.
    #[cfg(unix)]
    fn adopt_pty_into_new_tab(
        &mut self,
        command: String,
        _args: Vec<String>,
        fd: std::os::unix::io::OwnedFd,
    ) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        let label = std::path::Path::new(&command)
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "adopted".to_string());
        match ShellSession::adopt_fd(
            fd,
            rows as u16,
            cols as u16,
            palette().text_fg,
            palette().clear_bg,
            &label,
        ) {
            Ok(s) => {
                let pane = Pane {
                    kind: PaneKind::Shell { session: Some(s) },
                    grid: grid::Grid::new(cols, rows, palette().clear_bg),
                    last_cursor: None,
                    label: label.clone(),
                    attention: false,
                    last_status: None,
                };
                self.tabs.push(Tab {
                    layout: Layout::Leaf(0),
                    panes: vec![pane],
                    focused: 0,
                    label,
                    custom_name: None,
                });
                self.active = self.tabs.len() - 1;
                self.on_tab_focused();
                self.relayout_all_panes();
            }
            Err(e) => {
                eprintln!("tmnl: pty-fd adoption failed: {e}");
                // `fd` already consumed by the failed call's
                // `OwnedFd::into::<File>` — no further cleanup needed.
            }
        }
    }

    /// Append a fresh shell tab and switch to it. The new tab's pane
    /// owns its own grid, sized to the current window; spawn failures
    /// leave the existing active tab in place and toast to stderr.
    pub(crate) fn new_shell_tab(&mut self) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        match ShellSession::spawn(
            rows as u16,
            cols as u16,
            palette().text_fg,
            palette().clear_bg,
        ) {
            Ok(s) => {
                let label = s.shell_name().to_string();
                let pane = Pane {
                    kind: PaneKind::Shell { session: Some(s) },
                    grid: grid::Grid::new(cols, rows, palette().clear_bg),
                    last_cursor: None,
                    label: label.clone(),
                    attention: false,
                    last_status: None,
                };
                self.tabs.push(Tab {
                    layout: Layout::Leaf(0),
                    panes: vec![pane],
                    focused: 0,
                    label,
                    custom_name: None,
                });
                self.active = self.tabs.len() - 1;
            }
            Err(e) => eprintln!("tmnl: new tab failed: {e}"),
        }
    }

    /// Close the active tab. Closing the last tab exits the process
    /// (matches macOS Terminal: ⌘W on last tab quits the window).
    /// Active index clamps to a valid position; Mode resources drop
    /// when their Tab is removed (Launcher::Drop kills the spawned
    /// client; ShellSession's reader thread joins via its own Drop).
    pub(crate) fn close_active_tab(&mut self) {
        self.close_tab_at(self.active);
    }

    /// Close a specific tab by index. Used by middle-click on a chip;
    /// when `idx` is the active tab, behaves identically to
    /// `close_active_tab`. Closing the last tab exits the process.
    fn close_tab_at(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        // A rename in flight pins a tab index that removing a tab would
        // invalidate (indices shift) — drop it.
        self.renaming_tab = None;
        if self.tabs.len() <= 1 {
            self.should_quit = true;
            return;
        }
        let was_active = idx == self.active;
        self.tabs.remove(idx);
        if was_active {
            // Active tab closed — clamp the index to a valid slot.
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
            self.on_tab_focused();
        } else if idx < self.active {
            // Closed a non-active tab to the left — shift active index
            // down so it still points at the same tab.
            self.active -= 1;
        }
        // If `idx > self.active`, the active index is unaffected.
    }

    /// Switch to tab `idx` (0-based). No-op if out of range.
    pub(crate) fn switch_to_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() || idx == self.active {
            return;
        }
        self.active = idx;
        self.on_tab_focused();
    }

    /// Rebuild the atlas at a new font-zoom level (relative step). After
    /// resizing the grid the new cell dims are forwarded to every pane's
    /// session so the hosted shell / mnml repaints at the new dimensions.
    pub(crate) fn zoom_font(&mut self, delta: f32) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let target = gpu.font_zoom + delta;
        let resize = gpu.set_font_zoom(target);
        self.forward_font_resize(resize);
    }

    /// Reset the font zoom to 1.0 (⌘0). Same resize plumbing as `zoom_font`.
    pub(crate) fn reset_font_zoom(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let resize = gpu.set_font_zoom(1.0);
        self.forward_font_resize(resize);
    }

    /// Pipe a grid resize (caused by a font-zoom change) out to every
    /// pane — same shape as the window-resize / inset / strip paths.
    fn forward_font_resize(&mut self, resize: Option<(u16, u16)>) {
        if resize.is_some() {
            self.relayout_all_panes();
        }
    }

    /// Cycle to the next (`forward=true`) / previous tab, wrapping.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        if self.tabs.len() <= 1 {
            return;
        }
        let n = self.tabs.len();
        self.active = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
        self.on_tab_focused();
    }

    /// Housekeeping when a tab becomes active. Pre-splits this swapped
    /// grid snapshots between the GPU and the tabs; panes now own their
    /// grids permanently (the compositor reads them in place), so the
    /// only thing left is clearing the attention badge — the user is
    /// now looking at this tab.
    ///
    /// Triggers `relayout_all_panes` so any Browser panes on the
    /// newly-active tab move on-screen and the just-deactivated tab's
    /// Browser panes get parked off-screen — wry's native widget
    /// composites over the wgpu frame regardless of which tab "owns"
    /// it, so we need to actively reposition them on every tab swap.
    fn on_tab_focused(&mut self) {
        for pane in &mut self.tabs[self.active].panes {
            pane.attention = false;
        }
        self.relayout_all_panes();
    }

    /// Resize every pane in every tab to its leaf rect under the
    /// current window grid, and forward the new size to each pane's
    /// session. Layout-aware: a tab with splits gets each pane sized to
    /// its share of the window, not the whole window. Called whenever
    /// the window grid is resized (window resize, font zoom, strip /
    /// inset change) OR a tab's layout changes (split / close).
    fn relayout_all_panes(&mut self) {
        let (cols, rows, cell_w, cell_h, strip_h_px, inset_px) = match self.gpu.as_ref() {
            Some(gpu) => (
                gpu.grid.cols,
                gpu.grid.rows,
                gpu.atlas.cell_w,
                gpu.atlas.cell_h,
                gpu.strip_h,
                gpu.inset_px,
            ),
            None => return,
        };
        let area = Rect::new(0, 0, cols, rows);
        let active_tab = self.active;
        for (tab_idx, tab) in self.tabs.iter_mut().enumerate() {
            let tab_visible = tab_idx == active_tab;
            for (pane_id, rect) in tab.layout.leaf_rects(area) {
                let Some(pane) = tab.panes.get_mut(pane_id) else {
                    continue;
                };
                pane.grid.resize(rect.w, rect.h);
                let Pane { kind, grid, .. } = pane;
                match kind {
                    PaneKind::Shell { session } => {
                        if let Some(s) = session {
                            s.resize(rect.h as u16, rect.w as u16);
                        }
                    }
                    PaneKind::Native { server, conn, .. } => {
                        server.send_resize(rect.w as u16, rect.h as u16);
                        // Re-center the idle banner for not-yet-streaming
                        // panes so it isn't stranded off-center.
                        if *conn != ConnState::Streaming {
                            paint_idle(grid, *conn, &server.socket_path);
                        }
                    }
                    PaneKind::Browser {
                        url,
                        webview,
                        chrome,
                    } => {
                        // Paint the chrome strip on row 0 + the
                        // placeholder underneath (visible only if the
                        // wry mount failed).
                        paint_browser_placeholder(grid, url);
                        paint_browser_chrome(grid, url, chrome);
                        if let Some(v) = webview.as_ref() {
                            if tab_visible {
                                // Project the leaf rect (in cells) to
                                // logical pixels inside the parent window
                                // and resize the WebView to match.
                                // `strip_h_px` accounts for the tab strip
                                // above the body; `inset_px` is the
                                // body's left/top inset. Reserve one
                                // cell row at the top for the chrome
                                // strip so the WebView doesn't overlay
                                // it.
                                let x_px = inset_px + rect.x as f32 * cell_w;
                                let y_px = strip_h_px + rect.y as f32 * cell_h + cell_h;
                                let w_px = rect.w as f32 * cell_w;
                                let h_px = (rect.h.saturating_sub(1)) as f32 * cell_h;
                                let _ = v.set_bounds(wry::Rect {
                                    position: wry::dpi::LogicalPosition::new(
                                        x_px as i32,
                                        y_px as i32,
                                    )
                                    .into(),
                                    size: wry::dpi::LogicalSize::new(
                                        w_px.max(1.0) as u32,
                                        h_px.max(1.0) as u32,
                                    )
                                    .into(),
                                });
                            } else {
                                // Pane #90: tab isn't active. The wry
                                // WebView is a native NSView/HWND/
                                // GtkWidget parented to the window — it
                                // would otherwise float over whichever
                                // tab IS active. Park it off-screen at
                                // 1×1 (set_bounds(0,0,0,0) is rejected
                                // by some backends). The next relayout
                                // when this tab regains focus restores
                                // it to its real rect, preserving page
                                // state.
                                let _ = v.set_bounds(wry::Rect {
                                    position: wry::dpi::LogicalPosition::new(-32_000, -32_000)
                                        .into(),
                                    size: wry::dpi::LogicalSize::new(1, 1).into(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    /// Split the focused pane — a fresh shell pane takes half its area.
    /// `SplitDir::Vertical` puts the new pane to the right, `Horizontal`
    /// below. The new pane takes focus.
    pub(crate) fn split_active_pane(&mut self, dir: SplitDir) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        // New panes are shells (cheap; a Native split is a follow-up).
        let session = match ShellSession::spawn(
            rows as u16,
            cols as u16,
            palette().text_fg,
            palette().clear_bg,
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tmnl: split failed: {e}");
                return;
            }
        };
        let label = session.shell_name().to_string();
        let tab = &mut self.tabs[self.active];
        let new_id = tab.panes.len();
        tab.panes.push(Pane {
            kind: PaneKind::Shell {
                session: Some(session),
            },
            // relayout_all_panes resizes this to its real leaf rect.
            grid: grid::Grid::new(cols, rows, palette().clear_bg),
            last_cursor: None,
            label,
            attention: false,
            last_status: None,
        });
        tab.layout.split_leaf(tab.focused, dir, new_id);
        tab.focused = new_id;
        self.relayout_all_panes();
    }

    /// Split the active pane in direction `dir`, opening a Browser
    /// pane pointed at `url`. Mounts a `wry::WebView` as a sub-region
    /// of the parent winit window; the WebView's native surface
    /// (WKWebView / WebKitGTK / WebView2) composites over the wgpu
    /// frame. Position is tightened to the pane's rect by
    /// `relayout_all_panes` at the end.
    pub(crate) fn split_active_pane_browser(&mut self, dir: SplitDir, url: String) {
        self.split_active_pane_browser_with_init(dir, url, None);
    }

    /// Variant of [`Self::split_active_pane_browser`] that registers an
    /// initialization script with the WebView. The script runs on
    /// every page load *before* page scripts execute. Used by
    /// `browser.attach_dashboard_auto` to install a MutationObserver
    /// that clicks the first session row + hides the chrome once the
    /// dashboard's React tree mounts.
    pub(crate) fn split_active_pane_browser_with_init(
        &mut self,
        dir: SplitDir,
        url: String,
        init_script: Option<&str>,
    ) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        let mut grid = grid::Grid::new(cols, rows, palette().clear_bg);
        paint_browser_placeholder(&mut grid, &url);
        let host = url
            .split("://")
            .nth(1)
            .unwrap_or(&url)
            .split('/')
            .next()
            .unwrap_or(&url)
            .to_string();

        // Attempt to mount the webview. Initial bounds are 0,0,0,0
        // — `relayout_all_panes` immediately repositions it to the
        // pane's actual rect. Failure to create (e.g. WebView2
        // runtime missing on Windows) leaves `webview = None`; the
        // placeholder grid is the visible fallback.
        let webview = self.try_create_webview(&url, init_script);

        let tab = &mut self.tabs[self.active];
        let new_id = tab.panes.len();
        tab.panes.push(Pane {
            kind: PaneKind::Browser {
                url: url.clone(),
                webview,
                chrome: crate::BrowserChrome::default(),
            },
            grid,
            last_cursor: None,
            label: host,
            attention: false,
            last_status: None,
        });
        tab.layout.split_leaf(tab.focused, dir, new_id);
        tab.focused = new_id;
        self.relayout_all_panes();
    }

    /// Create the `wry::WebView` mounted in `self.window`. Returns
    /// `None` if there's no parent window yet (pre-`resumed`) or if
    /// wry's child-mount fails (e.g. missing WebView2 runtime on
    /// Windows). Same signature on all platforms — wry abstracts the
    /// per-OS native view.
    fn try_create_webview(&self, url: &str, init_script: Option<&str>) -> Option<wry::WebView> {
        use raw_window_handle::HasWindowHandle;
        let window = self.window.as_ref()?;
        let handle = window.window_handle().ok()?;
        let mut builder = wry::WebViewBuilder::new_as_child(&handle)
            .with_url(url)
            .with_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0, 0).into(),
                size: wry::dpi::LogicalSize::new(1, 1).into(),
            });
        if let Some(script) = init_script {
            builder = builder.with_initialization_script(script);
        }
        match builder.build() {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("tmnl: WebView mount failed for {url}: {e}");
                None
            }
        }
    }

    /// Open a new Native pane running `command args…` as a vertical
    /// split off the focused pane — the server side of
    /// `Message::OpenPane`. tmnl mints the socket; the `Launcher`
    /// appends `--blit <socket>`, so `command` is the bare program
    /// (e.g. `mixr`). Used by mnml's `mixr.show` to bring mixr up
    /// beside the editor instead of nesting it as an mnml pty pane.
    fn open_pane_with_command(&mut self, command: String, args: Vec<String>) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        let socket_path = native_tab_socket_path(self.native_tab_nonce);
        self.native_tab_nonce = self.native_tab_nonce.saturating_add(1);
        let server = match Server::start(socket_path.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tmnl: open-pane server start failed: {e}");
                return;
            }
        };
        // cwd: reuse the editor template's workspace when there is one,
        // else the process cwd — the launched program keys off its own
        // config ($HOME), so this is just a sane default.
        let workspace = self
            .editor_template
            .as_ref()
            .map(|t| t.workspace.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
        let label = std::path::Path::new(&command)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string();
        // Record this launch in the persistent recents — the welcome
        // overlay reads from this on the next launch. De-dup + cap +
        // best-effort write live in `crate::recents`.
        crate::recents::record(crate::recents::Entry {
            command: std::path::PathBuf::from(&command),
            args: args.clone(),
            workspace: Some(workspace.clone()),
            label: Some(label.clone()),
        });
        let cfg = LauncherConfig {
            command: std::path::PathBuf::from(&command),
            workspace,
            socket: socket_path.clone(),
            extra_args: args,
        };
        let mut l = Launcher::new(cfg);
        let launcher = match l.spawn() {
            Ok(()) => Some(l),
            Err(e) => {
                eprintln!(
                    "tmnl: open-pane launch failed ({e}); start it manually with --blit {}",
                    socket_path.display()
                );
                None
            }
        };
        let mut grid = grid::Grid::new(cols, rows, palette().clear_bg);
        paint_idle(&mut grid, ConnState::Waiting, &socket_path);
        let pane = Pane {
            kind: PaneKind::Native {
                server,
                conn: ConnState::Waiting,
                launcher,
                client_title: None,
            },
            grid,
            last_cursor: None,
            label,
            attention: false,
            last_status: None,
        };
        let tab = &mut self.tabs[self.active];
        let new_id = tab.panes.len();
        tab.panes.push(pane);
        tab.layout
            .split_leaf(tab.focused, SplitDir::Vertical, new_id);
        tab.focused = new_id;
        self.relayout_all_panes();
    }

    /// Close the focused pane — its split collapses so the sibling
    /// takes the freed space. Closing a tab's last pane closes the
    /// whole tab.
    pub(crate) fn close_focused_pane(&mut self) {
        if self.tabs[self.active].panes.len() <= 1 {
            self.close_active_tab();
            return;
        }
        let tab = &mut self.tabs[self.active];
        let closed = tab.focused;
        // The pane to focus next, captured before ids shift.
        let next = tab.layout.sibling_leaf(closed);
        if !tab.layout.remove_leaf(closed) {
            return; // not a leaf in the tree (shouldn't happen)
        }
        tab.panes.remove(closed);
        tab.layout.shift_ids_after_removal(closed);
        // `next` is in pre-removal id space — shift it the same way.
        tab.focused = next
            .map(|id| if id > closed { id - 1 } else { id })
            .unwrap_or(0);
        self.relayout_all_panes();
    }

    /// Ask every connected Native pane for its command registry so
    /// the palette can aggregate them with tmnl's own commands. Fires
    /// `Message::ListClientCommands`; responses arrive asynchronously
    /// as `ServerEvent::ClientCommands` and populate
    /// `App.palette.remote_commands`. v1: queries every Native pane;
    /// multi-source aggregation across panes is a v2 follow-up.
    pub(crate) fn request_client_commands_for_palette(&mut self) {
        for tab in &mut self.tabs {
            for pane in tab.panes.iter_mut() {
                if let PaneKind::Native { server, .. } = &mut pane.kind {
                    server.send_list_client_commands();
                }
            }
        }
    }

    /// Forward a command id back to the focused Native pane via
    /// `Message::RunClientCommand`. Kept for callers that don't need
    /// per-source routing (legacy v1 path). The v2 multi-source
    /// palette flow uses [`Self::send_run_client_command_to_pane`].
    #[allow(dead_code)]
    pub(crate) fn send_run_client_command_to_focused_pane(&mut self, id: &str) {
        let tab = &mut self.tabs[self.active];
        if let PaneKind::Native { server, .. } = &mut tab.focused_pane_mut().kind {
            server.send_run_client_command(id);
        }
    }

    /// Send `RunClientCommand(id)` to a specific Native pane addressed
    /// by `(tab_idx, pane_idx)`. Used by the v2 multi-source palette
    /// routing — each remote command's source is captured when its
    /// `ClientCommands` response arrives. Falls back to a no-op when
    /// the addressed pane no longer exists or isn't Native (panes can
    /// close between the palette open and the user's Enter).
    pub(crate) fn send_run_client_command_to_pane(
        &mut self,
        tab_idx: usize,
        pane_idx: usize,
        id: &str,
    ) {
        let Some(tab) = self.tabs.get_mut(tab_idx) else {
            return;
        };
        let Some(pane) = tab.panes.get_mut(pane_idx) else {
            return;
        };
        if let PaneKind::Native { server, .. } = &mut pane.kind {
            server.send_run_client_command(id);
        }
    }

    /// Move keyboard focus to the pane nearest the focused one in
    /// `dir`. No-op if there's no pane in that direction.
    pub(crate) fn focus_dir(&mut self, dir: FocusDir) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        let area = Rect::new(0, 0, cols, rows);
        let tab = &mut self.tabs[self.active];
        let rects = tab.layout.leaf_rects(area);
        if let Some(id) = nearest_in_dir(&rects, tab.focused, dir) {
            tab.focused = id;
        }
    }

    /// The pane under the mouse cursor + the cursor translated into
    /// that pane's local cell coordinates. `None` when the cursor is
    /// on a divider or outside every pane.
    fn pane_under_cursor(&self) -> Option<(PaneId, u16, u16)> {
        let gpu = self.gpu.as_ref()?;
        let area = Rect::new(0, 0, gpu.grid.cols, gpu.grid.rows);
        let (cx, cy) = (self.cursor_cell.0 as u32, self.cursor_cell.1 as u32);
        let tab = &self.tabs[self.active];
        tab.layout
            .leaf_rects(area)
            .into_iter()
            .find(|(_, r)| r.contains(cx, cy))
            .map(|(id, r)| (id, (cx - r.x) as u16, (cy - r.y) as u16))
    }

    /// The index — into the active tab's `divider_lines` — of the
    /// divider the cursor is on, if any. Starts a drag-resize.
    fn divider_at_cursor(&self) -> Option<usize> {
        let gpu = self.gpu.as_ref()?;
        let area = Rect::new(0, 0, gpu.grid.cols, gpu.grid.rows);
        let (cx, cy) = (self.cursor_cell.0 as u32, self.cursor_cell.1 as u32);
        self.tabs[self.active]
            .layout
            .divider_lines(area)
            .iter()
            .position(|(r, _)| r.contains(cx, cy))
    }

    /// Ensure the AI completion worker thread is running.
    fn ensure_fim(&mut self) {
        if self.fim.is_none() {
            self.fim = Some(fim::FimWorker::spawn());
        }
    }

    /// The text currently typed on the shell command line — `None`
    /// unless we're in shell mode with an OSC 133 anchor.
    fn command_line(&self) -> Option<String> {
        let pane = self.tabs[self.active].focused_pane();
        let cur = pane.last_cursor?;
        let cols = pane.grid.cols.max(1);
        let row = (cur as u32 / cols) as u16;
        let col = (cur as u32 % cols) as u16;
        match &pane.kind {
            PaneKind::Shell { session: Some(s) } => s.current_command_line(&pane.grid, row, col),
            _ => None,
        }
    }

    /// ⌘I — request an AI continuation of the current command line.
    /// No-op without an OSC 133 anchor (integration snippet not
    /// installed).
    pub(crate) fn trigger_ai_completion(&mut self) {
        let Some(prefix) = self.command_line() else {
            return;
        };
        if prefix.trim().is_empty() {
            return;
        }
        self.ghost = None; // drop any stale suggestion
        self.fim_redraw = true;
        let id = self.fim_next_id;
        self.fim_next_id += 1;
        self.fim_pending = Some(PendingReq {
            id,
            erase: 0,
            below: false,
        });
        self.ensure_fim();
        if let Some(f) = &self.fim {
            f.request(id, &prefix, "");
        }
    }

    /// ⌘K — generate a shell command from the natural-language
    /// description typed on the command line (Stage 2). The reply is
    /// previewed on the row below; accepting it erases the description
    /// and types the command.
    pub(crate) fn trigger_ai_generate(&mut self) {
        let Some(raw) = self.command_line() else {
            return;
        };
        let desc = raw.trim();
        if desc.is_empty() {
            return;
        }
        self.ghost = None;
        self.fim_redraw = true;
        let id = self.fim_next_id;
        self.fim_next_id += 1;
        self.fim_pending = Some(PendingReq {
            id,
            erase: raw.chars().count(),
            below: true,
        });
        // The shebang biases the code model toward a zsh one-liner.
        let prompt = format!("#!/bin/zsh\n# {desc}\n");
        self.ensure_fim();
        if let Some(f) = &self.fim {
            f.request(id, &prompt, "\n");
        }
    }

    /// Drain AI completion replies; a reply matching the in-flight
    /// request id becomes the ghost suggestion.
    fn poll_fim(&mut self) {
        let replies = match &self.fim {
            Some(f) => f.poll(),
            None => return,
        };
        for (id, result) in replies {
            if id == fim::STATUS_ID {
                match result {
                    Ok(msg) => log::info!("fim: {msg}"),
                    Err(e) => log::warn!("fim: {e}"),
                }
                continue;
            }
            if self.fim_pending.as_ref().map(|p| p.id) != Some(id) {
                continue; // stale — the command line changed since
            }
            let pending = self.fim_pending.take().unwrap();
            // Refresh either way — clears the "generating…" placeholder
            // whether the reply yields a suggestion or not.
            self.fim_redraw = true;
            if let Ok(text) = result {
                let line = text.lines().next().unwrap_or("").trim_end();
                if !line.is_empty() {
                    self.ghost = Some(Ghost {
                        text: line.to_string(),
                        erase: pending.erase,
                        below: pending.below,
                    });
                }
            }
        }
    }

    fn tick(&mut self, event_loop: &ActiveEventLoop) {
        // Drain the menu-event channel first — `muda` delivers menu picks
        // (and accelerator-fired items like ⌘, / ⌘+ / ⌘-) through this
        // global channel. Acting on them before the per-frame work means
        // the next render reflects whatever the menu changed.
        self.drain_menu_events();
        self.poll_fim();
        // Drain pending pty-fd handoffs (task #50). Each handoff
        // produces a new adopted-shell tab in the focused window.
        self.drain_transfer_events();
        // Cheap stat() on mnml's config file — fires the heavier
        // TOML reload only when its mtime moves. Lets tmnl track
        // the user's theme choice in mnml without restart.
        if crate::theme::poll_mnml_config()
            && let Some(w) = &self.window
        {
            w.request_redraw();
        }

        if self.gpu.is_none() {
            return;
        }

        // Per-pane housekeeping: refresh each pane's strip label, drain
        // its attention flag, and keep background panes' grids current
        // (their server events + frames are applied off-screen so a
        // switch back is instant). The active tab's focused pane is
        // ticked separately below.
        let active_idx = self.active;
        // Aggregated `Message::ClientCommands` responses from every
        // Native pane (focused + secondary). Tagged with
        // `(tab_idx, pane_idx)` so the palette can route
        // `RunClientCommand` back to the source pane.
        let mut client_commands_responses: Vec<(usize, usize, Vec<tmnl_protocol::CommandInfo>)> =
            Vec::new();
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let focused = tab.focused;
            for (pi, pane) in tab.panes.iter_mut().enumerate() {
                let new_label = compute_pane_label(pane);
                if pane.label != new_label {
                    pane.label = new_label;
                }
                // Drain the shell session's attention flag (always — so
                // it doesn't accumulate). The focused-active pane keeps
                // it cleared; other panes OR it in so the chip badge
                // sticks until the user actually focuses the tab. OSC
                // 1337 also means "Claude finished", so drop the sticky
                // status cache — next tick the OSC title takes over.
                let is_focused_active = i == active_idx && pi == focused;
                if let PaneKind::Shell { session: Some(s) } = &pane.kind {
                    let new_attn = s.take_attention();
                    if new_attn {
                        pane.last_status = None;
                    }
                    if is_focused_active {
                        pane.attention = false;
                    } else if new_attn {
                        pane.attention = true;
                    }
                } else if is_focused_active {
                    pane.attention = false;
                }
                // Every pane except the active tab's focused one is
                // ticked here: Native panes always drain their server
                // events + frames; a *visible* shell pane (a split in
                // the active tab) also refreshes its grid. The focused
                // pane gets its full tick below.
                if !is_focused_active {
                    let from_secondary = tick_secondary_pane(pane, i == active_idx);
                    for items in from_secondary {
                        client_commands_responses.push((i, pi, items));
                    }
                }
            }
            // A user-set custom name wins over the auto-derived label.
            tab.label = tab
                .custom_name
                .clone()
                .unwrap_or_else(|| tab.panes[tab.focused].label.clone());
        }

        // Disambiguate duplicate labels with " (N)" — only when the
        // same exact string appears more than once. Chrome / VS Code
        // pattern: don't number eagerly, but number every occurrence
        // (including the first) once there's a collision.
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for t in &self.tabs {
            *counts.entry(t.label.as_str()).or_insert(0) += 1;
        }
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        // The tab being renamed (if any) shows its live edit buffer in
        // the chip instead of its label, with a caret.
        let rename = self
            .renaming_tab
            .as_ref()
            .map(|r| (r.tab_idx, r.buf.clone()));
        let chips: Vec<(String, bool, bool)> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if let Some((idx, buf)) = &rename
                    && *idx == i
                {
                    // Inline rename field — render active-styled so it's
                    // clearly the edit target.
                    return (format!("{buf}▏"), true, false);
                }
                let label = if counts.get(t.label.as_str()).copied().unwrap_or(0) > 1 {
                    let n = seen.entry(t.label.as_str()).or_insert(0);
                    *n += 1;
                    format!("{} ({})", t.label, n)
                } else {
                    t.label.clone()
                };
                // Attention dot is painted by the chip renderer (red `●`).
                // Cleared on switch-to.
                let attention = t.panes.iter().any(|p| p.attention) && i != self.active;
                (label, i == self.active, attention)
            })
            .collect();

        // Pick the strip height to match the current mode:
        // * multi-tab → tall strip with chip space
        // * single-tab + TUI (native or shell with altscreen)
        //   → minimal band, just enough to clear the macOS traffic
        //     lights (pre-tabs look)
        // * single-tab + bare shell prompt → taller band so the first
        //   prompt row isn't kissing the traffic lights.
        let multi_tab = self.tabs.len() > 1;
        let active_is_native = matches!(
            &self.tabs[self.active].focused_pane().kind,
            PaneKind::Native { .. }
        );
        let tui_active = active_is_native || self.altscreen_active;
        // Pre-flight the chip wrap layout to know how many rows the
        // chips will need at the current window width. The strip
        // grows by one TAB_ROW_H_PX per row of chips (palette stays
        // at its single-tab position above). Single-tab path skips
        // the layout call.
        let strip_or_sidebar_resize = {
            let gpu = self.gpu.as_mut().unwrap();
            // Make sure the GPU knows what the user picked in
            // `[tab_layout]` — settings UI may have flipped it.
            gpu.set_tab_layout(self.cfg.tab_layout);
            gpu.set_strip_chips(&chips);
            let rows = if multi_tab {
                gpu.chip_row_count(&chips)
            } else {
                1
            };
            let target_strip = Gpu::required_strip_h(self.cfg.tab_layout, &chips, rows, tui_active);
            let target_sidebar =
                if matches!(self.cfg.tab_layout, crate::config::TabLayout::Vertical) && multi_tab {
                    gpu.compute_sidebar_w_px(&chips)
                } else {
                    0.0
                };
            let r1 = gpu.set_strip_h(target_strip);
            let r2 = gpu.set_sidebar_w_px(target_sidebar);
            r1.or(r2)
        };
        if strip_or_sidebar_resize.is_some() {
            self.relayout_all_panes();
        }

        // Shell mode only: auto-switch the inset when a full-screen TUI
        // takes the alt-screen (edge-to-edge), and back on exit.
        let altscreen = match &self.tabs[self.active].focused_pane().kind {
            PaneKind::Shell { session: Some(s) } => Some(s.altscreen_active()),
            _ => None,
        };
        if let Some(altscreen) = altscreen
            && altscreen != self.altscreen_active
        {
            self.altscreen_active = altscreen;
            let target_inset = self.cfg.active_inset(altscreen);
            let inset_resize = self.gpu.as_mut().unwrap().set_inset_px(target_inset);
            if inset_resize.is_some() {
                self.relayout_all_panes();
            }
        }

        // Sibling-pane requests from the focused native client
        // (`Message::OpenPane`) — collected during the focused-pane
        // tick, applied below once the pane borrow is released.
        let mut open_pane_reqs: Vec<(String, Vec<String>)> = Vec::new();
        let mut host_command_reqs: Vec<String> = Vec::new();
        // (client_commands_responses is declared at the top of tick
        // so the secondary-pane loop can push into it too.)

        // Tick the active tab's focused pane.
        let want_ghost_refresh = self.fim_redraw;
        let focused = self.tabs[self.active].focused;
        {
            let Pane {
                kind,
                grid,
                last_cursor,
                ..
            } = &mut self.tabs[self.active].panes[focused];
            match kind {
                PaneKind::Shell { session } => {
                    let Some(s) = session.as_mut() else {
                        return;
                    };
                    if s.exited() {
                        self.should_quit = true;
                        return;
                    }
                    if s.dirty() || want_ghost_refresh {
                        let (cc, cr, vis) = s.apply_to_grid(grid);
                        *last_cursor = None; // shell tracks the cursor via apply_to_grid
                        if vis && (cc as u32) < grid.cols && (cr as u32) < grid.rows {
                            let i = (cr as u32 * grid.cols + cc as u32) as usize;
                            // Suppress the cursor at (0,0) on a default-empty
                            // cell — vt100's "just spawned, no output" state.
                            let suppress = cc == 0 && cr == 0 && {
                                let cell = &grid.cells[i];
                                cell.ch == ' ' && cell.attrs == 0
                            };
                            if !suppress {
                                grid.cells[i].attrs |= ATTR_CURSOR_BLOCK;
                                *last_cursor = Some(i);
                            }
                        }
                    }
                }
                PaneKind::Native {
                    server,
                    conn,
                    launcher,
                    client_title,
                } => {
                    while let Ok(ev) = server.events.try_recv() {
                        match ev {
                            ServerEvent::ClientConnected => {
                                *conn = ConnState::Connected;
                                *client_title = None; // fresh connection, fresh title
                                server.send_resize(grid.cols as u16, grid.rows as u16);
                                paint_idle(grid, *conn, &server.socket_path);
                            }
                            ServerEvent::ClientDisconnected => {
                                *conn = ConnState::Waiting;
                                *client_title = None;
                                paint_idle(grid, *conn, &server.socket_path);
                            }
                            ServerEvent::Title(s) => {
                                *client_title = Some(s);
                            }
                            ServerEvent::OpenPane { command, args } => {
                                open_pane_reqs.push((command, args));
                            }
                            ServerEvent::RunHostCommand(id) => {
                                host_command_reqs.push(id);
                            }
                            ServerEvent::ClientCommands(items) => {
                                client_commands_responses.push((self.active, focused, items));
                            }
                        }
                    }
                    // Diffs are cumulative — apply every frame in order.
                    let mut applied = 0u32;
                    while let Ok(f) = server.frame_rx.try_recv() {
                        if *conn != ConnState::Streaming {
                            *conn = ConnState::Streaming;
                        }
                        apply_frame_to_grid(grid, last_cursor, &f);
                        applied += 1;
                    }
                    if applied > 0 {
                        log::debug!("[tick] applied {applied} frame(s)");
                    }
                    if let Some(l) = launcher.as_mut() {
                        match l.poll() {
                            LauncherPoll::Running | LauncherPoll::Idle => {}
                            LauncherPoll::Restart => {
                                log::info!("launcher: restart requested (exit 75); respawning");
                                if let Err(e) = l.spawn() {
                                    eprintln!("tmnl: failed to respawn child: {e}");
                                    self.should_quit = true;
                                }
                            }
                            LauncherPoll::Exited(code) => {
                                log::info!(
                                    "launcher: child exited with code {code}; closing window"
                                );
                                self.should_quit = true;
                            }
                        }
                    }
                }
                PaneKind::Browser { .. } => {
                    // Phase 1: focused-pane work is a no-op (placeholder
                    // grid stays put). Phase 2 will route mouse/keyboard
                    // events into the wry WebView's NSView/HWND.
                }
            }
        }

        // A focused native client asked to open a sibling pane
        // (mnml's `mixr.show`) — honor it now the pane borrow is
        // released.
        for (command, args) in open_pane_reqs.drain(..) {
            self.open_pane_with_command(command, args);
        }
        // Same shape for RunHostCommand — fire each command via the
        // registry. Use case: mnml's left-rail INTEGRATIONS chips
        // with `command = "tmnl:browser.attach_dashboard"` etc.
        for id in host_command_reqs.drain(..) {
            if !crate::command::dispatch_by_id(&id, self, event_loop) {
                eprintln!("tmnl: RunHostCommand unknown or guarded id {id:?}");
            }
        }
        // Drop any ClientCommands responses straight onto the palette
        // (if one is open). Multiple panes may respond — we concat
        // v2 multi-source: tag each command with `(tab_idx, pane_idx)`
        // so the palette's Enter handler can route `RunClientCommand`
        // back to the exact source pane. De-dup by `(source, id)` so
        // a pane re-responding to ListClientCommands doesn't show
        // double rows, but different panes' identically-named commands
        // remain distinct entries (rare in practice — mnml's
        // `git.commit` wouldn't collide with mixr's `deck.play`).
        if let Some(st) = self.palette.as_mut() {
            for (tab_idx, pane_idx, items) in client_commands_responses.drain(..) {
                for info in items {
                    if !st.remote_commands.iter().any(|r| {
                        r.source_tab == tab_idx && r.source_pane == pane_idx && r.info.id == info.id
                    }) {
                        st.remote_commands.push(crate::palette::RemoteCommand {
                            info,
                            source_tab: tab_idx,
                            source_pane: pane_idx,
                        });
                    }
                }
            }
        }

        // Overlay the AI ghost suggestion, or a "generating…"
        // placeholder while a request is in flight (dim) — written into
        // the pane grid so the compositor picks it up.
        {
            let pane = &mut self.tabs[self.active].panes[focused];
            if let PaneKind::Shell { session } = &pane.kind
                && let Some(cur) = pane.last_cursor
            {
                let cols = (pane.grid.cols as usize).max(1);
                // Stage 2 (`below`) renders on the row below, aligned
                // under the command-line input start; Stage 1 at the
                // cursor.
                let anchor_col = session
                    .as_ref()
                    .and_then(|s| s.input_anchor())
                    .map_or(0, |(_, c)| c as usize);
                let below_at = (cur / cols + 1) * cols + anchor_col;
                if let Some(g) = &self.ghost {
                    let at = if g.below { below_at } else { cur };
                    draw_ghost(&mut pane.grid, at, &g.text);
                    // Accept hint, a couple of cells past the suggestion.
                    draw_ghost(&mut pane.grid, at + g.text.chars().count() + 2, "[tab]");
                } else if let Some(p) = &self.fim_pending {
                    let at = if p.below { below_at } else { cur };
                    draw_ghost(&mut pane.grid, at, "generating…");
                }
            }
        }
        self.fim_redraw = false;
    }

    /// Pick up menu events fired since the last tick (selections + chord
    /// accelerators both land here). Drain into a Vec first so we can
    /// dispatch with `&mut self` afterwards (the menu borrow + a mutable
    /// self borrow would otherwise conflict).
    fn drain_menu_events(&mut self) {
        if self.app_menu.is_none() {
            return;
        }
        let ids: Vec<muda::MenuId> = std::iter::from_fn(|| {
            muda::MenuEvent::receiver()
                .try_recv()
                .ok()
                .map(|e| e.id().clone())
        })
        .collect();
        let menu = self.app_menu.as_ref().unwrap().clone_ids();
        for id in ids {
            // Split-pane items carry plain string IDs (see `menu.rs`).
            if id == "split_right" {
                self.split_active_pane(SplitDir::Vertical);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                continue;
            } else if id == "split_down" {
                self.split_active_pane(SplitDir::Horizontal);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                continue;
            } else if id == "focus_left" {
                self.focus_dir(FocusDir::Left);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                continue;
            } else if id == "focus_right" {
                self.focus_dir(FocusDir::Right);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                continue;
            } else if id == "focus_up" {
                self.focus_dir(FocusDir::Up);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                continue;
            } else if id == "focus_down" {
                self.focus_dir(FocusDir::Down);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                continue;
            } else if id == "close_pane" {
                self.close_focused_pane();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                continue;
            }
            if id == menu.id_settings {
                self.open_settings();
            } else if id == menu.id_new_window {
                log::info!("menu: New Window — placeholder, not wired yet");
            } else if id == menu.id_font_inc {
                self.zoom_font(FONT_ZOOM_STEP);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            } else if id == menu.id_font_dec {
                self.zoom_font(-FONT_ZOOM_STEP);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            } else if id == menu.id_font_reset {
                self.reset_font_zoom();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            } else if id == menu.id_toggle_fullscreen {
                if let Some(w) = self.window.as_ref() {
                    let next = if w.fullscreen().is_some() {
                        None
                    } else {
                        Some(winit::window::Fullscreen::Borderless(None))
                    };
                    w.set_fullscreen(next);
                }
            } else if id == menu.id_tmnl_help {
                log::info!("menu: tmnl Help — placeholder, not wired yet");
            } else {
                log::debug!("menu: unhandled id {id:?}");
            }
        }
    }

    fn open_settings(&mut self) {
        if self.settings.is_none() {
            self.settings = Some(SettingsState::open(self.cfg.clone()));
        }
    }

    /// Apply the inset from the (possibly edited) config to gpu + grid,
    /// and propagate the new dimensions to whatever's filling the grid
    /// — mnml/mixr (via the wire `Resize` message) or the shell's pty
    /// (via `ShellSession::resize`). Without the propagation the source
    /// of cells keeps writing to the *old* col count and overflows past
    /// the now-smaller grid.
    ///
    /// TUIs always render at 0: native mode is a TUI by definition;
    /// shell mode with the alt-screen active hosts one. Only the shell
    /// prompt view uses the configured value.
    fn apply_inset_from_cfg(&mut self, cfg: &Config) {
        let native = matches!(
            &self.tabs[self.active].focused_pane().kind,
            PaneKind::Native { .. }
        );
        let tui_active = native || self.altscreen_active;
        let new_inset = cfg.active_inset(tui_active);
        let resize = match self.gpu.as_mut() {
            Some(gpu) => gpu.set_inset_px(new_inset),
            None => return,
        };
        if resize.is_some() {
            self.relayout_all_panes();
        }
    }

    /// Route a keystroke into the welcome overlay. Returns true if the
    /// key was consumed (a digit-pick, ↑/↓ selection move, Enter open,
    /// `r` drop, or `Esc`/`n` dismiss).
    fn welcome_handle_key(&mut self, ke: &winit::event::KeyEvent) -> bool {
        if self.welcome.is_none() {
            return false;
        }
        use winit::keyboard::Key;
        match &ke.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.welcome = None;
                true
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(st) = self.welcome.as_ref()
                    && let Some(entry) = st.entries.get(st.selected).cloned()
                {
                    self.welcome = None;
                    self.open_recent_entry(entry);
                }
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(st) = self.welcome.as_mut() {
                    st.move_selection(-1);
                }
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                if let Some(st) = self.welcome.as_mut() {
                    st.move_selection(1);
                }
                true
            }
            Key::Character(s) => match s.as_str() {
                "k" => {
                    if let Some(st) = self.welcome.as_mut() {
                        st.move_selection(-1);
                    }
                    true
                }
                "j" => {
                    if let Some(st) = self.welcome.as_mut() {
                        st.move_selection(1);
                    }
                    true
                }
                "n" => {
                    // Dismiss — drop into the shell pane that's already
                    // there underneath.
                    self.welcome = None;
                    true
                }
                "r" => {
                    // Drop the selected entry from the recents file.
                    // Re-load so the welcome list stays in sync.
                    if let Some(st) = self.welcome.as_mut() {
                        if let Some(entry) = st.entries.get(st.selected).cloned() {
                            // Remove it from disk via a noop append +
                            // immediate save with the entry filtered out.
                            let mut entries = crate::recents::load();
                            entries.retain(|e| {
                                !(e.command == entry.command
                                    && e.args == entry.args
                                    && e.workspace == entry.workspace)
                            });
                            // Save with the rest.
                            for e in entries.iter().rev() {
                                crate::recents::record(e.clone());
                            }
                            // Re-pull the now-trimmed list.
                            st.entries = crate::recents::load();
                            if st.selected >= st.entries.len() && !st.entries.is_empty() {
                                st.selected = st.entries.len() - 1;
                            }
                            // If nothing's left, just close the overlay.
                            if st.entries.is_empty() {
                                self.welcome = None;
                            }
                        }
                    }
                    true
                }
                // 1..9 digit picker.
                d if d.len() == 1 => {
                    let c = d.chars().next().unwrap();
                    if let Some(digit) = c.to_digit(10)
                        && (1..=9).contains(&digit)
                    {
                        if let Some(st) = self.welcome.as_ref()
                            && let Some(idx) = st.pick_by_digit(digit as u8)
                            && let Some(entry) = st.entries.get(idx).cloned()
                        {
                            self.welcome = None;
                            self.open_recent_entry(entry);
                        }
                        true
                    } else {
                        // Any other printable — swallow it so it doesn't
                        // type into the shell underneath. The welcome
                        // modal is the focus; only its keys matter.
                        true
                    }
                }
                _ => true,
            },
            _ => true,
        }
    }

    /// Resolve a `recents::Entry` into a "replace the active tab's
    /// focused pane with this native client" action. Shared between
    /// the digit-pick and Enter paths in `welcome_handle_key`.
    ///
    /// Why replace instead of split: the welcome screen runs on
    /// startup against a fresh shell tab. The user picking mixr
    /// expects "open mixr" — same window, no leftover split with the
    /// throwaway shell on the side. mnml's `mixr.show` (which DOES
    /// want a split next to the editor) goes through a different
    /// path — `open_pane_with_command`.
    fn open_recent_entry(&mut self, entry: crate::recents::Entry) {
        let command = entry.command.to_string_lossy().into_owned();
        // Honor the per-entry workspace — that's the whole point of
        // pinning a recent at a specific repo. Without this, picking
        // entry 2 (`mnml ~/Projects/tmnl`) opens mnml at whatever
        // editor_template / current_dir resolves to (`/` when tmnl.app
        // is launched from /Applications), which surfaces as the
        // "mnml opened in the wrong folder" bug.
        self.replace_focused_pane_with_native(command, entry.args, entry.workspace);
    }

    /// Swap the active tab's focused pane for a freshly-launched
    /// native pane running `command args…`. Used by the welcome
    /// screen — see [`Self::open_recent_entry`].
    fn replace_focused_pane_with_native(
        &mut self,
        command: String,
        args: Vec<String>,
        // Per-call override — set by `open_recent_entry` to honor the
        // recents entry's pinned workspace. When `None`, falls back to
        // the editor_template's workspace (the path tmnl was launched
        // with) and finally current_dir.
        workspace_override: Option<std::path::PathBuf>,
    ) {
        let (cols, rows) = match self.gpu.as_ref() {
            Some(gpu) => (gpu.grid.cols, gpu.grid.rows),
            None => return,
        };
        let socket_path = native_tab_socket_path(self.native_tab_nonce);
        self.native_tab_nonce = self.native_tab_nonce.saturating_add(1);
        let server = match Server::start(socket_path.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("tmnl: replace-pane server start failed: {e}");
                return;
            }
        };
        let workspace = workspace_override
            .or_else(|| self.editor_template.as_ref().map(|t| t.workspace.clone()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
        let label = std::path::Path::new(&command)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string();
        // Record the launch in the persistent recents — same as the
        // split-mode `open_pane_with_command`.
        crate::recents::record(crate::recents::Entry {
            command: std::path::PathBuf::from(&command),
            args: args.clone(),
            workspace: Some(workspace.clone()),
            label: Some(label.clone()),
        });
        let cfg = LauncherConfig {
            command: std::path::PathBuf::from(&command),
            workspace,
            socket: socket_path.clone(),
            extra_args: args,
        };
        let mut l = Launcher::new(cfg);
        let launcher = match l.spawn() {
            Ok(()) => Some(l),
            Err(e) => {
                eprintln!(
                    "tmnl: replace-pane launch failed ({e}); start it manually with --blit {}",
                    socket_path.display()
                );
                None
            }
        };
        let mut grid = grid::Grid::new(cols, rows, palette().clear_bg);
        paint_idle(&mut grid, ConnState::Waiting, &socket_path);
        let pane = Pane {
            kind: PaneKind::Native {
                server,
                conn: ConnState::Waiting,
                launcher,
                client_title: None,
            },
            grid,
            last_cursor: None,
            label,
            attention: false,
            last_status: None,
        };
        // Drop the old pane in place; the layout slot keeps its
        // existing id so the tab tree stays intact. The old pane's
        // Drop handles cleanup (ShellSession terminates the shell,
        // Native sends Quit + waits).
        let tab = &mut self.tabs[self.active];
        let id = tab.focused;
        if id < tab.panes.len() {
            tab.panes[id] = pane;
        } else {
            // Shouldn't happen — `focused` is always a valid index.
            // Fall back to push if it ever does.
            tab.panes.push(pane);
            tab.focused = tab.panes.len() - 1;
        }
        // Update the tab label too — it usually follows the focused
        // pane's label.
        tab.label = tab.panes[tab.focused].label.clone();
        self.relayout_all_panes();
    }

    /// Route a keystroke into the help overlay. Returns true if the
    /// key was consumed. Esc / `?` close; arrows + PageUp/PageDown
    /// scroll. Greedy while the overlay is open so a stray ⌘T can't
    /// open a tab behind it.
    pub(crate) fn help_handle_key(&mut self, ke: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let Some(st) = self.help.as_mut() else {
            return false;
        };
        let consumed = match &ke.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.help = None;
                true
            }
            Key::Character(s) if s.as_str() == "?" => {
                self.help = None;
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                st.scroll(-1);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                st.scroll(1);
                true
            }
            Key::Character(s) if s.as_str() == "k" => {
                st.scroll(-1);
                true
            }
            Key::Character(s) if s.as_str() == "j" => {
                st.scroll(1);
                true
            }
            Key::Named(NamedKey::PageUp) => {
                st.scroll(-10);
                true
            }
            Key::Named(NamedKey::PageDown) => {
                st.scroll(10);
                true
            }
            _ => false,
        };
        if consumed && let Some(w) = &self.window {
            w.request_redraw();
        }
        consumed
    }

    /// Greedy modal handler for the command palette. Returns true
    /// when the key was consumed (so the regular keymap doesn't
    /// double-fire).
    pub(crate) fn palette_handle_key(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        ke: &winit::event::KeyEvent,
    ) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let Some(st) = self.palette.as_mut() else {
            return false;
        };
        let consumed = match &ke.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.palette = None;
                true
            }
            Key::Named(NamedKey::Enter) => {
                // Snapshot the entry before dropping the modal —
                // local entries route through the local dispatcher;
                // remote entries are forwarded to the focused Native
                // pane via `send_run_client_command`. Closing the
                // palette first releases the &mut borrow on `self`.
                let entry = st.current_entry();
                self.palette = None;
                match entry {
                    Some(crate::palette::PaletteEntry::Local(id)) => {
                        let id_owned = id.to_string();
                        crate::command::dispatch_by_id(&id_owned, self, event_loop);
                    }
                    Some(crate::palette::PaletteEntry::Remote(rc)) => {
                        self.send_run_client_command_to_pane(
                            rc.source_tab,
                            rc.source_pane,
                            &rc.info.id,
                        );
                    }
                    None => {}
                }
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                st.move_selection(-1);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                st.move_selection(1);
                true
            }
            Key::Named(NamedKey::Backspace) => {
                st.backspace();
                true
            }
            Key::Character(s) => {
                for c in s.chars() {
                    st.insert_char(c);
                }
                true
            }
            _ => false,
        };
        if consumed && let Some(w) = &self.window {
            w.request_redraw();
        }
        consumed
    }

    /// Route a keystroke into the Settings modal. Returns true if the
    /// key was consumed (mode-specific handlers should skip it).
    fn settings_handle_key(&mut self, ke: &winit::event::KeyEvent) -> bool {
        let Some(st) = self.settings.as_mut() else {
            return false;
        };
        use winit::keyboard::Key;
        match &ke.logical_key {
            Key::Named(NamedKey::Escape) => {
                let original = st.original.clone();
                self.settings = None;
                self.cfg = original.clone();
                self.apply_inset_from_cfg(&original);
                true
            }
            Key::Named(NamedKey::Enter) => {
                let edited = st.cfg.clone();
                self.settings = None;
                self.cfg = edited.clone();
                if let Err(e) = self.cfg.save() {
                    log::warn!("config save failed: {e}");
                }
                self.apply_inset_from_cfg(&edited);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                st.focus_prev();
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                st.focus_next();
                true
            }
            Key::Named(NamedKey::ArrowLeft) => {
                st.nudge(-1.0);
                let edited = st.cfg.clone();
                self.cfg = edited.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                st.nudge(1.0);
                let edited = st.cfg.clone();
                self.cfg = edited.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                st.reset_row();
                let edited = st.cfg.clone();
                self.cfg = edited.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            // `r` — reset focused row (family convention). `⌫` above is
            // kept as an alias for muscle memory.
            Key::Character(s) if s.as_str() == "r" => {
                st.reset_row();
                let edited = st.cfg.clone();
                self.cfg = edited.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            // `R` (shift+r) — reset all.
            Key::Character(s) if s.as_str() == "R" => {
                st.reset_all();
                let edited = st.cfg.clone();
                self.cfg = edited.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            // Every other key gets eaten so it doesn't reach the shell
            // / native target underneath.
            _ => true,
        }
    }

    /// Begin renaming tab `idx` — its strip chip becomes an inline text
    /// field. Any rename already in progress is committed first.
    fn start_rename(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.renaming_tab.is_some() {
            self.commit_rename();
        }
        // Seed with the current custom name (empty if never renamed) so
        // the user can tweak it or clear it back to the auto label.
        let buf = self.tabs[idx].custom_name.clone().unwrap_or_default();
        self.renaming_tab = Some(RenameState { tab_idx: idx, buf });
    }

    /// Commit the in-progress rename: a non-empty buffer becomes the
    /// tab's `custom_name`; an empty buffer clears it (reverts to the
    /// auto-derived label). No-op when nothing is being renamed.
    fn commit_rename(&mut self) {
        let Some(st) = self.renaming_tab.take() else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(st.tab_idx) {
            tab.custom_name = committed_tab_name(&st.buf);
        }
    }

    /// Abandon the in-progress rename without changing the tab name.
    fn cancel_rename(&mut self) {
        self.renaming_tab = None;
    }

    /// Feed a key to the in-progress tab rename. Returns `true` while a
    /// rename is active — the key is consumed either way so it can't
    /// leak to the hosted process. `false` ⇒ no rename in progress,
    /// the caller handles the key normally.
    fn rename_handle_key(&mut self, ke: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::Key;
        if self.renaming_tab.is_none() {
            return false;
        }
        match &ke.logical_key {
            Key::Named(NamedKey::Escape) => self.cancel_rename(),
            Key::Named(NamedKey::Enter) => self.commit_rename(),
            Key::Named(NamedKey::Backspace) => {
                if let Some(st) = self.renaming_tab.as_mut() {
                    st.buf.pop();
                }
            }
            _ => {
                // Append the key's resolved text (layout + shift
                // applied), skipping control chars.
                if let Some(txt) = &ke.text
                    && let Some(st) = self.renaming_tab.as_mut()
                {
                    for ch in txt.chars().filter(|c| !c.is_control()) {
                        st.buf.push(ch);
                    }
                }
            }
        }
        true
    }

    fn shutdown_gracefully(&mut self) {
        match &mut self.tabs[self.active].focused_pane_mut().kind {
            PaneKind::Shell { session } => {
                // Drop the ShellSession — its Drop hardkills the child.
                *session = None;
            }
            PaneKind::Native {
                server, launcher, ..
            } => {
                server.send_quit();
                if let Some(l) = launcher.as_mut() {
                    let _ = l.wait_for_exit(std::time::Duration::from_millis(1200));
                }
            }
            PaneKind::Browser { webview, .. } => {
                // Drop the wry WebView (no-op in Phase 1 — `webview`
                // is the unit-type placeholder).
                *webview = None;
            }
        }
    }

    fn handle_resized(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        let resize = {
            let Some(gpu) = self.gpu.as_mut() else {
                return;
            };
            gpu.resize(size.width, size.height)
        };
        // Always paint a frame after a resize — the surface was
        // reconfigured (even if cols×rows stayed the same), so
        // the framebuffer is fresh and would briefly show through
        // as palette().clear_bg until the next event-driven render. Without
        // this the window flickers during interactive resizes.
        if let Some(w) = &self.window {
            w.request_redraw();
        }
        // Re-lay-out every tab's panes to the new window size.
        if resize.is_some() {
            self.relayout_all_panes();
        }
    }

    fn handle_keyboard_input(&mut self, event_loop: &ActiveEventLoop, ke: winit::event::KeyEvent) {
        if ke.state != ElementState::Pressed {
            return;
        }
        // Welcome overlay swallows keys while open — first priority so
        // a quick `1`/`2`/Esc lands before any tab-management chord
        // tries to interpret the keystroke.
        if self.welcome.is_some() && self.welcome_handle_key(&ke) {
            return;
        }
        // Settings panel swallows keys while open (Esc closes,
        // Enter saves; everything else either adjusts the
        // selected field or no-ops).
        if self.settings.is_some() && self.settings_handle_key(&ke) {
            return;
        }
        // A tab rename swallows keys while active — Esc cancels,
        // Enter commits, Backspace deletes, everything else
        // types into the name.
        if self.renaming_tab.is_some() && self.rename_handle_key(&ke) {
            return;
        }
        // Help overlay: Esc / ? close, arrows scroll. Greedy while
        // open so a stray ⌘T doesn't open a tab behind it.
        if self.help.is_some() && self.help_handle_key(&ke) {
            return;
        }
        // Command palette: typed text refines filter, Enter
        // dispatches. Greedy while open.
        if self.palette.is_some() && self.palette_handle_key(event_loop, &ke) {
            return;
        }
        // Command-registry dispatch — chords migrated to
        // `crate::command::builtin_commands` are handled here. See
        // `docs/COMMAND_MIGRATION.md`.
        if crate::command::try_dispatch(&ke, self, event_loop) {
            return;
        }
        // ── Every Cmd-prefixed chord migrated to the registry. ──
        // The legacy `if self.mods.super_key() && ... { match ... }`
        // block here is gone (preserved in docs/COMMAND_MIGRATION.md).
        // Same for ⌘⌥+Arrow / ⌘I/K / Shift+PgUp+Dn — all now in
        // `command::builtin_commands`, dispatched by `try_dispatch`
        // at the top of this function.
        let focused = self.tabs[self.active].focused;
        match &mut self.tabs[self.active].panes[focused].kind {
            PaneKind::Shell { session } => {
                if let Some(s) = session.as_mut() {
                    // Any keystroke cancels an in-flight AI
                    // request — the command line is changing.
                    if self.fim_pending.take().is_some() {
                        self.fim_redraw = true;
                    }
                    // An active ghost suggestion intercepts Tab
                    // (accept); any other key dismisses it, then
                    // is forwarded to the shell normally.
                    let mut consumed = false;
                    if self.ghost.is_some() {
                        if matches!(&ke.logical_key, Key::Named(winit::keyboard::NamedKey::Tab)) {
                            if let Some(g) = self.ghost.take() {
                                // Stage 2 replaces the typed
                                // description; Stage 1 appends.
                                if g.erase > 0 {
                                    s.write_bytes(&vec![0x7f; g.erase]);
                                }
                                s.write_bytes(g.text.as_bytes());
                            }
                            consumed = true;
                        } else {
                            self.ghost = None;
                        }
                        self.fim_redraw = true;
                    }
                    if !consumed && let Some(bytes) = winit_key_to_bytes(&ke.logical_key, self.mods)
                    {
                        s.scroll_reset(); // typing snaps to the bottom
                        s.write_bytes(&bytes);
                    }
                }
            }
            PaneKind::Native { server, .. } => {
                if let Some(code) = translate_key(&ke.logical_key, self.mods) {
                    let input = InputEvent::Key(KeyInput {
                        code,
                        mods: pack_mods(self.mods),
                        press: true,
                    });
                    server.send_input(&input);
                }
            }
            PaneKind::Browser {
                url,
                webview,
                chrome,
            } => {
                // URL bar edit mode: keys go to the edit buffer, not
                // the WebView. The WebView itself receives keyboard
                // input directly from the OS (focused NSView /
                // GtkWidget / HWND) — so when the bar isn't being
                // edited we don't need to forward anything.
                if let Some(edit) = chrome.edit.as_mut() {
                    use winit::keyboard::NamedKey;
                    let mut redraw = false;
                    match &ke.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            chrome.edit = None;
                            chrome.cursor = 0;
                            redraw = true;
                        }
                        Key::Named(NamedKey::Enter) => {
                            // Commit: load whatever's typed. If it
                            // doesn't look like a URL, prepend https://
                            // — same heuristic as a browser's address
                            // bar (clipboard split uses the same one).
                            let raw = edit.trim().to_string();
                            let candidate = if raw.starts_with("http://")
                                || raw.starts_with("https://")
                                || raw.starts_with("about:")
                                || raw.starts_with("file://")
                            {
                                raw
                            } else if raw.contains('.') {
                                format!("https://{raw}")
                            } else {
                                // No dot → search query. DuckDuckGo
                                // is the default landing page; route
                                // queries there for consistency.
                                format!(
                                    "https://duckduckgo.com/?q={}",
                                    crate::url_query_encode(&raw)
                                )
                            };
                            if let Some(v) = webview.as_ref() {
                                let _ = v.load_url(&candidate);
                            }
                            *url = candidate;
                            chrome.edit = None;
                            chrome.cursor = 0;
                            redraw = true;
                        }
                        Key::Named(NamedKey::Backspace) => {
                            if chrome.cursor > 0 {
                                // Convert cursor (in chars) to byte
                                // index, drop the previous char.
                                let byte = edit
                                    .char_indices()
                                    .nth(chrome.cursor - 1)
                                    .map(|(b, _)| b)
                                    .unwrap_or(0);
                                let end = edit
                                    .char_indices()
                                    .nth(chrome.cursor)
                                    .map(|(b, _)| b)
                                    .unwrap_or_else(|| edit.len());
                                edit.replace_range(byte..end, "");
                                chrome.cursor -= 1;
                                redraw = true;
                            }
                        }
                        Key::Named(NamedKey::Delete) => {
                            if chrome.cursor < edit.chars().count() {
                                let byte = edit
                                    .char_indices()
                                    .nth(chrome.cursor)
                                    .map(|(b, _)| b)
                                    .unwrap_or(0);
                                let end = edit
                                    .char_indices()
                                    .nth(chrome.cursor + 1)
                                    .map(|(b, _)| b)
                                    .unwrap_or_else(|| edit.len());
                                edit.replace_range(byte..end, "");
                                redraw = true;
                            }
                        }
                        Key::Named(NamedKey::ArrowLeft) => {
                            if chrome.cursor > 0 {
                                chrome.cursor -= 1;
                                redraw = true;
                            }
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            if chrome.cursor < edit.chars().count() {
                                chrome.cursor += 1;
                                redraw = true;
                            }
                        }
                        Key::Named(NamedKey::Home) => {
                            chrome.cursor = 0;
                            redraw = true;
                        }
                        Key::Named(NamedKey::End) => {
                            chrome.cursor = edit.chars().count();
                            redraw = true;
                        }
                        _ => {
                            // Typed text — only commit printable chars.
                            if let Some(text) = &ke.text
                                && !text.is_empty()
                            {
                                let byte = edit
                                    .char_indices()
                                    .nth(chrome.cursor)
                                    .map(|(b, _)| b)
                                    .unwrap_or_else(|| edit.len());
                                edit.insert_str(byte, text);
                                chrome.cursor += text.chars().count();
                                redraw = true;
                            }
                        }
                    }
                    if redraw {
                        self.relayout_all_panes();
                    }
                }
                // No edit in progress → keys fall on the floor at the
                // app layer; the WebView's native widget already has
                // OS-level keyboard focus and handles its own input.
            }
        }
    }

    fn handle_cursor_moved(&mut self, position: winit::dpi::PhysicalPosition<f64>) {
        // Snapshot what we need off the GPU up front so the
        // borrow is released before `relayout_all_panes` (which
        // needs `&mut self`).
        let Some((cell, (gcols, grows), chrome_h, sidebar_w)) = self.gpu.as_ref().map(|g| {
            (
                g.pixel_to_cell(position.x, position.y),
                (g.grid.cols, g.grid.rows),
                (g.inset_px + g.strip_h) as f64,
                (g.inset_px + g.sidebar_w_px) as f64,
            )
        }) else {
            return;
        };
        let prev = self.cursor_cell;
        self.cursor_cell = cell;
        self.cursor_px = (position.x, position.y);
        let (col, row) = cell;
        // Treat the left-edge sidebar (when active) as chrome too so
        // hover drags + chip-reorder respect it. `sidebar_w` is the
        // base inset alone in horizontal mode, so the check collapses
        // to `position.x < 0` (always false) and is a no-op there.
        let in_chrome = position.y < chrome_h
            || (sidebar_w > self.gpu.as_ref().unwrap().inset_px as f64 && position.x < sidebar_w);

        // A divider drag owns the event while it's armed — move
        // the split's ratio so the divider tracks the cursor.
        if let Some(idx) = self.dragging_divider {
            if !in_chrome && self.buttons_down & (1u8 << BUTTON_LEFT) != 0 {
                let area = Rect::new(0, 0, gcols, grows);
                self.tabs[self.active]
                    .layout
                    .resize_split_at(area, idx, col as u32, row as u32);
                self.relayout_all_panes();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            return;
        }

        // Drags that originated in the strip stay in chrome —
        // don't forward them as terminal drag events.
        if in_chrome {
            // Drag-to-reorder: while a chip drag is armed and the
            // cursor crosses into a different chip's rect, swap
            // the two tabs.
            if let Some(src) = self.dragging_tab
                && self.buttons_down & (1u8 << BUTTON_LEFT) != 0
                && let Some(gpu) = &self.gpu
            {
                let dst = gpu
                    .strip_chip_rects
                    .iter()
                    .find(|(x0, x1, y0, y1, _)| {
                        position.x >= *x0 as f64
                            && position.x < *x1 as f64
                            && position.y >= *y0 as f64
                            && position.y < *y1 as f64
                    })
                    .map(|(_, _, _, _, idx)| *idx);
                if let Some(dst) = dst
                    && dst != src
                    && dst < self.tabs.len()
                {
                    self.tabs.swap(src, dst);
                    if self.active == src {
                        self.active = dst;
                    } else if self.active == dst {
                        self.active = src;
                    }
                    self.dragging_tab = Some(dst);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            return;
        }
        // Route hover / drag to the pane under the cursor, in
        // that pane's local coordinates. A Native client needs
        // Moved events for hover tooltips / divider highlights;
        // a plain shell only gets motion under ?1003h but a
        // Native tab always wants them.
        if prev != self.cursor_cell
            && let Some((id, lc, lr)) = self.pane_under_cursor()
        {
            let held = self.buttons_down != 0;
            let mouse = MouseInput {
                kind: if held {
                    MouseKind::Drag
                } else {
                    MouseKind::Moved
                },
                button: if held {
                    first_button(self.buttons_down)
                } else {
                    BUTTON_NONE
                },
                col: lc,
                row: lr,
                mods: pack_mods(self.mods),
            };
            match &mut self.tabs[self.active].panes[id].kind {
                PaneKind::Native { server, .. } => {
                    server.send_input(&InputEvent::Mouse(mouse));
                }
                // Forward hover + drag to a pty child when it has
                // requested motion tracking (DECSET 1002 / 1003).
                // No tracking ⇒ `write_mouse_motion` no-ops silently
                // (no garbage on stdin from idle hover).
                PaneKind::Shell { session: Some(s) } => {
                    let held_button = if held {
                        Some(first_button(self.buttons_down))
                    } else {
                        None
                    };
                    s.write_mouse_motion(lc, lr, held_button, pack_mods(self.mods));
                }
                _ => {}
            }
        }
    }

    fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        let b = match button {
            MouseButton::Left => BUTTON_LEFT,
            MouseButton::Right => BUTTON_RIGHT,
            MouseButton::Middle => BUTTON_MIDDLE,
            _ => BUTTON_NONE,
        };
        let pressed = state == ElementState::Pressed;
        if pressed {
            self.buttons_down |= 1u8 << b;
        } else {
            self.buttons_down &= !(1u8 << b);
        }
        // A click anywhere commits an in-progress tab rename, so
        // the inline-edit chip can't be stranded by a stray
        // click. (A right-click to start a *new* rename commits
        // the old one first, then re-enters below.)
        if pressed && self.renaming_tab.is_some() {
            self.commit_rename();
        }
        // Strip-region intercept — clicks on tab chips switch
        // (left) / close (middle). Tested against the cached
        // pixel cursor against the rects produced by the last
        // `strip_chip_instances` pass. Runs only on press so
        // releases don't fire a second time; never forwards
        // strip clicks to the terminal (return early).
        if let Some(gpu) = &self.gpu {
            let (px, py) = self.cursor_px;
            // Chrome region = top strip OR (in vertical-tab mode)
            // the left-edge sidebar. Either way clicks here go to
            // tab chips / palette chips and never to the pty/native
            // pane below — the chip hit-rects below handle the
            // routing via their (x0,x1,y0,y1) tuples.
            let in_top_strip = py < (gpu.inset_px + gpu.strip_h) as f64;
            let in_left_sidebar =
                gpu.sidebar_w_px > 0.0 && px < (gpu.inset_px + gpu.sidebar_w_px) as f64;
            let in_chrome = in_top_strip || in_left_sidebar;
            if in_chrome {
                if pressed {
                    // Palette cluster (centered in strip) — four hit
                    // rects, each sends a different key combo to the
                    // active native tab so mnml's existing keybindings
                    // fire. Tested before the tab-chip rects so the
                    // cluster can safely overlap tabs visually.
                    let hits = |rect: Option<(f32, f32, f32, f32)>| -> bool {
                        rect.map(|(x0, x1, y0, y1)| {
                            px >= x0 as f64 && px < x1 as f64 && py >= y0 as f64 && py < y1 as f64
                        })
                        .unwrap_or(false)
                    };
                    let palette_key: Option<KeyInput> = if hits(gpu.strip_palette_back_rect) {
                        // Back → Ctrl+PageUp (buffer.prev).
                        Some(KeyInput {
                            code: KeyCode::PageUp,
                            mods: MOD_CTRL,
                            press: true,
                        })
                    } else if hits(gpu.strip_palette_fwd_rect) {
                        // Forward → Ctrl+PageDown (buffer.next).
                        Some(KeyInput {
                            code: KeyCode::PageDown,
                            mods: MOD_CTRL,
                            press: true,
                        })
                    } else if hits(gpu.strip_palette_chip_rect) {
                        // Search chip → Ctrl+Shift+P (palette).
                        Some(KeyInput {
                            code: KeyCode::Char('P'),
                            mods: MOD_CTRL | MOD_SHIFT,
                            press: true,
                        })
                    } else if hits(gpu.strip_palette_dropdown_rect) {
                        // Dropdown chevron → Ctrl+R (recent files).
                        Some(KeyInput {
                            code: KeyCode::Char('R'),
                            mods: MOD_CTRL,
                            press: true,
                        })
                    } else {
                        None
                    };
                    if let Some(key) = palette_key
                        && button == MouseButton::Left
                    {
                        if let Some(active) = self.tabs.get(self.active)
                            && let PaneKind::Native { server, .. } =
                                &active.panes[active.focused].kind
                        {
                            server.send_input(&InputEvent::Key(key));
                        }
                        return;
                    }
                    // `+` new-tab button sits past the last chip;
                    // check it first since its rect is disjoint.
                    let on_plus = gpu
                        .strip_new_tab_rect
                        .map(|(x0, x1, y0, y1)| {
                            px >= x0 as f64 && px < x1 as f64 && py >= y0 as f64 && py < y1 as f64
                        })
                        .unwrap_or(false);
                    if on_plus && button == MouseButton::Left {
                        if self.editor_template.is_some() {
                            self.new_native_tab();
                        } else {
                            self.new_shell_tab();
                        }
                        return;
                    }
                    // Per-chip `⊗` close badge — left-click on
                    // the badge closes that specific tab.
                    // Tested BEFORE the chip rect so the close
                    // cell isn't also treated as a switch click.
                    let close_hit = gpu
                        .strip_chip_close_rects
                        .iter()
                        .find(|(x0, x1, y0, y1, _)| {
                            px >= *x0 as f64
                                && px < *x1 as f64
                                && py >= *y0 as f64
                                && py < *y1 as f64
                        })
                        .map(|(_, _, _, _, idx)| *idx);
                    if let Some(idx) = close_hit
                        && button == MouseButton::Left
                    {
                        self.close_tab_at(idx);
                        return;
                    }
                    let hit = gpu
                        .strip_chip_rects
                        .iter()
                        .find(|(x0, x1, y0, y1, _)| {
                            px >= *x0 as f64
                                && px < *x1 as f64
                                && py >= *y0 as f64
                                && py < *y1 as f64
                        })
                        .map(|(_, _, _, _, idx)| *idx);
                    if let Some(idx) = hit {
                        match button {
                            MouseButton::Left => {
                                self.switch_to_tab(idx);
                                // Arm a potential drag — a
                                // subsequent CursorMoved over
                                // a different chip will swap.
                                self.dragging_tab = Some(self.active);
                            }
                            MouseButton::Middle => self.close_tab_at(idx),
                            // Right-click → rename the tab inline.
                            MouseButton::Right => self.start_rename(idx),
                            _ => {}
                        }
                    }
                } else if button == MouseButton::Left {
                    // Released — end any in-flight drag.
                    self.dragging_tab = None;
                }
                return;
            }
        }
        // Releasing the left button ends any in-flight drag —
        // chip-reorder or divider-resize.
        if !pressed && button == MouseButton::Left {
            self.dragging_tab = None;
            self.dragging_divider = None;
        }
        // A left-press on a divider starts a drag-resize of that
        // split — it owns the gesture, no pane dispatch.
        if pressed
            && button == MouseButton::Left
            && let Some(idx) = self.divider_at_cursor()
        {
            self.dragging_divider = Some(idx);
            return;
        }
        // Body click — focus the pane under the cursor (on
        // press) and forward the event to it in that pane's
        // local coordinates. Shell panes don't take mouse input
        // yet; only Native panes are forwarded to.
        if let Some((id, lc, lr)) = self.pane_under_cursor() {
            if pressed {
                self.tabs[self.active].focused = id;
            }
            // Browser pane chrome row — intercept the click before
            // the WebView would otherwise eat it. The WebView
            // composites over rows 1.., so any click landing at
            // local row 0 is on our cell-grid chrome strip.
            if pressed
                && let PaneKind::Browser {
                    url,
                    webview,
                    chrome,
                } = &mut self.tabs[self.active].panes[id].kind
                && let Some(chip) = crate::browser_chip_at(lc, lr)
            {
                match chip {
                    crate::BrowserChip::Back => {
                        if let Some(v) = webview.as_ref() {
                            let _ = v.evaluate_script("history.back()");
                        }
                    }
                    crate::BrowserChip::Forward => {
                        if let Some(v) = webview.as_ref() {
                            let _ = v.evaluate_script("history.forward()");
                        }
                    }
                    crate::BrowserChip::Reload => {
                        if let Some(v) = webview.as_ref() {
                            let _ = v.evaluate_script("location.reload()");
                        }
                    }
                    crate::BrowserChip::UrlBar => {
                        // Start an edit pre-loaded with the current URL,
                        // cursor at end.
                        let s = url.clone();
                        let cursor = s.chars().count();
                        chrome.edit = Some(s);
                        chrome.cursor = cursor;
                    }
                }
                self.relayout_all_panes();
                return;
            }
            // A click outside any chrome row commits-as-cancel any
            // in-progress URL edit on any Browser pane. Same idea as
            // the tab-rename inline editor.
            if pressed {
                self.cancel_browser_url_edits();
            }
            match &mut self.tabs[self.active].panes[id].kind {
                PaneKind::Native { server, .. } => {
                    server.send_input(&InputEvent::Mouse(MouseInput {
                        kind: if pressed {
                            MouseKind::Down
                        } else {
                            MouseKind::Up
                        },
                        button: b,
                        col: lc,
                        row: lr,
                        mods: pack_mods(self.mods),
                    }));
                }
                // Forward to the pty child when the child has requested
                // mouse tracking (DECSET 1000/1002/1006). When it hasn't
                // (e.g. a bare shell prompt), `write_mouse` returns false
                // and we drop the event silently — no garbage in stdin.
                PaneKind::Shell { session: Some(s) } => {
                    s.write_mouse(lc, lr, b, pressed, pack_mods(self.mods));
                }
                _ => {}
            }
        }
    }

    /// Drop any in-progress URL edits across every tab's Browser panes.
    /// Called on a body click outside the chrome strip — same gesture
    /// that commits a tab rename.
    fn cancel_browser_url_edits(&mut self) {
        let mut changed = false;
        for tab in self.tabs.iter_mut() {
            for pane in tab.panes.iter_mut() {
                if let PaneKind::Browser { chrome, .. } = &mut pane.kind
                    && chrome.edit.is_some()
                {
                    chrome.edit = None;
                    chrome.cursor = 0;
                    changed = true;
                }
            }
        }
        if changed {
            self.relayout_all_panes();
        }
    }

    fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        // Wheel-in-sidebar scrolls the chip list (vertical mode
        // overflow). Wheel-in-top-strip does nothing (chip-row
        // scrolling for horizontal wrap is a separate v2). Either
        // way the body terminal doesn't see the event.
        if let Some(gpu) = &self.gpu {
            let (px, py) = self.cursor_px;
            let in_top_strip = py < (gpu.inset_px + gpu.strip_h) as f64;
            let in_left_sidebar =
                gpu.sidebar_w_px > 0.0 && px < (gpu.inset_px + gpu.sidebar_w_px) as f64;
            if in_left_sidebar {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_x, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 24.0,
                };
                let chip_count = self.tabs.len();
                let gpu = self.gpu.as_mut().unwrap();
                if gpu.scroll_sidebar(dy, chip_count)
                    && let Some(w) = &self.window
                {
                    w.request_redraw();
                }
                return;
            }
            if in_top_strip {
                return;
            }
        }
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x, y),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 24.0, p.y as f32 / 24.0),
        };
        // Scroll the pane under the cursor (None ⇒ on a divider).
        let Some((id, col, row)) = self.pane_under_cursor() else {
            return;
        };
        let mods = pack_mods(self.mods);
        match &mut self.tabs[self.active].panes[id].kind {
            // Shell mode, alt-screen NOT active — scroll vt100's
            // scrollback. Wheel up → into history.
            PaneKind::Shell { session: Some(s) } if !s.altscreen_active() => {
                let lines = (dy * 3.0).round() as i32;
                if lines != 0 {
                    s.scroll(lines);
                }
            }
            // Shell mode, alt-screen IS active — forward each tick of
            // wheel motion to the pty child as xterm wheel events.
            // Mouse-tracking-aware via `write_mouse`: if the child
            // hasn't enabled mouse capture, the call no-ops silently
            // (no garbage on stdin). TUIs like mnml that DO want
            // wheel events receive them as crossterm scroll events.
            PaneKind::Shell { session: Some(s) } => {
                const BUTTON_WHEEL_UP: u8 = 4;
                const BUTTON_WHEEL_DOWN: u8 = 5;
                let ticks = (dy.abs() * 3.0).round() as i32;
                if ticks > 0 {
                    let button = if dy > 0.0 {
                        BUTTON_WHEEL_UP
                    } else {
                        BUTTON_WHEEL_DOWN
                    };
                    for _ in 0..ticks {
                        s.write_mouse(col, row, button, true, mods);
                    }
                }
            }
            // Native mode — forward the scroll to the backing app.
            PaneKind::Native { server, .. } => {
                if dy.abs() >= dx.abs() {
                    let kind = if dy > 0.0 {
                        MouseKind::ScrollUp
                    } else if dy < 0.0 {
                        MouseKind::ScrollDown
                    } else {
                        return;
                    };
                    server.send_input(&InputEvent::Mouse(MouseInput {
                        kind,
                        button: BUTTON_NONE,
                        col,
                        row,
                        mods,
                    }));
                } else {
                    let kind = if dx > 0.0 {
                        MouseKind::ScrollRight
                    } else {
                        MouseKind::ScrollLeft
                    };
                    server.send_input(&InputEvent::Mouse(MouseInput {
                        kind,
                        button: BUTTON_NONE,
                        col,
                        row,
                        mods,
                    }));
                }
            }
            _ => {}
        }
    }

    fn handle_redraw_requested(&mut self, event_loop: &ActiveEventLoop) {
        self.tick(event_loop);
        // Composite the active tab's panes into the window grid
        // the GPU renders.
        if let Some(gpu) = self.gpu.as_mut() {
            composite(&self.tabs[self.active], &mut gpu.grid);
        }
        // Settings modal paints over the current grid right
        // before GPU render. Because we overlay every frame,
        // the underlying mnml/shell render keeps refreshing
        // below it — close the modal and the world reappears
        // on the next tick.
        if let (Some(gpu), Some(st)) = (self.gpu.as_mut(), self.settings.as_ref()) {
            settings_ui::draw(&mut gpu.grid, st);
        }
        // Welcome overlay — startup-only, dismissed on Esc / pick.
        // Painted last so it sits above the settings modal too (the
        // settings panel can't actually be open at startup, but the
        // layering matches the conceptual stack).
        if let (Some(gpu), Some(st)) = (self.gpu.as_mut(), self.welcome.as_ref()) {
            crate::welcome::draw(&mut gpu.grid, st);
        }
        // Help overlay — toggled by `view.help` (default ⌘⇧/).
        // Painted last so it sits above settings + welcome.
        if let (Some(gpu), Some(st)) = (self.gpu.as_mut(), self.help.as_ref()) {
            crate::help::draw(&mut gpu.grid, st);
        }
        // Command palette overlay — toggled by `view.palette`
        // (default ⌘⇧P). Painted after help so an open palette
        // sits above an in-flight help, on the off-chance both
        // are open (palette opens close help via the modal
        // dispatch ordering, but defensive ordering doesn't hurt).
        if let (Some(gpu), Some(st)) = (self.gpu.as_mut(), self.palette.as_ref()) {
            crate::palette::draw(&mut gpu.grid, st);
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.render();
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}
