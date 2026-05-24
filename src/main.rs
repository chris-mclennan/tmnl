mod app;
mod atlas;
mod config;
mod fim;
mod grid;
mod headless;
mod launcher;
mod layout;
mod menu;
mod osc133;
mod pipeline;
mod recents;
mod server;
mod settings_ui;
mod shell;
mod shell_prompt;
mod transfer;
mod welcome;

use tmnl_protocol as protocol;

use std::path::PathBuf;
use std::sync::Arc;
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::Window;

use atlas::Atlas;
use config::Config;
use grid::Grid;
use launcher::{Launcher, LauncherConfig, LauncherPoll};
use layout::{Layout, PaneId, Rect, SplitDir};
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
/// How far a non-focused split pane's text is faded toward its own
/// background — the focus cue. Per-pane, so it sidesteps the
/// shared-divider-cell problem the old divider tint had.
const INACTIVE_DIM: f32 = 0.4;
const ATTR_CURSOR_BLOCK: u32 = 1 << 16;
const ATTR_CURSOR_UNDERLINE: u32 = 1 << 17;
const ATTR_CURSOR_BAR: u32 = 1 << 18;

#[derive(Clone, Copy, PartialEq)]
enum ConnState {
    Waiting,
    Connected,
    Streaming,
}

/// A four-way direction for `⌘⌥`-arrow split-pane focus movement.
#[derive(Clone, Copy)]
enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

/// The leaf payload of a pane — what `Tab.mode` used to hold before
/// the splits refactor. Renamed from `Mode`; otherwise unchanged.
enum PaneKind {
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

/// One pane — a leaf in a tab's split layout. Each pane owns its
/// `Grid` permanently. Pre-splits, a single shared grid lived on
/// `Gpu` and was swapped per-tab through a `grid_snapshot`; owning
/// the grid means background panes keep their state for free and a
/// switch back to them is instant. Phase 1 of the splits work
/// (`docs/splits-plan.md`): a tab always has exactly one pane, so
/// this is behaviorally identical to the pre-splits single-`Mode`
/// tab.
struct Pane {
    /// Hosted process / connection state for this pane.
    kind: PaneKind,
    /// This pane's cell grid — the source of truth the compositor
    /// blits into the window grid each frame.
    grid: grid::Grid,
    /// Index of the cell currently carrying a cursor overlay bit, so
    /// the next frame can clear it before drawing the new one.
    last_cursor: Option<usize>,
    /// Cached strip label — refreshed each tick from `kind` (and the
    /// shell's OSC title / spinner where applicable).
    label: String,
    /// Set true when the hosted process emits an OSC 1337 attention
    /// signal (Claude Code does this when a turn finishes and it's
    /// waiting for user input). Cleared when the user focuses the
    /// pane's tab. Rendered as a `●` prefix in the chip.
    attention: bool,
    /// Sticky cache of the most recent detected spinner line and
    /// when we last saw it. Keeps the chip label stable for a short
    /// window after the spinner glyph cycles off-screen (Claude
    /// typically pauses for a few hundred ms between "Wandering…"
    /// and "Pondering…" — without stickiness the chip flips back
    /// to the static OSC title and flickers).
    last_status: Option<(String, std::time::Instant)>,
}

/// One open tab in the tmnl window. A tab is a split tree of panes
/// (`layout`) over a flat `panes` Vec the tree indexes into, plus the
/// `focused` pane that receives keyboard input. Phase 1 keeps every
/// tab at exactly one pane (`Layout::Leaf(0)`); the split / focus /
/// close verbs that grow the tree land in later phases
/// (`docs/splits-plan.md`).
struct Tab {
    /// Binary split tree; leaves index into `panes`.
    layout: Layout,
    /// The tab's panes — leaves of `layout`. Always non-empty.
    panes: Vec<Pane>,
    /// The pane that receives keyboard input + draws a cursor.
    focused: PaneId,
    /// Cached strip label. `App::tick` rewrites this each frame from
    /// `custom_name` if set, otherwise the focused pane's label.
    label: String,
    /// User-set tab name (right-click a chip → rename). `Some`
    /// overrides the auto-derived focused-pane label; `None` (or an
    /// empty rename) reverts to it. Session-only — not persisted.
    custom_name: Option<String>,
}

impl Tab {
    fn focused_pane(&self) -> &Pane {
        &self.panes[self.focused]
    }
    fn focused_pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.focused]
    }
}

/// An in-progress tab rename. The strip chip for `tab_idx` becomes an
/// inline text field showing `buf`; Esc cancels, Enter commits, and a
/// click anywhere commits. Entered by right-clicking a tab chip.
struct RenameState {
    tab_idx: usize,
    buf: String,
}

/// An AI suggestion shown as dim ghost text. Created from a worker
/// reply; cleared on accept or dismiss.
struct Ghost {
    /// The suggested text, rendered dim.
    text: String,
    /// Backspaces to send before `text` on accept. `0` appends at the
    /// cursor (Stage 1 continuation); `>0` replaces the typed
    /// description (Stage 2 NL→command).
    erase: usize,
    /// `true` for a Stage 2 NL→command preview — rendered on the row
    /// below the prompt instead of inline at the cursor.
    below: bool,
}

/// An in-flight completion request. The reply carrying `id` becomes a
/// [`Ghost`] with `erase` / `below` copied across.
struct PendingReq {
    id: u64,
    erase: usize,
    below: bool,
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
    /// Welcome overlay — `Some` while the startup welcome is up.
    /// Cleared when the user picks a recent (transitions to opening
    /// it as a native tab) or dismisses with Esc.
    welcome: Option<welcome::WelcomeState>,
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
    /// In-progress tab rename, if any — see [`RenameState`].
    renaming_tab: Option<RenameState>,
    /// Index (into the active tab's `divider_lines`) of the divider
    /// currently being dragged to resize a split. Set on a left-press
    /// on a divider, cleared on left-release.
    dragging_divider: Option<usize>,
    /// Local AI command-completion worker (`fim-engine`). Spawned
    /// lazily on the first ⌘I trigger so the model only loads if the
    /// feature is used.
    fim: Option<fim::FimWorker>,
    /// The in-flight completion request; a reply with any other id is
    /// stale (the user typed since) and is dropped.
    fim_pending: Option<PendingReq>,
    /// Monotonic id source for completion requests.
    fim_next_id: u64,
    /// The active AI ghost suggestion — rendered dim, written to the
    /// pty on accept (Tab).
    ghost: Option<Ghost>,
    /// Set when `ghost` changes; forces one shell-grid repaint so the
    /// suggestion appears (or, when cleared, disappears).
    fim_redraw: bool,
    /// Pty-fd handoff receiver — dedicated SCM_RIGHTS listener (task
    /// #50). `None` if the listener failed to start (rare; only when
    /// the socket path is unbindable). Children inherit the socket
    /// path via the `TMNL_TRANSFER_SOCKET` env var injected in
    /// `Launcher::spawn`. Drained on each tick into a fresh adopted
    /// shell tab.
    transfer_listener: Option<transfer::TransferListener>,
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
    /// The window-sized composite grid the GPU pipeline renders.
    /// `composite()` rebuilds it each frame by blitting the active
    /// tab's pane grids into it; the panes hold the source of truth.
    grid: Grid,
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
            return Some((cols as u16, rows as u16));
        }
        None
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

/// Apply a `Frame` (diff runs + cursor metadata) to a `Grid` +
/// `last_cursor` slot. A free function (not a `Gpu` method) so any
/// pane's grid — foreground or background — can be updated through
/// the same path; background Native panes drain frames here every
/// tick so a switch back to them shows current state immediately.
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

/// Overlay `text` as dim "ghost" cells starting at grid cell `start` —
/// the AI suggestion. Existing cell `attrs` are preserved, so the cursor
/// still shows through on an inline suggestion. Stops at the grid edge.
fn draw_ghost(grid: &mut grid::Grid, start: usize, text: &str) {
    let total = grid.cells.len();
    for (offset, ch) in text.chars().enumerate() {
        let i = start + offset;
        if i >= total {
            break;
        }
        grid.cells[i] = grid::Cell {
            ch,
            fg: DIM_FG,
            bg: grid.cells[i].bg,
            attrs: grid.cells[i].attrs,
        };
    }
}

/// Composite a tab's panes into the window grid the GPU renders.
/// Splits the window `Rect` per `tab.layout` into one sub-rect per
/// leaf, blits each pane's grid into its rect, then paints the divider
/// lines between splits. Phase 1 had a single leaf; Phase 2 makes it N
/// leaves + dividers — both ride the same `leaf_rects` /
/// `divider_lines` recursion.
fn composite(tab: &Tab, window: &mut grid::Grid) {
    // Uncovered cells (a pane grid briefly smaller than its rect
    // mid-resize) read as background — clear first, then paint over.
    window.clear();
    let area = Rect::new(0, 0, window.cols, window.rows);
    for (pane_id, rect) in tab.layout.leaf_rects(area) {
        if let Some(pane) = tab.panes.get(pane_id) {
            blit_pane(&pane.grid, rect, window, pane_id == tab.focused);
        }
    }
    paint_dividers(window, &tab.layout.divider_lines(area));
}

/// Fade `fg` toward `bg` by [`INACTIVE_DIM`] — a non-focused split
/// pane's text, lower-contrast so the focused pane reads as active.
fn dim_fg(fg: [f32; 4], bg: [f32; 4]) -> [f32; 4] {
    [
        fg[0] + (bg[0] - fg[0]) * INACTIVE_DIM,
        fg[1] + (bg[1] - fg[1]) * INACTIVE_DIM,
        fg[2] + (bg[2] - fg[2]) * INACTIVE_DIM,
        fg[3],
    ]
}

/// Blit `src`'s cells into `window` at `rect`'s top-left, clipped to
/// `rect`, to `src`'s own extent, and to the window's bounds. Only the
/// focused pane draws a cursor + full-bright text; every other pane
/// has its cursor overlay bits stripped and its text dimmed as the
/// cells are copied — the focus cue.
fn blit_pane(src: &grid::Grid, rect: Rect, window: &mut grid::Grid, focused: bool) {
    let cols = rect.w.min(src.cols).min(window.cols.saturating_sub(rect.x)) as usize;
    let rows = rect.h.min(src.rows).min(window.rows.saturating_sub(rect.y));
    for r in 0..rows {
        let s = (r * src.cols) as usize;
        let d = ((rect.y + r) * window.cols + rect.x) as usize;
        let dst = &mut window.cells[d..d + cols];
        let src_row = &src.cells[s..s + cols];
        if focused {
            dst.copy_from_slice(src_row);
        } else {
            for (dc, sc) in dst.iter_mut().zip(src_row) {
                let mut cell = *sc;
                cell.attrs &= !(ATTR_CURSOR_BLOCK | ATTR_CURSOR_UNDERLINE | ATTR_CURSOR_BAR);
                cell.fg = dim_fg(cell.fg, cell.bg);
                *dc = cell;
            }
        }
    }
}

/// The box-drawing glyph for a divider cell with the given edge
/// connectivity. A plain run is `│` / `─`; where dividers meet, the
/// matching junction glyph (`├ ┤ ┬ ┴ ┼` / corners) makes the strokes
/// physically join instead of leaving a half-cell gap.
fn box_glyph(up: bool, down: bool, left: bool, right: bool) -> char {
    match (up, down, left, right) {
        (true, true, true, true) => '┼',
        (true, true, false, true) => '├',
        (true, true, true, false) => '┤',
        (false, true, true, true) => '┬',
        (true, false, true, true) => '┴',
        (false, true, false, true) => '┌',
        (false, true, true, false) => '┐',
        (true, false, false, true) => '└',
        (true, false, true, false) => '┘',
        (false, false, true, true) | (false, false, true, false) | (false, false, false, true) => {
            '─'
        }
        // up/down only, a single up or down, or an isolated cell.
        _ => '│',
    }
}

/// Paint every divider cell, choosing the box-drawing glyph that
/// matches its connectivity so dividers join cleanly at T-junctions
/// and crosses. Dividers render in one uniform dim colour — quiet
/// chrome. (There's deliberately no focus tint: a divider cell at a
/// junction is shared between a focused-pane edge and a non-focused
/// one, so no single colour reads right. Focus is shown by the
/// cursor — only the focused pane draws one.)
fn paint_dividers(window: &mut grid::Grid, lines: &[(Rect, SplitDir)]) {
    let (cols, rows) = (window.cols, window.rows);
    if cols == 0 || rows == 0 {
        return;
    }
    // Mark every divider cell so connectivity can be tested per-cell.
    let mut is_div = vec![false; (cols * rows) as usize];
    for (line, _) in lines {
        for dy in 0..line.h {
            for dx in 0..line.w {
                let (x, y) = (line.x + dx, line.y + dy);
                if x < cols && y < rows {
                    is_div[(y * cols + x) as usize] = true;
                }
            }
        }
    }
    let div_at = |x: u32, y: u32| x < cols && y < rows && is_div[(y * cols + x) as usize];
    for y in 0..rows {
        for x in 0..cols {
            if !div_at(x, y) {
                continue;
            }
            let glyph = box_glyph(
                y > 0 && div_at(x, y - 1),
                div_at(x, y + 1),
                x > 0 && div_at(x - 1, y),
                div_at(x + 1, y),
            );
            let i = (y * cols + x) as usize;
            window.cells[i] = grid::Cell {
                ch: glyph,
                fg: DIM_FG,
                bg: window.cells[i].bg,
                attrs: 0,
            };
        }
    }
}

/// The pane nearest `focused` in direction `dir`, by leaf-rect
/// centers — only panes that genuinely lie that way qualify. Pure
/// geometry, so `App::focus_dir` is a thin wrapper that just feeds it
/// the current layout's rects.
fn nearest_in_dir(rects: &[(PaneId, Rect)], focused: PaneId, dir: FocusDir) -> Option<PaneId> {
    let fr = rects
        .iter()
        .find(|(id, _)| *id == focused)
        .map(|(_, r)| *r)?;
    let (fcx, fcy) = (fr.x + fr.w / 2, fr.y + fr.h / 2);
    let mut best: Option<(PaneId, u32)> = None;
    for &(id, r) in rects {
        if id == focused {
            continue;
        }
        let qualifies = match dir {
            FocusDir::Left => r.x + r.w <= fr.x,
            FocusDir::Right => r.x >= fr.x + fr.w,
            FocusDir::Up => r.y + r.h <= fr.y,
            FocusDir::Down => r.y >= fr.y + fr.h,
        };
        if !qualifies {
            continue;
        }
        let (rcx, rcy) = (r.x + r.w / 2, r.y + r.h / 2);
        let dist = fcx.abs_diff(rcx) + fcy.abs_diff(rcy);
        if best.is_none_or(|(_, d)| dist < d) {
            best = Some((id, dist));
        }
    }
    best.map(|(id, _)| id)
}

/// Resolve a tab-rename buffer to a tab's `custom_name`: a non-blank
/// buffer becomes `Some(trimmed)`; a blank one clears it to `None`,
/// reverting the tab to its auto-derived label. Pure — unit-tested.
fn committed_tab_name(buf: &str) -> Option<String> {
    let name = buf.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Resolve a pane's strip label — the stable name (OSC title /
/// foreground process / shell), with Claude Code's spinner glyph
/// appended when a session is thinking.
fn compute_pane_label(pane: &mut Pane) -> String {
    match &mut pane.kind {
        PaneKind::Shell { session } => {
            // Detect Claude Code's `✽ Wandering…` spinner — just its
            // glyph (which cycles each frame) is appended to the name
            // below, so a thinking tab stays identifiable. Cached
            // sticky for `STATUS_STICKY_MS` so brief gaps between
            // spinner redraws don't blink the glyph off.
            const STATUS_STICKY_MS: u128 = 2000;
            let now = std::time::Instant::now();
            let live = session.as_ref().and_then(|s| s.detect_status_line());
            if let Some(s) = live {
                pane.last_status = Some((s, now));
            }
            let sticky = pane
                .last_status
                .as_ref()
                .filter(|(_, when)| now.duration_since(*when).as_millis() < STATUS_STICKY_MS)
                .map(|(t, _)| t.clone());
            let osc = session.as_ref().and_then(|s| {
                let t = s.osc_title();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            });
            let fg = session
                .as_mut()
                .and_then(|s| s.fg_proc_name().map(|n| n.to_string()));
            // The name is the stable identity: OSC title → foreground
            // process → shell name. Claude's spinner is layered on as
            // *just its glyph* (`name ✽`) — the name stays put so a
            // thinking tab is still tellable apart from its siblings;
            // the status word ("Wandering…") would crowd it out.
            let name = osc.or(fg).unwrap_or_else(|| {
                session
                    .as_ref()
                    .map(|s| s.shell_name().to_string())
                    .unwrap_or_else(|| "shell".to_string())
            });
            match sticky.as_deref().and_then(|s| s.chars().next()) {
                Some(glyph) => format!("{name} {glyph}"),
                None => name,
            }
        }
        PaneKind::Native {
            conn, client_title, ..
        } => match conn {
            ConnState::Waiting => "(no client)".to_string(),
            ConnState::Connected => "(connecting…)".to_string(),
            // Client-supplied title takes priority; falls back to
            // "mnml" pre-handshake.
            ConnState::Streaming => client_title.clone().unwrap_or_else(|| "mnml".to_string()),
        },
    }
}

/// Tick a pane that isn't the active tab's focused pane. A Native pane
/// always drains its server events + frames so its grid tracks live
/// state — essential for a Native split pane, which would otherwise
/// freeze on the idle banner. A shell pane only refreshes its grid
/// when `visible` (a split in the active tab); an off-screen shell is
/// left to refresh on focus (its pty reader thread keeps vt100
/// current meanwhile). Never handles launcher restart/exit — that's
/// the focused-pane path.
fn tick_secondary_pane(pane: &mut Pane, visible: bool) {
    let Pane {
        kind,
        grid,
        last_cursor,
        ..
    } = pane;
    match kind {
        PaneKind::Native {
            server,
            conn,
            client_title,
            launcher,
        } => {
            while let Ok(ev) = server.events.try_recv() {
                match ev {
                    ServerEvent::ClientConnected => {
                        *conn = ConnState::Connected;
                        *client_title = None;
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
                    // A non-focused pane can't be interacted with to
                    // trigger an OpenPane — drop it.
                    ServerEvent::OpenPane { .. } => {}
                }
            }
            while let Ok(f) = server.frame_rx.try_recv() {
                if matches!(conn, ConnState::Connected) {
                    *conn = ConnState::Streaming;
                }
                apply_frame_to_grid(grid, last_cursor, &f);
            }
            if let Some(l) = launcher.as_mut() {
                // Lightweight poll — no respawn logic for non-focused
                // panes (that's the focused-pane path).
                let _ = l.poll();
            }
        }
        PaneKind::Shell { session } => {
            if visible
                && let Some(s) = session
                && s.dirty()
            {
                s.apply_to_grid(grid);
                *last_cursor = None; // only the focused pane draws a cursor
            }
        }
    }
}

fn main() {
    env_logger::init();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Headless mode — no window, scripted stdin, text grid dumps (see
    // `src/headless.rs`). Branches out before any winit / wgpu / AppKit
    // setup so it runs fine with no display.
    if argv.iter().any(|a| a == "--headless") {
        headless::run();
        return;
    }

    // Start the pty-fd transfer listener BEFORE anything that spawns
    // threads or child processes — children inherit the env, so
    // `TMNL_TRANSFER_SOCKET` must be set first or `:tmnl.pop-pty`
    // from inside mnml toasts "not running under tmnl" even when it
    // *is*. Audit caught this regression.
    //
    // SAFETY: this point in main() runs before any thread spawn
    // (env_logger doesn't spawn one) and before any `Command::spawn`,
    // so `std::env::set_var` is single-threaded here.
    let transfer_listener = match transfer::TransferListener::start(transfer::default_socket_path())
    {
        Ok(l) => {
            unsafe { std::env::set_var("TMNL_TRANSFER_SOCKET", &l.socket_path) };
            Some(l)
        }
        Err(e) => {
            eprintln!("tmnl: pty-fd transfer listener disabled: {e}");
            None
        }
    };
    // `--mnml` / `--mixr` both launch in native/integrated mode (UDS
    // blit channel, wgpu renders, the spawned app drives input). The
    // chosen app populates `editor_template` so ⌘T spawns more tabs
    // of the same flavor.
    let which_app = if argv.iter().any(|a| a == "--mixr") {
        Some(launcher::LaunchApp::Mixr)
    } else if argv.iter().any(|a| a == "--mnml") {
        Some(launcher::LaunchApp::Mnml)
    } else {
        None
    };
    let editor_mode = which_app.is_some();
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
            "--mnml" | "--mixr" => {}
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
    let editor_template: Option<EditorTabTemplate> = if let Some(app) = which_app {
        let workspace = launcher::resolve_workspace(workspace_arg.as_deref());
        let command = launcher::resolve_launch_command_for(app);
        let extra_args = launcher::default_extra_args_for(app);
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
        PaneKind::Native {
            server,
            conn: ConnState::Waiting,
            launcher,
            client_title: None,
        }
    } else {
        eprintln!("tmnl: shell mode (run with --editor to launch mnml instead)");
        PaneKind::Shell { session: None }
    };

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    // Start with one tab holding a single pane. The pane's grid is a
    // placeholder here — `resumed` resizes it once the GPU exists and
    // the real window dimensions are known.
    let initial_pane = Pane {
        kind: mode,
        grid: Grid::new(80, 24, CLEAR_BG),
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
        welcome: None,
        editor_template,
        native_tab_nonce: 1,
        dragging_tab: None,
        renaming_tab: None,
        dragging_divider: None,
        fim: None,
        fim_pending: None,
        fim_next_id: 0,
        ghost: None,
        fim_redraw: false,
        // Listener started above, before Server::start + Launcher::spawn,
        // so children inherit the env var. Moved during the audit-pass
        // bugfix; do not relocate back into this initializer.
        transfer_listener,
    };
    // Show the welcome overlay on a "bare" tmnl launch (no --mnml, not
    // headless) when the user has a recents file with entries — so they
    // can re-open their familiar TUI with a single keypress instead of
    // having to type the path. Skipped in editor mode (the user already
    // told us what to open) + when recents is empty (nothing to offer).
    if !editor_mode {
        // Welcome list: user's recents on top, then the always-present
        // built-in launchers (mnml / mixr) so a fresh tmnl install
        // still has a one-keypress path to native-app tabs.
        let mut list = recents::load();
        for built in recents::builtin_entries() {
            // De-dup: if the user has already launched a built-in,
            // their (more-specific) recents entry wins.
            let already = list
                .iter()
                .any(|e| e.command == built.command && e.args == built.args);
            if !already {
                list.push(built);
            }
        }
        if !list.is_empty() {
            app.welcome = Some(welcome::WelcomeState::open(list));
        }
    }
    event_loop.run_app(&mut app).unwrap();
    drop(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_tab_name_trims_and_clears_on_blank() {
        // A real name is trimmed and kept.
        assert_eq!(
            committed_tab_name("my session"),
            Some("my session".to_string())
        );
        assert_eq!(committed_tab_name("  spaced  "), Some("spaced".to_string()));
        // A blank / whitespace buffer clears the custom name, reverting
        // the tab to its auto-derived label.
        assert_eq!(committed_tab_name(""), None);
        assert_eq!(committed_tab_name("   "), None);
    }

    /// A pane whose grid is filled with `ch` — no real session, a
    /// pure-data fixture for compositor tests.
    fn filled_pane(cols: u32, rows: u32, ch: char) -> Pane {
        let mut grid = Grid::new(cols, rows, CLEAR_BG);
        for cell in &mut grid.cells {
            cell.ch = ch;
        }
        Pane {
            kind: PaneKind::Shell { session: None },
            grid,
            last_cursor: None,
            label: String::new(),
            attention: false,
            last_status: None,
        }
    }

    fn two_pane_tab(focused: PaneId) -> Tab {
        Tab {
            layout: Layout::Split {
                dir: SplitDir::Vertical,
                ratio: 0.5,
                first: Box::new(Layout::Leaf(0)),
                second: Box::new(Layout::Leaf(1)),
            },
            panes: vec![filled_pane(10, 4, 'A'), filled_pane(10, 4, 'B')],
            focused,
            label: String::new(),
            custom_name: None,
        }
    }

    #[test]
    fn composite_single_leaf_fills_the_window() {
        let tab = Tab {
            layout: Layout::Leaf(0),
            panes: vec![filled_pane(10, 4, 'A')],
            focused: 0,
            label: String::new(),
            custom_name: None,
        };
        let mut window = Grid::new(10, 4, CLEAR_BG);
        composite(&tab, &mut window);
        assert!(window.cells.iter().all(|c| c.ch == 'A'));
    }

    #[test]
    fn composite_tiles_two_panes_with_a_divider() {
        // 21 wide ⇒ 20 usable, 10/10 either side of a 1-col divider.
        let mut window = Grid::new(21, 4, CLEAR_BG);
        composite(&two_pane_tab(0), &mut window);
        let at = |x: u32, y: u32| window.cells[(y * 21 + x) as usize].ch;
        assert_eq!(at(0, 0), 'A');
        assert_eq!(at(9, 0), 'A');
        assert_eq!(at(10, 0), '│'); // divider column
        assert_eq!(at(11, 0), 'B');
        assert_eq!(at(20, 0), 'B');
    }

    #[test]
    fn composite_dims_unfocused_panes() {
        // Focus the left pane; the right (unfocused) pane's text fades.
        let mut window = Grid::new(21, 4, CLEAR_BG);
        composite(&two_pane_tab(0), &mut window);
        // Focused pane 0 keeps full-bright text.
        assert_eq!(window.cells[0].fg, [1.0; 4]);
        // Unfocused pane 1 (right half, from col 11) is dimmed.
        assert_ne!(window.cells[11].fg, [1.0; 4]);
    }

    #[test]
    fn composite_dividers_join_at_junctions() {
        // A T-layout: pane 0 fills the left; the right column splits
        // into pane 1 (top) and pane 2 (bottom). The cell where the
        // vertical and horizontal dividers meet draws a `├` so the
        // strokes join; dividers are one uniform dim colour.
        let tab = Tab {
            layout: Layout::Split {
                dir: SplitDir::Vertical,
                ratio: 0.5,
                first: Box::new(Layout::Leaf(0)),
                second: Box::new(Layout::Split {
                    dir: SplitDir::Horizontal,
                    ratio: 0.5,
                    first: Box::new(Layout::Leaf(1)),
                    second: Box::new(Layout::Leaf(2)),
                }),
            },
            panes: vec![
                filled_pane(21, 9, 'A'),
                filled_pane(21, 9, 'B'),
                filled_pane(21, 9, 'C'),
            ],
            focused: 2,
            label: String::new(),
            custom_name: None,
        };
        let mut window = Grid::new(21, 9, CLEAR_BG);
        composite(&tab, &mut window);
        // V divider at column 10, H divider at row 4 — junction (10, 4).
        let junction = &window.cells[4 * 21 + 10];
        assert_eq!(junction.ch, '├', "junction draws a T-glyph");
        // Every divider cell is the same quiet chrome colour.
        assert_eq!(junction.fg, DIM_FG);
        assert_eq!(window.cells[10].fg, DIM_FG);
    }

    #[test]
    fn nearest_in_dir_picks_the_adjacent_pane() {
        // 0 | 1 — side by side.
        let rects = vec![(0, Rect::new(0, 0, 10, 8)), (1, Rect::new(11, 0, 10, 8))];
        assert_eq!(nearest_in_dir(&rects, 0, FocusDir::Right), Some(1));
        assert_eq!(nearest_in_dir(&rects, 1, FocusDir::Left), Some(0));
        // Nothing to the left of 0 or above either pane.
        assert_eq!(nearest_in_dir(&rects, 0, FocusDir::Left), None);
        assert_eq!(nearest_in_dir(&rects, 0, FocusDir::Up), None);
    }

    #[test]
    fn nearest_in_dir_chooses_the_closest_of_several() {
        // 0 on the left; the right column is split into 1 (tall, top)
        // and 2 (short, bottom).
        let rects = vec![
            (0, Rect::new(0, 0, 10, 8)),
            (1, Rect::new(11, 0, 10, 5)),
            (2, Rect::new(11, 6, 10, 2)),
        ];
        // From 0 (center y≈4), Right → 1 — its center is nearer than 2's.
        assert_eq!(nearest_in_dir(&rects, 0, FocusDir::Right), Some(1));
        // 1 ↕ 2 are stacked.
        assert_eq!(nearest_in_dir(&rects, 1, FocusDir::Down), Some(2));
        assert_eq!(nearest_in_dir(&rects, 2, FocusDir::Up), Some(1));
        assert_eq!(nearest_in_dir(&rects, 1, FocusDir::Left), Some(0));
    }

    #[test]
    fn composite_strips_the_cursor_from_unfocused_panes() {
        let mut tab = two_pane_tab(0);
        // A cursor overlay bit on cell 0 of each pane.
        tab.panes[0].grid.cells[0].attrs |= ATTR_CURSOR_BLOCK;
        tab.panes[1].grid.cells[0].attrs |= ATTR_CURSOR_BLOCK;
        let mut window = Grid::new(21, 4, CLEAR_BG);
        composite(&tab, &mut window);
        // Focused pane 0 keeps its cursor; pane 1's (at window col 11)
        // is stripped.
        assert_ne!(window.cells[0].attrs & ATTR_CURSOR_BLOCK, 0);
        assert_eq!(window.cells[11].attrs & ATTR_CURSOR_BLOCK, 0);
    }

    #[test]
    fn box_glyph_picks_the_right_junction() {
        // Straight runs.
        assert_eq!(box_glyph(true, true, false, false), '│');
        assert_eq!(box_glyph(false, false, true, true), '─');
        // T-junctions.
        assert_eq!(box_glyph(true, true, false, true), '├');
        assert_eq!(box_glyph(true, true, true, false), '┤');
        assert_eq!(box_glyph(false, true, true, true), '┬');
        assert_eq!(box_glyph(true, false, true, true), '┴');
        // Corners.
        assert_eq!(box_glyph(false, true, false, true), '┌');
        assert_eq!(box_glyph(false, true, true, false), '┐');
        assert_eq!(box_glyph(true, false, false, true), '└');
        assert_eq!(box_glyph(true, false, true, false), '┘');
        // A 4-way cross, and the lone-cell fallback.
        assert_eq!(box_glyph(true, true, true, true), '┼');
        assert_eq!(box_glyph(false, false, false, false), '│');
    }

    #[test]
    fn dim_fg_fades_toward_the_background() {
        // 40% of the way from white toward black ⇒ 0.6 grey.
        let dimmed = dim_fg([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
        assert!((dimmed[0] - 0.6).abs() < 1e-6);
        assert_eq!(dimmed[3], 1.0); // alpha untouched
        // fg already equal to bg ⇒ no change.
        assert_eq!(dim_fg([0.5; 4], [0.5; 4]), [0.5; 4]);
    }

    #[test]
    fn first_button_finds_the_lowest_set_bit() {
        assert_eq!(first_button(0), BUTTON_NONE);
        assert_eq!(first_button(1 << BUTTON_LEFT), BUTTON_LEFT);
        assert_eq!(first_button(1 << BUTTON_RIGHT), BUTTON_RIGHT);
        // Left + Right both held — the lowest (Left) wins.
        assert_eq!(
            first_button((1 << BUTTON_LEFT) | (1 << BUTTON_RIGHT)),
            BUTTON_LEFT
        );
    }

    #[test]
    fn arg_f32_parses_a_flag_value() {
        let argv = vec!["--inset".to_string(), "12.5".to_string()];
        assert_eq!(arg_f32(&argv, "--inset"), Some(12.5));
        assert_eq!(arg_f32(&argv, "--missing"), None);
        // Present but unparseable.
        let bad = vec!["--inset".to_string(), "huge".to_string()];
        assert_eq!(arg_f32(&bad, "--inset"), None);
        // Present but nothing follows.
        assert_eq!(arg_f32(&["--inset".to_string()], "--inset"), None);
    }

    #[test]
    fn pack_mods_maps_each_modifier_bit() {
        assert_eq!(pack_mods(ModifiersState::empty()), 0);
        assert_eq!(pack_mods(ModifiersState::SHIFT), MOD_SHIFT);
        assert_eq!(pack_mods(ModifiersState::CONTROL), MOD_CTRL);
        assert_eq!(pack_mods(ModifiersState::ALT), MOD_ALT);
        assert_eq!(pack_mods(ModifiersState::SUPER), MOD_SUPER);
        assert_eq!(
            pack_mods(ModifiersState::SHIFT | ModifiersState::CONTROL),
            MOD_SHIFT | MOD_CTRL
        );
    }

    #[test]
    fn pack_mods_cmd_to_ctrl_folds_super_into_ctrl() {
        // ⌘ alone lands as Ctrl on the wire.
        assert_eq!(pack_mods_cmd_to_ctrl(ModifiersState::SUPER), MOD_CTRL);
        assert_eq!(pack_mods_cmd_to_ctrl(ModifiersState::CONTROL), MOD_CTRL);
        // ⌘ + ⌃ together don't double-count.
        assert_eq!(
            pack_mods_cmd_to_ctrl(ModifiersState::SUPER | ModifiersState::CONTROL),
            MOD_CTRL
        );
        assert_eq!(
            pack_mods_cmd_to_ctrl(ModifiersState::SUPER | ModifiersState::SHIFT),
            MOD_CTRL | MOD_SHIFT
        );
    }

    #[test]
    fn translate_key_maps_named_and_char_keys() {
        use winit::keyboard::{Key, NamedKey};
        let none = ModifiersState::empty();
        assert!(matches!(
            translate_key(&Key::Named(NamedKey::Enter), none),
            Some(KeyCode::Enter)
        ));
        assert!(matches!(
            translate_key(&Key::Named(NamedKey::ArrowLeft), none),
            Some(KeyCode::Left)
        ));
        assert!(matches!(
            translate_key(&Key::Named(NamedKey::F5), none),
            Some(KeyCode::F(5))
        ));
        // Tab → Tab; Shift+Tab → BackTab.
        assert!(matches!(
            translate_key(&Key::Named(NamedKey::Tab), none),
            Some(KeyCode::Tab)
        ));
        assert!(matches!(
            translate_key(&Key::Named(NamedKey::Tab), ModifiersState::SHIFT),
            Some(KeyCode::BackTab)
        ));
        // A character key.
        assert!(matches!(
            translate_key(&Key::Character("k".into()), none),
            Some(KeyCode::Char('k'))
        ));
    }

    #[test]
    fn draw_ghost_writes_dim_cells_and_clips_at_the_end() {
        let mut g = Grid::new(6, 1, CLEAR_BG);
        draw_ghost(&mut g, 2, "hello");
        // "hell" fits at indices 2..=5; the final "o" is past the grid.
        assert_eq!(g.cells[2].ch, 'h');
        assert_eq!(g.cells[5].ch, 'l');
        assert_eq!(g.cells[2].fg, DIM_FG);
        assert!(!g.cells.iter().any(|c| c.ch == 'o'));
    }

    #[test]
    fn apply_frame_to_grid_writes_runs_and_the_cursor() {
        use super::protocol::{DiffRun, Frame, WireCell};
        let mut g = Grid::new(4, 2, CLEAR_BG);
        let mut last_cursor = None;
        // A run of two cells from index 1: 'A', 'B'.
        let frame = Frame {
            seq: 0,
            cols: 4,
            rows: 2,
            cursor_col: 2,
            cursor_row: 1,
            cursor_shape: 0,
            cursor_visible: 1,
            runs: vec![DiffRun {
                start: 1,
                cells: vec![
                    WireCell {
                        ch: 'A' as u32,
                        fg: 0,
                        bg: 0,
                        attrs: 0,
                    },
                    WireCell {
                        ch: 'B' as u32,
                        fg: 0,
                        bg: 0,
                        attrs: 0,
                    },
                ],
            }],
        };
        apply_frame_to_grid(&mut g, &mut last_cursor, &frame);
        assert_eq!(g.cells[1].ch, 'A');
        assert_eq!(g.cells[2].ch, 'B');
        // Cursor at (col 2, row 1) ⇒ index 1*4 + 2 = 6, block bit set.
        assert_eq!(last_cursor, Some(6));
        assert_ne!(g.cells[6].attrs & ATTR_CURSOR_BLOCK, 0);
    }

    #[test]
    fn apply_frame_to_grid_clears_the_previous_cursor() {
        use super::protocol::Frame;
        let mut g = Grid::new(4, 2, CLEAR_BG);
        let mut last_cursor = Some(3);
        g.cells[3].attrs |= ATTR_CURSOR_BLOCK;
        // A frame whose cursor is hidden — the old overlay bit clears.
        let frame = Frame {
            seq: 1,
            cols: 4,
            rows: 2,
            cursor_col: 0,
            cursor_row: 0,
            cursor_shape: 0,
            cursor_visible: 0,
            runs: vec![],
        };
        apply_frame_to_grid(&mut g, &mut last_cursor, &frame);
        assert_eq!(last_cursor, None);
        assert_eq!(g.cells[3].attrs & ATTR_CURSOR_BLOCK, 0);
    }
}
