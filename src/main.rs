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

use std::path::PathBuf;
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
use protocol::{
    BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_NONE, BUTTON_RIGHT, Frame, InputEvent, KeyCode, KeyInput,
    MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_SUPER, MouseInput, MouseKind, unpack_rgba,
};
use server::{Server, ServerEvent, default_socket_path, native_tab_socket_path};
use settings_ui::SettingsState;
use shell::{ShellSession, winit_key_to_bytes};

const FONT_PX: f32 = 14.0;
/// Height of the chrome strip at the top of the window. Houses the
/// traffic-light buttons (left ~80 px) and the tab chips (everything
/// to their right). The cell grid starts immediately below it.
/// `with_titlebar_transparent + fullsize_content_view` lets our wgpu
/// surface extend through this area; the `StripPipeline` paints the
/// background, and the cell pipeline draws on top with no overlap
/// (offset by `inset_px + gpu.strip_h`).
/// Multi-tab chrome height — when there's more than one tab, the
/// strip is tall enough to hold a row of chips plus generous padding.
#[cfg(target_os = "macos")]
const MACOS_TAB_STRIP_PX_MULTI: f32 = 72.0;
#[cfg(not(target_os = "macos"))]
const MACOS_TAB_STRIP_PX_MULTI: f32 = 0.0;
/// Single-tab chrome height — a small breathing-room band above the
/// grid so the first row of content isn't kissing the macOS traffic
/// lights, but no visible chrome strip (the strip pipeline paints this
/// region in `CLEAR_BG` instead of `STRIP_BG` when there are no chips,
/// so it blends invisibly with the surrounding clear color).
#[cfg(target_os = "macos")]
const MACOS_TAB_STRIP_PX_SINGLE: f32 = 24.0;
#[cfg(not(target_os = "macos"))]
const MACOS_TAB_STRIP_PX_SINGLE: f32 = 0.0;
/// Single-tab strip height for *shell* mode (no TUI hosted, e.g. a bare
/// `zsh` prompt). Larger than the TUI value so the prompt's first row
/// doesn't sit right under the macOS traffic lights. The strip pipeline
/// still paints CLEAR_BG so this band is invisible — pure padding.
#[cfg(target_os = "macos")]
const MACOS_TAB_STRIP_PX_SHELL: f32 = 42.0;
#[cfg(not(target_os = "macos"))]
const MACOS_TAB_STRIP_PX_SHELL: f32 = 0.0;
// Frame background — fills (a) the top pad reserved for the macOS
// traffic-light buttons, (b) the letterbox gutter at the bottom when
// the window height isn't a clean row multiple, and (c) any sub-cell
// pixel overflow on the right. Matches mnml's `bg_darker` (the chrome
// color used by tree rail + bufferline) so the inner padding reads as
// "extension of the app chrome" instead of a hard black border around
// the cell grid. Apps with different chrome would want to override
// this through the protocol (TODO once a second app talks to tmnl).
const CLEAR_BG: [f32; 4] = [0.106, 0.122, 0.153, 1.0];
// Tab-strip background — the chrome row across the top of the window
// where the traffic-light buttons + tab chips sit. `#22262e` matches
// mnml's `statusline_bg` so the top and bottom chrome share a palette.
const STRIP_BG: [f32; 4] = [0.133, 0.149, 0.180, 1.0];
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
        /// Tab title supplied by the connected client via
        /// `Message::Title`. `None` until the client sends one — falls
        /// back to "mnml" in the label-resolution chain.
        client_title: Option<String>,
    },
}

/// One open tab in the tmnl window. Each tab is independent: its own
/// `Mode` (Shell or Native) + its own current label. Multi-tab work
/// builds up around this struct — the App holds a `Vec<Tab>` + an
/// `active: usize` index; chip rendering walks `tabs` and highlights
/// the active one; ⌘T / ⌘W / ⌘1..9 keybinds manipulate the Vec.
///
/// First step (this commit): App carries a `Vec<Tab>` with exactly
/// one entry. Behaviorally identical to a single-Mode app. Future
/// commits add per-tab `Grid` storage so background tabs preserve
/// state, plus the keybinds and chip rendering.
struct Tab {
    /// Hosted process / connection state for this tab.
    mode: Mode,
    /// Cached label for the strip — refreshed each tick from `mode`
    /// + the shell's OSC title where applicable. `App::tick`
    /// rewrites this; the renderer reads from `tabs[active].label`.
    label: String,
    /// Set true when the hosted process emits an OSC 1337 attention
    /// signal (Claude Code does this when a turn finishes and it's
    /// waiting for user input). Cleared when the user switches to
    /// this tab. Rendered as a `●` prefix in the chip.
    attention: bool,
    /// Sticky cache of the most recent detected spinner line and
    /// when we last saw it. Keeps the chip label stable for a short
    /// window after the spinner glyph cycles off-screen (Claude
    /// typically pauses for a few hundred ms between "Wandering…"
    /// and "Pondering…" — without stickiness the chip flips back
    /// to the static OSC title and flickers).
    last_status: Option<(String, std::time::Instant)>,
    /// Snapshot of this tab's `Grid` and cursor index, captured when
    /// the tab is in the background. The live grid sits on `Gpu` for
    /// the *active* tab; switching tabs swaps the current live grid
    /// into the outgoing tab's snapshot and restores the incoming
    /// tab's snapshot into the GPU. `None` ⇒ this tab is currently
    /// active (its grid is on the GPU, not here).
    grid_snapshot: Option<grid::Grid>,
    last_cursor_snapshot: Option<usize>,
}

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    mods: ModifiersState,
    cursor_cell: (u16, u16),
    /// Raw cursor pixel position from the most recent `CursorMoved`.
    /// Cached so `MouseInput` can hit-test the strip region (where pixel
    /// resolution matters — chip rects sit between cell boundaries).
    cursor_px: (f64, f64),
    buttons_down: u8,
    /// Open tabs. Always non-empty (closing the last tab exits the
    /// process). Single-tab today; multi-tab pieces (keybinds, chip
    /// rendering, per-tab grids) land in follow-up commits.
    tabs: Vec<Tab>,
    /// Index into `tabs` of the currently visible tab. Invariant:
    /// `active < tabs.len()`.
    active: usize,
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
    /// Template for spawning a new Native (editor) tab. Captured at
    /// startup when `--editor` is set; used by ⌘T to spin up another
    /// mnml instance on a fresh socket. `None` ⇒ shell mode (⌘T opens
    /// a shell instead).
    editor_template: Option<EditorTabTemplate>,
    /// Monotonic counter for unique per-tab Native socket paths.
    /// Combined with the process PID to keep tab sockets disjoint
    /// (`tmnl-<pid>-<nonce>.sock`).
    native_tab_nonce: u32,
    /// Index of the tab currently being dragged. Set on a chip
    /// left-press, cleared on left-release. While `Some`, a
    /// `CursorMoved` event over a *different* chip swaps `tabs[src]`
    /// and `tabs[dst]` and updates the index.
    dragging_tab: Option<usize>,
}

#[derive(Clone)]
struct EditorTabTemplate {
    command: PathBuf,
    workspace: PathBuf,
    extra_args: Vec<String>,
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
    /// Strip pipeline — paints the tab-strip background rect.
    strip_pipeline: pipeline::StripPipeline,
    /// Pixel-x extents `(x0, x1, tab_idx)` of every rendered chip,
    /// captured by `strip_chip_instances`. Used by the main event loop
    /// to route strip-region mouse clicks (left → switch_to_tab,
    /// middle → close_tab_at).
    strip_chip_rects: Vec<(f32, f32, usize)>,
    /// Pixel-x extents `(x0, x1, tab_idx)` of the trailing `⊗` close
    /// badge on each non-active chip. Click → `close_tab_at`. Active
    /// chip has no close badge (the user closes the active tab via
    /// ⌘W or middle-click).
    strip_chip_close_rects: Vec<(f32, f32, usize)>,
    /// Pixel-x extent `(x0, x1)` of the trailing `+` new-tab button.
    /// Painted only when chips are visible. Click → `new_shell_tab`.
    strip_new_tab_rect: Option<(f32, f32)>,
    /// Tab chips painted in the strip. `(label, is_active, attention)` per tab,
    /// in display order. App rewrites this each tick. Empty Vec ⇒
    /// strip is bg only. Length 1 ⇒ single label, centered (Safari-
    /// style "window title"). Length > 1 ⇒ N chips left-aligned
    /// after the traffic-light buttons.
    strip_chips: Vec<(String, bool, bool)>,
    /// Current chrome height (px). Refreshed each tick — shrinks to
    /// `MACOS_TAB_STRIP_PX_SINGLE` when only one tab is open and the
    /// chip strip would be empty (gives the user the pre-tabs
    /// look), grows to `MACOS_TAB_STRIP_PX_MULTI` when chips appear.
    strip_h: f32,
    /// Font zoom multiplier applied to `FONT_PX` (1.0 = default).
    /// ⌘+ / ⌘- step it; ⌘0 resets. Clamped to [`FONT_ZOOM_MIN`,
    /// `FONT_ZOOM_MAX`]. Rebuilds the atlas + cell pipeline on change.
    font_zoom: f32,
}

const FONT_ZOOM_MIN: f32 = 0.6;
const FONT_ZOOM_MAX: f32 = 3.0;
const FONT_ZOOM_STEP: f32 = 0.15;

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
        let (cols, rows) = grid_dims(
            size.width,
            size.height,
            &atlas,
            inset_px,
            MACOS_TAB_STRIP_PX_SINGLE,
        );
        let g = Grid::new(cols, rows, CLEAR_BG);

        let pipeline = CellPipeline::new(&device, format, &atlas, (cols * rows).max(1024) as u64);
        let strip_pipeline = pipeline::StripPipeline::new(&device, format);

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
            strip_pipeline,
            strip_chips: Vec::new(),
            strip_chip_rects: Vec::new(),
            strip_chip_close_rects: Vec::new(),
            strip_new_tab_rect: None,
            // Default to the minimal (single-tab) chrome height; App
            // bumps to the taller multi-tab value once a second tab
            // is added.
            strip_h: MACOS_TAB_STRIP_PX_SINGLE,
            font_zoom: 1.0,
        }
    }

    /// Rebuild the glyph atlas + cell pipeline at a new font-px multiplier.
    /// Cap at `[FONT_ZOOM_MIN, FONT_ZOOM_MAX]`. Returns the new `(cols, rows)`
    /// when the cell-grid dimensions change so callers can forward the resize
    /// to the hosted shell / native client. No-op when the zoom is unchanged
    /// after clamping.
    fn set_font_zoom(&mut self, zoom: f32) -> Option<(u16, u16)> {
        let target = zoom.clamp(FONT_ZOOM_MIN, FONT_ZOOM_MAX);
        if (self.font_zoom - target).abs() < f32::EPSILON {
            return None;
        }
        self.font_zoom = target;
        let new_atlas = match Atlas::new(&self.device, &self.queue, FONT_PX * self.scale * target) {
            Ok(a) => a,
            Err(e) => {
                log::warn!("font zoom: atlas rebuild failed ({e}); keeping previous size");
                return None;
            }
        };
        let (w, h) = (self.config.width, self.config.height);
        let (cols, rows) = grid_dims(w, h, &new_atlas, self.inset_px, self.strip_h);
        let new_pipeline = CellPipeline::new(
            &self.device,
            self.config.format,
            &new_atlas,
            (cols * rows).max(1024) as u64,
        );
        self.atlas = new_atlas;
        self.pipeline = new_pipeline;
        self.last_cursor = None;
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            return Some((cols as u16, rows as u16));
        }
        None
    }

    /// Set the chip list rendered in the tab strip. App calls this
    /// each tick with one entry per open tab
    /// (`(label, is_active, attention)`). Skips the write when
    /// contents haven't changed.
    fn set_strip_chips(&mut self, chips: &[(String, bool, bool)]) {
        if self.strip_chips.len() != chips.len()
            || self
                .strip_chips
                .iter()
                .zip(chips)
                .any(|((a, b, c), (d, e, f))| a != d || b != e || c != f)
        {
            self.strip_chips = chips.to_vec();
        }
    }

    /// Pixel-x where multi-chip rendering starts (clear of the macOS
    /// traffic-light buttons + a comfortable guard so the first chip
    /// doesn't visually touch the rightmost button).
    const CHIP_START_X_PX: f32 = 170.0;
    /// Inner padding around the chip label (one cell-width on each
    /// side so the chip reads as a pill rather than just colored
    /// text).
    const CHIP_PAD_CELLS: f32 = 1.0;
    /// Spacing between adjacent chips.
    const CHIP_GAP_CELLS: f32 = 1.0;

    /// Build glyph instances for the current chip list, positioned to
    /// land inside the tab strip. Uses fractional / negative `cell_pos`
    /// values so the existing cell pipeline draws each glyph (and its
    /// per-cell bg) at pixel-precise locations regardless of inset.
    /// Layout per chip (consistent active / inactive, single / multi):
    ///
    ///   ` <attn?> <label> · × `
    ///
    /// Active chip: bg = `ACTIVE_CHIP_BG` (lightened) + BOLD fg.
    /// Inactive chip: bg = STRIP_BG, fg = DIM_FG.
    /// Attention chip (inactive only): a leading `● ` in red.
    /// The `×` close glyph is muted (no red shout) and sits one cell
    /// away from the label so it doesn't crowd the text. Always present
    /// — click is a no-op on single-tab (close_tab_at refuses), but the
    /// visual stays consistent across active / inactive and single /
    /// multi-tab. After the last chip: a `+` new-tab button.
    fn strip_chip_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        self.strip_chip_rects.clear();
        self.strip_chip_close_rects.clear();
        self.strip_new_tab_rect = None;
        // Skip rendering chips when there's only one tab — the user
        // sees just the bare title-bar inset (pre-tabs look). Also
        // bail when strip is disabled (non-macOS).
        if self.strip_chips.len() <= 1 || self.strip_h <= 0.0 {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;
        let label_y_px = ((self.strip_h - cell_h) * 0.5).max(0.0);
        let inset_y_total = self.inset_px + self.strip_h;
        let base_y = (label_y_px - inset_y_total) / cell_h;
        // Active chip's bg — a lightened version of STRIP_BG so the
        // active tab stands out as a pill. Roughly `STRIP_BG + 0.06`.
        const ACTIVE_CHIP_BG: [f32; 4] = [0.21, 0.24, 0.28, 1.0];
        // Attention dot color — red, matches OSC 1337 "needs attention"
        // urgency level.
        const ATTENTION_FG: [f32; 4] = [0.95, 0.32, 0.32, 1.0];
        // Muted close glyph color — dimmer than dim_fg so the `×` reads
        // as chrome rather than a callout. Brighter on the active chip
        // (TEXT_FG-dimmed) so it's discoverable but not loud.
        const CLOSE_FG_INACTIVE: [f32; 4] = [0.40, 0.42, 0.48, 1.0];
        const CLOSE_FG_ACTIVE: [f32; 4] = [0.70, 0.72, 0.78, 1.0];
        const ATTR_BOLD: u32 = 1;

        // Always left-align starting from CHIP_START_X_PX — no Safari-style
        // centering for the single-tab case (it caused a jarring shift left
        // when the user opened a second tab).
        let start_x_px = Self::CHIP_START_X_PX;
        let base_x = (start_x_px - self.inset_px) / cell_w;
        let mut col_offset = 0.0_f32;
        // Snapshot chips so we don't borrow self.strip_chips through
        // the loop (atlas.glyph wants &mut self.atlas concurrently).
        let chips: Vec<(String, bool, bool)> = self.strip_chips.clone();
        let space_g = self.atlas.glyph(' ', style_from_attrs(0), &self.queue);
        let mut out: Vec<pipeline::Instance> = Vec::new();

        let push_pad = |out: &mut Vec<pipeline::Instance>,
                        col_offset: &mut f32,
                        bg: [f32; 4],
                        fg: [f32; 4],
                        space_g: &crate::atlas::AtlasGlyph| {
            out.push(pipeline::Instance {
                cell_pos: [base_x + *col_offset, base_y],
                fg,
                bg,
                uv_min: space_g.uv_min,
                uv_max: space_g.uv_max,
                glyph_offset: space_g.offset,
                glyph_size: space_g.size,
                attrs: 0,
                _pad: 0,
            });
            *col_offset += 1.0;
        };

        for (i, (label, active, attention)) in chips.iter().enumerate() {
            let chip_x0_px = start_x_px + col_offset * cell_w;
            let (fg, bg, attrs) = if *active {
                (TEXT_FG, ACTIVE_CHIP_BG, ATTR_BOLD)
            } else {
                (DIM_FG, STRIP_BG, 0)
            };
            // Left pad.
            for _ in 0..Self::CHIP_PAD_CELLS as usize {
                push_pad(&mut out, &mut col_offset, bg, fg, &space_g);
            }
            // Attention dot — only on inactive chips (active tab clears
            // the flag on focus). Red `●` + a trailing space.
            if *attention && !*active {
                let dot_g = self.atlas.glyph('●', style_from_attrs(0), &self.queue);
                out.push(pipeline::Instance {
                    cell_pos: [base_x + col_offset, base_y],
                    fg: ATTENTION_FG,
                    bg,
                    uv_min: dot_g.uv_min,
                    uv_max: dot_g.uv_max,
                    glyph_offset: dot_g.offset,
                    glyph_size: dot_g.size,
                    attrs: 0,
                    _pad: 0,
                });
                col_offset += 1.0;
                push_pad(&mut out, &mut col_offset, bg, fg, &space_g);
            }
            // Label glyphs.
            for ch in label.chars() {
                let g = self.atlas.glyph(ch, style_from_attrs(attrs), &self.queue);
                out.push(pipeline::Instance {
                    cell_pos: [base_x + col_offset, base_y],
                    fg,
                    bg,
                    uv_min: g.uv_min,
                    uv_max: g.uv_max,
                    glyph_offset: g.offset,
                    glyph_size: g.size,
                    attrs,
                    _pad: 0,
                });
                col_offset += 1.0;
            }
            // One-cell gap between label and the `×` close glyph — the
            // close used to sit flush against the label and crowded it.
            push_pad(&mut out, &mut col_offset, bg, fg, &space_g);
            // `×` close glyph — painted on every chip (single and
            // multi-tab, active and inactive). Muted on inactive,
            // slightly brighter on active.
            let close_fg = if *active {
                CLOSE_FG_ACTIVE
            } else {
                CLOSE_FG_INACTIVE
            };
            let close_g = self
                .atlas
                .glyph('\u{00D7}', style_from_attrs(0), &self.queue);
            let close_x_px = start_x_px + col_offset * cell_w;
            out.push(pipeline::Instance {
                cell_pos: [base_x + col_offset, base_y],
                fg: close_fg,
                bg,
                uv_min: close_g.uv_min,
                uv_max: close_g.uv_max,
                glyph_offset: close_g.offset,
                glyph_size: close_g.size,
                attrs: 0,
                _pad: 0,
            });
            col_offset += 1.0;
            // Right pad.
            for _ in 0..Self::CHIP_PAD_CELLS as usize {
                push_pad(&mut out, &mut col_offset, bg, fg, &space_g);
            }
            // Record the chip's pixel-x extent BEFORE moving past the
            // gap so the click region matches the painted chip exactly.
            let chip_x1_px = start_x_px + col_offset * cell_w;
            self.strip_chip_rects.push((chip_x0_px, chip_x1_px, i));
            self.strip_chip_close_rects
                .push((close_x_px, close_x_px + cell_w, i));
            // Inter-chip gap.
            col_offset += Self::CHIP_GAP_CELLS;
        }
        // `+` new-tab button after the gap past the last chip.
        let plus_x_px = start_x_px + col_offset * cell_w;
        self.push_plus_button(&mut out, plus_x_px, base_y);
        out
    }

    /// Paint the trailing `+` new-tab button at `plus_x_px` and record
    /// its pixel-x extent on `strip_new_tab_rect`. The chrome is a
    /// single glyph (`+`) padded left/right with two spaces so the
    /// click target is comfortably-sized.
    fn push_plus_button(&mut self, out: &mut Vec<pipeline::Instance>, plus_x_px: f32, base_y: f32) {
        use crate::atlas::style_from_attrs;
        // Slightly lifted bg so the button reads as chrome rather than
        // strip filler. Same shade as the active chip.
        const PLUS_BG: [f32; 4] = [0.18, 0.20, 0.24, 1.0];
        let cell_w = self.atlas.cell_w;
        let plus_x = (plus_x_px - self.inset_px) / cell_w;
        let space_g = self.atlas.glyph(' ', style_from_attrs(0), &self.queue);
        let plus_g = self.atlas.glyph('+', style_from_attrs(0), &self.queue);
        // 3-cell button: [space, +, space]
        for (i, g) in [&space_g, &plus_g, &space_g].iter().enumerate() {
            out.push(pipeline::Instance {
                cell_pos: [plus_x + i as f32, base_y],
                fg: TEXT_FG,
                bg: PLUS_BG,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
        }
        self.strip_new_tab_rect = Some((plus_x_px, plus_x_px + 3.0 * cell_w));
    }

    fn resize(&mut self, w: u32, h: u32) -> Option<(u16, u16)> {
        if w == 0 || h == 0 {
            return None;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        let (cols, rows) = grid_dims(w, h, &self.atlas, self.inset_px, self.strip_h);
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
        let (cols, rows) = grid_dims(w, h, &self.atlas, self.inset_px, self.strip_h);
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            self.last_cursor = None;
            return Some((cols as u16, rows as u16));
        }
        None
    }

    /// Update the strip-chrome height live. Returns the new grid dims
    /// if they shifted so the caller can forward the resize to the
    /// native client. The strip grows when chips appear (multi-tab)
    /// and shrinks back to a bare title-bar inset when only one tab
    /// is left — matching the pre-tabs look.
    fn set_strip_h(&mut self, strip_h: f32) -> Option<(u16, u16)> {
        if (self.strip_h - strip_h).abs() < f32::EPSILON {
            return None;
        }
        self.strip_h = strip_h;
        let (w, h) = (self.config.width, self.config.height);
        let (cols, rows) = grid_dims(w, h, &self.atlas, self.inset_px, self.strip_h);
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            self.last_cursor = None;
            return Some((cols as u16, rows as u16));
        }
        None
    }

    fn apply_frame(&mut self, f: &Frame) {
        apply_frame_to_grid(&mut self.grid, &mut self.last_cursor, f);
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
        // and `inset_px + strip_h` on top so the tab-strip
        // chrome row sits above the grid. The strip pipeline paints
        // its background rect over the same `[0, 0]..[w, strip_h]`
        // area in the same render pass — the cell pipeline draws
        // on top with `inset_y >= strip_h`, so no overlap.
        // inset == 0 still gets the ceil'd grid_dims so the rightmost
        // cells reach the edge — true edge-to-edge for TUIs.
        self.pipeline.write_globals(
            &self.queue,
            [self.config.width as f32, self.config.height as f32],
            [self.atlas.cell_w, self.atlas.cell_h],
            [self.inset_px, self.inset_px + self.strip_h],
        );
        // Single-tab: paint the strip in CLEAR_BG so it blends with the
        // surrounding clear color (no visible chrome band — but the grid
        // still starts below the strip so content doesn't kiss the macOS
        // traffic lights). Multi-tab: STRIP_BG separates the chip strip
        // from the body.
        let strip_color = if self.strip_chips.len() <= 1 {
            CLEAR_BG
        } else {
            STRIP_BG
        };
        self.strip_pipeline.write_globals(
            &self.queue,
            [self.config.width as f32, self.config.height as f32],
            self.strip_h,
            strip_color,
        );
        let mut instances = CellPipeline::build_instances(&self.grid, &mut self.atlas, &self.queue);
        // Append tab-strip label glyphs (rendered through the same cell
        // pipeline via fractional `cell_pos` values — they land in the
        // strip area above the grid).
        instances.extend(self.strip_chip_instances());
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
                label: Some("frame"),
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
            // Strip background first — the cell pipeline draws on top.
            pass.set_pipeline(&self.strip_pipeline.pipeline);
            pass.set_bind_group(0, &self.strip_pipeline.bind_group, &[]);
            pass.draw(0..4, 0..1);
            // Cell grid (body content).
            pass.set_pipeline(&self.pipeline.pipeline);
            pass.set_bind_group(0, &self.pipeline.bind_group, &[]);
            pass.set_vertex_buffer(0, self.pipeline.instance_buf.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

fn grid_dims(w: u32, h: u32, atlas: &Atlas, inset_px: f32, strip: f32) -> (u32, u32) {
    // `inset_px == 0` → edge-to-edge horizontally; vertically we
    // reserve `strip` pixels for the tab-strip chrome (caller passes
    // the dynamic strip height: shrinks to the single-tab value when
    // there's only one tab, grows when chips appear). Ceil cols so
    // the rightmost cells reach the window edge (the partial overflow
    // is clipped by the wgpu surface — no clear-bg stripe at the right
    // seam). Floor rows so the LAST cell row gets its full font-row
    // height — any leftover sub-row pixels at the bottom become a
    // small letterbox gutter painted in `CLEAR_BG` by the wgpu clear
    // (industry standard: Apple Terminal, iTerm2, Alacritty, Kitty all
    // do this). The alternative — ceiling rows + clipping the last
    // partial row — leaves a few-pixel sliver of whatever the app drew
    // on the bottom row (status bar / cmdline), which reads as visual
    // noise.
    // `inset_px > 0` → reserve `inset_px` pixels on every side
    // (and tab-strip on top); floor cols/rows so the cells fit inside.
    if inset_px <= 0.0 {
        let cols = (w as f32 / atlas.cell_w).ceil().max(1.0) as u32;
        let usable_h = (h as f32 - strip).max(atlas.cell_h);
        let rows = (usable_h / atlas.cell_h).floor().max(1.0) as u32;
        return (cols, rows);
    }
    let usable_w = (w as f32 - 2.0 * inset_px).max(atlas.cell_w);
    let usable_h = (h as f32 - 2.0 * inset_px - strip).max(atlas.cell_h);
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
        match &mut self.tabs[self.active].mode {
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
                let resize = gpu.resize(size.width, size.height);
                // Always paint a frame after a resize — the surface was
                // reconfigured (even if cols×rows stayed the same), so
                // the framebuffer is fresh and would briefly show through
                // as CLEAR_BG until the next event-driven render. Without
                // this the window flickers during interactive resizes.
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                let Some((cols, rows)) = resize else {
                    return;
                };
                // Resize every tab's background grid-snapshot to match
                // the new window size — otherwise switching to a tab
                // whose snapshot was captured at the old dimensions
                // would render a stale-size grid (mismatched cell
                // count, wrong cursor positions). For shell tabs we
                // ALSO resize the underlying pty so the program inside
                // re-paints at the new size.
                for (i, tab) in self.tabs.iter_mut().enumerate() {
                    if i != self.active
                        && let Some(snap) = tab.grid_snapshot.as_mut()
                    {
                        snap.resize(cols as u32, rows as u32);
                    }
                    if let Mode::Shell { session: Some(s) } = &mut tab.mode {
                        s.resize(rows, cols);
                    } else if let Mode::Native { server, conn, .. } = &mut tab.mode {
                        if i == self.active {
                            server.send_resize(cols, rows);
                            if *conn != ConnState::Streaming {
                                paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                            }
                        } else {
                            server.send_resize(cols, rows);
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
                // Tab-management chords: ⌘T new, ⌘W close, ⌘1..⌘9 jump,
                // ⌘⇧[ / ⌘⇧] cycle. macOS Terminal / iTerm / Safari
                // conventions. Cmd = winit's `super_key()`. Intercept
                // before forwarding to the active tab's Mode so the
                // chord never reaches the hosted process.
                if self.mods.super_key()
                    && let Key::Character(s) = &ke.logical_key
                {
                    let c = s.chars().next();
                    let shift = self.mods.shift_key();
                    match (c, shift) {
                        (Some('t'), false) => {
                            // ⌘T spawns the same kind of tab the window
                            // launched with — Native when --editor was
                            // set, shell otherwise.
                            if self.editor_template.is_some() {
                                self.new_native_tab();
                            } else {
                                self.new_shell_tab();
                            }
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        (Some('w'), false) => {
                            self.close_active_tab(event_loop);
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        (Some(d), false) if d.is_ascii_digit() && d != '0' => {
                            let n = d.to_digit(10).unwrap() as usize - 1;
                            self.switch_to_tab(n);
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        (Some('['), true) => {
                            self.cycle_tab(false);
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        (Some(']'), true) => {
                            self.cycle_tab(true);
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        // Font zoom: ⌘+ / ⌘= zoom in (both share the
                        // physical key — `=` unmodified, `+` with shift),
                        // ⌘- zoom out, ⌘0 reset. macOS Terminal / iTerm /
                        // browser convention.
                        (Some('='), _) | (Some('+'), _) => {
                            self.zoom_font(FONT_ZOOM_STEP);
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        (Some('-'), false) | (Some('_'), _) => {
                            self.zoom_font(-FONT_ZOOM_STEP);
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        (Some('0'), false) => {
                            self.reset_font_zoom();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        // Mac-style editing chords → translate to Ctrl-equivalent
                        // for the hosted Native client (mnml understands Ctrl+Z
                        // as undo, Ctrl+C/V/X/A/S/F for clipboard/select-all/
                        // save/find). Only fires for Native tabs — Shell tabs
                        // are bare terminals where remapping Cmd would break
                        // ⌘C-as-copy / ⌘V-as-paste in the surrounding OS.
                        (Some(ch), _)
                            if matches!(
                                ch,
                                'z' | 'x' | 'c' | 'v' | 'a' | 's' | 'f' | 'n'
                            ) && matches!(
                                &self.tabs[self.active].mode,
                                Mode::Native { .. }
                            ) =>
                        {
                            let translated_mods = pack_mods_cmd_to_ctrl(self.mods);
                            if let Mode::Native { server, .. } =
                                &mut self.tabs[self.active].mode
                                && let Some(code) =
                                    translate_key(&ke.logical_key, self.mods)
                            {
                                server.send_input(&InputEvent::Key(KeyInput {
                                    code,
                                    mods: translated_mods,
                                    press: true,
                                }));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                match &mut self.tabs[self.active].mode {
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
                    self.cursor_px = (position.x, position.y);
                    // Drags that originated in the strip stay in chrome —
                    // don't forward them as terminal drag events. (Hit-test
                    // by Y: anything above the grid is chrome.)
                    let in_chrome = position.y < (gpu.inset_px + gpu.strip_h) as f64;
                    if in_chrome {
                        // Drag-to-reorder: while a chip drag is armed
                        // and the cursor crosses into a different
                        // chip's rect, swap the two tabs. Chip rects
                        // get re-computed each render, so subsequent
                        // moves keep dragging the same tab through
                        // the strip.
                        if let Some(src) = self.dragging_tab
                            && self.buttons_down & (1u8 << BUTTON_LEFT) != 0
                        {
                            let dst = gpu
                                .strip_chip_rects
                                .iter()
                                .find(|(x0, x1, _)| {
                                    position.x >= *x0 as f64 && position.x < *x1 as f64
                                })
                                .map(|(_, _, idx)| *idx);
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
                    if let Mode::Native { server, .. } = &self.tabs[self.active].mode
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
                // Strip-region intercept — clicks on tab chips switch
                // (left) / close (middle). Tested against the cached
                // pixel cursor against the rects produced by the last
                // `strip_chip_instances` pass. Runs only on press so
                // releases don't fire a second time; never forwards
                // strip clicks to the terminal (return early).
                if let Some(gpu) = &self.gpu {
                    let (px, py) = self.cursor_px;
                    let in_chrome = py < (gpu.inset_px + gpu.strip_h) as f64;
                    if in_chrome {
                        if pressed {
                            // `+` new-tab button sits past the last chip;
                            // check it first since its rect is disjoint.
                            let on_plus = gpu
                                .strip_new_tab_rect
                                .map(|(x0, x1)| px >= x0 as f64 && px < x1 as f64)
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
                                .find(|(x0, x1, _)| px >= *x0 as f64 && px < *x1 as f64)
                                .map(|(_, _, idx)| *idx);
                            if let Some(idx) = close_hit
                                && button == MouseButton::Left
                            {
                                self.close_tab_at(idx, event_loop);
                                return;
                            }
                            let hit = gpu
                                .strip_chip_rects
                                .iter()
                                .find(|(x0, x1, _)| px >= *x0 as f64 && px < *x1 as f64)
                                .map(|(_, _, idx)| *idx);
                            if let Some(idx) = hit {
                                match button {
                                    MouseButton::Left => {
                                        self.switch_to_tab(idx);
                                        // Arm a potential drag — a
                                        // subsequent CursorMoved over
                                        // a different chip will swap.
                                        self.dragging_tab = Some(self.active);
                                    }
                                    MouseButton::Middle => self.close_tab_at(idx, event_loop),
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
                // Outside the strip — releasing the left button also
                // ends a drag that started in chrome.
                if !pressed && button == MouseButton::Left {
                    self.dragging_tab = None;
                }
                if let Mode::Native { server, .. } = &self.tabs[self.active].mode {
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
                // Don't scroll the terminal when the wheel event lands in
                // the chrome strip — chip-scrolling can come later but
                // forwarding now would scroll the underlying terminal,
                // which is surprising when the cursor is on a chip.
                if let Some(gpu) = &self.gpu {
                    let (_, py) = self.cursor_px;
                    if py < (gpu.inset_px + gpu.strip_h) as f64 {
                        return;
                    }
                }
                let Mode::Native { server, .. } = &self.tabs[self.active].mode else {
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

/// Same as [`pack_mods`] but swaps Super (Mac Cmd) for Ctrl — used when
/// translating Mac-style editing chords (⌘Z / ⌘C / etc) into their Ctrl
/// equivalents on the wire so the hosted Linux/cross-platform app sees
/// what it expects.
fn pack_mods_cmd_to_ctrl(m: ModifiersState) -> u8 {
    let mut out = 0u8;
    if m.shift_key() {
        out |= MOD_SHIFT;
    }
    if m.control_key() || m.super_key() {
        out |= MOD_CTRL;
    }
    if m.alt_key() {
        out |= MOD_ALT;
    }
    out
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
        let inset_y = self.inset_px as f64 + self.strip_h as f64;
        let col = ((px - inset_x).max(0.0) / self.atlas.cell_w as f64).floor() as u16;
        let row = ((py - inset_y).max(0.0) / self.atlas.cell_h as f64).floor() as u16;
        (col, row)
    }
}

/// Apply a `Frame` (diff runs + cursor metadata) to an arbitrary
/// `Grid` + `last_cursor` slot. Pulled out of `Gpu::apply_frame` so
/// background Native tabs can keep their off-screen `grid_snapshot`
/// up to date even when their tab isn't the active one. On switch,
/// the snapshot is then already current and the user sees the
/// latest state immediately (instead of the stale snapshot from
/// when this tab was last active).
fn apply_frame_to_grid(grid: &mut grid::Grid, last_cursor: &mut Option<usize>, f: &Frame) {
    // Clear the previous cursor's overlay bits first — runs may not
    // cover that cell, so we have to do it explicitly.
    if let Some(old) = *last_cursor
        && let Some(c) = grid.cells.get_mut(old)
    {
        c.attrs &= !(ATTR_CURSOR_BLOCK | ATTR_CURSOR_UNDERLINE | ATTR_CURSOR_BAR);
    }

    let grid_cols = grid.cols;
    let grid_rows = grid.rows;
    let grid_max = (grid_cols * grid_rows) as usize;
    let frame_cols = f.cols as u32;
    for run in &f.runs {
        let start = run.start as usize;
        for (i, wc) in run.cells.iter().enumerate() {
            let abs = start + i;
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
            grid.cells[dst] = grid::Cell {
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
        grid.cells[i].attrs |= bit;
        *last_cursor = Some(i);
    } else {
        *last_cursor = None;
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
    /// Append a fresh shell tab and switch to it. Sized from the
    /// current GPU grid; spawn failures leave the existing active
    /// tab in place and toast the error to stderr.
    /// Spawn a new Native (editor) tab — fresh socket, fresh Server,
    /// fresh Launcher pointing at the same `editor_template.command`
    /// the original tab used. No-op when shell mode is active
    /// (`editor_template == None`).
    fn new_native_tab(&mut self) {
        let Some(tmpl) = self.editor_template.clone() else {
            // Fall back to a shell tab — gives ⌘T a sensible behavior
            // in shell-mode windows.
            self.new_shell_tab();
            return;
        };
        let Some(gpu) = self.gpu.as_mut() else {
            return;
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
        // Save outgoing tab's grid + create a fresh blank for the new
        // Native tab (mirrors new_shell_tab).
        self.tabs[self.active].grid_snapshot = Some(gpu.grid.clone());
        self.tabs[self.active].last_cursor_snapshot = gpu.last_cursor;
        gpu.grid = grid::Grid::new(gpu.grid.cols, gpu.grid.rows, CLEAR_BG);
        gpu.last_cursor = None;
        let tab = Tab {
            mode: Mode::Native {
                server,
                conn: ConnState::Waiting,
                launcher,
                client_title: None,
            },
            label: "mnml".to_string(),
            attention: false,
            last_status: None,
            grid_snapshot: None,
            last_cursor_snapshot: None,
        };
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    fn new_shell_tab(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let (cols, rows) = (gpu.grid.cols as u16, gpu.grid.rows as u16);
        match ShellSession::spawn(rows, cols, TEXT_FG, CLEAR_BG) {
            Ok(s) => {
                // Save outgoing tab's grid into its snapshot, then
                // create a fresh blank grid on the GPU for the new tab.
                self.tabs[self.active].grid_snapshot = Some(gpu.grid.clone());
                self.tabs[self.active].last_cursor_snapshot = gpu.last_cursor;
                gpu.grid = grid::Grid::new(gpu.grid.cols, gpu.grid.rows, CLEAR_BG);
                gpu.last_cursor = None;
                let initial_label = s.shell_name().to_string();
                let tab = Tab {
                    mode: Mode::Shell { session: Some(s) },
                    label: initial_label,
                    attention: false,
                    last_status: None,
                    grid_snapshot: None,
                    last_cursor_snapshot: None,
                };
                self.tabs.push(tab);
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
    fn close_active_tab(&mut self, event_loop: &ActiveEventLoop) {
        self.close_tab_at(self.active, event_loop);
    }

    /// Close a specific tab by index. Used by middle-click on a chip;
    /// when `idx` is the active tab, behaves identically to
    /// `close_active_tab`. Closing the last tab exits the process.
    fn close_tab_at(&mut self, idx: usize, event_loop: &ActiveEventLoop) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs.len() <= 1 {
            event_loop.exit();
            return;
        }
        let was_active = idx == self.active;
        self.tabs.remove(idx);
        if was_active {
            // Active tab closed — clamp + swap from prev_active=self.active
            // (the now-stale slot, which `swap_active_tab` treats as a
            // no-op for the outgoing save).
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
            self.swap_active_tab(self.active);
        } else if idx < self.active {
            // Closed a non-active tab to the left — shift active index
            // down so it still points at the same tab.
            self.active -= 1;
        }
        // If `idx > self.active`, the active index is unaffected.
    }

    /// Switch to tab `idx` (0-based). No-op if out of range. Restores
    /// the new tab's `grid_snapshot` to the GPU.
    fn switch_to_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() || idx == self.active {
            return;
        }
        let prev = self.active;
        self.active = idx;
        self.swap_active_tab(prev);
    }

    /// Cycle to the next (`forward=true`) / previous tab, wrapping.
    /// Rebuild the atlas at a new font-zoom level (relative step). After
    /// resizing the grid the new cell dims are forwarded to the active
    /// tab's session so the hosted shell / mnml repaints at the new
    /// dimensions.
    fn zoom_font(&mut self, delta: f32) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let target = gpu.font_zoom + delta;
        let resize = gpu.set_font_zoom(target);
        self.forward_font_resize(resize);
    }

    /// Reset the font zoom to 1.0 (⌘0). Same resize plumbing as `zoom_font`.
    fn reset_font_zoom(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let resize = gpu.set_font_zoom(1.0);
        self.forward_font_resize(resize);
    }

    /// Pipe a grid resize (caused by a font-zoom change) to the active
    /// tab's session — same shape as the inset / strip resize paths.
    fn forward_font_resize(&mut self, resize: Option<(u16, u16)>) {
        let Some((cols, rows)) = resize else {
            return;
        };
        match &mut self.tabs[self.active].mode {
            Mode::Shell { session } => {
                if let Some(s) = session.as_mut() {
                    s.resize(rows, cols);
                }
            }
            Mode::Native { server, .. } => {
                server.send_resize(cols, rows);
            }
        }
    }

    fn cycle_tab(&mut self, forward: bool) {
        if self.tabs.len() <= 1 {
            return;
        }
        let prev = self.active;
        let n = self.tabs.len();
        self.active = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
        self.swap_active_tab(prev);
    }

    /// Swap grids: save the outgoing tab's current grid into its
    /// snapshot, restore the incoming tab's snapshot onto the GPU.
    /// Caller has already updated `self.active` to the NEW index;
    /// this method needs the OLD index so it knows which tab to
    /// save into. After the swap, the active tab's grid is on the
    /// GPU; background tabs hold their grids in `grid_snapshot`.
    ///
    /// For Native tabs whose snapshot is still empty (no frame
    /// received yet), repaint the idle banner so the chrome isn't
    /// blank while the client reconnects.
    fn swap_active_tab(&mut self, prev_active: usize) {
        // Active tab can't be in "attention" state — the user is
        // looking at it. Clear the badge as part of the switch.
        self.tabs[self.active].attention = false;
        let Some(gpu) = self.gpu.as_mut() else { return };
        // Stash outgoing.
        if prev_active < self.tabs.len() && prev_active != self.active {
            self.tabs[prev_active].grid_snapshot = Some(gpu.grid.clone());
            self.tabs[prev_active].last_cursor_snapshot = gpu.last_cursor;
        }
        // Restore incoming.
        if let Some(snap) = self.tabs[self.active].grid_snapshot.take() {
            gpu.grid = snap;
            gpu.last_cursor = self.tabs[self.active].last_cursor_snapshot.take();
        } else {
            // First focus on a tab that hasn't drawn yet — fall back
            // to a blank grid + idle banner (Native) or
            // apply_to_grid (Shell).
            gpu.grid = grid::Grid::new(gpu.grid.cols, gpu.grid.rows, CLEAR_BG);
            gpu.last_cursor = None;
            match &mut self.tabs[self.active].mode {
                Mode::Shell { session: Some(s) } => {
                    let (cc, cr, vis) = s.apply_to_grid(&mut gpu.grid);
                    if vis && (cc as u32) < gpu.grid.cols && (cr as u32) < gpu.grid.rows {
                        let i = (cr as u32 * gpu.grid.cols + cc as u32) as usize;
                        let suppress = cc == 0 && cr == 0 && {
                            let cell = &gpu.grid.cells[i];
                            cell.ch == ' ' && cell.attrs == 0
                        };
                        if !suppress {
                            gpu.grid.cells[i].attrs |= ATTR_CURSOR_BLOCK;
                            gpu.last_cursor = Some(i);
                        }
                    }
                }
                Mode::Native { conn, server, .. } => {
                    paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                }
                _ => {}
            }
        }
    }

    fn tick(&mut self, event_loop: &ActiveEventLoop) {
        // Drain the menu-event channel first — `muda` delivers menu picks
        // (and accelerator-fired items like ⌘, / ⌘+ / ⌘-) through this
        // global channel. Acting on them before the per-frame work means
        // the next render reflects whatever the menu changed.
        self.drain_menu_events();

        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        // Refresh each tab's strip label from its current Mode/Conn,
        // then hand the active-tab marker list to the renderer. Cheap —
        // both updates skip the write when nothing changed.
        let active_idx = self.active;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let new_label: String = match &mut tab.mode {
                Mode::Shell { session } => {
                    // Prefer the live status line if visible — Claude
                    // Code's `✽ Wandering…` spinner pattern cycles each
                    // frame so the tab label updates in sync with what
                    // the user sees inside the tab. Sticky for
                    // `STATUS_STICKY_MS` after the spinner cycles off
                    // so brief gaps between "Wandering…" /
                    // "Pondering…" don't flicker back to the OSC
                    // title. Stickiness is cleared immediately on
                    // OSC 1337 attention (Claude signaling it's truly
                    // done) — handled below.
                    const STATUS_STICKY_MS: u128 = 2000;
                    let now = std::time::Instant::now();
                    let live = session.as_ref().and_then(|s| s.detect_status_line());
                    if let Some(s) = live.clone() {
                        tab.last_status = Some((s, now));
                    }
                    let sticky = tab
                        .last_status
                        .as_ref()
                        .filter(|(_, when)| {
                            now.duration_since(*when).as_millis() < STATUS_STICKY_MS
                        })
                        .map(|(t, _)| t.clone());
                    let osc = session.as_ref().and_then(|s| {
                        let t = s.osc_title();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t.to_string())
                        }
                    });
                    // Fallback chain: sticky status > OSC title >
                    // foreground process name > shell name. The fg
                    // name surfaces when something like `vim` / `htop`
                    // / `less` is running but doesn't set an OSC title.
                    let fg = session
                        .as_mut()
                        .and_then(|s| s.fg_proc_name().map(|n| n.to_string()));
                    sticky.or(osc).or(fg).unwrap_or_else(|| {
                        session
                            .as_ref()
                            .map(|s| s.shell_name().to_string())
                            .unwrap_or_else(|| "shell".to_string())
                    })
                }
                Mode::Native {
                    conn, client_title, ..
                } => match conn {
                    ConnState::Waiting => "(no client)".to_string(),
                    ConnState::Connected => "(connecting…)".to_string(),
                    // Client-supplied title (`Message::Title`, v3) takes
                    // priority; falls back to "mnml" pre-handshake.
                    ConnState::Streaming => {
                        client_title.clone().unwrap_or_else(|| "mnml".to_string())
                    }
                },
            };
            if tab.label != new_label {
                tab.label = new_label;
            }
            // Drain the shell session's attention flag (always — even
            // for the active tab so the flag doesn't accumulate between
            // unfocused intervals). On the active tab we keep it
            // cleared; on background tabs we OR it into Tab.attention
            // so the chip badge sticks until the user actually focuses.
            // OSC 1337 also signals "Claude finished" so we drop the
            // sticky status cache — next tick the OSC title takes over.
            if let Mode::Shell { session: Some(s) } = &tab.mode {
                let new_attn = s.take_attention();
                if new_attn {
                    tab.last_status = None;
                }
                if i == active_idx {
                    tab.attention = false;
                } else if new_attn {
                    tab.attention = true;
                }
            } else if i == active_idx {
                tab.attention = false;
            }
            // Background Native tabs: drain server events here so
            // conn state + client_title stay current even when the
            // tab isn't focused. Frames are applied to the tab's
            // grid_snapshot (an off-screen Grid) so the snapshot
            // tracks live state — on switch, the user sees fresh
            // content instantly instead of the snapshot from when
            // this tab was last active.
            if i != active_idx {
                // Capture the tab's snapshot dims for any fresh
                // allocation. Borrow split: we'll need `&mut
                // tab.grid_snapshot` and `&mut tab.mode` separately.
                let (snap_cols, snap_rows) = tab
                    .grid_snapshot
                    .as_ref()
                    .map(|g| (g.cols, g.rows))
                    .unwrap_or((0, 0));
                if let Mode::Native {
                    server,
                    conn,
                    client_title,
                    launcher,
                } = &mut tab.mode
                {
                    while let Ok(ev) = server.events.try_recv() {
                        match ev {
                            ServerEvent::ClientConnected => {
                                *conn = ConnState::Connected;
                                *client_title = None;
                            }
                            ServerEvent::ClientDisconnected => {
                                *conn = ConnState::Waiting;
                                *client_title = None;
                            }
                            ServerEvent::Title(s) => {
                                *client_title = Some(s);
                            }
                        }
                    }
                    let mut got_frame = false;
                    while let Ok(f) = server.frame_rx.try_recv() {
                        got_frame = true;
                        if matches!(conn, ConnState::Connected) {
                            *conn = ConnState::Streaming;
                        }
                        // Lazy-allocate the snapshot if this tab
                        // hasn't had one set yet (just-spawned bg
                        // tab). Use the sender's dims so the diff
                        // applies cleanly without per-row clipping.
                        if tab.grid_snapshot.is_none() {
                            let cols = if snap_cols > 0 {
                                snap_cols
                            } else {
                                f.cols as u32
                            };
                            let rows = if snap_rows > 0 {
                                snap_rows
                            } else {
                                f.rows as u32
                            };
                            tab.grid_snapshot = Some(grid::Grid::new(cols, rows, CLEAR_BG));
                            tab.last_cursor_snapshot = None;
                        }
                        if let Some(snap) = tab.grid_snapshot.as_mut() {
                            apply_frame_to_grid(snap, &mut tab.last_cursor_snapshot, &f);
                        }
                    }
                    let _ = got_frame;
                    if let Some(l) = launcher.as_mut() {
                        // Keep poll lightweight — no respawn logic
                        // for background tabs (that's the active-tab
                        // path).
                        let _ = l.poll();
                    }
                }
            }
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
        let chips: Vec<(String, bool, bool)> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let label = if counts.get(t.label.as_str()).copied().unwrap_or(0) > 1 {
                    let n = seen.entry(t.label.as_str()).or_insert(0);
                    *n += 1;
                    format!("{} ({})", t.label, n)
                } else {
                    t.label.clone()
                };
                // Attention dot is painted by the chip renderer (red `●`).
                // Cleared on switch-to.
                let attention = t.attention && i != self.active;
                (label, i == self.active, attention)
            })
            .collect();
        gpu.set_strip_chips(&chips);
        // Pick the strip height to match the current mode:
        // * multi-tab → tall strip with chip space
        // * single-tab + TUI (native or shell with altscreen)
        //   → minimal band, just enough to clear the macOS
        //     traffic lights (pre-tabs look)
        // * single-tab + bare shell prompt → taller band so the
        //   first prompt row isn't kissing the traffic lights
        //   (the strip pipeline paints this in CLEAR_BG so it's
        //   pure invisible padding).
        let multi_tab = self.tabs.len() > 1;
        let active_is_native = matches!(&self.tabs[self.active].mode, Mode::Native { .. });
        let tui_active = active_is_native || self.altscreen_active;
        let target_strip = if multi_tab {
            MACOS_TAB_STRIP_PX_MULTI
        } else if tui_active {
            MACOS_TAB_STRIP_PX_SINGLE
        } else {
            MACOS_TAB_STRIP_PX_SHELL
        };
        let strip_resize = gpu.set_strip_h(target_strip);
        match &mut self.tabs[self.active].mode {
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
                if let Some((cols, rows)) = strip_resize {
                    s.resize(rows, cols);
                }
                if s.dirty() {
                    let (cc, cr, vis) = s.apply_to_grid(&mut gpu.grid);
                    gpu.last_cursor = None; // Shell mode tracks cursor via apply_to_grid
                    if vis && (cc as u32) < gpu.grid.cols && (cr as u32) < gpu.grid.rows {
                        let i = (cr as u32 * gpu.grid.cols + cc as u32) as usize;
                        // Suppress the cursor when it sits at (0, 0) on a
                        // default-empty cell — that's vt100's "shell just
                        // spawned, no output yet" state. Painting it would
                        // flash a white block at the top-left before the
                        // shell prompt appears. Real cursors sit on
                        // rendered content.
                        let suppress = cc == 0 && cr == 0 && {
                            let cell = &gpu.grid.cells[i];
                            cell.ch == ' ' && cell.attrs == 0
                        };
                        if !suppress {
                            gpu.grid.cells[i].attrs |= ATTR_CURSOR_BLOCK;
                            gpu.last_cursor = Some(i);
                        }
                    }
                }
            }
            Mode::Native {
                server,
                conn,
                launcher,
                client_title,
            } => {
                if let Some((cols, rows)) = strip_resize {
                    server.send_resize(cols, rows);
                }
                // Drain server events.
                while let Ok(ev) = server.events.try_recv() {
                    match ev {
                        ServerEvent::ClientConnected => {
                            *conn = ConnState::Connected;
                            *client_title = None; // fresh connection, fresh title
                            server.send_resize(gpu.grid.cols as u16, gpu.grid.rows as u16);
                            paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                        }
                        ServerEvent::ClientDisconnected => {
                            *conn = ConnState::Waiting;
                            *client_title = None;
                            paint_idle(&mut gpu.grid, *conn, &server.socket_path);
                        }
                        ServerEvent::Title(s) => {
                            *client_title = Some(s);
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
        let native = matches!(&self.tabs[self.active].mode, Mode::Native { .. });
        let tui_active = native || self.altscreen_active;
        let new_inset = cfg.active_inset(tui_active);
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let Some((cols, rows)) = gpu.set_inset_px(new_inset) else {
            return;
        };
        match &mut self.tabs[self.active].mode {
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
        match &mut self.tabs[self.active].mode {
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

    // Capture launch-time defaults for spawning additional Native tabs
    // via ⌘T later. `None` ⇒ shell mode (⌘T opens a shell instead).
    let editor_template: Option<EditorTabTemplate> = if editor_mode {
        let workspace = launcher::resolve_workspace(workspace_arg.as_deref());
        let command = launcher::resolve_launch_command();
        let extra_args = launcher::default_extra_args();
        Some(EditorTabTemplate {
            command,
            workspace,
            extra_args,
        })
    } else {
        None
    };
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
        } else if let Some(tmpl) = editor_template.as_ref() {
            let cfg = LauncherConfig {
                command: tmpl.command.clone(),
                workspace: tmpl.workspace.clone(),
                socket: socket_path.clone(),
                extra_args: tmpl.extra_args.clone(),
            };
            let mut l = Launcher::new(cfg);
            match l.spawn() {
                Ok(()) => {
                    eprintln!(
                        "tmnl: spawned {} for workspace {}",
                        tmpl.command.display(),
                        tmpl.workspace.display()
                    );
                    Some(l)
                }
                Err(e) => {
                    eprintln!(
                        "tmnl: failed to launch {} ({e}); start mnml manually with --blit {}",
                        tmpl.command.display(),
                        socket_path.display()
                    );
                    None
                }
            }
        } else {
            None
        };
        Mode::Native {
            server,
            conn: ConnState::Waiting,
            launcher,
            client_title: None,
        }
    } else {
        eprintln!("tmnl: shell mode (run with --editor to launch mnml instead)");
        Mode::Shell { session: None }
    };

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    // Start with one tab holding the initial Mode. Multi-tab keybinds
    // append to this Vec in a follow-up commit.
    let initial_tab = Tab {
        mode,
        label: String::new(),
        attention: false,
        last_status: None,
        grid_snapshot: None,
        last_cursor_snapshot: None,
    };
    let mut app = App {
        window: None,
        gpu: None,
        mods: ModifiersState::empty(),
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
        editor_template,
        native_tab_nonce: 1,
        dragging_tab: None,
    };
    event_loop.run_app(&mut app).unwrap();
    drop(app);
}
