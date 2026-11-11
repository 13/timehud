//! Hotkey combo parsing and formatting. Pure logic; keysyms are plain X11 values.

/// X11 ModMask bits.
const SHIFT: u16 = 1;
const CONTROL: u16 = 4;
const MOD1: u16 = 8;
const MOD4: u16 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    /// X11 ModMask bits: Shift=1, Control=4, Mod1(Alt)=8, Mod4(Super)=64.
    pub mods: u16,
    pub keysym: u32,
}

/// Parses `"Ctrl+Shift+Space"`-style strings. Empty input means no hotkey.
pub fn parse_combo(s: &str) -> Result<Option<Combo>, String> {
    if s.trim().is_empty() {
        return Ok(None);
    }
    let mut mods = 0;
    let mut keysym = None;
    for part in s.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("empty key in combo '{s}'"));
        }
        match modifier_bit(part) {
            Some(bit) => mods |= bit,
            None if keysym.is_some() => return Err(format!("more than one key in '{s}'")),
            None => {
                keysym = Some(keysym_for_name(part).ok_or_else(|| format!("unknown key '{part}'"))?)
            }
        }
    }
    match keysym {
        Some(keysym) => Ok(Some(Combo { mods, keysym })),
        None => Err(format!("no key in combo '{s}'")),
    }
}

/// Canonical form, e.g. `"Ctrl+Shift+R"`. Modifier order: Ctrl, Shift, Alt, Super.
pub fn format_combo(c: &Combo) -> String {
    let key = keysym_name(c.keysym).unwrap_or_else(|| format!("0x{:x}", c.keysym));
    let mut parts: Vec<&str> = Vec::new();
    for (bit, name) in [
        (CONTROL, "Ctrl"),
        (SHIFT, "Shift"),
        (MOD1, "Alt"),
        (MOD4, "Super"),
    ] {
        if c.mods & bit != 0 {
            parts.push(name);
        }
    }
    parts.push(&key);
    parts.join("+")
}

/// Builds a combo from an egui key name and modifier flags. `None` for unknown keys.
pub fn egui_to_combo(
    key_name: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    super_: bool,
) -> Option<Combo> {
    // egui names the digit keys "Num0".."Num9".
    let key = match key_name.strip_prefix("Num") {
        Some(d) if d.len() == 1 && d.bytes().all(|b| b.is_ascii_digit()) => d,
        _ => key_name,
    };
    let mut mods = 0;
    for (on, bit) in [(ctrl, CONTROL), (shift, SHIFT), (alt, MOD1), (super_, MOD4)] {
        if on {
            mods |= bit;
        }
    }
    Some(Combo {
        mods,
        keysym: keysym_for_name(key)?,
    })
}

fn modifier_bit(name: &str) -> Option<u16> {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(CONTROL),
        "shift" => Some(SHIFT),
        "alt" | "mod1" => Some(MOD1),
        "super" | "win" | "mod4" => Some(MOD4),
        _ => None,
    }
}

/// Case-insensitive key name to X11 keysym.
fn keysym_for_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    if let [b] = lower.as_bytes()
        && (b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        // Latin-1 keysyms for letters and digits equal their ASCII codes.
        return Some(u32::from(*b));
    }
    if let Some(n) = lower.strip_prefix('f')
        && n.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n @ 1..=12) = n.parse::<u32>()
    {
        return Some(0xffbd + n);
    }
    Some(match lower.as_str() {
        "space" => 0x20,
        "return" | "enter" => 0xff0d,
        "escape" | "esc" => 0xff1b,
        "tab" => 0xff09,
        "backspace" => 0xff08,
        "delete" | "del" => 0xffff,
        "home" => 0xff50,
        "left" | "arrowleft" => 0xff51,
        "up" | "arrowup" => 0xff52,
        "right" | "arrowright" => 0xff53,
        "down" | "arrowdown" => 0xff54,
        "prior" | "pageup" => 0xff55,
        "next" | "pagedown" => 0xff56,
        "end" => 0xff57,
        "insert" => 0xff63,
        "pause" => 0xff13,
        _ => return None,
    })
}

/// X11 keysym to canonical key name.
fn keysym_name(keysym: u32) -> Option<String> {
    Some(match keysym {
        0x61..=0x7a => char::from(keysym as u8).to_ascii_uppercase().to_string(),
        0x30..=0x39 => char::from(keysym as u8).to_string(),
        0xffbe..=0xffc9 => format!("F{}", keysym - 0xffbd),
        0x20 => "Space".into(),
        0xff0d => "Return".into(),
        0xff1b => "Escape".into(),
        0xff09 => "Tab".into(),
        0xff08 => "BackSpace".into(),
        0xffff => "Delete".into(),
        0xff50 => "Home".into(),
        0xff51 => "Left".into(),
        0xff52 => "Up".into(),
        0xff53 => "Right".into(),
        0xff54 => "Down".into(),
        0xff55 => "PageUp".into(),
        0xff56 => "PageDown".into(),
        0xff57 => "End".into(),
        0xff63 => "Insert".into(),
        0xff13 => "Pause".into(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_ctrl_shift_r() {
        let c = parse_combo("ctrl+shift+r").unwrap().unwrap();
        assert_eq!(c.mods, CONTROL | SHIFT);
        assert_eq!(c.keysym, 0x72);
        assert_eq!(format_combo(&c), "Ctrl+Shift+R");
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(parse_combo("").unwrap(), None);
    }

    #[test]
    fn modifiers_without_key_is_error() {
        assert!(parse_combo("Ctrl+Shift").is_err());
    }

    #[test]
    fn unknown_key_is_error() {
        assert!(parse_combo("Ctrl+Banana").is_err());
    }

    #[test]
    fn f5_keysym() {
        let c = parse_combo("F5").unwrap().unwrap();
        assert_eq!(c.keysym, 0xffc2);
        assert_eq!(c.mods, 0);
    }

    #[test]
    fn space_keysym_and_format() {
        let c = parse_combo("Ctrl+Shift+Space").unwrap().unwrap();
        assert_eq!(c.keysym, 0x20);
        assert_eq!(format_combo(&c), "Ctrl+Shift+Space");
    }

    #[test]
    fn modifier_aliases() {
        let c = parse_combo("Control+Alt+Win+x").unwrap().unwrap();
        assert_eq!(c.mods, CONTROL | MOD1 | MOD4);
        assert_eq!(format_combo(&c), "Ctrl+Alt+Super+X");
        let c = parse_combo("mod1+mod4+shift+7").unwrap().unwrap();
        assert_eq!(c.mods, MOD1 | MOD4 | SHIFT);
        assert_eq!(c.keysym, 0x37);
    }

    #[test]
    fn named_keys() {
        assert_eq!(parse_combo("Return").unwrap().unwrap().keysym, 0xff0d);
        assert_eq!(parse_combo("Enter").unwrap().unwrap().keysym, 0xff0d);
        assert_eq!(parse_combo("Escape").unwrap().unwrap().keysym, 0xff1b);
        assert_eq!(parse_combo("PageUp").unwrap().unwrap().keysym, 0xff55);
        assert_eq!(parse_combo("Prior").unwrap().unwrap().keysym, 0xff55);
        assert_eq!(parse_combo("PageDown").unwrap().unwrap().keysym, 0xff56);
        assert_eq!(parse_combo("Next").unwrap().unwrap().keysym, 0xff56);
        assert_eq!(parse_combo("Left").unwrap().unwrap().keysym, 0xff51);
        assert_eq!(parse_combo("Pause").unwrap().unwrap().keysym, 0xff13);
        assert_eq!(parse_combo("F12").unwrap().unwrap().keysym, 0xffc9);
    }

    #[test]
    fn egui_names_map() {
        let c = egui_to_combo("A", true, true, false, false).unwrap();
        assert_eq!(c.keysym, 0x61);
        assert_eq!(c.mods, CONTROL | SHIFT);
        assert_eq!(format_combo(&c), "Ctrl+Shift+A");
        assert_eq!(
            egui_to_combo("Space", false, false, false, false)
                .unwrap()
                .keysym,
            0x20
        );
        assert_eq!(
            egui_to_combo("F5", false, false, false, false)
                .unwrap()
                .keysym,
            0xffc2
        );
        assert_eq!(
            egui_to_combo("Enter", false, false, false, false)
                .unwrap()
                .keysym,
            0xff0d
        );
        assert_eq!(
            egui_to_combo("PageUp", false, false, false, false)
                .unwrap()
                .keysym,
            0xff55
        );
        assert_eq!(
            egui_to_combo("ArrowLeft", false, false, false, false)
                .unwrap()
                .keysym,
            0xff51
        );
        assert_eq!(
            egui_to_combo("Num5", false, false, false, false)
                .unwrap()
                .keysym,
            0x35
        );
        assert!(egui_to_combo("Banana", false, false, false, false).is_none());
    }

    #[test]
    fn egui_super_alt_flags() {
        let c = egui_to_combo("Z", false, false, true, true).unwrap();
        assert_eq!(c.mods, MOD1 | MOD4);
        assert_eq!(format_combo(&c), "Alt+Super+Z");
    }
}
