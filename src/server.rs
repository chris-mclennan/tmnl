use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use tmnl_protocol::{
    Frame, InputEvent, Message, PROTOCOL_VERSION, Resize, read_message, write_message,
};

pub struct Server {
    pub socket_path: PathBuf,
    pub frame_rx: Receiver<Frame>,
    pub events: Receiver<ServerEvent>,
    writer: Arc<Mutex<Option<UnixStream>>>,
}

pub enum ServerEvent {
    ClientConnected,
    ClientDisconnected,
    /// Client supplied a tab title (`Message::Title`). Renderer
    /// updates the Native tab's label to this string. Repeated
    /// titles overwrite (each Title replaces the previous one).
    Title(String),
}

impl Server {
    pub fn start(socket_path: PathBuf) -> std::io::Result<Self> {
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path)?;
        let (frame_tx, frame_rx) = channel::<Frame>();
        let (event_tx, event_rx) = channel::<ServerEvent>();
        let writer: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
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
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn accept_loop(
    listener: UnixListener,
    frame_tx: Sender<Frame>,
    event_tx: Sender<ServerEvent>,
    writer_slot: Arc<Mutex<Option<UnixStream>>>,
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
        let hello_ok = {
            let mut guard = writer_slot.lock().unwrap();
            match guard.as_mut() {
                Some(s) => write_message(
                    s,
                    &Message::Hello {
                        version: PROTOCOL_VERSION,
                    },
                )
                .is_ok(),
                None => false,
            }
        };
        if !hello_ok {
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
    stream: UnixStream,
    frame_tx: Sender<Frame>,
    event_tx: Sender<ServerEvent>,
    writer_slot: Arc<Mutex<Option<UnixStream>>>,
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
            Ok(Message::Title(s)) => {
                if event_tx.send(ServerEvent::Title(s)).is_err() {
                    log::warn!("event_tx.send(Title) failed; reader exiting");
                    break;
                }
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

pub fn default_socket_path() -> PathBuf {
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let pid = std::process::id();
    PathBuf::from(format!("{}tmnl-{}.sock", strip_trailing_slash(&tmp), pid))
}

fn strip_trailing_slash(s: &str) -> String {
    let mut out = s.to_string();
    if !out.ends_with('/') {
        out.push('/');
    }
    out
}
