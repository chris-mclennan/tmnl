//! fake_server — mirrors what tmnl does over the wire so we can validate
//! mnml's blit::run end-to-end without spinning up a wgpu window.
//!
//!   $ cargo run --example fake_server -- /tmp/test-tmnl.sock
//!
//! Then in another shell:
//!   $ mnml /tmp/some-workspace --blit /tmp/test-tmnl.sock --input standard
//!
//! fake_server will: accept the mnml client → send Hello → send Resize →
//! every 200ms send a synthetic Input (a letter, Esc, Down arrow) →
//! print every Frame it receives (just seq + cursor pos) so you can see
//! they're flowing back.

use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use tmnl_protocol::{
    BUTTON_NONE, InputEvent, KeyCode, KeyInput, Message, MouseInput, MouseKind, PROTOCOL_VERSION,
    Resize, read_message, write_message,
};

fn main() {
    let socket = std::env::args().nth(1).map(PathBuf::from).expect(
        "usage: fake_server <socket-path>  (give a path; will be created/recreated and bound)",
    );
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("bind");
    eprintln!("fake_server: listening on {}", socket.display());

    let (stream, _) = listener.accept().expect("accept");
    eprintln!("client connected");

    let reader_stream = stream.try_clone().expect("clone");
    let mut writer = stream;

    write_message(
        &mut writer,
        &Message::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .expect("hello");

    let (cols, rows) = (80u16, 24u16);
    write_message(&mut writer, &Message::Resize(Resize { cols, rows })).expect("resize");

    let (frame_tx, frame_rx) = channel::<(u64, usize, usize)>();
    thread::spawn(move || {
        let mut r = BufReader::new(reader_stream);
        loop {
            match read_message(&mut r) {
                Ok(Message::Hello { version }) => eprintln!("client hello v{version}"),
                Ok(Message::Frame(f)) => {
                    let n_runs = f.runs.len();
                    let n_cells: usize = f.runs.iter().map(|r| r.cells.len()).sum();
                    let _ = frame_tx.send((f.seq, n_runs, n_cells));
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("read end: {e:?}");
                    break;
                }
            }
        }
    });

    let scripted: &[InputEvent] = &[
        InputEvent::Key(KeyInput {
            code: KeyCode::Char('h'),
            mods: 0,
            press: true,
        }),
        InputEvent::Key(KeyInput {
            code: KeyCode::Char('i'),
            mods: 0,
            press: true,
        }),
        InputEvent::Key(KeyInput {
            code: KeyCode::Esc,
            mods: 0,
            press: true,
        }),
        InputEvent::Key(KeyInput {
            code: KeyCode::Down,
            mods: 0,
            press: true,
        }),
        InputEvent::Mouse(MouseInput {
            kind: MouseKind::Moved,
            button: BUTTON_NONE,
            col: 10,
            row: 5,
            mods: 0,
        }),
    ];

    let mut frames_seen = 0u64;
    let mut last_seen: u64 = u64::MAX;
    let mut last_runs = 0usize;
    let mut last_cells = 0usize;
    let mut total_runs = 0usize;
    let mut total_cells = 0usize;
    for (i, ev) in scripted.iter().enumerate() {
        thread::sleep(Duration::from_millis(200));
        if write_message(&mut writer, &Message::Input(*ev)).is_err() {
            eprintln!("write failed");
            break;
        }
        while let Ok((seq, n_runs, n_cells)) = frame_rx.try_recv() {
            frames_seen += 1;
            last_seen = seq;
            last_runs = n_runs;
            last_cells = n_cells;
            total_runs += n_runs;
            total_cells += n_cells;
        }
        eprintln!(
            "step {i}: sent {:?} · {frames_seen} frames · last seq {} · last frame {} runs {} cells",
            ev,
            if last_seen == u64::MAX {
                "-".to_string()
            } else {
                last_seen.to_string()
            },
            last_runs,
            last_cells,
        );
    }

    thread::sleep(Duration::from_millis(500));
    while let Ok((seq, n_runs, n_cells)) = frame_rx.try_recv() {
        frames_seen += 1;
        last_seen = seq;
        last_runs = n_runs;
        last_cells = n_cells;
        total_runs += n_runs;
        total_cells += n_cells;
    }
    let avg_runs = total_runs as f64 / frames_seen.max(1) as f64;
    let avg_cells = total_cells as f64 / frames_seen.max(1) as f64;
    eprintln!(
        "done: {frames_seen} frames received, last seq {} (last frame {} runs {} cells; avg {:.1} runs, {:.1} cells/frame)",
        if last_seen == u64::MAX {
            "-".to_string()
        } else {
            last_seen.to_string()
        },
        last_runs,
        last_cells,
        avg_runs,
        avg_cells,
    );

    // v5.5: graceful Quit instead of just dropping the socket.
    let send_quit = std::env::args().any(|a| a == "--send-quit");
    if send_quit {
        let quit_at = Instant::now();
        eprintln!("sending Quit");
        if let Err(e) = write_message(&mut writer, &Message::Quit) {
            eprintln!("Quit write failed: {e:?}");
        }
        // Watch for the client to disconnect (read end will EOF when mnml exits).
        let mut last_frame_ms = 0u64;
        while quit_at.elapsed() < Duration::from_secs(3) {
            if frame_rx.try_recv().is_err() {
                std::thread::sleep(Duration::from_millis(20));
            } else {
                last_frame_ms = quit_at.elapsed().as_millis() as u64;
            }
        }
        eprintln!(
            "post-Quit: waited {}ms; last frame seen at {}ms after Quit",
            quit_at.elapsed().as_millis(),
            last_frame_ms,
        );
    }

    drop(writer);
    let _ = std::fs::remove_file(&socket);
}
