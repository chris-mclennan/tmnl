#!/usr/bin/env bash
# tmnl wrapper — family-common dev subcommands + tmnl-specific launch modes.
# Family convention: `build`/`release`/`test`/`check`/`watch`/`help` are
# shared across mnml + tmnl + mixr-rs + internal-app.
#
# Usage:
#   ./run.sh                      Run tmnl (shell mode — opens a window with
#                                 $SHELL). Uses the release profile.
#
# Common dev subcommands (family-wide):
#   ./run.sh build [args]         cargo build [args]
#   ./run.sh release [args]       cargo build --release [args]
#   ./run.sh test [args]          cargo test [args]
#   ./run.sh check                cargo fmt --check + cargo clippy
#                                 --all-targets  (matches CI)
#   ./run.sh watch                cargo watch -x build  (needs cargo-watch)
#   ./run.sh help                 show this
#
# tmnl-specific modes:
#   ./run.sh mnml [WORKSPACE]     Launch tmnl with mnml as a native tab
#                                 (`tmnl --mnml`). tmnl binds a UDS, spawns
#                                 mnml with `--blit <socket>`, renders into
#                                 its own wgpu window. Pass-through args
#                                 (workspace path, --input, --ascii) go to
#                                 the spawned mnml.
#   ./run.sh mixr [args...]       Launch tmnl with mixr as a native tab
#                                 (`tmnl --mixr`). Same machinery as `mnml`
#                                 mode but resolves to the mixr binary and
#                                 defaults to `--dashboard`.
#   ./run.sh headless             tmnl --headless (no window, scripted stdin).
#   ./run.sh no-launch            tmnl --mnml --no-launch — editor mode but
#                                 don't auto-spawn; useful for manually
#                                 attaching a debug-built mnml.
#
# Env:
#   TMNL_INSET=N   override the pixel inset around the shell-prompt view
#                  (also editable via Cmd+, → Settings overlay).
set -o pipefail
cd "$(dirname "$0")"

case "${1:-default}" in
  # ── Family-wide dev subcommands ─────────────────────────────────
  build)   shift; exec cargo build "$@" ;;
  release) shift; exec cargo build --release "$@" ;;
  test)    shift; exec cargo test "$@" ;;
  check)
    cargo fmt --check || exit 1
    exec cargo clippy --all-targets
    ;;
  watch)
    if ! command -v cargo-watch >/dev/null 2>&1; then
      echo "[run.sh] cargo-watch not installed — \`cargo install cargo-watch\`" >&2
      exit 1
    fi
    exec cargo watch -x build
    ;;
  # ── tmnl-specific modes ─────────────────────────────────────────
  mnml)
    shift
    cargo build --release --quiet
    if [ -d ../mnml ]; then
      (cd ../mnml && cargo build --release --quiet) || \
        echo "[run.sh] warning: mnml release build failed; tmnl will fall back to stale binary" >&2
    fi
    exec ./target/release/tmnl --mnml "$@"
    ;;
  mixr)
    shift
    cargo build --release --quiet
    if [ -d ../mixr-rs ]; then
      (cd ../mixr-rs && cargo build --release --quiet) || \
        echo "[run.sh] warning: mixr release build failed; tmnl will fall back to stale binary" >&2
    fi
    exec ./target/release/tmnl --mixr "$@"
    ;;
  headless)
    cargo build --release --quiet
    exec ./target/release/tmnl --headless
    ;;
  no-launch)
    cargo build --release --quiet
    exec ./target/release/tmnl --mnml --no-launch
    ;;
  -h|--help|help) grep -E '^# ' "$0" | sed 's/^# \?//'; exit 0 ;;
  # ── Default — shell mode ────────────────────────────────────────
  default)
    cargo build --release --quiet
    exec ./target/release/tmnl
    ;;
  # Unknown — pass through to cargo run for one-off testing.
  *)
    exec cargo run --release -- "$@"
    ;;
esac
