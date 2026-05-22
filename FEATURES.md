# tmnl — Features

The shipped feature inventory. For the overview see [README.md](README.md); for
the roadmap and design notes see [`.local/PLAN.md`](.local/PLAN.md).

tmnl has two tracks. **Native mode** is the differentiator and where the design
energy goes. **Shell mode** needs to clear table stakes so tmnl is a credible
daily driver — without turning into a race to out-feature WezTerm / Ghostty.

---

## Rendering

- **GPU rendering** — a `wgpu` cell pipeline plus a separate chrome-strip
  pipeline.
- **True-color cells** — RGBA foreground and background per cell.
- **Cursor** — shape and visibility, driven by the cell source.
- **Configurable window** — size and prompt inset, persisted to
  `~/.config/tmnl/config.toml`.

## Shell mode

- **Real pty** — hosts your `$SHELL`, output parsed into cells with `vt100`.
- **Mouse input** — click, drag, move, and scroll (all four directions).
- **OSC 133 shell integration** — parses semantic-prompt marks to track the
  command lifecycle and the command-line anchor. See
  [`docs/shell-integration.md`](docs/shell-integration.md).
- **Local AI command completion** — entirely offline, nothing leaves the machine:
  - `⌘I` completes the half-typed command line (continuation).
  - `⌘K` turns a natural-language description on the prompt into a shell command.
  - Both run a quantized qwen2.5-coder model in-process via the embedded
    `fim-engine` crate; results render as dim ghost text, `Tab` accepts.

## Native mode

- **`tmnl-protocol` over a Unix socket** — apps send structured `Frame`s of
  cells instead of ANSI escape codes.
- **Partial-frame updates** — `DiffRun` puts only changed cell-runs on the wire.
- **App-set tab titles** — via `Message::Title`.
- **Reference client** — [`mnml`](https://github.com/chris-mclennan/mnml-rs) runs as
  a native tmnl tab; [`examples/hello_client.rs`](examples/hello_client.rs) is a
  minimal template.

## Window & chrome

- **Native tabs.**
- **Native macOS menu bar** — tmnl / Shell / Edit / View / Window / Help.
- **Mac-style editing chords** — `⌘Z` / `X` / `C` / `V` / `A` / `S` / `F` / `N`.
- **In-grid settings modal** — persisted to `~/.config/tmnl/config.toml`.

## Tooling

- **Headless mode** (`--headless`) — scriptable cell-grid dumps, so a piped
  command script doubles as a pass/fail test.
- **Protocol smoke harness** — `examples/fake_server` and `examples/fake_client`
  exercise both sides of `tmnl-protocol` without a GPU window.

---

**Roadmap** — scrollback & search, splits / panes, clickable URLs, theming, font
config, cross-platform support, and the native-mode differentiators (a published
SDK, a second reference client, capability negotiation) are tracked in
[`.local/PLAN.md`](.local/PLAN.md).
