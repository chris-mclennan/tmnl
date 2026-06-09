# tmnl backlog

Free-form list of features + polish items that aren't yet tracked
in a commit / branch. Add to the top; cross out (or delete) when
shipped. No severity / dates required — those live in commit
messages and `findings/` reports.

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
