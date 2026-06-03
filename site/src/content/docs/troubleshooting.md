---
title: Troubleshooting
description: Nightly app bundle, macOS Tahoe warnings, and other launch-time gotchas.
---

A short list of things that aren't bugs but can look like them on first
encounter.

## Nightly app bundle

If you build tmnl from source and want one-click access to your latest
`cargo build` output alongside the released app, the repo ships a nightly
bundle target:

```sh
./scripts/build-app.sh --nightly
```

This produces `target/tmnl-nightly.app` with bundle identifier
`rs.tmnl.app.nightly` and an inverted warm-orange icon (vs. the released
build's charcoal icon), so the two coexist in `/Applications` and pin to the
dock as separate entries. The nightly launcher `exec`s your local release
binary directly — there's no dispatch shim — so updating means rebuilding the
binary, not rebuilding the bundle.

The nightly bundle is **local-only**. It isn't produced by release CI and
isn't shipped to GitHub Releases. The intended use is a personal dock pin for
contributors and the author; everyone else should download a [tagged
release](/install/).

## macOS Tahoe — "Support Ending for Intel-based Apps" warning

If you're on macOS 26 (Tahoe) and a `tmnl.app` you previously installed
triggers a "Support Ending for Intel-based Apps" warning at launch, the cause
is `LSMinimumSystemVersion` in the bundle's `Info.plist`, not the binary
itself. Pre-`v0.0.4` builds declared `LSMinimumSystemVersion = 10.14`, which
Tahoe uses to classify the bundle as legacy Intel — even though the binary
itself is a real arm64 build.

The fix landed in **v0.0.4** — `LSMinimumSystemVersion` is now `11.0` in both
the stable and nightly `Info.plist`. Redownload the DMG from
[releases](https://github.com/chris-mclennan/tmnl/releases) and the warning
goes away.

`v0.0.4` is also the first release that ships with macOS code-signing and
notarization wired into CI, so on a clean install Gatekeeper now trusts the
DMG without the separate "unidentified developer" warning either.
