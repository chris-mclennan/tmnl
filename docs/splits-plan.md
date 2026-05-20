# tmnl splits / panes — implementation plan

The single biggest "why I can't switch" gap (FEATURES.md). Today a tmnl
window has `tabs: Vec<Tab>` and each `Tab` hosts exactly one `Mode`
(one shell *or* one native client). Splits = each tab becomes a **tree
of panes**, several grids drawn side by side.

This is a real architectural change to the tab/grid/render/input core.
It is its own focused session — do not bolt it on. The plan below is
phased so every phase compiles, passes `/check`, and is shippable.

## Reference

mnml already solved this exact shape — study it first:
- `~/Projects/mnml/src/layout.rs` — the `Layout` split tree.
- `~/Projects/mnml/src/pane.rs` — the leaf payload enum + `PaneId`.
- `~/Projects/mnml/src/ui/mod.rs::draw` — recursive area-splitting render.
- mnml's `view.split_*` / `view.focus_*` commands — the management verbs.

tmnl's job is to port that model onto its wgpu grid renderer.

## Target data model

```rust
/// The leaf payload — what `Tab.mode` holds today, plus the pane's own
/// grid (grid currently lives on `Gpu`; it moves here).
struct Pane {
    kind: PaneKind,              // the current `Mode`, renamed
    grid: Grid,                  // sized to this pane's rect
    last_cursor: Option<usize>,
    label: String,
    attention: bool,
    // …spinner/OSC-133 cache, all the per-Mode bookkeeping that's on Tab now
}
enum PaneKind { Shell { … }, Native { … } }   // == today's `Mode`

type PaneId = usize;             // index into Tab.panes (or a slotmap)

/// A binary split tree. Mirrors mnml's `Layout`.
enum Layout {
    Leaf(PaneId),
    Split { dir: SplitDir, ratio: f32, first: Box<Layout>, second: Box<Layout> },
}
enum SplitDir { Horizontal, Vertical }

struct Tab {
    layout: Layout,
    panes: Vec<Pane>,            // leaves; Layout indexes into this
    focused: PaneId,
    label: String,               // tab-strip label = focused pane's label
}
```

`grid_snapshot` on `Tab` **goes away** — every pane owns its grid
permanently, so background tabs/panes keep state for free. The
`gpu.grid` ↔ `grid_snapshot` swap on tab-switch is deleted.

## Rendering — the key simplification

Keep **one window-sized grid** for the GPU. Do NOT make the wgpu cell
pipeline draw N grids. Instead add a **compositor** step:

1. Recursively split the window `Rect` per `Tab.layout` → one sub-rect
   per leaf (same recursion as mnml's `draw`).
2. For each leaf, blit the pane's `Grid` cells into the window grid at
   the sub-rect's `(x, y)` offset, clipped to the rect.
3. Paint 1-cell divider lines (`│` / `─`) between splits; tint the
   focused pane's divider/border so focus is visible.
4. Hand the composited window grid to the existing GPU pipeline — it is
   unchanged.

So the renderer change is one new function, `composite(tab, window_rect,
&mut window_grid)`. `render()` calls it instead of drawing `gpu.grid`
directly.

## Input routing

- **Keys** → `tab.panes[tab.focused]`'s session/conn (the existing
  per-Mode write path, just indexed by `focused`).
- **Mouse** → hit-test the leaf rects; the pane under the cursor gets
  the event, and a click also sets `tab.focused`.
- **Split-management chords** — pick a set and document them. Suggested
  (tmnl already remaps ⌘→⌃ for Native tabs, so keep these tmnl-level,
  intercepted before forwarding):
  - `⌘D` split right (Vertical), `⌘⇧D` split down (Horizontal)
  - `⌘⌥←/→/↑/↓` focus the pane in that direction
  - `⌘⇧W` close the focused pane (collapse its split; last pane closes
    the tab — reconcile with today's `⌘W`)
- New pane on split: default to a Shell pane (cheap); a Native pane can
  be a follow-up.

## Resize

On window resize *or* a split-ratio change: recompute every leaf rect,
then resize each pane to its rect — Shell panes via the pty `set_size`,
Native panes by sending the client a resize/`Hello`-shaped message and
re-`set_size`'ing the grid. This already happens once per tab; splits
just do it per leaf.

## Native-mode specifics

Each Native pane needs its own `Server` (UDS socket) + `Launcher`. tmnl
already mints per-tab sockets (`native_tab_socket_path` + a nonce);
extend that to per-pane. Per-pane `client_title` / `attention` /
spinner state — already per-Mode, just more instances.

## Phasing

Each phase compiles + passes `/check`:

1. **Types, one leaf.** Introduce `Pane` / `PaneKind` / `Layout`;
   `Tab` carries `layout` + `panes` + `focused`, always exactly one
   leaf. Move `Grid` from `Gpu` onto `Pane`. Compositor handles the
   1-leaf case (blit the single pane full-window). Delete
   `grid_snapshot`. Behaviorally identical to today — pure refactor.
2. **Compositor for N leaves.** Recursive rect-split + multi-pane
   blit + dividers + focus tint. `render()` composites.
3. **Split / close / focus.** The keybindings + mouse focus +
   collapse-on-close. New panes spawn a shell.
4. **Resize.** Per-pane resize on window-resize + ratio change;
   drag-to-resize dividers (optional polish).

## Gotchas

- `apply_frame_to_grid` / `ShellSession::apply_to_grid` now target a
  *pane's* grid, not the shared `gpu.grid`.
- Only the focused pane draws a cursor.
- The headless harness (`src/headless.rs`) is single-grid — leave it
  single-pane for now, or add a `focus <n>` command later.
- Background *tabs* still need their panes' grids live enough that
  switching back is instant — since panes own grids permanently, this
  is automatic; just don't drop background tabs' panes.
- Verify each phase with `tmnl --headless` (now has `expect`) and a
  visual GPU-window check — splits are inherently visual.
