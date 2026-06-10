# Keyboard-only bug-hunt — tmnl (2026-06-09)

Drove via `./target/release/tmnl --headless --app` using only `key <spec>` / `type` / `state-json`. 10 findings; 2 SEV-2, 5 SEV-3, 3 SEV-4 (coverage gaps).

## SEV-2 findings

**1. Find overlay greedy-eats every `cmd+letter` chord.** While the Find overlay is open, ANY `cmd+letter` chord drops its modifier and the bare character is appended to the find query. Tested: `cmd+f` → open. Then `cmd+t cmd+w cmd+a cmd+1` produces `find.query = "twa1"`. Also confirmed for `cmd+v`, `cmd+s`, `cmd+shift+p` (inserts `"P"`), `cmd+,` (inserts `","`). Root cause: `src/app.rs:2702` (real `find_handle_key`) and `src/app.rs:374` (`dispatch_synthetic_key`) both match `Key::Character(s)` with no modifier guard. The doc-string at `app.rs:343` literally says "greedily consumes keys" — that's the bug. Affects real GUI loop, not just headless.

**2. Same shape on `tab_search` modal.** Identical pattern at `src/app.rs:330` (synthetic) and `src/app.rs:2750` (real). Couldn't fire live because tab-search has no keyboard chord (see Finding 5/7), but the code path is identical — mouse-open the chip then press `cmd+t` and the "t" will be typed instead of opening a new tab.

## SEV-3 findings

**3. Rapid `cmd+f cmd+f` inserts literal "f".** Sub-case of Finding 1: second `cmd+f` while find is open → `find.query = "f"`. Third → `"ff"`. Same fix as Finding 1.

**4. `cmd+,` does NOT open Settings.** Pressed in isolation, `settings_open` stays `false`. Root cause: no `cmd+,` entry in `src/command.rs` — Settings is wired only to the macOS menu (`menu.id_settings` → `open_settings()` at `app.rs:1916`). In headless `--app` no menu loop runs; in real GUI the chord works through AppKit but there's no fallback for a focused pty grabbing the keystroke. Add `Command { id: "view.settings", keys: &["cmd+,"], … }`.

**5. `cmd+shift+t` is unbound.** `tab_search` stays `null`. No keyboard chord opens tab-search anywhere in `src/command.rs`.

**6. `esc` does NOT close palette in headless.** `cmd+shift+p` opens (`palette_open: true`) but subsequent `esc` leaves it open. Root cause: `dispatch_synthetic_key` (`src/app.rs:301-406`) inlines the `tab_search` and `find` modal handlers but **never calls** `palette_handle_key`, `settings_handle_key`, or `welcome_handle_key`. The real path calls them at lines 2787/2790/2806. Headless-only behavior bug, but it blocks coverage of Findings 4 / 8 / Settings UI.

**7. Tab-search is keyboard-unreachable.** Only mouse-openable via the strip chip (`src/app.rs:3418`) or sidebar search rect (`:3487`). Key-only users have no path in.

## SEV-4 findings (coverage gaps)

**8. `cmd+shift+/` (help) can't be verified.** No `help_open` field in state-json. `view.help` IS registered at `src/command.rs:963` with the right keys + alias `cmd+?`, so it likely works. Recommend adding `help_open` to the state-json schema (`src/headless.rs:381`).

**9. `cmd+c` behavior can't be verified.** No state-json field exposes selection bounds or pty input bytes. Tested only "does not crash".

**10. `cmd+9` clamps to last tab.** With 5 tabs, `cmd+9` → `active: 4` (last tab). Matches Chrome convention; flagging because prompt phrased this as "switch to tab N". `cmd+1`..`cmd+5` worked exactly as expected (`active: 0`..`4`).

## Passed cleanly

`cmd+t` (tabs grows), `cmd+w` (tabs shrinks / quits on last), `cmd+1`..`cmd+5`, `cmd+9` (last-tab), `cmd+shift+]` (next, wraps from last to first), `cmd+shift+[` (prev, wraps from first to last), `cmd+f` single-shot open + type + backspace + esc-close, `cmd+a` and `cmd+c` (no crash), `cmd+shift+p` open (close blocked by Finding 6).

## Recommended fix priority

1. Findings 1/2/3 — one fix: skip the `Key::Character` modal arms when `mods.super_key() || mods.control_key() || mods.alt_key()` so command registry receives the chord. Touches 4 spots in `src/app.rs`.
2. Finding 6 — mirror the palette/settings/welcome handler chain inside `dispatch_synthetic_key`. Unblocks everything else under headless.
3. Findings 4 + 5 — register `cmd+,` → settings and `cmd+shift+t` → tab-search as proper `Command` entries.
4. Findings 8 + 9 — extend state-json schema (`src/headless.rs:381`) with `help_open` + `selection_active`/`last_pty_bytes`.
5. Finding 10 — confirm Chrome convention is intentional.
