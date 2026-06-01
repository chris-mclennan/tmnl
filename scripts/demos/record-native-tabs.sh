#!/usr/bin/env bash
# Record a ~25 s GIF demonstrating tmnl's native-tab feature.
#
#   shell tab + welcome overlay
#     -> press 2 to open mnml as a native tab
#     -> mnml renders for a few seconds
#     -> Cmd+T opens a fresh shell tab + welcome overlay
#     -> press 2 to open a second mnml native tab
#     -> Cmd+Shift+[ / Cmd+Shift+] to switch between the two
#
# Output: site/src/assets/demos/native-tabs.gif
# Working files in /tmp/tmnl-recording (kept after the run).
#
# Prereqs:
#   * macOS Screen Recording permission granted for Terminal (or whichever
#     app runs this script). System Settings -> Privacy & Security ->
#     Screen Recording. First run prompts -- grant + re-run.
#   * Accessibility permission for Terminal (or whichever app runs this
#     script) so AppleScript keystrokes route to tmnl. Same Settings page,
#     Accessibility section.
#   * Screen unlocked. screencapture pulls window pixels through the
#     compositor even while locked, but System Events keystrokes route to
#     loginwindow when the screen is locked, so the choreography below
#     won't run.
#   * `gifski` on PATH. `brew install gifski` if missing.
#   * The user's tmnl recents (~/.config/tmnl/recents.toml) lists *at
#     least one* mnml entry pointing at a binary that actually exists.
#     The script picks the first numerical entry as "the mnml entry";
#     adjust DEMO_DIGIT if recents order differs.
#
# This script does NOT touch the user's running tmnl. It launches a fresh
# dev binary from `target/release/tmnl --no-launch` so the demo happens
# in isolation. The dev tmnl is killed after the recording.

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
TMNL_BIN="$REPO_ROOT/target/release/tmnl"
DRIVER_SCPT="$(dirname -- "${BASH_SOURCE[0]}")/drive-native-tabs.scpt"
OUT_DIR="$REPO_ROOT/site/src/assets/demos"
WORK_DIR="/tmp/tmnl-recording"
MP4="$WORK_DIR/native-tabs.mp4"
GIF="$OUT_DIR/native-tabs.gif"
DURATION=28          # seconds of screencapture
GIF_FPS=15
GIF_WIDTH=1200
# Which recent entry to pick at each welcome overlay -- 1-indexed digit
# the AppleScript driver types. Adjust if the user's recents.toml order
# differs from the assumed { mnml, mnml-other, mixr, ... } order.
DEMO_DIGIT_FIRST=2   # first overlay -> open mnml
DEMO_DIGIT_SECOND=2  # second overlay -> open another mnml

mkdir -p "$WORK_DIR" "$OUT_DIR"

if [[ ! -x "$TMNL_BIN" ]]; then
  echo "tmnl release binary not found at $TMNL_BIN" >&2
  echo "run: cargo build --release" >&2
  exit 1
fi
if ! command -v gifski >/dev/null 2>&1; then
  echo "gifski not on PATH. Install with: brew install gifski" >&2
  exit 1
fi
if [[ ! -f "$DRIVER_SCPT" ]]; then
  echo "AppleScript driver missing at $DRIVER_SCPT" >&2
  exit 1
fi

# Sanity check: is the screen locked? If so, refuse -- keystrokes will go
# to loginwindow and the demo will record a static welcome screen.
LOCKED=$(/usr/bin/python3 - <<'PY' 2>/dev/null || echo 0
import ctypes, ctypes.util, objc  # noqa
from Foundation import NSString  # noqa
PY
)
# Simpler: query via swift (always available on macOS)
LOCKED_OUT="$(/usr/bin/swift - <<'PY' 2>&1 || true
import Cocoa
typealias F = @convention(c) () -> CFDictionary?
let h = dlopen("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics", RTLD_NOW)
let p = dlsym(h, "CGSessionCopyCurrentDictionary")
let f = unsafeBitCast(p, to: F.self)
if let d = f() as? [String: Any], let l = d["CGSSessionScreenIsLocked"] as? Int {
    print(l)
} else { print(0) }
PY
)"
if [[ "$LOCKED_OUT" == "1" ]]; then
  echo "Screen is locked. Unlock the Mac and re-run -- keystrokes route to" >&2
  echo "loginwindow while locked." >&2
  exit 2
fi

# Launch the dev tmnl with --no-launch so it opens in shell mode (welcome
# overlay appears because recents.toml has entries). Keep its PID.
echo "Launching dev tmnl from $TMNL_BIN ..."
"$TMNL_BIN" --no-launch &
TMNL_PID=$!
trap '[[ -n "${TMNL_PID:-}" ]] && kill "$TMNL_PID" 2>/dev/null || true' EXIT
# Wait for window to materialize
sleep 2

# Discover the window id of the freshly-spawned tmnl
WIN_ID="$(/usr/bin/swift - "$TMNL_PID" <<'PY' 2>&1
import Cocoa
let targetPid = pid_t(Int(CommandLine.arguments[1]) ?? -1)
guard let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] else { exit(1) }
for w in list {
    let pid = w[kCGWindowOwnerPID as String] as? Int ?? -1
    let name = w[kCGWindowName as String] as? String ?? ""
    let onScreen = w[kCGWindowIsOnscreen as String] as? Bool ?? false
    let wid = w[kCGWindowNumber as String] as? Int ?? -1
    if pid == Int(targetPid) && name == "tmnl" && onScreen {
        print(wid); exit(0)
    }
}
exit(1)
PY
)"
if [[ -z "$WIN_ID" ]]; then
  echo "Could not find tmnl window for pid $TMNL_PID" >&2
  exit 3
fi
echo "Recording window id $WIN_ID for ${DURATION}s ..."

# Start screencapture in the background (window-id-targeted video).
# -k shows clicks, -x silences sounds.
screencapture -v -V "$DURATION" -k -x -l "$WIN_ID" "$MP4" &
CAP_PID=$!
# screencapture takes ~0.5s to initialize the encoder
sleep 1

# Drive the choreography via AppleScript. The script focuses the dev tmnl
# by pid (looks it up via System Events), then sends the key sequence.
osascript "$DRIVER_SCPT" "$TMNL_PID" "$DEMO_DIGIT_FIRST" "$DEMO_DIGIT_SECOND" &
DRV_PID=$!

# Wait for screencapture to finish (it'll exit on its own when -V elapses)
wait "$CAP_PID" || true
# Make sure the driver is done
wait "$DRV_PID" 2>/dev/null || true

if [[ ! -s "$MP4" ]]; then
  echo "screencapture produced no output at $MP4" >&2
  exit 4
fi
echo "Encoded mp4 at $MP4 ($(du -h "$MP4" | cut -f1))"

# Convert mp4 -> gif via gifski (decode mp4 frames with ffmpeg first;
# gifski can also accept frames directly).
echo "Converting to GIF ($GIF_WIDTH px wide, $GIF_FPS fps) ..."
FRAMES_DIR="$WORK_DIR/frames"
rm -rf "$FRAMES_DIR"
mkdir -p "$FRAMES_DIR"
ffmpeg -loglevel error -i "$MP4" -vf "fps=$GIF_FPS,scale=$GIF_WIDTH:-1:flags=lanczos" \
  "$FRAMES_DIR/frame-%04d.png"
gifski --width "$GIF_WIDTH" --fps "$GIF_FPS" --quality 90 \
  --output "$GIF" "$FRAMES_DIR"/frame-*.png
echo "GIF written to $GIF ($(du -h "$GIF" | cut -f1))"

# Clean up dev tmnl
kill "$TMNL_PID" 2>/dev/null || true
echo "Done."
