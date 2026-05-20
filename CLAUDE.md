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
    server.rs         ← Unix-socket server for native mode
    launcher.rs       ← spawns the backing app for native mode
    menu.rs           ← native macOS menu bar (muda)
    settings_ui.rs    ← in-grid Settings modal
    config.rs         ← ~/.config/tmnl/config.toml persistence
  examples/
    fake_server.rs    ← stub of a backing app (sends frames, prints input)
    fake_client.rs    ← stub of tmnl (sends input, prints frames)
  scripts/
    build-app.sh      ← bundles target/tmnl.app
    Info.plist
../tmnl-protocol/     ← sibling crate, wire format types (path dep)
```

## Build & run

```bash
cargo run --bin tmnl              # dev — runs as a plain binary
./scripts/build-app.sh            # bundle target/tmnl.app (debug)
./scripts/build-app.sh release    # bundle release
open target/tmnl.app              # launch the bundle
```

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
# Terminal A — pretend to be the backing app
cargo run --example fake_server -- /tmp/test-tmnl.sock

# Terminal B — drive it with the stub client
cargo run --example fake_client -- /tmp/test-tmnl.sock
```

Or use the `/fake-protocol` skill to start both in parallel.

## Settings persistence

`~/.config/tmnl/config.toml`, loaded at startup by `config::Config::load`.
CLI flags + env vars still win (escape hatches for one-off launches); the
Settings window edits and persists this file.
