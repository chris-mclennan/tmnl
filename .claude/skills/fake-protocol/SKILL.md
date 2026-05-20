---
description: Smoke-test the tmnl-protocol wire format end-to-end using the bundled fake_server and fake_client examples. Use when the user is changing the protocol crate, the server/launcher modules, or wants to verify both sides of the socket without spinning up a real backing app.
disable-model-invocation: true
allowed-tools: Bash(cargo run --example *) Bash(rm -f /tmp/test-tmnl.sock)
---

Run the protocol stubs against each other so we can see frames + input
flow across the Unix socket without a real backing app or wgpu window.

Steps:

1. Pick a socket path: `/tmp/test-tmnl.sock`. Remove any stale file at
   that path (`rm -f /tmp/test-tmnl.sock`).
2. Start `fake_server` in the background:
   `cargo run --example fake_server -- /tmp/test-tmnl.sock` (run in
   background so we don't block on it).
3. Give it ~1s to bind, then start `fake_client`:
   `cargo run --example fake_client -- /tmp/test-tmnl.sock`
   (foreground, so we see its output).
4. Let them run for ~5s, then stop both.
5. Report what flowed. `fake_server` is the **tmnl stub** — its stderr
   should show `client connected`, `client hello`, and a per-step count
   of `Frame`s received. `fake_client` is the **backing-app stub** — its
   stderr should show `server hello`, the `resize`, and a final
   `done after N frames`.

If either side fails to connect, the most common cause is a stale socket
file or a leftover process from a previous run — clean those up and
retry.
