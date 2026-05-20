---
description: Run tmnl from cargo for fast iteration (no .app bundle, no native menu bar). Use when the user wants to quickly try a change without rebundling. For the full macOS app experience use /build-app.
disable-model-invocation: true
allowed-tools: Bash(cargo run --bin tmnl*)
---

Run the dev binary directly:

```
cargo run --bin tmnl
```

This skips the `.app` bundling step, so the dock icon + native menu bar
won't behave correctly — but it's the fastest way to verify a code
change. For the full app experience use `/build-app`.
