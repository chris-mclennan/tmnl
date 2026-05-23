# tmnl — Plan & Roadmap

Working roadmap and design notes. Shipped features live in
[`FEATURES.md`](../FEATURES.md); the user-facing summary in
[`CHANGELOG.md`](../CHANGELOG.md).

Two tracks. **Native mode** is the differentiator and gets the design energy.
**Shell mode** clears table stakes so tmnl is a credible daily driver — the goal
there is "nobody bounces," not feature maximalism.

---

## Roadmap — table stakes (shell mode)

Ordered roughly by impact-to-effort.

- [ ] **Scrollback + search** — scroll history and find within it.
- [ ] **Splits / panes** — the single biggest "why I can't switch" gap. Design
      notes in [`docs/splits-plan.md`](../docs/splits-plan.md).
- [~] **Shell integration (OSC 133)** — parsing + command-lifecycle tracking
      landed; jump-to-prompt and a command-status UI still to do.
- [ ] **Clickable URLs** — plain-text detection + OSC 8 hyperlinks.
- [ ] **Color schemes / theming** — bundled themes + user themes.
- [ ] **Font config** — family, size, fallback, ligatures.
- [ ] **Keybinding remapping** — user-defined chords.
- [ ] **Selection + copy/paste polish** — block select, auto-copy option.
- [ ] **Cross-platform** — currently macOS-only; Linux is the likely next target
      (winit + wgpu already portable; the `muda` menu + `.app` bundle are not).
- [ ] **Image protocols** — Kitty graphics / iTerm / Sixel; evaluate later.

## Roadmap — native mode (the differentiator)

- [~] **Published SDK** — an ergonomic client layer in `tmnl-protocol` (connect +
      handshake + frame builder) so a backing app is ~20 lines, not ~100. See
      [`docs/sdk-guide.md`](../docs/sdk-guide.md).
- [ ] **A second reference client** — something other than mnml targeting the
      protocol (a file picker, a git UI). Until this exists, "TUI runtime" is
      aspirational; the protocol is just mnml's renderer.
- [ ] **Capability negotiation** — `Hello` carries a feature set so the protocol
      can grow without breaking older clients.
- [ ] **Richer input** — hover regions, focus events, IME / composition.
- [ ] **Embedded content** — images / inline widgets in a native frame, something
      a pty terminal cannot express cheaply.
- [ ] **Latency benchmark** — publish input→frame latency vs a pty terminal. If
      native mode is visibly snappier, that *is* the pitch.

## Design notes

### Autosuggestion & autocomplete

Two distinct things, settled after working through the architecture:

- **Inline history autosuggestion** — fish-style ghost text from shell history.
  The shell already does this well, so tmnl just documents `zsh-autosuggestions`
  (see [`docs/shell-integration.md`](../docs/shell-integration.md)). A
  tmnl-native re-implementation would be redundant — so there isn't one.
- **AI command completion** — local, offline, private. A quantized qwen2.5-coder
  model (the `fim-engine` sibling crate, candle inference, in-process) completes
  a half-typed command. *This* is the differentiated feature: Warp's AI is cloud;
  this never leaves the machine. Shipped — `⌘I` continuation, `⌘K` NL→command.

Dropped along the way: a tmnl-native *history* ghost text (redundant with
`zsh-autosuggestions`), and a "native-mode suggestion API" (native mode has no
command line — a backing app renders its own suggestions).

### Scope discipline

Shell mode should not become a race to out-feature WezTerm / Ghostty — that race
is unwinnable. It needs to clear table stakes and stop. The differentiation is
native mode: a clean, structured rendering target that apps draw to directly.

## Refactor: shrink `main.rs`

**Problem.** `src/main.rs` is **3,724 lines** — the only outlier in tmnl. It mixes the entry point, the App struct, winit event handlers, the wgpu render-loop driver, and glue. The rest of the codebase is well-factored (everything else < 1 k lines), so this is small and contained — much smaller than mnml's parallel refactor.

**Approach.** Pull the App state + the event-loop body into a new `src/app.rs`; `main.rs` becomes just `fn main()` + the winit `EventLoop` bootstrap.

**Phases.**

- [ ] **1. Extract the App struct + its `impl`** into `src/app.rs`. `main.rs` `use crate::app::App;`. Verify build + the existing 92 tests.
- [ ] **2. Extract the winit handler body** (the bulk of the `EventLoop::run` closure) into `App::handle_event` or similar in `src/app.rs`. `main.rs`'s `EventLoop::run` becomes a thin shim that forwards events to `App`. Verify.
- [ ] **3. (Optional) further split** `src/app.rs` if it ends > 2 k lines — e.g. `src/app/render.rs` for any per-frame draw bits that didn't already move to `src/render/`.

**Target.** `src/main.rs` < 200 lines; `src/app.rs` < 2 k lines; no behaviour change; tests green; `tmnl --headless` still works.

**Out of scope.** Renaming things, splitting `src/shell.rs` (908 lines — fine), and unrelated reorg.
