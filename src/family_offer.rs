//! First-launch "did you know about the rest of the family?" check.
//! Same shape as mnml + mixr's `family_offer` — keep them close,
//! sync by hand when one changes. See mnml/src/family_offer.rs for
//! the full design notes.
//!
//! v1 surfaces via stderr only (matches the family_offer-less
//! welcome.rs surface today). A v2 will pipe the hints into
//! `WelcomeState` so they show on the GPU welcome overlay.

use std::path::PathBuf;

const FAMILY: &[&str] = &["mnml", "mixr", "tmnl"];
const SELF: &str = "tmnl";

pub fn maybe_offer_at_launch() {
    if marker_path().exists() {
        return;
    }
    let missing: Vec<&'static str> = FAMILY
        .iter()
        .copied()
        .filter(|name| *name != SELF && !is_installed(name))
        .collect();
    if missing.is_empty() {
        return;
    }
    for app in &missing {
        eprintln!("tmnl: try {app} too — {}", hint_for(app));
    }
    mark_shown();
}

fn hint_for(app: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        format!("brew install chris-mclennan/tap/{app}  ·  https://{app}.sh")
    }
    #[cfg(all(target_os = "linux", not(target_os = "macos")))]
    {
        format!("brew install chris-mclennan/tap/{app}  ·  apt/dnf/AppImage at https://{app}.sh")
    }
    #[cfg(target_os = "windows")]
    {
        format!("winget install chris-mclennan.{app}  ·  https://{app}.sh")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        format!("https://{app}.sh")
    }
}

fn is_installed(app: &str) -> bool {
    if path_lookup(app) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        let p = format!("/Applications/{app}.app");
        if std::path::Path::new(&p).exists() {
            return true;
        }
    }
    false
}

fn path_lookup(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for entry in std::env::split_paths(&path) {
        let candidate = entry.join(name);
        if candidate.is_file() {
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            for ext in &[".exe", ".cmd", ".bat"] {
                let mut p = candidate.clone();
                let stem = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                p.set_file_name(format!("{stem}{ext}"));
                if p.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

fn mark_shown() {
    let path = marker_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, b"shown\n");
}

fn marker_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config")
        .join("tmnl")
        .join(".family-offer-shown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_contains_self() {
        assert!(FAMILY.contains(&SELF));
    }

    #[test]
    fn hint_for_includes_app_name() {
        assert!(hint_for("mnml").contains("mnml"));
    }

    #[test]
    fn path_lookup_finds_common_binary() {
        assert!(path_lookup("ls") || path_lookup("ls.exe"));
    }
}
