---
title: Updates
description: How tmnl checks for new releases on launch.
---

tmnl pings GitHub at launch to see whether you're on the latest version. The
check is best-effort, runs in the background, and never blocks startup.

## What happens

On every launch, a short-lived background thread issues a single blocking GET
to `https://api.github.com/repos/chris-mclennan/tmnl/releases/latest`, compares
the response's `tag_name` against the running `CARGO_PKG_VERSION`, and — if the
remote is newer — prints a one-liner to stderr:

```
tmnl: v0.0.5 available — https://github.com/chris-mclennan/tmnl/releases/tag/v0.0.5
```

If the request fails (offline, GitHub down, rate-limited), tmnl stays silent
and continues. Nothing in the foreground UI changes either way.

The implementation lives in [`src/update_check.rs`](https://github.com/chris-mclennan/tmnl/blob/master/src/update_check.rs).
It uses [`ureq`](https://docs.rs/ureq) (the small blocking HTTP client) instead
of an async crate so the check drops directly into tmnl's `winit` + `wgpu`
setup without dragging in a runtime.

## Where the message lands

In **v1** (current), the announcement only goes to stderr. If you launched
tmnl from another terminal you'll see it there; if you launched from the dock,
it lands in the GUI app's stderr stream (Console.app on macOS).

A **v2** integration into the welcome-banner overlay is queued — the read
APIs (`latest()` and `take_pending_announcement()`) are already wired but
`dead_code`-gated until that lands. The plan is that the welcome screen will
surface "new version available" as a non-blocking chip you can click.

## Turning it off

There's no config opt-out yet — that's tracked as a future enhancement. The
check is a single HTTPS request to GitHub's public REST API and contains no
personally-identifying payload beyond what a default `ureq` request sends, but
if you need it off today you can build from source with the call to
`update_check::spawn()` removed in `main()`.

## Family consistency

mnml and mixr ship the same `update_check` shape against their own repos, so
the three apps stay easy to keep in sync. If you have all three installed,
each one announces its own updates independently.
