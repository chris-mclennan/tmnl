# tmnl hybrid bug-hunt — 2026-06-10

Drove the headless `--app` harness through the 8 targeted hybrid flows. **One new SEV-2 + one SEV-4 + one SEV-5.** Several coverage gaps (rename keys, drag-with-button-held, Native panes) the harness can't drive.

## Finding 1 — SEV-2 — Sidebar-toggle leaves `tab_search` armed invisibly

Toggling `vertical → horizontal` while `tab_search` is open does NOT clear search state. The sidebar (visible host for the query) vanishes, but `tab_search` stays `Some("…")`. Subsequent printable keys flow into the invisible buffer instead of the focused shell. Only `Esc` releases the trap.

**Repro (verified twice):**
```
tab.new; tab.new
hover 100 100             # sidebar=206, vertical
click 100 75              # opens sidebar search (tab_search="")
type hello                # tab_search="hello"
click 280 20              # sidebar-toggle button → horizontal, sidebar=0
state-json                # tab_search="hello"  ← still armed, no UI
type abc                  # tab_search="helloabc"  ← keys invisibly appended
                          #   (these chars NEVER reach the shell prompt)
key escape                # tab_search=None  ← only escape works
```

Trace:
```
#4 layout=vertical    sidebar=206 tab_search='hello'
#5 layout=horizontal  sidebar=0   tab_search='hello'         ← orphaned
#6 layout=horizontal  sidebar=0   tab_search='helloabc'      ← key trap
#7 layout=horizontal  sidebar=0   tab_search=None            ← Esc fixes
```

**Root cause:** `src/app.rs::handle_mouse_input` toggle branch (~line 3584) flips `cfg.tab_layout` + persists, but does not touch `self.tab_search`. The vertical-only sidebar-header overlay (where the query renders) goes invisible while `tab_search_handle_key` (line 328 in `dispatch_synthetic_key`, line 2864 in the live path) keeps greedily eating printable chars.

**Fix:** clear `self.tab_search = None` (and likely `self.find = None` for parity) immediately after the toggle so the only modal-input state that survives is one with visible chrome. Two-line change in the toggle branch (around line 3594).

Same family of bug as the 06-09 hybrid report's SEV-2s — keys silently captured by an overlay the user can't see.

## Finding 2 — SEV-4 — Find query does not persist across re-open; tab_search does

Asymmetric overlay behavior. After 2026-06-10's "keep query visible after Enter" fix for `tab_search`, the parallel `find` overlay still drops its query when dismissed.

**Repro:**
```
key cmd+f; type test       # find={query:"test"}
key escape                  # find=None
key cmd+f                   # find={query:""}    ← prior query gone
```

vs. `tab_search`:
```
click 100 75; type hello    # tab_search="hello"
key enter                   # tab_search="hello"  ← preserved
click 100 165               # tab_search="hello"  ← preserved across switch
```

**Suggested fix:** persist `last_find_query: Option<String>` on `App` and re-seed on next `Cmd+F`. `src/app.rs::handle_keyboard_input` find branch (~line 2971) and `dispatch_synthetic_key` find branch (~line 371) both build a fresh `FindState` on every open.

## Finding 3 — SEV-5 — Truncation overhead overcounts by 2 for empty-label vertical chips

In `src/main.rs` (~line 1581), `max_label` is computed with `overhead = pad + attn + 2.0  // gap + close`. The 2.0 counts the close glyph + gap-before-close, but `close_glyph_rendered = !(vertical && label.is_empty())` at line 1621 suppresses the close glyph entirely for empty-label vertical chips. The truncation budget for an empty-label chip is consequently 2 cells too tight — a brand-new tab whose first arrived title is right at the budget would truncate one char sooner than needed. Cosmetic boundary case.

## Confirmed-correct (the spec'd flows that held up cleanly)

- **Flow 1 — search-persists-across-tab-switch (vertical).** `tab_search` retains its query after `Enter` and after clicking sidebar chips to switch tabs. The 2026-06-10 "previously did `tab_search.take()`" comment is wired correctly.
- **Flow 2 — body click dismisses tab_search AND find.** Both `tab_search.is_some()` and `find.is_some()` branches in the `pane_under_cursor()` press path (~line 3905) clear on press into body. Cursor returns to the focused shell.
- **Flow 4 — single click in the divider grab zone does NOT move the separator.** The `sidebar_drag_press_x` + `sidebar_drag_prev_override` snapshot/restore pair in `handle_mouse_input` keeps the override frozen on release-without-drag. Verified with 10+ probes inside the [border_x − cell_w, border_x] zone.
- **Flow 7 — sidebar-toggle flips `cfg.tab_layout`.** Verified `vertical ↔ horizontal` round-trip via `state-json`; the 2026-06-09 "save only on change" guard is preserved.
- **Right-click outside any chip is a no-op** (does not start a stray rename).

## Coverage gaps (harness limits, NOT bugs)

- **Flow 3 — inline rename key isolation.** `dispatch_synthetic_key` (the headless-only key path, ~line 303) has NO rename branch. The live `handle_keyboard_input` invokes `rename_handle_key` at line 2952; the synthetic path skips it. So after a `click _ _ right` arms a rename, subsequent `type …` chars take the pty-fallback branch and stream to the shell, not the rename buffer. Rename is unreachable from headless. The production path looks correct on inspection but I can't cover-test it. **Recommend** adding a `RenameState`-aware branch to `dispatch_synthetic_key`; and exposing `renaming_tab` in `state-json`.
- **Flow 5 — drag-then-click leak regression.** `synthetic_click` issues `Press + Release` atomically with no `CursorMoved` between, so a real drag (`Press, Move, Move, Release`) can't be simulated. Code inspection of the release paths (~lines 3794-3805 + 3823-3834) shows both in-chrome and post-chrome release branches now clear `dragging_sidebar` / `sidebar_drag_press_x` / `sidebar_drag_prev_override` unconditionally — the 2026-06-09 fix looks robust on paper but isn't headless-coverable. **Recommend** a `mouse-press <px> <py>` / `mouse-move` / `mouse-release` decomposition of `click`.
- **Flow 6 — long-label truncation w/ narrow sidebar.** Headless rename is broken (above), so I can't drive a tab into a 50-char label without code edits. Inspection: `chip_cells` truncation (~line 1577) clamps with `max_label = (avail − overhead − right_margin).max(1.0) as usize` and pushes `…` — correct shape; see SEV-5 above for the only nit. **Recommend** a headless `tab.rename <idx> <label>` command for direct-set.
- **Flow 8 — Native pane drag swallow test.** No way to spawn a `Native` pane from `--headless --app` (needs a `--blit` socket + peer). The selection-drag fix lives in `handle_cursor_moved` (`!in_chrome` branch around line 3247-3257) — selection only arms outside chrome AND only with `dragging_selection` set on the prior press, both of which a Native pane's pre-existing `dragging_*` state should override on its own buttons-down path. **Recommend** a stub Native pane (no real blit child) so routing is reachable.
- **Cmd+, for settings.** Routes through the macOS menu bar; headless has no menu surface.

## Side note — flow 7 mutates user config

The sidebar-toggle button persists `tab_layout` to `~/.config/tmnl/config.toml` on every flip (`cfg.save()` at line ~3600). A bug-hunt session that exercises this flow flips the user's default away from `vertical`. Not a bug per se — the persistence is intentional — but a test-harness annoyance. Consider an env-var override (e.g. `TMNL_DISABLE_CONFIG_SAVE=1`) for headless runs.

## Summary

The recent 06-09 / 06-10 hybrid hardening (chord-eating leaks in find / tab_search, drag-state leak across releases, sidebar-toggle persistence, Enter-keeps-query-visible) all hold up. **One new SEV-2 leaked through**: the sidebar-toggle button removes the visible chrome that hosts `tab_search` without clearing the modal-input state, so subsequent typing is silently swallowed. Two-line fix at `src/app.rs:~3594`. Plus a SEV-4 cosmetic ("find loses query on close — tab_search now keeps it") and a SEV-5 boundary-case off-by-2 in truncation overhead.

Three of the eight target flows are partly uncoverable from `--headless --app` today; suggestions inline.
