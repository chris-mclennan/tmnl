//! The tmnl command registry — the spine the help overlay, command
//! palette (later), and tab-management chords hang off of.
//!
//! Each [`Command`] is a named, group-tagged action with optional
//! default keys and a `fn(&mut App, &ActiveEventLoop)` handler. The
//! registry is process-global (`OnceLock`) and built once at startup
//! from [`builtin_commands`].
//!
//! Mirrors mnml's `command.rs` shape so the family stays structurally
//! similar — the help overlay (in `app::help`) reads this registry
//! plus the resolved [`crate::keymap::Keymap`] to render its
//! `<chord>  <title>` rows. See `docs/COMMAND_MIGRATION.md`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use winit::event::KeyEvent;
use winit::event_loop::ActiveEventLoop;

use crate::App;

/// Command handler. `KeyEvent` is `Some` when the command is dispatched
/// from a keypress; `None` when fired by other paths (chip click,
/// `RunHostCommand` protocol message, ex command, …). Key-forwarding
/// handlers (`forward_as_ctrl`, `goto_tab_or_forward`) bail when it's
/// `None`; everything else ignores the parameter.
pub type CommandFn = fn(&mut App, &ActiveEventLoop, Option<&KeyEvent>);
/// Context predicate. Returns true when the command is eligible to
/// fire for the current `App` state. `None` ⇒ "always eligible"
/// (rare; most tmnl chords have at least a "no modal open" guard).
pub type WhenFn = fn(&App) -> bool;

#[derive(Clone)]
pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    /// Help-overlay section (e.g. `"Tabs"`, `"Splits"`, `"View"`).
    pub group: &'static str,
    /// Default keyspecs (`"cmd+t"`, `"cmd+shift+w"`, `"cmd+1"`, …).
    /// [`crate::keymap::Keymap`] parses these.
    pub keys: &'static [&'static str],
    pub run: CommandFn,
    pub when: Option<WhenFn>,
}

impl Command {
    pub fn key_hint(&self) -> String {
        self.keys.join(" / ")
    }
}

pub struct Registry {
    commands: Vec<Command>,
    by_id: HashMap<&'static str, usize>,
}

impl Registry {
    fn build() -> Self {
        let commands = builtin_commands();
        let by_id = commands
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id, i))
            .collect();
        Registry { commands, by_id }
    }

    pub fn get(&self, id: &str) -> Option<&Command> {
        self.by_id.get(id).map(|&i| &self.commands[i])
    }

    pub fn all(&self) -> &[Command] {
        &self.commands
    }
}

pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Registry::build)
}

/// Walk the registry and yield `(keys, title, group)` rows for every
/// command with a non-empty default `keys`. The `keys` field is the
/// joined display form (`"cmd+t / cmd+n"`). Used by the help overlay
/// to auto-generate rows.
pub fn help_rows() -> Vec<(String, &'static str, &'static str)> {
    registry()
        .all()
        .iter()
        .filter(|c| !c.keys.is_empty())
        .map(|c| (c.keys.join(" / "), c.title, c.group))
        .collect()
}

/// Look up `key` + `app.mods` in `app.keymap`, resolve the resulting
/// id in the registry, check the command's `when` guard, and run it.
/// Returns `true` when dispatched.
///
/// Takes `&mut App` (not a separate `&Keymap`) because `Keymap` lives
/// inside `App` and Rust can't split-borrow it from the rest of the
/// struct. We resolve to an owned `String` to drop the keymap borrow
/// before calling the handler.
pub fn try_dispatch(key: &KeyEvent, app: &mut App, event_loop: &ActiveEventLoop) -> bool {
    let mods = app.mods;
    let ids: Vec<String> = app.keymap.resolve_all(key, mods).to_vec();
    if ids.is_empty() {
        return false;
    }
    for id in &ids {
        let Some(cmd) = registry().get(id) else {
            continue;
        };
        let when = cmd.when;
        let run = cmd.run;
        if let Some(w) = when
            && !w(app)
        {
            continue;
        }
        run(app, event_loop, Some(key));
        return true;
    }
    false
}

/// Fire a command by id without a key event. Used by non-keyboard
/// dispatch paths: chip clicks, `Message::RunHostCommand` from a hosted
/// app, ex commands, …. Returns `true` when the id existed and the
/// `when` guard passed (the command may still no-op internally if it
/// genuinely required a key event, e.g. key-forwarding chords).
pub fn dispatch_by_id(id: &str, app: &mut App, event_loop: &ActiveEventLoop) -> bool {
    let Some(cmd) = registry().get(id) else {
        return false;
    };
    if let Some(w) = cmd.when
        && !w(app)
    {
        return false;
    }
    (cmd.run)(app, event_loop, None);
    true
}

/// Read a URL string from the OS clipboard. Returns `None` when the
/// clipboard isn't text or doesn't look URL-ish. macOS uses
/// `pbpaste`; Linux falls back to `xclip` / `wl-paste`; Windows
/// `powershell Get-Clipboard`. We shell out to avoid pulling in a
/// `clipboard` / `arboard` crate just for one button.
fn read_clipboard_url() -> Option<String> {
    let raw = read_clipboard_text()?;
    let url = raw.trim();
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Try to discover an already-running Playwright dashboard via CDP
/// at `debug_port` first; if nothing's there, spawn one with
/// `PLAYWRIGHT_DASHBOARD_DEBUG_PORT=debug_port` (headless mode)
/// and poll CDP until the dashboard target shows up.
///
/// Returns the dashboard's local URL (e.g. `http://localhost:54321/`)
/// — that's what gets opened in the Browser pane.
fn spawn_or_attach_dashboard(debug_port: u16) -> Result<String, String> {
    if let Some(url) = discover_dashboard_url(debug_port) {
        return Ok(url);
    }
    spawn_playwright_show(debug_port)?;
    // ~5s poll. Cold-start of `playwright-cli show` is dominated by
    // Node bootstrap + chromium download check + persistent context
    // launch — most of a second on warm caches, longer on first
    // run after a Playwright upgrade.
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if let Some(url) = discover_dashboard_url(debug_port) {
            return Ok(url);
        }
    }
    Err("timed out waiting for dashboard CDP at localhost:9222".into())
}

/// CDP /json/list returns an array of targets. We want the one
/// whose URL is the dashboard's HTTP-server URL — `http://localhost:`
/// prefixed and *not* the `data:text/html,` bootstrap target chromium
/// opens before navigating.
fn discover_dashboard_url(debug_port: u16) -> Option<String> {
    let body = ureq::get(&format!("http://localhost:{debug_port}/json/list"))
        .timeout(std::time::Duration::from_secs(2))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    // Hand-rolled scan — avoid `serde_json` to keep the binary lean
    // (same call we made in update_check.rs).
    let needle = "\"url\":\"http://localhost:";
    let mut from = 0;
    while let Some(rel) = body[from..].find(needle) {
        let start = from + rel + "\"url\":\"".len();
        let after = &body[start..];
        if let Some(end) = after.find('"') {
            let url = &after[..end];
            if !url.contains("data:") && !url.starts_with("chrome://") {
                return Some(url.to_string());
            }
        }
        from += rel + needle.len();
    }
    None
}

fn spawn_playwright_show(debug_port: u16) -> Result<(), String> {
    let mut cmd = std::process::Command::new("playwright-cli");
    cmd.arg("show")
        .env("PLAYWRIGHT_DASHBOARD_DEBUG_PORT", debug_port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().map_err(|e| {
        format!(
            "spawn playwright-cli: {e}. Is `playwright-cli` on PATH? Try `npm i -g @playwright/cli` or run from a directory with @playwright/cli in node_modules."
        )
    })?;
    Ok(())
}

/// Initialization script registered on a `browser.attach_dashboard_auto`
/// WebView. Runs *before* page scripts on every navigation, but the
/// `MutationObserver` waits for the dashboard's React tree to mount
/// the sidebar before clicking the first session + installing the
/// hide-chrome CSS. Self-disarming once a session is selected; gives
/// up after 10s to avoid a long-running observer if the page never
/// settles into the expected shape.
///
/// The observer uses the same CSS classes [`toggle_dashboard_chrome`]
/// targets (`.split-view-sidebar` / `.split-view-sash` /
/// `.split-view-main`), so `Cmd+Opt+H` can still toggle the chrome
/// back on if the user wants to switch sessions.
pub(crate) const DASHBOARD_AUTO_INIT_JS: &str = r#"
    (function() {
        if (window.__tmnlDashboardAutoInit) return;
        window.__tmnlDashboardAutoInit = true;
        var armed = false;
        var done = function() {
            if (armed) return true;
            var sidebar = document.querySelector('.split-view-sidebar');
            if (!sidebar) return false;
            var first = sidebar.querySelector('a[href], [role="treeitem"], [data-testid]')
                || sidebar.querySelector('li, [role="button"]')
                || sidebar.querySelector('div > div');
            if (!first) return false;
            try { first.click(); } catch (e) { /* swallow */ }
            armed = true;
            var styleId = 'tmnl-hide-dashboard-chrome';
            if (!document.getElementById(styleId)) {
                var s = document.createElement('style');
                s.id = styleId;
                s.textContent =
                    '.split-view-sidebar { display: none !important; }' +
                    '.split-view-sash { display: none !important; }' +
                    '.split-view-main { width: 100% !important; flex: 1 1 100% !important; }';
                document.head.appendChild(s);
            }
            return true;
        };
        var attach = function() {
            if (done()) return;
            var obs = new MutationObserver(function() {
                if (done()) obs.disconnect();
            });
            obs.observe(document.body || document.documentElement, {
                childList: true,
                subtree: true,
            });
            setTimeout(function() { obs.disconnect(); }, 10000);
        };
        if (document.readyState === 'loading') {
            document.addEventListener('DOMContentLoaded', attach, { once: true });
        } else {
            attach();
        }
    })();
"#;

/// Toggle the Playwright dashboard's left sidebar in the focused
/// Browser pane. Idempotent — re-running undoes the previous toggle
/// by re-checking `<style id="tmnl-hide-dashboard-chrome">`. The
/// CSS hides `.split-view-sidebar` (the session list) + its
/// resizer; the `.split-view-main` viewport stretches to fill the
/// pane via the dashboard's own flex layout.
fn toggle_dashboard_chrome(app: &mut crate::App) {
    use crate::PaneKind;
    let focused = app.tabs[app.active].focused;
    let pane = &app.tabs[app.active].panes[focused];
    let PaneKind::Browser { webview, .. } = &pane.kind else {
        eprintln!("tmnl: cmd+alt+h only works in a Browser pane");
        return;
    };
    let Some(v) = webview.as_ref() else {
        eprintln!("tmnl: this Browser pane's WebView didn't mount");
        return;
    };
    // Self-contained IIFE; toggles by inserting/removing the
    // `<style>` tag with a known id. Wrapped in try/catch so a
    // page that hasn't fully loaded doesn't surface as an error.
    let js = r#"
        (function() {
            try {
                var id = 'tmnl-hide-dashboard-chrome';
                var existing = document.getElementById(id);
                if (existing) {
                    existing.remove();
                } else {
                    var s = document.createElement('style');
                    s.id = id;
                    s.textContent =
                        '.split-view-sidebar { display: none !important; }' +
                        '.split-view-sash { display: none !important; }' +
                        '.split-view-main { width: 100% !important; flex: 1 1 100% !important; }';
                    document.head.appendChild(s);
                }
            } catch (e) {
                /* swallow */
            }
        })();
    "#;
    let _ = v.evaluate_script(js);
}

/// Write `text` to the system clipboard. Used by the text-selection
/// drag → copy path. `pbcopy` on macOS, `wl-copy` / `xclip -i` on
/// Linux, PowerShell `Set-Clipboard` on Windows. Errors are swallowed
/// + logged — a missing clipboard tool shouldn't crash the editor.
pub(crate) fn write_clipboard_text(text: &str) {
    use std::io::Write;
    let cmd: (&str, &[&str]);
    #[cfg(target_os = "macos")]
    {
        cmd = ("pbcopy", &[]);
    }
    #[cfg(target_os = "linux")]
    {
        cmd = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            ("wl-copy", &[])
        } else {
            ("xclip", &["-selection", "clipboard"])
        };
    }
    #[cfg(target_os = "windows")]
    {
        cmd = ("powershell", &["-NoProfile", "-Command", "Set-Clipboard"]);
    }
    let child = std::process::Command::new(cmd.0)
        .args(cmd.1)
        .stdin(std::process::Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        log::warn!("clipboard: spawn {} failed", cmd.0);
        return;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}

fn read_clipboard_text() -> Option<String> {
    let cmd: (&str, &[&str]);
    #[cfg(target_os = "macos")]
    {
        cmd = ("pbpaste", &[]);
    }
    #[cfg(target_os = "linux")]
    {
        cmd = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            ("wl-paste", &["--no-newline"])
        } else {
            ("xclip", &["-selection", "clipboard", "-o"])
        };
    }
    #[cfg(target_os = "windows")]
    {
        cmd = ("powershell", &["-NoProfile", "-Command", "Get-Clipboard"]);
    }
    let out = std::process::Command::new(cmd.0)
        .args(cmd.1)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Cmd+V → paste the system clipboard into the focused pane.
///
/// Two paths:
///
/// - **Native pane** (mnml/mixr/etc.) — forward as `Ctrl+V` so the
///   client's own paste binding fires. The client decides whether
///   to consume bytes from its own clipboard or request from us;
///   we never read the clipboard ourselves on its behalf.
/// - **Shell pane** — read the system clipboard (`pbpaste` /
///   `wl-paste` / PowerShell `Get-Clipboard`) and write the bytes
///   straight to the pty. Wraps the content in bracketed-paste
///   markers (`ESC[200~ … ESC[201~`) so zsh / bash / fish can
///   distinguish it from typed input + skip auto-execution of
///   multi-line pastes.
///
/// 2026-06-09 fix — prior version sent `Ctrl+V` to the shell, which
/// in zsh means `quoted-insert`, not paste, so Cmd+V appeared to do
/// nothing.
pub(crate) fn paste_into_focused(app: &mut App, ke: Option<&winit::event::KeyEvent>) {
    use crate::PaneKind;
    // Decide path WITHOUT a mutable borrow that conflicts with the
    // `forward_as_ctrl` call below (which itself takes `&mut App`).
    let is_shell = matches!(
        app.tabs[app.active].focused_pane().kind,
        PaneKind::Shell { .. }
    );
    if !is_shell {
        // Native / Browser / future kinds: forward as Ctrl+V so the
        // client decides what to do. Browser webview already handles
        // paste internally via its own AppKit chord pipeline; the
        // forward is a harmless no-op there.
        forward_as_ctrl(app, ke);
        return;
    }
    let Some(text) = read_clipboard_text() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    // Bracketed-paste markers. Modern shells parse these out;
    // older shells will see the literal escape bytes and may
    // misrender — acceptable trade-off since every shell tmnl
    // supports (zsh / bash / fish) handles bracketed paste
    // correctly out of the box.
    const BP_START: &[u8] = b"\x1b[200~";
    const BP_END: &[u8] = b"\x1b[201~";
    if let PaneKind::Shell { session } = &mut app.tabs[app.active].focused_pane_mut().kind
        && let Some(s) = session.as_mut()
    {
        s.write_bytes(BP_START);
        s.write_bytes(text.as_bytes());
        s.write_bytes(BP_END);
    }
}

/// Forward the current key event to the focused Native pane with
/// Cmd → Ctrl modifier remap. Used by Mac-style editing chords
/// (⌘Z/X/C/V/A/S/F/N/P/B/G/⌘/) so mnml's standard-mode bindings
/// (Ctrl+...) light up under Mac muscle memory. No-op for Shell
/// tabs (they're bare terminals where the OS already handles
/// Cmd-clipboard, EXCEPT paste — see `paste_into_focused`).
fn forward_as_ctrl(app: &mut App, ke: Option<&winit::event::KeyEvent>) {
    use crate::PaneKind;
    use crate::protocol::{InputEvent, KeyInput};
    let Some(ke) = ke else { return };
    if !matches!(
        &app.tabs[app.active].focused_pane().kind,
        PaneKind::Native { .. }
    ) {
        return;
    }
    let translated_mods = crate::pack_mods_cmd_to_ctrl(app.mods);
    if let PaneKind::Native { server, .. } = &mut app.tabs[app.active].focused_pane_mut().kind
        && let Some(code) = crate::translate_key(&ke.logical_key, app.mods)
    {
        server.send_input(&InputEvent::Key(KeyInput {
            code,
            mods: translated_mods,
            press: true,
        }));
    }
}

/// Tab N (0-indexed): Native tabs forward as ⌥(digit+1) so mnml's
/// `tab.goto_N` chord switches mnml tab pages; Shell tabs switch
/// tmnl tabs.
fn goto_tab_or_forward(app: &mut App, ke: Option<&winit::event::KeyEvent>, n: usize) {
    use crate::PaneKind;
    use crate::protocol::{InputEvent, KeyInput, MOD_ALT};
    if matches!(
        &app.tabs[app.active].focused_pane().kind,
        PaneKind::Native { .. }
    ) {
        // No key event ⇒ called from a non-keyboard dispatch path; the
        // Native pane has nothing to forward, so just switch the tmnl tab.
        let Some(ke) = ke else {
            app.switch_to_tab(n);
            if let Some(w) = &app.window {
                w.request_redraw();
            }
            return;
        };
        if let PaneKind::Native { server, .. } = &mut app.tabs[app.active].focused_pane_mut().kind
            && let Some(code) = crate::translate_key(&ke.logical_key, app.mods)
        {
            server.send_input(&InputEvent::Key(KeyInput {
                code,
                mods: MOD_ALT,
                press: true,
            }));
        }
        return;
    }
    app.switch_to_tab(n);
    if let Some(w) = &app.window {
        w.request_redraw();
    }
}

/// True when no modal overlay is capturing keystrokes — safe to
/// dispatch tab-management chords. The default guard for tmnl
/// chord migrations.
fn no_modal_open(app: &App) -> bool {
    app.welcome.is_none()
        && app.settings.is_none()
        && app.renaming_tab.is_none()
        && app.palette.is_none()
}

/// Initial command set — `Cmd`-prefixed tab/split management chords.
/// Migrating one at a time from `app.rs::handle_keyboard_input`. See
/// `docs/COMMAND_MIGRATION.md`.
fn builtin_commands() -> Vec<Command> {
    vec![
        // ⌘T — new tab of the same kind the window launched with
        // (Native when --editor was set, shell otherwise).
        Command {
            id: "tab.new",
            title: "New tab",
            group: "Tabs",
            keys: &["cmd+t"],
            run: |app, _event_loop, _ke| {
                if app.editor_template.is_some() {
                    app.new_native_tab();
                } else {
                    app.new_shell_tab();
                }
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌘⇧W — close the focused split pane (collapses split if
        // siblings remain; closes tab if last pane).
        Command {
            id: "pane.close",
            title: "Close focused split pane",
            group: "Splits",
            keys: &["cmd+shift+w"],
            run: |app, _event_loop, _ke| {
                app.close_focused_pane();
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌘D — split focused pane right (new shell pane).
        Command {
            id: "split.right",
            title: "Split right",
            group: "Splits",
            keys: &["cmd+d"],
            run: |app, _event_loop, _ke| {
                app.split_active_pane(crate::layout::SplitDir::Vertical);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌘⇧D — split focused pane down.
        Command {
            id: "split.down",
            title: "Split down",
            group: "Splits",
            keys: &["cmd+shift+d"],
            run: |app, _event_loop, _ke| {
                app.split_active_pane(crate::layout::SplitDir::Horizontal);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌥⌘B — split focused pane vertically, opening a Browser pane
        // at duckduckgo. Quick "I want to browse something" default.
        Command {
            id: "split.browser_right",
            title: "Split right with browser",
            group: "Splits",
            keys: &["cmd+alt+b"],
            run: |app, _event_loop, _ke| {
                app.split_active_pane_browser(
                    crate::layout::SplitDir::Vertical,
                    "https://duckduckgo.com".to_string(),
                );
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌥⌘V — paste clipboard URL into a new Browser pane split.
        // The dashboard workflow: run `playwright-cli show` in any
        // terminal, copy the URL from Chrome's address bar, hit
        // Cmd+Opt+V here, the dashboard renders inside a tmnl pane.
        // Same pattern works for any URL.
        Command {
            id: "split.browser_clipboard",
            title: "Split right with browser at clipboard URL",
            group: "Splits",
            keys: &["cmd+alt+v"],
            run: |app, _event_loop, _ke| {
                let Some(url) = read_clipboard_url() else {
                    eprintln!("tmnl: clipboard doesn't look like a URL (need http:// or https://)");
                    return;
                };
                app.split_active_pane_browser(crate::layout::SplitDir::Vertical, url);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌥⌘D — spawn (or attach to an existing) Playwright
        // dashboard, embed it in a new Browser pane. Handles the
        // "go to terminal and run playwright-cli show" step + the
        // "copy URL from Chrome" step in one chord.
        //
        // Mechanism: spawns `playwright-cli show` with
        // `PLAYWRIGHT_DASHBOARD_DEBUG_PORT=9222` set, so the
        // dashboard's chromium goes headless (no floating window)
        // but the HTTP server still serves on a random port. We
        // poll CDP at 9222 for up to ~5s, ask `/json/list` for
        // the open targets, and pick the one whose URL points at
        // localhost — that's the dashboard URL. Open it in a wry
        // pane. After that the user clicks a session in the
        // dashboard's sidebar + Cmd+Opt+H to hide chrome.
        //
        // If `playwright-cli` isn't on PATH the chord no-ops with
        // an stderr hint.
        Command {
            id: "browser.attach_dashboard",
            title: "Spawn + attach Playwright dashboard",
            group: "Browser",
            keys: &["cmd+alt+d"],
            run: |app, _event_loop, _ke| match spawn_or_attach_dashboard(9222) {
                Ok(url) => {
                    app.split_active_pane_browser(crate::layout::SplitDir::Vertical, url);
                    if let Some(w) = &app.window {
                        w.request_redraw();
                    }
                }
                Err(e) => eprintln!("tmnl: dashboard attach failed — {e}"),
            },
            when: Some(no_modal_open),
        },
        // ⌘⇧P / F1 — family-wide "command palette" chord. Native
        // pane: forward as ⌃⇧P so mnml's command palette opens
        // (its existing `ctrl+shift+p` binding). Anywhere else
        // (Shell, Browser, no pane): open tmnl's own palette
        // overlay over the registry. Family-wide single chord.
        Command {
            id: "palette.open_or_forward",
            title: "Command palette",
            group: "View",
            keys: &["cmd+shift+p", "f1"],
            run: |app, _event_loop, ke| {
                use crate::PaneKind;
                use crate::protocol::{InputEvent, KeyInput, MOD_CTRL, MOD_SHIFT};
                // Native pane + we have a real KeyEvent ⇒ forward.
                if let Some(ke) = ke
                    && matches!(
                        &app.tabs[app.active].focused_pane().kind,
                        PaneKind::Native { .. }
                    )
                {
                    if let PaneKind::Native { server, .. } =
                        &mut app.tabs[app.active].focused_pane_mut().kind
                        && let Some(code) = crate::translate_key(&ke.logical_key, app.mods)
                    {
                        server.send_input(&InputEvent::Key(KeyInput {
                            code,
                            mods: MOD_CTRL | MOD_SHIFT,
                            press: true,
                        }));
                    }
                    return;
                }
                // Otherwise open the standalone palette overlay.
                app.palette = Some(crate::palette::PaletteState::new());
                // Aggregate commands from every connected Native pane.
                // Each responds independently via `Message::ClientCommands`;
                // the App side appends them to `palette.remote_commands`
                // tagged with source pane index.
                app.request_client_commands_for_palette();
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌥⌘⇧D — same as `browser.attach_dashboard` but registers a
        // page-load script that clicks the first session row + hides
        // the chrome as soon as the dashboard's React tree mounts.
        // One-chord ergonomics for the common "watch the first slot"
        // case; the user can still Cmd+Opt+H to bring the chrome back.
        Command {
            id: "browser.attach_dashboard_auto",
            title: "Auto-attach Playwright dashboard (first session, hide chrome)",
            group: "Browser",
            keys: &["cmd+alt+shift+d"],
            run: |app, _event_loop, _ke| match spawn_or_attach_dashboard(9222) {
                Ok(url) => {
                    app.split_active_pane_browser_with_init(
                        crate::layout::SplitDir::Vertical,
                        url,
                        Some(DASHBOARD_AUTO_INIT_JS),
                    );
                    if let Some(w) = &app.window {
                        w.request_redraw();
                    }
                }
                Err(e) => eprintln!("tmnl: dashboard auto-attach failed — {e}"),
            },
            when: Some(no_modal_open),
        },
        // ⌥⌘H — toggle the Playwright dashboard's chrome (sidebar
        // + split-view sash) in the focused Browser pane. After
        // running `playwright-cli show` and pasting its URL with
        // Cmd+Opt+V, click the session you want, then Cmd+Opt+H to
        // hide the sidebar — the session's viewport fills the
        // pane. Cmd+Opt+H again to bring the sidebar back to
        // switch sessions. Same wry pane, same dashboard URL,
        // same WebSocket / CDP-screencast performance — we're
        // just toggling CSS.
        Command {
            id: "browser.toggle_dashboard_chrome",
            title: "Toggle Playwright dashboard chrome (sidebar)",
            group: "Browser",
            keys: &["cmd+alt+h"],
            run: |app, _event_loop, _ke| {
                toggle_dashboard_chrome(app);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌘⇧[ — cycle tab backward.
        Command {
            id: "tab.cycle_back",
            title: "Cycle to previous tab",
            group: "Tabs",
            keys: &["cmd+shift+["],
            run: |app, _event_loop, _ke| {
                app.cycle_tab(false);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌘⇧] — cycle tab forward.
        Command {
            id: "tab.cycle_forward",
            title: "Cycle to next tab",
            group: "Tabs",
            keys: &["cmd+shift+]"],
            run: |app, _event_loop, _ke| {
                app.cycle_tab(true);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // ⌘W — Native tabs forward as ⌃W (mnml closes its buffer
        // with confirmation prompt); Shell tabs close the whole tab.
        Command {
            id: "tab.close_or_forward",
            title: "Close tab / forward ⌃W to Native",
            group: "Tabs",
            keys: &["cmd+w"],
            run: |app, _event_loop, ke| {
                use crate::PaneKind;
                use crate::protocol::{InputEvent, KeyInput};
                if let Some(ke) = ke
                    && matches!(
                        &app.tabs[app.active].focused_pane().kind,
                        PaneKind::Native { .. }
                    )
                {
                    let translated_mods = crate::pack_mods_cmd_to_ctrl(app.mods);
                    if let PaneKind::Native { server, .. } =
                        &mut app.tabs[app.active].focused_pane_mut().kind
                        && let Some(code) = crate::translate_key(&ke.logical_key, app.mods)
                    {
                        server.send_input(&InputEvent::Key(KeyInput {
                            code,
                            mods: translated_mods,
                            press: true,
                        }));
                    }
                } else {
                    app.close_active_tab();
                    if let Some(w) = &app.window {
                        w.request_redraw();
                    }
                }
            },
            when: Some(no_modal_open),
        },
        // ⌘1..⌘9 — Native tabs forward as ⌥N (mnml's tab.goto_N);
        // Shell tabs switch tmnl tabs. 9 commands (one per digit)
        // since `keys` is a static slice.
        Command {
            id: "tab.goto_1",
            title: "Jump to tab 1",
            group: "Tabs",
            keys: &["cmd+1"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 0),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_2",
            title: "Jump to tab 2",
            group: "Tabs",
            keys: &["cmd+2"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 1),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_3",
            title: "Jump to tab 3",
            group: "Tabs",
            keys: &["cmd+3"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 2),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_4",
            title: "Jump to tab 4",
            group: "Tabs",
            keys: &["cmd+4"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 3),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_5",
            title: "Jump to tab 5",
            group: "Tabs",
            keys: &["cmd+5"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 4),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_6",
            title: "Jump to tab 6",
            group: "Tabs",
            keys: &["cmd+6"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 5),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_7",
            title: "Jump to tab 7",
            group: "Tabs",
            keys: &["cmd+7"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 6),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_8",
            title: "Jump to tab 8",
            group: "Tabs",
            keys: &["cmd+8"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 7),
            when: Some(no_modal_open),
        },
        Command {
            id: "tab.goto_9",
            title: "Jump to tab 9",
            group: "Tabs",
            keys: &["cmd+9"],
            run: |app, _el, ke| goto_tab_or_forward(app, ke, 8),
            when: Some(no_modal_open),
        },
        // Font zoom: ⌘= / ⌘+ in, ⌘- / ⌘_ out, ⌘0 reset.
        Command {
            id: "view.zoom_in",
            title: "Zoom font in",
            group: "View",
            keys: &["cmd+=", "cmd+shift+="],
            run: |app, _event_loop, _ke| {
                app.zoom_font(crate::FONT_ZOOM_STEP);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        Command {
            id: "view.zoom_out",
            title: "Zoom font out",
            group: "View",
            keys: &["cmd+-", "cmd+shift+-"],
            run: |app, _event_loop, _ke| {
                app.zoom_font(-crate::FONT_ZOOM_STEP);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        Command {
            id: "view.zoom_reset",
            title: "Reset font zoom",
            group: "View",
            keys: &["cmd+0"],
            run: |app, _event_loop, _ke| {
                app.reset_font_zoom();
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // Manual theme reload. The tick loop already polls mnml's
        // config file for mtime changes so live reload is automatic
        // in the common case; this is the escape hatch for situations
        // where the auto-poll missed (mnml writing the file
        // out-of-band, network-mounted homedir, etc.).
        Command {
            id: "theme.refresh",
            title: "Theme: reload chrome palette from mnml",
            group: "View",
            keys: &[],
            run: |app, _event_loop, _ke| {
                let changed = crate::theme::refresh();
                if changed && let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: None,
        },
        // ⌘⇧/ — Toggle the help overlay (lists every chord in the
        // registry, grouped by section). macOS Help-key convention.
        Command {
            id: "view.help",
            title: "Toggle help overlay",
            group: "View",
            keys: &["cmd+shift+/", "cmd+?"],
            run: |app, _el, _ke| {
                if app.help.is_some() {
                    app.help = None;
                } else {
                    app.help = Some(crate::help::HelpState::new());
                }
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            // No `no_modal_open` guard — pressing ⌘⇧/ in any state
            // toggles the overlay. (Welcome/settings/rename modals
            // intercept the chord before try_dispatch runs anyway,
            // since they have their own greedy handlers above.)
            when: None,
        },
        // ⌘I — AI completion of the current command line (Shell mode).
        Command {
            id: "ai.completion",
            title: "AI: complete current command line",
            group: "AI",
            keys: &["cmd+i"],
            run: |app, _el, _ke| app.trigger_ai_completion(),
            when: Some(no_modal_open),
        },
        // ⌘K — Generate a command from a typed description.
        Command {
            id: "ai.generate",
            title: "AI: generate command from description",
            group: "AI",
            keys: &["cmd+k"],
            run: |app, _el, _ke| app.trigger_ai_generate(),
            when: Some(no_modal_open),
        },
        // ⌘⌥ + Arrow — focus the split pane in that direction.
        // Works in both Shell and Native tabs (consumed locally).
        Command {
            id: "focus.left",
            title: "Focus pane ←",
            group: "Splits",
            keys: &["cmd+alt+left"],
            run: |app, _el, _ke| {
                app.focus_dir(crate::FocusDir::Left);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        Command {
            id: "focus.right",
            title: "Focus pane →",
            group: "Splits",
            keys: &["cmd+alt+right"],
            run: |app, _el, _ke| {
                app.focus_dir(crate::FocusDir::Right);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        Command {
            id: "focus.up",
            title: "Focus pane ↑",
            group: "Splits",
            keys: &["cmd+alt+up"],
            run: |app, _el, _ke| {
                app.focus_dir(crate::FocusDir::Up);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        Command {
            id: "focus.down",
            title: "Focus pane ↓",
            group: "Splits",
            keys: &["cmd+alt+down"],
            run: |app, _el, _ke| {
                app.focus_dir(crate::FocusDir::Down);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // Mac-style editing/navigation chords — translated to
        // Ctrl-equivalent and forwarded to the focused Native pane.
        // Shell tabs fall through to the bare terminal (which the OS
        // already handles for ⌘C/V copy/paste).
        Command {
            id: "fwd.cmd_z",
            title: "Undo (⌘Z → ⌃Z forwarded to Native)",
            group: "Forwarded chords",
            keys: &["cmd+z"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_x",
            title: "Cut (⌘X → ⌃X)",
            group: "Forwarded chords",
            keys: &["cmd+x"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_c",
            title: "Copy (⌘C → ⌃C)",
            group: "Forwarded chords",
            keys: &["cmd+c"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_v",
            title: "Paste (⌘V)",
            group: "Forwarded chords",
            keys: &["cmd+v"],
            run: |a, _, k| paste_into_focused(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_a",
            title: "Select all (⌘A → ⌃A)",
            group: "Forwarded chords",
            keys: &["cmd+a"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_s",
            title: "Save (⌘S → ⌃S)",
            group: "Forwarded chords",
            keys: &["cmd+s"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "find.open",
            title: "Find in pane (⌘F)",
            group: "View",
            keys: &["cmd+f"],
            run: |a, _, _| {
                a.open_find();
            },
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_n",
            title: "New (⌘N → ⌃N)",
            group: "Forwarded chords",
            keys: &["cmd+n"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_p",
            title: "File picker (⌘P → ⌃P)",
            group: "Forwarded chords",
            keys: &["cmd+p"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_b",
            title: "Toggle tree (⌘B → ⌃B)",
            group: "Forwarded chords",
            keys: &["cmd+b"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_g",
            title: "Goto line (⌘G → ⌃G)",
            group: "Forwarded chords",
            keys: &["cmd+g"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        Command {
            id: "fwd.cmd_slash",
            title: "Toggle comment (⌘/ → ⌃/)",
            group: "Forwarded chords",
            keys: &["cmd+/"],
            run: |a, _, k| forward_as_ctrl(a, k),
            when: Some(no_modal_open),
        },
        // Shift+PageUp / Shift+PageDown — scroll shell scrollback.
        Command {
            id: "scroll.page_up",
            title: "Scroll up (shell scrollback)",
            group: "View",
            keys: &["shift+pageup"],
            run: |app, _el, _ke| {
                use crate::PaneKind;
                let page = app
                    .gpu
                    .as_ref()
                    .map_or(20, |g| g.grid.rows.saturating_sub(1) as i32);
                if let PaneKind::Shell { session: Some(s) } =
                    &mut app.tabs[app.active].focused_pane_mut().kind
                {
                    s.scroll(page);
                }
            },
            when: Some(no_modal_open),
        },
        Command {
            id: "scroll.page_down",
            title: "Scroll down (shell scrollback)",
            group: "View",
            keys: &["shift+pagedown"],
            run: |app, _el, _ke| {
                use crate::PaneKind;
                let page = app
                    .gpu
                    .as_ref()
                    .map_or(20, |g| g.grid.rows.saturating_sub(1) as i32);
                if let PaneKind::Shell { session: Some(s) } =
                    &mut app.tabs[app.active].focused_pane_mut().kind
                {
                    s.scroll(-page);
                }
            },
            when: Some(no_modal_open),
        },
    ]
}
