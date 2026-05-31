use std::io::BufReader;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

// Cross-platform blit IPC: file-path Unix-domain sockets on every
// platform. Unix uses `std::os::unix::net`; Windows uses the
// `uds_windows` crate (a thin wrapper around Windows' native AF_UNIX
// support, available since Win10 1803). The API surface matches std's
// shape so the rest of this module stays platform-agnostic —
// `Listener::bind(path)`, `Stream::try_clone()`, `Read`/`Write` on the
// stream, all behave the same on both.
#[cfg(unix)]
use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};
#[cfg(windows)]
use uds_windows::{UnixListener as Listener, UnixStream as Stream};

use tmnl_protocol::{
    Frame, InputEvent, Message, PROTOCOL_VERSION, Resize, pack_rgba_u8, read_message, write_message,
};

pub struct Server {
    pub socket_path: PathBuf,
    pub frame_rx: Receiver<Frame>,
    pub events: Receiver<ServerEvent>,
    writer: Arc<Mutex<Option<Stream>>>,
}

#[derive(Debug)]
pub enum ServerEvent {
    ClientConnected,
    ClientDisconnected,
    /// Client supplied a tab title (`Message::Title`). Renderer
    /// updates the Native tab's label to this string. Repeated
    /// titles overwrite (each Title replaces the previous one).
    Title(String),
    /// Client asked to open a sibling pane running `command args…`
    /// (`Message::OpenPane`) — e.g. mnml's `mixr.show`. The renderer
    /// splits + launches it as a native client.
    OpenPane {
        command: String,
        args: Vec<String>,
    },
}

/// Bind a UDS listener at the file path. Identical shape on Unix
/// (std `UnixListener`) and Windows (`uds_windows::UnixListener`);
/// the unlink-then-bind dance handles a stale socket file from a
/// crashed previous run.
fn bind_listener(socket_path: &std::path::Path) -> std::io::Result<Listener> {
    let _ = std::fs::remove_file(socket_path);
    Listener::bind(socket_path)
}

/// Best-effort cleanup of the listener's socket file when the server
/// drops. Filesystem unlink works on every platform now that we use
/// file-path UDS everywhere.
fn remove_endpoint(socket_path: &std::path::Path) {
    let _ = std::fs::remove_file(socket_path);
}

impl Server {
    pub fn start(socket_path: PathBuf) -> std::io::Result<Self> {
        let listener = bind_listener(&socket_path)?;
        let (frame_tx, frame_rx) = channel::<Frame>();
        let (event_tx, event_rx) = channel::<ServerEvent>();
        let writer: Arc<Mutex<Option<Stream>>> = Arc::new(Mutex::new(None));
        let writer_clone = writer.clone();
        thread::spawn(move || {
            accept_loop(listener, frame_tx, event_tx, writer_clone);
        });
        Ok(Self {
            socket_path,
            frame_rx,
            events: event_rx,
            writer,
        })
    }

    pub fn send_resize(&self, cols: u16, rows: u16) {
        self.send(&Message::Resize(Resize { cols, rows }));
    }

    pub fn send_input(&self, ev: &InputEvent) {
        self.send(&Message::Input(*ev));
    }

    pub fn send_quit(&self) {
        self.send(&Message::Quit);
    }

    fn send(&self, msg: &Message) {
        let mut guard = self.writer.lock().unwrap();
        if let Some(s) = guard.as_mut()
            && let Err(e) = write_message(s, msg)
        {
            log::warn!("send: {e:?}");
            *guard = None;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Give the client a chance to save before we tear the
        // connection down. This pairs with Launcher::shutdown's grace
        // period — the client (mnml's blit loop) sees the Quit, runs
        // save_session_on_quit, and exits cleanly before the kill
        // arrives.
        self.send_quit();
        remove_endpoint(&self.socket_path);
    }
}

fn accept_loop(
    listener: Listener,
    frame_tx: Sender<Frame>,
    event_tx: Sender<ServerEvent>,
    writer_slot: Arc<Mutex<Option<Stream>>>,
) {
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                log::warn!("accept failed: {e:?}");
                continue;
            }
        };
        let reader_half = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("clone stream failed: {e:?}");
                continue;
            }
        };
        {
            let mut guard = writer_slot.lock().unwrap();
            if guard.is_some() {
                log::warn!("rejecting second client (single-client v1)");
                drop(stream);
                continue;
            }
            *guard = Some(stream);
        }
        eprintln!("tmnl: client connected");
        let _ = event_tx.send(ServerEvent::ClientConnected);
        let handshake_ok = {
            let mut guard = writer_slot.lock().unwrap();
            match guard.as_mut() {
                Some(s) => {
                    let hello = write_message(
                        s,
                        &Message::Hello {
                            version: PROTOCOL_VERSION,
                        },
                    );
                    // Hand the client our chrome colors so it can theme
                    // its bg / fg / accent to match the window. Without
                    // this, blit-host clients (mixr / mnml / etc.) fall
                    // back to their own defaults and a darker pane sits
                    // inside a lighter (or differently-tinted) tmnl
                    // window, with a visible mismatch ring around the
                    // pane edge.
                    // Values mirror the constants in `main.rs`:
                    // CLEAR_BG / TEXT_FG / a teal accent from the
                    // launcher script.
                    let palette = write_message(
                        s,
                        &Message::Palette {
                            bg: pack_rgba_u8(0x1b, 0x1f, 0x27, 0xff),
                            fg: pack_rgba_u8(0xdb, 0xde, 0xeb, 0xff),
                            accent: pack_rgba_u8(0x53, 0xc0, 0xbc, 0xff),
                        },
                    );
                    hello.is_ok() && palette.is_ok()
                }
                None => false,
            }
        };
        if !handshake_ok {
            let _ = event_tx.send(ServerEvent::ClientDisconnected);
            *writer_slot.lock().unwrap() = None;
            continue;
        }
        let ftx = frame_tx.clone();
        let etx = event_tx.clone();
        let slot = writer_slot.clone();
        thread::spawn(move || {
            reader_loop(reader_half, ftx, etx, slot);
        });
    }
}

fn reader_loop(
    stream: Stream,
    frame_tx: Sender<Frame>,
    event_tx: Sender<ServerEvent>,
    writer_slot: Arc<Mutex<Option<Stream>>>,
) {
    let mut r = BufReader::new(stream);
    loop {
        match read_message(&mut r) {
            Ok(Message::Frame(f)) => {
                if frame_tx.send(f).is_err() {
                    log::warn!("frame_tx.send failed; reader exiting");
                    break;
                }
            }
            Ok(Message::Hello { version }) => {
                log::info!("client hello v{version}");
            }
            Ok(Message::Resize(_)) => {}
            Ok(Message::Input(_)) => {}
            Ok(Message::Quit) => {}
            // Server → client message; tmnl-as-server never receives one.
            Ok(Message::Palette { .. }) => {}
            Ok(Message::Title(s)) => {
                if event_tx.send(ServerEvent::Title(s)).is_err() {
                    log::warn!("event_tx.send(Title) failed; reader exiting");
                    break;
                }
            }
            Ok(Message::OpenPane { command, args }) => {
                if event_tx
                    .send(ServerEvent::OpenPane { command, args })
                    .is_err()
                {
                    log::warn!("event_tx.send(OpenPane) failed; reader exiting");
                    break;
                }
            }
            // `OpenPaneTransfer` requires SCM_RIGHTS fd-passing on the
            // same `sendmsg` call — incompatible with the BufReader on
            // this stream (which would consume past the cmsg boundary).
            // Senders use a dedicated connection + `send_message_with_fd`
            // / `read_message_with_fd` (see DESIGN-FD-HANDOFF.md in
            // tmnl-protocol). Receiving one here means the sender used
            // the wrong API on the wrong stream — log + drop.
            Ok(Message::OpenPaneTransfer { command, args: _ }) => {
                log::warn!(
                    "OpenPaneTransfer received on streaming connection — \
                     this requires SCM_RIGHTS via send_message_with_fd \
                     on a dedicated socket; ignoring (command={command:?})"
                );
            }
            Err(e) => {
                if e.kind() != std::io::ErrorKind::UnexpectedEof {
                    log::warn!("read error: {e:?}");
                }
                break;
            }
        }
    }
    *writer_slot.lock().unwrap() = None;
    let _ = event_tx.send(ServerEvent::ClientDisconnected);
}

/// `<tempdir>/tmnl-<pid>.sock`. Cross-platform: `std::env::temp_dir()`
/// resolves to `$TMPDIR` (or `/tmp`) on Unix and `%TEMP%` on Windows.
pub fn default_socket_path() -> PathBuf {
    let pid = std::process::id();
    std::env::temp_dir().join(format!("tmnl-{pid}.sock"))
}

/// Unique socket path for a non-initial Native tab in the same tmnl
/// process. `nonce` is a per-tab counter (`App.native_tab_nonce`).
pub fn native_tab_socket_path(nonce: u32) -> PathBuf {
    let pid = std::process::id();
    std::env::temp_dir().join(format!("tmnl-{pid}-{nonce}.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use tmnl_protocol::{DiffRun, KeyCode, KeyInput, MouseInput, MouseKind, WireCell};

    const TIMEOUT: Duration = Duration::from_secs(2);

    /// A distinct socket path per test — `cargo test` runs in parallel,
    /// so two tests must never bind the same path.
    fn unique_socket() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("tmnl-test-{}-{}.sock", std::process::id(), n))
    }

    fn sample_frame() -> Frame {
        Frame {
            seq: 42,
            cols: 3,
            rows: 2,
            cursor_col: 2,
            cursor_row: 1,
            cursor_shape: 1,
            cursor_visible: 1,
            runs: vec![
                DiffRun {
                    start: 0,
                    cells: vec![
                        WireCell {
                            ch: 'h' as u32,
                            fg: 0x00FF_FFFF,
                            bg: 0,
                            attrs: 1,
                        },
                        WireCell {
                            ch: 'i' as u32,
                            fg: 0x00FF_FFFF,
                            bg: 0,
                            attrs: 0,
                        },
                    ],
                },
                DiffRun {
                    start: 4,
                    cells: vec![WireCell {
                        ch: '!' as u32,
                        fg: 0,
                        bg: 0,
                        attrs: 0,
                    }],
                },
            ],
        }
    }

    // ── wire-format round-trips ──────────────────────────────────

    fn roundtrip(msg: &Message) -> Message {
        let mut buf = Vec::new();
        write_message(&mut buf, msg).expect("write_message");
        read_message(&mut &buf[..]).expect("read_message")
    }

    #[test]
    fn roundtrip_hello() {
        match roundtrip(&Message::Hello { version: 9 }) {
            Message::Hello { version } => assert_eq!(version, 9),
            m => panic!("expected Hello, got {m:?}"),
        }
    }

    #[test]
    fn roundtrip_resize() {
        match roundtrip(&Message::Resize(Resize {
            cols: 200,
            rows: 60,
        })) {
            Message::Resize(r) => assert_eq!((r.cols, r.rows), (200, 60)),
            m => panic!("expected Resize, got {m:?}"),
        }
    }

    #[test]
    fn roundtrip_quit() {
        assert!(matches!(roundtrip(&Message::Quit), Message::Quit));
    }

    #[test]
    fn roundtrip_title() {
        match roundtrip(&Message::Title("mnml · src/main.rs".to_string())) {
            Message::Title(s) => assert_eq!(s, "mnml · src/main.rs"),
            m => panic!("expected Title, got {m:?}"),
        }
    }

    #[test]
    fn roundtrip_open_pane() {
        match roundtrip(&Message::OpenPane {
            command: "mixr".to_string(),
            args: vec!["--dashboard".to_string(), "x".to_string()],
        }) {
            Message::OpenPane { command, args } => {
                assert_eq!(command, "mixr");
                assert_eq!(args, vec!["--dashboard".to_string(), "x".to_string()]);
            }
            m => panic!("expected OpenPane, got {m:?}"),
        }
        // No-args form.
        match roundtrip(&Message::OpenPane {
            command: "mixr".to_string(),
            args: vec![],
        }) {
            Message::OpenPane { command, args } => {
                assert_eq!(command, "mixr");
                assert!(args.is_empty());
            }
            m => panic!("expected OpenPane, got {m:?}"),
        }
    }

    #[test]
    fn roundtrip_frame_preserves_runs_and_cursor() {
        match roundtrip(&Message::Frame(sample_frame())) {
            Message::Frame(f) => {
                assert_eq!(f.seq, 42);
                assert_eq!((f.cols, f.rows), (3, 2));
                assert_eq!((f.cursor_col, f.cursor_row), (2, 1));
                assert_eq!((f.cursor_shape, f.cursor_visible), (1, 1));
                assert_eq!(f.runs.len(), 2);
                assert_eq!(f.runs[0].start, 0);
                assert_eq!(f.runs[0].cells.len(), 2);
                assert_eq!(f.runs[0].cells[0].ch, 'h' as u32);
                assert_eq!(f.runs[0].cells[0].attrs, 1);
                assert_eq!(f.runs[1].start, 4);
                assert_eq!(f.runs[1].cells[0].ch, '!' as u32);
            }
            m => panic!("expected Frame, got {m:?}"),
        }
    }

    #[test]
    fn roundtrip_key_input() {
        // A character key with modifier bits + press state.
        let key = Message::Input(InputEvent::Key(KeyInput {
            code: KeyCode::Char('q'),
            mods: 5,
            press: true,
        }));
        match roundtrip(&key) {
            Message::Input(InputEvent::Key(k)) => {
                assert!(matches!(k.code, KeyCode::Char('q')));
                assert_eq!(k.mods, 5);
                assert!(k.press);
            }
            m => panic!("expected Key input, got {m:?}"),
        }
        // An F-key — exercises the `F(u8)` payload.
        match roundtrip(&Message::Input(InputEvent::Key(KeyInput {
            code: KeyCode::F(7),
            mods: 0,
            press: false,
        }))) {
            Message::Input(InputEvent::Key(k)) => {
                assert!(matches!(k.code, KeyCode::F(7)));
                assert!(!k.press);
            }
            m => panic!("expected F-key input, got {m:?}"),
        }
    }

    #[test]
    fn roundtrip_mouse_input() {
        let mouse = Message::Input(InputEvent::Mouse(MouseInput {
            kind: MouseKind::ScrollUp,
            button: 0,
            col: 17,
            row: 9,
            mods: 2,
        }));
        match roundtrip(&mouse) {
            Message::Input(InputEvent::Mouse(m)) => {
                assert_eq!(m.kind, MouseKind::ScrollUp);
                assert_eq!((m.col, m.row), (17, 9));
                assert_eq!(m.mods, 2);
            }
            m => panic!("expected Mouse input, got {m:?}"),
        }
    }

    #[test]
    fn read_message_rejects_garbage() {
        // Zero-length payload.
        assert!(read_message(&mut &[0u8; 4][..]).is_err());
        // Valid length, unknown message-kind byte.
        let mut bad = Vec::new();
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.push(0xFE);
        assert!(read_message(&mut &bad[..]).is_err());
        // Truncated — the length says 8 bytes but only 2 follow.
        let mut short = Vec::new();
        short.extend_from_slice(&8u32.to_le_bytes());
        short.extend_from_slice(&[0u8; 2]);
        assert!(read_message(&mut &short[..]).is_err());
    }

    // ── Server end-to-end over a real Unix-domain socket ─────────

    #[test]
    fn client_connect_surfaces_an_event_and_a_hello() {
        let path = unique_socket();
        let server = Server::start(path.clone()).expect("server start");
        let client = Stream::connect(&path).expect("client connect");
        client.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert!(matches!(
            server.events.recv_timeout(TIMEOUT),
            Ok(ServerEvent::ClientConnected)
        ));
        // The server greets the client with a Hello.
        let mut r = BufReader::new(&client);
        match read_message(&mut r) {
            Ok(Message::Hello { version }) => assert_eq!(version, PROTOCOL_VERSION),
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    #[test]
    fn server_receives_a_frame_from_the_client() {
        let path = unique_socket();
        let server = Server::start(path.clone()).expect("server start");
        let mut client = Stream::connect(&path).expect("client connect");
        assert!(server.events.recv_timeout(TIMEOUT).is_ok()); // ClientConnected
        write_message(&mut client, &Message::Frame(sample_frame())).expect("send frame");
        let got = server.frame_rx.recv_timeout(TIMEOUT).expect("frame");
        assert_eq!(got.seq, 42);
        assert_eq!(got.runs.len(), 2);
    }

    #[test]
    fn server_receives_a_title_from_the_client() {
        let path = unique_socket();
        let server = Server::start(path.clone()).expect("server start");
        let mut client = Stream::connect(&path).expect("client connect");
        assert!(server.events.recv_timeout(TIMEOUT).is_ok()); // ClientConnected
        write_message(&mut client, &Message::Title("editing".to_string())).expect("send title");
        loop {
            match server.events.recv_timeout(TIMEOUT) {
                Ok(ServerEvent::Title(s)) => {
                    assert_eq!(s, "editing");
                    break;
                }
                Ok(_) => continue,
                Err(e) => panic!("no Title event: {e}"),
            }
        }
    }

    #[test]
    fn server_forwards_resize_and_quit_to_the_client() {
        let path = unique_socket();
        let server = Server::start(path.clone()).expect("server start");
        let client = Stream::connect(&path).expect("client connect");
        client.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert!(server.events.recv_timeout(TIMEOUT).is_ok());
        let mut r = BufReader::new(&client);
        // First two messages off the wire are the server's Hello +
        // Palette (chrome colors so the client can theme to match).
        assert!(matches!(read_message(&mut r), Ok(Message::Hello { .. })));
        assert!(matches!(read_message(&mut r), Ok(Message::Palette { .. })));
        server.send_resize(132, 43);
        match read_message(&mut r) {
            Ok(Message::Resize(rz)) => assert_eq!((rz.cols, rz.rows), (132, 43)),
            other => panic!("expected Resize, got {other:?}"),
        }
        server.send_quit();
        assert!(matches!(read_message(&mut r), Ok(Message::Quit)));
    }

    #[test]
    fn server_forwards_input_to_the_client() {
        let path = unique_socket();
        let server = Server::start(path.clone()).expect("server start");
        let client = Stream::connect(&path).expect("client connect");
        client.set_read_timeout(Some(TIMEOUT)).unwrap();
        assert!(server.events.recv_timeout(TIMEOUT).is_ok());
        let mut r = BufReader::new(&client);
        // Hello + Palette before any test-injected messages.
        assert!(matches!(read_message(&mut r), Ok(Message::Hello { .. })));
        assert!(matches!(read_message(&mut r), Ok(Message::Palette { .. })));
        server.send_input(&InputEvent::Key(KeyInput {
            code: KeyCode::Enter,
            mods: 0,
            press: true,
        }));
        match read_message(&mut r) {
            Ok(Message::Input(InputEvent::Key(k))) => assert!(matches!(k.code, KeyCode::Enter)),
            other => panic!("expected Key input, got {other:?}"),
        }
    }

    #[test]
    fn dropping_the_client_surfaces_a_disconnect() {
        let path = unique_socket();
        let server = Server::start(path.clone()).expect("server start");
        let client = Stream::connect(&path).expect("client connect");
        assert!(server.events.recv_timeout(TIMEOUT).is_ok()); // ClientConnected
        drop(client);
        loop {
            match server.events.recv_timeout(TIMEOUT) {
                Ok(ServerEvent::ClientDisconnected) => break,
                Ok(_) => continue,
                Err(e) => panic!("no ClientDisconnected: {e}"),
            }
        }
    }

    #[test]
    fn a_second_client_is_rejected() {
        let path = unique_socket();
        let server = Server::start(path.clone()).expect("server start");
        let _client1 = Stream::connect(&path).expect("client 1 connect");
        assert!(server.events.recv_timeout(TIMEOUT).is_ok()); // ClientConnected
        // The single-client server accepts a second connection only to
        // drop it — so client 2 gets no Hello, just a closed socket.
        let client2 = Stream::connect(&path).expect("client 2 connect");
        client2.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mut r = BufReader::new(&client2);
        assert!(read_message(&mut r).is_err());
    }
}
