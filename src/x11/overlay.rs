//! The HUD's override-redirect ARGB overlay window and its input translation.

use super::X;
use crate::app::{Button, Cursor, Input, Key};
use crate::layout::Rect;
use std::collections::HashMap;
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, SK, SO};
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, EventMask,
    KeyButMask, MapState, StackMode,
};

/// A glyph cursor from the core "cursor" font (black on white, like Xlib's
/// `XCreateFontCursor`). The font is closed again; the cursor keeps it.
fn hand_cursor(x: &X) -> Option<u32> {
    let font = x.conn.generate_id().ok()?;
    x.conn.open_font(font, b"cursor").ok()?.check().ok()?;
    let cursor = x.conn.generate_id().ok()?;
    let made = x
        .conn
        .create_glyph_cursor(
            cursor,
            font,
            font,
            XC_HAND2,
            XC_HAND2 + 1,
            0,
            0,
            0,
            0xffff,
            0xffff,
            0xffff,
        )
        .ok()
        .and_then(|c| c.check().ok());
    let _ = x.conn.close_font(font);
    made.map(|()| cursor)
}

pub struct OverlayWin {
    pub id: u32,
    pub gc: u32,
    /// Button 1 pressed on the overlay and not yet released.
    left_held: bool,
    /// The core-font hand cursor, created on first use (0 = failed).
    hand: Option<u32>,
}

/// `XC_hand2` in the core "cursor" font; its mask is the next glyph.
const XC_HAND2: u16 = 60;

impl OverlayWin {
    /// Creates the (unmapped) overlay: override-redirect, ARGB, `WM_CLASS`
    /// "timehud", `_NET_WM_NAME` "TimeHUD", `_COMPTON_SHADOW = 0`, type
    /// `_NET_WM_WINDOW_TYPE_NOTIFICATION`.
    pub fn create(x: &X, w: i32, h: i32, px: i32, py: i32) -> anyhow::Result<Self> {
        let events = EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::ENTER_WINDOW
            | EventMask::LEAVE_WINDOW
            | EventMask::KEY_PRESS
            | EventMask::EXPOSURE;
        let (id, gc) = x.create_window(
            Rect { x: px, y: py, w, h },
            events,
            "TimeHUD",
            x.atoms._NET_WM_WINDOW_TYPE_NOTIFICATION,
            false,
        )?;
        x.conn.flush()?;
        Ok(OverlayWin {
            id,
            gc,
            left_held: false,
            hand: None,
        })
    }

    /// Sets the pointer shape over the overlay. `Default` is `None`, so the
    /// root window's cursor shows through.
    pub fn set_cursor(&mut self, x: &X, cursor: Cursor) {
        let id = match cursor {
            Cursor::Default => NONE,
            Cursor::Hand => *self
                .hand
                .get_or_insert_with(|| hand_cursor(x).unwrap_or(NONE)),
        };
        let aux = ChangeWindowAttributesAux::new().cursor(id);
        let _ = x.conn.change_window_attributes(self.id, &aux);
        let _ = x.conn.flush();
    }

    pub fn set_geometry(&self, x: &X, px: i32, py: i32, w: i32, h: i32) {
        let aux = ConfigureWindowAux::new()
            .x(px)
            .y(py)
            .width(w.max(1) as u32)
            .height(h.max(1) as u32);
        let _ = x.conn.configure_window(self.id, &aux);
        let _ = x.conn.flush();
    }

    /// `on`: empty input shape, so all pointer input passes through.
    /// `off`: input shape reset to the window's bounding shape.
    pub fn set_click_through(&self, x: &X, on: bool) {
        let _ = if on {
            shape::rectangles(
                &x.conn,
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                self.id,
                0,
                0,
                &[],
            )
        } else {
            shape::mask(&x.conn, SO::SET, SK::INPUT, self.id, 0, 0, NONE)
        };
        let _ = x.conn.flush();
    }

    pub fn raise(&self, x: &X) {
        raise_window(x, self.id);
    }

    pub fn map(&self, x: &X) {
        let _ = x.conn.map_window(self.id);
        let _ = x.conn.flush();
    }

    pub fn unmap(&self, x: &X) {
        let _ = x.conn.unmap_window(self.id);
        let _ = x.conn.flush();
    }
}

pub(super) fn raise_window(x: &X, win: u32) {
    let _ = x
        .conn
        .configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    let _ = x.conn.flush();
}

/// True when another top-level window was mapped or restacked directly above
/// one of `own` (the overlay and any popup windows), i.e. the overlay should
/// be raised again (then any open popup above it). Override-redirect
/// windows (screen lockers, other overlays, menus) are left alone so the two
/// never fight in a raise loop. Requires the root `SubstructureNotify`
/// selection made by [`X::connect`].
pub fn restack_needed(ev: &Event, root: u32, own: &[u32]) -> bool {
    match ev {
        Event::MapNotify(e) => e.event == root && !e.override_redirect && !own.contains(&e.window),
        Event::ConfigureNotify(e) => {
            e.event == root
                && !e.override_redirect
                && !own.contains(&e.window)
                && own.contains(&e.above_sibling)
        }
        _ => false,
    }
}

/// Spots restacks of other top-levels that may have put them above the
/// overlay indirectly (above some other sibling of ours, e.g. a
/// notification), which [`restack_needed`] cannot see. Remembers each
/// managed top-level's `above_sibling`, so the many ConfigureNotify events
/// of plain moves and resizes are not mistaken for restacks.
#[derive(Debug, Default)]
pub struct StackWatch {
    above: HashMap<u32, u32>,
}

impl StackWatch {
    /// True when `ev` restacked a non-override-redirect top-level (not one
    /// of `own`) somewhere above the bottom: then check [`covered`].
    pub fn restacked(&mut self, ev: &Event, root: u32, own: &[u32]) -> bool {
        match ev {
            Event::ConfigureNotify(e)
                if e.event == root && !e.override_redirect && !own.contains(&e.window) =>
            {
                let before = self.above.insert(e.window, e.above_sibling);
                e.above_sibling != NONE && before != Some(e.above_sibling)
            }
            Event::DestroyNotify(e) if e.event == root => {
                self.above.remove(&e.window);
                false
            }
            // Reparented into a WM frame: no longer a top-level.
            Event::ReparentNotify(e) if e.event == root && e.parent != root => {
                self.above.remove(&e.window);
                false
            }
            _ => false,
        }
    }
}

/// Whether a mapped, non-override-redirect window other than `own` is
/// stacked above `overlay`. `stack` is the root's children bottom to top
/// (QueryTree order); `attrs` gives a window's (viewable, override-redirect)
/// state, `None` if it is gone. Override-redirect windows are not fought
/// (see [`restack_needed`]).
pub fn covered(
    stack: &[u32],
    overlay: u32,
    own: &[u32],
    attrs: impl Fn(u32) -> Option<(bool, bool)>,
) -> bool {
    let Some(i) = stack.iter().position(|&w| w == overlay) else {
        return false;
    };
    stack[i + 1..]
        .iter()
        .filter(|w| !own.contains(w))
        .any(|&w| attrs(w).is_some_and(|(viewable, or)| viewable && !or))
}

/// [`covered`] against the live stacking order (one QueryTree, then the
/// attributes of the windows above the overlay, pipelined).
pub fn covered_now(x: &X, overlay: u32, own: &[u32]) -> bool {
    let Some(tree) = x.conn.query_tree(x.root).ok().and_then(|c| c.reply().ok()) else {
        return false;
    };
    let stack = tree.children;
    let Some(i) = stack.iter().position(|&w| w == overlay) else {
        return false;
    };
    let cookies: Vec<_> = stack[i + 1..]
        .iter()
        .filter(|w| !own.contains(w))
        .map(|&w| (w, x.conn.get_window_attributes(w).ok()))
        .collect();
    let attrs: HashMap<u32, (bool, bool)> = cookies
        .into_iter()
        .filter_map(|(w, c)| {
            let a = c?.reply().ok()?;
            Some((w, (a.map_state == MapState::VIEWABLE, a.override_redirect)))
        })
        .collect();
    covered(&stack, overlay, own, |w| attrs.get(&w).copied())
}

fn button(detail: u8) -> Option<Button> {
    match detail {
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        _ => None,
    }
}

fn key(keysym: u32) -> Key {
    match keysym {
        0x20 => Key::Space,
        0x72 | 0x52 => Key::R,
        0x71 | 0x51 => Key::Q,
        0xff1b => Key::Escape,
        _ => Key::Other,
    }
}

impl OverlayWin {
    /// Translates an event for the overlay into an app input; `keysym` maps a
    /// keycode to its unshifted keysym ([`X::keysym`]).
    ///
    /// Motion is always emitted (the app needs it for button hover). To keep a
    /// lost ButtonRelease from leaving the window stuck dragging, a motion event
    /// whose state lacks `Button1Mask` while a left press is outstanding is
    /// turned into a synthetic `Release { button: Left }` instead (that one
    /// motion's hover update is dropped; the next motion restores it).
    pub fn translate(&mut self, ev: &Event, keysym: impl Fn(u8) -> u32) -> Option<Input> {
        let win = self.id;
        match ev {
            Event::ButtonPress(e) if e.event == win => {
                let (x, y) = (e.event_x.into(), e.event_y.into());
                match e.detail {
                    4 | 5 => Some(Input::Wheel {
                        x,
                        y,
                        up: e.detail == 4,
                    }),
                    d => {
                        let button = button(d)?;
                        if button == Button::Left {
                            self.left_held = true;
                        }
                        Some(Input::Press {
                            x,
                            y,
                            root_x: e.root_x.into(),
                            root_y: e.root_y.into(),
                            button,
                        })
                    }
                }
            }
            Event::ButtonRelease(e) if e.event == win => {
                let button = button(e.detail)?;
                if button == Button::Left {
                    self.left_held = false;
                }
                Some(Input::Release {
                    root_x: e.root_x.into(),
                    root_y: e.root_y.into(),
                    button,
                })
            }
            Event::MotionNotify(e) if e.event == win => {
                let (root_x, root_y) = (e.root_x.into(), e.root_y.into());
                if !e.state.contains(KeyButMask::BUTTON1) && std::mem::take(&mut self.left_held) {
                    return Some(Input::Release {
                        root_x,
                        root_y,
                        button: Button::Left,
                    });
                }
                Some(Input::Motion {
                    x: e.event_x.into(),
                    y: e.event_y.into(),
                    root_x,
                    root_y,
                })
            }
            Event::EnterNotify(e) if e.event == win => Some(Input::Enter),
            Event::LeaveNotify(e) if e.event == win => Some(Input::Leave),
            Event::KeyPress(e) if e.event == win => Some(Input::Key {
                key: key(keysym(e.detail)),
                ctrl: e.state.contains(KeyButMask::CONTROL),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::{
        ButtonPressEvent, ConfigureNotifyEvent, MapNotifyEvent, MotionNotifyEvent,
    };

    const WIN: u32 = 0x40_0001;
    const ROOT: u32 = 0x1e3;

    fn overlay() -> OverlayWin {
        OverlayWin {
            id: WIN,
            gc: 0,
            left_held: false,
            hand: None,
        }
    }

    fn press(detail: u8) -> ButtonPressEvent {
        ButtonPressEvent {
            response_type: 4,
            detail,
            sequence: 0,
            time: 0,
            root: ROOT,
            event: WIN,
            child: 0,
            root_x: 110,
            root_y: 220,
            event_x: 10,
            event_y: 20,
            state: KeyButMask::default(),
            same_screen: true,
        }
    }

    fn motion(state: KeyButMask) -> Event {
        Event::MotionNotify(MotionNotifyEvent {
            response_type: 6,
            detail: 0.into(),
            sequence: 0,
            time: 0,
            root: ROOT,
            event: WIN,
            child: 0,
            root_x: 130,
            root_y: 240,
            event_x: 30,
            event_y: 40,
            state,
            same_screen: true,
        })
    }

    #[test]
    fn buttons_and_wheel() {
        let mut w = overlay();
        assert_eq!(
            w.translate(&Event::ButtonPress(press(3)), |_| 0),
            Some(Input::Press {
                x: 10,
                y: 20,
                root_x: 110,
                root_y: 220,
                button: Button::Right
            })
        );
        assert_eq!(
            w.translate(&Event::ButtonPress(press(4)), |_| 0),
            Some(Input::Wheel {
                x: 10,
                y: 20,
                up: true
            })
        );
        assert_eq!(
            w.translate(&Event::ButtonPress(press(5)), |_| 0),
            Some(Input::Wheel {
                x: 10,
                y: 20,
                up: false
            })
        );
        assert_eq!(w.translate(&Event::ButtonPress(press(8)), |_| 0), None);
        // Wheel "releases" are dropped.
        assert_eq!(w.translate(&Event::ButtonRelease(press(4)), |_| 0), None);
        // Other windows' events are ignored.
        let mut other = OverlayWin {
            id: WIN + 1,
            ..overlay()
        };
        assert_eq!(other.translate(&Event::ButtonPress(press(1)), |_| 0), None);
    }

    #[test]
    fn motion_without_button_is_hover() {
        let mut w = overlay();
        assert_eq!(
            w.translate(&motion(KeyButMask::default()), |_| 0),
            Some(Input::Motion {
                x: 30,
                y: 40,
                root_x: 130,
                root_y: 240
            })
        );
    }

    #[test]
    fn drag_motion_then_release() {
        let mut w = overlay();
        w.translate(&Event::ButtonPress(press(1)), |_| 0);
        assert!(matches!(
            w.translate(&motion(KeyButMask::BUTTON1), |_| 0),
            Some(Input::Motion { .. })
        ));
        assert!(matches!(
            w.translate(&Event::ButtonRelease(press(1)), |_| 0),
            Some(Input::Release {
                button: Button::Left,
                ..
            })
        ));
        // Released normally: later plain motion is hover again.
        assert!(matches!(
            w.translate(&motion(KeyButMask::default()), |_| 0),
            Some(Input::Motion { .. })
        ));
    }

    #[test]
    fn lost_release_ends_drag() {
        let mut w = overlay();
        w.translate(&Event::ButtonPress(press(1)), |_| 0);
        // The release went elsewhere; the first button-less motion ends the drag.
        assert_eq!(
            w.translate(&motion(KeyButMask::default()), |_| 0),
            Some(Input::Release {
                root_x: 130,
                root_y: 240,
                button: Button::Left
            })
        );
        assert!(matches!(
            w.translate(&motion(KeyButMask::default()), |_| 0),
            Some(Input::Motion { .. })
        ));
    }

    #[test]
    fn key_mapping() {
        assert_eq!(key(0x20), Key::Space);
        assert_eq!(key(0x52), Key::R);
        assert_eq!(key(0x71), Key::Q);
        assert_eq!(key(0xff1b), Key::Escape);
        assert_eq!(key(0x61), Key::Other);
    }

    fn configure(window: u32, above: u32) -> Event {
        Event::ConfigureNotify(ConfigureNotifyEvent {
            response_type: 22,
            sequence: 0,
            event: ROOT,
            window,
            above_sibling: above,
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            border_width: 0,
            override_redirect: false,
        })
    }

    #[test]
    fn restack_triggers() {
        let own = [WIN, WIN + 1];
        let map = |window| {
            Event::MapNotify(MapNotifyEvent {
                response_type: 19,
                sequence: 0,
                event: ROOT,
                window,
                override_redirect: false,
            })
        };
        assert!(restack_needed(&map(0x77), ROOT, &own));
        assert!(!restack_needed(&map(WIN), ROOT, &own));
        // Another window went directly above the overlay or popup.
        assert!(restack_needed(&configure(0x77, WIN), ROOT, &own));
        assert!(restack_needed(&configure(0x77, WIN + 1), ROOT, &own));
        // Restacked somewhere below us, or our own restack.
        assert!(!restack_needed(&configure(0x77, 0x55), ROOT, &own));
        assert!(!restack_needed(&configure(WIN, 0x77), ROOT, &own));
    }

    #[test]
    fn restack_ignores_override_redirect_windows() {
        // Screen lockers and other overlays manage their own stacking:
        // fighting them would be an endless raise loop.
        let own = [WIN];
        let map = Event::MapNotify(MapNotifyEvent {
            response_type: 19,
            sequence: 0,
            event: ROOT,
            window: 0x77,
            override_redirect: true,
        });
        assert!(!restack_needed(&map, ROOT, &own));
        let Event::ConfigureNotify(mut e) = configure(0x77, WIN) else {
            unreachable!()
        };
        e.override_redirect = true;
        assert!(!restack_needed(&Event::ConfigureNotify(e), ROOT, &own));
    }

    #[test]
    fn stack_watch_reports_restacks_not_moves() {
        let own = [WIN];
        let mut w = StackWatch::default();
        // First sighting above some sibling: may be a restack.
        assert!(w.restacked(&configure(0x77, 0x55), ROOT, &own));
        // Moved/resized, same sibling: not a restack.
        assert!(!w.restacked(&configure(0x77, 0x55), ROOT, &own));
        // Restacked above another sibling (e.g. a notification above us).
        assert!(w.restacked(&configure(0x77, 0x66), ROOT, &own));
        // Lowered to the bottom: cannot cover us.
        assert!(!w.restacked(&configure(0x77, NONE), ROOT, &own));
        // Our own and override-redirect windows are ignored.
        assert!(!w.restacked(&configure(WIN, 0x55), ROOT, &own));
        let Event::ConfigureNotify(mut e) = configure(0x88, 0x55) else {
            unreachable!()
        };
        e.override_redirect = true;
        assert!(!w.restacked(&Event::ConfigureNotify(e), ROOT, &own));
        // Destroyed windows are forgotten (a new one may reuse the id).
        w.restacked(&configure(0x99, 0x55), ROOT, &own);
        let destroy = Event::DestroyNotify(x11rb::protocol::xproto::DestroyNotifyEvent {
            response_type: 17,
            sequence: 0,
            event: ROOT,
            window: 0x99,
        });
        assert!(!w.restacked(&destroy, ROOT, &own));
        assert!(w.restacked(&configure(0x99, 0x55), ROOT, &own));
    }

    #[test]
    fn covered_only_by_mapped_managed_windows_above() {
        const POPUP: u32 = WIN + 1;
        let own = [WIN, POPUP];
        // (viewable, override-redirect)
        let attrs = |w: u32| match w {
            0x10 | 0x11 => Some((true, false)), // managed app windows
            0x20 => Some((true, true)),         // notification / other overlay
            0x30 => Some((false, false)),       // unmapped
            _ => None,                          // gone
        };
        // Below us only.
        assert!(!covered(&[0x10, 0x11, WIN, POPUP], WIN, &own, attrs));
        // An app went above the notification that is above us.
        assert!(covered(&[0x10, WIN, 0x20, 0x11], WIN, &own, attrs));
        // Only override-redirect, unmapped or vanished windows above.
        assert!(!covered(&[0x10, WIN, 0x20, 0x30, 0x40], WIN, &own, attrs));
        // Not in the tree (not mapped yet): nothing to do.
        assert!(!covered(&[0x10, 0x11], WIN, &own, attrs));
    }

    #[test]
    #[ignore = "needs an X display (DISPLAY=:0)"]
    fn smoke_overlay_maps() {
        use crate::config::Config;
        use crate::render::{View, render};
        use crate::text::FontSet;
        use crate::themes::{Rgb, get_theme};

        let x = X::connect().unwrap();
        let cfg = Config::default();
        let view = View {
            cfg: &cfg,
            theme: get_theme("classic"),
            scale: 1.0,
            clock: "12:34:56".into(),
            timer: "05:00".into(),
            timer_color: Rgb(255, 255, 255),
            timer_px_mul: 1.0,
            mode_text: "COUNTDOWN  5:00".into(),
            mode_color: Rgb(102, 102, 102),
            show_controls_row: true,
            controls_pos: 1.0,
            running: false,
            mode_btn: "CD",
            hover_btn: None,
            progress: -1.0,
            progress_color: Rgb(255, 0, 0),
            opacity: 1.0,
        };
        let (pm, _) = render(&view, &mut FontSet::bundled()).unwrap();
        let win = OverlayWin::create(&x, 200, 100, 40, 40).unwrap();
        win.map(&x);
        win.raise(&x);
        x.present(win.id, win.gc, &pm).unwrap();
        win.set_click_through(&x, true);
        win.set_click_through(&x, false);
        win.set_geometry(&x, 60, 60, 200, 100);
        let attrs = x
            .conn
            .get_window_attributes(win.id)
            .unwrap()
            .reply()
            .unwrap();
        assert!(attrs.override_redirect);
        let geo = x.conn.get_geometry(win.id).unwrap().reply().unwrap();
        assert_eq!((geo.width, geo.height, geo.depth), (200, 100, 32));
        std::thread::sleep(std::time::Duration::from_millis(300));
        win.unmap(&x);
        x.destroy_window(win.id, win.gc);
        x.conn.flush().unwrap();
        assert!(x.scale() > 0.0);
        let area = x.screens().work_area_at(0, 0);
        assert!(area.w > 0 && area.h > 0, "{area:?}");
    }
}
