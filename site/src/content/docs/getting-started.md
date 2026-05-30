---
title: First run
description: Launch tmnl, get a shell prompt, and learn the basic keymap.
---

## Launch

Open tmnl. On macOS double-click the app or run `tmnl` from another terminal. On Windows and Linux, run `tmnl` from anywhere on your `PATH`.

You'll get a single tab with your default shell (`$SHELL` on Unix; PowerShell on Windows).

## Default keys

| Action | Mac | Win / Linux |
| --- | --- | --- |
| New tab | `Cmd-T` | `Ctrl-Shift-T` |
| Close tab | `Cmd-W` | `Ctrl-Shift-W` |
| Next tab | `Cmd-Shift-]` | `Ctrl-Tab` |
| Previous tab | `Cmd-Shift-[` | `Ctrl-Shift-Tab` |
| Open settings | `Cmd-,` | `Ctrl-,` |

(Settings has a section that lets you rebind these.)

## Native tabs from mnml / mixr

If you have `mnml` or `mixr` installed and tmnl is running, certain commands inside them can spawn a *new tmnl tab* rather than nesting a pseudo-terminal inside the existing app:

- `mnml :tmnl.pop-pty` — transfers the focused pty pane out of mnml and into its own tmnl tab (no state loss, the pty fd moves via SCM_RIGHTS)
- `mnml :tmnl.open-tab <command>` — opens a new tmnl tab running `<command>`
- `mixr.show` from mnml — opens mixr as a sibling tmnl tab (instead of an embedded panel)

This relies on tmnl exposing a transfer socket via the `TMNL_TRANSFER_SOCKET` environment variable in spawned shells. Tmnl sets this automatically; you shouldn't have to configure anything.

## Next

- Read about [the cell-streaming protocol](https://github.com/chris-mclennan/tmnl-protocol) if you want to write an app that renders into a tmnl tab directly.
- Browse the [GitHub repo](https://github.com/chris-mclennan/tmnl-rs) for source and the issue tracker.
