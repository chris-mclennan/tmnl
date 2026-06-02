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

pub type CommandFn = fn(&mut App, &ActiveEventLoop);
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
        run(app, event_loop);
        return true;
    }
    false
}

/// True when no modal overlay is capturing keystrokes — safe to
/// dispatch tab-management chords. The default guard for tmnl
/// chord migrations.
fn no_modal_open(app: &App) -> bool {
    app.welcome.is_none() && app.settings.is_none() && app.renaming_tab.is_none()
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
            run: |app, _event_loop| {
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
            run: |app, event_loop| {
                app.close_focused_pane(event_loop);
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
            run: |app, _event_loop| {
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
            run: |app, _event_loop| {
                app.split_active_pane(crate::layout::SplitDir::Horizontal);
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
            run: |app, _event_loop| {
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
            run: |app, _event_loop| {
                app.cycle_tab(true);
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
        // Font zoom: ⌘= / ⌘+ in, ⌘- / ⌘_ out, ⌘0 reset.
        Command {
            id: "view.zoom_in",
            title: "Zoom font in",
            group: "View",
            keys: &["cmd+=", "cmd+shift+="],
            run: |app, _event_loop| {
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
            run: |app, _event_loop| {
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
            run: |app, _event_loop| {
                app.reset_font_zoom();
                if let Some(w) = &app.window {
                    w.request_redraw();
                }
            },
            when: Some(no_modal_open),
        },
    ]
}
