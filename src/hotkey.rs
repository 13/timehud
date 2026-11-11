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

/// Named keys: keysym, canonical name, other accepted spellings (egui's
/// `Key::name()` among them). Matching is case-insensitive.
const NAMED_KEYS: &[(u32, &str, &[&str])] = &[
    (0x20, "Space", &[" "]),
    (0xff0d, "Return", &["enter"]),
    (0xff1b, "Escape", &["esc"]),
    (0xff09, "Tab", &[]),
    (0xff08, "BackSpace", &[]),
    (0xffff, "Delete", &["del"]),
    (0xff50, "Home", &[]),
    (0xff51, "Left", &["arrowleft"]),
    (0xff52, "Up", &["arrowup"]),
    (0xff53, "Right", &["arrowright"]),
    (0xff54, "Down", &["arrowdown"]),
    (0xff55, "PageUp", &["prior"]),
    (0xff56, "PageDown", &["next"]),
    (0xff57, "End", &[]),
    (0xff63, "Insert", &[]),
    (0xff13, "Pause", &[]),
    (0xff61, "Print", &["printscreen"]),
    // Punctuation (Latin-1 keysyms equal the ASCII code). `+` has no
    // single-character spelling: it separates the combo's parts.
    (0x2d, "Minus", &["-"]),
    (0x3d, "Equal", &["equals", "="]),
    (0x2c, "Comma", &[","]),
    (0x2e, "Period", &["."]),
    (0x2f, "Slash", &["/"]),
    (0x5c, "Backslash", &["\\"]),
    (0x3b, "Semicolon", &[";"]),
    (0x27, "Apostrophe", &["quote", "'"]),
    (0x60, "Grave", &["backtick", "`"]),
    (0x5b, "BracketLeft", &["openbracket", "["]),
    (0x5d, "BracketRight", &["closebracket", "]"]),
    (0x3a, "Colon", &[":"]),
    (0x2b, "Plus", &[]),
    (0x7c, "Bar", &["pipe", "|"]),
    (0x3f, "Question", &["questionmark", "?"]),
    (0x21, "Exclam", &["exclamationmark", "!"]),
    (0x7b, "BraceLeft", &["opencurlybracket", "{"]),
    (0x7d, "BraceRight", &["closecurlybracket", "}"]),
    // XF86 media and browser keys.
    (0x1008ff14, "AudioPlay", &["mediaplaypause"]),
    (0x1008ff15, "AudioStop", &["mediastop"]),
    (0x1008ff16, "AudioPrev", &["mediatrackprevious"]),
    (0x1008ff17, "AudioNext", &["mediatracknext"]),
    (0x1008ff12, "AudioMute", &["audiovolumemute"]),
    (0x1008ff11, "AudioLowerVolume", &["audiovolumedown"]),
    (0x1008ff13, "AudioRaiseVolume", &["audiovolumeup"]),
    (0x1008ff19, "Mail", &["launchmail"]),
    (0x1008ff18, "HomePage", &["browserhome"]),
    (0x1008ff1b, "Search", &["browsersearch"]),
    (0x1008ff26, "Back", &["browserback"]),
    (0x1008ff27, "Forward", &["browserforward"]),
    (0x1008ff28, "Stop", &["browserstop"]),
    (0x1008ff29, "Refresh", &["browserrefresh"]),
    (0x1008ff30, "Favorites", &["browserfavorites"]),
];

/// F1..F35 are consecutive keysyms from 0xffbe.
const F1: u32 = 0xffbe;
const MAX_F: u32 = 35;

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
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n @ 1..=MAX_F) = n.parse::<u32>()
    {
        return Some(F1 + n - 1);
    }
    NAMED_KEYS
        .iter()
        .find(|(_, canon, aliases)| {
            canon.eq_ignore_ascii_case(&lower) || aliases.iter().any(|a| *a == lower)
        })
        .map(|(sym, _, _)| *sym)
}

/// X11 keysym to canonical key name.
fn keysym_name(keysym: u32) -> Option<String> {
    match keysym {
        0x61..=0x7a => Some(char::from(keysym as u8).to_ascii_uppercase().to_string()),
        0x30..=0x39 => Some(char::from(keysym as u8).to_string()),
        k if (F1..F1 + MAX_F).contains(&k) => Some(format!("F{}", k - F1 + 1)),
        _ => NAMED_KEYS
            .iter()
            .find(|(sym, _, _)| *sym == keysym)
            .map(|(_, canon, _)| (*canon).to_string()),
    }
}

/// Whether an egui key name is a bare modifier (pressed on its own while
/// a combo is being captured).
pub fn is_modifier_key(key_name: &str) -> bool {
    ["Shift", "Control", "Alt", "Super", "Meta"]
        .iter()
        .any(|m| {
            key_name
                .strip_prefix(m)
                .is_some_and(|r| r.is_empty() || r == "Left" || r == "Right")
        })
}

/// Indices (into `combos`) of entries whose combo another entry also uses;
/// spelling differences ("ctrl+r" vs "Ctrl+R") do not matter, empty or
/// invalid entries are ignored.
pub fn duplicate_combos(combos: &[&str]) -> Vec<usize> {
    let parsed: Vec<Option<Combo>> = combos
        .iter()
        .map(|c| parse_combo(c).ok().flatten())
        .collect();
    (0..parsed.len())
        .filter(|&i| {
            parsed[i].is_some()
                && parsed
                    .iter()
                    .enumerate()
                    .any(|(j, other)| j != i && *other == parsed[i])
        })
        .collect()
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
    fn punctuation_function_and_media_keys() {
        for (egui, sym, canon) in [
            ("Minus", 0x2d, "Ctrl+Minus"),
            ("Equals", 0x3d, "Ctrl+Equal"),
            ("Comma", 0x2c, "Ctrl+Comma"),
            ("Period", 0x2e, "Ctrl+Period"),
            ("Slash", 0x2f, "Ctrl+Slash"),
            ("Backslash", 0x5c, "Ctrl+Backslash"),
            ("Semicolon", 0x3b, "Ctrl+Semicolon"),
            ("Quote", 0x27, "Ctrl+Apostrophe"),
            ("Backtick", 0x60, "Ctrl+Grave"),
            ("OpenBracket", 0x5b, "Ctrl+BracketLeft"),
            ("CloseBracket", 0x5d, "Ctrl+BracketRight"),
            ("Plus", 0x2b, "Ctrl+Plus"),
            ("Questionmark", 0x3f, "Ctrl+Question"),
            ("F13", 0xffca, "Ctrl+F13"),
            ("F24", 0xffd5, "Ctrl+F24"),
            ("F35", 0xffe0, "Ctrl+F35"),
            ("MediaPlayPause", 0x1008ff14, "Ctrl+AudioPlay"),
            ("AudioVolumeUp", 0x1008ff13, "Ctrl+AudioRaiseVolume"),
            ("BrowserBack", 0x1008ff26, "Ctrl+Back"),
        ] {
            let c = egui_to_combo(egui, true, false, false, false).unwrap();
            assert_eq!(c.keysym, sym, "{egui}");
            assert_eq!(format_combo(&c), canon);
            // The canonical form parses back to the same combo.
            assert_eq!(parse_combo(canon).unwrap(), Some(c), "{canon}");
        }
        assert_eq!(parse_combo("Alt+-").unwrap().unwrap().keysym, 0x2d);
        assert!(parse_combo("F36").is_err());
        assert!(egui_to_combo("ShiftLeft", true, false, false, false).is_none());
    }

    #[test]
    fn modifier_keys() {
        assert!(is_modifier_key("ShiftLeft"));
        assert!(is_modifier_key("ControlRight"));
        assert!(is_modifier_key("Alt"));
        assert!(!is_modifier_key("A"));
        assert!(!is_modifier_key("Space"));
    }

    #[test]
    fn duplicates_ignore_spelling_and_empties() {
        assert_eq!(
            duplicate_combos(&["ctrl+shift+r", "", "Ctrl+Shift+R"]),
            vec![0, 2]
        );
        assert!(duplicate_combos(&["", "", "F5"]).is_empty());
        assert!(duplicate_combos(&["Ctrl+R", "Alt+R", "bogus+key"]).is_empty());
    }

    #[test]
    fn egui_super_alt_flags() {
        let c = egui_to_combo("Z", false, false, true, true).unwrap();
        assert_eq!(c.mods, MOD1 | MOD4);
        assert_eq!(format_combo(&c), "Alt+Super+Z");
    }
}
