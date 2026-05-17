use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tmnl_protocol::{
    DiffRun, Frame, Message, PROTOCOL_VERSION, WireCell, pack_rgba, read_message, write_message,
};

fn main() {
    let socket = std::env::args().nth(1).map(PathBuf::from).expect(
        "usage: fake_client <socket-path>  (path printed by tmnl on startup, e.g. /tmp/tmnl-NNNN.sock)",
    );

    eprintln!("connecting to {}", socket.display());
    let stream = UnixStream::connect(&socket).expect("connect");
    let mut writer = stream.try_clone().expect("clone writer");
    let reader_stream = stream;

    let mut r = BufReader::new(reader_stream);
    write_message(
        &mut writer,
        &Message::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .expect("hello");

    let (cols, rows) = loop {
        match read_message(&mut r).expect("read") {
            Message::Hello { version } => {
                eprintln!("server hello v{version}");
            }
            Message::Resize(rz) => {
                eprintln!("resize {}x{}", rz.cols, rz.rows);
                break (rz.cols, rz.rows);
            }
            _ => {}
        }
    };

    let bg = pack_rgba(0.063, 0.067, 0.078, 1.0);
    let fg = pack_rgba(0.93, 0.73, 0.45, 1.0);
    let dim = pack_rgba(0.48, 0.50, 0.58, 1.0);

    let banner = "tmnl  •  fake client streaming";

    let start = Instant::now();
    let mut seq: u64 = 0;
    let frame_period = Duration::from_millis(16);
    loop {
        let t = start.elapsed().as_secs_f32();
        if t > 30.0 {
            break;
        }

        let mut cells = vec![
            WireCell {
                ch: ' ' as u32,
                fg: dim,
                bg,
                attrs: 0,
            };
            cols as usize * rows as usize
        ];

        let mid_row = (rows / 2) as usize;
        let banner_col_base = (cols as isize - banner.chars().count() as isize).max(0) as usize / 2;
        for (i, ch) in banner.chars().enumerate() {
            let col = banner_col_base + i;
            if col < cols as usize {
                cells[mid_row * cols as usize + col] = WireCell {
                    ch: ch as u32,
                    fg,
                    bg,
                    attrs: 0,
                };
            }
        }

        let bar_row = (rows.saturating_sub(2)) as usize;
        let phase = ((t * (cols as f32) * 0.5) as usize) % (cols as usize);
        for c in 0..cols as usize {
            let on = c == phase || c == (phase + cols as usize / 2) % cols as usize;
            cells[bar_row * cols as usize + c] = WireCell {
                ch: if on { '█' as u32 } else { '·' as u32 },
                fg: if on { fg } else { dim },
                bg,
                attrs: 0,
            };
        }

        let frame = Frame {
            seq,
            cols,
            rows,
            cursor_col: (phase as u16).min(cols - 1),
            cursor_row: mid_row as u16,
            cursor_shape: 0,
            cursor_visible: 1,
            runs: vec![DiffRun { start: 0, cells }],
        };
        if let Err(e) = write_message(&mut writer, &Message::Frame(frame)) {
            eprintln!("write failed: {e:?}");
            break;
        }
        seq += 1;
        std::thread::sleep(frame_period);
    }
    eprintln!("done after {} frames", seq);
}
