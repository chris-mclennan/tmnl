# Command + Keymap migration (#58)

Tmnl is being incrementally refactored to mirror mixr's `Command` +
`Keymap` registry pattern (see `chris-mclennan/mixr#59`), adapted for
`winit::KeyEvent` instead of crossterm.

## What landed

Foundation only. Compiles + the keymap chord parser tests pass; no
chord is dispatched through the registry yet.

- `src/command.rs` — `Command` struct, `Registry`, `registry()`,
  `try_dispatch(ke, app, event_loop)`. Empty `builtin_commands` for now.
- `src/keymap.rs` — winit-flavored `Chord` (logical key + mods),
  `parse_key_spec` (accepts `cmd+t`, `cmd+shift+w`, `cmd+1`, etc.),
  `Keymap::build` walks the command registry, multi-value Vec per
  chord (same shape mixr ended up with).
- `App` gains a `keymap: Keymap` field built in `main.rs`.
- 2 unit tests pass.

## The shape

Tmnl chords are mostly `Cmd`-prefixed (macOS native window). Examples:

- `⌘T` — new tab
- `⌘W` — close tab / close pane (depends on tab kind)
- `⌘⇧W` — close focused split pane
- `⌘D` / `⌘⇧D` — split right / down
- `⌘1`-`⌘9` — jump to tab N
- `⌘⇧[` / `⌘⇧]` — cycle tabs
- `⌘,` — settings

There are no view modes the way mixr/mnml have — tmnl is a terminal
emulator hosting tabs. Modal contexts: welcome overlay, settings panel,
tab rename, focus arrows. Each is its own early-return at the top of
`handle_keyboard_input` (line 1548 in `src/app.rs`).

## Status: **complete** ✓

All Cmd-prefixed chords + ⌘⌥+Arrow + ⌘I/⌘K + Shift+PgUp/Dn
migrated. **39 commands** total in the registry. The legacy
`if self.mods.super_key() && ... { match (c, shift) { ... } }`
block in `handle_keyboard_input` is gone — a marker comment is
all that's left.

### Commands by group

| Group              | Commands |
|--------------------|----------|
| Tabs               | tab.new, tab.close_or_forward, tab.goto_1..9, tab.cycle_back, tab.cycle_forward |
| Splits             | pane.close, split.right, split.down, focus.left/right/up/down |
| View               | view.zoom_in/out/reset, scroll.page_up/down |
| AI                 | ai.completion, ai.generate |
| Forwarded chords   | fwd.cmd_z/x/c/v/a/s/f/n/p/b/g/slash |

Each Command has:
- `id` — namespaced identifier (`tab.goto_3`)
- `title` — user-facing description for help
- `group` — section in help/palette
- `keys` — default chord(s) as strings (`"cmd+shift+w"`)
- `run: fn(&mut App, &ActiveEventLoop, &KeyEvent)` — handler
- `when: Option<fn(&App) -> bool>` — context guard

### Modal handlers staying put

Like mixr, tmnl has modal contexts that greedily capture keys
and **intentionally don't migrate**:

- `welcome_handle_key` (welcome overlay)
- `settings_handle_key` (settings panel)
- `rename_handle_key` (tab title editing)
- The per-pane forwarding match in `handle_keyboard_input` for
  Shell/Native key dispatch (ordinary character typing → pty or
  protocol server, ghost-suggestion handling, etc.)

These aren't chords — they're modal text input that absorbs every
keystroke. A single fn-pointer `Command` doesn't model that. The
modal handler shape is the right answer for them.

### Future work

- Help screen UI (registry already populates via `help_rows()`,
  just needs a panel to render into — `view.help` command stub
  could open a centered overlay similar to mnml's).
- Command palette (filter the registry by title/id and run on
  Enter — would replace the native macOS menu bar's shortcut
  hints with an in-app discoverable list).
- Custom rebinding via `~/.config/tmnl/keys.toml` — the
  multi-value keymap already supports config-driven overlays
  (same as mixr/mnml); just needs a config schema + parse step.
