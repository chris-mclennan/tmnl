---
description: Bundle tmnl.app and launch it. Use when the user asks to build the app bundle, test the macOS menu bar / dock icon behavior, or wants to "open tmnl" after a change. For plain `cargo run` use /run instead.
argument-hint: "[debug|release]"
disable-model-invocation: true
allowed-tools: Bash(./scripts/build-app.sh:*) Bash(open target/tmnl.app)
---

Build the macOS `.app` bundle, then launch it.

`$ARGUMENTS` is the profile (`debug` or `release`). Default to `debug` if
empty.

Steps:

1. Run `./scripts/build-app.sh $ARGUMENTS` from the project root. If
   `$ARGUMENTS` is empty, run `./scripts/build-app.sh` (the script
   defaults to debug).
2. If the build succeeds, run `open target/tmnl.app`.
3. If the build fails, report the error and stop — do **not** try to
   launch a stale bundle.

The `.app` bundle is required for the native macOS menu bar + dock icon
to behave correctly. For fast iteration without those, prefer `/run`.
