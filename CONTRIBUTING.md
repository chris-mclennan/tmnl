# Contributing to tmnl

Thanks for your interest in tmnl. This guide covers the workflow, conventions,
and the bit of architecture worth knowing before you change code.

## Getting started

```bash
git clone https://github.com/chris-mclennan/tmnl
cd tmnl
cargo build
cargo run --bin tmnl
```

tmnl builds on stable Rust — MSRV **1.85**, edition 2024. It is **macOS-only**
for now; see the roadmap in [`.local/PLAN.md`](.local/PLAN.md) for the
cross-platform plan.

tmnl depends on the sibling crates
[`tmnl-protocol`](https://github.com/chris-mclennan/tmnl-protocol) and
`fim-engine` by path — check them out alongside this repo.

## The verification gate

Every change must pass, in order:

```bash
cargo fmt
cargo build
cargo clippy --all-targets   # warning-free
cargo test
```

## Architecture

```
 shell mode:  pty + vt100  ─┐
                            ├──▶  Grid  ──▶  wgpu pipelines  ──▶  window
 native mode: socket +     ─┘
              tmnl-protocol
```

`Grid` (the cell buffer) is the **single source of truth**. Everything upstream
is a cell *producer* (the pty parser, or a native-mode socket client);
everything downstream is the *renderer*. When adding a feature, the first
question is almost always: **is this a producer change or a renderer change?**
Keeping that boundary clean is what lets shell mode and native mode share one
renderer.

The native-mode wire format is defined once, in `tmnl-protocol`, and shared with
backing apps. Changes to it ripple to every client — see the protocol crate's
own guidance before touching it.

## Conventions

- Run `cargo fmt` and keep `cargo clippy --all-targets` warning-free before every
  commit.
- Add tests for new behaviour — `--headless` mode makes the cell grid scriptable,
  so most rendering and protocol behaviour is testable without a GPU window.
- Smoke-test protocol changes with `examples/fake_server` + `examples/fake_client`.
- Keep commits small and focused; match the surrounding code style.

## Pull requests

1. Branch from `master`.
2. Make your change with tests; run the verification gate.
3. Open a PR describing the change and how you verified it.
4. CI runs `fmt` + `clippy -D warnings` + `test` on macOS — keep it green.

## Reporting bugs & requesting features

Use the [issue tracker](https://github.com/chris-mclennan/tmnl/issues). For bugs,
include your macOS version and steps to reproduce.

## License

By contributing, you agree that your contributions will be dual licensed under
the MIT and Apache-2.0 licenses, as described in [README.md](README.md#license),
without any additional terms or conditions.
