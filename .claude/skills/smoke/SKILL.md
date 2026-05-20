---
description: Run a headless shell-mode smoke test — builds tmnl, drives `--headless` through a scripted scenario, and shows the rendered cell-grid dump. Use to verify shell-mode rendering, OSC 133 integration, or any shell-mode UI change without launching the GUI app.
disable-model-invocation: true
allowed-tools: Bash(cargo build:*) Bash(./target/debug/tmnl --headless:*)
---

Verify shell-mode rendering headlessly — no GPU window needed.

1. Build: `cargo build --bin tmnl`.
2. Drive `tmnl --headless` with a scripted scenario on stdin. The
   harness (`src/headless.rs`) accepts these line commands: `type <text>`,
   `key <name>` (enter/tab/esc/backspace/space/up/down/left/right/
   home/end), `wait <ms>`, `dump`, `quit`. Each `dump` prints the cell
   grid as text with a header (size, cursor, OSC 133 state).

   Default scenario — type a command, run it, dump the screen:

   ```bash
   printf 'type echo smoke-test\nkey enter\nwait 500\ndump\nquit\n' \
     | ./target/debug/tmnl --headless
   ```

3. Read the dump: check the prompt, the typed command, and its output
   land where expected.

To exercise OSC 133 integration, source the snippet inside the session
first — `integration: active=true` in the dump header confirms the marks
are being parsed, and `running=true` appears while a command runs:

```bash
printf 'type source %s/shell-integration/tmnl.zsh\nkey enter\nwait 700\ndump\nquit\n' "$PWD" \
  | ./target/debug/tmnl --headless
```

Adapt the scripted commands to whatever change you're verifying. If a
command produces output slowly, add a `wait <ms>` before the `dump`.
