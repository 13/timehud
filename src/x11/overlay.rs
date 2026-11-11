//! The HUD's override-redirect ARGB overlay window and its input translation.

use super::{X, keysym_of};
use crate::app::{Button, Input, Key};
use crate::layout::Rect;
use std::cell::Cell;
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, SK, SO};
use x11rb::protocol::xproto::{
    ClipOrdering, ConfigureWindowAux, ConnectionExt as _, EventMask, KeyButMask, StackMode,
};

pub struct OverlayWin {
    pub id: u32,
    pub gc: u32,
}

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
        Ok(OverlayWin { id, gc })
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

thread_local! {
    /// Button 1 pressed on the overlay and not yet released.
    static LEFT_HELD: Cell<bool> = const { Cell::new(false) };
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

/// Translates an event for overlay window `win` into an app input.
///
/// Motion is always emitted (the app needs it for button hover). To keep a
/// lost ButtonRelease from leaving the window stuck dragging, a motion event
/// whose state lacks `Button1Mask` while a left press is outstanding is
/// turned into a synthetic `Release { button: Left }` instead (that one
/// motion's hover update is dropped; the next motion restores it).
pub fn translate(ev: &Event, win: u32) -> Option<Input> {
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
                        LEFT_HELD.set(true);
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
                LEFT_HELD.set(false);
            }
            Some(Input::Release {
                root_x: e.root_x.into(),
                root_y: e.root_y.into(),
                button,
            })
        }
        Event::MotionNotify(e) if e.event == win => {
            let (root_x, root_y) = (e.root_x.into(), e.root_y.into());
            if !e.state.contains(KeyButMask::BUTTON1) && LEFT_HELD.replace(false) {
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
            key: key(keysym_of(e.detail)),
            ctrl: e.state.contains(KeyButMask::CONTROL),
        }),
        _ => None,
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
        assert_eq!(
            translate(&Event::ButtonPress(press(3)), WIN),
            Some(Input::Press {
                x: 10,
                y: 20,
                root_x: 110,
                root_y: 220,
                button: Button::Right
            })
        );
        assert_eq!(
            translate(&Event::ButtonPress(press(4)), WIN),
            Some(Input::Wheel {
                x: 10,
                y: 20,
                up: true
            })
        );
        assert_eq!(
            translate(&Event::ButtonPress(press(5)), WIN),
            Some(Input::Wheel {
                x: 10,
                y: 20,
                up: false
            })
        );
        assert_eq!(translate(&Event::ButtonPress(press(8)), WIN), None);
        // Wheel "releases" are dropped.
        assert_eq!(translate(&Event::ButtonRelease(press(4)), WIN), None);
        // Other windows' events are ignored.
        assert_eq!(translate(&Event::ButtonPress(press(1)), WIN + 1), None);
    }

    #[test]
    fn motion_without_button_is_hover() {
        LEFT_HELD.set(false);
        assert_eq!(
            translate(&motion(KeyButMask::default()), WIN),
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
        LEFT_HELD.set(false);
        translate(&Event::ButtonPress(press(1)), WIN);
        assert!(matches!(
            translate(&motion(KeyButMask::BUTTON1), WIN),
            Some(Input::Motion { .. })
        ));
        assert!(matches!(
            translate(&Event::ButtonRelease(press(1)), WIN),
            Some(Input::Release {
                button: Button::Left,
                ..
            })
        ));
        // Released normally: later plain motion is hover again.
        assert!(matches!(
            translate(&motion(KeyButMask::default()), WIN),
            Some(Input::Motion { .. })
        ));
    }

    #[test]
    fn lost_release_ends_drag() {
        LEFT_HELD.set(false);
        translate(&Event::ButtonPress(press(1)), WIN);
        // The release went elsewhere; the first button-less motion ends the drag.
        assert_eq!(
            translate(&motion(KeyButMask::default()), WIN),
            Some(Input::Release {
                root_x: 130,
                root_y: 240,
                button: Button::Left
            })
        );
        assert!(matches!(
            translate(&motion(KeyButMask::default()), WIN),
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
