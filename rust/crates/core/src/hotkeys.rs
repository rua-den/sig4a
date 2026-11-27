//! Shared shortcut grammar and dashboard/tray protocol. Windows ownership is checked by the tray.
use serde_json::{json, Value};

pub const KEYS: [&str; 4] = ["toolbox", "grammar", "translation", "ocr"];
pub const CONFIG: &str = "global_shortcuts";
pub const TRAY_CLASS: &str = "ClaudeUsageTray";
pub const APPLY_MESSAGE: usize = 0x5347484b;
pub const STATUS_MESSAGE: u32 = 0x8000 + 4;
pub const OPEN_MESSAGE: u32 = 0x8000 + 3;
pub const OK: usize = 1;
pub const INVALID: usize = 2;
pub const IO_ERROR: usize = 3;
pub const DISABLED: usize = 4;
pub const CONFLICT: usize = 16;
pub const STATUS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub mods: u32,
    pub vk: u32,
}
pub type Bindings = [Option<Chord>; 4];

impl Chord {
    pub fn parse(s: &str) -> Result<Option<Self>, ()> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        if s.len() > 40 || !s.is_ascii() {
            return Err(());
        }
        let parts: Vec<_> = s.split('+').map(str::trim).collect();
        let Some((key, mods)) = parts.split_last() else { return Err(()); };
        let mut flags = 0;
        for m in mods {
            let bit = match m.to_ascii_lowercase().as_str() {
                "alt" => 1,
                "ctrl" => 2,
                "shift" => 4,
                _ => return Err(()),
            };
            if flags & bit != 0 { return Err(()); }
            flags |= bit;
        }
        // Always require Ctrl or Alt, so a plain letter, Shift+digit, or F key never gets intercepted.
        if flags & 3 == 0 { return Err(()); }
        let key = key.to_ascii_uppercase();
        let vk = if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() {
            u32::from(key.as_bytes()[0])
        } else if let Some(f) = key.strip_prefix('F') {
            let n: u32 = f.parse().map_err(|_| ())?;
            if !(1..=11).contains(&n) || n.to_string() != f { return Err(()); }
            0x6f + n
        } else {
            return Err(());
        };
        Ok(Some(Self { mods: flags, vk }))
    }

    pub fn label(self) -> String {
        let mut s = String::new();
        for (bit, name) in [(2, "Ctrl+"), (1, "Alt+"), (4, "Shift+")] {
            if self.mods & bit != 0 { s.push_str(name); }
        }
        if self.vk >= 0x70 {
            s.push_str(&format!("F{}", self.vk - 0x6f));
        } else {
            s.push(self.vk as u8 as char);
        }
        s
    }
}

pub fn defaults() -> Bindings {
    [None, Some(Chord { mods: 3, vk: b'G' as u32 }),
        Some(Chord { mods: 3, vk: b'T' as u32 }), Some(Chord { mods: 3, vk: b'O' as u32 })]
}

/// Error: offending action index and whether it repeats a chord used by an earlier action.
pub fn parse(v: &Value) -> Result<Bindings, (usize, bool)> {
    let mut out = [None; 4];
    for (i, key) in KEYS.iter().enumerate() {
        let value = v[*key].as_str().ok_or((i, false))?;
        let chord = Chord::parse(value).map_err(|_| (i, false))?;
        if chord.is_some() && out[..i].contains(&chord) {
            return Err((i, true));
        }
        out[i] = chord;
    }
    Ok(out)
}

pub fn configured(cfg: &crate::config::Config) -> Bindings {
    cfg.get(CONFIG).and_then(|v| parse(v).ok()).unwrap_or_else(defaults)
}

pub fn value(bindings: Bindings) -> Value {
    let mut out = serde_json::Map::new();
    for (key, chord) in KEYS.into_iter().zip(bindings) {
        out.insert(key.to_string(), json!(chord.map(Chord::label).unwrap_or_default()));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_canonicalizes_distinct_bindings_and_explicit_disable() {
        let p = parse(&json!({"toolbox":"alt + ctrl+b","grammar":"","translation":"Ctrl+Shift+f11","ocr":"Alt+9"})).unwrap();
        assert_eq!(value(p), json!({"toolbox":"Ctrl+Alt+B","grammar":"","translation":"Ctrl+Shift+F11","ocr":"Alt+9"}));
        assert_eq!(parse(&value(defaults())), Ok(defaults()));
        assert_eq!(configured(&crate::config::defaults()), defaults());
    }

    #[test]
    fn detects_canonical_duplicates_and_rejects_unsupported_keys() {
        assert_eq!(parse(&json!({"toolbox":"Alt+Ctrl+t","grammar":"","translation":"Ctrl+Alt+T","ocr":"Ctrl+O"})), Err((2, true)));
        for invalid in ["Q", "Shift+Q", "Alt+F12", "Win+Q", "Alt+Ctrl+Alt+Q", "Alt+Delete", "Alt++Q", "Alt+F01", "Ctrl+Ố"] {
            assert!(Chord::parse(invalid).is_err(), "{invalid}");
        }
    }
}
