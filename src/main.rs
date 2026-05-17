mod atlas;
mod config;
mod grid;
mod launcher;
mod menu;
mod pipeline;
mod server;
mod settings_ui;
mod shell;

use tmnl_protocol as protocol;

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use atlas::Atlas;
use config::Config;
use grid::Grid;
use launcher::{Launcher, LauncherConfig, LauncherPoll};
use menu::AppMenu;
use pipeline::CellPipeline;
use settings_ui::SettingsState;
use protocol::{
    BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_NONE, BUTTON_RIGHT, Frame, InputEvent, KeyCode, KeyInput,
    MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_SUPER, MouseInput, MouseKind, unpack_rgba,
};
use server::{Server, ServerEvent, default_socket_path};
use shell::{ShellSession, winit_key_to_bytes};

const FONT_PX: f32 = 14.0;
/// Extra top-side padding (added to whatever inset is configured) so
/// the macOS traffic-light buttons don't sit right on top of the first
/// cell row. `with_titlebar_transparent + fullsize_content_view` lets
/// our wgpu surface extend behind where the title bar used to be —
/// without this nudge, row 0 lands underneath the close/min/max chips.
///
/// Two values:
///   - `TUI`  (16 px) used when a Native client is streaming TUI
///     frames. The client's top row is typically chrome (bufferline /
///     tab strip) so a smaller pad is fine — the buttons sit against
///     a dark chrome bg rather than user content.
///   - `BARE` (30 px) used everywhere else: idle screen, hosted
///     `Mode::Shell`, or a connected-but-not-yet-streaming client.
///     Row 0 holds free user content (shell prompt, idle banner) so
///     the buttons need a fully separated band above it.
#[cfg(target_os = "macos")]
const MACOS_TOP_PAD_TUI_PX: f32 = 16.0;
#[cfg(target_os = "macos")]
const MACOS_TOP_PAD_BARE_PX: f32 = 35.0;
#[cfg(not(target_os = "macos"))]
const MACOS_TOP_PAD_TUI_PX: f32 = 0.0;
#[cfg(not(target_os = "macos"))]
const MACOS_TOP_PAD_BARE_PX: f32 = 0.0;
// Frame background — fills (a) the top pad reserved for the macOS
// traffic-light buttons, (b) the letterbox gutter at the bottom when
// the window height isn't a clean row multiple, and (c) any sub-cell
// pixel overflow on the right. Matches mnml's `bg_darker` (the chrome
// color used by tree rail + bufferline) so the inner padding reads as
// "extension of the app chrome" instead of a hard black border around
// the cell grid. Apps with different chrome would want to override
// this through the protocol (TODO once a second app talks to tmnl).
const CLEAR_BG: [f32; 4] = [0.106, 0.122, 0.153, 1.0];
const TEXT_FG: [f32; 4] = [0.86, 0.87, 0.92, 1.0];
const ACCENT_FG: [f32; 4] = [0.93, 0.73, 0.45, 1.0];
const DIM_FG: [f32; 4] = [0.48, 0.50, 0.58, 1.0];
const ATTR_CURSOR_BLOCK: u32 = 1 << 16;
const ATTR_CURSOR_UNDERLINE: u32 = 1 << 17;
const ATTR_CURSOR_BAR: u32 = 1 << 18;

#[derive(Clone, Copy, PartialEq)]
enum ConnState {
    Waiting,
    Connected,
    Streaming,
}

enum Mode {
    /// tmnl runs $SHELL itself; vt100 parses output → Grid.
    Shell { session: Option<ShellSession> },
    /// tmnl launches mnml as a UDS client; blit cells stream into Grid.
    Native {
        server: Server,
        conn: ConnState,
        launcher: Option<Launcher>,
    },
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    mods: ModifiersState,
    cursor_cell: (u16, u16),
    buttons_down: u8,
    mode: Mode,
    /// Pre-resolved pixel inset (CLI / env / config / default) handed to
    /// `Gpu::new` on `resumed`. Per-mode — native can opt out
    /// (edge-to-edge TUI) while shell keeps a margin.
    inset_px: f32,
    /// Persisted config, loaded once at startup. Settings UI edits this
    /// copy live; Enter saves to disk.
    cfg: Config,
    /// In shell mode, true while a full-screen TUI (vim / mnml / mixr /
    /// htop, …) has the xterm alt-screen buffer active. Drives the
    /// auto-switch from `inset_shell` (padded prompt view) → 0 (TUI
    /// goes edge-to-edge) without the user having to flip anything in
    /// Settings.
    altscreen_active: bool,
    /// Native macOS menu bar — built once at startup and kept alive for
    /// the process. `None` until `resumed` runs (winit needs `NSApp` up
    /// first). Some platforms ignore this; macOS is the target.
    app_menu: Option<AppMenu>,
    /// Settings modal — `Some` while the user has the panel open.
    settings: Option<SettingsState>,
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    #[allow(dead_code)]
    scale: f32,
    atlas: Atlas,
    pipeline: CellPipeline,
    grid: Grid,
    last_cursor: Option<usize>,
    /// Equal-width pixel margin reserved on every side of the grid.
    /// Resolved from `--inset` / `TMNL_INSET` / `DEFAULT_INSET_PX` at
    /// startup (in that order).
    inset_px: f32,
    /// Current top-side pad reserved for the macOS traffic-light buttons.
    /// Switches between `MACOS_TOP_PAD_TUI_PX` (smaller, when a Native
    /// client is streaming) and `MACOS_TOP_PAD_BARE_PX` (larger, for
    /// idle / shell / pre-stream) — see `set_top_pad_px`.
    top_pad_px: f32,
}

impl Gpu {
    async fn new(window: Arc<Window>, inset_px: f32) -> Self {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no compatible GPU adapter");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("tmnl"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .expect("device request failed");
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let caps = surface.get_capabilities(&adapter);
        // Pick a NON-sRGB surface so the hex color values we pack into
        // `fg`/`bg` instances land on screen unchanged. With an sRGB surface
        // the GPU would apply a linear→sRGB encode on write, double-gamma-
        // correcting values that already came from sRGB hex literals — that
        // was the "faded" / washed-out look (blacks lifted, saturated colors
        // muted). We do the alpha compositing for monochrome glyphs directly
        // in sRGB space, which is wrong-in-theory but matches every CPU
        // terminal renderer ever shipped.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let atlas =
            Atlas::new(&device, &queue, FONT_PX * scale).expect("failed to build glyph atlas");
        // Start with the BARE pad — the first frame is always the idle
        // banner ("waiting for client") or a hosted shell. App layer
        // shrinks it to TUI once a Native client begins streaming.
        let top_pad_px = MACOS_TOP_PAD_BARE_PX;
        let (cols, rows) = grid_dims(size.width, size.height, &atlas, inset_px, top_pad_px);
        let g = Grid::new(cols, rows, CLEAR_BG);

        let pipeline = CellPipeline::new(&device, format, &atlas, (cols * rows).max(1024) as u64);

        Self {
            surface,
            device,
            queue,
            config,
            scale,
            atlas,
            pipeline,
            grid: g,
            last_cursor: None,
            inset_px,
            top_pad_px,
        }
    }

    fn resize(&mut self, w: u32, h: u32) -> Option<(u16, u16)> {
        if w == 0 || h == 0 {
            return None;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        let (cols, rows) = grid_dims(w, h, &self.atlas, self.inset_px, self.top_pad_px);
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            self.last_cursor = None;
            return Some((cols as u16, rows as u16));
        }
        None
    }

    /// Update the pixel inset live (Settings slider). Returns the new
    /// grid dims if they shifted — caller pipes the size out to the
    /// native client (mnml/mixr) so its layout adapts.
    fn set_inset_px(&mut self, inset_px: f32) -> Option<(u16, u16)> {
        if (self.inset_px - inset_px).abs() < f32::EPSILON {
            return None;
        }
        self.inset_px = inset_px;
        let (w, h) = (self.config.width, self.config.height);
        let (cols, rows) = grid_dims(w, h, &self.atlas, self.inset_px, self.top_pad_px);
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            self.last_cursor = None;
            return Some((cols as u16, rows as u16));
        }
        None
    }

    /// Switch the top pad between TUI (compact) and BARE (wide). Called
    /// from the App layer on mode/state transitions (Streaming ⇄ idle).
    /// Returns the new grid dims if they shifted so the caller can push
    /// the new size out to the connected client.
    fn set_top_pad_px(&mut self, top_pad_px: f32) -> Option<(u16, u16)> {
        if (self.top_pad_px - top_pad_px).abs() < f32::EPSILON {
            return None;
        }
        self.top_pad_px = top_pad_px;
        let (w, h) = (self.config.width, self.config.height);
        let (cols, rows) = grid_dims(w, h, &self.atlas, self.inset_px, self.top_pad_px);
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            self.last_cursor = None;
            return Some((cols as u16, rows as u16));
        }
        None
    }

    fn apply_frame(&mut self, f: &Frame) {
        // Clear the previous cursor's overlay bits first — runs may not cover
        // that cell, so we have to do it explicitly.
        if let Some(old) = self.last_cursor
            && let Some(c) = self.grid.cells.get_mut(old)
        {
            c.attrs &= !(ATTR_CURSOR_BLOCK | ATTR_CURSOR_UNDERLINE | ATTR_CURSOR_BAR);
        }

        let grid_cols = self.grid.cols;
        let grid_rows = self.grid.rows;
        let grid_max = (grid_cols * grid_rows) as usize;
        let frame_cols = f.cols as u32;
        // Diff runs are encoded against the sender's (cols, rows). If we
        // resized faster than the sender, clip per-row.
        for run in &f.runs {
            let start = run.start as usize;
            for (i, wc) in run.cells.iter().enumerate() {
                let abs = start + i;
                // Translate sender-grid index → our grid index.
                let r = (abs as u32) / frame_cols;
                let c = (abs as u32) % frame_cols;
                if r >= grid_rows || c >= grid_cols {
                    continue;
                }
                let dst = (r * grid_cols + c) as usize;
                if dst >= grid_max {
                    continue;
                }
                let ch = char::from_u32(wc.ch).unwrap_or(' ');
                self.grid.cells[dst] = grid::Cell {
                    ch,
                    fg: unpack_rgba(wc.fg),
                    bg: unpack_rgba(wc.bg),
                    attrs: wc.attrs,
                };
            }
        }

        if f.cursor_visible != 0
            && (f.cursor_col as u32) < grid_cols
            && (f.cursor_row as u32) < grid_rows
        {
            let i = (f.cursor_row as u32 * grid_cols + f.cursor_col as u32) as usize;
            let bit = match f.cursor_shape {
                1 => ATTR_CURSOR_UNDERLINE,
                2 => ATTR_CURSOR_BAR,
                _ => ATTR_CURSOR_BLOCK,
            };
            self.grid.cells[i].attrs |= bit;
            self.last_cursor = Some(i);
        } else {
            self.last_cursor = None;
        }
    }

    fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(e) => {
                log::warn!("dropped frame: {e:?}");
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Render the grid offset by exactly `inset_px` on left/right
        // and `inset_px + top_pad_px` on top so every per-pixel
        // nudge in Settings is visible AND the traffic-light buttons
        // have a tiny strip of clear bg above the grid. The leftover
        // (window % cell_w) lands on the right/bottom — at most one
        // cell width, accepted as the price of an integer-cell grid.
        // inset == 0 still gets the ceil'd grid_dims so the rightmost
        // cells reach the edge — true edge-to-edge for TUIs.
        self.pipeline.write_globals(
            &self.queue,
            [self.config.width as f32, self.config.height as f32],
            [self.atlas.cell_w, self.atlas.cell_h],
            [self.inset_px, self.inset_px + self.top_pad_px],
        );
        let instances =
            CellPipeline::build_instances(&self.grid, &mut self.atlas, &self.queue);
        self.pipeline
            .ensure_capacity(&self.device, instances.len() as u64);
        self.pipeline.upload(&self.queue, &instances);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cells"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: CLEAR_BG[0] as f64,
                            g: CLEAR_BG[1] as f64,
                            b: CLEAR_BG[2] as f64,
                            a: CLEAR_BG[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline.pipeline);
            pass.set_bind_group(0, &self.pipeline.bind_group, &[]);
            pass.set_vertex_buffer(0, self.pipeline.instance_buf.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

fn grid_dims(w: u32, h: u32, atlas: &Atlas, inset_px: f32, top_pad_px: f32) -> (u32, u32) {
    // `inset_px == 0` → edge-to-edge horizontally; vertically we still
    // reserve `top_pad_px` so the traffic-light buttons don't sit on
    // top of the grid. Ceil cols so the rightmost cells reach the
    // window edge (the partial overflow is clipped by the wgpu
    // surface — no clear-bg stripe at the right seam). Floor rows
    // so the LAST cell row gets its full font-row height — any
    // leftover sub-row pixels at the bottom become a small letterbox
    // gutter painted in `CLEAR_BG` by the wgpu clear (industry
    // standard: Apple Terminal, iTerm2, Alacritty, Kitty all do
    // this). The alternative — ceiling rows + clipping the last
    // partial row — leaves a few-pixel sliver of whatever the app
    // drew on the bottom row (status bar / cmdline), which reads
    // as visual noise.
    // `inset_px > 0` → reserve `inset_px` pixels on every side; floor
    // cols/rows so the cells fit inside.
    if inset_px <= 0.0 {
        let cols = (w as f32 / atlas.cell_w).ceil().max(1.0) as u32;
        let usable_h = (h as f32 - top_pad_px).max(atlas.cell_h);
        let rows = (usable_h / atlas.cell_h).floor().max(1.0) as u32;
        return (cols, rows);
    }
    let usable_w = (w as f32 - 2.0 * inset_px).max(atlas.cell_w);
    let usable_h = (h as f32 - 2.0 * inset_px - top_pad_px).max(atlas.cell_h);
    let cols = (usable_w / atlas.cell_w).floor().max(1.0) as u32;
    let rows = (usable_h / atlas.cell_h).floor().max(1.0) as u32;
    (cols, rows)
}

fn paint_idle(grid: &mut Grid, state: ConnState, socket_path: &std::path::Path) {
    grid.clear();
    if grid.rows < 3 || grid.cols < 10 {
        return;
    }
    let title = "tmnl";
    let row = grid.rows / 2;
    let title_col = (grid.cols.saturating_sub(title.chars().count() as u32)) / 2;
    grid.write(title_col, row.saturating_sub(2), title, ACCENT_FG, CLEAR_BG);

    let (status, color) = match state {
        ConnState::Waiting => ("waiting for client", DIM_FG),
        ConnState::Connected => ("client connected — awaiting first frame", TEXT_FG),
        ConnState::Streaming => return,
    };
    let status_col = (grid.cols.saturating_sub(status.chars().count() as u32)) / 2;
    grid.write(status_col, row, status, color, CLEAR_BG);

    let hint = format!("socket: {}", socket_path.display());
    if hint.chars().count() < grid.cols as usize {
        let col = (grid.cols.saturating_sub(hint.chars().count() as u32)) / 2;
        grid.write(col, row + 2, &hint, DIM_FG, CLEAR_BG);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("tmnl")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0));
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
        let mut gpu = pollster::block_on(Gpu::new(window.clone(), self.inset_px));
        match &mut self.mode {
            Mode::Shell { session } => {
                let (cols, rows) = (gpu.grid.cols as u16, gpu.grid.rows as u16);
                match ShellSession::spawn(rows, cols, TEXT_FG, CLEAR_BG) {
                    Ok(s) => *session = Some(s),
                    Err(e) => {
                        eprintln!("tmnl: failed to start shell: {e}");
                        event_loop.exit();
                        return;
                    }
                }
            }
            Mode::Native { server, conn, .. } => {
                paint_idle(&mut gpu.grid, *conn, &server.socket_path);
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
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let Some(gpu) = self.gpu.as_mut() else {
                    return;
                };
                let Some((cols, rows)) = gpu.resize(size.width, size.height) else {
                    return;
                };
                match &mut self.mode {
                    Mode::Shell { session } => {
                        if let Some(s) = session.as_mut() {
                            s.resize(rows, cols);
                        }
                    }
                    Mode::Native { server, conn, .. } => {
                        server.send_resize(cols, rows);
                        if *conn != ConnState::Streaming {
                            paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                        }
                    }
                }
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
                if ke.state != ElementState::Pressed {
                    return;
                }
                // Settings panel swallows keys while open (Esc closes,
                // Enter saves; everything else either adjusts the
                // selected field or no-ops).
                if self.settings.is_some() && self.settings_handle_key(&ke) {
                    return;
                }
                match &mut self.mode {
                    Mode::Shell { session } => {
                        if let Some(s) = session.as_mut()
                            && let Some(bytes) = winit_key_to_bytes(&ke.logical_key, self.mods)
                        {
                            s.write_bytes(&bytes);
                        }
                    }
                    Mode::Native { server, .. } => {
                        if let Some(code) = translate_key(&ke.logical_key, self.mods) {
                            let input = InputEvent::Key(KeyInput {
                                code,
                                mods: pack_mods(self.mods),
                                press: true,
                            });
                            server.send_input(&input);
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(gpu) = &self.gpu {
                    let (col, row) = gpu.pixel_to_cell(position.x, position.y);
                    let prev = self.cursor_cell;
                    self.cursor_cell = (col, row);
                    if let Mode::Native { server, .. } = &self.mode
                        && self.buttons_down != 0
                        && prev != self.cursor_cell
                    {
                        let button = first_button(self.buttons_down);
                        server.send_input(&InputEvent::Mouse(MouseInput {
                            kind: MouseKind::Drag,
                            button,
                            col,
                            row,
                            mods: pack_mods(self.mods),
                        }));
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
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
                if let Mode::Native { server, .. } = &self.mode {
                    let (col, row) = self.cursor_cell;
                    server.send_input(&InputEvent::Mouse(MouseInput {
                        kind: if pressed {
                            MouseKind::Down
                        } else {
                            MouseKind::Up
                        },
                        button: b,
                        col,
                        row,
                        mods: pack_mods(self.mods),
                    }));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Mode::Native { server, .. } = &self.mode else {
                    return;
                };
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 24.0, p.y as f32 / 24.0),
                };
                let (col, row) = self.cursor_cell;
                let mods = pack_mods(self.mods);
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
            WindowEvent::RedrawRequested => {
                self.tick(event_loop);
                // Settings modal paints over the current grid right
                // before GPU render. Because we overlay every frame,
                // the underlying mnml/shell render keeps refreshing
                // below it — close the modal and the world reappears
                // on the next tick.
                if let (Some(gpu), Some(st)) = (self.gpu.as_mut(), self.settings.as_ref()) {
                    settings_ui::draw(&mut gpu.grid, st);
                }
                if let Some(gpu) = &mut self.gpu {
                    gpu.render();
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn pack_mods(m: ModifiersState) -> u8 {
    let mut out = 0u8;
    if m.shift_key() {
        out |= MOD_SHIFT;
    }
    if m.control_key() {
        out |= MOD_CTRL;
    }
    if m.alt_key() {
        out |= MOD_ALT;
    }
    if m.super_key() {
        out |= MOD_SUPER;
    }
    out
}

fn first_button(mask: u8) -> u8 {
    for b in 0..4 {
        if mask & (1u8 << b) != 0 {
            return b;
        }
    }
    BUTTON_NONE
}

impl Gpu {
    /// Convert a pixel (CursorMoved / MouseInput) to a grid cell using
    /// the same literal-inset math the shader applies in `write_globals`.
    /// Clicks inside the inset margin saturate to (0, 0).
    fn pixel_to_cell(&self, px: f64, py: f64) -> (u16, u16) {
        let inset_x = self.inset_px as f64;
        let inset_y = self.inset_px as f64 + self.top_pad_px as f64;
        let col = ((px - inset_x).max(0.0) / self.atlas.cell_w as f64).floor() as u16;
        let row = ((py - inset_y).max(0.0) / self.atlas.cell_h as f64).floor() as u16;
        (col, row)
    }
}

fn translate_key(k: &Key, mods: ModifiersState) -> Option<KeyCode> {
    match k {
        Key::Named(n) => match n {
            NamedKey::Enter => Some(KeyCode::Enter),
            NamedKey::Backspace => Some(KeyCode::Backspace),
            NamedKey::Tab => {
                if mods.shift_key() {
                    Some(KeyCode::BackTab)
                } else {
                    Some(KeyCode::Tab)
                }
            }
            NamedKey::Escape => Some(KeyCode::Esc),
            NamedKey::Delete => Some(KeyCode::Delete),
            NamedKey::Insert => Some(KeyCode::Insert),
            NamedKey::Home => Some(KeyCode::Home),
            NamedKey::End => Some(KeyCode::End),
            NamedKey::PageUp => Some(KeyCode::PageUp),
            NamedKey::PageDown => Some(KeyCode::PageDown),
            NamedKey::ArrowLeft => Some(KeyCode::Left),
            NamedKey::ArrowRight => Some(KeyCode::Right),
            NamedKey::ArrowUp => Some(KeyCode::Up),
            NamedKey::ArrowDown => Some(KeyCode::Down),
            NamedKey::F1 => Some(KeyCode::F(1)),
            NamedKey::F2 => Some(KeyCode::F(2)),
            NamedKey::F3 => Some(KeyCode::F(3)),
            NamedKey::F4 => Some(KeyCode::F(4)),
            NamedKey::F5 => Some(KeyCode::F(5)),
            NamedKey::F6 => Some(KeyCode::F(6)),
            NamedKey::F7 => Some(KeyCode::F(7)),
            NamedKey::F8 => Some(KeyCode::F(8)),
            NamedKey::F9 => Some(KeyCode::F(9)),
            NamedKey::F10 => Some(KeyCode::F(10)),
            NamedKey::F11 => Some(KeyCode::F(11)),
            NamedKey::F12 => Some(KeyCode::F(12)),
            NamedKey::Space => Some(KeyCode::Char(' ')),
            _ => None,
        },
        Key::Character(s) => s.chars().next().map(KeyCode::Char),
        _ => None,
    }
}

impl App {
    fn tick(&mut self, event_loop: &ActiveEventLoop) {
        // Drain the menu-event channel first — `muda` delivers menu picks
        // (and accelerator-fired items like ⌘, / ⌘+ / ⌘-) through this
        // global channel. Acting on them before the per-frame work means
        // the next render reflects whatever the menu changed.
        self.drain_menu_events();

        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        match &mut self.mode {
            Mode::Shell { session } => {
                let Some(s) = session.as_mut() else {
                    return;
                };
                if s.exited() {
                    event_loop.exit();
                    return;
                }
                // Auto-switch the inset when a full-screen TUI takes
                // over (xterm alt-screen). The TUI gets edge-to-edge;
                // the shell prompt gets its padded view back on exit.
                let altscreen = s.altscreen_active();
                if altscreen != self.altscreen_active {
                    self.altscreen_active = altscreen;
                    let target_inset = self.cfg.active_inset(altscreen);
                    if let Some((cols, rows)) = gpu.set_inset_px(target_inset) {
                        s.resize(rows, cols);
                    }
                }
                if s.dirty() {
                    let (cc, cr, vis) = s.apply_to_grid(&mut gpu.grid);
                    gpu.last_cursor = None; // Shell mode tracks cursor via apply_to_grid
                    if vis
                        && (cc as u32) < gpu.grid.cols
                        && (cr as u32) < gpu.grid.rows
                    {
                        let i = (cr as u32 * gpu.grid.cols + cc as u32) as usize;
                        gpu.grid.cells[i].attrs |= ATTR_CURSOR_BLOCK;
                        gpu.last_cursor = Some(i);
                    }
                }
            }
            Mode::Native {
                server,
                conn,
                launcher,
            } => {
                // Drain server events.
                while let Ok(ev) = server.events.try_recv() {
                    match ev {
                        ServerEvent::ClientConnected => {
                            *conn = ConnState::Connected;
                            server.send_resize(gpu.grid.cols as u16, gpu.grid.rows as u16);
                            paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                        }
                        ServerEvent::ClientDisconnected => {
                            *conn = ConnState::Waiting;
                            // Drop back to BARE pad — the idle banner needs the
                            // full button clearance now that there's no chrome row.
                            if let Some((cols, rows)) = gpu.set_top_pad_px(MACOS_TOP_PAD_BARE_PX) {
                                let _ = (cols, rows); // no client to resize
                            }
                            paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                        }
                    }
                }
                // Drain frames — diffs are cumulative, so every frame must be
                // applied in order. (The earlier "keep only the last" was the
                // bug that left the full seq=0 frame stranded behind empty
                // seq=1/seq=2 diffs.)
                let mut applied = 0u32;
                while let Ok(f) = server.frame_rx.try_recv() {
                    if *conn != ConnState::Streaming {
                        *conn = ConnState::Streaming;
                        // Streaming TUI frames now — the client's top row is
                        // chrome, so shrink the top pad. Notify the client of
                        // the new grid size so its layout adapts.
                        if let Some((cols, rows)) = gpu.set_top_pad_px(MACOS_TOP_PAD_TUI_PX) {
                            server.send_resize(cols, rows);
                        }
                    }
                    gpu.apply_frame(&f);
                    applied += 1;
                }
                if applied > 0 {
                    log::debug!("[tick] applied {applied} frame(s)");
                }
                // Poll launcher.
                if let Some(l) = launcher.as_mut() {
                    match l.poll() {
                        LauncherPoll::Running | LauncherPoll::Idle => {}
                        LauncherPoll::Restart => {
                            log::info!("launcher: restart requested (exit 75); respawning");
                            if let Err(e) = l.spawn() {
                                eprintln!("tmnl: failed to respawn child: {e}");
                                event_loop.exit();
                            }
                        }
                        LauncherPoll::Exited(code) => {
                            log::info!("launcher: child exited with code {code}; closing window");
                            event_loop.exit();
                        }
                    }
                }
            }
        }
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
            if id == menu.id_settings {
                self.open_settings();
            } else if id == menu.id_new_window {
                log::info!("menu: New Window — placeholder, not wired yet");
            } else if id == menu.id_font_inc {
                log::info!("menu: Increase Font Size — placeholder, not wired yet");
            } else if id == menu.id_font_dec {
                log::info!("menu: Decrease Font Size — placeholder, not wired yet");
            } else if id == menu.id_font_reset {
                log::info!("menu: Reset Font Size — placeholder, not wired yet");
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
        let native = matches!(self.mode, Mode::Native { .. });
        let tui_active = native || self.altscreen_active;
        let new_inset = cfg.active_inset(tui_active);
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let Some((cols, rows)) = gpu.set_inset_px(new_inset) else {
            return;
        };
        match &mut self.mode {
            Mode::Native { server, conn, .. } => {
                server.send_resize(cols, rows);
                if *conn != ConnState::Streaming {
                    paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                }
            }
            Mode::Shell { session } => {
                if let Some(s) = session.as_mut() {
                    s.resize(rows, cols);
                }
            }
        }
    }

    /// Route a keystroke into the Settings modal. Returns true if the
    /// key was consumed (mode-specific handlers should skip it).
    fn settings_handle_key(&mut self, ke: &winit::event::KeyEvent) -> bool {
        let Some(st) = self.settings.as_mut() else {
            return false;
        };
        use winit::keyboard::{Key, NamedKey};
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
            Key::Named(NamedKey::ArrowLeft) => {
                st.nudge(-1.0);
                let edited = st.cfg.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            Key::Named(NamedKey::ArrowRight) => {
                st.nudge(1.0);
                let edited = st.cfg.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                st.reset();
                let edited = st.cfg.clone();
                self.apply_inset_from_cfg(&edited);
                true
            }
            // Every other key gets eaten so it doesn't reach the shell
            // / native target underneath.
            _ => true,
        }
    }

    fn shutdown_gracefully(&mut self) {
        match &mut self.mode {
            Mode::Shell { session } => {
                // Drop the ShellSession — its Drop hardkills the child.
                *session = None;
            }
            Mode::Native {
                server, launcher, ..
            } => {
                server.send_quit();
                if let Some(l) = launcher.as_mut() {
                    let _ = l.wait_for_exit(std::time::Duration::from_millis(1200));
                }
            }
        }
    }
}

/// Look up `--<key> <value>` in `argv` and parse as f32. Returns None if
/// missing or unparseable.
fn arg_f32(argv: &[String], key: &str) -> Option<f32> {
    argv.iter()
        .position(|a| a == key)
        .and_then(|i| argv.get(i + 1))
        .and_then(|v| v.parse::<f32>().ok())
}

fn env_f32(key: &str) -> Option<f32> {
    std::env::var(key).ok().and_then(|s| s.parse::<f32>().ok())
}

/// Resolve the pixel inset at launch. Priority:
///   1. `--inset <N>` CLI flag
///   2. `TMNL_INSET` env var
///   3. config file (`~/.config/tmnl/config.toml`)
///   4. `Config::default().inset` (20px)
///
/// TUIs always render at 0 regardless — only the shell-prompt view
/// uses this value.
fn resolve_inset(argv: &[String], cfg: &Config) -> f32 {
    arg_f32(argv, "--inset")
        .or_else(|| env_f32("TMNL_INSET"))
        .unwrap_or(cfg.inset)
        .max(0.0)
}

fn main() {
    env_logger::init();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // `--mnml` launches mnml in native/integrated mode (UDS blit channel,
    // wgpu renders, mnml drives input). Future siblings: `--mixr`, etc.
    let editor_mode = argv.iter().any(|a| a == "--mnml");
    let no_launch = argv.iter().any(|a| a == "--no-launch");
    let cfg = Config::load();
    // Native mode hosts a TUI directly — always edge-to-edge. Shell
    // mode starts at the configured value; the alt-screen detector
    // flips to 0 when a TUI takes over.
    let inset_px = if editor_mode {
        0.0
    } else {
        resolve_inset(&argv, &cfg)
    };
    // Filter out our own flags (and their values) before handing the
    // remainder to the launcher's positional parser.
    let mut filtered: Vec<String> = Vec::new();
    let mut iter = argv.iter().peekable();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--mnml" => {}
            "--inset" | "--inset-native" | "--inset-shell" => {
                // Skip the value too if there is one.
                if iter.peek().is_some_and(|v| !v.starts_with("--")) {
                    iter.next();
                }
            }
            _ => filtered.push(a.clone()),
        }
    }
    let (workspace_arg, _) = launcher::parse_argv(&filtered);

    let mode = if editor_mode {
        let socket_path = default_socket_path();
        eprintln!("tmnl: editor mode — listening on {}", socket_path.display());
        let server = Server::start(socket_path.clone()).expect("failed to start tmnl server");
        let launcher = if no_launch {
            eprintln!(
                "tmnl: --no-launch — start mnml manually with --blit {}",
                socket_path.display()
            );
            None
        } else {
            let workspace = launcher::resolve_workspace(workspace_arg.as_deref());
            let command = launcher::resolve_launch_command();
            let extra_args = launcher::default_extra_args();
            let cfg = LauncherConfig {
                command: command.clone(),
                workspace: workspace.clone(),
                socket: socket_path.clone(),
                extra_args,
            };
            let mut l = Launcher::new(cfg);
            match l.spawn() {
                Ok(()) => {
                    eprintln!(
                        "tmnl: spawned {} for workspace {}",
                        command.display(),
                        workspace.display()
                    );
                    Some(l)
                }
                Err(e) => {
                    eprintln!(
                        "tmnl: failed to launch {} ({e}); start mnml manually with --blit {}",
                        command.display(),
                        socket_path.display()
                    );
                    None
                }
            }
        };
        Mode::Native {
            server,
            conn: ConnState::Waiting,
            launcher,
        }
    } else {
        eprintln!("tmnl: shell mode (run with --editor to launch mnml instead)");
        Mode::Shell { session: None }
    };

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        window: None,
        gpu: None,
        mods: ModifiersState::empty(),
        cursor_cell: (0, 0),
        buttons_down: 0,
        mode,
        inset_px,
        cfg,
        altscreen_active: false,
        app_menu: None,
        settings: None,
    };
    event_loop.run_app(&mut app).unwrap();
    drop(app);
}
