---
title: Running mnml inside tmnl
description: How tmnl and mnml integrate — auto-promote to a native tab, chrome de-duplication, mouse forwarding, and live theme adoption — so the two apps feel like one product instead of two stacked terminals.
---

[mnml](https://mnml.sh) is the editor in this family; tmnl is the terminal. Most days you'll use both — open mnml from a tmnl shell, edit, run a command in another tab, switch back. This page covers everything tmnl does to make that flow feel like one product rather than "two terminals stacked on top of each other."

The short version: typing `mnml` in a tmnl shell launches it as a native tab (not a pty inside the shell), tmnl's chrome strip retints to match mnml's theme, the inline palette bar mnml normally draws is hidden because tmnl already shows one, and mouse clicks on tabs / tree rows reach mnml even when it does run as a pty child.

## Two ways mnml can run inside tmnl

mnml supports two runtime shapes when launched under tmnl, and the differences matter for what features work.

### Native blit client

`mnml --blit <socket>` connects to a tmnl Unix socket and ships typed `Frame`s of cells over the wire. tmnl's wgpu cell pipeline draws those cells directly into the same `Grid` it uses for shell tabs. No `vt100`, no escape codes.

This is the integrated path. Everything in the rest of this page assumes you want this:

- mnml's tab chip lives next to your shell tab chips in tmnl's strip — `Cmd+1`–`Cmd+9` to switch.
- `mixr.show` from inside mnml asks tmnl to open mixr as a *sibling* native tab, not a nested pty inside mnml.
- mnml's `:tmnl.pop-pty` hands a running pty (e.g. a long-lived `claude` session) over to tmnl as a fresh shell tab — same `claude` process, no restart.

### Standard pty child

`mnml` typed at a shell prompt with no `--blit` flag runs as an ordinary pty child of that shell. tmnl hosts the pty, the `vt100` parser turns mnml's escape-code output back into cells, and tmnl draws those cells. Functionally it works — you can edit, save, exit — but a few things are subtly worse:

- mnml fills the body of one shell tab; you don't get a dedicated tab chip.
- `mixr.show` falls back to spawning mixr as a pty inside mnml instead of a sibling tab.
- `:tmnl.pop-pty` toasts an error (there's no blit channel to route through).

For most workflows the native path is strictly better. tmnl makes that the default automatically — see the next section.

## Auto-promote — the default for `mnml` at a tmnl prompt

When you type `mnml` (or `mnml ~/some/repo`, or `mnml file.txt`) at a tmnl shell prompt, mnml detects that it's running inside tmnl and asks tmnl to relaunch it as a fresh native tab, then exits. The pty session in your shell flickers and ends; a new mnml tab chip appears in tmnl's strip.

### How tmnl signals it

tmnl exports `TMNL_TRANSFER_SOCKET=<TMPDIR>/tmnl-<pid>-transfer.sock` into every child process's environment **before** any subprocess `exec`. That env var is set once at startup and inherited through the whole subshell tree, so any program — including a `mnml` typed five subshells deep — can find it.

### How mnml uses it

At startup, mnml checks five conditions. All must be true for auto-promote to fire:

1. `--blit <socket>` is **not** passed (mnml isn't already a blit client).
2. `--headless` is **not** passed.
3. `--no-native-promote` is **not** passed (the opt-out).
4. `stdin` is a TTY (interactive launch, not `mnml < script.txt` or a CI invocation).
5. `TMNL_TRANSFER_SOCKET` is set (we're inside tmnl).

When all five hold, mnml opens the transfer socket, sends `Message::OpenPane { command: "mnml", args: [...] }` with no fd attached, and exits cleanly. tmnl's transfer listener receives the message, maps it to a `PromoteToNative` event, and spawns mnml as a new top-level native tab. The user-visible effect is that `mnml` at the shell prompt opens a mnml tab.

If any of the five conditions fails, mnml falls through to the standard pty path and works as a pty child of the shell.

### The `--no-native-promote` opt-out

Two workflows want pty mnml even inside tmnl:

- **Split-pane workflows.** You've got tmnl's `Cmd+D` split horizontally and want mnml in the left half + a shell in the right half. Auto-promote would yank mnml out into its own tab, breaking that layout.
- **Transient `mnml file.txt` edits.** You want a one-off edit in your current shell context — same cwd, same env, no fresh tab to clean up.

For these, pass `--no-native-promote`:

```sh
mnml --no-native-promote
mnml --no-native-promote some-file.txt
```

Or alias it in your shell so all `mnml` invocations stay pty-shaped:

```sh
alias mnml='mnml --no-native-promote'
```

### Failure modes

Auto-promote is best-effort — if it can't reach tmnl it silently falls through to the standard pty path so a stale env var never bricks startup. Conditions that fall through with a stderr note:

- `TMNL_TRANSFER_SOCKET` points at a path that doesn't exist (stale env from a closed tmnl). mnml prints `mnml: auto-native: TMNL_TRANSFER_SOCKET=… connect failed (…) — continuing as a pty session. Pass --no-native-promote to silence.` and runs as a pty child.
- The transfer socket exists but `send` fails. Same fallback, different stderr line (`send failed (…) — continuing as a pty session`).

No round-trip ack: tmnl processes the message on its next tick, the user-visible effect is "mnml's tab appears in tmnl shortly after this pty exits."

## Chrome de-duplication

mnml normally draws an inline VS Code-style palette bar across the top of its window — the "search files, run commands…" chip with back/forward buttons next to it. tmnl draws the same chip in its native chrome strip (the band above the body that holds the macOS traffic lights, the tab chips, and the centered palette cluster).

Showing both would mean two stacked palette chips with one millimeter of gap. So when mnml detects it's inside tmnl — either as a native blit client (`under_tmnl`) or as a pty child (`inside_tmnl_pty`, set from the `TMNL_TRANSFER_SOCKET` env var) — it hides its inline bar:

```rust
// mnml's src/ui/mod.rs
let palette_bar_visible = area.width >= 80 && !app.is_inside_tmnl();
```

Both modes get the same treatment. You see exactly one palette chip, in tmnl's chrome, even if mnml didn't get auto-promoted.

The palette cluster in tmnl's strip isn't decorative either — its hit-rects forward clicks to the focused native pane:

| Chip | Click sends |
| --- | --- |
| Search chip | `Ctrl+Shift+P` (mnml's command palette) |
| Back arrow | `Ctrl+PageUp` (mnml's `buffer.prev`) |
| Forward arrow | `Ctrl+PageDown` (mnml's `buffer.next`) |
| Dropdown chevron | `Ctrl+R` (mnml's recent files) |

So clicking the search chip in tmnl's chrome opens mnml's palette. The chrome is one bar; it acts like mnml's bar.

## Mouse and wheel forwarding

When mnml runs as a *native blit client*, mouse events route over the tmnl-protocol socket as `InputEvent::Mouse` — tmnl translates a winit mouse event into protocol values and ships it; mnml receives crossterm-shaped events. That's wired through and always worked.

When mnml runs as a *pty child* (auto-promote opted out, or detection didn't fire), tmnl now encodes mouse events as xterm mouse-protocol bytes and writes them to the pty's master. The shell-mode pty pane gained two methods for this — `write_mouse` for clicks and `write_mouse_motion` for hover / drag — both in `src/shell.rs`.

### Honoring the child's DECSET mode

A bare shell prompt doesn't want mouse bytes — they'd land in stdin as garbage. The pty stays silent on the wire (the parser reports `MouseProtocolMode::None`) and `write_mouse` returns `false` without sending anything.

When a TUI like mnml comes up and sends `\e[?1000h` / `\e[?1002h` / `\e[?1003h` / `\e[?1006h` (DECSET requests for mouse tracking), the parser flips to `Press` / `ButtonMotion` / `AnyMotion` / `Sgr` respectively. `write_mouse` checks the active mode + encoding on every event and encodes accordingly:

- **SGR 1006** (`\e[<button;col;row;M`/`m`) when the child requested it. Most modern TUIs prefer this — it handles columns past 223.
- **X10** (`\e[M<button+32><col+33><row+33>`) as the fallback for legacy children that didn't enable SGR.

Press / release semantics are encoded per the mode: SGR uses `M`/`m` to distinguish; X10 encodes release as button-bits `3` because the protocol doesn't carry which button was released.

### Hover and drag

`write_mouse_motion` covers the `?1002h` (drag-only) and `?1003h` (any motion) modes. Hover events fire with no button held; drag events carry the held button's xterm code. mnml's tooltips, the tree rail's hover-preview thumbnails, and the splitter-divider yellow tint all work over the pty boundary because of this.

### Wheel scrolling

Wheel ticks route through the same path. tmnl translates each wheel tick into a synthesized xterm wheel-button event (button `4` for up, `5` for down) and feeds it through `write_mouse`. So in a pty mnml, wheel-down in the tree scrolls the tree, wheel-up in the editor scrolls the editor, identically to native mode.

One nuance: shell mode has a *second* wheel path for when the alt-screen isn't active. A wheel-up on a plain `zsh` prompt scrolls vt100's scrollback (history) — that's the terminal acting normally, not forwarding to the child. The alt-screen check disambiguates:

```rust
PaneKind::Shell { session: Some(s) } if !s.altscreen_active() => {
    // bare shell — scroll vt100 scrollback
    s.scroll(lines);
}
PaneKind::Shell { session: Some(s) } => {
    // full-screen TUI (mnml / vim / htop) — forward wheel to the child
    s.write_mouse(col, row, BUTTON_WHEEL_UP, true, mods);
}
```

mnml flips to the alt-screen on startup, so it gets the wheel events; an idle shell prompt doesn't, so wheel scrolls the scrollback.

## Theme adoption

tmnl's chrome — the strip background, the arrow buttons, the active-tab pill, the search-chip body, the tab labels, the dim hint text, the accent color — retints to match whatever mnml theme you have selected. The result: opened side-by-side, the two apps look like one design.

### At startup

`theme::init()` runs once at the top of tmnl's `main()`. It tries `Palette::from_mnml()`, which:

1. Reads `~/.config/mnml/config.toml` and pulls `ui.theme` (defaults to `"onedark"` if the file exists but doesn't specify a theme).
2. Locates `themes/<name>.toml` — first `~/.config/mnml/themes/`, then the macOS data dir, then `~/Projects/mnml/themes/` as a contributor fallback.
3. Parses the theme's `[base_30]` section and projects mnml's field names onto tmnl's chrome roles:

| tmnl role | mnml `base_30` field |
| --- | --- |
| `strip_bg` (and `clear_bg`) | `darker_black` — mnml's bufferline color |
| `btn_bg` (arrow buttons) | `black` — mnml's editor body |
| `active_chip_bg` (active tab pill) | `black2` (optional; falls back to default) |
| `chip_bg` (search chip body) | `one_bg` — mnml's selected-pane color |
| `text_fg` (active tab labels) | `white` |
| `tab_fg` (inactive tab labels) | `grey_fg` |
| `dim_fg` (hints, URLs) | `grey_fg2` / `light_grey` |
| `accent_fg` (highlights) | `yellow` / `orange` |

Best-effort: any parse / IO error falls back to defaults silently (with a `log::warn` line, not a crash).

### Live reload

The chrome retints within ~1 second of saving `~/.config/mnml/config.toml`. The mechanism is cheap — `poll_mnml_config()` runs once per tick in tmnl's app loop and `stat()`s the config file. If its mtime hasn't moved, nothing else happens. When it does move, `refresh()` re-reads the theme, swaps the global palette behind an `RwLock`, and requests a redraw. The full TOML read only fires on actual changes.

This means switching themes in mnml — either by editing the config or via mnml's settings overlay — recolors tmnl's chrome with no restart and no manual sync. Setting `ui.theme = "tokyonight"` in mnml's config retints tmnl to tokyonight by the time you've saved the file.

### The `theme.refresh` escape hatch

The mtime poll is robust in the common case but can miss when mnml writes its config out-of-band (atomic-rename via a tempfile that gets the same mtime as the old file, network-mounted homedir with truncated mtime resolution, etc.). For those, the palette command `theme.refresh` forces a re-read:

```
Cmd+Shift+P → "Theme: reload chrome palette from mnml"
```

## Defaults when mnml isn't installed

A user who installed tmnl without mnml still gets the same look. tmnl ships hardcoded fallbacks in `Palette::defaults()` that are *eyedropped from mnml's onedark rendered in Apple Terminal* — not the literal hex values from mnml's theme file.

The distinction matters. Terminal apps apply a small color transform between source hex (`#1b1f27`) and what reaches the screen (`rgb(26, 29, 34)`). The shipped defaults are the rendered bytes, so a tmnl-only user gets the colors that mnml-in-a-terminal *looks like*, not the source-of-truth hex that would render slightly off in tmnl's GPU pipeline.

The five baseline values:

| Role | RGB (out of 255) |
| --- | --- |
| `strip_bg` | `26 29 34` |
| `btn_bg` | `30 34 40` |
| `active_chip_bg` | `36 39 45` |
| `chip_bg` | `41 45 53` |
| `tab_fg` | `159 167 180` |

When mnml *is* installed, `from_mnml()` reads the source hex directly (the same display renders both apps, so no transform applies) and adopts those values verbatim.

## Troubleshooting

### Auto-promote doesn't fire — `mnml` runs as a pty child

Check, in order:

1. **`TMNL_TRANSFER_SOCKET` is set in the shell.** From the tmnl shell prompt: `echo $TMNL_TRANSFER_SOCKET`. Should print `/var/folders/.../tmnl-<pid>-transfer.sock` or similar. If empty, tmnl didn't export it — restart tmnl from `/Applications` (or `tmnl` from a shell) so a fresh `main()` re-exports it.

2. **stdin is a TTY.** Auto-promote skips piped invocations (`echo q | mnml`, `mnml < script`). It's interactive-only by design.

3. **You're not opting out.** `--no-native-promote` and `--headless` both suppress. Same for `--blit <socket>` (mnml is already a blit client).

4. **Check stderr for the failure note.** If the env var is set but the socket connect failed, mnml prints `mnml: auto-native: TMNL_TRANSFER_SOCKET=… connect failed (…) — continuing as a pty session.` The most common cause is a stale env var from a closed tmnl process that left its socket file behind. Restart tmnl.

### Nightly bundle: tmnl-nightly can't find `mnml` on PATH

When tmnl launches from `/Applications` (or via the nightly launcher script), macOS gives it a stripped LaunchServices environment — no `~/.zshrc` exports, system PATH only. tmnl now backfills the login-shell env at startup specifically to fix this: if stdin isn't a TTY, it runs `$SHELL -l -c env` and re-exports each variable onto its own process before spawning anything. Children (mnml, mixr, shells) inherit the full env, so a `Launcher::spawn("mnml", …)` finds `~/.cargo/bin/mnml` (or wherever you installed it).

If `mnml` still isn't found from a launched-from-Applications tmnl, check that your login shell prints it on the right PATH:

```sh
$SHELL -l -c 'which mnml'
```

If that comes back empty, mnml isn't on your login-shell PATH and tmnl's backfill can't conjure it — install mnml so it ends up on PATH (`cargo install mnml`) or symlink it into `/usr/local/bin/`.

### Pty mnml works but I want native; auto-promote keeps failing

Force the native path manually. From a tmnl shell:

```sh
unset TMNL_TRANSFER_SOCKET   # forget the broken socket
exec $SHELL                  # fresh shell, no socket env

# then open a new mnml tab via the GUI:
# Cmd+T in tmnl (if the window template is mnml-shaped)
# or launch a fresh tmnl with: tmnl --mnml ~/your/workspace
```

`tmnl --mnml WORKSPACE` is the explicit "open mnml as a native tab" entry point and never relies on auto-promote.

## Next

- [Tabs, splits, and panes](/manual/tabs-and-splits/) — how mnml's tab chip sits next to shell tabs and what Cmd-chords do where.
- [Native tabs](/manual/native-tabs/) — the underlying tmnl-protocol that makes blit mode work.
- [Getting started](/manual/getting-started/) — first-launch walkthrough including the welcome screen + recents list.
- [Troubleshooting](/troubleshooting/) — system-level diagnostics outside the mnml integration.
- [mnml.sh](https://mnml.sh) — the editor's own manual.
