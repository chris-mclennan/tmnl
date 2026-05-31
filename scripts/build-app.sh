#!/usr/bin/env bash
#
# Build tmnl.app — a macOS app bundle for the GPU-rendered terminal.
#
#   ./scripts/build-app.sh                    # debug profile, builds target/tmnl.app
#   ./scripts/build-app.sh release            # release profile
#   ./scripts/build-app.sh --bin-path PATH    # skip cargo build, use this binary
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
        *)
            echo "usage: $0 [debug|release] [--bin-path PATH]" >&2
            exit 2
            ;;
    esac
done

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

APP="target/tmnl.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_PATH" "$APP/Contents/MacOS/tmnl"
cp scripts/Info.plist "$APP/Contents/Info.plist"

# App icon — built on demand if missing (no external image-tool deps;
# scripts/icon/gen_icon.swift draws from scratch in AppKit).
if [ ! -f scripts/icon/AppIcon.icns ]; then
    echo "building app icon…"
    (cd scripts/icon && ./build.sh) >/dev/null
fi
cp scripts/icon/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

# Strip the quarantine bit so Finder doesn't Gatekeeper-block the
# first launch. Best-effort.
xattr -d com.apple.quarantine "$APP" 2>/dev/null || true

echo "built $APP"
echo "launch: open $APP"
