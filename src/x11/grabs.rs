//! Global hotkeys as exclusive passive key grabs on the root window.

use super::X;
use crate::hotkey::{Combo, format_combo};
use x11rb::connection::Connection;
use x11rb::cookie::VoidCookie;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, KeyPressEvent, ModMask};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HotkeyAction {
    Toggle,
    Reset,
    Visibility,
}

/// Modifiers that distinguish combos (Shift, Control, Mod1, Mod4).
const RELEVANT_MODS: u16 = 1 | 4 | 8 | 64;
const SHIFT: u16 = 1;
/// The core Lock modifier (CapsLock).
const LOCK: u16 = 2;
const NUM_LOCK_KEYSYM: u32 = 0xff7f;
const SCROLL_LOCK_KEYSYM: u32 = 0xff14;

/// The modifier bits that carry NumLock and ScrollLock, per the server's
/// modifier mapping (`keycodes` holds `per` keycodes for each of the 8
/// modifiers). Bits that also distinguish combos are left out, as are
/// unmapped locks (0).
fn lock_bits(keycodes: &[u8], per: usize, keysym: impl Fn(u8) -> u32) -> (u16, u16) {
    let (mut num, mut scroll) = (0u16, 0u16);
    if per == 0 {
        return (0, 0);
    }
    for (i, row) in keycodes.chunks(per).take(8).enumerate() {
        for &kc in row.iter().filter(|kc| **kc != 0) {
            match keysym(kc) {
                NUM_LOCK_KEYSYM => num |= 1 << i,
                SCROLL_LOCK_KEYSYM => scroll |= 1 << i,
                _ => {}
            }
        }
    }
    (num & !RELEVANT_MODS, scroll & !RELEVANT_MODS)
}

/// Every combination of CapsLock, NumLock and ScrollLock bits (without
/// duplicates when a lock is unmapped), grabbed per combo so hotkeys work
/// whatever the lock state.
fn lock_variants(num: u16, scroll: u16) -> Vec<u16> {
    let mut v: Vec<u16> = Vec::with_capacity(8);
    for caps in [0, LOCK] {
        for n in [0, num] {
            for s in [0, scroll] {
                let m = caps | n | s;
                if !v.contains(&m) {
                    v.push(m);
                }
            }
        }
    }
    v
}

/// One grabbed hotkey: the modifiers include Shift when the keysym sits
/// in the keycode's shifted column (e.g. "Ctrl+Plus" on a US layout).
struct Active {
    keycode: u8,
    mods: u16,
    action: HotkeyAction,
}

pub struct Grabs {
    active: Vec<Active>,
    /// Lock variants the active grabs were made with (to ungrab them).
    locks: Vec<u16>,
}

impl Default for Grabs {
    fn default() -> Grabs {
        Grabs {
            active: Vec::new(),
            locks: lock_variants(0, 0),
        }
    }
}

/// NumLock and ScrollLock bits of the current modifier mapping; none if the
/// request fails.
fn query_lock_bits(x: &X) -> (u16, u16) {
    let Some(reply) = x
        .conn
        .get_modifier_mapping()
        .ok()
        .and_then(|c| c.reply().ok())
    else {
        return (0, 0);
    };
    let per = reply.keycodes.len() / 8;
    lock_bits(&reply.keycodes, per, |kc| x.keysym(kc))
}

impl Grabs {
    /// Replaces all grabs with `binds`. Returns the combos that could not be
    /// grabbed with a reason (e.g. already taken by another client).
    pub fn set(&mut self, x: &X, binds: &[(Combo, HotkeyAction)]) -> Vec<(Combo, String)> {
        for a in self.active.drain(..) {
            ungrab(x, a.keycode, a.mods, &self.locks);
        }
        let mut failures = Vec::new();
        if binds.is_empty() {
            let _ = x.conn.flush();
            return failures;
        }
        let (num, scroll) = query_lock_bits(x);
        self.locks = lock_variants(num, scroll);
        for &(combo, action) in binds {
            let Some((keycode, column)) = x.keycode(combo.keysym) else {
                failures.push((
                    combo,
                    "no key on this keyboard produces it without AltGr or a layout switch".into(),
                ));
                continue;
            };
            let mods = if column == 1 {
                combo.mods | SHIFT
            } else {
                combo.mods
            };
            match grab(x, keycode, mods, &self.locks) {
                Ok(()) => self.active.push(Active {
                    keycode,
                    mods,
                    action,
                }),
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
            .find(|a| a.keycode == ev.detail && a.mods == mods)
            .map(|a| a.action)
    }
}

/// Grabs `keycode` + `mods` with every lock variant: all requests are sent
/// before any reply is awaited. On any failure the ones that succeeded are
/// released again.
fn grab(x: &X, keycode: u8, mods: u16, locks: &[u16]) -> Result<(), String> {
    let cookies: Vec<Result<VoidCookie<'_, _>, _>> = locks
        .iter()
        .map(|lock| {
            x.conn.grab_key(
                false,
                x.root,
                ModMask::from(mods | lock),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
        })
        .collect();
    let mut granted = Vec::new();
    let mut error = None;
    for (lock, cookie) in locks.iter().zip(cookies) {
        match cookie.map_err(ReplyError::from).and_then(|c| c.check()) {
            Ok(()) => granted.push(*lock),
            Err(e) => {
                error.get_or_insert(e);
            }
        }
    }
    let Some(e) = error else {
        return Ok(());
    };
    ungrab(x, keycode, mods, &granted);
    Err(match e {
        ReplyError::X11Error(e) if e.error_kind == ErrorKind::Access => {
            "already grabbed by another application".into()
        }
        e => e.to_string(),
    })
}

fn ungrab(x: &X, keycode: u8, mods: u16, locks: &[u16]) {
    for lock in locks {
        let _ = x
            .conn
            .ungrab_key(keycode, x.root, ModMask::from(mods | lock));
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

    fn grabs(active: &[(u8, u16, HotkeyAction)]) -> Grabs {
        Grabs {
            active: active
                .iter()
                .map(|&(keycode, mods, action)| Active {
                    keycode,
                    mods,
                    action,
                })
                .collect(),
            ..Grabs::default()
        }
    }

    #[test]
    fn match_ignores_lock_modifiers() {
        let g = grabs(&[(65, 4 | 1, HotkeyAction::Toggle)]);
        assert_eq!(g.match_event(&press(65, 5)), Some(HotkeyAction::Toggle));
        // CapsLock + NumLock + a held mouse button don't matter.
        assert_eq!(
            g.match_event(&press(65, 5 | 2 | 16 | 256)),
            Some(HotkeyAction::Toggle)
        );
        // ScrollLock (Mod5 here) neither.
        assert_eq!(
            g.match_event(&press(65, 5 | 128 | 2)),
            Some(HotkeyAction::Toggle)
        );
        assert_eq!(g.match_event(&press(65, 4)), None);
        assert_eq!(g.match_event(&press(66, 5)), None);
    }

    #[test]
    fn shifted_keysym_matches_with_shift_held() {
        // "Ctrl+Plus" on the shifted '=' key was grabbed as Ctrl+Shift.
        let g = grabs(&[(21, 4 | SHIFT, HotkeyAction::Reset)]);
        assert_eq!(g.match_event(&press(21, 4 | 1)), Some(HotkeyAction::Reset));
        assert_eq!(g.match_event(&press(21, 4)), None);
    }

    /// 8 modifiers x 2 keycodes: Shift(50, 62), Lock(66), Control(37, 105),
    /// Mod1(64), Mod2(77 = Num_Lock), Mod3(-), Mod4(133), Mod5(92 = ISO_Level3_Shift).
    fn modmap() -> Vec<u8> {
        vec![50, 62, 66, 0, 37, 105, 64, 0, 77, 0, 0, 0, 133, 0, 92, 0]
    }

    fn keysym(kc: u8) -> u32 {
        match kc {
            77 => NUM_LOCK_KEYSYM,
            78 => SCROLL_LOCK_KEYSYM,
            92 => 0xfe03, // ISO_Level3_Shift
            _ => 0x61,
        }
    }

    #[test]
    fn lock_bits_follow_the_modifier_mapping() {
        // NumLock on Mod2; ScrollLock unmapped (Mod5 is AltGr here).
        assert_eq!(lock_bits(&modmap(), 2, keysym), (16, 0));
        // ScrollLock added to Mod3.
        let mut m = modmap();
        m[10] = 78;
        assert_eq!(lock_bits(&m, 2, keysym), (16, 32));
        // A lock on a combo modifier (Mod4) is not treated as a lock.
        let mut m = modmap();
        m[12] = 77;
        m[8] = 0;
        assert_eq!(lock_bits(&m, 2, keysym), (0, 0));
        assert_eq!(lock_bits(&[], 0, keysym), (0, 0));
    }

    #[test]
    fn lock_variants_cover_mapped_locks_only() {
        let mut v = lock_variants(16, 128);
        v.sort_unstable();
        assert_eq!(v, [0, 2, 16, 18, 128, 130, 144, 146]);
        let mut v = lock_variants(16, 0);
        v.sort_unstable();
        assert_eq!(v, [0, 2, 16, 18], "no duplicates for an unmapped lock");
        assert_eq!(lock_variants(0, 0).len(), 2);
        assert!(v.iter().all(|l| l & RELEVANT_MODS == 0));
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
        let (keycode, _) = x.keycode(combo.keysym).unwrap();
        let (num, _) = query_lock_bits(&x);
        assert_eq!(
            g.match_event(&press(keycode, combo.mods | num)),
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

    #[test]
    #[ignore = "needs an X display (DISPLAY=:0)"]
    fn smoke_shifted_keysym_grabs_with_shift() {
        use x11rb::wrapper::ConnectionExt as _;

        let x = X::connect().unwrap();
        // Ctrl+Alt+Super+BraceLeft: Shift is added on layouts where '{' is shifted.
        let combo = Combo {
            mods: 4 | 8 | 64,
            keysym: 0x7b,
        };
        let Some((keycode, column)) = x.keycode(combo.keysym) else {
            return;
        };
        let mut g = Grabs::default();
        assert!(g.set(&x, &[(combo, HotkeyAction::Toggle)]).is_empty());
        let shift = if column == 1 { SHIFT } else { 0 };
        assert_eq!(
            g.match_event(&press(keycode, combo.mods | shift)),
            Some(HotkeyAction::Toggle)
        );
        g.set(&x, &[]);
        x.conn.sync().unwrap();
    }
}
