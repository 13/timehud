//! The tray icon's tooltip: XEmbed has no tooltip protocol, so the HUD shows
//! its own small override-redirect window near the icon.

use super::{AlphaShape, X};
use crate::layout::{Rect, tooltip_pos};
use crate::menu::{render_tooltip, tooltip_size};
use crate::text::FontSet;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, EventMask};

/// Pointer rest time over the tray icon before the tooltip shows.
pub const TOOLTIP_DELAY: f64 = 0.6;
/// Distance between the icon and the tooltip.
const GAP: f32 = 4.0;

pub struct Tooltip {
    win: u32,
    gc: u32,
    text: String,
    /// The window's current geometry.
    rect: Rect,
    anchor: Rect,
    area: Rect,
    /// No compositor: the window is clipped to its rounded panel.
    shape: Option<AlphaShape>,
}

impl Tooltip {
    /// Maps a tooltip saying `text` next to `anchor` (the icon, root
    /// coordinates), inside `area`. It is drawn on its `Expose`. `shaped`
    /// (no compositor): clip it to its rounded panel, as window alpha is
    /// ignored then.
    pub fn open(
        x: &X,
        text: &str,
        anchor: Rect,
        area: Rect,
        shaped: bool,
        fonts: &mut FontSet,
    ) -> anyhow::Result<Tooltip> {
        let rect = place(x, text, anchor, area, fonts);
        let (win, gc) = x.create_window(
            rect,
            EventMask::EXPOSURE,
            "TimeHUD tooltip",
            x.atoms._NET_WM_WINDOW_TYPE_TOOLTIP,
            true,
        )?;
        let mut t = Tooltip {
            win,
            gc,
            text: text.to_string(),
            rect,
            anchor,
            area,
            shape: shaped.then(AlphaShape::default),
        };
        t.reshape(x, fonts);
        x.conn.map_window(win)?;
        Ok(t)
    }

    pub fn window(&self) -> u32 {
        self.win
    }

    /// Shows new text (re-placed, resized if needed); a no-op if it is
    /// unchanged. A resize clears the window and brings an `Expose` (which
    /// redraws), so only a same-size update draws right away.
    pub fn set_text(&mut self, x: &X, text: &str, fonts: &mut FontSet) {
        if text == self.text {
            return;
        }
        text.clone_into(&mut self.text);
        let r = place(x, text, self.anchor, self.area, fonts);
        let old = std::mem::replace(&mut self.rect, r);
        let resized = (r.w, r.h) != (old.w, old.h);
        if r != old {
            let mut aux = ConfigureWindowAux::new().x(r.x).y(r.y);
            if resized {
                aux = aux.width(r.w.max(1) as u32).height(r.h.max(1) as u32);
            }
            let _ = x.conn.configure_window(self.win, &aux);
        }
        if resized {
            self.reshape(x, fonts);
        } else {
            self.redraw(x, fonts);
        }
    }

    /// Re-clips a shaped tooltip to its panel (the outline only depends on
    /// the size).
    fn reshape(&mut self, x: &X, fonts: &mut FontSet) {
        if let Some(shape) = &mut self.shape {
            shape.set(x, self.win, &render_tooltip(&self.text, fonts, x.scale()));
        }
    }

    pub fn redraw(&self, x: &X, fonts: &mut FontSet) {
        let pm = render_tooltip(&self.text, fonts, x.scale());
        if let Err(e) = x.present(self.win, self.gc, &pm) {
            eprintln!("timehud: tooltip redraw failed: {e:#}");
        }
    }

    pub fn close(self, x: &X) {
        x.destroy_window(self.win, self.gc);
        let _ = x.conn.flush();
    }
}

fn place(x: &X, text: &str, anchor: Rect, area: Rect, fonts: &mut FontSet) -> Rect {
    let scale = x.scale();
    let (w, h) = tooltip_size(text, fonts, scale);
    let gap = (GAP * scale).round() as i32;
    let (px, py) = tooltip_pos(anchor, w, h, area, gap);
    Rect { x: px, y: py, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs an X display (DISPLAY=:0)"]
    fn smoke_shaped_tooltip_follows_its_size() {
        let x = X::connect().unwrap();
        let mut fonts = FontSet::bundled();
        let icon = Rect {
            x: 100,
            y: 100,
            w: 24,
            h: 24,
        };
        let area = x.screens().work_area_at(100, 100);
        let mut t = Tooltip::open(&x, "TimeHUD", icon, area, true, &mut fonts).unwrap();
        assert!(super::super::corners_cut(&x, t.window()));
        t.set_text(&x, "TimeHUD: countdown running, 24:59 left", &mut fonts);
        assert!(
            super::super::corners_cut(&x, t.window()),
            "reshaped on resize"
        );
        t.close(&x);
        let t = Tooltip::open(&x, "TimeHUD", icon, area, false, &mut fonts).unwrap();
        assert!(!super::super::corners_cut(&x, t.window()));
        t.close(&x);
    }
}
