# tmnl

A wgpu-rendered macOS terminal. Two modes share one renderer:

- **Shell mode** — hosts a real pty via `portable-pty`, parses output with
  `vt100`, writes cells into the same `Grid` the renderer reads from.
- **Native mode** — speaks the binary `tmnl-protocol` wire format over a
  Unix socket to a backing app (e.g. `mnml`). The backing app sends
  `Frame`s of cells; tmnl sends `InputEvent`s back.

Either way, the cell `Grid` is the source of truth and the wgpu cell +
strip pipelines draw it.

## Workspace layout

```
tmnl/                 ← this crate (the app binary)
  src/
    main.rs           ← winit event loop + mode arbitration
    grid.rs           ← Cell + Grid (source of truth for what's on screen)
    atlas.rs          ← font glyph atlas (fontdue + GPU texture)
    pipeline.rs       ← cell pipeline (text)
    strip.wgsl        ← chrome strip shader (tab chips, traffic-light gap)
    cell.wgsl         ← cell shader
    shell.rs          ← pty host + vt100 parser → Grid
    osc133.rs         ← OSC 133 semantic-prompt parsing (shell mode)
    fim.rs            ← local AI command-completion worker (fim-engine)
    headless.rs       ← `--headless` text-dump mode (verification harness)
    server.rs         ← Unix-socket server for native mode
    launcher.rs       ← spawns the backing app for native mode
    menu.rs           ← native macOS menu bar (muda)
    settings_ui.rs    ← in-grid Settings modal
    config.rs         ← ~/.config/tmnl/config.toml persistence
  docs/
    sdk-guide.md      ← how to build a native-mode backing app
  examples/
    fake_server.rs    ← tmnl stub: binds socket, sends input, prints frames
    fake_client.rs    ← backing-app stub: connects, streams frames
    hello_client.rs   ← minimal backing-app template (SDK quickstart)
  scripts/
    build-app.sh      ← bundles target/tmnl.app
    Info.plist
  shell-integration/
    tmnl.zsh          ← OSC 133 snippet for the user's ~/.zshrc
  FEATURES.md         ← feature matrix + roadmap
../tmnl-protocol/     ← sibling crate, wire format types (path dep)
../fim-engine/        ← sibling crate, local AI completion engine (path dep)
```

## Build & run

```bash
cargo run --bin tmnl              # dev — runs as a plain binary
./scripts/build-app.sh            # bundle target/tmnl.app (debug)
./scripts/build-app.sh release    # bundle release
open target/tmnl.app              # launch the bundle
cargo run --bin tmnl -- --headless  # no window; scripted stdin + grid dumps
```

The `./run.sh` wrapper has the family-wide dev subcommands
(`build`/`release`/`test`/`check`/`watch`/`help`) plus tmnl-specific
launch modes (`mnml [WS]` / `headless` / `no-launch`). Default is shell
mode — release profile, opens a window with `$SHELL`. See README's
`run.sh` section for the full list.

## Verifying shell-mode changes

`--headless` runs a shell session with no GPU window, takes `type` /
`key` / `wait` / `dump` / `fim` / `quit` commands on stdin, and prints
the rendered cell `Grid` as text (`src/headless.rs`). This is how to
verify shell-mode rendering without a window — use the `/smoke` skill, or:

```bash
printf 'type echo hi\nkey enter\nwait 500\ndump\nquit\n' | \
  cargo run --bin tmnl -- --headless
```

The `fim` command exercises AI command completion end-to-end (source
`shell-integration/tmnl.zsh` first so the OSC 133 anchor exists).

## AI command completion

Two shell-mode AI features, both via `fim-engine` (local qwen2.5-coder,
offline) and a worker thread in `src/fim.rs` the App polls in `tick`.
Both reconstruct the command line between the OSC 133 `B` anchor and the
cursor, so both need the integration snippet installed — without an
anchor they are silent no-ops.

- **⌘I — continuation.** Completes the half-typed command. The command
  line is the FIM prefix; the result renders as dim ghost text *at the
  cursor*. Tab accepts (appends), any other key dismisses.
- **⌘K — NL→command.** Treats the command line as a natural-language
  description, wraps it in a shebang-shaped FIM prompt, and previews the
  generated command as dim ghost text *on the row below*. Tab accepts
  (erases the description, types the command).

The `ghost` / `PendingReq` `erase` + `below` fields carry which mode a
suggestion is: `erase=0,below=false` is ⌘I, `erase>0,below=true` is ⌘K.

The `.app` bundle is needed for the native macOS menu bar + dock icon to
behave correctly. Plain `cargo run` works for fast iteration but loses the
menu bar.

## Conventions visible in the code

- **Heavy `//!` and `///` doc comments** explain *why* a thing exists,
  not just what it does — see e.g. `menu.rs`, `settings_ui.rs`,
  `shell.rs` headers. Match that style when adding modules.
- **macOS-specific bits are `#[cfg(target_os = "macos")]`-guarded** with
  a `#[cfg(not(target_os = "macos"))]` fallback (usually a `0.0` constant
  or stub). See the `MACOS_TAB_STRIP_PX_*` constants in `main.rs`.
- **Two pipelines, no overlap.** The strip pipeline paints the top chrome
  band (tab chips + traffic-light gap); the cell pipeline draws below it
  offset by `inset_px + gpu.strip_h`. Don't draw cells into the strip.
- **Crate versions are shared with sibling repos.** `muda` matches
  `mixr-rs`, `vt100` + `portable-pty` match `mnml`'s `pty_pane`. If you
  bump one here, check the sibling first.
- **Restart exit code is 75** (see `launcher.rs`). The launcher relaunches
  the backing app on this code.

## Protocol smoke test

```bash
# Terminal A — tmnl stub: binds the socket, sends scripted input
cargo run --example fake_server -- /tmp/test-tmnl.sock

# Terminal B — backing-app stub: connects and streams frames
cargo run --example fake_client -- /tmp/test-tmnl.sock
```

Or use the `/fake-protocol` skill to start both in parallel.

`examples/hello_client.rs` is the minimal, well-commented backing-app
template — the starting point for anyone writing a native-mode client.
See `docs/sdk-guide.md` for the protocol walkthrough.

## Protocol roles (don't mix these up)

The **server** binds the Unix socket; the **client** connects to it.

- **tmnl is the server** — binds the socket, owns the window + GPU. Sends
  `Hello`, `Resize`, `Input`. Receives `Frame`, `Title`.
- **The backing app is the client** — connects to the socket. Sends
  `Hello`, `Frame`, `Title`. Receives `Resize`, `Input`.

## Pty-fd handoff (receiver, task #50)

tmnl receives running ptys from native-mode clients via SCM_RIGHTS. A
dedicated standalone listener (`src/transfer.rs`) binds
`<TMPDIR>/tmnl-<pid>-transfer.sock` at startup — the path is exported
via the `TMNL_TRANSFER_SOCKET` env var **before** any thread spawn or
child `exec` in `main()` (children inherit the env; `Launcher::spawn`
doesn't strip it), so the mnml client can find the socket. Each
accepted connection reads exactly one `Message::OpenPaneTransfer` with
an attached pty master fd via
`tmnl_protocol::read_message_with_fd`; the fd is wrapped in
`ShellSession::adopt_fd` and surfaces as a new adopted-shell tab on the
next `tick`.

The sender side lives in mnml as `:tmnl.pop-pty` (task #49). The
transfer can't ride the streaming `tmnl-protocol` connection — SCM_RIGHTS
ancillary data can't be read through a `BufReader` — hence the separate
single-message socket.

## Settings persistence

`~/.config/tmnl/config.toml`, loaded at startup by `config::Config::load`.
CLI flags + env vars still win (escape hatches for one-off launches); the
Settings window edits and persists this file.

## Welcome screen + recents

`~/.config/tmnl/recents.toml` — every native-tab launch (mnml + workspace,
mixr, internal-app, etc.) is appended to this file by
`open_pane_with_command`. Capped at `MAX_RECENTS = 20`; de-duped by
`(command, args, workspace)` tuple so a re-launch bumps the existing
entry to the top of the list (most-recent-first).

On a bare `tmnl` launch (no `--mnml`, not headless) when recents has
entries: tmnl shows a centered bordered welcome overlay listing the
recent launches numbered 1-9. Keys:

  1-9        — open that recent entry as a new native tab
  ↑/↓ / k/j  — move the selection
  Enter      — open the focused entry
  r          — drop the focused entry from recents
  Esc / n    — dismiss (keep the shell-mode pane underneath)

The overlay is purely additive — the shell-mode pane is already there
under it, so dismissing drops you straight into the shell. Recently-
opened TUIs reappear with a single keypress, no path-typing.

## Family settings UI convention

tmnl's Settings modal (Cmd+, → `src/settings_ui.rs`) follows the family
settings UI convention shared with mnml + mixr — `▸` focus marker, `*`
modified marker (lights when the row differs from `Config::default()`),
`←→` adjust value, `r` reset focused row, `R` reset all (placeholder
matching family convention; same as `r` while there's only one
setting), `Enter` save + close, `Esc` cancel (reverts via the
`SettingsState.original` snapshot). `⌫` / `Delete` kept as an alias
for `r` (muscle memory). The full sectioned-list shape (section
headers, `[bracket]` choices) doesn't apply yet — with one numeric
setting (`inset`) the modal stays single-row. Numeric-row support is a
v2 convention extension; the modal will graduate to the full sectioned
list when tmnl grows more settings (font size, cursor style, …).
