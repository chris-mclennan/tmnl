# Mouse-only chrome bug-hunt — 2026-06-09

## Coverage prelude

Drove a fresh `./target/release/tmnl --headless --app` (120×36 cell viewport ≈
960×636 physical px) entirely via `click` / `hover` / `wheel` / `type`
commands, with `state-json` snapshots between each step. Vertical-tab mode
(today's default) was the primary subject; flipped briefly into horizontal
to verify the toggle's round-trip.

What I tested:
1. Sidebar toggle button at the divider (vertical ↔ horizontal flip).
2. Sidebar search bar (toggle on, typing, toggle off, persistence
   across layout flips).
3. Sidebar `+` button (grows `tabs`).
4. Tab chip clicks (chip body → activate).
5. Tab chip close `×` hit-rect placement.
6. Top-strip palette cluster: back / fwd / search-chip / chevron.
7. Wheel-scroll over the sidebar with 5-15 tabs.
8. Sidebar divider drag grab-zone position (source-level reasoning;
   synthetic_click can't drive press→move→release in one event).
9. Tab drag-reorder — not exercisable via `synthetic_click` (would need a
   `press` / `move` / `release` triplet).
10. Empty chrome click + edge clicks (x=0, x=window_w-1).

What I did NOT test:
- Welcome / settings / palette overlays (no chrome-discoverable affordance
  for mouse alone; they're keyboard-only).
- Renaming a tab (right-click → start_rename — not part of the brief).
- Native (mnml-blit) panes specifically; all panes were Shell.
- Real cursor-shape / hover-tooltip visuals (state-json doesn't expose
  them).
- Live-mode (windowed) refresh timing — reasoned from source.

## Findings

### 1. SEV-1 — Sidebar disappears after toggle round-trip until any other event fires

**What I did:** start headless in vertical mode. `click 180 20` → toggles
to horizontal. `click 180 20` again → toggles back to vertical.

**What I expected:** sidebar re-appears with its full computed width.

**What happened:** `state-json` reports `tab_layout:"vertical"` AND
`sidebar_w_px:0`. Body grid reflows to no-sidebar width (cols jumps from
79 to 102). The next mouse-move event (any hover) calls
`refresh_strip_layout()` and the sidebar pops back in (sidebar_w_px → 206,
cols → 79).

Repro stream (input → state):
```
init:                tab_layout=vertical, sidebar_w_px=206
click 180 20:        tab_layout=horizontal, sidebar_w_px=206 (stale, harmless)
click 180 20:        tab_layout=vertical, sidebar_w_px=0   ← BUG
hover anywhere:      tab_layout=vertical, sidebar_w_px=206 (recovered)
```

**Root cause:** `App::handle_mouse_input`'s toggle branch
(`src/app.rs:3389`) flips `cfg.tab_layout`, saves config, calls
`window.request_redraw()` — but never calls `refresh_strip_layout()`.
Live mode auto-recovers because the next `tick()` (which runs every frame)
calls it at `src/app.rs:1636`. Headless reveals the gap. In live mode this
manifests as a one-frame flicker (no sidebar painted for ~16ms after the
click); still user-visible on a CRT-flicker scale.

**Severity:** SEV-1 in headless / smoke-test contexts (sidebar literally
gone). SEV-3 visual in live mode (one-frame transient between click and
next tick). Easy fix: add `self.refresh_strip_layout()` after the
`self.cfg.save()` call in the toggle handler.

### 2. SEV-2 — Sidebar divider drag grab-zone is off by `inset_px`

**What I did:** read the source (the synthetic_click harness can't drive a
press→move→release sequence, so this is source-level reasoning, not a
runtime repro).

**What I expected:** the 4px grab zone armed on
`mouseDown` should sit at the actual right edge of the sidebar — pixel
`inset_px + sidebar_w_px`.

**What happened:** at `src/app.rs:3281`, the press handler computes:
```rust
let border_x = gpu.sidebar_w_px as f64;
let grab = 4.0_f64;
…
if self.cursor_px.0 >= border_x - grab && self.cursor_px.0 <= border_x + grab
```
That's the WIDTH, not the absolute x. The actual divider is painted at
`inset_px + sidebar_w_px` (cf. `Gpu::sidebar_header_instances`
at `src/main.rs:2128`: `sidebar_right_x_px = self.inset_px + self.sidebar_w_px`).
With inset=20 and sidebar=206, the divider is at x=226 but the grab zone
is [202, 210]. The user clicks on the visible seam (~226) and gets
nothing; clicking blindly 20px LEFT of the seam initiates the drag.

**Severity:** SEV-2. Drag-resize is reachable but only by accident.
Trivial fix: `let border_x = (gpu.inset_px + gpu.sidebar_w_px) as f64;`.

### 3. SEV-1 — Clicking the body of the empty-label first tab CLOSES it instead of activating

**What I did:** vertical mode, 3 tabs (`tab.new` × 2). The first tab has
no label (initial shell tab with `label = ""`). `click 50 120` —
visually inside the chip body (chip 0 occupies x≈28..92, y≈108..140).

**What I expected:** tab 0 becomes active.

**What happened:** `tabs` goes from 3 → 2 (tab 0 gone). The close-`×`
hit rect for the empty-label chip lives at x≈46..56 (one cell, ~10px),
which lands in what the user reads as the chip's body — there's no label
glyph between the left pad and the close × in a zero-char chip.

Pixel probe of chip-0's body / close zones (cell_w ≈ 10):
```
x=30..45:  activates (chip body left of close)
x=46..55:  CLOSES tab        ← bug surface
x=56..60:  activates (chip body right of close, before right pad)
```

The hit-rect math is correct (`close_x_px = chip_x0 + 2*cell_w` for an
empty label) — the rendering and hit zone agree. The bug is the LAYOUT:
the close `×` sits in the visual middle of a 4-cell chip when the label
is empty.

**Severity:** SEV-1. First chip is always the first shell session, which
always has an empty label until the title is set. A first-time user
clicking what they think is "tab 1" gets it closed. The same bug class
exists for any chip with very short labels (1-2 chars) where the close
zone can still appear inside the chip's optical center.

**Fix sketch:** require a minimum chip body width (pad chips with empty
or 1-char labels to 4+ chars worth of pre-close cells), OR place the
close button only at the very right edge with an explicit right-side
hit zone independent of label length, OR show no close-× on the focused
chip and require a hover-reveal pattern.

### 4. SEV-2 — Sidebar header has a dead-pixel band between search input and `+` button

**What I did:** vertical mode, 1 tab (sidebar_w_px=206). Pixel-probe clicks
at y=90 (header row) across x=160..210.

```
x=160..170:  toggles tab_search   (search input zone)
x=180:       NO-OP                ← dead zone
x=190..200:  spawns tab           (+ button zone)
x=205..:     NO-OP                (chrome margin, expected)
```

**What I expected:** continuous coverage — every click in the header row
should either land on search or `+`, with no gap.

**What happened:** ~12-20px band (the `gap_cells = 1.0` constant +
`right_pad` margin) between the search input's right edge and the `+`
button's left edge swallows clicks silently. Mouse-only users sliding
between the two targets get phantom no-ops.

**Severity:** SEV-2 / SEV-3. Functional (no data loss) but reads as
"the button doesn't work when I click between the icons." Easy fix:
extend either rect by half the gap so they meet in the middle.

### 5. SEV-2 — Clicking middle/right of an active "zsh" chip (past the close ×) falls into a dead zone

**What I did:** vertical mode, 3 tabs. Click in tab-1 chip's row at
various x — the chip is "zsh" (3-char label), so total chip width is
~7 cells × 11.6px ≈ 81px starting at x=28; chip ends ~x=109. Sidebar
is 206px wide.

```
x=28..107:  activates tab 1
x=108..119: closes tab 1 (close × hit zone)
x=120..226: NO-OP  ← dead band  (rest of the sidebar row, ~100px wide)
```

**What I expected:** clicking ANYWHERE in the visual row band (out to the
sidebar's right edge at x=226) activates the tab — that's what users
expect from sidebar tabs in VS Code / Warp / browsers.

**What happened:** only the literal chip-cell width is hot. The remaining
~100px of the sidebar row is unclickable.

**Severity:** SEV-2 UX. The active chip's accent background only paints
its own cells, so there's a visual cue that the right of the row is
"empty space" — but other sidebar UIs (and arguably "tabs" mental model)
suggest the whole row should be clickable. Fix: extend `strip_chip_rects`
to the right edge of the sidebar for vertical mode, OR widen the
active-chip background paint to match.

### 6. SEV-3 — Top-strip palette dropdown chevron sends `Ctrl+R` to focused pane (no visible effect on Shell panes)

**What I did:** vertical mode, 1 shell tab. `click 665 20` — the dropdown
chevron at the right edge of the palette chip.

**What I expected:** something visible — a recent-files menu opens, or a
toast, or it's a no-op.

**What happened:** state-json shows no change. Looking at
`src/app.rs:3373`, the dropdown forwards `Ctrl+R` to the focused pane.
For a Shell pane that maps to a shell history search if the user's shell
implements it (`bck-i-search` in zsh), but no visible chrome reaction.
A mouse-only user clicks the chevron, gets nothing visible, and has no
idea whether the click "took."

**Severity:** SEV-3 discoverability. Mouse-only users can't tell the
dropdown is conditional on pane type. Fix: either disable / hide the
chevron when no native pane has a defined recent-files menu, or toast a
helpful "no recent items" / forward to a chrome-side menu.

### 7. SEV-2 — Sidebar toggle persists `tab_layout` to disk on every click

**What I did:** toggled the sidebar a few times during the test session.

**What I expected:** the toggle could either (a) save eagerly, or
(b) save on quit / explicitly. Whatever it does should be obvious.

**What happened:** every click of the toggle calls `self.cfg.save()`
(`src/app.rs:3399`), persisting `tab_layout = "horizontal"` (or back) to
`~/.config/tmnl/config.toml` immediately. During the test session this
clobbered my user-mode default, then later restored it on the inverse
toggle. Two implications:

- The headless test harness can't run a layout-toggle test without
  also writing to the user's persistent config (a sandbox / test-config
  override would isolate this).
- If tmnl crashes between the first and second toggle, the user is
  stuck in horizontal mode next launch — minor but real.

**Severity:** SEV-2 (test isolation), SEV-3 (user-impact). Lower-priority
than the missing `refresh_strip_layout()` issue (#1) — but worth
considering whether tab_layout deserves auto-persistence or a "save on
quit" pattern.

### 8. SEV-3 — Wheel scroll over sidebar can't be observed via state-json

**What I did:** with 7 then 15 tabs, fired `wheel ±3 100 300` over the
sidebar.

**What I expected:** confirm scroll position via state-json.

**What happened:** state-json doesn't expose `sidebar_scroll_rows`. The
scroll logic in `Gpu::scroll_sidebar` looks correct, but I can't verify
end-to-end behavior in headless. Not a bug — a TEST-HARNESS gap. Adding
`sidebar_scroll_rows` and `tab_layout`-effective-sidebar (clipped /
hidden / overflowing) to the `state-json` payload would let mouse-only
test agents verify wheel paths.

### 9. SEV-3 — `sidebar_w_px` reported as the stale-from-vertical value while in horizontal mode

**What I did:** vertical → horizontal toggle.

**What I expected:** sidebar_w_px reset to 0 when in horizontal mode (no
sidebar painted).

**What happened:** state-json still shows `sidebar_w_px:206` (the
last vertical-mode width) while `tab_layout:"horizontal"`. Rendering
branches on `tab_layout`, not `sidebar_w_px`, so it's harmless — but it's
a stale-state smell that could trip future code paths that assume the
field means "currently-painted sidebar width."

**Severity:** SEV-3. Cosmetic API hygiene.

### 10. Negative result — chip clicks, +, search, toggle, top palette chip, edge / extreme clicks do not crash

Multi-tab (1-15 tabs) chip clicks at varying y/x, repeated +, repeated
search toggle, repeated layout toggle, wheel over sidebar / strip / body,
and edge clicks (0,0) and (959,0) all completed cleanly without panic
or state corruption. Good baseline.

## Summary

- 3 SEV-1: sidebar-disappears-on-toggle-return; click-body-closes-empty-
  label-tab; (combined) impact on first-launch users.
- 4 SEV-2: divider drag grab-zone off by inset_px; sidebar-header dead
  band between search and +; sidebar-row-right-of-chip not clickable;
  config persistence on every toggle.
- 3 SEV-3: dropdown-chevron-does-nothing-visible on Shell panes;
  sidebar_w_px stale during horizontal mode; harness can't observe wheel
  scroll state.
