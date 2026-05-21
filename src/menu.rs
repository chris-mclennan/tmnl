//! Native macOS menu bar — tmnl / Shell / Edit / View / Window / Help.
//!
//! Predefined items (About, Hide, Show All, Quit, Cut/Copy/Paste, Minimize…)
//! delegate to AppKit so macOS handles them with the OS's own actions —
//! cmd-key bindings, the system Edit menu intercepts, etc. Custom items
//! get an ID we route through `drain_menu_events` in the event loop.

use muda::{
    AboutMetadata, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};

/// Wrapper that holds the live menu + the IDs we need to route.
pub struct AppMenu {
    /// muda's `Menu` must live for the lifetime of the app — drop it and
    /// the menu disappears.
    #[allow(dead_code)]
    menu: Menu,
    pub id_settings: MenuId,
    pub id_new_window: MenuId,
    pub id_font_inc: MenuId,
    pub id_font_dec: MenuId,
    pub id_font_reset: MenuId,
    pub id_toggle_fullscreen: MenuId,
    pub id_tmnl_help: MenuId,
}

/// Lightweight ID-only snapshot so the dispatcher can compare without
/// borrowing the whole `AppMenu` (which would conflict with `&mut self`).
pub struct MenuIds {
    pub id_settings: MenuId,
    pub id_new_window: MenuId,
    pub id_font_inc: MenuId,
    pub id_font_dec: MenuId,
    pub id_font_reset: MenuId,
    pub id_toggle_fullscreen: MenuId,
    pub id_tmnl_help: MenuId,
}

impl AppMenu {
    pub fn clone_ids(&self) -> MenuIds {
        MenuIds {
            id_settings: self.id_settings.clone(),
            id_new_window: self.id_new_window.clone(),
            id_font_inc: self.id_font_inc.clone(),
            id_font_dec: self.id_font_dec.clone(),
            id_font_reset: self.id_font_reset.clone(),
            id_toggle_fullscreen: self.id_toggle_fullscreen.clone(),
            id_tmnl_help: self.id_tmnl_help.clone(),
        }
    }
}

impl AppMenu {
    /// Build the menu bar and install it as the global app menu on macOS.
    pub fn build_and_install() -> Self {
        let menu = Menu::new();

        // ── tmnl (app menu) — macOS puts the first submenu's title here
        //    automatically, replacing it with the bundle's actual name.
        let app_menu = Submenu::new("tmnl", true);
        let id_settings = MenuId::new("settings");
        let settings = MenuItem::with_id(
            id_settings.clone(),
            "Settings…",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Comma)),
        );
        app_menu
            .append_items(&[
                &PredefinedMenuItem::about(
                    Some("About tmnl"),
                    Some(AboutMetadata {
                        name: Some("tmnl".to_string()),
                        version: Some(env!("CARGO_PKG_VERSION").to_string()),
                        ..Default::default()
                    }),
                ),
                &PredefinedMenuItem::separator(),
                &settings,
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ])
            .expect("build app menu");

        // ── Shell
        let shell_menu = Submenu::new("Shell", true);
        let id_new_window = MenuId::new("new_window");
        let new_window = MenuItem::with_id(
            id_new_window.clone(),
            "New Window",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::KeyN)),
        );
        // Pane verbs. These carry plain string IDs (routed by
        // `drain_menu_events`) rather than stored `MenuId` fields —
        // `MenuId: PartialEq<&str>` makes the dispatch a direct compare.
        // The accelerators mirror the tmnl-level keyboard chords, so the
        // menus double as a discoverable reference. Split + close live
        // in the Shell menu; pane-focus navigation in the Window menu.
        let split_item = |id: &str, label: &str, mods: Modifiers, code: Code| {
            MenuItem::with_id(
                MenuId::new(id),
                label,
                true,
                Some(Accelerator::new(Some(mods), code)),
            )
        };
        let sup = Modifiers::SUPER;
        let sup_shift = Modifiers::SUPER | Modifiers::SHIFT;
        let sup_alt = Modifiers::SUPER | Modifiers::ALT;
        let split_right = split_item("split_right", "Split Right", sup, Code::KeyD);
        let split_down = split_item("split_down", "Split Down", sup_shift, Code::KeyD);
        let focus_left = split_item("focus_left", "Focus Pane Left", sup_alt, Code::ArrowLeft);
        let focus_right = split_item("focus_right", "Focus Pane Right", sup_alt, Code::ArrowRight);
        let focus_up = split_item("focus_up", "Focus Pane Up", sup_alt, Code::ArrowUp);
        let focus_down = split_item("focus_down", "Focus Pane Down", sup_alt, Code::ArrowDown);
        let close_pane = split_item("close_pane", "Close Pane", sup_shift, Code::KeyW);
        shell_menu
            .append_items(&[
                &new_window,
                &PredefinedMenuItem::separator(),
                &split_right,
                &split_down,
                &close_pane,
            ])
            .expect("build Shell menu");

        // ── Edit — predefined items hand off to AppKit so the OS
        //    intercepts cmd-X/C/V/A natively in any focused text field.
        let edit_menu = Submenu::new("Edit", true);
        edit_menu
            .append_items(&[
                &PredefinedMenuItem::undo(None),
                &PredefinedMenuItem::redo(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::cut(None),
                &PredefinedMenuItem::copy(None),
                &PredefinedMenuItem::paste(None),
                &PredefinedMenuItem::select_all(None),
            ])
            .expect("build Edit menu");

        // ── View
        let view_menu = Submenu::new("View", true);
        let id_font_inc = MenuId::new("font_inc");
        let id_font_dec = MenuId::new("font_dec");
        let id_font_reset = MenuId::new("font_reset");
        let id_toggle_fullscreen = MenuId::new("toggle_fullscreen");
        let font_inc = MenuItem::with_id(
            id_font_inc.clone(),
            "Increase Font Size",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Equal)),
        );
        let font_dec = MenuItem::with_id(
            id_font_dec.clone(),
            "Decrease Font Size",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Minus)),
        );
        let font_reset = MenuItem::with_id(
            id_font_reset.clone(),
            "Reset Font Size",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Digit0)),
        );
        let toggle_full = MenuItem::with_id(
            id_toggle_fullscreen.clone(),
            "Toggle Full Screen",
            true,
            Some(Accelerator::new(
                Some(Modifiers::SUPER | Modifiers::CONTROL),
                Code::KeyF,
            )),
        );
        view_menu
            .append_items(&[
                &font_inc,
                &font_dec,
                &font_reset,
                &PredefinedMenuItem::separator(),
                &toggle_full,
            ])
            .expect("build View menu");

        // ── Window — macOS's home for navigation: pane-focus moves
        //    live here, the split / close verbs stay under Shell.
        let window_menu = Submenu::new("Window", true);
        window_menu
            .append_items(&[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::separator(),
                &focus_left,
                &focus_right,
                &focus_up,
                &focus_down,
            ])
            .expect("build Window menu");

        // ── Help
        let help_menu = Submenu::new("Help", true);
        let id_tmnl_help = MenuId::new("help");
        let tmnl_help = MenuItem::with_id(id_tmnl_help.clone(), "tmnl Help", true, None);
        help_menu
            .append_items(&[&tmnl_help])
            .expect("build Help menu");

        menu.append_items(&[
            &app_menu,
            &shell_menu,
            &edit_menu,
            &view_menu,
            &window_menu,
            &help_menu,
        ])
        .expect("append top-level submenus");

        // Set the global NSApp menu — the OS picks this up immediately.
        #[cfg(target_os = "macos")]
        menu.init_for_nsapp();

        AppMenu {
            menu,
            id_settings,
            id_new_window,
            id_font_inc,
            id_font_dec,
            id_font_reset,
            id_toggle_fullscreen,
            id_tmnl_help,
        }
    }
}
