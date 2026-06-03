//! Background "is there a newer release?" check. Same shape as
//! mnml + mixr's update_check (see those for full design notes).
//! Uses ureq instead of reqwest because tmnl has no async runtime
//! and a single blocking GET on a background thread is the right
//! tool.
//!
//! Result surfaces in two places:
//!   - Stderr at startup (visible when launched from a terminal /
//!     via `tmnl-launcher` — the .app launcher logs to its own
//!     log file).
//!   - `WelcomeState.update_notice` if the user lands on the
//!     welcome overlay (bare-tmnl launch, no `--mnml`/`--mixr`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const REPO: &str = "chris-mclennan/tmnl";
const USER_AGENT: &str = "tmnl-update-check (https://github.com/chris-mclennan/tmnl)";

#[allow(dead_code)] // `announced` + the read APIs are wired in v2
// (welcome banner integration). Kept now so the
// shape matches mnml + mixr's update_check
// exactly — easier to sync the three when one
// changes.
pub struct UpdateCheck {
    pub latest_version: Mutex<Option<String>>,
    pub announced: AtomicBool,
}

impl UpdateCheck {
    pub fn spawn() -> Arc<Self> {
        let handle = Arc::new(Self {
            latest_version: Mutex::new(None),
            announced: AtomicBool::new(false),
        });
        let bg = Arc::clone(&handle);
        std::thread::spawn(move || {
            if let Some(latest) = fetch_latest_tag() {
                let current = env!("CARGO_PKG_VERSION");
                if latest != current
                    && let Ok(mut slot) = bg.latest_version.lock()
                {
                    *slot = Some(latest.clone());
                    eprintln!("tmnl: v{latest} available — {}", Self::release_url(&latest));
                }
            }
        });
        handle
    }

    /// Read-only access for the welcome overlay banner.
    #[allow(dead_code)]
    pub fn latest(&self) -> Option<String> {
        self.latest_version.lock().ok()?.clone()
    }

    /// One-shot variant for surfaces that want to fire-and-forget
    /// (matches the mnml/mixr toast pattern).
    #[allow(dead_code)]
    pub fn take_pending_announcement(&self) -> Option<String> {
        if self.announced.load(Ordering::Relaxed) {
            return None;
        }
        let latest = self.latest_version.lock().ok()?.clone()?;
        self.announced.store(true, Ordering::Relaxed);
        Some(latest)
    }

    pub fn release_url(latest: &str) -> String {
        format!("https://github.com/{REPO}/releases/tag/v{latest}")
    }
}

fn fetch_latest_tag() -> Option<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .set("User-Agent", USER_AGENT)
        .call()
        .ok()?;
    if !(200..300).contains(&resp.status()) {
        return None;
    }
    let body = resp.into_string().ok()?;
    // Tiny ad-hoc parser — avoid adding serde_json just to read one
    // key. GitHub's response is well-formed; this picks the first
    // `"tag_name":"…"` occurrence which is at the top level of the
    // /releases/latest payload.
    let needle = "\"tag_name\":";
    let start = body.find(needle)? + needle.len();
    let after = &body[start..].trim_start();
    let after = after.strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].trim_start_matches('v').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_url_format() {
        assert_eq!(
            UpdateCheck::release_url("0.0.5"),
            format!("https://github.com/{REPO}/releases/tag/v0.0.5")
        );
    }
}
