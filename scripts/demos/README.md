# Demo recordings

Scripts that record screencapture GIFs of tmnl flows for the manual on
[tmnl.sh](https://tmnl.sh). Output lands in
`site/src/assets/demos/`; each recording embeds at the top of its
corresponding manual page.

## `record-native-tabs.sh`

Records `native-tabs.gif` — a ~25 s walkthrough of:

1. Welcome overlay over a fresh shell tab.
2. Press `2` → mnml opens as a native tab.
3. `Cmd+T` → fresh shell tab, welcome reappears.
4. Press `2` → second mnml native tab.
5. `Cmd+Shift+[` / `Cmd+Shift+]` → switch between the two.

Embedded at the top of
`site/src/content/docs/manual/getting-started.mdx`.

### Prereqs

1. **macOS Screen Recording permission** for whichever app runs the
   script (Terminal, iTerm, etc.). System Settings → Privacy & Security
   → Screen Recording. First run prompts — grant and re-run.
2. **macOS Accessibility permission** for the same app so AppleScript
   `System Events` can synthesize keystrokes. Same Settings page,
   Accessibility section.
3. **Screen unlocked.** `screencapture` can pull window pixels through
   the compositor while the Mac is locked, but synthetic keystrokes
   route to `loginwindow` while locked — the choreography won't run.
   The script aborts early if it detects a locked session via
   `CGSessionCopyCurrentDictionary`.
4. **`gifski` on `PATH`.** `brew install gifski`.
5. **`ffmpeg` on `PATH`.** Used to decode the mp4 → frames the gifski
   encoder consumes. `brew install ffmpeg`.
6. **A working mnml binary.** The script picks the first numeric digit
   from the welcome overlay (default `2`); ensure that entry in
   `~/.config/tmnl/recents.toml` points at a binary that actually
   exists. If your recents lists `mixr` at the top and mnml at row 2,
   the defaults work. If not, edit `DEMO_DIGIT_FIRST` /
   `DEMO_DIGIT_SECOND` at the top of `record-native-tabs.sh`.
7. **The dev tmnl built in release.** `cargo build --release` from the
   repo root.

### How to run

```sh
./scripts/demos/record-native-tabs.sh
```

The script:

1. Launches a fresh `target/release/tmnl --no-launch` so it doesn't
   touch your main running tmnl session.
2. Locates the new window via `CGWindowListCopyWindowInfo` (matched on
   the dev tmnl's pid).
3. Starts `screencapture -v -V 28 -l <wid>` to record just that window.
4. Runs `drive-native-tabs.scpt` to send the key sequence via
   `System Events`.
5. Decodes the mp4 → png frames via `ffmpeg`, then `gifski` to encode
   the final GIF.
6. Kills the dev tmnl.

Final output: `site/src/assets/demos/native-tabs.gif`. Intermediate
files in `/tmp/tmnl-recording/` (kept for inspection).

### Tweaking the demo

- Steps + timings live in `drive-native-tabs.scpt`. Each `delay N`
  controls the dwell at that step.
- The shell wrapper's `DURATION=28` should be a couple seconds longer
  than the sum of `delay`s in the script so screencapture doesn't
  cut off the last action.
- `GIF_WIDTH=1200` and `GIF_FPS=15` in the wrapper trade size vs
  smoothness. Bumping FPS makes Cmd+T transitions smoother; bumping
  width sharpens text.

### Known limitations

- **Single display, primary screen only.** The window-id targeting
  works on either display, but `screencapture -l` doesn't follow a
  window if you drag it between displays mid-recording. Don't.
- **Mouse cursor is not captured by `-l`.** Window-id captures don't
  include the cursor by default. Add `-C` to the screencapture
  invocation if you want it.
- **VHS / asciinema won't work.** tmnl renders via `wgpu` (not vt100
  in a stream), so neither tool sees output to record. Hence
  screencapture.

## Sibling: `native-tabs-welcome.png`

A still snapshot of the welcome overlay state, used as the fallback
hero asset in the manual when the GIF hasn't been re-recorded after a
UI change. Updated by re-running the screen-still capture step:

```sh
./scripts/demos/record-native-tabs.sh --still-only   # not yet implemented
```

For now, the still is captured manually from a frame of an existing
recording, or via:

```sh
screencapture -x -l <tmnl_window_id> \
  site/src/assets/demos/native-tabs-welcome.png
```
