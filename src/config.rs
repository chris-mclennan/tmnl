//! tmnl's persisted settings — `~/.config/tmnl/config.toml`.
//!
//! Load-on-startup, save-from-Settings-window. CLI flags + env vars
//! still win at launch time (escape hatches); the Settings window edits
//! and persists this file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Pixel inset around the shell-prompt view (apple-terminal style
    /// padding so the prompt doesn't hug the window edge). Applied
    /// only in shell mode while a full-screen TUI is NOT active —
    /// `inset_native` covers native-mode + alt-screen TUIs.
    pub inset: f32,
    /// Pixel inset around native-mode TUIs (mnml / mixr / etc.) and
    /// shell-mode children that have switched to the xterm alt-screen.
    ///
    /// **Defaults to 0** — design rule is the hosted TUI itself
    /// decides what to do with the full-screen space (push elements
    /// to the edge, or add its own internal padding where its panel
    /// borders need breathing room). Set to 1 for the "customary"
    /// minimal padding, or higher for more if you'd rather the host
    /// own the margin than the TUI.
    pub inset_native: f32,
    /// How tab chips lay out when 2+ tabs are open.
    ///
    /// * `Horizontal` (default) — chips flow left-to-right in the
    ///   chrome strip below the palette cluster. When the row fills,
    ///   chips wrap to a new row; the strip grows downward by one
    ///   `TAB_ROW_H_PX` per added row.
    /// * `Vertical` — chips stack down a left-edge sidebar. Strip
    ///   stays single-row (just the palette); the grid's `inset_x`
    ///   grows to accommodate the sidebar. Better for narrow
    ///   windows or users who keep many tabs open. (v0.x — currently
    ///   logs a "not yet implemented" message on apply; falls back
    ///   to horizontal.)
    pub tab_layout: TabLayout,
    /// Themed powerline prompt for spawned shells. When `true`, tmnl
    /// exports `MNML_PROMPT_SCRIPT` + `MNML_CONTEXT=tmnl` (and the
    /// active theme palette as `MNML_PROMPT_*` colour vars) so the
    /// user's `~/.zshrc` source line picks up the prompt. When
    /// `false`, no env vars are exported and shells get the user's
    /// normal prompt — the rc-file source line silently no-ops, so
    /// flipping back to `true` is just an env-var change with no
    /// rc-file edit.
    ///
    /// First time it's flipped on (from any path — settings UI, CLI
    /// flag, manual toml edit), tmnl checks the user's `~/.zshrc`
    /// (and `.bashrc` if present) and appends the standard source
    /// line if it's missing. See `shell_prompt::ensure_rc_sourced`.
    pub themed_prompt: bool,
    /// Icons in the left-edge launcher rail. Each entry renders as
    /// one glyph in a narrow vertical column at the window's left
    /// edge; left-click spawns `command` (with `args`) in a new tab.
    /// Empty (the default) ⇒ no rail, no left-edge column.
    ///
    /// TOML shape:
    /// ```toml
    /// [[launcher_icon]]
    /// id      = "slack"
    /// glyph   = ""
    /// command = "mnml-msg-slack"
    /// tooltip = "Slack"
    ///
    /// [[launcher_icon]]
    /// id      = "mixr"
    /// glyph   = "♪"
    /// command = "mixr-rs"
    /// args    = ["--dashboard"]
    /// color   = "#7aa2f7"
    /// ```
    #[serde(default, rename = "launcher_icon")]
    pub launcher_icons: Vec<LauncherIcon>,
    /// Where launcher icons render. Default: `Left` (vertical
    /// column on window's left edge). `Top` puts them inline in
    /// the bufferline; `Bottom` is reserved (not yet wired).
    #[serde(default)]
    pub launcher_position: LauncherPosition,
    /// Anchor shell prompts at the bottom of the body, Warp-style.
    /// Default `Natural` matches every other terminal.
    #[serde(default)]
    pub prompt_position: PromptPosition,
    /// Show the "welcome to tmnl" overlay on startup. Default
    /// `true` — flip via Settings or the panel's `D` ("don't show
    /// again") action. When `false`, tmnl drops straight into the
    /// shell tab without surfacing the recents picker.
    #[serde(default = "default_show_welcome")]
    pub show_welcome: bool,
}

fn default_show_welcome() -> bool {
    true
}

/// One entry in the left-edge launcher rail. `id` is the stable
/// handle used by future palette commands (`launcher.open.<id>`);
/// every other field is presentation or invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherIcon {
    /// Stable identifier — must be unique across launcher_icons in
    /// a config. Used for palette command names + log lines. Not
    /// shown in the UI.
    pub id: String,
    /// Single nerd-font (or unicode) glyph painted in the rail.
    /// Multi-char strings render their first cell only.
    pub glyph: String,
    /// Binary to spawn on click. Resolved against `$PATH` at spawn
    /// time — same lookup rules as a normal shell command.
    pub command: String,
    /// Extra args appended after the binary. Default: none.
    #[serde(default)]
    pub args: Vec<String>,
    /// Text shown when the user hovers the icon. Default: `command`.
    /// (Tooltip rendering itself is a follow-up — wired in v0.2.)
    #[serde(default)]
    pub tooltip: String,
    /// `#rrggbb` accent for the glyph foreground. Defaults to the
    /// active palette's `accent_fg`. Invalid hex falls back the same way.
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TabLayout {
    #[default]
    Horizontal,
    Vertical,
}

/// Whether shell prompts render at their natural cursor position
/// or anchored to the bottom of the body grid (Warp / Claude Code
/// style). v1: render-time shift only — empty rows appear at top
/// of the body when output is short. Full Warp-style scrollback
/// rendering is a later iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptPosition {
    /// Prompt sits wherever the shell put it. Natural terminal
    /// behavior — short output stays at the top, long output
    /// scrolls naturally.
    #[default]
    Natural,
    /// Prompt is shifted to the bottom row regardless of how much
    /// output preceded it. Empty rows fill the top until enough
    /// output exists to fill the grid.
    Bottom,
}

/// Where the launcher rail's icons render.
///
/// * `Left` — vertical column on the window's left edge (default;
///   matches mnml's `> INTEGRATIONS` placement).
/// * `Top` — chips inline in the bufferline strip after the `+`
///   new-tab chip.
/// * `Bottom` — chips on a dedicated bottom strip below the body
///   grid. Not yet wired — selecting it currently falls back to
///   `Left` with a log line; queued in TODO.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LauncherPosition {
    #[default]
    Left,
    Top,
    Bottom,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            inset: 20.0,
            inset_native: 0.0,
            tab_layout: TabLayout::Horizontal,
            themed_prompt: false,
            launcher_icons: Vec::new(),
            launcher_position: LauncherPosition::Left,
            prompt_position: PromptPosition::Natural,
            show_welcome: true,
        }
    }
}

impl Config {
    /// Inset to use right now. Returns the configured `inset_native`
    /// when a TUI has taken over (native mode or alt-screen), otherwise
    /// the configured `inset` (shell-prompt padding).
    pub fn active_inset(&self, tui_active: bool) -> f32 {
        if tui_active {
            self.inset_native.max(0.0)
        } else {
            self.inset.max(0.0)
        }
    }

    /// Load `~/.config/tmnl/config.toml`. Missing file ⇒ defaults; parse
    /// failure ⇒ defaults + log so we don't crash on a user typo.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Config::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        match toml::from_str::<Config>(&text) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("config: parse {} failed: {e}", path.display());
                Config::default()
            }
        }
    }

    /// Persist to `~/.config/tmnl/config.toml`. Creates the parent dir
    /// if needed. Errors swallowed + logged — we don't want a failed
    /// save to crash the running session.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = config_path() else {
            return Err(std::io::Error::other("no config dir"));
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, text)?;
        log::info!("config: saved → {}", path.display());
        Ok(())
    }
}

pub fn config_path() -> Option<PathBuf> {
    // Use the XDG location explicitly — `dirs::config_dir()` returns
    // `~/Library/Application Support/` on macOS, which would split
    // config.toml off from recents.toml (which always uses
    // `~/.config/tmnl/`). Every doc + the welcome page references
    // `~/.config/tmnl/config.toml`, so we standardize on XDG across
    // platforms. SEV-2 chrome-hunt finding from 2026-06-07.
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("tmnl").join("config.toml"));
    }
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("tmnl")
            .join("config.toml")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_inset_is_twenty() {
        assert_eq!(Config::default().inset, 20.0);
    }

    /// `Config { .. }` with both `inset` + `inset_native` set; trims
    /// boilerplate in the assertions below.
    fn cfg(inset: f32, inset_native: f32) -> Config {
        Config {
            inset,
            inset_native,
            tab_layout: TabLayout::Horizontal,
            themed_prompt: false,
            launcher_icons: Vec::new(),
            launcher_position: LauncherPosition::Left,
            prompt_position: PromptPosition::Natural,
            show_welcome: true,
        }
    }

    #[test]
    fn active_inset_uses_inset_native_under_a_tui() {
        // Native mode / alt-screen ⇒ the `inset_native` value (0 by
        // default; user can configure a small breathing-room band).
        assert_eq!(cfg(20.0, 0.0).active_inset(true), 0.0);
        assert_eq!(cfg(20.0, 1.0).active_inset(true), 1.0);
    }

    #[test]
    fn active_inset_uses_the_config_for_the_shell_prompt() {
        assert_eq!(cfg(14.0, 0.0).active_inset(false), 14.0);
    }

    #[test]
    fn active_inset_clamps_a_negative_config() {
        // A bogus negative inset can't push content off-window.
        assert_eq!(cfg(-5.0, 0.0).active_inset(false), 0.0);
        assert_eq!(cfg(20.0, -5.0).active_inset(true), 0.0);
    }

    #[test]
    fn toml_empty_string_falls_back_to_defaults() {
        // `#[serde(default)]` ⇒ a missing field takes the default.
        let c: Config = toml::from_str("").expect("empty toml parses");
        assert_eq!(c.inset, 20.0);
        assert_eq!(c.inset_native, 0.0);
    }

    #[test]
    fn toml_parses_an_explicit_inset() {
        let c: Config = toml::from_str("inset = 8.5").expect("toml parses");
        assert_eq!(c.inset, 8.5);
    }

    #[test]
    fn toml_parses_an_explicit_inset_native() {
        let c: Config = toml::from_str("inset_native = 2.0").expect("toml parses");
        assert_eq!(c.inset_native, 2.0);
    }

    #[test]
    fn toml_round_trips_through_serialize() {
        let text = toml::to_string_pretty(&cfg(31.0, 4.0)).expect("serialize");
        let back: Config = toml::from_str(&text).expect("re-parse");
        assert_eq!(back.inset, 31.0);
        assert_eq!(back.inset_native, 4.0);
    }

    #[test]
    fn tab_layout_defaults_to_horizontal() {
        assert_eq!(Config::default().tab_layout, TabLayout::Horizontal);
    }

    #[test]
    fn toml_parses_tab_layout_horizontal() {
        let c: Config = toml::from_str("tab_layout = \"horizontal\"").expect("toml parses");
        assert_eq!(c.tab_layout, TabLayout::Horizontal);
    }

    #[test]
    fn toml_parses_tab_layout_vertical() {
        let c: Config = toml::from_str("tab_layout = \"vertical\"").expect("toml parses");
        assert_eq!(c.tab_layout, TabLayout::Vertical);
    }

    #[test]
    fn launcher_icons_default_empty() {
        assert!(Config::default().launcher_icons.is_empty());
    }

    #[test]
    fn launcher_icons_parse_minimal() {
        let toml_src = r#"
            [[launcher_icon]]
            id = "slack"
            glyph = ""
            command = "mnml-msg-slack"
        "#;
        let c: Config = toml::from_str(toml_src).expect("toml parses");
        assert_eq!(c.launcher_icons.len(), 1);
        let s = &c.launcher_icons[0];
        assert_eq!(s.id, "slack");
        assert_eq!(s.glyph, "");
        assert_eq!(s.command, "mnml-msg-slack");
        assert!(s.args.is_empty(), "args default to empty");
        assert_eq!(s.tooltip, "", "tooltip defaults to empty");
        assert!(s.color.is_none(), "color defaults to none");
    }

    #[test]
    fn launcher_icons_parse_with_args_color_tooltip() {
        let toml_src = r##"
            [[launcher_icon]]
            id = "mixr"
            glyph = "♪"
            command = "mixr-rs"
            args = ["--dashboard"]
            tooltip = "Mixr (mixing dashboard)"
            color = "#7aa2f7"
        "##;
        let c: Config = toml::from_str(toml_src).expect("toml parses");
        assert_eq!(c.launcher_icons.len(), 1);
        let m = &c.launcher_icons[0];
        assert_eq!(m.args, vec!["--dashboard"]);
        assert_eq!(m.tooltip, "Mixr (mixing dashboard)");
        assert_eq!(m.color.as_deref(), Some("#7aa2f7"));
    }

    #[test]
    fn launcher_icons_parse_multiple_in_order() {
        let toml_src = r#"
            [[launcher_icon]]
            id = "slack"
            glyph = ""
            command = "mnml-msg-slack"

            [[launcher_icon]]
            id = "teams"
            glyph = ""
            command = "mnml-msg-teams"
        "#;
        let c: Config = toml::from_str(toml_src).expect("toml parses");
        assert_eq!(c.launcher_icons.len(), 2);
        // Render order = TOML order — the rail paints top-to-bottom.
        assert_eq!(c.launcher_icons[0].id, "slack");
        assert_eq!(c.launcher_icons[1].id, "teams");
    }
}
