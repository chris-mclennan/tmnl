# Building a tmnl backing app

A *backing app* renders into tmnl's window without speaking ANSI. It
connects to tmnl over a Unix socket and exchanges messages defined by the
`tmnl-protocol` crate: it **sends frames of cells**, and **receives input
and resize events**. tmnl owns the window, the GPU, the font atlas, and
the OS integration; your app owns what's on the grid.

This guide covers the protocol end to end. The companion files:

- [`../examples/hello_client.rs`](../examples/hello_client.rs) — a
  minimal working backing app. Copy it as a starting point.
- [`../examples/fake_client.rs`](../examples/fake_client.rs) — a streaming
  animation client (more moving parts).
- The `tmnl-protocol` crate — every type named below, plus
  `read_message` / `write_message`.

## Roles: who connects to whom

The **server** binds the socket; the **client** connects.

- **tmnl is the server.** It binds a Unix socket and prints the path on
  startup (e.g. `/tmp/tmnl-NNNN.sock`). It sends `Hello`, `Resize`, and
  `Input`. It receives `Frame` and `Title`.
- **Your app is the client.** It connects to that socket. It sends
  `Hello`, `Frame`, and `Title`. It receives `Resize` and `Input`.

(`examples/fake_server.rs` is a tmnl *stub* for testing — it binds the
socket. Don't let the filename confuse you: "server" is always the side
that owns the socket and the window.)

## The handshake

```
client                          server (tmnl)
  │  connect(socket)               │
  │ ──────── Hello{v:3} ─────────▶ │
  │ ◀──────── Hello{v:3} ───────── │
  │ ◀──────── Resize{cols,rows} ── │
  │                                │
  │  (now render: send Frame) ───▶ │
```

1. Connect to the socket path tmnl printed.
2. Send `Message::Hello { version: PROTOCOL_VERSION }` immediately.
3. Read messages until you get a `Resize` — that tells you the grid
   dimensions. tmnl also sends its own `Hello`; you can check the version.
4. Render your first `Frame` using those dimensions.
5. Loop: read input/resize, update state, send a new frame.

Don't render before the first `Resize` — you won't know `cols`/`rows`.

## Wire framing

Every message on the socket is length-prefixed:

```
[ u32 payload_len (little-endian) ][ payload_len bytes ]
```

The first payload byte is the message-type tag. You never hand-encode
this — `write_message` and `read_message` from `tmnl-protocol` do it for
you, over any `Write` / `Read`. Wrap the read half in a `BufReader`.

## Message reference

| Message | Direction | Meaning |
|---|---|---|
| `Hello { version: u32 }` | both ways | Handshake. Use `PROTOCOL_VERSION`. |
| `Resize { cols: u16, rows: u16 }` | tmnl → app | Grid size. Sent at startup and whenever the window resizes. |
| `Input(InputEvent)` | tmnl → app | A keypress or mouse event. |
| `Frame(Frame)` | app → tmnl | The cells to draw. Your main output. |
| `Title(String)` | app → tmnl | Tab-chip label for this connection. ≤ 4096 bytes. |
| `Quit` | either way | Graceful shutdown. Stop and close the socket. |
| `OpenPane { command, args }` | app → tmnl | Request that tmnl spawn `command` as a new sibling native tab. |
| `OpenPaneTransfer { command, args }` + attached fd | app → tmnl | Hand a running pty's master fd to tmnl (SCM_RIGHTS) to be adopted as a new shell tab. Sent over the dedicated transfer socket at `$TMNL_TRANSFER_SOCKET`, not the streaming connection. |

## Cells, colors, and the grid

The grid is **row-major**: the cell at `(col, row)` lives at index
`row * cols + col`.

A `WireCell` is four `u32`s:

```rust
WireCell {
    ch:    u32,  // Unicode scalar value (a char as u32)
    fg:    u32,  // packed RGBA, foreground
    bg:    u32,  // packed RGBA, background
    attrs: u32,  // style bitfield — 0 = plain text
}
```

Pack colors with the helpers in `tmnl-protocol`:

```rust
let bg = pack_rgba(0.06, 0.07, 0.08, 1.0);   // f32 channels, 0.0..=1.0
let fg = pack_rgba_u8(238, 238, 238, 255);   // or u8 channels, 0..=255
```

`attrs` is a renderer-interpreted style bitfield (bold / italic /
underline and friends). Use `0` for plain text; for styled text see
`style_from_attrs` in `src/atlas.rs` for the current bit assignments.

## Frames and diff runs

A `Frame` carries the cursor and a list of `DiffRun`s — contiguous spans
of cells:

```rust
DiffRun { start: u32, cells: Vec<WireCell> }   // start = grid cell index
```

**Full redraw** — one run covering the whole grid:

```rust
Frame {
    seq, cols, rows,
    cursor_col, cursor_row,
    cursor_shape: 0,        // renderer-defined; 0 is the default
    cursor_visible: 1,      // 0 hides the cursor
    runs: vec![DiffRun { start: 0, cells: all_cells }],  // len == cols*rows
}
```

**Partial update** — only the spans that changed. This is the cheap path,
and the reason the protocol beats re-emitting a screen of ANSI. If row 4
changed, send just that row:

```rust
runs: vec![DiffRun { start: 4 * cols as u32, cells: row_4_cells }]
```

Validation tmnl enforces on every frame — violate these and the frame is
rejected:

- A run must stay in bounds: `start + cells.len() <= cols * rows`.
- At most ~1M runs per frame.

`seq` is a monotonically increasing `u64` you assign. tmnl echoes nothing
back; `seq` is for your own logging/diagnostics. Just `seq += 1` per frame.

Start simple: send a full-grid run every frame. Move to diff runs only
once you have a reason to — correctness first, then optimize the wire.

## Receiving input and resize

Your read loop will see two things from tmnl:

- **`Resize`** — re-allocate your cell buffer to the new `cols`×`rows`
  and send a fresh full frame.
- **`Input(InputEvent)`** — either `InputEvent::Key(KeyInput)` or
  `InputEvent::Mouse(MouseInput)`.

```rust
match input {
    InputEvent::Key(k) => match k.code {
        KeyCode::Char(c)  => { /* k.mods has MOD_CTRL/ALT/SHIFT/SUPER */ }
        KeyCode::Enter    => { /* ... */ }
        KeyCode::Esc      => { /* ... */ }
        KeyCode::Up       => { /* arrow keys, Home/End, F(n), ... */ }
        _ => {}
    },
    InputEvent::Mouse(m) => {
        // m.kind (Down/Up/Drag/Moved/Scroll*), m.button, m.col, m.row
    }
}
```

`KeyInput.press` is `true` for key-down. `mods` is a bitmask of
`MOD_SHIFT | MOD_CTRL | MOD_ALT | MOD_SUPER`.

## A minimal client

The smallest useful loop, single-threaded and blocking:

```rust
// 1. connect + handshake
let stream = UnixStream::connect(&socket_path)?;
let mut writer = stream.try_clone()?;
let mut reader = BufReader::new(stream);
write_message(&mut writer, &Message::Hello { version: PROTOCOL_VERSION })?;
write_message(&mut writer, &Message::Title("my app".into()))?;

// 2. wait for the first Resize to learn the grid size
let (mut cols, mut rows) = loop {
    match read_message(&mut reader)? {
        Message::Resize(r) => break (r.cols, r.rows),
        _ => continue,
    }
};

// 3. render, then react to events forever
render(&mut writer, cols, rows, &state)?;
loop {
    match read_message(&mut reader)? {
        Message::Resize(r) => { cols = r.cols; rows = r.rows; }
        Message::Input(ev) => { update(&mut state, ev); }
        Message::Quit      => break,
        _ => {}
    }
    render(&mut writer, cols, rows, &state)?;
}
```

`examples/hello_client.rs` is exactly this shape, fleshed out into a
runnable echo demo. Build on it.

## Practical notes

- **Blocking IO is fine.** A single thread that blocks on `read_message`
  and renders in response works well for input-driven apps. Use a second
  thread (or non-blocking IO) only if you must render on a timer
  independent of input — see `fake_client.rs` for the threaded shape.
- **Handle `Resize` before you render.** A frame whose runs exceed the
  *current* `cols`×`rows` is rejected.
- **Send `Title` early.** Otherwise the tab falls back to a default label.
- **Quit gracefully.** On `Quit`, or when you're done, stop sending
  frames and drop the socket. tmnl treats EOF as disconnect.
- **Version check.** Compare the server's `Hello.version` against
  `PROTOCOL_VERSION`. They match today; this is your hook for the day
  they don't.

## Testing without a window

You don't need a GPU or a real tmnl window to develop against the
protocol — `examples/fake_server.rs` is a tmnl stub that binds the
socket, sends a scripted burst of input, and prints the frames it
receives. Point your client at it:

```bash
cargo run --example fake_server -- /tmp/dev.sock   # terminal A
cargo run --example hello_client -- /tmp/dev.sock  # terminal B
```

Or run the `/fake-protocol` skill to drive both sides at once.
