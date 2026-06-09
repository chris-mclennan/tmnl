# tmnl backlog

Free-form list of features + polish items that aren't yet tracked
in a commit / branch. Add to the top; cross out (or delete) when
shipped. No severity / dates required — those live in commit
messages and `findings/` reports.

---

## Find (Cmd+F in scrollback)

Search visible + buffered scrollback for a query string. Surfaces:

- `Cmd+F` opens an inline find bar (along the top strip? or bottom?).
- Live highlight of every match in the visible scrollback as the
  user types (cell pipeline glyph attrs — yellow bg, dim fg).
- `Enter` / `Cmd+G` jumps to next match; `Shift+Enter` / `Shift+Cmd+G`
  goes back. Wrap-around with a status hint.
- `Esc` closes the find bar + clears highlights.
- Per-pane state (each ShellSession has its own scrollback, so the
  find bar applies to the FOCUSED pane).
- Match count chip (`3 of 12`) in the find bar.

Open questions:

- Regex vs literal? Default literal with a toggle for regex.
- Case sensitivity? Default smart-case (lowercase query → case-insensitive).
- How far back to search — just the visible viewport, the full
  scrollback buffer, or both with a "search older" affordance?
- Does the body grid need a "find overlay" pipeline layer, or can
  the existing cell pipeline carry the highlight via per-cell
  attribute bits? The latter is simpler if there's a free attr bit.
