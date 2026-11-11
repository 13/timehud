//! Global hotkeys as exclusive passive key grabs on the root window.

use super::X;
use crate::hotkey::{Combo, format_combo};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, KeyPressEvent, ModMask};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HotkeyAction {
    Toggle,
    Reset,
    Visibility,
}

/// Lock (CapsLock), Mod2 (NumLock) and Mod5 (ScrollLock) combinations
/// grabbed for every combo, so the hotkey works whatever their state.
const LOCK_VARIANTS: [u16; 8] = [0, 2, 16, 2 | 16, 128, 2 | 128, 16 | 128, 2 | 16 | 128];
/// Modifiers that distinguish combos (Shift, Control, Mod1, Mod4).
const RELEVANT_MODS: u16 = 1 | 4 | 8 | 64;

#[derive(Default)]
pub struct Grabs {
    active: Vec<(Combo, u8, HotkeyAction)>,
}

impl Grabs {
    /// Replaces all grabs with `binds`. Returns the combos that could not be
    /// grabbed with a reason (e.g. already taken by another client).
    pub fn set(&mut self, x: &X, binds: &[(Combo, HotkeyAction)]) -> Vec<(Combo, String)> {
        for (combo, keycode, _) in self.active.drain(..) {
            ungrab(x, combo, keycode);
        }
        let mut failures = Vec::new();
        for &(combo, action) in binds {
            let Some(keycode) = x.keycode(combo.keysym) else {
                failures.push((combo, "no key on this keyboard produces it".into()));
                continue;
            };
            match grab(x, combo, keycode) {
                Ok(()) => self.active.push((combo, keycode, action)),
                Err(reason) => {
                    eprintln!("timehud: cannot grab {}: {reason}", format_combo(&combo));
                    failures.push((combo, reason));
                }
            }
        }
        let _ = x.conn.flush();
        failures
    }

    pub fn match_event(&self, ev: &KeyPressEvent) -> Option<HotkeyAction> {
        let mods = u16::from(ev.state) & RELEVANT_MODS;
        self.active
            .iter()
            .find(|(c, keycode, _)| *keycode == ev.detail && c.mods == mods)
            .map(|(_, _, a)| *a)
    }
}

/// Grabs all lock variants; on any failure releases the ones that succeeded.
fn grab(x: &X, combo: Combo, keycode: u8) -> Result<(), String> {
    for (i, lock) in LOCK_VARIANTS.iter().enumerate() {
        let res = x
            .conn
            .grab_key(
                false,
                x.root,
                ModMask::from(combo.mods | lock),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(ReplyError::from)
            .and_then(|c| c.check());
        if let Err(e) = res {
            for lock in &LOCK_VARIANTS[..i] {
                let _ = x
                    .conn
                    .ungrab_key(keycode, x.root, ModMask::from(combo.mods | lock));
            }
            return Err(match e {
                ReplyError::X11Error(e) if e.error_kind == ErrorKind::Access => {
                    "already grabbed by another application".into()
                }
                e => e.to_string(),
            });
        }
    }
    Ok(())
}

fn ungrab(x: &X, combo: Combo, keycode: u8) {
    for lock in LOCK_VARIANTS {
        let _ = x
            .conn
            .ungrab_key(keycode, x.root, ModMask::from(combo.mods | lock));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::KeyButMask;

    fn press(keycode: u8, state: u16) -> KeyPressEvent {
        KeyPressEvent {
            response_type: 2,
            detail: keycode,
            sequence: 0,
            time: 0,
            root: 1,
            event: 1,
            child: 0,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state: KeyButMask::from(state),
            same_screen: true,
        }
    }

    #[test]
    fn match_ignores_lock_modifiers() {
        let combo = Combo {
            mods: 4 | 1,
            keysym: 0x20,
        };
        let g = Grabs {
            active: vec![(combo, 65, HotkeyAction::Toggle)],
        };
        assert_eq!(g.match_event(&press(65, 5)), Some(HotkeyAction::Toggle));
        // CapsLock + NumLock + a held mouse button don't matter.
        assert_eq!(
            g.match_event(&press(65, 5 | 2 | 16 | 256)),
            Some(HotkeyAction::Toggle)
        );
        // ScrollLock (Mod5) neither.
        assert_eq!(
            g.match_event(&press(65, 5 | 128 | 2)),
            Some(HotkeyAction::Toggle)
        );
        assert_eq!(g.match_event(&press(65, 4)), None);
        assert_eq!(g.match_event(&press(66, 5)), None);
    }

    #[test]
    fn lock_variants_cover_caps_num_scroll_lock() {
        // Every combination of Lock (2), Mod2/NumLock (16), Mod5/ScrollLock (128).
        let mut v = LOCK_VARIANTS.to_vec();
        v.sort_unstable();
        assert_eq!(v, [0, 2, 16, 18, 128, 130, 144, 146]);
        assert!(LOCK_VARIANTS.iter().all(|l| l & RELEVANT_MODS == 0));
    }

    #[test]
    #[ignore = "needs an X display (DISPLAY=:0)"]
    fn smoke_grab_and_release() {
        use x11rb::wrapper::ConnectionExt as _;

        let x = X::connect().unwrap();
        // Ctrl+Alt+Super+F12: unlikely to be bound by anything else.
        let combo = Combo {
            mods: 4 | 8 | 64,
            keysym: 0xffc9,
        };
        let mut g = Grabs::default();
        let failures = g.set(&x, &[(combo, HotkeyAction::Reset)]);
        assert!(failures.is_empty(), "{failures:?}");
        let keycode = x.keycode(combo.keysym).unwrap();
        assert_eq!(
            g.match_event(&press(keycode, combo.mods | 16)),
            Some(HotkeyAction::Reset)
        );
        // A second client cannot take the same combo while we hold it.
        let other = X::connect().unwrap();
        let mut g2 = Grabs::default();
        let failures = g2.set(&other, &[(combo, HotkeyAction::Toggle)]);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].1.contains("already grabbed"), "{failures:?}");
        // After ungrabbing, it can.
        assert!(g.set(&x, &[]).is_empty());
        assert!(g2.set(&other, &[(combo, HotkeyAction::Toggle)]).is_empty());
        g2.set(&other, &[]);
        other.conn.sync().unwrap();
    }
}
