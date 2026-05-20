//! Local AI command completion — the worker side.
//!
//! Wraps the `fim-engine` crate: embedded fill-in-the-middle code
//! completion (a quantized qwen2.5-coder model run in-process via
//! candle, fully offline after a one-time ~1 GB download). tmnl uses it
//! to complete a half-typed shell command — `prefix` is the command so
//! far, `suffix` is empty at a shell prompt.
//!
//! The `FimEngine` is not shareable across threads, so it lives on one
//! long-lived worker thread. The UI thread sends [`Request`]s and drains
//! [`Reply`]s — no `Arc<Mutex>`, no blocking the render loop. The engine
//! loads lazily inside the worker on the first request (that first call
//! can take a while if the model cache is cold); every call after is
//! fast. Pattern ported from mnml's `fim_worker_loop`.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

/// A completion request: `(request id, prefix, suffix)`.
pub type Request = (u64, String, String);
/// A completion reply: `(request id, result)`.
pub type Reply = (u64, Result<String, String>);

/// Reply id used for the worker's load-status message — not a real
/// completion. The UI uses it to tell "model loaded" / "load failed"
/// apart from an actual suggestion.
pub const STATUS_ID: u64 = u64::MAX;

/// Caps on the context sent to the engine — keeps inference cheap. A
/// shell command line never approaches these, but clamp defensively.
const MAX_PREFIX: usize = 2000;
const MAX_SUFFIX: usize = 1000;
/// Tokens the engine may generate for one completion.
const MAX_TOKENS: usize = 64;

/// Handle to the completion worker thread. Cheap to create — the model
/// only loads on the first [`request`](FimWorker::request).
pub struct FimWorker {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
}

impl FimWorker {
    /// Spawn the worker thread. Returns immediately; the model loads
    /// lazily on the first request.
    pub fn spawn() -> Self {
        let (tx, worker_rx) = channel::<Request>();
        let (worker_tx, rx) = channel::<Reply>();
        thread::Builder::new()
            .name("tmnl-fim-worker".into())
            .spawn(move || worker_loop(worker_rx, worker_tx))
            .expect("spawn fim worker thread");
        Self { tx, rx }
    }

    /// Queue a completion request. `prefix` is the text before the
    /// cursor; `suffix` the text after (empty at a shell prompt).
    pub fn request(&self, id: u64, prefix: &str, suffix: &str) {
        let _ = self.tx.send((
            id,
            clamp_tail(prefix, MAX_PREFIX),
            clamp_head(suffix, MAX_SUFFIX),
        ));
    }

    /// Drain every reply that has arrived since the last call. Never
    /// blocks. A reply with id [`STATUS_ID`] is a load-status message.
    pub fn poll(&self) -> Vec<Reply> {
        self.rx.try_iter().collect()
    }
}

/// Worker thread body. Owns the `FimEngine`, loads it lazily, then
/// serves completions one at a time.
fn worker_loop(rx: Receiver<Request>, reply: Sender<Reply>) {
    let mut engine: Option<fim_engine::FimEngine> = None;
    let mut load_error: Option<String> = None;

    while let Ok((id, prefix, suffix)) = rx.recv() {
        if let Some(err) = &load_error {
            let _ = reply.send((id, Err(err.clone())));
            continue;
        }
        if engine.is_none() {
            // First request — load the model. Blocking, and slow if the
            // ~1 GB cache is cold; that's fine, this is the worker.
            match fim_engine::FimEngine::load(
                &fim_engine::default_cache_dir(),
                fim_engine::ModelChoice::Qwen1_5B,
                &|_| {},
            ) {
                Ok(e) => {
                    engine = Some(e);
                    let _ = reply.send((STATUS_ID, Ok("local model ready".to_string())));
                }
                Err(e) => {
                    let _ = reply.send((STATUS_ID, Err(format!("model load failed: {e}"))));
                    load_error = Some(e.clone());
                    let _ = reply.send((id, Err(e)));
                    continue;
                }
            }
        }
        if let Some(e) = engine.as_mut() {
            let _ = reply.send((id, e.complete(&prefix, &suffix, MAX_TOKENS)));
        }
    }
}

/// Keep at most the last `max` chars of `s` — for a prefix, the end
/// nearest the cursor is what matters. Char-boundary safe.
fn clamp_tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        s.chars().skip(n - max).collect()
    }
}

/// Keep at most the first `max` chars of `s` — for a suffix.
fn clamp_head(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_tail_keeps_the_end() {
        assert_eq!(clamp_tail("hello", 10), "hello");
        assert_eq!(clamp_tail("hello", 3), "llo");
    }

    #[test]
    fn clamp_head_keeps_the_start() {
        assert_eq!(clamp_head("hello", 10), "hello");
        assert_eq!(clamp_head("hello", 3), "hel");
    }

    #[test]
    fn clamp_is_char_boundary_safe() {
        // Multi-byte chars must not be sliced mid-codepoint.
        assert_eq!(clamp_tail("héllo→", 2), "o→");
        assert_eq!(clamp_head("→héllo", 2), "→h");
    }
}
