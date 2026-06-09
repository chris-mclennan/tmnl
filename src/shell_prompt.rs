//! Themed powerline prompt — sibling of `mnml`'s `src/shell_prompt.rs`,
//! both write the same `prompt.sh` to `~/.config/mnml/prompt.sh`
//! (idempotent), and tmnl exports `MNML_PROMPT_SCRIPT` + `MNML_CONTEXT`
//! so the user's `.zshrc` opt-in line picks it up. When the user has
//! `themed_prompt = true` in their tmnl config, we ALSO export the
//! active palette (`MNML_PROMPT_BG`, `_FG`, `_ACCENT`, `_GREY`) so the
//! prompt's chrome colour-matches whatever theme tmnl is wearing.
//!
//! Script source-of-truth lives in `mnml/themes/mnml-prompt.sh`. We
//! ship a verbatim copy here so tmnl doesn't have to take a path-dep
//! on mnml; both copies must stay in sync. Update both when the
//! script changes.

use std::io;
use std::path::PathBuf;

const SCRIPT: &str = include_str!("../themes/mnml-prompt.sh");

/// The single line we ask the user to add to their rc file. We
/// also use this exact substring (`"MNML_PROMPT_SCRIPT"`) to detect
/// whether the line is already there so we don't double-append it.
const RC_STANZA: &str = "\n# tmnl / mnml themed prompt (auto-added by tmnl)\n[ -n \"$MNML_PROMPT_SCRIPT\" ] && source \"$MNML_PROMPT_SCRIPT\"\n";

pub fn script_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("mnml").join("prompt.sh")
}

pub fn install_prompt_script() -> io::Result<PathBuf> {
    let path = script_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let needs_write = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != SCRIPT,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&path, SCRIPT)?;
    }
    Ok(path)
}

/// Returns the env-var set tmnl exports to every spawned shell.
///
/// * `themed = false` — only `MNML_CONTEXT=tmnl`. The user's rc-file
///   source line (if any) becomes a no-op because
///   `MNML_PROMPT_SCRIPT` is unset, so they get their normal prompt.
/// * `themed = true` — adds `MNML_PROMPT_SCRIPT` (path to the
///   installed `prompt.sh`) + the four palette vars the script
///   consumes (`MNML_PROMPT_BG`, `_FG`, `_ACCENT`, `_GREY`).
///   `_GREEN`/`_RED`/`_YELLOW` are left to the script's defaults
///   (tmnl's chrome palette doesn't carry those right now — a
///   later pass can lift them from mnml's full theme file).
pub fn env_vars(themed: bool) -> Vec<(String, String)> {
    let mut v = vec![("MNML_CONTEXT".into(), "tmnl".into())];
    if !themed {
        return v;
    }
    if let Ok(path) = install_prompt_script() {
        v.push(("MNML_PROMPT_SCRIPT".into(), path.display().to_string()));
    }
    let p = crate::theme::palette();
    v.push(("MNML_PROMPT_BG".into(), rgba_to_hex(p.clear_bg)));
    v.push(("MNML_PROMPT_FG".into(), rgba_to_hex(p.text_fg)));
    v.push(("MNML_PROMPT_ACCENT".into(), rgba_to_hex(p.accent_fg)));
    v.push(("MNML_PROMPT_GREY".into(), rgba_to_hex(p.dim_fg)));
    // Chip bg — matches the bufferline / statusline chip color so
    // the prompt's cwd-chip reads as part of the same family.
    v.push(("MNML_PROMPT_CHIP_BG".into(), rgba_to_hex(p.chip_bg)));
    v
}

/// `[r,g,b,a]` (0..1 floats) → `"#rrggbb"`. Alpha is dropped — the
/// prompt script only consumes opaque chrome colours.
fn rgba_to_hex([r, g, b, _]: [f32; 4]) -> String {
    let clamp = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", clamp(r), clamp(g), clamp(b))
}

/// Ensure the user's rc file(s) source the prompt script. Called on
/// the first save where `themed_prompt` transitions to `true`.
///
/// Touches every rc file that already exists in `$HOME` — `.zshrc`
/// AND `.bashrc` — so users who shell-hop see the prompt in both.
/// Skips files that already mention `MNML_PROMPT_SCRIPT` so re-runs
/// are idempotent. Never creates an rc file that doesn't already
/// exist; that would assume the user's shell.
///
/// Returns the list of files we actually appended to (empty when
/// every rc was already wired or no rc file exists).
pub fn ensure_rc_sourced() -> io::Result<Vec<PathBuf>> {
    let Some(home_os) = std::env::var_os("HOME") else {
        return Ok(Vec::new());
    };
    let home = PathBuf::from(home_os);
    let mut touched = Vec::new();
    for rc in [".zshrc", ".bashrc"] {
        let path = home.join(rc);
        if !path.exists() {
            continue;
        }
        let existing = std::fs::read_to_string(&path)?;
        if existing.contains("MNML_PROMPT_SCRIPT") {
            continue;
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path)?;
        f.write_all(RC_STANZA.as_bytes())?;
        touched.push(path);
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `HOME` / `XDG_CONFIG_HOME` mutation isn't process-isolated, so
    /// every test that calls `isolate_home` takes this lock first to
    /// serialize them against each other. Without this, parallel
    /// test runs race the env writes against the `script_path` /
    /// `ensure_rc_sourced` reads and produce flaky failures.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn isolate_home() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        // SAFETY: ENV_LOCK serializes all callers in this module.
        unsafe {
            std::env::set_var("HOME", d.path());
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        (d, guard)
    }

    #[test]
    fn env_vars_off_only_exports_context() {
        let _guard = isolate_home();
        let v = env_vars(false);
        let keys: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["MNML_CONTEXT"]);
        assert_eq!(v[0].1, "tmnl");
    }

    #[test]
    fn env_vars_on_exports_script_and_palette() {
        let _guard = isolate_home();
        let v = env_vars(true);
        let keys: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"MNML_CONTEXT"));
        assert!(keys.contains(&"MNML_PROMPT_SCRIPT"));
        assert!(keys.contains(&"MNML_PROMPT_BG"));
        assert!(keys.contains(&"MNML_PROMPT_FG"));
        assert!(keys.contains(&"MNML_PROMPT_ACCENT"));
        assert!(keys.contains(&"MNML_PROMPT_GREY"));
        let script = v.iter().find(|(k, _)| k == "MNML_PROMPT_SCRIPT").unwrap();
        assert!(std::path::Path::new(&script.1).exists());
    }

    #[test]
    fn rgba_to_hex_clamps_and_rounds() {
        assert_eq!(rgba_to_hex([0.0, 0.0, 0.0, 1.0]), "#000000");
        assert_eq!(rgba_to_hex([1.0, 1.0, 1.0, 1.0]), "#ffffff");
        assert_eq!(rgba_to_hex([0.5, 0.5, 0.5, 1.0]), "#808080");
        // Out-of-range values clamp rather than wrapping.
        assert_eq!(rgba_to_hex([-0.1, 0.0, 0.0, 1.0]), "#000000");
        assert_eq!(rgba_to_hex([1.5, 0.0, 0.0, 1.0]), "#ff0000");
    }

    #[test]
    fn ensure_rc_sourced_appends_when_missing() {
        let (d, _guard) = isolate_home();
        let rc = d.path().join(".zshrc");
        std::fs::write(&rc, "# my rc\nexport FOO=bar\n").unwrap();
        let touched = ensure_rc_sourced().unwrap();
        assert_eq!(touched, vec![rc.clone()]);
        let after = std::fs::read_to_string(&rc).unwrap();
        assert!(after.contains("MNML_PROMPT_SCRIPT"));
        // Original content preserved.
        assert!(after.contains("export FOO=bar"));
    }

    #[test]
    fn ensure_rc_sourced_idempotent() {
        let (d, _guard) = isolate_home();
        let rc = d.path().join(".zshrc");
        std::fs::write(&rc, "# my rc\n").unwrap();
        let first = ensure_rc_sourced().unwrap();
        assert_eq!(first.len(), 1);
        let second = ensure_rc_sourced().unwrap();
        assert!(second.is_empty(), "second run should not re-append");
    }

    #[test]
    fn ensure_rc_sourced_skips_missing_files() {
        let (d, _guard) = isolate_home();
        // Neither `.zshrc` nor `.bashrc` exists — should not crash,
        // should not create them.
        let touched = ensure_rc_sourced().unwrap();
        assert!(touched.is_empty());
        assert!(!d.path().join(".zshrc").exists());
        assert!(!d.path().join(".bashrc").exists());
    }
}
