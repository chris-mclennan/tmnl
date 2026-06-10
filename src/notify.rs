//! Dock-badge and chime helpers for the attention-notification
//! pipeline. Both are macOS-only no-ops on other platforms.
//!
//! - `set_dock_badge(Some("3"))` → red "3" on the Dock icon
//!   (Mail / Messages convention). `None` clears.
//! - `play_chime()` → spawns a non-blocking `afplay`. Caller is
//!   responsible for opt-in gating via config.

#[cfg(target_os = "macos")]
pub fn set_dock_badge(label: Option<&str>) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSApp;

    // SAFETY: NSApp returns the shared NSApplication; calling
    // `dockTile` + `setBadgeLabel:` on the main thread is the
    // standard pattern. We touch only Apple-owned classes and
    // pass an NSString (the autorelease pool we hit via mtm
    // covers the temporary). All calls live on the main thread
    // since this is invoked from App::tick.
    unsafe {
        let mtm = objc2::MainThreadMarker::new_unchecked();
        let app = NSApp(mtm);
        let tile: *mut AnyObject = msg_send![&*app, dockTile];
        if tile.is_null() {
            return;
        }
        let ns_label: *mut AnyObject = match label {
            Some(text) => {
                use objc2::class;
                let ns_string_cls = class!(NSString);
                let bytes = text.as_bytes();
                let ptr = bytes.as_ptr();
                let len = bytes.len();
                msg_send![
                    ns_string_cls,
                    stringWithBytes: ptr,
                    length: len,
                    encoding: 4_u64
                ] // 4 = NSUTF8StringEncoding
            }
            None => std::ptr::null_mut(),
        };
        let _: () = msg_send![tile, setBadgeLabel: ns_label];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_badge(_label: Option<&str>) {}

/// Spawn `afplay` with the system "Pop" sound. Non-blocking; we
/// detach the child so playback continues even after this returns.
/// Failures (sound file missing, afplay missing) are swallowed —
/// the chime is opt-in cosmetics, not load-bearing.
#[cfg(target_os = "macos")]
pub fn play_chime() {
    let _ = std::process::Command::new("/usr/bin/afplay")
        .arg("/System/Library/Sounds/Pop.aiff")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn play_chime() {}
