# tmnl

A GPU-rendered terminal for macOS — with a twist. tmnl is two things in one
binary:

1. **A terminal.** It hosts a real shell (a pty parsed with `vt100`) and
   draws it with `wgpu`. Tabs, a native macOS menu bar, a settings panel.
2. **A display surface for apps.** Instead of speaking ANSI escape codes,
   an app can connect to tmnl over a socket and send *structured cells*
   directly — typed glyphs, true-color, cursor state, partial-frame
   diffs. No pty, no escape-sequence parser, no ambiguity.

That second mode is the point. Every terminal renders ANSI byte streams;
tmnl can do that **and** act as a clean rendering target that apps draw
to the way a GUI app draws to a window. See [`docs/sdk-guide.md`](docs/sdk-guide.md).

> **Status:** v0.0.1, early and macOS-only. Expect sharp edges.

## The two modes

| | Shell mode | Native mode |
|---|---|---|
| Source of cells | a real pty + `vt100` parser | an app, over `tmnl-protocol` |
| The app speaks | ANSI escape codes | typed `Frame`s of cells |
| Use it for | a normal terminal | building a TUI without the ANSI tax |
| Reference | your `$SHELL` | [`mnml`](#), `examples/hello_client.rs` |

Both modes feed the same `Grid`, and the same `wgpu` cell + strip
pipelines draw it. The renderer doesn't care where cells came from.

## Build & run

```bash
cargo run --bin tmnl              # dev build — fast iteration
./scripts/build-app.sh            # bundle target/tmnl.app (debug)
./scripts/build-app.sh release    # bundle target/tmnl.app (release)
open target/tmnl.app              # launch the bundle
```

Use the `.app` bundle for the real macOS experience — the native menu bar
and dock icon only behave correctly when launched as a bundle. Plain
`cargo run` is fine for iterating on rendering or shell behavior.

## Shell mode

Shell mode runs your real `$SHELL`. For inline autosuggestions and OSC
133 semantic-prompt integration (so tmnl knows when a command is
running), see [`docs/shell-integration.md`](docs/shell-integration.md).

## Native mode / the SDK

An app becomes a tmnl "backing app" by speaking the `tmnl-protocol` wire
format over a Unix socket. The protocol is small — a handful of message
types, a binary cell layout, and a diff-run frame encoding.

- **Guide:** [`docs/sdk-guide.md`](docs/sdk-guide.md) — handshake,
  message reference, frame/diff semantics, a worked example.
- **Template:** [`examples/hello_client.rs`](examples/hello_client.rs) —
  a minimal, commented backing app you can copy and grow.
- **Crate:** `tmnl-protocol` (sibling crate) — the wire types and
  `read_message` / `write_message`.

Smoke-test both sides of the protocol without a GPU window:

```bash
cargo run --example fake_server -- /tmp/t.sock   # tmnl stub
cargo run --example fake_client -- /tmp/t.sock   # backing-app stub
```

## Features & roadmap

See [`FEATURES.md`](FEATURES.md) for what's shipped, the table-stakes
terminal features still planned, and the native-mode roadmap.

## Architecture

```
            ┌─────────────┐
 shell mode │ pty + vt100 │──┐
            └─────────────┘  │   ┌──────┐   ┌──────────────────┐
                             ├──▶│ Grid │──▶│ wgpu pipelines   │──▶ window
            ┌─────────────┐  │   └──────┘   │ (cell + strip)   │
native mode │ socket +    │──┘              └──────────────────┘
            │ tmnl-protocol│
            └─────────────┘
```

`Grid` (cells) is the single source of truth. Everything upstream is a
cell *producer*; everything downstream is the renderer. Adding a feature
usually means asking "is this a producer change or a renderer change?"

## Workspace

- `tmnl/` — this crate, the app binary.
- `tmnl-protocol/` — sibling crate, the native-mode wire format. Shared
  with backing apps so the protocol is defined exactly once.

## License

`tmnl-protocol` is MIT OR Apache-2.0. The `tmnl` app's license is not yet
decided.
