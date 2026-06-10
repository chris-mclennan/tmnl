mod app;
mod atlas;
mod command;
mod config;
mod family_offer;
mod fim;
mod grid;
mod headless;
mod help;
mod keymap;
mod launcher;
mod layout;
mod menu;
mod notify;
mod osc133;
mod palette;
mod pipeline;
mod recents;
mod server;
mod settings_ui;
mod shell;
mod shell_prompt;
mod theme;
mod transfer;
mod update_check;
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
/// Pixel height of the tab-chip row added below the palette in
/// multi-tab mode. Sized to fit one cell of glyph + comfortable
/// padding above and below — matches mnml's bufferline rhythm
/// (image #9 in the 2026-06-07 thread: mnml-as-host reference
/// versus tmnl native).
///
/// Iterated 28 → 38 → 32 (2026-06-07):
///   * 28 left ~4px on each side; chip bg bled past strip bottom.
///   * 38 fixed the bleed but tmnl read taller than mnml's
///     reference bufferline.
///   * 32 settles in between: ~6px above + below the glyph at
///     cell_h ≈ 20, no bleed, matches mnml's tighter rhythm.
///
/// Multi-tab strip height = `MACOS_TAB_STRIP_PX_SINGLE` (palette
/// zone) + `rows * (TAB_GAP_PX + TAB_ROW_H_PX)`. Each row carries
/// its own gap above it, so the spacing between successive chip
/// rows matches the spacing between the palette and the first row.
/// See `Gpu::required_strip_h`.
const TAB_ROW_H_PX: f32 = 32.0;

/// Vertical strip-bg gap between the palette zone (52px tall, palette
/// text centered inside it) and the top of row 0. Just 3px because
/// the palette zone already has ~16px of empty space below the
/// centered palette text — adding more here would push row 0 too
/// far from the palette label. Was 6px before 2026-05-24.
const TAB_GAP_PX: f32 = 3.0;

/// Vertical strip-bg gap between consecutive chip rows (row 0↔row 1,
/// row 1↔row 2, …). Larger than `TAB_GAP_PX` because the rows have
/// no "centered-inside-zone" empty space the way the palette does;
/// without this, row 1 sits visibly closer to row 0 than row 0 sits
/// to the palette text. 16px ≈ the empty space below the centered
/// palette text inside its 52px zone, so the visible label-to-
/// label distance between rows matches palette→row-0.
const INTER_ROW_GAP_PX: f32 = 16.0;

/// Vertical-mode inter-row gap between sidebar tab chips. Smaller
/// than horizontal's 16 because the sidebar is narrow — too much
/// gap and the chips read as separate floating pills, but ZERO
/// (the old behavior) makes them feel cramped. 12px lands close
/// to horizontal mode's 16 but still reads as a coherent column.
/// 2026-06-09 user feedback: 6 was too tight.
const VERT_INTER_ROW_GAP_PX: f32 = 12.0;

/// Extra space above the `+` new-tab chip in vertical mode — adds
/// to whatever inter-row gap the chips above already have, so the
/// total separation `+` ↔ last-tab is `VERT_INTER_ROW_GAP_PX +
/// VERT_PLUS_EXTRA_GAP_PX`. 16px chosen so the `+` clearly reads
/// as a distinct affordance rather than another tab.
const VERT_PLUS_EXTRA_GAP_PX: f32 = 16.0;

/// Sidebar-header row height in vertical-tab mode. Sits at the
/// top of the sidebar (just below the top strip) and contains a
/// Warp-style search input on the left + a `+` new-tab button on
/// the right. Tab chips render below it. 36 px = `TAB_ROW_H_PX` +
/// a 4 px breathing strip so the header reads as separate chrome
/// from the chip cluster.
const SIDEBAR_HEADER_H_PX: f32 = 36.0;
/// Single-tab chrome height — a small breathing-room band above the
/// grid so the first row of content isn't kissing the macOS traffic
/// lights, but no visible chrome strip (the strip pipeline paints this
/// region in `palette().clear_bg` instead of `palette().strip_bg` when there are no chips,
/// so it blends invisibly with the surrounding clear color).
///
/// Bumped 24 → 34 → 44 → 48 → 52 (2026-05-24) — successive bumps
/// until the corner-of-the-border text cleared the macOS
/// traffic-light buttons at default Retina scaling. When a 2nd tab
/// opens this swaps out for `MACOS_TAB_STRIP_PX_MULTI` and the
/// issue is moot.
#[cfg(target_os = "macos")]
const MACOS_TAB_STRIP_PX_SINGLE: f32 = 52.0;
#[cfg(not(target_os = "macos"))]
const MACOS_TAB_STRIP_PX_SINGLE: f32 = 32.0;
/// Single-tab strip height for *shell* mode (no TUI hosted, e.g. a bare
/// `zsh` prompt). Larger than the TUI value so the prompt's first row
/// doesn't sit right under the macOS traffic lights. The strip pipeline
/// still paints palette().clear_bg so this band is invisible — pure padding.
#[cfg(target_os = "macos")]
const MACOS_TAB_STRIP_PX_SHELL: f32 = 42.0;
#[cfg(not(target_os = "macos"))]
const MACOS_TAB_STRIP_PX_SHELL: f32 = 24.0;
// Chrome palette lives in `theme.rs` — at startup tmnl tries to
// adopt mnml's installed theme so the two apps blend visually when
// launched side-by-side; falls back to defaults eyedropped from
// mnml's onedark rendered in Apple Terminal otherwise. Access via
// `theme::palette()`. See `theme.rs` for the role table.
//
// Re-exported so `use crate::*` in app.rs / headless.rs picks it up.
pub use theme::palette;
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
    /// Web browser pane — hosts a `wry::WebView` overlaid on the wgpu
    /// surface in this pane's rect. The grid keeps the placeholder
    /// underneath but the webview's native surface composites over
    /// it (NSView on macOS, GtkWidget on Linux, HWND on Windows).
    /// See `app.rs::split_active_pane_browser` for the open flow.
    Browser {
        /// Current URL — the source of truth driving the webview's
        /// load target + the chip label.
        url: String,
        /// The wry WebView mounted as a sub-region of the parent
        /// winit window. `None` until the GPU/window is available
        /// (we don't create webviews pre-resume) or while we're
        /// re-mounting after a tab hide/show cycle.
        webview: Option<wry::WebView>,
        /// Top-of-pane chrome strip state — back/forward/reload chips
        /// and URL bar. See [`BrowserChrome`].
        chrome: BrowserChrome,
    },
}

/// Chrome strip on the top row of a Browser pane. Three chips —
/// `[<] [>] [⟳]` — followed by the URL bar. Clicks on a chip fire
/// the matching webview action; a click on the URL bar starts an
/// inline edit that loads on Enter and cancels on Esc.
#[derive(Default)]
pub(crate) struct BrowserChrome {
    /// URL edit buffer when focused. `None` ⇒ the URL bar shows the
    /// pane's `url` field read-only; `Some(s)` ⇒ keys go to the
    /// editor instead of the WebView.
    pub edit: Option<String>,
    /// Cursor position in `edit` (chars, not bytes). Meaningless when
    /// `edit` is `None`.
    pub cursor: usize,
}

/// Hit-test region on a Browser pane's chrome strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrowserChip {
    Back,
    Forward,
    Reload,
    UrlBar,
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

/// In-pane find — `⌘F` opens it, `Esc` dismisses, `Enter` /
/// `Shift+Enter` step through matches. Lives on `App.find` while
/// the user is searching; `None` otherwise.
pub struct FindState {
    /// Query — case-insensitive substring match against the focused
    /// pane's visible grid. Empty query renders the bar with no
    /// highlights / no match count.
    pub query: String,
    /// Inclusive `(row, col_start, col_end)` spans of every match in
    /// the focused pane's visible grid, recomputed each tick.
    pub matches: Vec<(u16, u16, u16)>,
    /// Index into `matches` of the "current" hit (`Enter` advances
    /// it). Clamped to `matches.len() - 1` on every recompute; 0
    /// when no matches.
    pub current: usize,
    /// Tab + pane this find session belongs to. Find auto-dismisses
    /// if the user switches tabs or closes the pane.
    pub tab_idx: usize,
    pub pane_idx: usize,
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
    /// Set by any handler that wants to quit the app (last-tab-
    /// closed, ⌘Q, blit-channel exit, etc.). The winit
    /// `ApplicationHandler` reads this at the end of `window_event`
    /// and calls `event_loop.exit()` if set. Lets headless mode
    /// run those same handlers without needing a real
    /// `ActiveEventLoop`. (Headless event loops just check the
    /// flag + break out of their stdin loop.)
    should_quit: bool,
    /// Resolved key bindings — chord → command id. Built from the
    /// [`crate::command`] registry at startup. See
    /// `docs/COMMAND_MIGRATION.md`.
    keymap: crate::keymap::Keymap,
    /// Help overlay state. `Some` while the overlay is up; `None`
    /// when closed. Toggled by `view.help` (default `cmd+shift+/`).
    help: Option<crate::help::HelpState>,
    /// Command palette overlay — VS Code-style fuzzy picker over
    /// every registered command. Opened by `view.palette`
    /// (default `cmd+shift+p` when no Native pane focused; for
    /// Native panes the same chord forwards to mnml). `None` when
    /// closed. Greedy modal — its keys win ahead of everything else.
    palette: Option<crate::palette::PaletteState>,
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
    /// Last tick's count of tabs with at least one pane needing
    /// attention. Used by the notification pipeline to:
    ///   * skip Dock-badge writes when the count is unchanged,
    ///   * play the opt-in chime only on a rising edge (new
    ///     attention) — not on every tick the chip dot is up.
    prev_attention_count: usize,
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
    /// When `Some(px)`, the user has dragged the vertical-tab
    /// sidebar's right edge to set a custom column width — that
    /// value is used in place of `Gpu::compute_sidebar_w_px` (which
    /// auto-fits to chip text). Cleared back to `None` by toggling
    /// to horizontal layout or via a hypothetical "reset sidebar"
    /// command. Ephemeral (no config persistence yet).
    sidebar_w_override: Option<f32>,
    /// `true` while a left-button drag on the sidebar's right-edge
    /// border is in flight. Updates `sidebar_w_override` to track
    /// the cursor each move event; cleared on left-release.
    dragging_sidebar: bool,
    /// Pixel-x of the original left-press when `dragging_sidebar`
    /// armed. Cursor-moved events compare against this to require
    /// a minimum drag distance before changing the sidebar width —
    /// without it, a plain click in the 4-px grab zone would snap
    /// the border to the cursor x as soon as any sub-pixel jitter
    /// reached the move handler. `None` outside an armed drag.
    sidebar_drag_press_x: Option<f64>,
    /// Snapshot of `sidebar_w_override` taken at press-time of a
    /// sidebar drag. If the user clicks in the grab zone but
    /// releases WITHOUT crossing the drag threshold, we restore
    /// this value — a click-only gesture doesn't persist any
    /// change. `None` outside a press session.
    sidebar_drag_prev_override: Option<Option<f32>>,
    /// In-flight text selection on the focused body grid — `Some`
    /// while a drag is in progress AND for as long as the user keeps
    /// the selection visible (cleared on a click outside, on Esc, or
    /// on Cmd+C copy). `(tab_idx, pane_idx, anchor_col, anchor_row,
    /// focus_col, focus_row)`. anchor is the press-time cell; focus
    /// tracks the cursor.
    text_selection: Option<(usize, usize, u16, u16, u16, u16)>,
    /// Tab-search mode — when `Some`, the search chip displays
    /// `App.tab_search` instead of the active-tab label, keystrokes
    /// route into the query, and tab chips whose labels don't
    /// substring-match get dimmed. Toggled by clicking the search
    /// chip on a Shell pane; Esc / Enter dismisses. Pushed to
    /// `Gpu.tab_search` each frame so the chrome renderer can show
    /// the query.
    tab_search: Option<String>,
    /// In-pane find — opened with ⌘F, dismissed with Esc. Shows an
    /// inline find bar at the top of the body and highlights every
    /// substring match in the focused pane's visible grid. `Enter`
    /// scrolls to the next match; `Shift+Enter` to the previous.
    find: Option<FindState>,
    /// `true` between a body-cell mouse-press and its release —
    /// drives `cursor_moved` selection extension. Cleared on release
    /// (the selection itself stays visible for copy / clear).
    dragging_selection: bool,
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
    /// `None` in headless mode (no window). Render paths early-
    /// return when surface is absent; resize paths skip the
    /// `configure` call. Every other field (device, queue, atlas,
    /// pipelines, grid + chrome state) is fully present so App
    /// logic that doesn't render still works identically.
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Surface-format config — width/height get read by render
    /// math even in headless mode (chip layout, sidebar geometry),
    /// so it stays non-Option. Format / present_mode fields are
    /// dummies in headless.
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
    /// Per-chip click hit-rect: `(x0, x1, y0, y1, chip_idx)`.
    /// Includes Y bounds so wrap-layout chips on different rows are
    /// distinguished — a click at "this column" on row 1 vs row 2
    /// otherwise resolves to the same chip without the Y check.
    strip_chip_rects: Vec<(f32, f32, f32, f32, usize)>,
    /// Pixel rects of the four palette-cluster hit-targets in the
    /// chrome strip — `(x0, x1, y0, y1)`. Order: back, forward,
    /// search-chip, dropdown-chevron. Click on each sends a different
    /// key combo to mnml (Ctrl+PageUp / Ctrl+PageDown / Ctrl+Shift+P /
    /// Ctrl+R) so the native client's existing keybindings fire.
    strip_palette_back_rect: Option<(f32, f32, f32, f32)>,
    /// Leftmost pixel-x of the palette cluster (the back-arrow's
    /// x0). Captured by `strip_palette_chip_instances` each frame
    /// so `strip_sidebar_toggle_instances` (which paints after
    /// it) can anchor the toggle just to the left of the
    /// cluster. `None` when the palette didn't render this tick.
    palette_left_x_px: Option<f32>,
    strip_palette_fwd_rect: Option<(f32, f32, f32, f32)>,
    strip_palette_chip_rect: Option<(f32, f32, f32, f32)>,
    strip_palette_dropdown_rect: Option<(f32, f32, f32, f32)>,
    /// Sidebar-toggle button in the top strip — clicking it cycles
    /// `tab_layout` Horizontal ↔ Vertical. Sits at the right end
    /// of the palette cluster. `None` when the button isn't
    /// currently painted (e.g. headless probe ran but no actual
    /// render happened).
    strip_sidebar_toggle_rect: Option<(f32, f32, f32, f32)>,
    /// Sidebar-header search input — the Warp-style search box at
    /// the top of the vertical-tab sidebar (left side of the
    /// header row). Click → toggles tab-search on Shell panes,
    /// fires Ctrl+Shift+P on Native panes (same as the top-strip
    /// palette chip). `None` when not vertical / no sidebar.
    sidebar_search_rect: Option<(f32, f32, f32, f32)>,
    /// Sidebar-header `+` new-tab button — right side of the
    /// header row in the vertical sidebar. Click → new tab.
    /// `None` when not vertical / no sidebar.
    sidebar_plus_rect: Option<(f32, f32, f32, f32)>,
    /// In-pane find highlight spans — `(row, col_start, col_end)`,
    /// inclusive col_end. Cells inside any span get a yellow bg
    /// override during render. App pushes the focused pane's match
    /// list here each tick; cleared when find closes.
    find_highlight_bounds: Vec<(u16, u16, u16)>,
    /// In-pane find bar — `(query, current_match, total_matches)`.
    /// `Some` while ⌘F is open. Renders as an inline chip at the
    /// top of the body (row 0), painted via the cell pipeline.
    /// App pushes its `FindState` summary here each tick.
    find_bar: Option<(String, usize, usize)>,
    /// "Current" match span — painted with a brighter highlight
    /// than the rest so the user can see which one Enter will
    /// jump to. `None` when no matches.
    find_current_bounds: Option<(u16, u16, u16)>,
    /// Tab-search mode — when `Some`, the search chip in the
    /// palette cluster shows the user's query instead of the
    /// active-tab label, and any tab chip whose label doesn't
    /// match the query gets dimmed. App owns the canonical state
    /// (`App.tab_search`) and pushes a clone here each frame.
    tab_search: Option<String>,
    /// Pixel-x extents `(x0, x1, tab_idx)` of the trailing `⊗` close
    /// badge on each non-active chip. Click → `close_tab_at`. Active
    /// chip has no close badge (the user closes the active tab via
    /// ⌘W or middle-click).
    /// Per-chip close-badge hit-rect: `(x0, x1, y0, y1, chip_idx)`.
    /// Same shape as `strip_chip_rects` — Y bounds make wrap-layout
    /// safe.
    strip_chip_close_rects: Vec<(f32, f32, f32, f32, usize)>,
    /// Pixel rect `(x0, x1, y0, y1)` of the trailing `+` new-tab
    /// button. Painted only when chips are visible. Click →
    /// `new_shell_tab`. Y bounds make it correct on the last chip
    /// row regardless of how many wrap-rows there are.
    strip_new_tab_rect: Option<(f32, f32, f32, f32)>,
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
    /// Chip layout mode — `Horizontal` (chips wrap across rows below
    /// the palette strip) or `Vertical` (chips stack down a left-edge
    /// sidebar). Mirrors the user's `[tab_layout]` config; App
    /// refreshes from config on each tick so a settings change takes
    /// effect within a frame. Drives [`Self::chip_layout`] +
    /// [`Self::required_strip_h`] + [`Self::sidebar_w_px`].
    tab_layout: crate::config::TabLayout,
    /// Pixel width of the left-edge tab sidebar when
    /// `tab_layout = Vertical` and there's >1 chip — otherwise 0.
    /// `App.tick` updates it from the current chip list via
    /// [`Self::compute_sidebar_w_px`]; render paths (`grid_dims`,
    /// `pixel_to_cell`, the grid render offset) add this to
    /// `inset_px` for the x-axis so the body shifts right to make
    /// room.
    sidebar_w_px: f32,
    /// Number of chip-rows scrolled past in the vertical sidebar.
    /// Increments when the user wheels DOWN over the sidebar (more
    /// chips slide UP / off the top). Capped at `chip_count -
    /// visible_chips + 1` by [`Self::clamp_sidebar_scroll`] so the
    /// `+` button stays reachable. 0 in horizontal mode.
    pub sidebar_scroll_rows: f32,
    /// `true` when `launcher_position` is `Top` / `Bottom` — icons
    /// render inline in the top strip instead of a left-edge
    /// vertical rail. Set by App in tick. When true,
    /// `launcher_w_px` is 0 and `launcher_chip_instances` paints
    /// horizontally starting just after the sidebar toggle.
    launcher_in_top_strip: bool,
    /// Pixel width of the left-edge launcher rail. Computed from
    /// the config's `launcher_icons` count: empty ⇒ 0, otherwise a
    /// fixed 3-cell column. Sits at the window's left edge — the
    /// tab sidebar (vertical mode) is offset by this width so the
    /// two chrome regions sit side by side, launcher first. Render
    /// paths add this to `inset_px` for the x-axis along with
    /// `sidebar_w_px`. 2026-06-09.
    launcher_w_px: f32,
    /// Bounds of the active text selection — `((min_col, min_row),
    /// (max_col, max_row))`. App sets this each frame from
    /// `App.text_selection` before calling `render`; cleared back
    /// to `None` when no selection is active. Used to override the
    /// per-cell bg in the body grid so selected cells visually
    /// inverted (selection_bg color).
    selection_bounds: Option<((u16, u16), (u16, u16))>,
    /// When `true`, body-grid rows are shifted down so the cursor
    /// row lands at the bottom of the visible area. App sets this
    /// from `cfg.prompt_position` before calling `render`. Empty
    /// rows fill the top of the body until enough output exists.
    /// Suppressed automatically when an alt-screen TUI is active —
    /// those apps take over the whole grid and would render weirdly
    /// if shifted.
    bottom_prompt: bool,
    /// One entry per launcher rail glyph — `(glyph, fg_color)`.
    /// App pushes the live list into Gpu via `set_launcher_icons`
    /// whenever the config changes. Rendered by
    /// `launcher_chip_instances`. Empty ⇒ no rail (launcher_w_px is
    /// also 0).
    launcher_icons: Vec<(String, [f32; 4])>,
    /// Hit rects for left-clicking a launcher icon. `(x0, x1, y0, y1, idx)`
    /// in window pixels — `idx` is the index into the config's
    /// `launcher_icons` Vec. Populated by `launcher_chip_instances`
    /// each frame; consumed by the mouse-press dispatcher. (Click
    /// handling lands in commit C; this commit only paints.)
    launcher_icon_rects: Vec<(f32, f32, f32, f32, usize)>,
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
            0.0, // no sidebar at startup — single tab, App refreshes later
            0.0, // no launcher rail until App sets launcher_icons
        );
        let g = Grid::new(cols, rows, palette().clear_bg);

        let pipeline = CellPipeline::new(&device, format, &atlas, (cols * rows).max(1024) as u64);
        let strip_pipeline = pipeline::StripPipeline::new(&device, format);

        Self {
            surface: Some(surface),
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
            strip_palette_back_rect: None,
            palette_left_x_px: None,
            strip_palette_fwd_rect: None,
            strip_palette_chip_rect: None,
            strip_sidebar_toggle_rect: None,
            sidebar_search_rect: None,
            sidebar_plus_rect: None,
            find_highlight_bounds: Vec::new(),
            find_bar: None,
            find_current_bounds: None,
            tab_search: None,
            strip_palette_dropdown_rect: None,
            strip_chip_close_rects: Vec::new(),
            strip_new_tab_rect: None,
            // Default to the minimal (single-tab) chrome height; App
            // bumps to the taller multi-tab value once a second tab
            // is added.
            strip_h: MACOS_TAB_STRIP_PX_SINGLE,
            font_zoom: 1.0,
            // App refreshes this from `cfg.tab_layout` on each tick;
            // default to Horizontal so single-tab startup matches the
            // legacy look before App has a chance to write its
            // configured value.
            tab_layout: crate::config::TabLayout::default(),
            sidebar_w_px: 0.0,
            sidebar_scroll_rows: 0.0,
            launcher_w_px: 0.0,
            launcher_in_top_strip: false,
            selection_bounds: None,
            bottom_prompt: false,
            launcher_icons: Vec::new(),
            launcher_icon_rects: Vec::new(),
        }
    }

    /// Construct a window-less Gpu for headless mode. Uses wgpu's
    /// fallback adapter (software rasterizer if no real GPU is
    /// available — usually fine for tests because we don't render).
    /// `Surface` is `None`; `device` / `queue` / `atlas` / `pipelines`
    /// are all real so App logic that inspects cell dimensions, chip
    /// layout, or geometry works identically. Width / height seed
    /// `config` for the initial grid_dims pass.
    async fn new_headless(width: u32, height: u32, inset_px: f32) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        // No surface needed — pass `compatible_surface: None` so wgpu
        // doesn't filter adapters for swapchain compatibility. Try
        // the default adapter first (Metal on macOS, whatever Vulkan
        // exposes on Linux). Fall back to the software adapter only
        // if no real adapter accepts the no-surface request — many
        // systems' "fallback" adapter actually requires a surface
        // (catch-22), so we try real first.
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
        {
            Some(a) => a,
            None => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
                .ok_or_else(|| "no compatible wgpu adapter (real or fallback)".to_string())?,
        };
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("tmnl-headless"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .map_err(|e| format!("headless device request failed: {e}"))?;

        // Pick a sensible non-sRGB format for `config`. We don't
        // create a real surface to query capabilities, so just go
        // with a widely-supported default that matches what the
        // real `Gpu::new` would pick on most platforms.
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        // Atlas + pipelines still get built so chip layout + hit-rect
        // math match what real mode does. Glyph rendering goes into
        // the atlas texture but never reaches a Surface — that's fine.
        let scale = 1.0_f32;
        let atlas = Atlas::new(&device, &queue, FONT_PX * scale)
            .map_err(|e| format!("headless atlas: {e}"))?;
        let (cols, rows) = grid_dims(
            width,
            height,
            &atlas,
            inset_px,
            MACOS_TAB_STRIP_PX_SINGLE,
            0.0,
            0.0, // no launcher rail in headless until set
        );
        let g = Grid::new(cols, rows, palette().clear_bg);
        let pipeline = CellPipeline::new(&device, format, &atlas, (cols * rows).max(1024) as u64);
        let strip_pipeline = pipeline::StripPipeline::new(&device, format);

        Ok(Self {
            surface: None,
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
            strip_palette_back_rect: None,
            palette_left_x_px: None,
            strip_palette_fwd_rect: None,
            strip_palette_chip_rect: None,
            strip_sidebar_toggle_rect: None,
            sidebar_search_rect: None,
            sidebar_plus_rect: None,
            find_highlight_bounds: Vec::new(),
            find_bar: None,
            find_current_bounds: None,
            tab_search: None,
            strip_palette_dropdown_rect: None,
            strip_chip_close_rects: Vec::new(),
            strip_new_tab_rect: None,
            strip_h: MACOS_TAB_STRIP_PX_SINGLE,
            font_zoom: 1.0,
            tab_layout: crate::config::TabLayout::default(),
            sidebar_w_px: 0.0,
            sidebar_scroll_rows: 0.0,
            launcher_w_px: 0.0,
            launcher_in_top_strip: false,
            selection_bounds: None,
            bottom_prompt: false,
            launcher_icons: Vec::new(),
            launcher_icon_rects: Vec::new(),
        })
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
        let (cols, rows) = grid_dims(
            w,
            h,
            &new_atlas,
            self.inset_px,
            self.strip_h,
            self.sidebar_w_px,
            self.launcher_w_px,
        );
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

    /// Total cell width of one chip's pill body. Components:
    /// `pad + (attention ? 2 : 0) + label + gap + close + pad`.
    /// Used by `chip_layout` to decide when to wrap to a new row.
    /// Inter-chip gap is added separately (after each chip except
    /// the last) — see `chip_layout`.
    fn chip_cells(label: &str, active: bool, attention: bool) -> f32 {
        let pad = Self::CHIP_PAD_CELLS * 2.0;
        let attn = if attention && !active { 2.0 } else { 0.0 };
        let label_cells = label.chars().count() as f32;
        let gap_before_close = 1.0;
        let close = 1.0;
        pad + attn + label_cells + gap_before_close + close
    }

    /// Pixel width the left-edge tab sidebar needs to fit every chip
    /// in the given list under `tab_layout = Vertical`. Layout:
    /// `[SIDEBAR_PAD_LEFT_PX (left pad)] [widest chip] [1 cell pad +
    /// strip_bg → clear_bg color transition (the visible border)]`.
    /// Returns 0 when chips ≤ 1 (single-tab mode hides the strip
    /// anyway).
    ///
    /// 2026-06-08: removed an erroneous `CHIP_START_X_PX` addition
    /// here (a horizontal-mode constant that was bleeding into the
    /// vertical sidebar width math). It made the column ~170px
    /// wider than needed, so the body ended up far to the right of
    /// the chips with a big empty gap. With this fix the chips sit
    /// flush against a tight border, and the body starts right
    /// after the column.
    pub fn compute_sidebar_w_px(&self, chips: &[(String, bool, bool)]) -> f32 {
        let widest = chips
            .iter()
            .map(|(label, active, attention)| Self::chip_cells(label, *active, *attention))
            .fold(0.0_f32, f32::max);
        // Plus button is 3 cells — make sure the sidebar accommodates
        // it on the last row so the `+` doesn't overflow.
        let with_plus = widest.max(3.0);
        // 2026-06-09 user feedback (third pass): "reduce sidebar
        // width by ~25%". Scaled formula + floor by 0.75 — was
        // `widest * 3 + 6` / floor 30; now ~2.25/4.5/22. Still
        // wide enough for "Search tabs…" + magnify + plus button.
        let chip_target_cells = (with_plus * 2.25 + 4.5).max(22.0);
        let raw = Self::SIDEBAR_PAD_LEFT_PX + chip_target_cells * self.atlas.cell_w;
        // Apply the same `[min, max]` envelope the drag-override path
        // uses — a long chip label can otherwise push the column past
        // the cap and the body shrinks unhelpfully.
        self.clamp_sidebar_w_px(raw)
    }

    /// Clamp a sidebar width to `[min, max]` — covers both
    /// user-drag overrides and the auto-fit path. `min` is enough
    /// for the `+` button + a tiny chip; `max` is half the window
    /// width so the column can't take over.
    ///
    /// Narrow-window guard: if `viewport / 2 < min`, we hand back
    /// `min` rather than `f32::clamp(min, max)`, which panics when
    /// `min > max`. Headless / startup with a tiny viewport hits
    /// this if not defended.
    pub fn clamp_sidebar_w_px(&self, w: f32) -> f32 {
        let min_w = Self::SIDEBAR_PAD_LEFT_PX + 4.0 * self.atlas.cell_w;
        let max_w = (self.config.width as f32) * 0.5;
        if max_w < min_w {
            return min_w;
        }
        w.clamp(min_w, max_w)
    }

    /// Compute the wrap layout for a given chip list: how many rows
    /// they need and where each chip sits (row + col-offset within
    /// the row). The `+` new-tab button slot is included so it
    /// doesn't get cut off the right edge.
    ///
    /// Returns `(slots, plus_slot, row_count)` where `slots[i]` is
    /// `(row_idx, col_offset_cells)` for chip `i`, `plus_slot` is
    /// the slot the `+` button occupies, and `row_count` is the
    /// total number of rows needed (≥ 1 when chips are present).
    ///
    /// Branches on `self.tab_layout`:
    ///   * `Horizontal` — chips flow L→R, wrap on overflow.
    ///   * `Vertical` — each chip is its own row at col 0; `+`
    ///     button trails on the row after the last chip.
    fn chip_layout(
        &self,
        chips: &[(String, bool, bool)],
    ) -> (Vec<(usize, f32)>, (usize, f32), usize) {
        if matches!(self.tab_layout, crate::config::TabLayout::Vertical) {
            // Vertical sidebar: each chip on its own row at col 0;
            // the `+` button trails immediately after. No
            // overflow-wrap (a v2 follow-up wires scrolling).
            let slots: Vec<(usize, f32)> = (0..chips.len()).map(|i| (i, 0.0)).collect();
            let plus_slot = (chips.len(), 0.0);
            let row_count = chips.len() + 1;
            return (slots, plus_slot, row_count);
        }
        let cell_w = self.atlas.cell_w;
        let window_w = self.config.width as f32;
        let start_x = Self::CHIP_START_X_PX;
        // Right edge safe area: traffic-lights-equivalent + room for
        // the `+` button. The button is 3 cells wide (pad + glyph +
        // pad); add a comfortable margin so chips don't kiss the
        // window edge.
        let right_margin_px = 16.0;
        let max_x_px = window_w - right_margin_px;
        let row_max_cells = ((max_x_px - start_x) / cell_w).max(0.0);
        let plus_cells = 3.0;
        let gap = Self::CHIP_GAP_CELLS;

        let mut slots: Vec<(usize, f32)> = Vec::with_capacity(chips.len());
        let mut row = 0_usize;
        let mut col = 0.0_f32;
        for (label, active, attention) in chips {
            let cells = Self::chip_cells(label, *active, *attention);
            // Wrap if this chip doesn't fit AND we've placed at least
            // one chip on the current row (avoids infinite-wrap loop
            // when one chip alone exceeds the available width — it
            // just gets cut off the right; better than spinning).
            if col > 0.0 && col + cells > row_max_cells {
                row += 1;
                col = 0.0;
            }
            slots.push((row, col));
            col += cells + gap;
        }
        // `+` button — wraps to a new row if it wouldn't fit after the
        // last chip.
        let plus_slot = if col + plus_cells > row_max_cells && col > 0.0 {
            (row + 1, 0.0)
        } else {
            (row, col)
        };
        let row_count = plus_slot.0 + 1;
        (slots, plus_slot, row_count)
    }

    /// Required strip height for the current chip list. Branches on
    /// `tab_layout`:
    ///   * `Horizontal` — palette zone + (rows × `TAB_ROW_H_PX`)
    ///     when chips wrap; single-tab → palette zone only.
    ///   * `Vertical` — palette zone only (tab chips live in the
    ///     left-edge sidebar, not the top strip).
    pub fn required_strip_h(
        layout: crate::config::TabLayout,
        chips: &[(String, bool, bool)],
        rows: usize,
        tui_active: bool,
    ) -> f32 {
        let single_h = if tui_active {
            MACOS_TAB_STRIP_PX_SINGLE
        } else {
            MACOS_TAB_STRIP_PX_SHELL
        };
        if chips.len() <= 1 {
            return single_h;
        }
        match layout {
            crate::config::TabLayout::Vertical => single_h,
            crate::config::TabLayout::Horizontal => {
                // Palette zone + TAB_GAP_PX above row 0 + every row's
                // height + INTER_ROW_GAP_PX above each row past row 0.
                // The two gap constants are different sizes because
                // the palette zone has built-in centering empty space
                // below its text while the chip rows don't — see the
                // const docstrings.
                let rows = rows.max(1) as f32;
                let inter_row_gaps = (rows - 1.0).max(0.0) * INTER_ROW_GAP_PX;
                MACOS_TAB_STRIP_PX_SINGLE + TAB_GAP_PX + rows * TAB_ROW_H_PX + inter_row_gaps
            }
        }
    }

    /// Public surface for `chip_layout` so the App can ask "how many
    /// rows do the current chips need?" before deciding the strip
    /// height.
    pub fn chip_row_count(&self, chips: &[(String, bool, bool)]) -> usize {
        if chips.len() <= 1 {
            return 1;
        }
        self.chip_layout(chips).2
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

    /// Push the focused pane's find state (highlight spans + bar
    /// summary + the "current" hit) for the next render. App calls
    /// this each tick. When `bar` is `None` no bar / highlights
    /// render.
    pub fn set_find(
        &mut self,
        bounds: Vec<(u16, u16, u16)>,
        bar: Option<(String, usize, usize)>,
        current: Option<(u16, u16, u16)>,
    ) {
        self.find_highlight_bounds = bounds;
        self.find_bar = bar;
        self.find_current_bounds = current;
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
    /// Pixel pad on the left edge of the vertical sidebar (when
    /// `tab_layout = Vertical`). Small breathing room between the
    /// window edge / inset and the first column of chip glyphs.
    const SIDEBAR_PAD_LEFT_PX: f32 = 8.0;

    /// Build glyph instances for the current chip list, positioned to
    /// land inside the tab strip. Uses fractional / negative `cell_pos`
    /// values so the existing cell pipeline draws each glyph (and its
    /// per-cell bg) at pixel-precise locations regardless of inset.
    /// Layout per chip (consistent active / inactive, single / multi):
    ///
    ///   ` <attn?> <label> · × `
    ///
    /// Active chip: bg = `palette().active_chip_bg` (lightened) + BOLD fg.
    /// Inactive chip: bg = palette().strip_bg, fg = palette().dim_fg.
    /// Attention chip (inactive only): a leading `● ` in red.
    /// The `×` close glyph is muted (no red shout) and sits one cell
    /// away from the label so it doesn't crowd the text. Always present
    /// — click is a no-op on single-tab (close_tab_at refuses), but the
    /// visual stays consistent across active / inactive and single /
    /// multi-tab. After the last chip: a `+` new-tab button.
    /// Headless-only: rebuild chrome hit-rects (strip chips + palette
    /// cluster) without emitting glyphs / drawing to a Surface. Render
    /// short-circuits in headless mode so the rects normally populated
    /// as a side effect of `strip_chip_instances` + `strip_palette_chip_instances`
    /// stay empty — and chip-area clicks find nothing. Call this from
    /// the headless tick before dispatching a click and the rects are
    /// populated identically to real mode.
    pub fn populate_hit_rects(&mut self) {
        let _ = self.strip_chip_instances();
        let _ = self.strip_palette_chip_instances();
        let _ = self.strip_sidebar_toggle_instances();
        let _ = self.sidebar_header_instances();
    }

    /// Emit one cell instance per configured launcher icon — a
    /// vertical column on the left edge of the body region. Glyph
    /// sits in the middle cell of the 3-cell-wide rail (left-pad +
    /// glyph + right-pad). Each icon occupies `TAB_ROW_H_PX` of
    /// vertical space starting at the top of the body.
    ///
    /// Also (re)populates `launcher_icon_rects` so the click
    /// dispatcher can hit-test against the same geometry it just
    /// painted. Returns the instance list; caller appends to the
    /// frame's instance stream.
    /// Top-strip variant of the launcher rail — paints icons
    /// horizontally in the chrome bar starting just past the
    /// sidebar toggle, each icon 3 cells wide ([pad] [glyph]
    /// [pad]). Hit-rects recorded in window pixels into the same
    /// `launcher_icon_rects` Vec the click router reads.
    fn launcher_top_strip_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        if self.strip_h <= 0.0 {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;
        // Start just past the toggle's right edge (toggle x ends at
        // ~180 + 5 cells = ~265). Add 1-cell gap before the icons.
        const TOGGLE_X_END_PX: f32 = 180.0 + 5.0 * 17.0;
        let start_x_px = TOGGLE_X_END_PX + cell_w * 2.0;
        let palette_zone_h = MACOS_TAB_STRIP_PX_SINGLE;
        let inset_y_total = self.inset_px + self.strip_h;
        let label_y = ((palette_zone_h - cell_h) * 0.5).max(0.0);
        let base_y = (label_y - inset_y_total) / cell_h;
        let mut out: Vec<pipeline::Instance> = Vec::with_capacity(self.launcher_icons.len());
        let icons = self.launcher_icons.clone();
        for (i, (glyph, fg)) in icons.iter().enumerate() {
            let icon_x_px = start_x_px + i as f32 * 3.0 * cell_w;
            // Cell-pipeline x-inset = inset_px + launcher_w_px +
            // sidebar_w_px. Cancel to land at icon_x_px.
            let base_col =
                (icon_x_px - self.inset_px - self.launcher_w_px - self.sidebar_w_px) / cell_w + 1.0; // +1 for the leading pad cell
            let Some(ch) = glyph.chars().next() else {
                continue;
            };
            let g = self.atlas.glyph(ch, style_from_attrs(0), &self.queue);
            out.push(pipeline::Instance {
                cell_pos: [base_col, base_y],
                fg: *fg,
                bg: palette().strip_bg,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
            // Hit-rect spans the whole 3-cell column for the click.
            let x0 = icon_x_px;
            let x1 = icon_x_px + 3.0 * cell_w;
            let y0 = 0.0;
            let y1 = self.strip_h;
            self.launcher_icon_rects.push((x0, x1, y0, y1, i));
        }
        out
    }

    fn launcher_chip_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        self.launcher_icon_rects.clear();
        if self.launcher_icons.is_empty() {
            return Vec::new();
        }
        // Top-strip variant: icons render horizontally inline in
        // the chrome bar, starting just past the sidebar toggle.
        if self.launcher_in_top_strip {
            return self.launcher_top_strip_instances();
        }
        if self.launcher_w_px <= 0.0 {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;
        let mut out = Vec::with_capacity(self.launcher_icons.len());

        // Glyph sits in col 1 of the 3-cell rail (col 0 is pad,
        // col 2 is pad). In window pixels: column 1's left edge is
        // `1.0 * cell_w`. The cell pipeline applies
        // `inset_px + launcher_w_px + sidebar_w_px` as x-inset; we
        // subtract that here so the glyph lands at the right window
        // pixel after the inset is added back. Same trick the strip
        // chips use against the sidebar.
        let start_x_px = cell_w; // 1 cell from window left
        let base_x = (start_x_px - self.inset_px - self.launcher_w_px - self.sidebar_w_px) / cell_w;

        // Icons stack down from the top of the body. y=0 of the
        // body grid sits at window pixel `inset_px + strip_h`; the
        // cell pipeline adds that back via `inset_y`. So base_y in
        // body-grid cell coords is row index.
        let row_h_px = TAB_ROW_H_PX;
        let icons = self.launcher_icons.clone();
        for (i, (glyph, fg)) in icons.iter().enumerate() {
            let row_top_px = self.strip_h + (i as f32) * row_h_px;
            // Center the glyph vertically within the row.
            let glyph_top_px = row_top_px + (row_h_px - cell_h) * 0.5;
            let base_y = (glyph_top_px - self.inset_px - self.strip_h) / cell_h;

            // First char only — the config's `glyph` is a single
            // cell. Multi-cell strings paint just their first cell.
            let Some(ch) = glyph.chars().next() else {
                continue;
            };
            let g = self.atlas.glyph(ch, style_from_attrs(0), &self.queue);
            out.push(pipeline::Instance {
                cell_pos: [base_x, base_y],
                fg: *fg,
                bg: palette().strip_bg,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
            // Hit-rect spans the WHOLE row (full rail width) so the
            // click target is forgiving — users don't need to land
            // pixel-perfect on the glyph itself.
            let x0 = 0.0;
            let x1 = self.launcher_w_px;
            let y0 = self.inset_px + row_top_px;
            let y1 = y0 + row_h_px;
            self.launcher_icon_rects.push((x0, x1, y0, y1, i));
        }
        out
    }

    fn strip_chip_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        self.strip_chip_rects.clear();
        self.strip_chip_close_rects.clear();
        self.strip_new_tab_rect = None;
        // Skip when strip is disabled (non-macOS without strip
        // support). For single-tab horizontal mode, also bail — one
        // chip in a row reads weird. For single-tab VERTICAL mode,
        // keep rendering so the user sees their one tab in the
        // sidebar (gives the layout toggle a visible effect even
        // with a single tab).
        let vertical = matches!(self.tab_layout, crate::config::TabLayout::Vertical);
        if self.strip_h <= 0.0 {
            return Vec::new();
        }
        if !vertical && self.strip_chips.len() <= 1 {
            return Vec::new();
        }
        // 2026-06-09: vertical-mode chips render in the sidebar
        // column, but if the sidebar is auto-hidden (single-tab
        // TUI) `sidebar_w_px == 0` — the chip would otherwise
        // paint at the body's top-left corner overlapping the
        // hosted TUI. Skip the render in that case.
        if vertical && self.sidebar_w_px <= 0.0 {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;

        // Tab chips render in rows BELOW the palette. The palette
        // occupies the top `MACOS_TAB_STRIP_PX_SINGLE` pixels (so it
        // doesn't shift between single-tab and multi-tab modes); a
        // `TAB_GAP_PX` separator follows so the tab row reads as
        // distinct from the palette chrome above. When chips don't
        // fit on one row, they wrap to a new row; each row adds
        // `TAB_ROW_H_PX` to the strip. The total strip height was
        // set by App tick via `Gpu::required_strip_h` so there's
        // already vertical room for every row.
        let tab_zone_top_px = MACOS_TAB_STRIP_PX_SINGLE + TAB_GAP_PX;
        // Compute the wrap layout — per-chip (row, col_offset).
        let chips: Vec<(String, bool, bool)> = self.strip_chips.clone();
        let (slots, plus_slot, _rows) = self.chip_layout(&chips);

        // Active tab pill bg from the global chrome palette
        // (`theme.rs`). Kept as a local binding so the inner loops
        // can reuse the [f32; 4] without re-dereffing the OnceLock
        // each iteration.
        let active_chip_bg = palette().active_chip_bg;
        // Attention dot color — red, matches OSC 1337 "needs attention".
        const ATTENTION_FG: [f32; 4] = [0.95, 0.32, 0.32, 1.0];
        // Muted close glyph color — dimmer than dim_fg.
        const CLOSE_FG_INACTIVE: [f32; 4] = [0.40, 0.42, 0.48, 1.0];
        const CLOSE_FG_ACTIVE: [f32; 4] = [0.70, 0.72, 0.78, 1.0];
        const ATTR_BOLD: u32 = 1;

        let vertical = matches!(self.tab_layout, crate::config::TabLayout::Vertical);
        // In horizontal mode chips start `CHIP_START_X_PX` from the
        // window left (clear of the traffic-light buttons). In
        // vertical mode chips render in the LEFT SIDEBAR — start
        // `inset_px + small-pad` from the window left so they sit
        // flush with where the body grid would otherwise start.
        let start_x_px = if vertical {
            self.inset_px + Self::SIDEBAR_PAD_LEFT_PX
        } else {
            Self::CHIP_START_X_PX
        };
        // The cell pipeline applies `inset_px + sidebar_w_px` as its
        // x-inset (so the body grid sits right of the sidebar
        // column). Strip chips go through that same pipeline but
        // need to render AT `start_x_px` — not `start_x_px +
        // sidebar_w_px`. Subtract sidebar_w_px from base_x in cell
        // units so the cell-pipeline inset cancels out for chip
        // glyphs. 2026-06-08 fix — chips were rendering INSIDE the
        // body's column the whole time, painting over the prompt.
        let base_x = (start_x_px - self.inset_px - self.sidebar_w_px) / cell_w;
        let inset_y_total = self.inset_px + self.strip_h;
        // In horizontal mode rows stack BELOW the palette strip; in
        // vertical mode they stack BELOW the strip too but each row
        // is one chip. The vertical-mode formula aligns the chip's
        // TEXT baseline with the body's first text row (which sits
        // at `inset_px + strip_h`): subtract the chip-row's internal
        // top padding `(TAB_ROW_H_PX - cell_h) / 2` so the chip's
        // centered glyph lands at the body's row-0 y. Before
        // 2026-06-08 first_row_top_px = strip_h, which placed chips
        // ~12px above the prompt.
        let first_row_top_px = if vertical {
            // 2026-06-09: shift chips down by SIDEBAR_HEADER_H_PX
            // (header row) + SIDEBAR_HEADER_GAP_PX (breathing strip
            // — was 0, first chip kissed the search box).
            const SIDEBAR_HEADER_GAP_PX: f32 = 10.0;
            let off = SIDEBAR_HEADER_H_PX + SIDEBAR_HEADER_GAP_PX;
            (self.inset_px + self.strip_h + off - (TAB_ROW_H_PX - cell_h) * 0.5)
                .max(self.strip_h + off)
        } else {
            tab_zone_top_px
        };
        // Vertical mode: shift visible rows up by `sidebar_scroll_rows`
        // so wheel-scroll over the sidebar reveals chips below the
        // visible window. Horizontal mode never scrolls.
        let scroll_rows = if vertical {
            self.sidebar_scroll_rows
        } else {
            0.0
        };
        // Bottom of the chip-render area — used to skip chips that
        // would land below the visible window in vertical mode.
        let viewport_h = self.config.height as f32;
        let max_chip_y_px = viewport_h - TAB_ROW_H_PX;
        // Per-row Y coordinates — pre-compute so the chip render loop
        // doesn't need to recalculate per glyph.
        // HORIZONTAL mode: each row past row 0 gets `INTER_ROW_GAP_PX`
        // of extra space above it so the visible label-to-label
        // distance between wrapped chip rows matches the palette→
        // row-0 distance.
        // VERTICAL mode: 6px inter-row gap (`VERT_INTER_ROW_GAP_PX`)
        // so tabs read as distinct rows without the 16px-horizontal
        // gap that made the sidebar look stretched in an earlier
        // iteration. 2026-06-09 user feedback: the prior 0px felt
        // cramped.
        let inter_row_gap_for_mode = if vertical {
            VERT_INTER_ROW_GAP_PX
        } else {
            INTER_ROW_GAP_PX
        };
        let row_geom = |row: usize, is_plus: bool| -> (f32, f32, f32) {
            // (y0_px, y1_px, base_y_in_cell_coords)
            // The `+` chip in vertical mode gets an extra gap above
            // it (`VERT_PLUS_EXTRA_GAP_PX`) so it's visually
            // separated from the chip cluster — reads as an
            // affordance rather than another tab.
            let plus_extra = if is_plus && vertical {
                VERT_PLUS_EXTRA_GAP_PX
            } else {
                0.0
            };
            let y0 = first_row_top_px
                + (row as f32 - scroll_rows) * TAB_ROW_H_PX
                + row as f32 * inter_row_gap_for_mode
                + plus_extra;
            let y1 = y0 + TAB_ROW_H_PX;
            let label_y = (y0 + (TAB_ROW_H_PX - cell_h) * 0.5).max(0.0);
            (y0, y1, (label_y - inset_y_total) / cell_h)
        };
        // Vertical mode: skip chips whose row falls outside the
        // visible window — above (y0 < first_row_top_px) or below
        // (y0 > max_chip_y_px). Hit rects are also skipped, so
        // off-screen chips can't be clicked.
        let row_visible = |y0: f32| !vertical || (y0 >= first_row_top_px && y0 <= max_chip_y_px);
        let space_g = self.atlas.glyph(' ', style_from_attrs(0), &self.queue);
        let mut out: Vec<pipeline::Instance> = Vec::new();

        for (i, (label, active, attention)) in chips.iter().enumerate() {
            let (row, slot_col) = slots[i];
            let (chip_y0_px, chip_y1_px, base_y) = row_geom(row, false);
            // Vertical mode: skip chips scrolled off-screen so we
            // don't paint glyphs over the body grid above/below the
            // sidebar.
            if !row_visible(chip_y0_px) {
                continue;
            }
            let mut col_offset = slot_col;
            let chip_x0_px = start_x_px + col_offset * cell_w;
            // Tab-search filter: chips whose label doesn't substring-
            // match the query render dim. The active tab keeps its
            // bold/accent treatment so the user can see where they
            // are.
            let q_match = self
                .tab_search
                .as_deref()
                .map(|q| q.is_empty() || label.to_lowercase().contains(&q.to_lowercase()))
                .unwrap_or(true);
            let (fg, bg, attrs) = if *active {
                (palette().text_fg, active_chip_bg, ATTR_BOLD)
            } else if !q_match {
                (palette().dim_fg, palette().strip_bg, 0)
            } else {
                (palette().tab_fg, palette().strip_bg, 0)
            };

            // Helper: emit one cell at (base_x + col_offset, base_y),
            // advancing col_offset. Inlined so the borrow checker
            // is happy across the mutable &mut self.atlas calls in
            // the per-char glyph loop below.
            macro_rules! push_cell {
                ($glyph:expr, $cell_fg:expr, $cell_bg:expr, $cell_attrs:expr) => {{
                    let g = $glyph;
                    out.push(pipeline::Instance {
                        cell_pos: [base_x + col_offset, base_y],
                        fg: $cell_fg,
                        bg: $cell_bg,
                        uv_min: g.uv_min,
                        uv_max: g.uv_max,
                        glyph_offset: g.offset,
                        glyph_size: g.size,
                        attrs: $cell_attrs,
                        _pad: 0,
                    });
                    col_offset += 1.0;
                }};
            }

            // Left pad.
            for _ in 0..Self::CHIP_PAD_CELLS as usize {
                push_cell!(space_g, fg, bg, 0);
            }
            // Attention dot (red ● + trailing space) on inactive chips
            // that have the flag set. Active chips clear the flag on
            // focus so we don't repeat.
            if *attention && !*active {
                let dot_g = self.atlas.glyph('●', style_from_attrs(0), &self.queue);
                push_cell!(dot_g, ATTENTION_FG, bg, 0);
                push_cell!(space_g, fg, bg, 0);
            }
            // Label glyphs.
            for ch in label.chars() {
                let g = self.atlas.glyph(ch, style_from_attrs(attrs), &self.queue);
                push_cell!(g, fg, bg, attrs);
            }
            // Gap before close.
            push_cell!(space_g, fg, bg, 0);
            // Close glyph — SKIPPED for empty labels in vertical
            // mode. 2026-06-09 SEV-1 mouse-hunt fix: a brand-new
            // shell tab starts with an empty label, which puts the
            // close `×` at the LEFT side of the sidebar (x ≈ 50)
            // — clicking the chip's visible center closed the tab
            // because the close-rect overlapped the chip body. No
            // label = no close affordance until the title arrives.
            let close_glyph_rendered = !(vertical && label.is_empty());
            let close_x_px = start_x_px + col_offset * cell_w;
            if close_glyph_rendered {
                let close_fg = if *active {
                    CLOSE_FG_ACTIVE
                } else {
                    CLOSE_FG_INACTIVE
                };
                let close_g = self
                    .atlas
                    .glyph('\u{00D7}', style_from_attrs(0), &self.queue);
                push_cell!(close_g, close_fg, bg, 0);
            } else {
                push_cell!(space_g, fg, bg, 0);
            }
            // Right pad.
            for _ in 0..Self::CHIP_PAD_CELLS as usize {
                push_cell!(space_g, fg, bg, 0);
            }

            // Record hit-rects with Y bounds so wrap rows are
            // distinguished on click. 2026-06-09 SEV-2 mouse-hunt fix:
            // in vertical mode, EXTEND the chip's hit rect to the
            // sidebar's right edge so the whole row activates the
            // chip (Warp / VS Code behavior). The close-badge hit-
            // rect stays narrow so the × still works, and it's
            // tested BEFORE this rect in the click router. Horizontal
            // mode keeps the shrink-wrapped chip width.
            let chip_x1_px = if vertical {
                self.inset_px + self.sidebar_w_px
            } else {
                start_x_px + col_offset * cell_w
            };
            self.strip_chip_rects
                .push((chip_x0_px, chip_x1_px, chip_y0_px, chip_y1_px, i));
            if close_glyph_rendered {
                self.strip_chip_close_rects.push((
                    close_x_px,
                    close_x_px + cell_w,
                    chip_y0_px,
                    chip_y1_px,
                    i,
                ));
            }
        }
        // `+` new-tab button — wraps to its own row if the last chip
        // row didn't have space (the chip_layout helper figured this
        // out for us). In vertical mode skipped when scrolled off the
        // bottom; `clamp_sidebar_scroll` keeps it reachable.
        //
        // 2026-06-09: vertical mode now puts the `+` button in the
        // sidebar HEADER (next to the search input) — see
        // `sidebar_header_instances`. So we suppress the bottom-of-
        // sidebar plus in vertical to avoid duplicate render + hit
        // rects clobbering each other.
        if !vertical {
            let (plus_row, plus_col_offset) = plus_slot;
            let (plus_y0_px, plus_y1_px, plus_base_y) = row_geom(plus_row, true);
            if row_visible(plus_y0_px) {
                let plus_x_px = start_x_px + plus_col_offset * cell_w;
                self.push_plus_button(&mut out, plus_x_px, plus_base_y, plus_y0_px, plus_y1_px);
            }
        }
        out
    }

    /// VS Code-style command-palette cluster in the chrome strip —
    /// `[←][→]  [ 🔍  search files, run commands…  ▾ ]`. Three
    /// clickable regions are stored as separate rects:
    ///   * `strip_palette_back_rect` → `Ctrl+PageUp`  (buffer.prev)
    ///   * `strip_palette_fwd_rect`  → `Ctrl+PageDown` (buffer.next)
    ///   * `strip_palette_chip_rect` → `Ctrl+Shift+P` (palette)
    ///   * `strip_palette_dropdown_rect` → `Ctrl+R`   (picker.recent)
    ///
    /// Glyphs are Codicons (`nf-cod-*`) — same family VS Code uses, so
    /// the look matches. Renders only when the strip is visible.
    fn strip_palette_chip_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        self.strip_palette_back_rect = None;
        self.strip_palette_fwd_rect = None;
        self.strip_palette_chip_rect = None;
        self.strip_palette_dropdown_rect = None;
        self.palette_left_x_px = None;
        if self.strip_h <= 0.0 {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;
        // Palette always centers within the single-tab strip
        // region — even when chips are showing below. The strip
        // height grows in multi-tab mode (palette zone +
        // TAB_ROW_H_PX), but the palette's vertical position
        // stays put. Keeps the palette from appearing to "jump up"
        // when a 2nd tab opens.
        let palette_zone_h = MACOS_TAB_STRIP_PX_SINGLE;
        let label_y_px = ((palette_zone_h - cell_h) * 0.5).max(0.0);
        let inset_y_total = self.inset_px + self.strip_h;
        let base_y = (label_y_px - inset_y_total) / cell_h;

        // Build the cluster as a flat char vec so we can render each
        // glyph at a known column index AND map column ranges back to
        // hit-rects. The slot indices below are 0-based char offsets.
        //   back:  cells 0..3   (" ← ")
        //   fwd:   cells 3..6   (" → ")
        //   gap:   cells 6..9   (3 spaces, unclickable)
        //   chip:  cells 9..(9+chip_body_w)
        //   drop:  next 3 cells
        // Glyphs: EA9B nf-cod-arrow-left, EA9C nf-cod-arrow-right,
        //         F0349 nf-md-magnify, EAB4 nf-cod-chevron-down.
        let back_text = " \u{EA9B} ";
        let fwd_text = " \u{EA9C} ";
        // No explicit strip-bg gap between the nav cluster and the chip
        // — the buttons' built-in `" glyph "` padding + the chip's
        // built-in leading "  " already provide a balanced spacing that
        // matches the gap between the back and forward buttons.
        // (Was 3 cells of strip-bg; visually too wide.)
        let gap_text = "";
        // Chip label: prefer the active tab's title (e.g. mnml's
        // workspace name) over the static placeholder so the chip
        // matches what mnml's inline-mode palette bar shows. Falls
        // back to the placeholder when no native tab has sent a title
        // yet (welcome screen / shell mode). Padded to a constant
        // 24-cell width so the chip stays the same size regardless of
        // label length; long titles truncate with `…`.
        const CHIP_LABEL_W: usize = 24;
        // Tab-search mode wins over the active-tab label — show
        // the user's query (with a trailing cursor caret) so they
        // can see what they're typing. Empty query renders a
        // placeholder.
        //
        // Vertical-tab mode: the sidebar header IS the input the
        // user is typing into; mirroring that query into the top
        // palette chip too reads as distracting (the top chip is
        // not the active input). Skip the tab_search branch here
        // — top chip falls back to the active-tab label, the
        // sidebar header shows the live query.
        let vertical = matches!(self.tab_layout, crate::config::TabLayout::Vertical);
        let active_label = if !vertical && let Some(q) = self.tab_search.as_deref() {
            if q.is_empty() {
                "Search tabs…".to_string()
            } else {
                format!("{q}▏")
            }
        } else {
            self.strip_chips
                .iter()
                .find(|(_, active, _)| *active)
                .map(|(label, _, _)| label.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "search files, run commands…".to_string())
        };
        let label = if active_label.chars().count() > CHIP_LABEL_W {
            let mut s: String = active_label.chars().take(CHIP_LABEL_W - 1).collect();
            s.push('…');
            s
        } else {
            let need = CHIP_LABEL_W - active_label.chars().count();
            let mut s = active_label;
            s.extend(std::iter::repeat_n(' ', need));
            s
        };
        let chip_body = format!("  \u{F0349}  {label}  ");
        let chip_body = chip_body.as_str();
        let dropdown_text = " \u{EAB4} ";

        let back_cells = back_text.chars().count();
        let fwd_cells = fwd_text.chars().count();
        let gap_cells = gap_text.chars().count();
        let chip_body_cells = chip_body.chars().count();
        let dropdown_cells = dropdown_text.chars().count();
        let total_cells = back_cells + fwd_cells + gap_cells + chip_body_cells + dropdown_cells;
        let total_w_px = total_cells as f32 * cell_w;
        let window_w_px = self.config.width as f32;
        if window_w_px < Self::CHIP_START_X_PX + total_w_px + 40.0 {
            return Vec::new();
        }
        // Center over the FULL window in both layouts. Earlier the
        // palette centered over just the body region (right of the
        // sidebar), which made it sit visibly far-right in vertical
        // mode — looked like a misalignment rather than chrome.
        // Window-wide centering keeps the palette anchored to the
        // window the user is actually looking at, regardless of
        // sidebar visibility. 2026-06-09 user feedback.
        let start_x_px = (window_w_px - total_w_px) / 2.0;
        // Same cancel-out-the-cell-pipeline-x-inset trick the chip
        // path uses — see strip_chip_instances.
        let start_col = (start_x_px - self.inset_px - self.sidebar_w_px) / cell_w;

        // Chrome colors from the global palette (`theme.rs`). Mnml
        // uses a 3-tier gradient inside the bufferline cluster:
        // arrow buttons sit one tier off the strip; the search-chip
        // input is lifted further so it reads as the primary
        // affordance.
        let btn_bg = palette().btn_bg;
        let chip_bg = palette().chip_bg;
        // Foreground glyphs: brighter on the arrow buttons (the
        // navigation affordance reads as "actionable"), slightly
        // dimmer on the chip body where the search-text placeholder
        // lives. Both still well-contrasted against `chip_bg`.
        // (Local-only — not part of the user-facing palette.)
        const BTN_FG: [f32; 4] = [0.70, 0.72, 0.78, 1.0];
        const CHIP_FG: [f32; 4] = [0.55, 0.58, 0.65, 1.0];

        let mut out: Vec<pipeline::Instance> = Vec::new();
        let push = |out: &mut Vec<pipeline::Instance>,
                    col: f32,
                    ch: char,
                    fg: [f32; 4],
                    bg: [f32; 4],
                    atlas: &mut crate::atlas::Atlas,
                    queue: &wgpu::Queue| {
            let g = atlas.glyph(ch, style_from_attrs(0), queue);
            out.push(pipeline::Instance {
                cell_pos: [col, base_y],
                fg,
                bg,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
        };

        let mut col = start_col;
        // Back arrow.
        for ch in back_text.chars() {
            push(
                &mut out,
                col,
                ch,
                BTN_FG,
                btn_bg,
                &mut self.atlas,
                &self.queue,
            );
            col += 1.0;
        }
        // Forward arrow.
        for ch in fwd_text.chars() {
            push(
                &mut out,
                col,
                ch,
                BTN_FG,
                btn_bg,
                &mut self.atlas,
                &self.queue,
            );
            col += 1.0;
        }
        // Gap — render strip-bg spaces so the chip looks visually
        // detached from the arrows. (Currently `gap_text` is empty,
        // so this loop is a no-op; bg sourced from palette().strip_bg anyway
        // so the dead branch doesn't drift from the chrome palette.)
        for ch in gap_text.chars() {
            push(
                &mut out,
                col,
                ch,
                BTN_FG,
                palette().strip_bg,
                &mut self.atlas,
                &self.queue,
            );
            col += 1.0;
        }
        // Chip body.
        for ch in chip_body.chars() {
            push(
                &mut out,
                col,
                ch,
                CHIP_FG,
                chip_bg,
                &mut self.atlas,
                &self.queue,
            );
            col += 1.0;
        }
        // Dropdown.
        for ch in dropdown_text.chars() {
            push(
                &mut out,
                col,
                ch,
                CHIP_FG,
                chip_bg,
                &mut self.atlas,
                &self.queue,
            );
            col += 1.0;
        }
        let _ = btn_bg;
        let _ = col;

        // Map cells back to pixel rects.
        let cells_x = |c0: usize, c_count: usize| -> (f32, f32) {
            let x0 = start_x_px + c0 as f32 * cell_w;
            (x0, x0 + c_count as f32 * cell_w)
        };
        let y0 = 0.0;
        let y1 = self.strip_h;
        let (bx0, bx1) = cells_x(0, back_cells);
        let (fx0, fx1) = cells_x(back_cells, fwd_cells);
        let (cx0, cx1) = cells_x(back_cells + fwd_cells + gap_cells, chip_body_cells);
        let (dx0, dx1) = cells_x(
            back_cells + fwd_cells + gap_cells + chip_body_cells,
            dropdown_cells,
        );
        self.strip_palette_back_rect = Some((bx0, bx1, y0, y1));
        self.strip_palette_fwd_rect = Some((fx0, fx1, y0, y1));
        self.strip_palette_chip_rect = Some((cx0, cx1, y0, y1));
        self.strip_palette_dropdown_rect = Some((dx0, dx1, y0, y1));
        self.palette_left_x_px = Some(bx0);
        out
    }

    /// Sidebar-toggle button — sits between the macOS traffic-light
    /// buttons and the start of the tab chip cluster (just like
    /// Warp's toggle, image #21 reference). Clicking cycles
    /// `cfg.tab_layout` Horizontal ↔ Vertical (App handles the flip
    /// via `strip_sidebar_toggle_rect`).
    ///
    /// Geometry: 5-cell pill at pixel-x = 180, vertically centered
    /// in the strip. The `disable_window_drag` swizzle in
    /// `App::resumed` is what makes this position clickable —
    /// without it, macOS swallows clicks for window-dragging.
    fn strip_sidebar_toggle_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        self.strip_sidebar_toggle_rect = None;
        if self.strip_h <= 0.0 {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;
        // Tighter chip: 1 cell of internal padding on each side
        // instead of 2, AND zero cells of pad between the divider
        // and the chip's left edge. Was reading as "floating off
        // in body chrome" — now it hugs the seam, which matches
        // the "control for THIS divider" intent.
        // 2026-06-09 user feedback.
        let cells = [" ", "\u{EBF4}", " "].concat();
        let total_cells = cells.chars().count();
        // Position branches on tab_layout:
        //   * Vertical, sidebar visible — anchor to the body's
        //     LEFT edge (right against the divider, no pad).
        //   * Vertical, sidebar hidden (single-tab TUI) — same
        //     anchor (inset_px + sidebar_w_px = inset_px), so
        //     the toggle sits at the body's left edge and the
        //     user can click it to RE-OPEN the sidebar.
        //   * Horizontal — no sidebar; keep the legacy position
        //     next to the macOS traffic lights (x = 180).
        // 2026-06-09 (re-revisit): user asked for it next to the
        // palette arrows again. Anchor to the LEFT of the palette
        // cluster's back-arrow when the palette rendered this
        // tick; fall back to x=180 next to the traffic-light
        // buttons when it didn't.
        const TOGGLE_X_LEFT_PX: f32 = 180.0;
        let toggle_w_px = total_cells as f32 * cell_w;
        let gap_px = cell_w; // 1 cell of gap between toggle and back arrow
        let toggle_x_px = match self.palette_left_x_px {
            Some(palette_left) => palette_left - toggle_w_px - gap_px,
            None => TOGGLE_X_LEFT_PX,
        };
        // Vertically center against the palette zone (same constant
        // `strip_palette_chip_instances` uses) so the toggle sits at
        // the same y as the macOS traffic-light buttons regardless
        // of whether the strip has grown to hold wrapped tab rows.
        let palette_zone_h = MACOS_TAB_STRIP_PX_SINGLE;
        let inset_y_total = self.inset_px + self.strip_h;
        let label_y = ((palette_zone_h - cell_h) * 0.5).max(0.0);
        let base_y = (label_y - inset_y_total) / cell_h;
        let start_col = (toggle_x_px - self.inset_px - self.sidebar_w_px) / cell_w;
        const TOGGLE_FG: [f32; 4] = [0.78, 0.80, 0.85, 1.0];
        // Match the surrounding strip bg so the glyph reads as a
        // bare icon, not a pill — the lighter `btn_bg` painted a
        // visible square around the glyph against the strip.
        let toggle_bg = palette().strip_bg;
        let mut out: Vec<pipeline::Instance> = Vec::new();
        for (i, ch) in cells.chars().enumerate() {
            let g = self.atlas.glyph(ch, style_from_attrs(0), &self.queue);
            out.push(pipeline::Instance {
                cell_pos: [start_col + i as f32, base_y],
                fg: TOGGLE_FG,
                bg: toggle_bg,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
        }
        let x0 = toggle_x_px;
        let x1 = x0 + total_cells as f32 * cell_w;
        let y0_px = 0.0;
        let y1_px = self.strip_h;
        self.strip_sidebar_toggle_rect = Some((x0, x1, y0_px, y1_px));
        out
    }

    /// Warp-style sidebar header row — search input + `+` new-tab
    /// button stacked at the top of the vertical-tab sidebar (just
    /// below the macOS top strip). Renders only in vertical-tab
    /// mode with a non-zero sidebar width.
    ///
    /// Layout (left→right):
    ///   [SIDEBAR_PAD_LEFT_PX] [search input ........... ] [1 cell gap] [+ button] [1 cell pad]
    ///
    /// The search input shows the magnify glyph + a placeholder
    /// "Search tabs…" (or the active `tab_search` query). Clicking
    /// it toggles tab-search on Shell panes, or fires the palette
    /// chord on Native panes — same routing as the top-strip
    /// search chip.
    fn sidebar_header_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        self.sidebar_search_rect = None;
        self.sidebar_plus_rect = None;
        let vertical = matches!(self.tab_layout, crate::config::TabLayout::Vertical);
        if !vertical || self.sidebar_w_px <= 0.0 || self.strip_h <= 0.0 {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;

        // Total cells the sidebar can host (minus the left pad
        // before the search box).
        let sidebar_cells = ((self.sidebar_w_px - Self::SIDEBAR_PAD_LEFT_PX) / cell_w).floor();
        if sidebar_cells < 8.0 {
            // Too narrow to fit search + plus; just skip.
            return Vec::new();
        }
        // Reserve right-side cells: 3-cell `+` button + 1 cell gap.
        // `right_pad` = 2 cells of breathing room between the plus
        // button and the sidebar's right divider — was 1, plus
        // visibly kissed the divider. 2026-06-09 user feedback.
        let plus_cells = 3.0;
        let gap_cells = 1.0;
        let right_pad = 2.0;
        let search_cells = (sidebar_cells - plus_cells - gap_cells - right_pad).max(4.0);

        // Header sits in the body region just below the top strip.
        let y0_px = self.inset_px + self.strip_h;
        let y1_px = y0_px + SIDEBAR_HEADER_H_PX;
        let label_y_px = y0_px + (SIDEBAR_HEADER_H_PX - cell_h) * 0.5;
        let inset_y_total = self.inset_px + self.strip_h;
        let base_y = (label_y_px - inset_y_total) / cell_h;

        // Cell pipeline applies (inset_px + sidebar_w_px) as x-inset.
        // We render in the sidebar's own column, so cancel both.
        let start_x_px = self.inset_px + Self::SIDEBAR_PAD_LEFT_PX;
        let start_col = (start_x_px - self.inset_px - self.sidebar_w_px) / cell_w;

        // Search-box label — caret semantics:
        //   * None          — input is INACTIVE; show dim
        //                     "Search tabs…" placeholder.
        //   * Some(empty)   — input is ACTIVE but no chars yet;
        //                     show a caret so the click registers
        //                     visually.
        //   * Some(query)   — input has chars; show them with a
        //                     trailing caret.
        let label_raw = match self.tab_search.as_deref() {
            None => "Search tabs…".to_string(),
            Some("") => "▏".to_string(),
            Some(q) => format!("{q}▏"),
        };
        // Build body: " 🔍 label …pad… "
        // 1 cell pad + 1 cell magnify + 1 cell pad + label + right pad
        let inner_label_cells = (search_cells as usize).saturating_sub(4);
        let mut label_text: String = label_raw.chars().take(inner_label_cells).collect();
        while label_text.chars().count() < inner_label_cells {
            label_text.push(' ');
        }
        // Magnify glyph: F0349 nf-md-magnify (same as top-strip chip).
        let magnify = '\u{F0349}';

        let chip_bg = palette().chip_bg;
        const SEARCH_FG: [f32; 4] = [0.62, 0.65, 0.72, 1.0];
        const PLUS_BG: [f32; 4] = [0.18, 0.20, 0.24, 1.0];

        let mut out: Vec<pipeline::Instance> = Vec::new();
        let push = |out: &mut Vec<pipeline::Instance>,
                    col: f32,
                    ch: char,
                    fg: [f32; 4],
                    bg: [f32; 4],
                    atlas: &mut crate::atlas::Atlas,
                    queue: &wgpu::Queue| {
            let g = atlas.glyph(ch, style_from_attrs(0), queue);
            out.push(pipeline::Instance {
                cell_pos: [col, base_y],
                fg,
                bg,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
        };

        // Search input — full search_cells wide.
        let mut col = start_col;
        let search_start_col = col;
        // Leading pad.
        push(
            &mut out,
            col,
            ' ',
            SEARCH_FG,
            chip_bg,
            &mut self.atlas,
            &self.queue,
        );
        col += 1.0;
        // Magnify glyph.
        push(
            &mut out,
            col,
            magnify,
            SEARCH_FG,
            chip_bg,
            &mut self.atlas,
            &self.queue,
        );
        col += 1.0;
        // Gap.
        push(
            &mut out,
            col,
            ' ',
            SEARCH_FG,
            chip_bg,
            &mut self.atlas,
            &self.queue,
        );
        col += 1.0;
        // Label.
        for ch in label_text.chars() {
            push(
                &mut out,
                col,
                ch,
                SEARCH_FG,
                chip_bg,
                &mut self.atlas,
                &self.queue,
            );
            col += 1.0;
        }
        // Trailing pad.
        push(
            &mut out,
            col,
            ' ',
            SEARCH_FG,
            chip_bg,
            &mut self.atlas,
            &self.queue,
        );
        col += 1.0;
        let search_end_col = col;

        // Gap between search and `+`.
        col += gap_cells;

        // `+` button — 3 cells, distinct bg from search input.
        let plus_start_col = col;
        for (i, ch) in [' ', '+', ' '].iter().enumerate() {
            push(
                &mut out,
                col + i as f32,
                *ch,
                palette().text_fg,
                PLUS_BG,
                &mut self.atlas,
                &self.queue,
            );
        }
        let plus_end_col = plus_start_col + 3.0;

        // Pixel-rect hit targets — computed DIRECTLY from physical
        // pixels rather than walking the cell-column accumulator
        // through five padding/glyph increments. The accumulator
        // matched the render geometry but was easy to get
        // off-by-one (visually rendered but the click missed by a
        // cell). The click handler reads window pixels anyway, so
        // this removes a whole class of mismatch.
        let _ = (
            search_start_col,
            search_end_col,
            plus_start_col,
            plus_end_col,
            gap_cells,
        );
        // 2026-06-09 SEV-2 mouse-hunt fix: search and plus rects
        // used to leave a 1-cell `gap_cells` dead band between
        // them — clicks there hit nothing. Extend the search rect
        // up to the plus rect's left edge so the header has no
        // dead pixels.
        let sidebar_right_x_px = self.inset_px + self.sidebar_w_px;
        let plus_right_x_px = sidebar_right_x_px - right_pad * cell_w;
        let plus_left_x_px = plus_right_x_px - plus_cells * cell_w;
        let search_left_x_px = self.inset_px + Self::SIDEBAR_PAD_LEFT_PX;
        let search_right_x_px = plus_left_x_px;
        self.sidebar_search_rect = Some((search_left_x_px, search_right_x_px, y0_px, y1_px));
        self.sidebar_plus_rect = Some((plus_left_x_px, plus_right_x_px, y0_px, y1_px));
        out
    }

    /// Paint the trailing `+` new-tab button at `plus_x_px` and record
    /// its pixel-x extent on `strip_new_tab_rect`. The chrome is a
    /// single glyph (`+`) padded left/right with two spaces so the
    /// click target is comfortably-sized.
    fn push_plus_button(
        &mut self,
        out: &mut Vec<pipeline::Instance>,
        plus_x_px: f32,
        base_y: f32,
        y0_px: f32,
        y1_px: f32,
    ) {
        use crate::atlas::style_from_attrs;
        // Slightly lifted bg so the button reads as chrome rather than
        // strip filler. Same shade as the active chip.
        const PLUS_BG: [f32; 4] = [0.18, 0.20, 0.24, 1.0];
        let cell_w = self.atlas.cell_w;
        // Same cancel-the-cell-pipeline-x-inset correction the
        // chip / palette paths use — without `- sidebar_w_px`, the
        // `+` button renders inside the body column instead of the
        // sidebar. 2026-06-08: the chip path had this fix but the
        // `+` button render slipped through.
        let plus_x = (plus_x_px - self.inset_px - self.sidebar_w_px) / cell_w;
        let space_g = self.atlas.glyph(' ', style_from_attrs(0), &self.queue);
        let plus_g = self.atlas.glyph('+', style_from_attrs(0), &self.queue);
        // 3-cell button: [space, +, space]
        for (i, g) in [&space_g, &plus_g, &space_g].iter().enumerate() {
            out.push(pipeline::Instance {
                cell_pos: [plus_x + i as f32, base_y],
                fg: palette().text_fg,
                bg: PLUS_BG,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
        }
        self.strip_new_tab_rect = Some((plus_x_px, plus_x_px + 3.0 * cell_w, y0_px, y1_px));
    }

    /// In-pane find bar — a one-row chip at the top-left of the
    /// body grid. Renders the magnify glyph + query + match
    /// counter + "esc". Only paints when `find_bar` is `Some`.
    /// Lives on body row 0; cell instances overlay whatever vt100
    /// would have painted there. Cleared automatically when
    /// `find_bar` goes back to `None`.
    fn find_bar_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        let Some((query, current, total)) = self.find_bar.clone() else {
            return Vec::new();
        };
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;
        // Bar text: " 🔍 <query> (n of m) esc "
        let counter = if total == 0 {
            String::from("(no match)")
        } else {
            format!("({} of {})", current + 1, total)
        };
        let max_q = 32;
        let q_show: String = if query.chars().count() > max_q {
            let mut s: String = query.chars().take(max_q - 1).collect();
            s.push('…');
            s
        } else if query.is_empty() {
            "Find…".to_string()
        } else {
            query.clone()
        };
        let body = format!(" \u{F0349}  {q_show}  {counter}  esc ");
        let total_cells = body.chars().count() as f32;

        // Position: body row 0 column 0. Cell-pipeline applies
        // (inset_px + launcher_w_px + sidebar_w_px) as x inset +
        // (inset_px + strip_h) as y inset, so cell_pos = (0, 0)
        // lands at the top-left cell of the body grid.
        let chip_bg = palette().chip_bg;
        const FG: [f32; 4] = [0.95, 0.90, 0.45, 1.0]; // soft yellow
        let mut out: Vec<pipeline::Instance> = Vec::with_capacity(body.chars().count());
        for (i, ch) in body.chars().enumerate() {
            let g = self.atlas.glyph(ch, style_from_attrs(0), &self.queue);
            out.push(pipeline::Instance {
                cell_pos: [i as f32, 0.0],
                fg: FG,
                bg: chip_bg,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
        }
        let _ = (cell_w, cell_h, total_cells);
        out
    }

    /// Subtle vertical drag-handle painted on the sidebar's
    /// right edge — 3 stacked `⋮` glyphs at the body's vertical
    /// midpoint. Dim grey. The 1-px divider line painted by the
    /// strip pipeline carries the visual seam; this handle just
    /// hints "drag me" without dominating the chrome.
    fn sidebar_drag_handle_instances(&mut self) -> Vec<pipeline::Instance> {
        use crate::atlas::style_from_attrs;
        if !matches!(self.tab_layout, crate::config::TabLayout::Vertical)
            || self.sidebar_w_px <= 0.0
        {
            return Vec::new();
        }
        let cell_w = self.atlas.cell_w;
        let cell_h = self.atlas.cell_h;
        let handle_x_px = self.inset_px + self.sidebar_w_px - cell_w;
        let base_col = (handle_x_px - self.inset_px - self.sidebar_w_px) / cell_w;
        let body_top_px = self.inset_px + self.strip_h;
        let body_bottom_px = self.config.height as f32;
        let mid_py = (body_top_px + body_bottom_px) * 0.5;
        let inset_y_total = self.inset_px + self.strip_h;
        const HANDLE_FG: [f32; 4] = [0.30, 0.32, 0.36, 1.0];
        // `┃` (U+2503 BOX DRAWINGS HEAVY VERTICAL) — thicker
        // sibling of `│`. Box-drawing glyphs are designed to abut
        // top-to-bottom when stacked, so two cells form a
        // continuous line with no gap. Heavy variant reads more
        // clearly than the light one.
        let g = self.atlas.glyph('┃', style_from_attrs(0), &self.queue);
        let mut out: Vec<pipeline::Instance> = Vec::with_capacity(2);
        for offset in 0..2 {
            let glyph_top_px = mid_py + (offset as f32 - 1.0) * cell_h;
            let base_y = (glyph_top_px - inset_y_total) / cell_h;
            out.push(pipeline::Instance {
                cell_pos: [base_col, base_y],
                fg: HANDLE_FG,
                bg: palette().clear_bg,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                glyph_offset: g.offset,
                glyph_size: g.size,
                attrs: 0,
                _pad: 0,
            });
        }
        out
    }

    fn resize(&mut self, w: u32, h: u32) -> Option<(u16, u16)> {
        if w == 0 || h == 0 {
            return None;
        }
        self.config.width = w;
        self.config.height = h;
        // Headless mode skips the wgpu surface configure — there's
        // no surface to reconfigure. The grid_dims math below still
        // uses the new width/height, so logical resize still works.
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        let (cols, rows) = grid_dims(
            w,
            h,
            &self.atlas,
            self.inset_px,
            self.strip_h,
            self.sidebar_w_px,
            self.launcher_w_px,
        );
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
        let (cols, rows) = grid_dims(
            w,
            h,
            &self.atlas,
            self.inset_px,
            self.strip_h,
            self.sidebar_w_px,
            self.launcher_w_px,
        );
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
        let (cols, rows) = grid_dims(
            w,
            h,
            &self.atlas,
            self.inset_px,
            self.strip_h,
            self.sidebar_w_px,
            self.launcher_w_px,
        );
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            return Some((cols as u16, rows as u16));
        }
        None
    }

    /// Update the vertical-tab sidebar's pixel width live. Returns
    /// the new grid dims if they shifted (caller forwards the resize
    /// to the native client). Set to 0 in horizontal mode. Computed
    /// from the current chip list by [`Self::compute_sidebar_w_px`]
    /// each tick.
    fn set_sidebar_w_px(&mut self, sidebar_w_px: f32) -> Option<(u16, u16)> {
        if (self.sidebar_w_px - sidebar_w_px).abs() < f32::EPSILON {
            return None;
        }
        self.sidebar_w_px = sidebar_w_px;
        let (w, h) = (self.config.width, self.config.height);
        let (cols, rows) = grid_dims(
            w,
            h,
            &self.atlas,
            self.inset_px,
            self.strip_h,
            self.sidebar_w_px,
            self.launcher_w_px,
        );
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            return Some((cols as u16, rows as u16));
        }
        None
    }

    /// Pixel width of the left-edge launcher rail. The rail is a
    /// fixed 3-cell column when any launcher icons are configured
    /// (one cell each side of pad + one for the glyph), and 0 when
    /// the list is empty.
    fn compute_launcher_w_px(&self, icon_count: usize) -> f32 {
        if icon_count == 0 {
            return 0.0;
        }
        // 3 cells: left pad + glyph + right pad. Matches mnml's
        // integration-rail visual rhythm.
        3.0 * self.atlas.cell_w
    }

    /// Replace the cached `(glyph, fg)` list used by
    /// `launcher_chip_instances`. App calls this with the resolved
    /// list whenever `cfg.launcher_icons` changes (each entry's
    /// `color` field, if present, is parsed once here so the
    /// per-frame render path doesn't re-parse hex on every paint).
    pub fn set_launcher_icons(&mut self, icons: Vec<(String, [f32; 4])>) {
        self.launcher_icons = icons;
    }

    /// Physical-pixel width of a single cell in the active font.
    /// Used by App to size the sidebar's drag-handle hot zone to
    /// the rightmost cell column (matching mnml's tree-rail
    /// `tree_edge` design).
    pub fn atlas_cell_w(&self) -> f32 {
        self.atlas.cell_w
    }

    /// Switch the launcher rail between the left-edge column
    /// (default) and an inline strip in the top chrome bar. App
    /// reads `cfg.launcher_position` each tick and pushes the
    /// value here.
    pub fn set_launcher_in_top_strip(&mut self, on: bool) {
        self.launcher_in_top_strip = on;
    }

    /// Hit rects produced by the last `launcher_chip_instances`
    /// call — `(x0, x1, y0, y1, idx)` in window pixels with `idx`
    /// pointing into the config's `launcher_icons` Vec. Click
    /// dispatch (commit C) consumes this.
    #[allow(dead_code)] // wired in commit C of the launcher rail series
    pub fn launcher_icon_hit_rects(&self) -> &[(f32, f32, f32, f32, usize)] {
        &self.launcher_icon_rects
    }

    /// Push the requested rail width to the GPU + resize the body
    /// grid. Returns `Some((cols, rows))` iff the resize actually
    /// changed the grid (so callers know to relayout). Mirrors
    /// `set_sidebar_w_px`.
    fn set_launcher_w_px(&mut self, launcher_w_px: f32) -> Option<(u16, u16)> {
        if (self.launcher_w_px - launcher_w_px).abs() < f32::EPSILON {
            return None;
        }
        self.launcher_w_px = launcher_w_px;
        let (w, h) = (self.config.width, self.config.height);
        let (cols, rows) = grid_dims(
            w,
            h,
            &self.atlas,
            self.inset_px,
            self.strip_h,
            self.sidebar_w_px,
            self.launcher_w_px,
        );
        if cols != self.grid.cols || rows != self.grid.rows {
            self.grid.resize(cols, rows);
            return Some((cols as u16, rows as u16));
        }
        None
    }

    /// Update the tab-layout mode (Horizontal or Vertical). The App
    /// tick calls this whenever the user's `[tab_layout]` config
    /// changes. Returns `true` iff the value actually changed so the
    /// caller can refresh derived state (strip height, sidebar
    /// width, redraw request).
    fn set_tab_layout(&mut self, layout: crate::config::TabLayout) -> bool {
        if self.tab_layout == layout {
            return false;
        }
        self.tab_layout = layout;
        true
    }

    /// Number of chip rows that fit in the sidebar's visible region
    /// (between the top strip and the bottom of the window).
    /// Used by [`Self::clamp_sidebar_scroll`] to keep the `+` button
    /// reachable.
    fn sidebar_visible_rows(&self) -> f32 {
        let avail = (self.config.height as f32 - self.strip_h).max(0.0);
        (avail / TAB_ROW_H_PX).max(1.0)
    }

    /// Apply a wheel-scroll delta to the vertical sidebar. Positive
    /// `dy` (wheel up) scrolls toward the top (reveals earlier
    /// chips); negative scrolls down. Clamped so the first chip
    /// never moves below the top of the sidebar and the `+` button
    /// stays reachable at the bottom. Returns `true` iff the scroll
    /// actually moved (caller requests a redraw).
    pub fn scroll_sidebar(&mut self, dy: f32, chip_count: usize) -> bool {
        if !matches!(self.tab_layout, crate::config::TabLayout::Vertical) {
            return false;
        }
        // Total rows the sidebar would need: one per chip + one for
        // the `+` button. Visible rows in the sidebar viewport.
        let total = chip_count as f32 + 1.0;
        let visible = self.sidebar_visible_rows();
        // No overflow ⇒ scrolling is a no-op.
        if total <= visible {
            if self.sidebar_scroll_rows != 0.0 {
                self.sidebar_scroll_rows = 0.0;
                return true;
            }
            return false;
        }
        let max_scroll = (total - visible).max(0.0);
        // Wheel up (dy > 0) shows earlier chips ⇒ scroll DECREASES.
        let new_scroll = (self.sidebar_scroll_rows - dy).clamp(0.0, max_scroll);
        if (new_scroll - self.sidebar_scroll_rows).abs() < f32::EPSILON {
            return false;
        }
        self.sidebar_scroll_rows = new_scroll;
        true
    }

    fn render(&mut self) {
        // Headless mode (no surface) ⇒ nothing to render to. The
        // App's logic + state updates still ran; we just skip the
        // GPU pass. Callers can inspect post-tick state via the
        // headless command set without paying for a render.
        let Some(surface) = self.surface.as_ref() else {
            return;
        };
        let frame = match surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                surface.configure(&self.device, &self.config);
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
        // Per-axis inset:
        //   x = base inset + launcher rail width + tab sidebar width.
        //       Launcher rail sits at the left edge of the window; the
        //       tab sidebar (vertical mode) sits to its right. Either
        //       width is 0 when its feature isn't active, so the body
        //       grid still ends up at the right place in every mode.
        //   y = base inset + strip height.
        self.pipeline.write_globals(
            &self.queue,
            [self.config.width as f32, self.config.height as f32],
            [self.atlas.cell_w, self.atlas.cell_h],
            [
                self.inset_px + self.launcher_w_px + self.sidebar_w_px,
                self.inset_px + self.strip_h,
            ],
        );
        // Single-tab: paint the strip in palette().clear_bg so it blends with the
        // surrounding clear color (no visible chrome band — but the grid
        // still starts below the strip so content doesn't kiss the macOS
        // traffic lights). Multi-tab: palette().strip_bg separates the chip strip
        // from the body.
        let strip_color = if self.strip_chips.len() <= 1 {
            palette().clear_bg
        } else {
            palette().strip_bg
        };
        self.strip_pipeline.write_globals(
            &self.queue,
            [self.config.width as f32, self.config.height as f32],
            self.strip_h,
            self.sidebar_w_px,
            strip_color,
            // Border between sidebar column and body — slightly more
            // pronounced than `active_chip_bg` so it actually reads
            // as a separator. Roughly `strip_bg + [0.10, 0.10, 0.10]`.
            // Subtle but visible, no screaming. Same color is used
            // for the launcher rail's right-edge border.
            [0.22, 0.23, 0.26, 1.0],
            self.launcher_w_px,
        );
        let mut instances = CellPipeline::build_instances(&self.grid, &mut self.atlas, &self.queue);
        // Bottom-prompt mode — shift every body cell down so the
        // cursor row lands at the bottom of the visible grid. The
        // cursor's row is recovered from the per-cell cursor
        // attribute bits (set by `apply_frame_to_grid`). When the
        // cursor isn't on screen (e.g. just after a clear, no
        // output yet), the shift is 0 — natural rendering. Strip
        // chips + palette + launcher rail were appended AFTER this
        // call and are NOT shifted; they live in chrome cells.
        if self.bottom_prompt {
            let cols = self.grid.cols;
            let rows = self.grid.rows;
            let cursor_idx = instances.iter().take((cols * rows) as usize).position(|i| {
                i.attrs & (ATTR_CURSOR_BLOCK | ATTR_CURSOR_UNDERLINE | ATTR_CURSOR_BAR) != 0
            });
            if let Some(idx) = cursor_idx {
                let cursor_row = (idx as u32 / cols) as i32;
                let shift = (rows as i32 - 1 - cursor_row).max(0) as f32;
                if shift > 0.0 {
                    // Only touch body cells — strip + palette + rail
                    // chips append later in `render`. Body cells are
                    // the first `cols * rows` instances since
                    // `build_instances` writes row-major.
                    let body_count = (cols * rows) as usize;
                    for inst in instances.iter_mut().take(body_count) {
                        inst.cell_pos[1] += shift;
                    }
                }
            }
        }
        // Text-selection overlay — override the bg color for every
        // cell inside the selection bounds. Cells live in row-major
        // order in `instances` (col 0..cols, then row 0..rows), so
        // we can index directly into the slice. Color chosen so the
        // overlay reads against any theme bg without being too
        // shouty. 2026-06-09: first text-selection shipping.
        if let Some(((min_c, min_r), (max_c, max_r))) = self.selection_bounds {
            const SEL_BG: [f32; 4] = [0.32, 0.36, 0.46, 1.0];
            let cols = self.grid.cols as u16;
            for row in min_r..=max_r {
                for col in min_c..=max_c {
                    let idx = (row as u32 * cols as u32 + col as u32) as usize;
                    if let Some(inst) = instances.get_mut(idx) {
                        inst.bg = SEL_BG;
                    }
                }
            }
        }
        // In-pane find — yellow bg override for every match span,
        // brighter yellow for the "current" match. Applied BEFORE
        // appending chrome instances so chrome stays unaffected.
        if !self.find_highlight_bounds.is_empty() {
            const MATCH_BG: [f32; 4] = [0.55, 0.45, 0.10, 1.0];
            const MATCH_FG: [f32; 4] = [0.99, 0.97, 0.85, 1.0];
            let cols = self.grid.cols as u16;
            let rows = self.grid.rows as u16;
            for (row, col_start, col_end) in self.find_highlight_bounds.iter().copied() {
                if row >= rows {
                    continue;
                }
                let cs = col_start.min(cols.saturating_sub(1));
                let ce = col_end.min(cols.saturating_sub(1));
                for col in cs..=ce {
                    let idx = (row as u32 * cols as u32 + col as u32) as usize;
                    if let Some(inst) = instances.get_mut(idx) {
                        inst.bg = MATCH_BG;
                        inst.fg = MATCH_FG;
                    }
                }
            }
        }
        if let Some((row, col_start, col_end)) = self.find_current_bounds {
            const CURRENT_BG: [f32; 4] = [0.95, 0.78, 0.20, 1.0];
            const CURRENT_FG: [f32; 4] = [0.08, 0.08, 0.10, 1.0];
            let cols = self.grid.cols as u16;
            let rows = self.grid.rows as u16;
            if row < rows {
                let cs = col_start.min(cols.saturating_sub(1));
                let ce = col_end.min(cols.saturating_sub(1));
                for col in cs..=ce {
                    let idx = (row as u32 * cols as u32 + col as u32) as usize;
                    if let Some(inst) = instances.get_mut(idx) {
                        inst.bg = CURRENT_BG;
                        inst.fg = CURRENT_FG;
                    }
                }
            }
        }
        // Append tab-strip label glyphs (rendered through the same cell
        // pipeline via fractional `cell_pos` values — they land in the
        // strip area above the grid).
        instances.extend(self.strip_chip_instances());
        instances.extend(self.strip_palette_chip_instances());
        instances.extend(self.strip_sidebar_toggle_instances());
        instances.extend(self.sidebar_header_instances());
        instances.extend(self.sidebar_drag_handle_instances());
        instances.extend(self.launcher_chip_instances());
        // Find bar last so it paints over body row 0.
        instances.extend(self.find_bar_instances());
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
                            r: palette().clear_bg[0] as f64,
                            g: palette().clear_bg[1] as f64,
                            b: palette().clear_bg[2] as f64,
                            a: palette().clear_bg[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            // Chrome backgrounds first — the cell pipeline draws on
            // top. Five instances total: top strip, tab sidebar +
            // its right-edge border, launcher rail + its right-edge
            // border. The shader collapses sidebar quads when
            // `sidebar_w == 0` (horizontal mode) and launcher quads
            // when `launcher_w == 0` (no launcher_icons configured),
            // so passing 5 unconditionally is cheap.
            pass.set_pipeline(&self.strip_pipeline.pipeline);
            pass.set_bind_group(0, &self.strip_pipeline.bind_group, &[]);
            pass.draw(0..4, 0..5);
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

fn grid_dims(
    w: u32,
    h: u32,
    atlas: &Atlas,
    inset_px: f32,
    strip: f32,
    sidebar_w: f32,
    launcher_w: f32,
) -> (u32, u32) {
    // Launcher rail (when present) eats horizontal width the same
    // way the tab sidebar does — sum them so the body grid shrinks
    // by the total chrome width on the left edge.
    let sidebar_w = sidebar_w + launcher_w;
    // `inset_px == 0` → edge-to-edge horizontally; vertically we
    // reserve `strip` pixels for the tab-strip chrome (caller passes
    // the dynamic strip height: shrinks to the single-tab value when
    // there's only one tab, grows when chips appear). Ceil cols so
    // the rightmost cells reach the window edge (the partial overflow
    // is clipped by the wgpu surface — no clear-bg stripe at the right
    // seam). Floor rows so the LAST cell row gets its full font-row
    // height — any leftover sub-row pixels at the bottom become a
    // small letterbox gutter painted in `palette().clear_bg` by the wgpu clear
    // (industry standard: Apple Terminal, iTerm2, Alacritty, Kitty all
    // do this). The alternative — ceiling rows + clipping the last
    // partial row — leaves a few-pixel sliver of whatever the app drew
    // on the bottom row (status bar / cmdline), which reads as visual
    // noise.
    // `inset_px > 0` → reserve `inset_px` pixels on every side
    // (and tab-strip on top); floor cols/rows so the cells fit inside.
    // `sidebar_w` is non-zero only when `tab_layout = Vertical` — the
    // body grid shifts right by that amount to leave room for the
    // sidebar's tab chips.
    if inset_px <= 0.0 {
        let usable_w = (w as f32 - sidebar_w).max(atlas.cell_w);
        let cols = (usable_w / atlas.cell_w).ceil().max(1.0) as u32;
        let usable_h = (h as f32 - strip).max(atlas.cell_h);
        let rows = (usable_h / atlas.cell_h).floor().max(1.0) as u32;
        return (cols, rows);
    }
    let usable_w = (w as f32 - 2.0 * inset_px - sidebar_w).max(atlas.cell_w);
    let usable_h = (h as f32 - 2.0 * inset_px - strip).max(atlas.cell_h);
    let cols = (usable_w / atlas.cell_w).floor().max(1.0) as u32;
    let rows = (usable_h / atlas.cell_h).floor().max(1.0) as u32;
    (cols, rows)
}

/// Percent-encode a URL query value — minimal RFC3986 form that covers
/// the characters DuckDuckGo / search engines actually care about
/// (space → `+`, then any non-unreserved char → `%XX`). We don't pull
/// in a full `url` crate just for the address bar's search fallback.
pub(crate) fn url_query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Chrome strip on the top row of a Browser pane: `[<] [>] [⟳]` then
/// the URL. Painted on every relayout. Click hit-testing is computed
/// from the same chip layout in [`browser_chip_at`].
///
/// When `chrome.edit` is `Some(s)`, the URL bar shows the edit buffer
/// with a cursor block at `chrome.cursor` instead of the read-only URL.
pub(crate) fn paint_browser_chrome(grid: &mut Grid, url: &str, chrome: &BrowserChrome) {
    if grid.rows == 0 || grid.cols < 16 {
        return;
    }
    // Clear the top row first so previous frames don't bleed through.
    grid.write(
        0,
        0,
        &" ".repeat(grid.cols as usize),
        palette().text_fg,
        palette().clear_bg,
    );
    let chips: &[(u32, &str)] = &[(0, "[<]"), (4, "[>]"), (8, "[⟳]")];
    for (col, label) in chips {
        grid.write(*col, 0, label, palette().text_fg, palette().clear_bg);
    }
    // URL bar starts at column 13 — past the three 3-char chips with
    // single-cell gaps and a one-cell separator.
    let url_col = 13u32;
    if url_col >= grid.cols {
        return;
    }
    let avail = (grid.cols - url_col) as usize;
    if let Some(edit) = chrome.edit.as_ref() {
        // Edit mode — render the edit buffer, then highlight the cursor
        // cell. Truncate to `avail` chars; if the cursor falls past the
        // window, scroll the visible region so it stays on-screen.
        let chars: Vec<char> = edit.chars().collect();
        let cursor = chrome.cursor.min(chars.len());
        let start = if cursor >= avail {
            cursor - avail + 1
        } else {
            0
        };
        let end = (start + avail).min(chars.len());
        let visible: String = chars[start..end].iter().collect();
        grid.write(
            url_col,
            0,
            &visible,
            palette().accent_fg,
            palette().clear_bg,
        );
        // Cursor block — repaint the single cell with swapped colors.
        let cursor_col = url_col + (cursor - start) as u32;
        if cursor_col < grid.cols {
            // Use a vertical-bar so it stays visible on top of any
            // glyph; the underlying char (often a space at EOL) is fine.
            grid.write(cursor_col, 0, "│", palette().accent_fg, palette().clear_bg);
        }
    } else {
        // Read-only — show the URL truncated to fit. Dim a bit so the
        // chip glyphs stand out as the interactive elements.
        let url_str: String = url.chars().take(avail).collect();
        grid.write(url_col, 0, &url_str, palette().dim_fg, palette().clear_bg);
    }
}

/// Hit-test a chrome-strip click. `local_col` / `local_row` are
/// pane-local cell coords. Returns `None` outside the strip (row > 0).
pub(crate) fn browser_chip_at(local_col: u16, local_row: u16) -> Option<BrowserChip> {
    if local_row != 0 {
        return None;
    }
    match local_col {
        0..=2 => Some(BrowserChip::Back),
        4..=6 => Some(BrowserChip::Forward),
        8..=10 => Some(BrowserChip::Reload),
        12.. => Some(BrowserChip::UrlBar),
        _ => None,
    }
}

/// Placeholder grid for a Browser pane that has no live `wry::WebView`
/// yet (Phase 1 scaffolding). Centered "browser" title + the target
/// URL + a "(webview integration pending)" status line. Phase 2 will
/// stop painting this once the wgpu surface is overlaid by the
/// webview.
pub(crate) fn paint_browser_placeholder(grid: &mut Grid, url: &str) {
    grid.clear();
    if grid.rows < 4 || grid.cols < 16 {
        return;
    }
    let row = grid.rows / 2;
    let title = "browser";
    let title_col = (grid.cols.saturating_sub(title.chars().count() as u32)) / 2;
    grid.write(
        title_col,
        row.saturating_sub(2),
        title,
        palette().accent_fg,
        palette().clear_bg,
    );

    let url_str = url.chars().take(grid.cols as usize).collect::<String>();
    let url_col = (grid.cols.saturating_sub(url_str.chars().count() as u32)) / 2;
    grid.write(
        url_col,
        row,
        &url_str,
        palette().text_fg,
        palette().clear_bg,
    );

    let hint = "(webview integration pending)";
    if hint.chars().count() < grid.cols as usize {
        let col = (grid.cols.saturating_sub(hint.chars().count() as u32)) / 2;
        grid.write(col, row + 2, hint, palette().dim_fg, palette().clear_bg);
    }
}

fn paint_idle(grid: &mut Grid, state: ConnState, socket_path: &std::path::Path) {
    grid.clear();
    if grid.rows < 3 || grid.cols < 10 {
        return;
    }
    let title = "tmnl";
    let row = grid.rows / 2;
    let title_col = (grid.cols.saturating_sub(title.chars().count() as u32)) / 2;
    grid.write(
        title_col,
        row.saturating_sub(2),
        title,
        palette().accent_fg,
        palette().clear_bg,
    );

    let (status, color) = match state {
        ConnState::Waiting => ("waiting for client", palette().dim_fg),
        ConnState::Connected => ("client connected — awaiting first frame", palette().text_fg),
        ConnState::Streaming => return,
    };
    let status_col = (grid.cols.saturating_sub(status.chars().count() as u32)) / 2;
    grid.write(status_col, row, status, color, palette().clear_bg);

    let hint = format!("socket: {}", socket_path.display());
    if hint.chars().count() < grid.cols as usize {
        let col = (grid.cols.saturating_sub(hint.chars().count() as u32)) / 2;
        grid.write(col, row + 2, &hint, palette().dim_fg, palette().clear_bg);
    }
}

/// Same as [`pack_mods`] but swaps Super (Mac Cmd) for Ctrl — used when
/// translating Mac-style editing chords (⌘Z / ⌘C / etc) into their Ctrl
/// equivalents on the wire so the hosted Linux/cross-platform app sees
/// what it expects.
pub(crate) fn pack_mods_cmd_to_ctrl(m: ModifiersState) -> u8 {
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
        // X-axis inset includes the vertical-tab sidebar so clicks in
        // the sidebar region don't translate to cell coords (the
        // grid starts to the RIGHT of the sidebar). Y-axis inset
        // unchanged — strip is still top-only.
        let inset_x = self.inset_px as f64 + self.sidebar_w_px as f64;
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

pub(crate) fn translate_key(k: &Key, mods: ModifiersState) -> Option<KeyCode> {
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
    arg_f32(argv, "--inset-shell")
        .or_else(|| arg_f32(argv, "--inset"))
        .or_else(|| env_f32("TMNL_INSET"))
        .unwrap_or(cfg.inset)
        .max(0.0)
}

/// Resolve the pixel inset for native-mode TUIs (mnml / mixr) +
/// alt-screen shell children. CLI `--inset-native` wins, then
/// `$TMNL_INSET_NATIVE`, then `cfg.inset_native`. 0 ⇒ historic
/// edge-to-edge.
fn resolve_inset_native(argv: &[String], cfg: &Config) -> f32 {
    arg_f32(argv, "--inset-native")
        .or_else(|| env_f32("TMNL_INSET_NATIVE"))
        .unwrap_or(cfg.inset_native)
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
            fg: palette().dim_fg,
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
                fg: palette().dim_fg,
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

/// Layer a "thinking" indicator onto a pane's tab label without ever
/// producing two decorations side-by-side. Three cases:
///   * `* <title>` + thinking → swap leading `*` for the live
///     spinner glyph (Claude Code's OSC title format).
///   * `bare title` + thinking → prepend the glyph + space.
///   * not thinking → return the name pristine.
///
/// NEVER appends anything on the right of the title — the user's
/// ask was that the indicator stay on the LEFT only.
fn apply_thinking_glyph(name: &str, glyph: Option<char>) -> String {
    match (glyph, name.strip_prefix("* ")) {
        (Some(g), Some(rest)) => format!("{g} {rest}"),
        (Some(g), None) => format!("{g} {name}"),
        (None, _) => name.to_string(),
    }
}

/// Resolve a pane's strip label — the stable name (OSC title /
/// foreground process / shell), with Claude Code's spinner glyph
/// layered on by `apply_thinking_glyph` when a session is thinking.
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
            // process → shell name. Claude Code's OSC title format is
            // `* <title>` — a leading asterisk + space — so we layer
            // the spinner animation on by REPLACING the leading `*`
            // with the live cycling glyph (`✽ ✺ ◍ …`) instead of
            // appending anything on the right.
            //
            // Old behavior appended ` <glyph>` to the right of the
            // name; the user's complaint was that this read as a
            // second asterisk decoration alongside the existing
            // leading one — and worse, it stayed put even after the
            // session stopped thinking (cached `sticky` window).
            // 2026-06-08 fix: never append on the right.
            let name = osc.or(fg).unwrap_or_else(|| {
                session
                    .as_ref()
                    .map(|s| s.shell_name().to_string())
                    .unwrap_or_else(|| "shell".to_string())
            });
            let glyph = sticky.as_deref().and_then(|s| s.chars().next());
            apply_thinking_glyph(&name, glyph)
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
        PaneKind::Browser { url, .. } => {
            // Strip the scheme + path; show the bare host so the chip
            // stays scannable. `duckduckgo.com` is more useful than
            // `https://duckduckgo.com/?q=foo` on a tiny tab strip.
            url.split("://")
                .nth(1)
                .unwrap_or(url)
                .split('/')
                .next()
                .unwrap_or(url)
                .to_string()
        }
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
fn tick_secondary_pane(pane: &mut Pane, visible: bool) -> Vec<Vec<tmnl_protocol::CommandInfo>> {
    let mut collected: Vec<Vec<tmnl_protocol::CommandInfo>> = Vec::new();
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
                    // Same rationale for RunHostCommand: only the
                    // focused pane's client should be firing host
                    // commands at us.
                    ServerEvent::RunHostCommand(_) => {}
                    // ClientCommands from non-focused Native panes:
                    // collect them so the App-level tick can tag with
                    // (tab, pane) and route remote-invokes back to
                    // the source pane (v2 multi-source aggregation).
                    ServerEvent::ClientCommands(items) => {
                        collected.push(items);
                    }
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
        PaneKind::Browser { .. } => {
            // No-op in Phase 1 — the placeholder grid was painted at
            // creation by `app::paint_browser_placeholder`. Phase 2
            // mounts a wry WebView; this branch will become responsible
            // for repositioning/hiding the webview on tab show/hide.
        }
    }
    collected
}

/// When tmnl.app is double-clicked from /Applications, macOS launches
/// it with the bare LaunchServices environment — no `~/.zshrc` /
/// `~/.bash_profile` exports, so `PATH` is the system default and any
/// user-set vars (`BITBUCKET_ACCESS_TOKEN`, `OPENAI_API_KEY`, etc.)
/// aren't there. Children we spawn (mnml, mixr, shells) inherit this
/// stripped env, breaking integrations that rely on those exports
/// + tools installed outside `/usr/bin`.
///
/// Detection: stdin isn't a tty (parent is `launchd`, not a shell).
/// This catches both:
///   * `current_exe()` inside `.app/Contents/MacOS/` (the standard
///     bundle case), and
///   * `current_exe()` outside any bundle when a launcher script (like
///     `tmnl-nightly-launcher`) `exec`-ed into the dev binary — the
///     bundle identity survives in `Info.plist` but `current_exe()`
///     now points at the dev path, so a pure-path check missed this
///     case.
///
/// Without the fix, the `tmnl-nightly` flow saw a bare PATH and
/// `Command::new("mnml")` from `Launcher::spawn` would fail with
/// `Os(NotFound)` because `~/.cargo/bin` wasn't on PATH — the
/// auto-promote path then never got a working native tab, just
/// `(no client)` forever.
///
/// Run the user's login shell with `-l -c env` to dump its
/// environment and re-export each var onto our own process so
/// subsequent spawns inherit the full shell env.
///
/// No-op when launched from a shell — stdin is a tty there, PATH
/// already has the user's customizations.
/// Override `mouseDownCanMoveWindow` on winit's NSView so clicks in
/// the title-bar region reach our event loop instead of being
/// consumed by macOS for window-dragging.
///
/// With `titlebar_transparent + fullsize_content_view`, our wgpu
/// surface paints through the OS title region — but macOS still
/// treats the top-left half as a drag handle. Without this
/// override, the sidebar toggle button near the traffic lights is
/// visible but un-clickable.
///
/// Same pattern Warp's `WarpHostView` uses (returning `NO` from
/// `mouseDownCanMoveWindow` — see warpdotdev/warp's host_view.m).
/// We runtime-swizzle the existing NSView class via
/// `class_replaceMethod` so all instances return NO.
///
/// Side effect: window can no longer be dragged by clicking
/// anywhere in the chrome region. Standard Cmd-drag-anywhere
/// still works.
#[cfg(target_os = "macos")]
pub fn disable_window_drag(window: &winit::window::Window) {
    use objc2::ffi::class_replaceMethod;
    use objc2::runtime::{AnyObject, Sel};
    use objc2::sel;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    unsafe extern "C-unwind" fn mouse_down_can_move_window(_this: &AnyObject, _sel: Sel) -> bool {
        false
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let ns_view_ptr: *mut AnyObject = appkit.ns_view.as_ptr() as *mut _;
    if ns_view_ptr.is_null() {
        return;
    }
    unsafe {
        let view = &*ns_view_ptr;
        let cls = view.class();
        let sel = sel!(mouseDownCanMoveWindow);
        let types = c"B@:".as_ptr();
        let imp_typed: unsafe extern "C-unwind" fn(&AnyObject, Sel) -> bool =
            mouse_down_can_move_window;
        let imp: unsafe extern "C-unwind" fn() = std::mem::transmute(imp_typed);
        let cls_ptr: *const objc2::runtime::AnyClass = cls;
        class_replaceMethod(cls_ptr as *mut _, sel, imp, types);
    }
}

fn load_login_shell_env_if_needed() {
    use std::ffi::OsString;
    use std::io::IsTerminal;
    use std::process::Command;

    if std::io::stdin().is_terminal() {
        return;
    }
    let shell: OsString = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    // `-l` = login shell, sources zprofile/bash_profile/etc.
    // `-i` would also work but is slower (sources rc files too).
    let Ok(output) = Command::new(&shell).arg("-l").arg("-c").arg("env").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        // Skip a handful of vars where overwriting our own values would
        // confuse later code (`PWD`, `OLDPWD` reflect the shell's cwd,
        // not ours; `_` is the shell's "previous command" sentinel).
        if matches!(k, "PWD" | "OLDPWD" | "_" | "SHLVL") {
            continue;
        }
        // SAFETY: this runs at the top of main() before any thread
        // spawn or subprocess, so `set_var` is single-threaded.
        unsafe { std::env::set_var(k, v) };
    }
}

fn main() {
    env_logger::init();
    // Backfill the login-shell env when launched from /Applications —
    // PATH + user-set tokens otherwise aren't available to the
    // children we spawn. See doc comment on the function.
    load_login_shell_env_if_needed();
    // Load the chrome palette. Tries mnml's installed theme first
    // (so tmnl + mnml visually blend when launched side-by-side),
    // falls back to defaults eyedropped from mnml's onedark. Idempotent.
    theme::init();
    // Background "is there a newer release?" probe. Logs to stderr
    // when a newer tag than CARGO_PKG_VERSION is found. Fire-and-forget;
    // the returned handle is unused on the main thread here (the
    // welcome overlay reads from `WelcomeState::update_notice` via
    // a separate path).
    let _update_check = update_check::UpdateCheck::spawn();
    // First-launch family offer — prints stderr hints once per
    // missing sibling, then writes ~/.config/tmnl/.family-offer-shown
    // to silence subsequent launches.
    family_offer::maybe_offer_at_launch();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Headless mode — no window, scripted stdin, text grid dumps (see
    // `src/headless.rs`). Branches out before any winit / wgpu / AppKit
    // setup so it runs fine with no display.
    //
    // Two flavors:
    //   `--headless`         — single ShellSession harness (original)
    //   `--headless --app`   — full App with multi-tab driving (Phase B/2b)
    if argv.iter().any(|a| a == "--headless") {
        if argv.iter().any(|a| a == "--app") {
            headless::run_app();
        } else {
            headless::run();
        }
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
    // Inset selection:
    //   * Native mode (mnml / mixr) → `inset_native` so a TUI with
    //     its own borders doesn't hug the macOS window chrome /
    //     traffic-light buttons. Was hardcoded 0.0; users complained
    //     mixr's outer panel borders ran into the window edge.
    //   * Shell mode → `inset` (the apple-terminal-style prompt
    //     padding). The alt-screen detector swaps to `inset_native`
    //     once a full-screen TUI takes over the shell.
    let inset_px = if editor_mode {
        resolve_inset_native(&argv, &cfg)
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
        grid: Grid::new(80, 24, palette().clear_bg),
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
        prev_attention_count: 0,
        app_menu: None,
        settings: None,
        welcome: None,
        editor_template,
        native_tab_nonce: 1,
        dragging_tab: None,
        renaming_tab: None,
        dragging_divider: None,
        sidebar_w_override: None,
        dragging_sidebar: false,
        sidebar_drag_press_x: None,
        sidebar_drag_prev_override: None,
        text_selection: None,
        dragging_selection: false,
        tab_search: None,
        find: None,
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
    // Respect the user's `show_welcome` config — when off, skip the
    // overlay entirely on startup (their explicit opt-out via the
    // welcome's `D` action or the settings UI).
    if !editor_mode && app.cfg.show_welcome {
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

    /// Lock the "never two leading decorations side-by-side" invariant
    /// for tab labels. User-reported concern after the spinner-glyph
    /// rework — make sure no future refactor accidentally produces
    /// `* ✽ <title>` or `✽ * <title>`.
    #[test]
    fn apply_thinking_glyph_never_doubles_leading_decoration() {
        // Claude-style title + thinking: swap the `*` for the glyph.
        assert_eq!(
            apply_thinking_glyph("* Foo bar", Some('✽')),
            "✽ Foo bar".to_string()
        );
        // Bare title + thinking: prepend.
        assert_eq!(
            apply_thinking_glyph("Foo bar", Some('✽')),
            "✽ Foo bar".to_string()
        );
        // Not thinking: pristine (with `*` prefix).
        assert_eq!(
            apply_thinking_glyph("* Foo bar", None),
            "* Foo bar".to_string()
        );
        // Not thinking: pristine (bare).
        assert_eq!(apply_thinking_glyph("Foo bar", None), "Foo bar".to_string());
        // Edge case: spinner glyph happens to be `*`. Title has `* `
        // prefix → swap. Result is still ONE `*`, not two side-by-side.
        assert_eq!(
            apply_thinking_glyph("* Foo", Some('*')),
            "* Foo".to_string()
        );
        // Edge case: spinner glyph `*`, bare title → prepend. Single
        // `*`, no doubling.
        assert_eq!(apply_thinking_glyph("Foo", Some('*')), "* Foo".to_string());
    }

    #[test]
    fn browser_chip_at_maps_cells_to_chips() {
        use BrowserChip::*;
        // Row 0 — the chrome strip.
        assert_eq!(browser_chip_at(0, 0), Some(Back));
        assert_eq!(browser_chip_at(2, 0), Some(Back));
        assert_eq!(browser_chip_at(3, 0), None); // gap
        assert_eq!(browser_chip_at(4, 0), Some(Forward));
        assert_eq!(browser_chip_at(6, 0), Some(Forward));
        assert_eq!(browser_chip_at(8, 0), Some(Reload));
        assert_eq!(browser_chip_at(10, 0), Some(Reload));
        assert_eq!(browser_chip_at(11, 0), None); // gap
        assert_eq!(browser_chip_at(12, 0), Some(UrlBar));
        assert_eq!(browser_chip_at(500, 0), Some(UrlBar));
        // Any non-zero row → no chip (the WebView's territory).
        assert_eq!(browser_chip_at(0, 1), None);
        assert_eq!(browser_chip_at(20, 5), None);
    }

    #[test]
    fn url_query_encode_handles_unreserved_and_space() {
        assert_eq!(url_query_encode("abc"), "abc");
        assert_eq!(url_query_encode("a b c"), "a+b+c");
        assert_eq!(url_query_encode("foo?bar=baz"), "foo%3Fbar%3Dbaz");
        assert_eq!(url_query_encode("a-_.~"), "a-_.~");
        assert_eq!(url_query_encode(""), "");
    }

    /// Read row `r` of `g` as a `String` for assertion-friendly inspection.
    fn row_text(g: &Grid, r: u32) -> String {
        (0..g.cols)
            .map(|c| g.cells[(r * g.cols + c) as usize].ch)
            .collect()
    }

    #[test]
    fn paint_browser_chrome_renders_chips_and_url() {
        let mut g = Grid::new(40, 4, palette().clear_bg);
        let chrome = BrowserChrome::default();
        paint_browser_chrome(&mut g, "https://example.com", &chrome);
        let row0 = row_text(&g, 0);
        assert!(row0.starts_with("[<] [>] [⟳]"), "got {row0:?}");
        assert!(row0.contains("https://example.com"), "got {row0:?}");
    }

    #[test]
    fn paint_browser_chrome_shows_edit_buffer_and_cursor() {
        let mut g = Grid::new(40, 4, palette().clear_bg);
        let chrome = BrowserChrome {
            edit: Some("typing".to_string()),
            cursor: 6,
        };
        paint_browser_chrome(&mut g, "https://example.com", &chrome);
        let row0 = row_text(&g, 0);
        // URL bar shows the edit buffer, not the read-only URL.
        assert!(row0.contains("typing"), "got {row0:?}");
        assert!(!row0.contains("example.com"), "got {row0:?}");
        // Cursor block at the end (after `typing`).
        assert!(row0.contains('│'), "expected cursor block in {row0:?}");
    }

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
        let mut grid = Grid::new(cols, rows, palette().clear_bg);
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
        let mut window = Grid::new(10, 4, palette().clear_bg);
        composite(&tab, &mut window);
        assert!(window.cells.iter().all(|c| c.ch == 'A'));
    }

    #[test]
    fn composite_tiles_two_panes_with_a_divider() {
        // 21 wide ⇒ 20 usable, 10/10 either side of a 1-col divider.
        let mut window = Grid::new(21, 4, palette().clear_bg);
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
        let mut window = Grid::new(21, 4, palette().clear_bg);
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
        let mut window = Grid::new(21, 9, palette().clear_bg);
        composite(&tab, &mut window);
        // V divider at column 10, H divider at row 4 — junction (10, 4).
        let junction = &window.cells[4 * 21 + 10];
        assert_eq!(junction.ch, '├', "junction draws a T-glyph");
        // Every divider cell is the same quiet chrome colour.
        assert_eq!(junction.fg, palette().dim_fg);
        assert_eq!(window.cells[10].fg, palette().dim_fg);
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
        let mut window = Grid::new(21, 4, palette().clear_bg);
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
        let mut g = Grid::new(6, 1, palette().clear_bg);
        draw_ghost(&mut g, 2, "hello");
        // "hell" fits at indices 2..=5; the final "o" is past the grid.
        assert_eq!(g.cells[2].ch, 'h');
        assert_eq!(g.cells[5].ch, 'l');
        assert_eq!(g.cells[2].fg, palette().dim_fg);
        assert!(!g.cells.iter().any(|c| c.ch == 'o'));
    }

    #[test]
    fn apply_frame_to_grid_writes_runs_and_the_cursor() {
        use super::protocol::{DiffRun, Frame, WireCell};
        let mut g = Grid::new(4, 2, palette().clear_bg);
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
        let mut g = Grid::new(4, 2, palette().clear_bg);
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
