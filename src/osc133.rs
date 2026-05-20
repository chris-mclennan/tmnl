//! OSC 133 "semantic prompt" parsing for shell mode.
//!
//! When a shell has tmnl's integration snippet installed (see
//! `shell-integration/tmnl.zsh`) it emits OSC 133 marks around its
//! prompt and commands:
//!
//! ```text
//! ESC ] 133 ; A          ST   a fresh prompt is about to be drawn
//! ESC ] 133 ; B          ST   prompt drawn; command input begins here
//! ESC ] 133 ; C          ST   command submitted; its output begins
//! ESC ] 133 ; D [;exit]  ST   command finished
//! ```
//!
//! `ST` (string terminator) is either BEL (`0x07`) or `ESC \`.
//!
//! tmnl scans these out of the raw pty stream before handing bytes to
//! `vt100` — which neither interprets nor needs them. From the marks it
//! learns two things: whether a command is running (`C`/`D`), and where
//! the editable command line begins (`B` — captured as a terminal
//! cursor position, used to build the prefix for AI command
//! completion). If the snippet isn't installed no marks arrive and
//! [`State`] stays inert, so every consumer degrades gracefully.

/// A semantic-prompt mark recognized in the pty byte stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// `A` — a fresh prompt is about to be drawn.
    PromptStart,
    /// `B` — the prompt is drawn; the user's command input starts at
    /// the current cursor position.
    CommandStart,
    /// `C` — the command was submitted; its output starts here.
    CommandExecuted,
    /// `D` — the command finished. (Any `;exit` suffix is ignored for
    /// now; surfacing exit status is a later phase.)
    CommandFinished,
}

/// `ESC ] 133 ;` — the lead-in shared by every OSC 133 sequence.
const OSC_PREFIX: &[u8] = b"\x1b]133;";
/// OSC 133 bodies are tiny (`A`, `D;130`, …). A sequence whose body
/// runs past this without a terminator is treated as not-OSC-133.
const MAX_BODY: usize = 64;

/// Streaming OSC 133 scanner. Feed it pty chunks in order; it returns
/// each recognized mark paired with the byte offset **in that chunk**
/// just past the mark's terminator. The reader splits the byte stream
/// at that offset to sample the terminal cursor for the mark (an OSC
/// sequence emits no output, so the cursor there is well-defined).
/// Incomplete trailing sequences carry across chunk boundaries.
#[derive(Default)]
pub struct Scanner {
    /// An incomplete sequence from the end of the previous chunk.
    carry: Vec<u8>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan one chunk of pty output. Returns `(offset, mark)` pairs in
    /// stream order; `offset` is a byte index into `chunk` (0..=len).
    pub fn scan(&mut self, chunk: &[u8]) -> Vec<(usize, Mark)> {
        // Combine any carried prefix with the new chunk. `prefix_len`
        // lets us translate view offsets back to chunk offsets — the
        // carried bytes belong to the previous chunk.
        let prefix_len = self.carry.len();
        let view: Vec<u8> = if prefix_len == 0 {
            chunk.to_vec()
        } else {
            let mut v = std::mem::take(&mut self.carry);
            v.extend_from_slice(chunk);
            v
        };

        let mut out = Vec::new();
        let mut i = 0;
        while i < view.len() {
            if view[i] != 0x1b {
                i += 1;
                continue;
            }
            if i + OSC_PREFIX.len() <= view.len() && &view[i..i + OSC_PREFIX.len()] == OSC_PREFIX {
                let body_start = i + OSC_PREFIX.len();
                let scan_end = (body_start + MAX_BODY).min(view.len());
                let mut term_end: Option<usize> = None;
                let mut j = body_start;
                while j < scan_end {
                    if view[j] == 0x07 {
                        term_end = Some(j + 1);
                        break;
                    }
                    if view[j] == 0x1b && j + 1 < scan_end && view[j + 1] == b'\\' {
                        term_end = Some(j + 2);
                        break;
                    }
                    j += 1;
                }
                match term_end {
                    Some(end) => {
                        let body_end = if view[end - 1] == 0x07 {
                            end - 1
                        } else {
                            end - 2
                        };
                        if let Some(m) = parse_body(&view[body_start..body_end]) {
                            // View offset → chunk offset. A completed mark
                            // always ends past the carried prefix.
                            out.push((end.saturating_sub(prefix_len), m));
                        }
                        i = end;
                        continue;
                    }
                    None => {
                        // No terminator yet. Genuinely incomplete (ran out
                        // of bytes mid-body) → carry it; otherwise the body
                        // is over-long, so it isn't a short OSC 133.
                        if scan_end == view.len() && scan_end - body_start < MAX_BODY {
                            self.carry.extend_from_slice(&view[i..]);
                            return out;
                        }
                        i += 1;
                        continue;
                    }
                }
            }
            // The tail might be a partial `ESC ] 133 ;` prefix split by a
            // read boundary — carry it so the next chunk can complete it.
            let tail = &view[i..];
            if tail.len() < OSC_PREFIX.len() && OSC_PREFIX.starts_with(tail) {
                self.carry.extend_from_slice(tail);
                return out;
            }
            i += 1;
        }
        out
    }
}

/// Parse an OSC 133 body (the bytes between `133;` and the terminator).
fn parse_body(body: &[u8]) -> Option<Mark> {
    // The kind is the first `;`-separated field; D may carry an exit code.
    let kind = body.split(|&b| b == b';').next()?;
    match kind {
        b"A" => Some(Mark::PromptStart),
        b"B" => Some(Mark::CommandStart),
        b"C" => Some(Mark::CommandExecuted),
        b"D" => Some(Mark::CommandFinished),
        _ => None,
    }
}

/// Live shell-integration state, folded from OSC 133 marks. Shared
/// between the pty reader thread and the render thread.
#[derive(Default)]
pub struct State {
    active: bool,
    running: bool,
    /// vt100 cursor `(row, col)` captured at the last `B` mark — where
    /// the editable command line begins. `None` before the first
    /// prompt or while a command is running.
    input_anchor: Option<(u16, u16)>,
}

impl State {
    /// Fold one mark into the state. `cursor` is the terminal cursor
    /// `(row, col)` sampled at the mark's position in the stream —
    /// meaningful for `CommandStart`, ignored otherwise.
    pub fn apply(&mut self, mark: Mark, cursor: (u16, u16)) {
        self.active = true;
        match mark {
            Mark::PromptStart => {}
            Mark::CommandStart => self.input_anchor = Some(cursor),
            Mark::CommandExecuted => {
                self.running = true;
                self.input_anchor = None;
            }
            Mark::CommandFinished => {
                self.running = false;
                self.input_anchor = None;
            }
        }
    }

    /// `true` once any OSC 133 mark has been seen — i.e. the integration
    /// snippet is installed.
    pub fn active(&self) -> bool {
        self.active
    }

    /// `true` between `C` (command submitted) and `D` (finished).
    pub fn running(&self) -> bool {
        self.running
    }

    /// Terminal cursor `(row, col)` where the current command line
    /// begins, or `None` if there's no live prompt to complete into.
    pub fn input_anchor(&self) -> Option<(u16, u16)> {
        self.input_anchor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(chunks: &[&[u8]]) -> Vec<Mark> {
        let mut s = Scanner::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(s.scan(c).into_iter().map(|(_, m)| m));
        }
        out
    }

    #[test]
    fn recognizes_bel_terminated_marks() {
        assert_eq!(
            marks(&[b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D\x07"]),
            vec![
                Mark::PromptStart,
                Mark::CommandStart,
                Mark::CommandExecuted,
                Mark::CommandFinished,
            ],
        );
    }

    #[test]
    fn recognizes_st_terminated_marks() {
        assert_eq!(marks(&[b"\x1b]133;A\x1b\\"]), vec![Mark::PromptStart]);
    }

    #[test]
    fn d_with_exit_code_still_parses() {
        assert_eq!(marks(&[b"\x1b]133;D;130\x07"]), vec![Mark::CommandFinished]);
    }

    #[test]
    fn ignores_other_osc_and_plain_text() {
        assert!(marks(&[b"\x1b]0;window title\x07"]).is_empty());
        assert!(marks(&[b"\x1b]1337;Notify=hi\x07"]).is_empty());
        assert!(marks(&[b"just some normal output\r\n"]).is_empty());
    }

    #[test]
    fn handles_sequence_split_across_reads() {
        assert_eq!(
            marks(&[b"output \x1b]13", b"3;C\x07 more output"]),
            vec![Mark::CommandExecuted],
        );
    }

    #[test]
    fn handles_terminator_split_across_reads() {
        assert_eq!(marks(&[b"\x1b]133;A", b"\x07"]), vec![Mark::PromptStart]);
    }

    #[test]
    fn reports_chunk_offset_past_terminator() {
        // "ab" (2) + "\x1b]133;" (6) + "C" (1) + BEL (1) → terminator
        // ends at offset 10.
        let mut s = Scanner::new();
        assert_eq!(
            s.scan(b"ab\x1b]133;C\x07"),
            vec![(10, Mark::CommandExecuted)]
        );
    }

    #[test]
    fn offset_is_chunk_relative_after_a_split() {
        let mut s = Scanner::new();
        assert!(s.scan(b"\x1b]133;A").is_empty()); // carried
        // Terminator BEL is the only byte of this chunk → offset 1.
        assert_eq!(s.scan(b"\x07"), vec![(1, Mark::PromptStart)]);
    }

    #[test]
    fn state_tracks_command_lifecycle() {
        let mut st = State::default();
        assert!(!st.active() && !st.running() && st.input_anchor().is_none());

        st.apply(Mark::PromptStart, (0, 0));
        assert!(st.active() && !st.running());

        st.apply(Mark::CommandStart, (3, 7));
        assert_eq!(st.input_anchor(), Some((3, 7)));

        st.apply(Mark::CommandExecuted, (0, 0));
        assert!(st.running());
        assert!(st.input_anchor().is_none());

        st.apply(Mark::CommandFinished, (0, 0));
        assert!(!st.running());
    }
}
