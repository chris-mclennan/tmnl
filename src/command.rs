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

pub type CommandFn = fn(&mut App, &ActiveEventLoop, &KeyEvent);
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
        run(app, event_loop, key);
        return true;
    }
    false
}

/// Tab N (0-indexed): Native tabs forward as ⌥(digit+1) so mnml's
/// `tab.goto_N` chord switches mnml tab pages; Shell tabs switch
/// tmnl tabs.
fn goto_tab_or_forward(app: &mut App, ke: &winit::event::KeyEvent, n: usize) {
    use crate::PaneKind;
    use crate::protocol::{InputEvent, KeyInput, MOD_ALT};
    if matches!(
        &app.tabs[app.active].focused_pane().kind,
        PaneKind::Native { .. }
    ) {
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
            run: |app, event_loop, _ke| {
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
            run: |app, event_loop, ke| {
                use crate::PaneKind;
                use crate::protocol::{InputEvent, KeyInput};
                if matches!(
                    &app.tabs[app.active].focused_pane().kind,
                    PaneKind::Native { .. }
                ) {
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
                    app.close_active_tab(event_loop);
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
    ]
}
