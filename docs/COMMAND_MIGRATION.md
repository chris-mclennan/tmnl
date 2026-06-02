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

## Plan

1. **Phase 1 (done)**: Foundation files + App.keymap field.
2. **Phase 2**: Wire `try_dispatch` at the top of
   `handle_keyboard_input` (after the three modal early-returns and
   before the `if self.mods.super_key() && ...` Cmd-chord block).
3. **Phase 3**: Migrate Cmd chords one at a time. Each migration
   adds a `Command` entry + deletes the matching arm. Estimated 15-20
   chords total for tmnl (much smaller surface than mixr's 100+).
4. **Phase 4**: Help overlay generation — `help_rows()` already
   exists; once enough commands are registered, build a help screen
   that walks it.

## Modal handlers staying put

Like mixr, tmnl has modal contexts that need to greedily capture
keys: `welcome_handle_key`, `settings_handle_key`,
`rename_handle_key`. These won't migrate — they're inherently
greedy.
