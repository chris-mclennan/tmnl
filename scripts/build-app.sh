#!/usr/bin/env bash
#
# Build tmnl.app — a hand-rolled macOS app bundle.
#
#   ./scripts/build-app.sh          # debug profile, builds target/tmnl.app
#   ./scripts/build-app.sh release  # release profile
#
# Launch with:  open target/tmnl.app
#
# When run via `open`, working directory is /. The workspace resolver in
# launcher.rs falls back to $HOME so mnml opens somewhere useful.

set -euo pipefail

cd "$(dirname "$0")/.."

PROFILE="${1:-debug}"
case "$PROFILE" in
    debug)
        cargo build --bin tmnl
        ;;
    release)
        cargo build --release --bin tmnl
        ;;
    *)
        echo "usage: $0 [debug|release]" >&2
        exit 2
        ;;
esac

APP="target/tmnl.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "target/$PROFILE/tmnl" "$APP/Contents/MacOS/tmnl"
cp scripts/Info.plist "$APP/Contents/Info.plist"

# Strip the quarantine bit set by some build environments so Finder doesn't
# Gatekeeper-block the first launch. Best-effort.
xattr -d com.apple.quarantine "$APP" 2>/dev/null || true

echo "built $APP"
echo "launch: open $APP"
