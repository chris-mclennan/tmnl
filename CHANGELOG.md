# Changelog

All notable changes to **tmnl** are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The roadmap lives in [`.local/PLAN.md`](.local/PLAN.md).

## [Unreleased]

tmnl has not yet had a tagged release. The `0.0.1` line below summarises the
capabilities present in the current `master`.

### Added

- **Welcome screen + recents** — bare-launch overlay listing recent native-tab
  launches from `~/.config/tmnl/recents.toml`; `1`–`9` open, `r` drops, `Esc`
  dismisses into the shell pane.
- **Pty-fd handoff receiver** — SCM_RIGHTS listener at
  `<TMPDIR>/tmnl-<pid>-transfer.sock`, exported via `TMNL_TRANSFER_SOCKET`;
  accepts `Message::OpenPaneTransfer` from child clients (e.g. mnml's
  `:tmnl.pop-pty`) and presents the received fd as a new adopted-shell tab.
- **`run.sh`** — family-wide dev subcommands (`build` / `release` / `test` /
  `check` / `watch` / `help`) plus tmnl-specific modes (`mnml [WS]` /
  `headless` / `no-launch`).

### Changed

- **Settings modal** — retrofitted to the family settings UI convention
  (`▸` focus marker, `*` modified marker, `r` reset row, `R` reset all,
  `Esc` cancels via the opened-state snapshot).
- **`tmnl-protocol`** bumped to `0.0.2`; tmnl now handles
  `Message::OpenPaneTransfer`.
- **`dirs`** bumped to `6` to match the rest of the family.

## [0.0.4] - 2026-06-01

### Added

- macOS DMGs are now code-signed + notarized when the
  `APPLE_TEAM_ID` / `APPLE_DEVELOPER_ID_CERT_*` / `APPLE_ID` /
  `APPLE_APP_PASSWORD` GitHub secrets are configured. Gatekeeper trusts
  the signed DMG without the "unidentified developer" warning.

### Changed

- `scripts/notarize-dmg.sh` — robust signing identity lookup via
  `security find-identity` SHA1 (instead of by-name format which fails
  if the cert's common name doesn't match the expected pattern).
- `notarytool submit` now bounds the wait at 30 min and surfaces the
  verdict on failure instead of hanging the CI run.

## [0.0.3] - 2026-05-31

### Changed

- macOS `.dmg` artifact now ships with cargo-dist's standard naming
  (`tmnl-rs-<triple>.dmg`).
- Install page's macOS download button points at the DMG (drag-to-install).

## [0.0.2] - 2026-05-31

### Added

- First `.app` bundle + DMG artifacts shipping with releases.
- Refactor: `build-app.sh` / `build-dmg.sh` accept `--bin-path` so CI can
  package the cargo-dist-built binary directly.

## [0.0.1]

### Added

- **Shell mode** — hosts a real pty, output parsed into cells with `vt100`;
  mouse input (click, drag, move, scroll).
- **Native mode** — the `tmnl-protocol` wire format over a Unix socket; apps
  send structured `Frame`s of cells, with partial-frame `DiffRun` updates and
  app-set tab titles.
- **GPU rendering** — a `wgpu` cell pipeline plus a chrome-strip pipeline;
  true-color cells, cursor shape and visibility.
- **Window & chrome** — native tabs, a native macOS menu bar, Mac-style editing
  chords, and an in-grid settings modal persisted to
  `~/.config/tmnl/config.toml`.
- **OSC 133 shell integration** — command-lifecycle tracking and a command-line
  anchor.
- **Local AI command completion** — `⌘I` continuation and `⌘K`
  natural-language → command, offline via the embedded `fim-engine` model.
- **Headless mode** (`--headless`) — scriptable cell-grid dumps for tests, plus
  `fake_server` / `fake_client` examples that exercise the protocol without a
  GPU window.

[Unreleased]: https://github.com/chris-mclennan/tmnl/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/chris-mclennan/tmnl/releases/tag/v0.0.1
