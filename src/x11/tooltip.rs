//! The tray icon's tooltip: XEmbed has no tooltip protocol, so the HUD shows
//! its own small override-redirect window near the icon.

use super::X;
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
    anchor: Rect,
    area: Rect,
}

impl Tooltip {
    /// Maps a tooltip saying `text` next to `anchor` (the icon, root
    /// coordinates), inside `area`.
    pub fn open(
        x: &X,
        text: &str,
        anchor: Rect,
        area: Rect,
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
        x.conn.map_window(win)?;
        let t = Tooltip {
            win,
            gc,
            text: text.to_string(),
            anchor,
            area,
        };
        t.redraw(x, fonts);
        Ok(t)
    }

    pub fn window(&self) -> u32 {
        self.win
    }

    /// Shows new text (resized and re-placed); a no-op if it is unchanged.
    pub fn set_text(&mut self, x: &X, text: &str, fonts: &mut FontSet) {
        if text == self.text {
            return;
        }
        text.clone_into(&mut self.text);
        let r = place(x, text, self.anchor, self.area, fonts);
        let aux = ConfigureWindowAux::new()
            .x(r.x)
            .y(r.y)
            .width(r.w as u32)
            .height(r.h as u32);
        let _ = x.conn.configure_window(self.win, &aux);
        self.redraw(x, fonts);
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
