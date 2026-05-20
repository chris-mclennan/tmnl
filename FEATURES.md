# tmnl — features & roadmap

Two tracks. **Native mode** is the differentiator and where the design
energy goes. **Shell mode** needs to clear table stakes so tmnl is a
credible daily driver — but it should not turn into a race to out-feature
WezTerm/Ghostty, which is unwinnable.

## Shipped

- [x] Shell mode — hosts a real pty, parses output with `vt100`
- [x] Native mode — `tmnl-protocol` v3 over a Unix socket
- [x] GPU rendering — `wgpu` cell pipeline + chrome strip pipeline
- [x] True-color cells (rgba fg/bg)
- [x] Cursor shape + visibility
- [x] Mouse input — click, drag, move, scroll (4 directions)
- [x] Native tabs
- [x] Native macOS menu bar (tmnl / Shell / Edit / View / Window / Help)
- [x] Mac-style editing chords (⌘Z/X/C/V/A/S/F/N)
- [x] In-grid Settings modal, persisted to `~/.config/tmnl/config.toml`
- [x] Tab titles set by the backing app (`Message::Title`, protocol v3)
- [x] Partial-frame updates (`DiffRun` — only changed cell runs on the wire)
- [x] Configurable window size + prompt inset
- [x] OSC 133 shell integration — command lifecycle + command-line anchor
- [x] Local AI command completion — ⌘I continuation + ⌘K NL→command
      (`fim-engine`, offline qwen2.5-coder)
- [x] Headless mode (`--headless`) — scriptable cell-grid dumps for tests

## Planned — table stakes (shell mode)

Ordered roughly by impact-to-effort. The goal is "nobody bounces," not
feature maximalism.

- [ ] **Scrollback + search** — scroll history and find within it
- [ ] **Splits / panes** — the single biggest "why I can't switch" gap
- [~] **Shell integration (OSC 133)** — parsing + command-lifecycle
      tracking landed (`src/osc133.rs`, see autosuggestion Phase 1);
      jump-to-prompt and a command-status UI still to do
- [ ] **Clickable URLs** — plain-text detection + OSC 8 hyperlinks
- [ ] **Color schemes / theming** — bundled themes + user themes
- [ ] **Font config** — family, size, fallback, ligatures
- [ ] **Keybinding remapping** — user-defined chords
- [ ] **Selection + copy/paste polish** — block select, auto-copy option
- [ ] **Cross-platform** — currently macOS-only; Linux is the likely next
      target (winit + wgpu already portable; `muda` menu + `.app` are not)
- [ ] Image protocols (Kitty graphics / iTerm / Sixel) — evaluate later

## Planned — native mode (the differentiator)

- [ ] **Published SDK** — an ergonomic client layer in `tmnl-protocol`
      (connect + handshake + frame builder) so a backing app is ~20 lines,
      not ~100. See [`docs/sdk-guide.md`](docs/sdk-guide.md). *In progress.*
- [ ] **A second reference client** — something other than mnml targeting
      the protocol (a file picker, a git UI). Until this exists, "TUI
      runtime" is aspirational; the protocol is just mnml's renderer.
- [ ] **Capability negotiation** — `Hello` carries a feature set so the
      protocol can grow without breaking older clients
- [ ] **Richer input** — hover regions, focus events, IME/composition
- [ ] **Embedded content** — images / inline widgets in a native frame,
      something a pty terminal cannot express cheaply
- [ ] **Latency benchmark** — publish input→frame latency vs a pty
      terminal. If native mode is visibly snappier, that *is* the pitch.

## Autosuggestion & autocomplete

Two distinct things, settled after working through the architecture:

**Inline history autosuggestion** — fish-style ghost text from your
shell history. The shell already does this well, so tmnl just documents
`zsh-autosuggestions` (see `docs/shell-integration.md`). A tmnl-native
re-implementation would be redundant, so there isn't one.

**AI command completion** — local, offline, private. A quantized
qwen2.5-coder model (the `fim-engine` sibling crate, candle inference,
in-process) completes a half-typed command. This is the differentiated
feature: Warp's AI is cloud; this never leaves the machine.

- [x] **Phase 0** — `zsh-autosuggestions` documented in
      `docs/shell-integration.md`. History ghost text today, zero tmnl
      code.
- [x] **Phase 1** — OSC 133 parsing. tmnl scans semantic-prompt marks
      and tracks command lifecycle + the command-line anchor.
      `src/osc133.rs`, `shell-integration/tmnl.zsh`.
- [x] **Stage 1 — AI continuation.** ⌘I completes the current command
      line via `fim-engine` (local qwen2.5-coder, on-demand). The result
      renders as dim ghost text; Tab accepts, any other key dismisses.
      `src/fim.rs`. Needs the OSC 133 snippet installed (it supplies the
      command-line anchor). On-demand because CPU inference is ~0.3–1.6s;
      inline-as-you-type waits on `fim-engine`'s Metal-acceleration
      follow-up.
- [x] **Stage 2 — NL→command.** ⌘K turns a natural-language description
      typed on the prompt into a shell command — `fim-engine` with a
      shebang-shaped FIM prompt (`#!/bin/zsh` + the description as a
      comment), so no `fim-engine` change was needed. The command
      previews as dim ghost text on the row below; Tab accepts (erases
      the description, types the command).

Dropped along the way: a tmnl-native *history* ghost text (redundant
with `zsh-autosuggestions`), and a "native-mode suggestion API" (native
mode has no command line — a backing app renders its own suggestions).
