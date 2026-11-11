//! Context menu as a stack of override-redirect popup windows with an active
//! pointer + keyboard grab.
//!
//! Grab safety: `handle` releases the grabs and destroys the windows itself
//! before returning `Closed` or `Chosen`. A popup that is still open must be
//! closed with [`MenuPopup::close`]; `Drop` cannot reach the connection, so
//! dropping an open popup leaves the grabs in place until the process exits.
//! It logs an error (and fails debug builds) to catch that bug early. When
//! the connection itself is gone, [`MenuPopup::abandon`] forgets the popup.

use super::overlay::raise_window;
use super::{X, keysym_of};
use crate::layout::{Rect, clamp_into};
use crate::menu::{MenuAction, MenuItem, MenuLayout, layout_menu, render_menu};
use crate::text::FontSet;
use crate::themes::Theme;
use anyhow::bail;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, EventMask, GrabMode, GrabStatus};

pub enum PopupOutcome {
    Continue,
    Closed,
    Chosen(MenuAction),
}

struct Level {
    win: u32,
    gc: u32,
    /// Submenu indices from the root items to this level's items.
    path: Vec<usize>,
    layout: MenuLayout,
    /// Hovered item index (into this level's items).
    hover: Option<usize>,
    x: i32,
    y: i32,
}

impl Level {
    fn rect(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.layout.w,
            h: self.layout.h,
        }
    }
}

pub struct MenuPopup {
    items: Vec<MenuItem>,
    theme: &'static Theme,
    scale: f32,
    area: Rect,
    levels: Vec<Level>,
    grabbed: bool,
}

fn items_at<'a>(mut items: &'a [MenuItem], path: &[usize]) -> &'a [MenuItem] {
    for &i in path {
        match items.get(i) {
            Some(MenuItem::Submenu { items: sub, .. }) => items = sub,
            _ => return &[],
        }
    }
    items
}

fn selectable(item: &MenuItem) -> bool {
    match item {
        MenuItem::Action { enabled, .. } => *enabled,
        MenuItem::Submenu { items, .. } => !items.is_empty(),
        MenuItem::Separator => false,
    }
}

fn is_submenu(item: Option<&MenuItem>) -> bool {
    matches!(item, Some(MenuItem::Submenu { items, .. }) if !items.is_empty())
}

const KEY_ESCAPE: u32 = 0xff1b;
const KEY_RETURN: u32 = 0xff0d;
const KEY_KP_ENTER: u32 = 0xff8d;
const KEY_LEFT: u32 = 0xff51;
const KEY_UP: u32 = 0xff52;
const KEY_RIGHT: u32 = 0xff53;
const KEY_DOWN: u32 = 0xff54;

/// Another client's grab (e.g. the WM's) may still be active right after the
/// triggering click; retry briefly.
const GRAB_TRIES: u32 = 20;
const GRAB_RETRY: Duration = Duration::from_millis(10);

impl MenuPopup {
    /// Opens the menu at the pointer position (kept on-screen) and grabs the
    /// pointer (owner events) and keyboard. On failure nothing stays grabbed
    /// or mapped.
    pub fn open(
        x: &X,
        items: Vec<MenuItem>,
        root_x: i32,
        root_y: i32,
        theme: &'static Theme,
        fonts: &mut FontSet,
    ) -> anyhow::Result<Self> {
        let mut p = MenuPopup {
            items,
            theme,
            scale: x.scale(),
            area: x.work_area_at(root_x, root_y),
            levels: Vec::new(),
            grabbed: false,
        };
        let result = p
            .push_level(x, Vec::new(), (root_x, root_y), fonts)
            .and_then(|()| p.grab(x));
        match result {
            Ok(()) => Ok(p),
            Err(e) => {
                p.release(x);
                Err(e)
            }
        }
    }

    /// All popup windows (for [`super::overlay::restack_needed`]).
    pub fn windows(&self) -> Vec<u32> {
        self.levels.iter().map(|l| l.win).collect()
    }

    /// Raises every level, parents first. Call after raising the overlay.
    pub fn raise(&self, x: &X) {
        for l in &self.levels {
            raise_window(x, l.win);
        }
    }

    pub fn handle(&mut self, x: &X, ev: &Event, fonts: &mut FontSet) -> PopupOutcome {
        match ev {
            Event::Expose(e) if e.count == 0 => {
                if let Some(i) = self.levels.iter().position(|l| l.win == e.window) {
                    self.redraw(x, i, fonts);
                }
                PopupOutcome::Continue
            }
            Event::MotionNotify(e) => {
                if let Some((li, Some(idx))) = self.hit(e.root_x.into(), e.root_y.into()) {
                    let item = items_at(&self.items, &self.levels[li].path).get(idx);
                    let hover = item.filter(|i| selectable(i)).map(|_| idx);
                    self.set_hover(x, li, hover, true, fonts);
                }
                PopupOutcome::Continue
            }
            Event::ButtonPress(e) if (1..=3).contains(&e.detail) => {
                match self.hit(e.root_x.into(), e.root_y.into()) {
                    None => self.finish(x, PopupOutcome::Closed),
                    Some((li, Some(idx))) => self.activate(x, li, idx, fonts),
                    Some((_, None)) => PopupOutcome::Continue,
                }
            }
            Event::KeyPress(e) => self.key(x, keysym_of(e.detail), fonts),
            _ => PopupOutcome::Continue,
        }
    }

    /// Releases the grabs and destroys all popup windows.
    pub fn close(mut self, x: &X) {
        self.release(x);
    }

    /// Forgets an open popup without talking to X, for when the connection
    /// is gone: the server released its grabs and windows with it.
    pub fn abandon(mut self) {
        self.grabbed = false;
        self.levels.clear();
    }

    fn key(&mut self, x: &X, keysym: u32, fonts: &mut FontSet) -> PopupOutcome {
        let Some(li) = self.levels.len().checked_sub(1) else {
            return PopupOutcome::Continue;
        };
        let items = items_at(&self.items, &self.levels[li].path);
        let hover = self.levels[li].hover;
        match keysym {
            KEY_ESCAPE => return self.finish(x, PopupOutcome::Closed),
            KEY_UP | KEY_DOWN if !items.is_empty() => {
                let n = items.len();
                let start = hover.unwrap_or(if keysym == KEY_DOWN { n - 1 } else { 0 });
                let next = (1..=n)
                    .map(|k| {
                        if keysym == KEY_DOWN {
                            (start + k) % n
                        } else {
                            (start + n - k) % n
                        }
                    })
                    .find(|&i| selectable(&items[i]));
                if next.is_some() {
                    self.set_hover(x, li, next, false, fonts);
                }
            }
            KEY_RETURN | KEY_KP_ENTER | KEY_RIGHT => {
                if let Some(idx) = hover
                    && (keysym != KEY_RIGHT || is_submenu(items.get(idx)))
                {
                    return self.activate(x, li, idx, fonts);
                }
            }
            KEY_LEFT if li > 0 => self.truncate(x, li),
            _ => {}
        }
        PopupOutcome::Continue
    }

    /// Click / Return on item `idx` of level `li`.
    fn activate(&mut self, x: &X, li: usize, idx: usize, fonts: &mut FontSet) -> PopupOutcome {
        match items_at(&self.items, &self.levels[li].path).get(idx) {
            Some(MenuItem::Action {
                action,
                enabled: true,
                ..
            }) => {
                let action = action.clone();
                self.finish(x, PopupOutcome::Chosen(action))
            }
            Some(item) if is_submenu(Some(item)) => {
                self.set_hover(x, li, Some(idx), true, fonts);
                // Keyboard users land on the child's first entry.
                if let Some(child) = self.levels.get(li + 1)
                    && child.hover.is_none()
                {
                    let first = items_at(&self.items, &child.path)
                        .iter()
                        .position(selectable);
                    self.set_hover(x, li + 1, first, false, fonts);
                }
                PopupOutcome::Continue
            }
            _ => PopupOutcome::Continue,
        }
    }

    fn finish(&mut self, x: &X, outcome: PopupOutcome) -> PopupOutcome {
        self.release(x);
        outcome
    }

    /// Topmost level containing the root point, and the item row under it.
    fn hit(&self, rx: i32, ry: i32) -> Option<(usize, Option<usize>)> {
        self.levels.iter().enumerate().rev().find_map(|(li, l)| {
            let r = l.rect();
            if rx < r.x || rx >= r.x + r.w || ry < r.y || ry >= r.y + r.h {
                return None;
            }
            let (lx, ly) = (rx - l.x, ry - l.y);
            let row =
                l.layout.rows.iter().find(|(rr, _)| {
                    lx >= rr.x && lx < rr.x + rr.w && ly >= rr.y && ly < rr.y + rr.h
                });
            Some((li, row.and_then(|(_, idx)| *idx)))
        })
    }

    /// Sets level `li`'s hover, closing deeper levels; opens the hovered
    /// submenu if `open_child`.
    fn set_hover(
        &mut self,
        x: &X,
        li: usize,
        hover: Option<usize>,
        open_child: bool,
        fonts: &mut FontSet,
    ) {
        let child_wanted = open_child
            && hover
                .is_some_and(|i| is_submenu(items_at(&self.items, &self.levels[li].path).get(i)));
        let child_open = self.levels.len() > li + 1;
        if self.levels[li].hover == hover && (child_open || !child_wanted) {
            return;
        }
        self.truncate(x, li + 1);
        self.levels[li].hover = hover;
        self.redraw(x, li, fonts);
        if let (true, Some(idx)) = (child_wanted, hover) {
            let l = &self.levels[li];
            let row = l.layout.rows.iter().find(|(_, i)| *i == Some(idx));
            let row_y = row.map_or(0, |(r, _)| r.y);
            let first_row_y = l.layout.rows.first().map_or(0, |(r, _)| r.y);
            let mut path = l.path.clone();
            path.push(idx);
            // Align the child's first row with the hovered row.
            let y = l.y + row_y - first_row_y;
            let parent = l.rect();
            if let Err(e) = self.push_child(x, path, parent, y, fonts) {
                eprintln!("timehud: cannot open submenu: {e:#}");
            }
        }
    }

    fn push_child(
        &mut self,
        x: &X,
        path: Vec<usize>,
        parent: Rect,
        y: i32,
        fonts: &mut FontSet,
    ) -> anyhow::Result<()> {
        let w = layout_menu(items_at(&self.items, &path), fonts, self.scale).w;
        let right = parent.x + parent.w;
        let px = if right + w > self.area.x + self.area.w {
            parent.x - w
        } else {
            right
        };
        self.push_level(x, path, (px, y), fonts)
    }

    /// Creates, maps and draws a level at `pos` (clamped into the work area).
    fn push_level(
        &mut self,
        x: &X,
        path: Vec<usize>,
        pos: (i32, i32),
        fonts: &mut FontSet,
    ) -> anyhow::Result<()> {
        let layout = layout_menu(items_at(&self.items, &path), fonts, self.scale);
        let (px, py) = clamp_into(pos.0, pos.1, layout.w, layout.h, self.area);
        let events = EventMask::EXPOSURE
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::KEY_PRESS;
        let (win, gc) = x.create_window(
            Rect {
                x: px,
                y: py,
                w: layout.w,
                h: layout.h,
            },
            events,
            "TimeHUD menu",
            x.atoms._NET_WM_WINDOW_TYPE_POPUP_MENU,
            true,
        )?;
        x.conn.map_window(win)?;
        self.levels.push(Level {
            win,
            gc,
            path,
            layout,
            hover: None,
            x: px,
            y: py,
        });
        self.redraw(x, self.levels.len() - 1, fonts);
        Ok(())
    }

    fn redraw(&self, x: &X, li: usize, fonts: &mut FontSet) {
        let l = &self.levels[li];
        let items = items_at(&self.items, &l.path);
        let pm = render_menu(items, &l.layout, l.hover, self.theme, fonts, self.scale);
        if let Err(e) = x.present(l.win, l.gc, &pm) {
            eprintln!("timehud: menu redraw failed: {e:#}");
        }
    }

    /// Destroys levels `from..`.
    fn truncate(&mut self, x: &X, from: usize) {
        for l in self.levels.drain(from.min(self.levels.len())..) {
            x.destroy_window(l.win, l.gc);
        }
        let _ = x.conn.flush();
    }

    fn grab(&mut self, x: &X) -> anyhow::Result<()> {
        let win = self.levels[0].win;
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let mut pointer = false;
        for _ in 0..GRAB_TRIES {
            if !pointer {
                let r = x
                    .conn
                    .grab_pointer(
                        true,
                        win,
                        mask,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                        x11rb::NONE,
                        x11rb::NONE,
                        x11rb::CURRENT_TIME,
                    )?
                    .reply()?;
                pointer = r.status == GrabStatus::SUCCESS;
                // Mark as soon as anything is held so `release` undoes it.
                self.grabbed |= pointer;
            }
            if pointer {
                let r = x
                    .conn
                    .grab_keyboard(
                        true,
                        win,
                        x11rb::CURRENT_TIME,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                    )?
                    .reply()?;
                if r.status == GrabStatus::SUCCESS {
                    return Ok(());
                }
            }
            std::thread::sleep(GRAB_RETRY);
        }
        bail!("cannot grab the pointer/keyboard for the menu (another client holds a grab)")
    }

    /// Ungrabs and destroys every window. Idempotent.
    fn release(&mut self, x: &X) {
        if self.grabbed {
            let _ = x.conn.ungrab_pointer(x11rb::CURRENT_TIME);
            let _ = x.conn.ungrab_keyboard(x11rb::CURRENT_TIME);
            self.grabbed = false;
        }
        self.truncate(x, 0);
    }
}

impl Drop for MenuPopup {
    fn drop(&mut self) {
        if (self.grabbed || !self.levels.is_empty()) && !std::thread::panicking() {
            eprintln!("timehud: BUG: MenuPopup dropped without close(); grabs leaked");
            debug_assert!(false, "MenuPopup dropped without close()");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MenuAction;

    #[test]
    fn abandon_forgets_an_open_popup_quietly() {
        // Drop would report leaked grabs (and fail this debug build).
        let p = MenuPopup {
            items: Vec::new(),
            theme: crate::themes::get_theme("classic"),
            scale: 1.0,
            area: Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 100,
            },
            levels: Vec::new(),
            grabbed: true,
        };
        p.abandon();
    }

    fn act(label: &str, enabled: bool) -> MenuItem {
        MenuItem::Action {
            label: label.into(),
            action: MenuAction::Quit,
            checked: None,
            enabled,
        }
    }

    #[test]
    fn items_at_walks_submenus() {
        let items = vec![
            act("a", true),
            MenuItem::Submenu {
                label: "s".into(),
                items: vec![act("b", true), act("c", false)],
            },
        ];
        assert_eq!(items_at(&items, &[]).len(), 2);
        assert_eq!(items_at(&items, &[1]).len(), 2);
        assert!(items_at(&items, &[0]).is_empty());
        assert!(items_at(&items, &[7]).is_empty());
        assert!(selectable(&items_at(&items, &[1])[0]));
        assert!(!selectable(&items_at(&items, &[1])[1]));
        assert!(!selectable(&MenuItem::Separator));
    }

    #[test]
    #[ignore = "needs an X display (DISPLAY=:0)"]
    fn smoke_popup_opens_and_releases_grabs() {
        use crate::themes::get_theme;
        use x11rb::protocol::xproto::ConnectionExt as _;
        use x11rb::wrapper::ConnectionExt as _;

        let x = X::connect().unwrap();
        let mut fonts = FontSet::bundled();
        let items = vec![
            act("One", true),
            MenuItem::Separator,
            MenuItem::Submenu {
                label: "More".into(),
                items: vec![act("Two", true)],
            },
        ];
        let mut p = MenuPopup::open(&x, items, 100, 100, get_theme("classic"), &mut fonts).unwrap();
        // Keyboard navigation opens the submenu as a second level.
        p.key(&x, KEY_DOWN, &mut fonts);
        p.key(&x, KEY_DOWN, &mut fonts);
        assert!(matches!(
            p.key(&x, KEY_RIGHT, &mut fonts),
            PopupOutcome::Continue
        ));
        assert_eq!(p.windows().len(), 2);
        assert!(matches!(
            p.key(&x, KEY_RETURN, &mut fonts),
            PopupOutcome::Chosen(MenuAction::Quit)
        ));
        // `handle`-style finish already released everything.
        assert!(p.windows().is_empty());
        p.close(&x);

        // Another client can grab the pointer now, so ours is gone.
        let other = X::connect().unwrap();
        let r = other
            .conn
            .grab_pointer(
                false,
                other.root,
                EventMask::NO_EVENT,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                x11rb::NONE,
                x11rb::NONE,
                x11rb::CURRENT_TIME,
            )
            .unwrap()
            .reply()
            .unwrap();
        other.conn.ungrab_pointer(x11rb::CURRENT_TIME).unwrap();
        other.conn.sync().unwrap();
        assert_eq!(r.status, GrabStatus::SUCCESS);

        // Escape closes and releases too.
        let mut p = MenuPopup::open(
            &x,
            vec![act("A", true)],
            50,
            50,
            get_theme("classic"),
            &mut fonts,
        )
        .unwrap();
        assert!(matches!(
            p.key(&x, KEY_ESCAPE, &mut fonts),
            PopupOutcome::Closed
        ));
        assert!(p.windows().is_empty());
        p.close(&x);
        x.conn.sync().unwrap();
    }
}
