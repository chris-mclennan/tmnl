#!/usr/bin/env bash
#
# Build tmnl.app — a macOS app bundle for the GPU-rendered terminal.
#
#   ./scripts/build-app.sh                    # debug profile, builds target/tmnl.app
#   ./scripts/build-app.sh release            # release profile
#   ./scripts/build-app.sh --bin-path PATH    # skip cargo build, use this binary
#   ./scripts/build-app.sh --nightly          # builds target/tmnl-nightly.app
#                                             # (launcher always execs latest
#                                             # ~/Projects/tmnl/target/release/tmnl)
#
# Launch with:  open target/tmnl.app
#
# Bundle layout:
#   target/tmnl.app/Contents/
#     Info.plist
#     MacOS/tmnl                (the GPU binary — direct executable)
#     Resources/AppIcon.icns
#
# tmnl is a GUI app (winit + wgpu), so the binary is its own bundle
# executable — no launcher dispatch needed (the way mnml.app and
# mixr.app shim through `tmnl --mnml/--mixr` when tmnl is installed).
#
# `--bin-path` is for CI — cargo-dist has already built the binary
# at a known path; we just package it.

set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE="debug"
BIN_PATH=""
NIGHTLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        debug|release)
            PROFILE="$1"
            shift
            ;;
        --bin-path)
            BIN_PATH="$2"
            shift 2
            ;;
        --nightly)
            NIGHTLY=1
            shift
            ;;
        *)
            echo "usage: $0 [debug|release] [--bin-path PATH] [--nightly]" >&2
            exit 2
            ;;
    esac
done

if [ "$NIGHTLY" = 0 ]; then
    if [ -z "$BIN_PATH" ]; then
        case "$PROFILE" in
            debug)   cargo build --bin tmnl ;;
            release) cargo build --release --bin tmnl ;;
        esac
        BIN_PATH="target/$PROFILE/tmnl"
    fi
    if [ ! -f "$BIN_PATH" ]; then
        echo "error: binary not found at $BIN_PATH" >&2
        exit 1
    fi
fi

if [ "$NIGHTLY" = 1 ]; then
    APP="target/tmnl-nightly.app"
    PLIST_SRC="scripts/Info-nightly.plist"
    ICON_SRC="scripts/icon/AppIcon-nightly.icns"
else
    APP="target/tmnl.app"
    PLIST_SRC="scripts/Info.plist"
    ICON_SRC="scripts/icon/AppIcon.icns"
fi
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "$NIGHTLY" = 1 ]; then
    cp scripts/launcher-nightly.sh "$APP/Contents/MacOS/tmnl-nightly-launcher"
    chmod +x "$APP/Contents/MacOS/tmnl-nightly-launcher"
else
    cp "$BIN_PATH" "$APP/Contents/MacOS/tmnl"
fi
cp "$PLIST_SRC" "$APP/Contents/Info.plist"

# App icon — built on demand if missing (no external image-tool deps;
# scripts/icon/gen_icon.swift draws from scratch in AppKit).
if [ ! -f "$ICON_SRC" ]; then
    echo "building app icon ($ICON_SRC)…"
    if [ "$NIGHTLY" = 1 ]; then
        (cd scripts/icon && swift gen_icon.swift AppIcon-nightly.iconset nightly && iconutil -c icns AppIcon-nightly.iconset -o AppIcon-nightly.icns) >/dev/null
    else
        (cd scripts/icon && ./build.sh) >/dev/null
    fi
fi
cp "$ICON_SRC" "$APP/Contents/Resources/AppIcon.icns"

# Strip the quarantine bit so Finder doesn't Gatekeeper-block the
# first launch. Best-effort.
xattr -d com.apple.quarantine "$APP" 2>/dev/null || true

echo "built $APP"
echo "launch: open $APP"
