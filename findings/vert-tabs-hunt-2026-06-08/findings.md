# tmnl vert-tabs hunt — 2026-06-08

Focused review of yesterday's vertical-tab work (sidebar column,
drag-to-resize, version-in-settings, single-tab thinking glyph).
Static code review + headless drive — no live window.

**Severity counts:** SEV-1 2 · SEV-2 4 · SEV-3 5 · SEV-4 3

## SEV-1 — features advertised but dead in production

### #1 — sidebar drag-to-resize is unreachable

`src/app.rs:2706-2843`. `handle_mouse_input` computes
`in_chrome = in_top_strip || in_left_sidebar` and returns at
`:2843` for **every** press inside the chrome. The drag-arming
code at `src/app.rs:2859-2873` lives AFTER that return. The grab
zone (±4px around `sidebar_w_px`, i.e. the sidebar's right edge)
is itself inside `in_chrome`, so the chrome branch consumes the
click first and the drag never arms.

This explains the user's "still not draggable" complaint — even
after `rsync`'ing the fresh binary into `/Applications/tmnl.app`,
the feature was structurally broken. **Commit `3ee972b`'s
drag-to-resize shipped dead.**

**Fix:** hoist the sidebar-drag block to before the `in_chrome`
early-return. Grab zone is most-specific; chrome is least-specific.

### #2 — live `tick()` ignores `sidebar_w_override`

`src/app.rs:1435-1459`. `refresh_strip_layout` (`:709-749`) was
updated in `3ee972b` to consult `sidebar_w_override` via
`gpu.clamp_sidebar_w_px(w)`. But `tick()` still has a DUPLICATE
inline strip-layout block from before that change — it calls
`gpu.compute_sidebar_w_px(&chips)` directly with no override
branch. Every tick (60Hz) clobbers the override back to the
auto-fit width.

Even if SEV-1 #1 is fixed, the column would snap back to
auto-fit on the next frame.

**Fix:** delete the inline duplicate; call
`self.refresh_strip_layout()` from `tick()` instead. The `chips`
local at `:1391-1415` is also duplicated against
`compute_strip_chips()` and isn't used after `:1459` — delete it
too.

## SEV-2 — broken corner cases

### #3 — `clamp_sidebar_w_px` can panic on narrow windows

`src/main.rs:829`. Uses `f32::clamp(min, max)` where
`min = inset_px + 2.0 * cell_w` and `max = viewport_px * 0.5`. On
a sufficiently narrow window (`viewport < 4 * cell_w + 2 *
inset_px`), `min > max` and `clamp` **panics**. Path is
user-reachable via window shrink + tab open.

**Fix:** explicit `if min > max { return min; }` guard, or
`w.max(min).min(max.max(min))`.

### #4 — `compute_sidebar_w_px` has no upper clamp

`src/main.rs:806`. Returns `SIDEBAR_PAD_LEFT_PX + (with_plus +
3.0) * cell_w` directly. On a tab with a 60-char label, the
sidebar can grow past half the viewport. `relayout_all_panes`
will then compute a negative or zero body width. No crash, but
the body shrinks to nothing and the user can't see their
terminal.

**Fix:** same `viewport * 0.5` ceiling that `clamp_sidebar_w_px`
already enforces — share one helper.

### #5 — closing a tab during drag leaves `dragging_sidebar`
stuck true

`src/app.rs` — `close_tab_at` doesn't clear `dragging_sidebar`.
If the user middle-clicks a chip while drag is armed (e.g. drag
starts, then middle-click to close), the flag stays set and the
next mouse-move keeps resizing.

**Fix:** clear `dragging_sidebar = false` in `close_tab_at` (or
zero `sidebar_w_override` when tabs drop to ≤1).

### #6 — sidebar disappears on single-tab vertical layout

`src/app.rs:730-741` returns `0.0` for `target_sidebar` when
`!multi_tab`. The `+` new-tab chip is then invisible — the user
has no GUI affordance to create a second tab from a single-tab
state without a keyboard chord. (Top-strip palette is also hidden
in vertical mode.)

**Fix:** keep a minimal sidebar (just enough for the `+` chip)
on single-tab vertical. Or render `+` in the top strip as a
fallback when vertical is configured but only one tab.

## SEV-3 — UX rough edges

### #7 — no cursor-change on hover over grab zone

User has no visual cue the border is draggable. Best-practice
TUIs / browsers swap to `ew-resize` when the pointer is within
the grab zone. winit supports this via `Window::set_cursor`.
Polish, but the feature is undiscoverable without it.

### #8 — `sidebar_w_override` is never cleared

Once the user drags, the override sticks for the rest of the
session. No keybinding / palette command to reset to auto-fit.
Users who drag too narrow have no recovery short of editing
no-config-on-disk state.

**Fix:** `Esc` while hovering the grab zone resets to `None`,
OR a `tabs.reset_sidebar_width` palette command.

### #9 — headless `tab.new` doesn't trigger strip refresh

`src/headless.rs` — adding a tab via the IPC drive doesn't call
`refresh_strip_layout` (only `tick()` does, and that's the
SEV-1 path that ignores override). Tests can't assert the
post-add layout without `tick()`-ing several times.

**Fix:** call `refresh_strip_layout` after every state mutation
in headless drive.

### #10 — `state-json` IPC missing sidebar fields

`src/ipc/state.rs` (or wherever the state-json shape lives) —
emits `strip_h_px` but not `sidebar_w_px` / `sidebar_w_override`.
External tests that drive headless can't read back what they set.

### #11 — stale comment in `src/config.rs`

References the old "tab_layout enum with three values" — there
are only two (`Horizontal`, `Vertical`).

## SEV-4 — micro

- #12. Hardcoded `0.22, 0.23, 0.26` border color in
  `set_globals` (`src/main.rs`) — not exposed in the theme
  schema. mnml-themed apps will look mismatched.
- #13. `INTER_ROW_GAP_PX` is one constant for both vertical and
  horizontal modes after the day's churn. Originally split.
  Probably fine — `16px` works in both — but flag it.
- #14. `dragging_sidebar` is `bool` not `Option<DragStart>` —
  doesn't remember the initial drag-anchor x, so the user can't
  see a delta-style "this much wider" overlay during the drag.
  Polish item.

## Triage order

1. **SEV-1 #1 + #2** — ship now. Both block the feature the user
   asked for and reported as broken.
2. **SEV-2 #3** — guard against the panic. One-line fix.
3. **SEV-2 #5 + #6 + #4** — corner cases. Bundle.
4. **SEV-3** — polish next session.
5. **SEV-4** — backlog.

## Methodology

Static read of `src/app.rs` (mouse + tick paths) + `src/main.rs`
(GPU sidebar helpers) + the day's commits (`git log
2026-06-07..HEAD --stat`). Cross-checked against
`refresh_strip_layout` vs `tick()` inline block diff. No live
window — would need an interactive session to confirm SEV-3 #7.

Test suite: 128 unit tests pass post-change, clippy clean,
`apply_thinking_glyph` invariant test still locks the "never
two asterisks" rule. The bugs are in the connections between
correct unit-test-covered helpers — exactly what static review
catches and unit tests miss.
