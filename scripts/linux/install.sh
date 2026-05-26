#!/usr/bin/env bash
# tmnl Linux installer — builds + installs to `~/.cargo/bin/tmnl`,
# drops `tmnl.desktop` into `~/.local/share/applications/` (so the
# app launcher / activities overview finds it), and copies a PNG
# icon into `~/.local/share/icons/hicolor/256x256/apps/`.
#
# Run from the repo root (or `scripts/linux/`); paths are derived
# from the script's own location.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Where the user's per-user files live (XDG defaults).
USER_BIN="$HOME/.cargo/bin"
USER_APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
USER_ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"
USER_FONTS="${XDG_DATA_HOME:-$HOME/.local/share}/fonts"

echo "▶ building tmnl-rs (release) — this may take a few minutes on first build…"
cd "$REPO_ROOT"
cargo build --release --bin tmnl

mkdir -p "$USER_BIN" "$USER_APPS" "$USER_ICONS"
cp -f "$REPO_ROOT/target/release/tmnl" "$USER_BIN/tmnl"
echo "✓ installed binary → $USER_BIN/tmnl"

cp -f "$SCRIPT_DIR/tmnl.desktop" "$USER_APPS/tmnl.desktop"
echo "✓ installed launcher entry → $USER_APPS/tmnl.desktop"

# Best-effort PNG icon — convert the macOS icns iconset if available
# (the 256x256 PNG ships in the iconset directory). Skips silently if
# the iconset hasn't been generated (`scripts/icon/build.sh` is macOS
# only since it relies on `iconutil`).
if [ -f "$SCRIPT_DIR/../icon/AppIcon.iconset/icon_256x256.png" ]; then
    cp -f "$SCRIPT_DIR/../icon/AppIcon.iconset/icon_256x256.png" \
        "$USER_ICONS/tmnl.png"
    echo "✓ installed icon       → $USER_ICONS/tmnl.png"
else
    echo "  (skipped icon copy — run scripts/icon/build.sh on macOS first to generate the PNGs)"
fi

# Patch + install the Nerd Font for Claude/Codex glyphs if both
# FontForge AND the mnml repo's patcher script are present. Optional;
# the upstream JetBrainsMono Nerd Font works too, just without the
# branded glyphs in the INTEGRATIONS row.
MNML_PATCHER="$REPO_ROOT/../mnml/scripts/patch_nerd_font.py"
if command -v fontforge >/dev/null 2>&1 && [ -f "$MNML_PATCHER" ]; then
    SRC_FONT="$USER_FONTS/JetBrainsMonoNerdFontMono-Regular.ttf"
    OUT_FONT="$USER_FONTS/JetBrainsMonoNerdFontMono-Regular-mnml.ttf"
    if [ -f "$SRC_FONT" ] && [ ! -f "$OUT_FONT" ]; then
        echo "▶ patching JetBrainsMonoNerdFontMono with Claude/Codex glyphs…"
        fontforge -script "$MNML_PATCHER" \
            --font "$SRC_FONT" \
            --output "$OUT_FONT" \
            --glyph "$REPO_ROOT/../mnml/scripts/glyphs/claude_spark.svg:F8B0:claude:thin=15" \
            --glyph "$REPO_ROOT/../mnml/scripts/glyphs/codex.svg:F8B1:codex" \
            >/dev/null
        echo "✓ patched font       → $OUT_FONT"
        fc-cache -f "$USER_FONTS" >/dev/null 2>&1 || true
    fi
fi

# Tell the desktop environment about the new .desktop file. Best-effort
# — `update-desktop-database` may not be on PATH on minimal installs.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$USER_APPS" >/dev/null 2>&1 || true
fi

echo
echo "tmnl installed. Try:  tmnl"
echo "Or look for it in your app launcher / activities overview."
