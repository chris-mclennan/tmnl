//! Key-spec parsing + the keymap resolver for tmnl.
//!
//! Adapted from mixr/mnml's `keymap.rs` for `winit::KeyEvent` instead
//! of crossterm. winit hands us `Key::Named(NamedKey)` /
//! `Key::Character(SmolStr)` separately from the `ModifiersState`,
//! so the chord shape is slightly different.

#![allow(dead_code)]

use std::collections::HashMap;

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// A normalized chord — winit's logical key + modifier set, with
/// character keys lowered to a stable form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: ChordKey,
    pub mods: ChordMods,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChordKey {
    /// Single character (already case-folded; SHIFT lives in `mods`).
    Char(char),
    /// Named key (Tab/Esc/Enter/arrow/F-keys/etc.).
    Named(NamedKey),
}

/// Same shape as `winit::keyboard::ModifiersState` but `Hash`-able
/// and projection-stable across winit's internal state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ChordMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub sup: bool, // "super" / Cmd on macOS, Win key elsewhere
}

impl ChordMods {
    pub fn from(m: ModifiersState) -> Self {
        ChordMods {
            ctrl: m.control_key(),
            alt: m.alt_key(),
            shift: m.shift_key(),
            sup: m.super_key(),
        }
    }
}

impl Chord {
    pub fn of(ke: &KeyEvent, mods: ModifiersState) -> Option<Chord> {
        let key = match &ke.logical_key {
            Key::Character(s) => {
                let c = s.chars().next()?;
                ChordKey::Char(c.to_ascii_lowercase())
            }
            Key::Named(nk) => ChordKey::Named(*nk),
            _ => return None,
        };
        let mut m = ChordMods::from(mods);
        // If the character is uppercase, SHIFT is implicit — make it
        // explicit so `"a"` typed with shift collapses with `"shift+a"`.
        if let ChordKey::Char(c) = &key
            && let Key::Character(s) = &ke.logical_key
            && s.chars()
                .next()
                .is_some_and(|orig| orig.is_ascii_uppercase())
        {
            m.shift = true;
            let _ = c;
        }
        Some(Chord { key, mods: m })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Keymap {
    map: HashMap<Chord, Vec<String>>,
}

impl Keymap {
    pub fn build() -> Keymap {
        let mut km = Keymap::default();
        for cmd in crate::command::registry().all() {
            for spec in cmd.keys {
                if let Some(chord) = parse_key_spec(spec) {
                    km.map.entry(chord).or_default().push(cmd.id.to_string());
                }
            }
        }
        km
    }

    /// All command ids bound to `(ke, mods)`. Empty when nothing matches.
    pub fn resolve_all(&self, ke: &KeyEvent, mods: ModifiersState) -> &[String] {
        if let Some(chord) = Chord::of(ke, mods) {
            self.map.get(&chord).map(Vec::as_slice).unwrap_or(&[])
        } else {
            &[]
        }
    }

    pub fn resolve(&self, ke: &KeyEvent, mods: ModifiersState) -> Option<&str> {
        self.resolve_all(ke, mods).first().map(String::as_str)
    }

    /// Headless-friendly: resolve a chord directly from a logical
    /// `Key` + `ModifiersState`, skipping the `KeyEvent` envelope.
    /// winit's `KeyEvent` has private platform-specific fields that
    /// can't be constructed outside the crate, so synthetic key
    /// dispatch (used by the `tmnl --headless --app` driver) routes
    /// here instead of through `resolve_all`.
    pub fn resolve_all_chord(&self, key: &Key, mods: ModifiersState) -> &[String] {
        let chord_key = match key {
            Key::Character(s) => {
                let Some(c) = s.chars().next() else {
                    return &[];
                };
                ChordKey::Char(c.to_ascii_lowercase())
            }
            Key::Named(nk) => ChordKey::Named(*nk),
            _ => return &[],
        };
        let mut m = ChordMods::from(mods);
        if let ChordKey::Char(_) = &chord_key
            && let Key::Character(s) = key
            && s.chars()
                .next()
                .is_some_and(|orig| orig.is_ascii_uppercase())
        {
            m.shift = true;
        }
        let chord = Chord {
            key: chord_key,
            mods: m,
        };
        self.map.get(&chord).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn binding_count(&self) -> usize {
        self.map.values().map(Vec::len).sum()
    }
}

/// Parse a key spec. Modifiers (`cmd+`, `ctrl+`, `alt+`, `shift+`)
/// may prefix in any order; the final token is a named key or a
/// single character. `cmd` is `super_key()` (macOS Cmd / Windows
/// Win). Returns `None` for anything unrecognized.
pub fn parse_key_spec(spec: &str) -> Option<Chord> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let mut mods = ChordMods::default();
    let mut rest = spec;
    loop {
        let lower = rest.to_ascii_lowercase();
        if let Some(r) = lower
            .strip_prefix("cmd+")
            .or_else(|| lower.strip_prefix("super+"))
        {
            mods.sup = true;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower.strip_prefix("ctrl+") {
            mods.ctrl = true;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower
            .strip_prefix("alt+")
            .or_else(|| lower.strip_prefix("meta+"))
        {
            mods.alt = true;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower.strip_prefix("shift+") {
            mods.shift = true;
            rest = &rest[rest.len() - r.len()..];
        } else {
            break;
        }
    }
    let key = key_token(rest)?;
    Some(Chord { key, mods })
}

fn key_token(token: &str) -> Option<ChordKey> {
    let t = token.to_ascii_lowercase();
    Some(match t.as_str() {
        "enter" | "return" | "cr" => ChordKey::Named(NamedKey::Enter),
        "tab" => ChordKey::Named(NamedKey::Tab),
        "esc" | "escape" => ChordKey::Named(NamedKey::Escape),
        "space" => ChordKey::Char(' '),
        "backspace" | "bs" => ChordKey::Named(NamedKey::Backspace),
        "delete" | "del" => ChordKey::Named(NamedKey::Delete),
        "up" => ChordKey::Named(NamedKey::ArrowUp),
        "down" => ChordKey::Named(NamedKey::ArrowDown),
        "left" => ChordKey::Named(NamedKey::ArrowLeft),
        "right" => ChordKey::Named(NamedKey::ArrowRight),
        "home" => ChordKey::Named(NamedKey::Home),
        "end" => ChordKey::Named(NamedKey::End),
        "pageup" | "pgup" => ChordKey::Named(NamedKey::PageUp),
        "pagedown" | "pgdn" | "pgdown" => ChordKey::Named(NamedKey::PageDown),
        _ => {
            let mut chars = token.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            ChordKey::Char(c.to_ascii_lowercase())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modified() {
        let c = parse_key_spec("cmd+t").unwrap();
        assert_eq!(c.key, ChordKey::Char('t'));
        assert!(c.mods.sup);
        let c = parse_key_spec("cmd+shift+w").unwrap();
        assert!(c.mods.sup && c.mods.shift);
        let c = parse_key_spec("cmd+1").unwrap();
        assert_eq!(c.key, ChordKey::Char('1'));
        assert!(c.mods.sup);
    }

    #[test]
    fn parses_named() {
        let c = parse_key_spec("esc").unwrap();
        assert_eq!(c.key, ChordKey::Named(NamedKey::Escape));
    }
}
