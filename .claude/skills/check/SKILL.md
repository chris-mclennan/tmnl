---
description: Run the standard quality sweep — cargo fmt, cargo clippy with warnings as errors, and cargo test. Use before claiming work is done, before /build-app, or when the user asks to "check", "lint", or "verify" the code.
disable-model-invocation: true
allowed-tools: Bash(cargo fmt:*) Bash(cargo clippy:*) Bash(cargo test:*)
---

Run the project's quality sweep, in order, stopping at the first failure:

1. `cargo fmt --all -- --check` — formatting check (no changes).
2. `cargo clippy --all-targets -- -D warnings` — lints, warnings as errors.
3. `cargo test` — unit + integration tests.

If formatting fails, run `cargo fmt --all` to fix and report what changed.

Report a one-line summary per step plus any failure output. Don't run
later steps after a failure — the user wants to see and fix the first
problem first.
