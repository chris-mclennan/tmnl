//! Recents — persistent log of native-tab launches. Powers the welcome
//! screen's quick-resume picker.
//!
//! Lives at `~/.config/tmnl/recents.toml` (respecting `$XDG_CONFIG_HOME`).
//! Append-on-open, cap at [`MAX_RECENTS`], de-dup by full tuple — so a
//! second launch of the same `(command, args, workspace)` just bumps
//! the existing entry to the top of the list (most-recent-first).
//!
//! Format:
//!
//! ```toml
//! [[entry]]
//! command   = "/usr/local/bin/mnml"
//! args      = ["--input", "vim"]
//! workspace = "/Users/me/Projects/foo"
//! label     = "mnml — foo"      # display hint; defaults to command's filename
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Cap on the number of remembered launches. New entries push older
/// ones off the bottom; the welcome screen never offers more than this
/// many anyway (only 1-9 are keyboard-selectable).
pub const MAX_RECENTS: usize = 20;

/// One row in `~/.config/tmnl/recents.toml`. `command` is the binary
/// path (absolute or `$PATH`-resolvable); `args` are the extra args
/// passed alongside (typically `--input vim` or similar — the
/// `--blit <socket>` arg is added at spawn time, not stored).
/// `workspace` is the working directory the binary is launched in,
/// or `None` to inherit tmnl's cwd. `label` is a display hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub command: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub label: Option<String>,
}

impl Entry {
    /// Human-readable single-line summary — what the welcome screen
    /// shows next to the entry number. Format:
    ///   `<binary-filename>  <workspace-shortened>`
    /// Falls back to the binary's full path when there's no workspace.
    pub fn summary(&self) -> String {
        let bin = self
            .command
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_else(|| self.command.to_str().unwrap_or("?"));
        if let Some(ws) = self.workspace.as_ref() {
            format!("{bin}  {}", shorten_path(ws))
        } else {
            bin.to_string()
        }
    }
}

/// The whole TOML file's shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "entry")]
    entries: Vec<Entry>,
}

/// Load the recents file. Missing / unreadable / malformed ⇒ empty
/// list (so a corrupted file doesn't kill the welcome screen).
pub fn load() -> Vec<Entry> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match toml::from_str::<File>(&text) {
        Ok(f) => f.entries.into_iter().take(MAX_RECENTS).collect(),
        Err(_) => Vec::new(),
    }
}

/// The built-in launch options that always appear at the bottom of
/// the welcome list, even when `~/.config/tmnl/recents.toml` is
/// empty or missing. Lets users run mnml / mixr as native tabs on
/// a fresh tmnl install without having to type a path or remember
/// a flag.
///
/// Resolution mirrors `tmnl --mnml` / `tmnl --mixr`: walk for a
/// sibling `<repo>/target/{release,debug}/<bin>`, fall back to PATH.
pub fn builtin_entries() -> Vec<Entry> {
    fn resolve(repo: &str, bin: &str) -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            let root = std::path::Path::new("/");
            let mut cur: Option<&std::path::Path> = exe.parent();
            let mut hops = 0;
            while let Some(p) = cur {
                for profile in &["release", "debug"] {
                    let candidate = p.join(repo).join("target").join(profile).join(bin);
                    if candidate.exists() {
                        return candidate;
                    }
                }
                if p == root {
                    break;
                }
                cur = p.parent();
                hops += 1;
                if hops > 10 {
                    break;
                }
            }
        }
        PathBuf::from(bin)
    }
    vec![
        Entry {
            command: resolve("mnml", "mnml"),
            args: vec!["--input".into(), "standard".into()],
            workspace: None,
            label: Some("mnml — terminal IDE".into()),
        },
        Entry {
            command: resolve("mixr-rs", "mixr"),
            args: vec!["--dashboard".into()],
            workspace: None,
            label: Some("mixr — terminal DJ".into()),
        },
    ]
}

/// Record a launch — pushes `entry` to the front of the recents list
/// (most-recent-first) and writes the file. Existing entries with
/// identical `(command, args, workspace)` are removed first so the
/// list stays deduped. Caps at [`MAX_RECENTS`].
///
/// Silently best-effort: a write failure logs a warning but doesn't
/// propagate — tmnl shouldn't fail to spawn a tab because the
/// recents file is read-only.
pub fn record(entry: Entry) {
    let mut entries = load();
    entries.retain(|e| {
        !(e.command == entry.command && e.args == entry.args && e.workspace == entry.workspace)
    });
    entries.insert(0, entry);
    entries.truncate(MAX_RECENTS);

    let Some(path) = path() else { return };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        log::warn!("tmnl: recents: couldn't mkdir {}", parent.display());
        return;
    }
    let file = File { entries };
    match toml::to_string_pretty(&file) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                log::warn!("tmnl: recents: write {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("tmnl: recents: serialize: {e}"),
    }
}

/// Wipe every entry from the recents file. Used by the welcome
/// overlay's "clear all" action. No-op when the file doesn't
/// exist (the next `record` will create it fresh).
pub fn clear_all() {
    let Some(path) = path() else { return };
    if !path.exists() {
        return;
    }
    let file = File {
        entries: Vec::new(),
    };
    match toml::to_string_pretty(&file) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                log::warn!("tmnl: recents: clear_all write {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("tmnl: recents: clear_all serialize: {e}"),
    }
}

/// `$XDG_CONFIG_HOME/tmnl/recents.toml` or
/// `$HOME/.config/tmnl/recents.toml`.
fn path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("tmnl").join("recents.toml"));
    }
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("tmnl")
            .join("recents.toml")
    })
}

/// Display-friendly path: replaces a `$HOME` prefix with `~`; truncates
/// long paths with `…/` in the middle so they fit in a column.
fn shorten_path(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let home = std::env::var("HOME").unwrap_or_default();
    let s = if !home.is_empty() && s.starts_with(&home) {
        format!("~{}", &s[home.len()..])
    } else {
        s
    };
    const MAX: usize = 50;
    if s.chars().count() <= MAX {
        return s;
    }
    // Keep the leading "~/" and the tail; elide the middle.
    let chars: Vec<char> = s.chars().collect();
    let tail_n = MAX.saturating_sub(8);
    let tail: String = chars[chars.len().saturating_sub(tail_n)..].iter().collect();
    format!("~/…/{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_summary_with_workspace() {
        let e = Entry {
            command: PathBuf::from("/usr/local/bin/mnml"),
            args: vec![],
            workspace: Some(PathBuf::from("/Users/me/Projects/foo")),
            label: None,
        };
        let s = e.summary();
        assert!(s.starts_with("mnml"), "got: {s}");
    }

    #[test]
    fn entry_summary_without_workspace() {
        let e = Entry {
            command: PathBuf::from("mixr"),
            args: vec![],
            workspace: None,
            label: None,
        };
        assert_eq!(e.summary(), "mixr");
    }
}
