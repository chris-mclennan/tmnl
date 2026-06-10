# tmnl hybrid bug-hunt — 2026-06-09

Drove the headless app through ~30 sequences across 10 workflows. **Two SEV-2 + four SEV-3 findings.** Two SEV-2s share root cause.

## Finding 1 — SEV-2 — ⌘-chord keys leak into find overlay's query

`⌘F` while find is open appends `f` to the query instead of closing/next-matching. Same for `⌘T`, `⌘1`–`⌘9`, etc.

**Repro:**
```
key cmd+f      # find opens, query=""
key cmd+f      # actual: query="f"
key cmd+t      # actual: query="ft" (new tab NOT opened)
key cmd+1      # actual: query="ft1" (tab switch NOT happening)
```
Final state: `"find":{"query":"ft1"},"tabs":1,"active":0`.

**Root cause:** `src/app.rs::find_handle_key` (line ~2662) matches `Key::Character(s)` without checking `self.mods`. Same in the mirrored `dispatch_synthetic_key` find branch (~line 3343). Guard: `if !mods.super_key() && !mods.control_key() { push }` and let the chord registry handle the chord.

## Finding 2 — SEV-2 — Same leak in sidebar tab-search overlay

**Repro:**
```
tab.new; tab.new          # 3 tabs
click 100 75              # sidebar search opens
type z
key cmd+1                 # query="z1" (tab NOT switched)
key cmd+2                 # query="z12"
```
Final state: `"tab_search":"z12","active":2`.

**Root cause:** `src/app.rs::tab_search_handle_key` (line ~2720) — same shape as Finding 1.

## Finding 3 — SEV-3 — Sidebar stays widened after closing search

Opening search bumps the sidebar from 206→271.25 px. Closing the overlay does NOT restore the prior width.

**Repro:** `click 100 75` → `key esc` → `state-json` still shows `sidebar_w_px:271.25`, `cols:72` (was 79 before).

## Finding 4 — SEV-3 — Large dead-click area right of short tab chips

Tab-chip hit-rects shrink-wrap content. `""` label = ~4 cells (~67 px); `zsh` = ~7 cells (~117 px). Sidebar is ~271 px wide. Clicking the chip's "row" past the chip's right edge does NOTHING.

**Repro:** With 3 tabs and active=0, `click 150 165` (chip 1's row, x past "zsh" body) → active stays at 0. `click 100 165` (within body) correctly selects chip 1.

Recommend extending chip hit-rects to full sidebar row width (Warp / VSCode behavior).

## Finding 5 — SEV-3 — Empty-label tab's close `×` sits at x≈50

For tab 0 (empty label), the close badge ends up at the LEFT side of the sidebar. Clicking at `x=50, y=120` closes tab 0 (`tabs: 3→2`) — far from where users expect a close button.

## Finding 6 — SEV-3 — Re-clicking sidebar search discards typed query

`click 100 75` (open) → `type hello` → `click 100 75` (toggle close) silently drops `"hello"`. Re-opening starts blank.

## Coverage gaps

- **W5 Settings** — `⌘,` routes through the macOS menu only.
- **W8 Multi-pane find dismissal** — no headless `split.h`/`split.v` command.
- **W9 Paste verification** — `key cmd+v` doesn't crash, but `state-json` has no field exposing what got forwarded.
- **W10 Welcome overlay** — only built in the winit `main.rs` path, not headless.

## Summary

Both SEV-2 findings share one root: `Key::Character` is matched without filtering modifier-bearing chords in the find and tab-search overlays. A two-line guard in `find_handle_key`, `tab_search_handle_key`, and the matching synthetic-dispatch branches closes both. The SEV-3s cluster around sidebar geometry: width-sticky-after-close, dead click area right of short chips, empty-label tab `×` position, and re-click-loses-search-query.
