//! The HUD's override-redirect ARGB overlay window and its input translation.

use super::{X, to_i16, to_u16};
use crate::app::{Button, Cursor, Input, Key};
use crate::layout::Rect;
use std::collections::HashMap;
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, SK, SO};
use x11rb::protocol::xproto::{
    BackPixmap, ChangeWindowAttributesAux, ClipOrdering, ConfigureWindowAux, ConnectionExt as _,
    EventMask, KeyButMask, MapState, Rectangle, StackMode,
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
    /// The bounding shape set from the frame's alpha (no compositor):
    /// size and mask, to skip unchanged updates. `None` = unshaped.
    alpha_shape: Option<((u32, u32), Vec<u8>)>,
    /// Click-through wanted, and the panel rect (window coordinates) last
    /// laid out; together they give the input shape ([`input_rects`]).
    click_through: bool,
    panel: Option<Rect>,
    /// The input shape last sent, as (click-through, panel).
    input_sent: Option<(bool, Rect)>,
}

/// `XC_hand2` in the core "cursor" font; its mask is the next glyph.
const XC_HAND2: u16 = 60;

impl OverlayWin {
    /// Creates the (unmapped) overlay: override-redirect, ARGB, `WM_CLASS`
    /// "timehud", `_NET_WM_NAME` "TimeHUD", `_COMPTON_SHADOW = 0`, type
    /// `_NET_WM_WINDOW_TYPE_NOTIFICATION`. No background: the server does
    /// not clear it on resize (the next frame covers it), so no flicker.
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
        let aux = ChangeWindowAttributesAux::new().background_pixmap(BackPixmap::NONE);
        x.conn.change_window_attributes(id, &aux)?;
        x.conn.flush()?;
        Ok(OverlayWin {
            id,
            gc,
            left_held: false,
            hand: None,
            alpha_shape: None,
            click_through: false,
            panel: None,
            input_sent: None,
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
    /// `off`: input only over the panel (see [`OverlayWin::set_panel`]).
    pub fn set_click_through(&mut self, x: &X, on: bool) {
        self.click_through = on;
        self.sync_input(x);
    }

    /// The panel rect of the current layout (window coordinates): the
    /// input shape, so the transparent shadow margin around it does not
    /// take clicks meant for the windows below. Call on every layout.
    pub fn set_panel(&mut self, x: &X, panel: Rect) {
        self.panel = Some(panel);
        self.sync_input(x);
    }

    /// Sends the input shape if it changed (and a panel is known).
    fn sync_input(&mut self, x: &X) {
        let Some(panel) = self.panel else { return };
        let want = (self.click_through, panel);
        if self.input_sent == Some(want) {
            return;
        }
        self.input_sent = Some(want);
        let rects = input_rects(self.click_through, panel);
        let _ = shape::rectangles(
            &x.conn,
            SO::SET,
            SK::INPUT,
            ClipOrdering::UNSORTED,
            self.id,
            0,
            0,
            &rects,
        );
        let _ = x.conn.flush();
    }

    /// Without a compositor window alpha is ignored: clips the window to
    /// the pixels of `pm` (the frame drawn into it) with alpha >= 128, so
    /// the transparent corners do not show black. Skips the request when
    /// the mask did not change.
    pub fn set_alpha_shape(&mut self, x: &X, pm: &tiny_skia::Pixmap) {
        let size = (pm.width(), pm.height());
        let mask = x.alpha_mask(pm);
        if self
            .alpha_shape
            .as_ref()
            .is_some_and(|(s, m)| *s == size && *m == mask)
        {
            return;
        }
        let Ok(bitmap) = x.bitmap(self.id, &mask, size.0, size.1) else {
            return;
        };
        let _ = shape::mask(&x.conn, SO::SET, SK::BOUNDING, self.id, 0, 0, bitmap);
        let _ = x.conn.free_pixmap(bitmap);
        let _ = x.conn.flush();
        self.alpha_shape = Some((size, mask));
    }

    /// Removes the shape set by [`OverlayWin::set_alpha_shape`].
    pub fn clear_alpha_shape(&mut self, x: &X) {
        if self.alpha_shape.take().is_some() {
            let _ = shape::mask(&x.conn, SO::SET, SK::BOUNDING, self.id, 0, 0, NONE);
            let _ = x.conn.flush();
        }
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

/// Whether a compositing manager runs, i.e. owns `_NET_WM_CM_S<screen>`.
/// Without one, ARGB windows are shown opaque (alpha ignored), so the HUD
/// drops its shadow and shapes the window instead.
///
/// The owner is read at startup and re-read when [`CompositorWatch::recheck`]
/// says so: a `MANAGER` announcement, the owner window's destruction (its
/// `StructureNotify` is selected), or, while there is no compositor, a new
/// override-redirect top-level (a starting compositor creates one before it
/// takes the selection, hence a short delay).
#[derive(Debug, Default)]
pub struct CompositorWatch {
    owner: u32,
}

/// When to re-read the compositor selection owner.
#[derive(Debug, PartialEq)]
pub enum Recheck {
    Now,
    Soon,
}

/// Delay of a [`Recheck::Soon`], in seconds.
pub const CM_RECHECK_DELAY: f64 = 0.5;

impl CompositorWatch {
    pub fn new(x: &X) -> CompositorWatch {
        let mut w = CompositorWatch::default();
        w.refresh(x);
        w
    }

    pub fn active(&self) -> bool {
        self.owner != 0
    }

    /// Re-reads the selection owner (and watches it); true if
    /// [`CompositorWatch::active`] changed.
    pub fn refresh(&mut self, x: &X) -> bool {
        let was = self.active();
        self.owner = x.selection_owner(x.cm_selection);
        if self.owner != 0 {
            let aux = ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY);
            let watched = x
                .conn
                .change_window_attributes(self.owner, &aux)
                .ok()
                .is_some_and(|c| c.check().is_ok());
            if !watched {
                self.owner = 0; // gone already
            }
        }
        was != self.active()
    }

    /// Whether `ev` may mean the compositor started or stopped.
    pub fn recheck(&self, ev: &Event, root: u32, selection: u32, manager: u32) -> Option<Recheck> {
        match ev {
            Event::ClientMessage(e)
                if e.window == root
                    && e.type_ == manager
                    && e.format == 32
                    && e.data.as_data32()[1] == selection =>
            {
                Some(Recheck::Now)
            }
            Event::DestroyNotify(e) if self.owner != 0 && e.window == self.owner => {
                Some(Recheck::Now)
            }
            Event::CreateNotify(e)
                if self.owner == 0 && e.parent == root && e.override_redirect =>
            {
                Some(Recheck::Soon)
            }
            _ => None,
        }
    }
}

/// The overlay's input shape: nothing while click-through, else the panel.
fn input_rects(click_through: bool, panel: Rect) -> Vec<Rectangle> {
    if click_through {
        return Vec::new();
    }
    vec![Rectangle {
        x: to_i16(panel.x),
        y: to_i16(panel.y),
        width: to_u16(panel.w),
        height: to_u16(panel.h),
    }]
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
                Some(Input::Release { button })
            }
            Event::MotionNotify(e) if e.event == win => {
                if !e.state.contains(KeyButMask::BUTTON1) && std::mem::take(&mut self.left_held) {
                    return Some(Input::Release {
                        button: Button::Left,
                    });
                }
                let (root_x, root_y) = (e.root_x.into(), e.root_y.into());
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
            alpha_shape: None,
            click_through: false,
            panel: None,
            input_sent: None,
        }
    }

    #[test]
    fn input_shape_is_the_panel_unless_click_through() {
        let panel = Rect {
            x: 12,
            y: 12,
            w: 200,
            h: 90,
        };
        let fields = |r: &Rectangle| (r.x, r.y, r.width, r.height);
        let got: Vec<_> = input_rects(false, panel).iter().map(fields).collect();
        assert_eq!(got, [(12, 12, 200, 90)]);
        assert!(input_rects(true, panel).is_empty());
        let odd = Rect {
            x: -5,
            y: 0,
            w: -1,
            h: 100_000,
        };
        let got = input_rects(false, odd)[0];
        assert_eq!((got.x, got.width, got.height), (-5, 0, u16::MAX));
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
    fn compositor_rechecks() {
        use x11rb::protocol::xproto::{ClientMessageEvent, CreateNotifyEvent, DestroyNotifyEvent};
        const SEL: u32 = 0x1a0;
        const MANAGER: u32 = 0x1b0;
        let manager = |sel: u32| {
            Event::ClientMessage(ClientMessageEvent::new(
                32,
                ROOT,
                MANAGER,
                [0, sel, 0x77, 0, 0],
            ))
        };
        let create = |or: bool| {
            Event::CreateNotify(CreateNotifyEvent {
                response_type: 16,
                sequence: 0,
                parent: ROOT,
                window: 0x77,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                border_width: 0,
                override_redirect: or,
            })
        };
        let destroy = |window| {
            Event::DestroyNotify(DestroyNotifyEvent {
                response_type: 17,
                sequence: 0,
                event: window,
                window,
            })
        };
        let none = CompositorWatch { owner: 0 };
        let on = CompositorWatch { owner: 0x55 };
        let check = |w: &CompositorWatch, ev: &Event| w.recheck(ev, ROOT, SEL, MANAGER);
        // Announced: re-read now, whatever the state.
        assert_eq!(check(&none, &manager(SEL)), Some(Recheck::Now));
        assert_eq!(check(&on, &manager(SEL)), Some(Recheck::Now));
        // Another selection's manager (e.g. the tray).
        assert_eq!(check(&none, &manager(SEL + 1)), None);
        // The compositor went away.
        assert_eq!(check(&on, &destroy(0x55)), Some(Recheck::Now));
        assert_eq!(check(&on, &destroy(0x66)), None);
        assert_eq!(check(&none, &destroy(0x55)), None);
        // An override-redirect window appeared while there is none.
        assert_eq!(check(&none, &create(true)), Some(Recheck::Soon));
        assert_eq!(check(&none, &create(false)), None);
        assert_eq!(check(&on, &create(true)), None);
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
        eprintln!("compositor: {}", CompositorWatch::new(&x).active());
        let mut win = OverlayWin::create(&x, 200, 100, 40, 40).unwrap();
        win.map(&x);
        win.raise(&x);
        x.present(win.id, win.gc, &pm).unwrap();
        let id = win.id;
        let shaped = |x: &X| {
            shape::query_extents(&x.conn, id)
                .unwrap()
                .reply()
                .unwrap()
                .bounding_shaped
        };
        win.set_alpha_shape(&x, &pm);
        assert!(shaped(&x));
        win.clear_alpha_shape(&x);
        assert!(!shaped(&x));
        win.set_click_through(&x, true);
        win.set_panel(
            &x,
            Rect {
                x: 12,
                y: 12,
                w: 100,
                h: 50,
            },
        );
        win.set_click_through(&x, false);
        let input = shape::get_rectangles(&x.conn, win.id, SK::INPUT)
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(input.rectangles.len(), 1);
        assert_eq!(input.rectangles[0].width, 100);
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
