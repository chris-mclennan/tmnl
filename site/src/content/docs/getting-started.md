---
title: First run
description: Launch tmnl, try AI command completion, and see how native mode works.
---

## Launch

Open tmnl. On macOS double-click the app or run `tmnl` from another terminal. On Windows and Linux, run `tmnl` from anywhere on your `PATH`.

You'll get a single tab with your default shell (`$SHELL` on Unix; PowerShell on Windows). If you've launched tmnl before and have entries in `~/.config/tmnl/recents.toml`, a welcome overlay appears with recent native-tab launches you can re-open with `1`–`9`.

## Default keys

| Action | Mac | Win / Linux |
| --- | --- | --- |
| New tab | `⌘T` | `Ctrl-Shift-T` |
| Close tab | `⌘W` | `Ctrl-Shift-W` |
| Next tab | `⌘Shift-]` | `Ctrl-Tab` |
| Previous tab | `⌘Shift-[` | `Ctrl-Shift-Tab` |
| **AI complete the half-typed command** | `⌘I` | `Ctrl-I` |
| **NL → shell command** | `⌘K` | `Ctrl-K` |
| Accept AI suggestion | `Tab` | `Tab` |
| Open settings | `⌘,` | `Ctrl-,` |
| Find in scrollback | `⌘F` | `Ctrl-F` |

Mac chord defaults follow native conventions (`⌘Z` undo, `⌘X` cut, `⌘C` copy, etc.). All keys rebindable from Settings.

## AI command completion

This is tmnl's distinctive feature. Two modes:

**Continuation** (`⌘I`) — start typing a command, hit `⌘I`, and tmnl proposes the rest as dim ghost text. `Tab` accepts.

```sh
$ git checkout -b feat/  ←  cursor here, press ⌘I
$ git checkout -b feat/payments-refactor   ←  ghost text appears
```

**Natural language** (`⌘K`) — describe what you want, hit `⌘K`, get the command:

```sh
$ # find files larger than 100M not modified in 30 days  ←  press ⌘K
$ find . -type f -size +100M -mtime +30   ←  ghost text appears
```

Both run a quantized `qwen2.5-coder` model via the embedded `fim-engine` crate. Entirely offline — no API key, no network call, nothing leaves your machine.

## Native tabs from mnml / mixr

If you have `mnml` or `mixr` installed and tmnl is running, certain commands inside them spawn a *new tmnl tab* rather than nesting a pseudo-terminal inside the existing app:

- `:tmnl.pop-pty` (from inside mnml) — transfers the focused pty pane out of mnml and into its own tmnl tab. State preserved via SCM_RIGHTS fd transfer. Unix only.
- `:tmnl.open-tab <command>` (from inside mnml) — opens a new tmnl tab running `<command>`.
- `mixr.show` (from inside mnml) — opens mixr as a sibling tmnl tab (instead of an embedded panel).
- `mnml --blit <socket>` — run mnml directly as a tmnl native client.
- `mixr --blit <socket>` — same for mixr.

tmnl exposes a transfer socket via the `TMNL_TRANSFER_SOCKET` environment variable in spawned shells. Set automatically — no config needed.

## Settings

`⌘,` opens the in-grid settings modal — same UI convention as mnml + mixr (sectioned rows, `▸` focus, `*` modified marker, `←→` adjust, `r` reset row, `R` reset all, `Enter` save, `Esc` cancel). Persisted to `~/.config/tmnl/config.toml`.

Common toggles: font size, window size, prompt inset, AI completion enable/disable.

## Recent native-tab launches

Every time tmnl spawns a native tab (mnml + a workspace, mixr, any other blit-host app), the launch is appended to `~/.config/tmnl/recents.toml`. Capped at 20 entries, de-duped, most-recent first.

On a bare launch (no `--mnml`, not headless), tmnl shows the welcome overlay:

| Key | Action |
|---|---|
| `1`–`9` | Open that recent entry as a new native tab |
| `↑` / `↓` / `j` / `k` | Move the selection |
| `Enter` | Open the focused entry |
| `r` | Remove focused from recents |
| `Esc` / `n` | Dismiss (shell mode underneath) |

## Headless mode

```sh
tmnl --headless
```

Renders to a scriptable cell grid dumped to stdout — useful for `.test`-style end-to-end pass/fail tests. Same `App` and draw path as the GUI, no window.

## Next

- Read about [the `tmnl-protocol` cell-streaming protocol](https://github.com/chris-mclennan/tmnl-protocol) if you want to write an app that renders into a tmnl tab.
- See [`examples/hello_client.rs`](https://github.com/chris-mclennan/tmnl/blob/master/examples/hello_client.rs) for a minimal native-mode client template.
- Browse the [GitHub repo](https://github.com/chris-mclennan/tmnl) for source, issues, and roadmap.
