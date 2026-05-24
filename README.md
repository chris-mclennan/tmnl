<div align="center">

# tmnl

**A GPU-rendered terminal for macOS — and a display surface apps can draw to.**

Every terminal renders ANSI byte streams. tmnl does that *and* acts as a clean
rendering target apps draw to the way a GUI app draws to a window — typed cells,
true-color, partial-frame diffs, no escape-sequence tax.

[![Crates.io](https://img.shields.io/crates/v/tmnl-rs.svg?logo=rust)](https://crates.io/crates/tmnl-rs)
[![CI](https://github.com/chris-mclennan/tmnl-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/chris-mclennan/tmnl-rs/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)](#install)

</div>

```
┌─ tmnl ───────────────────────────────────────────────────────────────┐
│   shell      mnml      + │
├───────────────────────────────────────────────────────────────────────┤
│ ~/code/project $ cargo build                                          │
│    Compiling tmnl v0.0.1                                              │
│     Finished `dev` profile in 4.21s                                   │
│ ~/code/project $ ▏                                                    │
│                                                                       │
│   shell mode: a real pty, drawn on the GPU                            │
│   native mode: an app sends typed cells — no ANSI in sight            │
└───────────────────────────────────────────────────────────────────────┘
```

<!-- Swap this mockup for a real screen recording before launch — tmnl draws
     its own GPU window, so it can't be a VHS capture; drop a .mov/.gif in assets/. -->

---

**tmnl** is two things in one binary:

1. **A terminal.** It hosts a real shell — a pty parsed with `vt100` — and draws
   it with `wgpu`. Native tabs, a native macOS menu bar, an in-grid settings
   panel.
2. **A display surface for apps.** Instead of speaking ANSI escape codes, an app
   connects to tmnl over a Unix socket and sends *structured cells* directly:
   typed glyphs, true-color, cursor state, partial-frame diffs. No pty, no
   escape-sequence parser, no ambiguity.

That second mode is the point. See [`docs/sdk-guide.md`](docs/sdk-guide.md).

> **Status:** `v0.0.1` — early, and macOS-only. Expect sharp edges.

## The two modes

|  | Shell mode | Native mode |
|--|-----------|-------------|
| Source of cells | a real pty + `vt100` parser | an app, over `tmnl-protocol` |
| The app speaks | ANSI escape codes | typed `Frame`s of cells |
| Use it for | a normal terminal | building a TUI without the ANSI tax |
| Reference | your `$SHELL` | [`mnml`](https://github.com/chris-mclennan/mnml-rs), `examples/hello_client.rs` |

Both modes feed the same `Grid`, and the same `wgpu` pipelines draw it — the
renderer doesn't care where cells came from.

## Highlights

- **GPU rendering** — a `wgpu` cell pipeline plus a chrome-strip pipeline;
  true-color cells, cursor shape & visibility.
- **Native tabs & macOS menu bar** — tmnl / Shell / Edit / View / Window / Help,
  with Mac-style editing chords.
- **Native mode** — `tmnl-protocol` over a Unix socket; partial-frame `DiffRun`
  updates put only changed cell-runs on the wire.
- **OSC 133 shell integration** — command lifecycle tracking and a command-line
  anchor.
- **Local AI command completion** — `⌘I` continuation and `⌘K` natural-language →
  command, offline via the embedded `fim-engine` model. Nothing leaves the
  machine.
- **Headless mode** (`--headless`) — scriptable cell-grid dumps for tests.

See [FEATURES.md](FEATURES.md) for the full shipped inventory and
[`.local/PLAN.md`](.local/PLAN.md) for the roadmap.

## Install

```bash
cargo install tmnl-rs        # the crate is tmnl-rs; the binary it installs is `tmnl`
```

tmnl is **macOS-only** for now (winit + wgpu are portable; the `muda` menu bar
and `.app` bundle are not). Linux is the likely next target.

## Build & run

```bash
cargo run --bin tmnl              # dev build — fast iteration
./scripts/build-app.sh            # bundle target/tmnl.app (debug)
./scripts/build-app.sh release    # bundle target/tmnl.app (release)
open target/tmnl.app              # launch the bundle
```

Use the `.app` bundle for the real macOS experience — the native menu bar and
dock icon only behave correctly when launched as a bundle. Plain `cargo run` is
fine for iterating on rendering or shell behaviour.

tmnl builds on stable Rust (MSRV **1.85**, edition 2024).

### `run.sh`

A wrapper with family-wide dev subcommands (shared with `mnml`, `mixr-rs`,
`internal-app`) plus tmnl-specific launch modes:

```bash
./run.sh                  # shell mode (default) — release build, opens a window
./run.sh build [args]     # cargo build [args]
./run.sh release [args]   # cargo build --release [args]
./run.sh test [args]      # cargo test [args]
./run.sh check            # cargo clippy --all-targets
./run.sh watch            # cargo watch -x build (needs cargo-watch)
./run.sh help             # print the full mode list

./run.sh mnml [WS]        # `tmnl --mnml` — editor mode; binds a UDS, spawns
                          # mnml with --blit <socket>. Pass-through args
                          # (workspace, --input vim, --ascii) go to mnml.
./run.sh headless         # `tmnl --headless` (no window; scripted stdin).
./run.sh no-launch        # `tmnl --mnml --no-launch` — editor mode, no
                          # auto-spawn (for attaching a debug-built mnml).
```

## Welcome screen

On a bare launch (no `--mnml`, not headless), if
`~/.config/tmnl/recents.toml` has entries, tmnl shows a centered bordered
overlay listing recent native-tab launches (mnml workspaces, mixr,
internal-app, …) so you can re-open one with a single keypress.

| Key | Action |
|-----|--------|
| `1`–`9`    | Open that recent entry as a new native tab |
| `↑` / `↓` / `k` / `j` | Move the selection |
| `Enter`    | Open the focused entry |
| `r`        | Drop the focused entry from recents |
| `Esc` / `n`| Dismiss (the shell-mode pane is already underneath) |

Recents are appended on every native-tab launch, de-duped by
`(command, args, workspace)`, capped at 20. File shape:

```toml
[[entry]]
command   = "/usr/local/bin/mnml"
args      = ["--input", "vim"]
workspace = "/Users/me/Projects/foo"
label     = "mnml — foo"
```

## Settings overlay

`Cmd+,` (also tmnl → Settings… in the menu bar) opens an in-grid settings
modal. Follows the family settings UI convention — `▸` focus marker,
`*` modified marker on rows that differ from defaults.

| Key | Action |
|-----|--------|
| `←` / `→`  | Adjust the focused row's value |
| `↑` / `↓`  | Move between rows |
| `r` / `⌫`  | Reset the focused row to its default |
| `R`        | Reset all rows to defaults |
| `Enter`    | Save and close |
| `Esc`      | Cancel — reverts to the opened-state snapshot |

Persisted to `~/.config/tmnl/config.toml`.

## Themed shell prompt (optional)

When tmnl spawns a shell (default shell mode) it installs a small
powerline-style prompt script at `~/.config/mnml/prompt.sh` and
exports `$MNML_PROMPT_SCRIPT` + `MNML_CONTEXT=tmnl`. The same
script is shared with mnml — both write the same content, so
turning it on in one place themes both. To enable, add one line to
your `~/.zshrc` (or `.bashrc`):

```sh
[ -n "$MNML_PROMPT_SCRIPT" ] && source "$MNML_PROMPT_SCRIPT"
```

Outside tmnl / mnml shells the env var is unset and the line is a
no-op. The prompt shows `cwd · git branch (± when dirty) · exit
code (only when non-zero) · clock · context-chip`. Pure bash/zsh
+ ANSI — no external binary required.

## Pty-fd handoff (receiver)

tmnl is a receiver for SCM_RIGHTS pty-fd transfers — a child native-mode
client (e.g. `mnml` via `:tmnl.pop-pty`) can hand the master fd of a
running pty into tmnl, where it lands as a new adopted shell tab.

Mechanism: tmnl binds a dedicated transfer socket at
`<TMPDIR>/tmnl-<pid>-transfer.sock` and exports the path via the
`TMNL_TRANSFER_SOCKET` env var; children inherit it and connect there
to send one `Message::OpenPaneTransfer` with the fd attached.

## Native mode / the SDK

An app becomes a tmnl "backing app" by speaking the
[`tmnl-protocol`](https://github.com/chris-mclennan/tmnl-protocol) wire format
over a Unix socket. The protocol is small — a handful of message types, a binary
cell layout, and a diff-run frame encoding.

- **Guide** — [`docs/sdk-guide.md`](docs/sdk-guide.md): handshake, message
  reference, frame/diff semantics, a worked example.
- **Template** — [`examples/hello_client.rs`](examples/hello_client.rs): a
  minimal, commented backing app to copy and grow.
- **Crate** — [`tmnl-protocol`](https://github.com/chris-mclennan/tmnl-protocol):
  the wire types and `read_message` / `write_message`.

Smoke-test both sides of the protocol without a GPU window:

```bash
cargo run --example fake_server -- /tmp/t.sock   # tmnl stub
cargo run --example fake_client -- /tmp/t.sock   # backing-app stub
```

## Architecture

```
            ┌──────────────┐
 shell mode │ pty + vt100  │──┐
            └──────────────┘  │   ┌──────┐   ┌──────────────────┐
                              ├──▶│ Grid │──▶│ wgpu pipelines   │──▶ window
            ┌──────────────┐  │   └──────┘   │ (cell + strip)   │
native mode │ socket +     │──┘              └──────────────────┘
            │ tmnl-protocol│
            └──────────────┘
```

`Grid` (cells) is the single source of truth. Everything upstream is a cell
*producer*; everything downstream is the renderer. Adding a feature usually
means asking "is this a producer change or a renderer change?"

## The tmnl family

tmnl is one of a small family of terminal-native Rust tools:

| Project | What it is | |
|---------|-----------|--|
| **tmnl** | A GPU-accelerated terminal | ← you are here |
| [**mnml**](https://github.com/chris-mclennan/mnml-rs) | A terminal IDE | runs as a native tmnl tab |
| [**mixr**](https://github.com/chris-mclennan/mixr-rs) | A terminal DJ app | runs as a native tmnl tab |
| [**tmnl-protocol**](https://github.com/chris-mclennan/tmnl-protocol) | The binary wire protocol | native mode's wire format |
| [**fim-engine**](https://github.com/chris-mclennan/fim-engine) | Embedded code completion | powers tmnl's ⌘I completion |

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for the
workflow and conventions. The roadmap lives in
[`.local/PLAN.md`](.local/PLAN.md) and the release history in
[CHANGELOG.md](CHANGELOG.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
