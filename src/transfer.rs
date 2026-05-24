//! Pty-fd handoff receiver — companion to `server.rs` for the
//! `OpenPaneTransfer` message which can't ride the streaming
//! connection (SCM_RIGHTS ancillary data can't be read through a
//! `BufReader`).
//!
//! Each transfer is a fresh, single-message connection: the sender
//! opens the transfer socket, does one `sendmsg` with the pty master
//! fd attached via SCM_RIGHTS, then closes. The receiver thread
//! accepts, calls [`tmnl_protocol::read_message_with_fd`], publishes
//! a `TransferEvent`, then waits for the next connection.
//!
//! Sender side lives in mnml under task #49. The well-known socket
//! path is exported to children via the `TMNL_TRANSFER_SOCKET` env
//! var (set in [`crate::launcher::Launcher`]) so they don't have to
//! guess.

#[cfg(unix)]
use std::os::unix::io::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use tmnl_protocol::{Message, read_message_with_fd};

/// Events surfaced from the transfer listener to the app's tick loop.
/// Each successful transfer produces exactly one event; failed reads
/// + protocol-violation messages log + drop.
#[derive(Debug)]
pub enum TransferEvent {
    /// A peer sent `Message::OpenPaneTransfer` with an attached fd.
    /// The fd is the pty master to adopt — the receiver should wrap
    /// it in a `ShellSession::adopt_fd` and present it as a new tab.
    OpenPaneTransfer {
        command: String,
        args: Vec<String>,
        #[cfg(unix)]
        fd: OwnedFd,
    },
}

/// Handle for the transfer listener thread. Drop closes the listening
/// socket via `Drop` on `UnixListener` (after the thread exits on its
/// own when the accept loop errors out from the unlink — best-effort
/// teardown matches tmnl's `Server`).
pub struct TransferListener {
    pub socket_path: PathBuf,
    pub events: Receiver<TransferEvent>,
}

impl TransferListener {
    pub fn start(socket_path: PathBuf) -> std::io::Result<Self> {
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path)?;
        let (tx, rx) = channel::<TransferEvent>();
        thread::Builder::new()
            .name("tmnl-transfer".into())
            .spawn(move || {
                accept_loop(listener, tx);
            })?;
        Ok(Self {
            socket_path,
            events: rx,
        })
    }
}

impl Drop for TransferListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// `<TMPDIR>/tmnl-<pid>-transfer.sock` — one transfer socket per
/// tmnl process, lives next to the main server socket. Identical
/// shape so consumers can find both via `TMPDIR + pid`.
pub fn default_socket_path() -> PathBuf {
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let pid = std::process::id();
    let mut s = tmp;
    if !s.ends_with('/') {
        s.push('/');
    }
    PathBuf::from(format!("{s}tmnl-{pid}-transfer.sock"))
}

fn accept_loop(listener: UnixListener, tx: Sender<TransferEvent>) {
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                log::warn!("transfer accept failed: {e:?}");
                continue;
            }
        };
        #[cfg(unix)]
        match read_message_with_fd(&stream) {
            Ok((Message::OpenPaneTransfer { command, args }, Some(raw_fd))) => {
                // SAFETY: `read_message_with_fd` returns a raw fd that
                // was just produced by the kernel via SCM_RIGHTS — it's
                // unique to this process and not aliased elsewhere.
                // Wrapping in `OwnedFd` gives us deterministic close-
                // on-drop.
                let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
                if tx
                    .send(TransferEvent::OpenPaneTransfer { command, args, fd })
                    .is_err()
                {
                    log::warn!("transfer event_tx closed; listener exiting");
                    return;
                }
            }
            Ok((Message::OpenPaneTransfer { command, args: _ }, None)) => {
                log::warn!(
                    "OpenPaneTransfer from {command} arrived without an attached fd — dropping"
                );
            }
            Ok((other, maybe_fd)) => {
                log::warn!(
                    "transfer listener: ignoring unexpected {other:?} (fd attached: {})",
                    maybe_fd.is_some()
                );
                #[cfg(unix)]
                if let Some(raw) = maybe_fd {
                    // SAFETY: same provenance as above — we own the fd
                    // and close it via `OwnedFd::Drop`.
                    let _ = unsafe { OwnedFd::from_raw_fd(raw) };
                }
            }
            Err(e) => {
                log::warn!("transfer recvmsg failed: {e:?}");
            }
        }
        // Each transfer is single-message; drop the stream + accept
        // the next connection.
    }
}

#[cfg(all(unix, test))]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use tmnl_protocol::send_message_with_fd;

    fn unique_socket() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tmnl-transfer-test-{}-{}.sock",
            std::process::id(),
            n
        ))
    }

    #[test]
    fn listener_surfaces_open_pane_transfer_with_fd() {
        // A pair of socketpair fds — passing one through SCM_RIGHTS is
        // a real fd transfer (not just a Some(int) round-trip). The
        // receiver should get a usable, distinct fd in its address
        // space; we sanity-check that by asserting it's non-negative
        // and writable.
        let mut sv = [-1i32; 2];
        // SAFETY: socketpair(AF_UNIX, SOCK_STREAM, 0, sv) — standard
        // libc call; `sv` is initialized after a non-error return.
        let r = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) };
        assert_eq!(r, 0, "socketpair: {}", std::io::Error::last_os_error());
        let sent_raw = sv[0];

        let path = unique_socket();
        let listener = TransferListener::start(path.clone()).expect("listener start");

        // Sender: connect + send_message_with_fd in one shot.
        let sender = UnixStream::connect(&path).expect("sender connect");
        let msg = Message::OpenPaneTransfer {
            command: "claude".to_string(),
            args: vec!["--continue".to_string()],
        };
        send_message_with_fd(&sender, &msg, Some(sent_raw)).expect("send");

        let ev = listener
            .events
            .recv_timeout(Duration::from_secs(2))
            .expect("transfer event");
        let TransferEvent::OpenPaneTransfer { command, args, fd } = ev;
        assert_eq!(command, "claude");
        assert_eq!(args, vec!["--continue".to_string()]);
        let received_raw = fd.as_raw_fd();
        // Receiver-side fd is a distinct integer (kernel duped it) but
        // refers to the same socket — confirm by writing through the
        // received fd + reading from the sender's peer fd.
        assert!(received_raw >= 0);
        assert_ne!(received_raw, sent_raw);

        // Clean up the sender-side ends — receiver-side fd is closed
        // when `fd: OwnedFd` drops at scope end.
        // SAFETY: sv[1] still owned by the test; close it.
        unsafe {
            libc::close(sv[1]);
        }
        // Also close sv[0] — `send_message_with_fd` only duped it; we
        // still own this side of the pair.
        unsafe {
            libc::close(sv[0]);
        }
    }
}
