# tmnl backlog

Free-form list of features + polish items that aren't yet tracked
in a commit / branch. Add to the top; cross out (or delete) when
shipped. No severity / dates required — those live in commit
messages and `findings/` reports.

---

## Claude-Code-style prompt chrome with mode line

Alt prompt visual that wraps the shell's actual prompt with
terminal-rendered chrome (drawn by tmnl, not the shell script).
Inspired by Claude Code's CLI.

Layout — the prompt area occupies four rows of body chrome:

```
─────────────────────────────────────────  ← solid line (top)
❯ user's command line here_                 ← prompt + cursor
─────────────────────────────────────────  ← solid line (bottom)
auto mode • opus 4.x • <other mode chips>   ← mode line (reserved)
[gutter goes below]                         ← gutter / status
```

- **Two solid horizontal rules** bracket the single prompt row.
  Drawn with `─` (U+2500) at the chrome accent color.
- **Slim arrow** (`❯`, U+276F) prefixes the command. Same glyph
  Claude Code + zsh / starship use.
- **Cursor block** in the prompt row:
  - Window active → solid block (current behavior in cell.wgsl
    via `ATTR_CURSOR_BLOCK`).
  - Window inactive → hollow block (new attr bit + shader path,
    or a render-time recoloring of bg with `palette().clear_bg`).
- **Mode line** below the lower rule is one cell row reserved
  for future mode indicators (auto mode, model picker, prompt
  position, etc.). Empty until something fills it.

### Window-active detection

Hook winit's `Focused(bool)` event on `WindowEvent`. Push a
flag through to `Gpu` so cursor render branches on it.

### OSC 133 anchors

Tmnl already tracks prompt boundaries via `osc133::State`. The
chrome can paint at the `A`-mark row and the `B`-mark cursor
position. No new shell-side integration needed.

### Settings control

Settings overlay row: `Prompt style: [classic] / claude-code`.
`classic` = no chrome (today's behavior); `claude-code` =
the layout above. New config field `cfg.prompt_style`.

Compose with existing `prompt_position` (Natural / Bottom)
— `claude-code` style works in either; Bottom + claude-code
matches Warp + Claude Code's UX most exactly.

### Mode line population (later)

When `claude-code` is on, mode line shows chips based on app
state:
- `auto mode` when an opt-in "let agents run autonomously"
  setting is on (doesn't exist yet)
- model name when tmnl picks one up from `MNML_CONTEXT` env
- `bottom` / `natural` indicator for `prompt_position`
- arbitrary user `[ui.mode_line]` entries

Each chip is a small pill: bg + 1-cell pad + text + 1-cell pad,
separated by `•`. Clicking a chip toggles its setting where
applicable.

### Scope

~400-600 lines: cursor shader path (~30), focus tracking
(~20), prompt-chrome renderer (~150), mode-line renderer
(~150), Settings row (~30), config field + migration (~30),
docs (~50). Not a one-session feature.

---

## Sidebar toggle button in the strip

Add a small chrome button (matching the existing arrow-pill / palette
chip aesthetic) that toggles the vertical-tab sidebar on/off without
opening Settings.

- Glyph: nf-md-dock-left (looks like `⊟` / split-pane icon, similar
  to Warp / VS Code's sidebar toggle).
- Position: in the top strip, near the palette cluster on the left
  side of the search chip.
- Click: cycles `tab_layout` between Horizontal and Vertical without
  going through the settings overlay.
- Hover tooltip: "Toggle sidebar (S)".
- Optional keyboard shortcut: `Cmd+S` for sidebar toggle (won't
  collide — Cmd+S is the global save passthrough today, but tmnl's
  shell tabs don't have a meaningful save).

User asked for the icon style from a screenshot of (Warp? Arc?) that
shows the toggle as a stylized window-split glyph.

---

## Top-strip tab search ("Search tabs..." overlay)

When the user has many tabs open, finding a specific one by name is
hard. Add a search bar that filters open tabs by name fragment.

- Trigger: click the existing search chip in the top strip, or
  `Cmd+Shift+T`.
- Renders as: an inline input field that REPLACES the search chip
  (or expands it to ~30 cells wide), with a "Search tabs..."
  placeholder.
- Live-filter the visible tab chips below as the user types — chips
  whose label doesn't match the substring fade to dim or get hidden
  entirely.
- Right side: a "config" icon (gear) → opens Settings; a `+` icon →
  spawns a new tab. This visually moves the `+` button UP from its
  current after-last-tab position into the search bar.
- Enter / click: switches to the focused match. Esc: dismisses.

Open questions:

- Does the search live inside the existing strip (replacing chrome)
  or as a centered overlay? Inline feels Warp-ish; overlay feels
  Cmd+P-ish.
- Fuzzy match like `Ctrl+P` in mnml, or substring-only?
- When the search bar is up, do tab chips still receive clicks?

References: image #18 in the user's report (Warp / Arc-style tab
search bar with config + `+` icons on the right).

---

## Bottom-anchored prompt (Warp / Claude Code style)

The shell prompt sticks to the bottom of the window; command
output history scrolls upward above it. Same UX Warp + Claude
Code + (optionally) iTerm have. Removes the "where did my prompt
go?" hunt after a long-output command.

Mechanism:

- **OSC 133 prompt-boundary tracking**: already partially wired
  via `osc133::State` in `shell.rs`. The shell (zsh with
  `precmd`/`preexec` hooks, or fish, or bash with PROMPT_COMMAND)
  emits `OSC 133 ; A ST` (prompt start), `OSC 133 ; B ST`
  (command start), `OSC 133 ; C ST` (output start),
  `OSC 133 ; D [;exit] ST` (output end). tmnl's prompt script
  needs `A` / `B` markers added; mnml-prompt.sh currently only
  emits `MNML_CONTEXT` plumbing.
- **Split-region renderer**: today the grid is one flat region.
  Bottom-mode would split the body grid into two virtual regions:
  - **Scrollback region** — top portion, height = N rows. Renders
    the accumulated command-output history. Wheel scrolls this
    region independently.
  - **Live prompt region** — bottom portion, height = (1 + edit
    rows). Renders the current prompt + the edit line as the user
    types. Always visible.
- **vt100 grid integration**: vt100::Parser tracks the full
  scrollback as a single grid. The split-region renderer needs
  to (a) freeze rows above the current prompt as scrollback when
  a `C` marker arrives, (b) render the live cursor region at the
  bottom inset row, (c) auto-scroll the scrollback region as new
  output lands.
- **Settings UI**: new `PromptPosition` enum with `Natural`
  (current) + `Bottom`. New settings row.

Open questions:

- Does the scrollback region get its own wheel handling? (Yes —
  scroll up reveals older commands. Scrolling down past the
  newest output snaps back to "live" mode.)
- Behavior under TUI alt-screen apps (vim, htop)? They take over
  the full grid — bottom-prompt mode should silently fall back
  to natural rendering while alt-screen is active.
- Does the prompt's bg get a subtle divider to visually separate
  scrollback from the active prompt zone? (Probably yes — one
  pixel of `dim_fg` is enough.)

Scope: ~500 lines + a small shell-script update + significant
testing under different shells. Real feature build, not a
one-pass session.

---

## Mouse drag-select + copy (body text)

Standard terminal: mouse-press in body starts a selection, drag
extends it, release copies to clipboard. tmnl tracks
`dragging_tab` / `dragging_divider` / `dragging_sidebar` but has
no body-selection state.

Surfaces:

- `App.selection: Option<TextSelection>` where `TextSelection` is
  `{ pane_id, anchor_cell: (col, row), focus_cell: (col, row) }`
  — anchor is press-time, focus is the live cursor.
- Mouse press in body area (not chrome) → start selection at the
  cell under the cursor. Mouse motion with button held → update
  focus. Release → copy + clear (or keep visible — VS Code keeps,
  Terminal.app clears).
- Renderer: walk visible cells, compute `is_selected(col, row)`
  for each, override the cell's bg with `palette().selection_bg`
  (new field — eyedrop from mnml's editor selection). Cleanest is
  a per-instance override on the cell pipeline.
- Copy: pbcopy on macOS, wl-copy / xclip on Linux, clip.exe on
  Windows. Walk selection range in the grid, build a string with
  `\n` between rows, trim trailing whitespace per row.
- Cmd+C should also copy the current selection (today it forwards
  as Ctrl+C and SIGINT's the shell — annoying).
- Triple-click → select line. Double-click → select word.
- Shift+click → extend existing selection.

Open questions:

- Block-rectangular selection (Option+drag) like Terminal.app?
- Selection across scrollback (only the visible viewport, or also
  hidden scrollback)? The pty's scrollback lives in vt100's
  parser state — accessible but needs different cell-coord math.

---

## Launcher rail: Top + Bottom position implementations

The `launcher_position` setting accepts `Left` / `Top` / `Bottom`
but only `Left` is wired. Top and Bottom currently fall back to
Left silently.

**Top** — render the configured launcher icons inline in the top
bufferline strip, after the `+` new-tab chip. Reuse the existing
`strip_chip_instances` infrastructure (or a new sibling method).
Hit-rects record into a new `launcher_top_icon_rects` Vec on
`PaneRects`. Mouse dispatcher gets a new branch parallel to the
existing left-rail handler. Strip-width math doesn't change since
launchers slot into existing strip room.

**Bottom** — new chrome region below the body grid (mirror of
the top strip's geometry). Requires:
- New `StripGlobals.bottom_strip_h` + a 6th quad in `strip.wgsl`.
- Cell-pipeline globals `inset_y` becomes `inset_top` /
  `inset_bottom` so the body grid leaves room for the rail.
- `compute_bottom_strip_h` on Gpu, parallel to `compute_launcher_w_px`.
- New `bottom_launcher_chip_instances` emitter.
- Hit-rects + dispatcher branch.

Estimate: Top ~150 lines, Bottom ~300 lines (including the
shader + globals work).

---

## Left-edge launcher rail (sibling integrations)

Add a vertical icon strip pinned to the left edge of the window,
mirroring mnml's `> INTEGRATIONS` section but icons-only (no text
label by default — hover for tooltip). Click an icon → opens the
sibling in a new tmnl tab. Same launcher set also reachable via
the command palette so keyboard-only users get parity.

Surfaces:

- Narrow vertical column (`~3 cells wide`) along the left edge,
  always visible. Each row is one nerd-font glyph + a hover-only
  tooltip with the sibling name + binary path.
- Click → spawn `<binary>` in a new tab. Middle-click maybe →
  spawn in a horizontal split? (defer; see open questions)
- Section divider + `+` icon at the bottom for "add launcher" —
  opens an overlay similar to mnml's `+ Add integration` (browse
  the family catalog, show install state, install via `cargo
  install`, persist to tmnl config).
- Palette commands: `launcher.open <id>`, one per configured
  entry. Discoverable via `Ctrl+Shift+P` → "launcher".
- Config: new `[[ui.launcher_icon]]` array in
  `~/.config/tmnl/config.toml`. Port the struct from mnml's
  `LauncherIcon` (id / glyph / fallback / command / color /
  tooltip). The mnml-side `command` field accepts a registered
  command id OR a `:cmdline` string — tmnl only needs the
  binary-path / shell-command path.
- Composes with vert-tabs mode: launcher rail sits BETWEEN the
  window edge and the vert-tabs sidebar (or absorbed into it
  with a `── LAUNCHERS ──` section header — pick one).

Open questions:

- Click-in-split vs always-new-tab? mnml uses always-tab.
  Probably fine to start there.
- Width override (drag-to-resize like the vert-tabs sidebar)?
  Probably not needed for a fixed-width icon strip.
- Auto-detect installed siblings (port mnml's
  `integration_detect.rs`) vs config-only? Auto-detect feels
  better; ~50 lines of pure logic.

## Powerline prompt auto-wire

The themed prompt (`themes/mnml-prompt.sh`, commit `56a7aa3`) is
shipped + auto-installed to `~/.config/mnml/prompt.sh`, and tmnl
exports `MNML_PROMPT_SCRIPT` to every spawned shell. But it's
opt-in: the user has to add one line to their `.zshrc`:

```
[ -n "$MNML_PROMPT_SCRIPT" ] && source "$MNML_PROMPT_SCRIPT"
```

If they haven't, tmnl shells get the user's normal prompt — which
makes it look like "we never implemented the powerline thing".
Options to make it actually surface:

1. **First-run prompt overlay** — on tmnl startup, if `~/.zshrc`
   (and/or `.bashrc`) is missing the source line, show an overlay
   offering to append it. Like JetBrains' "install shell
   integration" flow. One click → done forever.
2. **Wrapper shell invocation** — spawn `zsh -c "source
   $MNML_PROMPT_SCRIPT; exec zsh -i"` instead of `zsh -l`. Forces
   the prompt without touching the user's rc files. Risk: bypasses
   `.zshrc`'s normal init order, may break some users' setups.
3. **Per-shell ENV-based prompt** — set `PROMPT=...` env var
   directly before spawn. zsh-only; bash uses `PS1`. Less elegant
   than the script but truly zero-config.

Option 1 is the most polite + most discoverable. Worth a small
dedicated overlay (similar to mnml's settings-row text-edit).

**Also surface as a Settings row.** New discrete-choice row in
the existing settings overlay:

```
── Shell ──
▸ Themed prompt: [off] / on
```

Behavior on toggle to `on`:

1. Set `cfg.shell.themed_prompt = true` (new config field).
2. Check `~/.zshrc` (and `.bashrc` if it exists). If neither
   contains the source line, append it + show a toast: "Added
   source line to ~/.zshrc — open a new tab to see it."
3. From now on, new shell spawns export `MNML_PROMPT_SCRIPT`
   as today; the rc-file line picks it up.

Behavior on toggle to `off`:

1. Set `cfg.shell.themed_prompt = false`.
2. New shell spawns DON'T export `MNML_PROMPT_SCRIPT`, so the
   rc-file source line silently no-ops. Leave the rc line in
   place — toggling back on is then just env-var flip, no rc
   edit. (Removing the line on `off` is more invasive than the
   user expects from a toggle.)

Theme awareness: now that `theme.rs` adopts mnml's installed
theme at startup, tmnl could ALSO export the full palette
env-var set (`MNML_PROMPT_BG`, `MNML_PROMPT_FG`, …) so the
prompt colors match the active mnml theme — same way mnml does
it. Currently we deliberately omit those, letting the script's
tokyo-night defaults take over. Probably worth adding once the
settings row exists (one more `if cfg.shell.themed_prompt` block
in `shell_prompt::env_vars`).

---

## Find (Cmd+F in scrollback)

Search visible + buffered scrollback for a query string. Surfaces:

- `Cmd+F` opens an inline find bar (along the top strip? or bottom?).
- Live highlight of every match in the visible scrollback as the
  user types (cell pipeline glyph attrs — yellow bg, dim fg).
- `Enter` / `Cmd+G` jumps to next match; `Shift+Enter` / `Shift+Cmd+G`
  goes back. Wrap-around with a status hint.
- `Esc` closes the find bar + clears highlights.
- Per-pane state (each ShellSession has its own scrollback, so the
  find bar applies to the FOCUSED pane).
- Match count chip (`3 of 12`) in the find bar.

Open questions:

- Regex vs literal? Default literal with a toggle for regex.
- Case sensitivity? Default smart-case (lowercase query → case-insensitive).
- How far back to search — just the visible viewport, the full
  scrollback buffer, or both with a "search older" affordance?
- Does the body grid need a "find overlay" pipeline layer, or can
  the existing cell pipeline carry the highlight via per-cell
  attribute bits? The latter is simpler if there's a free attr bit.
