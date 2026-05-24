//! Themed powerline prompt — sibling of `mnml`'s `src/shell_prompt.rs`,
//! both write the same `prompt.sh` to `~/.config/mnml/prompt.sh`
//! (idempotent), and tmnl exports `MNML_PROMPT_SCRIPT` + `MNML_CONTEXT`
//! so the user's `.zshrc` opt-in line picks it up. Theming env vars are
//! intentionally not set here — the script's built-in defaults
//! (tokyo-night-ish) render against any background. mnml — which knows
//! its active theme — sets the full palette env-var set.
//!
//! Script source-of-truth lives in `mnml/themes/mnml-prompt.sh`. We
//! ship a verbatim copy here so tmnl doesn't have to take a path-dep
//! on mnml; both copies must stay in sync. Update both when the
//! script changes.

use std::io;
use std::path::PathBuf;

const SCRIPT: &str = include_str!("../themes/mnml-prompt.sh");

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

/// Returns `[(MNML_PROMPT_SCRIPT, path), (MNML_CONTEXT, "tmnl")]` —
/// the minimum env-var set to enable the themed prompt from tmnl's
/// side. Colour env vars are deliberately omitted; the script's
/// built-in defaults take over.
pub fn env_vars() -> Vec<(String, String)> {
    let mut v = vec![("MNML_CONTEXT".into(), "tmnl".into())];
    if let Ok(path) = install_prompt_script() {
        v.push(("MNML_PROMPT_SCRIPT".into(), path.display().to_string()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_includes_context_and_script() {
        // Constrain to a temp HOME so we don't litter the real
        // ~/.config/mnml on test runs.
        let d = tempfile::tempdir().unwrap();
        // SAFETY: tests serialize env via this module's only writer.
        unsafe {
            std::env::set_var("HOME", d.path());
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        let v = env_vars();
        let keys: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"MNML_CONTEXT"));
        assert!(keys.contains(&"MNML_PROMPT_SCRIPT"));
        let ctx = v.iter().find(|(k, _)| k == "MNML_CONTEXT").unwrap();
        assert_eq!(ctx.1, "tmnl");
        // Script written + readable.
        let script = v.iter().find(|(k, _)| k == "MNML_PROMPT_SCRIPT").unwrap();
        assert!(std::path::Path::new(&script.1).exists());
    }
}
