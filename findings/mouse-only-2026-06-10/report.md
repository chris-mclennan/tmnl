# tmnl mouse-only bug-hunt — 2026-06-10

Headless probe via `./target/release/tmnl --headless --app` at default 960×636 px, vertical tab_layout, sidebar auto-sized to 206 px.

## SEV-2

**1. Dead-pixel band between sidebar tab chips.** Chip hit-rect is exactly `TAB_ROW_H_PX=32 px` but chips stack with `VERT_INTER_ROW_GAP_PX=12`. Click sweep at sidebar x=50: chip 0 = y 102-133, dead = 134-147 (14 px), chip 1 = 148-179, dead = 180-~192. Warp/VS Code style would extend chip_y1 to the next chip's y0. Source: `src/main.rs::strip_chip_instances` lines 1494-1512 (row_geom returns y0+TAB_ROW_H_PX with no gap absorption).

**2. Close-button hit-rect is ~1 cell (≈9 px) wide.** `close_x_px → close_x_px + cell_w`. On chip 1 ("zsh") the rect is x=109-117 — a 9-px-wide target. Industry standard ~24-28 px. Source: `src/main.rs::strip_chip_instances` lines 1656-1664.

**3. Chip drag-reorder has no movement threshold.** `dragging_tab` arms on every left-press in a chip. `handle_cursor_moved` swaps tabs the instant the cursor crosses into a different chip's rect with no distance gate. Combined with full-row vertical chip rects + 13-14 px inter-row dead bands, a click at chip 0's bottom + natural drift during release crosses into chip 1 and swaps. Sidebar-resize already uses `DRAG_THRESHOLD_PX=6` (line 3226) — chip-reorder should match. Source: `src/app.rs` lines 3266-3294 + 3754-3755.

## SEV-3

**4. `+` button doesn't dismiss `tab_search`.** Click search bar → `tab_search=Some("")`. Click `+` button → new tab spawns, `tab_search` stays open. Body-click dismissal exists but the `+` branch returns early at lines 3673-3680 without touching it.

**5. Header → chip 0 transition has a 5-6 px dead band.** Header y ≈ 72-95, chip 0 starts at y=102. The band y=96-101 is in `in_chrome` and hits neither. Source: `sidebar_header_instances` y1 vs `strip_chip_instances::first_row_top_px` (offset = `SIDEBAR_HEADER_H_PX + SIDEBAR_HEADER_GAP_PX=10` line 1455-1465).

**6. Sidebar drag handle has no cursor-icon feedback.** `src/app.rs` has zero `set_cursor` / `CursorIcon` calls. The 1-cell grab column at the sidebar's right edge is invisible to mouse users until accidentally pressed.

## MINOR

**7. Search vs `+` button has a 2 px dead band.** sidebar_w=206: search ends x=180, `+` starts x=183. The earlier fix-comment promised zero dead pixels.

## Not bugs (verified clean)

- body-click dismiss for tab_search + find works (3915-3926)
- search-bar double-click does not toggle-close (3693-3702 fix holds)
- middle-click closes any chip incl. empty-label chip 0
- close-rect tested before chip-rect (no double-fire)
- palette cluster consumes any-button click
- sidebar-toggle correctly anchors to palette left edge (~x=270-290)

## Methodology

Sweep scripts at `/tmp/tmnl_p*.txt`. Confirmed chip 0 = 102-133, chip 1 = 148-179, close on chip 1 = x 109-117, toggle = x 270-289 vertical / 265-289 horizontal, `+` button = x 183-225, search bar = x 28-180 / y 72-95. `synthetic_click` is atomic press+release, so SEV-2 drag-threshold is inferred from source; `synthetic_type` does not drive `rename_handle_key` (live winit path only).
