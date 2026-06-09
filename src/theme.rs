//! Chrome palette — colors used by tmnl's frame (top pad, strip,
//! tab chips, palette cluster). At startup tmnl tries to adopt
//! mnml's installed theme (so the two apps blend visually when
//! launched side-by-side); falls back to a curated default
//! eyedropped from mnml's `onedark` if no mnml config is present.
//!
//! Why eyedropped defaults instead of mnml's literal theme hex?
//! Terminal apps apply a small color transform between the source
//! hex (`#1b1f27`) and what reaches the screen (`rgb(26, 29, 34)`).
//! For users who DON'T have mnml installed, we want tmnl to look
//! the way mnml looks in their terminal — so we ship the rendered
//! bytes, not the source hex. For users who DO have mnml, we read
//! their selected theme's hex directly (no transform applies — tmnl
//! and mnml use the same display) and adopt those values verbatim.
//!
//! Theme adoption is best-effort: any parse / IO error falls back
//! to defaults silently (with a log line — not a crash).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::SystemTime;

/// All chrome colors tmnl renders. Each field is a straight-sRGB
/// `[r, g, b, a]` quad, alpha always 1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Top-of-window letterbox + sub-cell overflow. Same as `strip_bg`
    /// so the strip blends into the chrome.
    pub clear_bg: [f32; 4],
    /// Tab-strip background — the chrome row with traffic-light
    /// buttons + tab chips. Matches mnml's bufferline color.
    pub strip_bg: [f32; 4],
    /// Arrow button pill (back / forward). Slightly lifted off the
    /// strip — first tier of the 3-tier gradient inside the cluster.
    pub btn_bg: [f32; 4],
    /// Active tab pill. Second tier of the gradient — lifted above
    /// the strip but below the search chip.
    pub active_chip_bg: [f32; 4],
    /// Search-chip body. Third (highest) tier — primary affordance.
    pub chip_bg: [f32; 4],
    /// Bright text — active tab labels, palette title.
    pub text_fg: [f32; 4],
    /// Tab text — brighter than `dim_fg` (tabs are nav), used for
    /// inactive chip labels. Active tabs use `text_fg + bold`.
    pub tab_fg: [f32; 4],
    /// Dim text — URL strings, "waiting for client" hints, etc.
    /// Should stay dim even when a bright theme is loaded.
    pub dim_fg: [f32; 4],
    /// Accent color — keymap hints, highlight glyphs.
    pub accent_fg: [f32; 4],
}

impl Palette {
    /// Defaults — eyedropped from mnml's `onedark` rendered in
    /// Apple Terminal. Use when no mnml config is present.
    pub const fn defaults() -> Self {
        Self {
            clear_bg: [0.1020, 0.1137, 0.1333, 1.0],
            strip_bg: [0.1020, 0.1137, 0.1333, 1.0],
            btn_bg: [0.1176, 0.1333, 0.1569, 1.0],
            active_chip_bg: [0.1412, 0.1529, 0.1765, 1.0],
            chip_bg: [0.1608, 0.1765, 0.2078, 1.0],
            text_fg: [0.86, 0.87, 0.92, 1.0],
            tab_fg: [0.624, 0.655, 0.706, 1.0],
            dim_fg: [0.48, 0.50, 0.58, 1.0],
            accent_fg: [0.93, 0.73, 0.45, 1.0],
        }
    }

    /// Try to adopt mnml's currently-selected theme. Reads
    /// `~/.config/mnml/config.toml` for the theme name, then the
    /// corresponding `themes/<name>.toml`. Returns `None` if mnml
    /// isn't installed, the config can't be parsed, the named
    /// theme can't be found, or any required field is missing.
    pub fn from_mnml() -> Option<Self> {
        let theme_name = read_mnml_theme_name()?;
        let theme = read_mnml_theme_file(&theme_name)?;
        Some(map_mnml_to_palette(&theme))
    }
}

/// Read `ui.theme` from `~/.config/mnml/config.toml`. Returns
/// `"onedark"` (mnml's own default) if the file exists but doesn't
/// specify a theme — that way users on the stock config still get
/// theme adoption.
fn read_mnml_theme_name() -> Option<String> {
    let cfg_path = mnml_config_path()?;
    let text = std::fs::read_to_string(&cfg_path).ok()?;
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    let name = parsed
        .get("ui")
        .and_then(|ui| ui.get("theme"))
        .and_then(|v| v.as_str())
        .unwrap_or("onedark");
    Some(name.to_string())
}

/// Parse the `[base_30]` block of an mnml theme file into the
/// subset of fields tmnl needs.
#[derive(Debug, Clone, serde::Deserialize)]
struct MnmlBase30 {
    /// Bufferline / tree rail / overlay bg.
    darker_black: String,
    /// Editor body — used as arrow button pill bg in tmnl.
    black: String,
    /// Slightly lifted variant of `black` — active tab pill.
    #[serde(default)]
    black2: Option<String>,
    /// Selected pane / one-bg — search chip body.
    one_bg: String,
    /// Bright fg for active tabs / primary text.
    white: String,
    /// Mid-brightness fg for tab labels.
    grey_fg: String,
    /// Dim fg for hints, URLs.
    #[serde(alias = "grey_fg2", alias = "light_grey")]
    grey_fg2: Option<String>,
    /// Accent (orange in mnml's defaults).
    #[serde(alias = "yellow", alias = "orange")]
    yellow: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct MnmlTheme {
    base_30: MnmlBase30,
}

/// Locate and parse `themes/<name>.toml` from mnml's install. Tries
/// the user's installed location first (`~/.config/mnml/themes/`),
/// then the dev source tree (`~/Projects/mnml/themes/`) as a
/// fallback for contributors working from a clone.
fn read_mnml_theme_file(name: &str) -> Option<MnmlTheme> {
    let candidates = [
        dirs::config_dir()?
            .join("mnml")
            .join("themes")
            .join(format!("{name}.toml")),
        dirs::data_dir()?
            .join("mnml")
            .join("themes")
            .join(format!("{name}.toml")),
        PathBuf::from(std::env::var("HOME").ok()?)
            .join("Projects")
            .join("mnml")
            .join("themes")
            .join(format!("{name}.toml")),
    ];
    for path in candidates.iter() {
        if let Ok(text) = std::fs::read_to_string(path) {
            match toml::from_str::<MnmlTheme>(&text) {
                Ok(t) => return Some(t),
                Err(e) => log::warn!("theme: parse {} failed: {e}", path.display()),
            }
        }
    }
    None
}

/// Project mnml's base-30 field names onto tmnl's chrome roles.
/// Falls back per-field to `Palette::defaults()` for any missing
/// value (e.g. older theme files without `black2`).
fn map_mnml_to_palette(t: &MnmlTheme) -> Palette {
    let d = Palette::defaults();
    Palette {
        clear_bg: hex(&t.base_30.darker_black).unwrap_or(d.clear_bg),
        strip_bg: hex(&t.base_30.darker_black).unwrap_or(d.strip_bg),
        btn_bg: hex(&t.base_30.black).unwrap_or(d.btn_bg),
        active_chip_bg: t
            .base_30
            .black2
            .as_deref()
            .and_then(hex)
            .unwrap_or(d.active_chip_bg),
        chip_bg: hex(&t.base_30.one_bg).unwrap_or(d.chip_bg),
        text_fg: hex(&t.base_30.white).unwrap_or(d.text_fg),
        tab_fg: hex(&t.base_30.grey_fg).unwrap_or(d.tab_fg),
        dim_fg: t
            .base_30
            .grey_fg2
            .as_deref()
            .and_then(hex)
            .unwrap_or(d.dim_fg),
        accent_fg: t
            .base_30
            .yellow
            .as_deref()
            .and_then(hex)
            .unwrap_or(d.accent_fg),
    }
}

/// Parse a `#rrggbb` (or `rrggbb`) hex string into a normalized
/// straight-sRGB `[r, g, b, 1.0]` quad. Returns `None` for any
/// malformed input. Re-exported as `parse_hex_rgba` for callers
/// outside this module.
pub fn parse_hex_rgba(s: &str) -> Option<[f32; 4]> {
    hex(s)
}

fn hex(s: &str) -> Option<[f32; 4]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    ])
}

/// Global chrome palette. `RwLock` so live theme reload (when mnml's
/// config file mtime changes) can swap in a new palette without
/// restarting tmnl. Reads happen on every render frame — they hold
/// the read lock just long enough to copy out the 160-byte struct;
/// writes (theme reload) are rare so contention is negligible.
static PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();

/// Mtime of mnml's config file at the most recent `poll_mnml_config`
/// — refresh fires only when this changes, so the once-per-tick
/// `stat()` is the only cost in the steady state.
static LAST_MNML_CONFIG_MTIME: Mutex<Option<SystemTime>> = Mutex::new(None);

fn storage() -> &'static RwLock<Palette> {
    PALETTE.get_or_init(|| RwLock::new(Palette::defaults()))
}

/// Initialize the global palette from mnml (best-effort) or
/// defaults. Idempotent — first call wins; later calls are reloads
/// (use [`refresh`] explicitly for clarity in that case).
/// Call once at startup before any render path uses `palette()`.
pub fn init() {
    let chosen = match Palette::from_mnml() {
        Some(p) => {
            log::info!("theme: adopted from mnml");
            p
        }
        None => {
            log::debug!("theme: using tmnl defaults (no mnml config found)");
            Palette::defaults()
        }
    };
    let lk = storage();
    if let Ok(mut g) = lk.write() {
        *g = chosen;
    }
    // Prime the mtime tracker so the first `poll_mnml_config` doesn't
    // fire a redundant refresh on the very next tick.
    if let Some(path) = mnml_config_path()
        && let Ok(meta) = std::fs::metadata(&path)
        && let Ok(mtime) = meta.modified()
        && let Ok(mut last) = LAST_MNML_CONFIG_MTIME.lock()
    {
        *last = Some(mtime);
    }
}

/// A snapshot of the active chrome palette. Returns defaults if
/// `init()` was never called (defensive — render paths shouldn't
/// crash if startup ordering changes).
///
/// Named `palette()` rather than `chrome()` to avoid colliding with
/// the `chrome` field on the Browser pane variant and the `chrome`
/// param on `paint_browser_chrome`.
///
/// **Returns by value (Copy)** — for tight inner loops, hoist into
/// a `let` binding to avoid re-acquiring the read lock per glyph.
pub fn palette() -> Palette {
    match storage().read() {
        Ok(g) => *g,
        // RwLock can only be poisoned by a panic in a writer. In
        // that case fall back to defaults so render paths still get
        // a sane palette.
        Err(_) => Palette::defaults(),
    }
}

/// Re-read mnml's selected theme and swap it into the global palette.
/// Returns `true` iff the new palette differs from the old (caller
/// requests a redraw on `true`). Called by [`poll_mnml_config`] and
/// by the `theme.refresh` palette command for manual refresh.
pub fn refresh() -> bool {
    let new = Palette::from_mnml().unwrap_or_else(Palette::defaults);
    let lk = storage();
    let mut guard = match lk.write() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if *guard == new {
        return false;
    }
    *guard = new;
    log::info!("theme: live-reloaded from mnml");
    true
}

/// One-stat-per-tick check that mnml's config file hasn't changed.
/// Cheap enough to call once per frame; only triggers `refresh` (and
/// hence the heavier TOML read) when the mtime actually moves.
/// Returns `true` iff the palette changed (caller requests a redraw).
pub fn poll_mnml_config() -> bool {
    let Some(path) = mnml_config_path() else {
        return false;
    };
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut last = match LAST_MNML_CONFIG_MTIME.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if *last == mtime {
        return false;
    }
    *last = mtime;
    // Mtime changed (or file appeared/disappeared) — try a refresh.
    // Drop the mutex guard first so refresh() doesn't double-lock.
    drop(last);
    refresh()
}

fn mnml_config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("mnml").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_eyedropped_values() {
        let d = Palette::defaults();
        assert_eq!(d.strip_bg, [0.1020, 0.1137, 0.1333, 1.0]);
        assert_eq!(d.btn_bg, [0.1176, 0.1333, 0.1569, 1.0]);
        assert_eq!(d.active_chip_bg, [0.1412, 0.1529, 0.1765, 1.0]);
        assert_eq!(d.chip_bg, [0.1608, 0.1765, 0.2078, 1.0]);
    }

    #[test]
    fn hex_parses_with_and_without_hash() {
        assert_eq!(
            hex("#1b1f27"),
            Some([27.0 / 255.0, 31.0 / 255.0, 39.0 / 255.0, 1.0])
        );
        assert_eq!(
            hex("1b1f27"),
            Some([27.0 / 255.0, 31.0 / 255.0, 39.0 / 255.0, 1.0])
        );
    }

    #[test]
    fn hex_rejects_malformed() {
        assert_eq!(hex("xyz"), None);
        assert_eq!(hex("#12345"), None); // 5 chars
        assert_eq!(hex("#1234567"), None); // 7 chars
        assert_eq!(hex("#gggggg"), None); // non-hex
    }

    #[test]
    fn map_mnml_uses_defaults_for_missing_optional_fields() {
        let theme = MnmlTheme {
            base_30: MnmlBase30 {
                darker_black: "#000000".into(),
                black: "#111111".into(),
                black2: None, // ← missing
                one_bg: "#222222".into(),
                white: "#ffffff".into(),
                grey_fg: "#888888".into(),
                grey_fg2: None,
                yellow: None,
            },
        };
        let p = map_mnml_to_palette(&theme);
        // Required fields override defaults.
        assert_eq!(p.strip_bg, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(p.btn_bg[0], 17.0 / 255.0);
        // Missing optional fields fall back to defaults.
        let d = Palette::defaults();
        assert_eq!(p.active_chip_bg, d.active_chip_bg);
        assert_eq!(p.dim_fg, d.dim_fg);
        assert_eq!(p.accent_fg, d.accent_fg);
    }

    #[test]
    fn palette_returns_defaults_before_init() {
        // Don't call init() — accessor should still work.
        let p = palette();
        // Note: in test runs another test may have called init();
        // we only assert the call doesn't panic + returns *some*
        // palette. Exact values vary across the test binary.
        assert_eq!(p.strip_bg[3], 1.0);
    }
}
