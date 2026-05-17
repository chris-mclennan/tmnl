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
    /// padding so the prompt doesn't hug the window edge). Full-screen
    /// TUIs always render edge-to-edge — native mode (mnml / mixr) and
    /// shell mode with the xterm alt-screen active both bypass this.
    pub inset: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self { inset: 20.0 }
    }
}

impl Config {
    /// Inset to use right now. TUIs always get 0; only the shell prompt
    /// gets the configured padding.
    pub fn active_inset(&self, tui_active: bool) -> f32 {
        if tui_active { 0.0 } else { self.inset.max(0.0) }
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
        let text =
            toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&path, text)?;
        log::info!("config: saved → {}", path.display());
        Ok(())
    }
}

pub fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("tmnl").join("config.toml"))
}
