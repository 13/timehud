//! X11 platform layer: connection, ARGB windows, presenting pixmaps, keymap.
//!
//! The connection also selects `SubstructureNotify` on the root window so the
//! event loop sees other top-level windows map/restack (see
//! [`overlay::restack_needed`]).

pub mod grabs;
pub mod overlay;
pub mod popup;
pub mod tray;

use crate::layout::Rect;
use anyhow::{Context, bail};
use std::cell::RefCell;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::RwLock;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ColormapAlloc, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, ImageFormat, ImageOrder, PropMode, VisualClass, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        UTF8_STRING,
        _NET_WM_NAME,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NOTIFICATION,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WORKAREA,
        _NET_CURRENT_DESKTOP,
        _COMPTON_SHADOW,
        MANAGER,
        _NET_SYSTEM_TRAY_OPCODE,
        _NET_SYSTEM_TRAY_VISUAL,
        _XEMBED_INFO,
    }
}

pub struct X {
    pub conn: RustConnection,
    pub screen_num: usize,
    pub root: u32,
    pub argb_visual: u32,
    pub colormap: u32,
    pub atoms: Atoms,
    /// Reused BGRA conversion buffer for `present`.
    bgra: RefCell<Vec<u8>>,
}

impl X {
    /// Connects to `$DISPLAY`. Errors if the screen has no 32-bit TrueColor
    /// visual (needed for per-pixel alpha) or uses MSBFirst image byte order.
    pub fn connect() -> anyhow::Result<X> {
        let (conn, screen_num) = x11rb::connect(None).context("cannot connect to X display")?;
        let setup = conn.setup();
        if setup.image_byte_order != ImageOrder::LSB_FIRST {
            bail!("X server uses MSBFirst image byte order (unsupported)");
        }
        let screen = &setup.roots[screen_num];
        let root = screen.root;
        let argb_visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| &d.visuals)
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id)
            .context("no 32-bit TrueColor visual (is a compositor-capable X server running?)")?;
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, argb_visual)?;
        let atoms = Atoms::new(&conn)?.reply()?;
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
        )?;
        let x = X {
            conn,
            screen_num,
            root,
            argb_visual,
            colormap,
            atoms,
            bgra: RefCell::default(),
        };
        x.reload_keymap();
        x.conn.flush()?;
        Ok(x)
    }

    pub fn fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }

    /// `Xft.dpi / 96` from the root `RESOURCE_MANAGER` property, default 1.0.
    pub fn scale(&self) -> f32 {
        self.conn
            .get_property(
                false,
                self.root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                1 << 20,
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| parse_xft_dpi(&String::from_utf8_lossy(&r.value)))
            .map_or(1.0, |dpi| dpi / 96.0)
    }

    /// Pointer position in root coordinates; `(0, 0)` if the query fails.
    pub fn pointer(&self) -> (i32, i32) {
        self.conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or((0, 0), |r| (r.root_x.into(), r.root_y.into()))
    }

    /// The RandR monitor containing the point, intersected with the current
    /// desktop's `_NET_WORKAREA` if the WM sets one.
    pub fn work_area_at(&self, x: i32, y: i32) -> Rect {
        let screen = &self.conn.setup().roots[self.screen_num];
        let full = Rect {
            x: 0,
            y: 0,
            w: screen.width_in_pixels.into(),
            h: screen.height_in_pixels.into(),
        };
        let monitors: Vec<Rect> = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                r.monitors
                    .iter()
                    .map(|m| Rect {
                        x: m.x.into(),
                        y: m.y.into(),
                        w: m.width.into(),
                        h: m.height.into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let monitor = monitor_at(&monitors, x, y).unwrap_or(full);
        self.net_workarea()
            .and_then(|wa| intersect(monitor, wa))
            .unwrap_or(monitor)
    }

    fn cardinals(&self, atom: u32, len: u32) -> Option<Vec<u32>> {
        let r = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::CARDINAL, 0, len)
            .ok()?
            .reply()
            .ok()?;
        Some(r.value32()?.collect())
    }

    fn net_workarea(&self) -> Option<Rect> {
        let desktop = self
            .cardinals(self.atoms._NET_CURRENT_DESKTOP, 1)
            .and_then(|v| v.first().copied())
            .unwrap_or(0) as usize;
        let v = self.cardinals(self.atoms._NET_WORKAREA, 4 * 64)?;
        let r = v.get(desktop * 4..desktop * 4 + 4).or(v.get(0..4))?;
        Some(Rect {
            x: r[0] as i32,
            y: r[1] as i32,
            w: r[2] as i32,
            h: r[3] as i32,
        })
    }

    /// Uploads a premultiplied RGBA pixmap to `win` at (0, 0), in row bands
    /// that fit the server's maximum request size.
    pub fn present(&self, win: u32, gc: u32, pm: &tiny_skia::Pixmap) -> anyhow::Result<()> {
        let mut buf = self.bgra.borrow_mut();
        rgba_to_bgra(pm.data(), &mut buf);
        let (w, h) = (pm.width() as usize, pm.height() as usize);
        self.put_bgra(win, gc, &buf, (0, 0, w, h))?;
        self.conn.flush()?;
        Ok(())
    }

    /// Uploads `canvas.pixmap` to `win` at (0, 0), sending only the bounding
    /// box of the pixels that changed since the last upload (everything after
    /// [`Canvas::invalidate`] or a size change). Nothing to do without a pixmap.
    pub fn present_canvas(&self, win: u32, gc: u32, c: &mut Canvas) -> anyhow::Result<()> {
        let Some(pm) = &c.pixmap else {
            return Ok(());
        };
        let (w, h) = (pm.width() as usize, pm.height() as usize);
        rgba_to_bgra(pm.data(), &mut c.next);
        let full = (0, 0, w, h);
        let rect = if c.shown_w == w && c.shown.len() == c.next.len() {
            changed_rect(&c.shown, &c.next, w)
        } else {
            Some(full)
        };
        let sent = match rect {
            None => Ok(()),
            Some(r) if r == full => self.put_bgra(win, gc, &c.next, r),
            Some(r) => {
                copy_rect(&c.next, w, r, &mut c.sub);
                self.put_bgra(win, gc, &c.sub, r)
            }
        };
        std::mem::swap(&mut c.shown, &mut c.next);
        c.shown_w = w;
        if sent.is_err() {
            c.invalidate();
        }
        sent?;
        self.conn.flush()?;
        Ok(())
    }

    /// PutImage of tightly packed BGRA `data` covering `(x, y, w, h)` of
    /// `win`, in row bands that fit the server's maximum request size.
    fn put_bgra(
        &self,
        win: u32,
        gc: u32,
        data: &[u8],
        (x, y, w, h): (usize, usize, usize, usize),
    ) -> anyhow::Result<()> {
        let (w16, _) = image_dims(w, h)?;
        let (x, y) = (
            i16::try_from(x).context("image x offset")?,
            i16::try_from(y).context("image y offset")?,
        );
        let stride = w * 4;
        // PutImage header is 24 bytes (28 with BIG-REQUESTS); leave slack.
        let rows = (self.conn.maximum_request_bytes().saturating_sub(64) / stride).clamp(1, h);
        for y0 in (0..h).step_by(rows) {
            let n = rows.min(h - y0);
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                win,
                gc,
                w16,
                n as u16,
                x,
                y + y0 as i16,
                0,
                32,
                &data[y0 * stride..(y0 + n) * stride],
            )?;
        }
        Ok(())
    }

    /// Re-reads the keyboard mapping used by `translate`, the popup and
    /// `Grabs`. Call on `MappingNotify` (then re-grab hotkeys).
    pub fn reload_keymap(&self) {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let Some(r) = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        if let Ok(mut k) = KEYMAP.write() {
            *k = Keymap {
                min_keycode: min,
                per: r.keysyms_per_keycode,
                syms: r.keysyms,
            };
        }
    }

    /// Creates an unmapped override-redirect ARGB window with a GC.
    /// Sets `WM_CLASS` "timehud", the name, the EWMH window type and, unless
    /// `shadow`, `_COMPTON_SHADOW = 0` so picom draws no shadow.
    fn create_window(
        &self,
        rect: Rect,
        events: EventMask,
        name: &str,
        window_type: u32,
        shadow: bool,
    ) -> anyhow::Result<(u32, u32)> {
        let win = self.conn.generate_id()?;
        self.conn.create_window(
            32,
            win,
            self.root,
            rect.x as i16,
            rect.y as i16,
            rect.w.max(1) as u16,
            rect.h.max(1) as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.argb_visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .override_redirect(1)
                .colormap(self.colormap)
                .event_mask(events),
        )?;
        let gc = self.conn.generate_id()?;
        self.conn
            .create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0))?;
        let c = &self.conn;
        c.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"timehud\0timehud\0",
        )?;
        c.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            name.as_bytes(),
        )?;
        c.change_property8(
            PropMode::REPLACE,
            win,
            self.atoms._NET_WM_NAME,
            self.atoms.UTF8_STRING,
            name.as_bytes(),
        )?;
        c.change_property32(
            PropMode::REPLACE,
            win,
            self.atoms._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[window_type],
        )?;
        if !shadow {
            c.change_property32(
                PropMode::REPLACE,
                win,
                self.atoms._COMPTON_SHADOW,
                AtomEnum::CARDINAL,
                &[0],
            )?;
        }
        Ok((win, gc))
    }

    fn destroy_window(&self, win: u32, gc: u32) {
        let _ = self.conn.free_gc(gc);
        let _ = self.conn.destroy_window(win);
    }
}

/// Converts premultiplied RGBA bytes to the BGRA byte order of an LSBFirst
/// ARGB32 visual (also premultiplied, as X Render expects).
pub fn rgba_to_bgra(src: &[u8], dst: &mut Vec<u8>) {
    dst.clear();
    dst.reserve(src.len());
    for &[r, g, b, a] in src.as_chunks::<4>().0 {
        dst.extend_from_slice(&[b, g, r, a]);
    }
}

/// A window's back buffer: the pixmap drawn each frame and what was last
/// uploaded, so [`X::present_canvas`] can send only the changed pixels.
#[derive(Default)]
pub struct Canvas {
    pub pixmap: Option<tiny_skia::Pixmap>,
    /// BGRA of the window contents as last uploaded; empty when unknown.
    shown: Vec<u8>,
    shown_w: usize,
    next: Vec<u8>,
    sub: Vec<u8>,
}

impl Canvas {
    /// Forget what the window shows (e.g. after `Expose`): the next present
    /// uploads everything.
    pub fn invalidate(&mut self) {
        self.shown.clear();
        self.shown_w = 0;
    }
}

/// Bounding box `(x, y, w, h)` of the pixels that differ between two
/// same-size images of 4-byte pixels, `w` pixels wide; `None` if equal.
pub fn changed_rect(a: &[u8], b: &[u8], w: usize) -> Option<(usize, usize, usize, usize)> {
    let stride = w * 4;
    let rows = || a.chunks_exact(stride).zip(b.chunks_exact(stride));
    let y0 = rows().position(|(ra, rb)| ra != rb)?;
    let y1 = rows().rposition(|(ra, rb)| ra != rb)?;
    let (mut x0, mut x1) = (w, 0);
    for (ra, rb) in rows().skip(y0).take(y1 - y0 + 1) {
        let px = || ra.as_chunks::<4>().0.iter().zip(rb.as_chunks::<4>().0);
        if let Some(i) = px().position(|(pa, pb)| pa != pb) {
            x0 = x0.min(i);
            x1 = x1.max(px().rposition(|(pa, pb)| pa != pb).unwrap_or(i));
        }
    }
    Some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// Copies rect `(x, y, w, h)` of an image of 4-byte pixels, `stride_px`
/// pixels wide, into `dst` as tightly packed rows.
fn copy_rect(
    src: &[u8],
    stride_px: usize,
    (x, y, w, h): (usize, usize, usize, usize),
    dst: &mut Vec<u8>,
) {
    dst.clear();
    for row in src.chunks_exact(stride_px * 4).skip(y).take(h) {
        dst.extend_from_slice(&row[x * 4..(x + w) * 4]);
    }
}

/// Image size as PutImage dimensions; errors when empty or so large that
/// offsets inside it would not fit the protocol's 16-bit coordinates.
fn image_dims(w: usize, h: usize) -> anyhow::Result<(u16, u16)> {
    let max = i16::MAX as usize;
    if w == 0 || h == 0 || w > max || h > max {
        bail!("image size {w}x{h} out of range");
    }
    Ok((w as u16, h as u16))
}

/// `Xft.dpi` value from an X resource database string.
fn parse_xft_dpi(db: &str) -> Option<f32> {
    db.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == "Xft.dpi")
            .then(|| v.trim().parse::<f32>().ok())
            .flatten()
            .filter(|d| *d > 0.0)
    })
}

fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// Monitor containing the point, else the first one.
fn monitor_at(monitors: &[Rect], x: i32, y: i32) -> Option<Rect> {
    monitors
        .iter()
        .copied()
        .find(|m| contains(*m, x, y))
        .or(monitors.first().copied())
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    (x1 > x0 && y1 > y0).then_some(Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// Cached `GetKeyboardMapping` result shared by event translation and grabs.
struct Keymap {
    min_keycode: u8,
    per: u8,
    syms: Vec<u32>,
}

static KEYMAP: RwLock<Keymap> = RwLock::new(Keymap {
    min_keycode: 0,
    per: 0,
    syms: Vec::new(),
});

impl Keymap {
    /// Unshifted (first column) keysym of a keycode; 0 if unknown.
    fn keysym(&self, keycode: u8) -> u32 {
        let per = usize::from(self.per);
        keycode
            .checked_sub(self.min_keycode)
            .map(|i| usize::from(i) * per)
            .and_then(|i| self.syms.get(i).copied())
            .unwrap_or(0)
    }

    /// A keycode producing `keysym` in any column (letters match either case).
    fn keycode(&self, keysym: u32) -> Option<u8> {
        let alt = match keysym {
            0x61..=0x7a => keysym - 0x20,
            0x41..=0x5a => keysym + 0x20,
            _ => keysym,
        };
        let per = usize::from(self.per).max(1);
        let i = self
            .syms
            .iter()
            .position(|s| *s == keysym)
            .or_else(|| self.syms.iter().position(|s| *s == alt))?;
        u8::try_from(i / per + usize::from(self.min_keycode)).ok()
    }
}

fn keysym_of(keycode: u8) -> u32 {
    KEYMAP.read().map_or(0, |k| k.keysym(keycode))
}

fn keycode_of(keysym: u32) -> Option<u8> {
    KEYMAP.read().ok()?.keycode(keysym)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_bgra_swaps() {
        let mut out = Vec::new();
        rgba_to_bgra(&[1, 2, 3, 4], &mut out);
        assert_eq!(out, [3, 2, 1, 4]);
    }

    #[test]
    fn rgba_to_bgra_reuses_buffer() {
        let mut out = vec![9; 12];
        rgba_to_bgra(&[1, 2, 3, 4, 5, 6, 7, 8], &mut out);
        assert_eq!(out, [3, 2, 1, 4, 7, 6, 5, 8]);
    }

    /// 4x3 image of 4-byte pixels, each pixel's bytes = its index.
    fn img() -> Vec<u8> {
        (0..12u8).flat_map(|i| [i; 4]).collect()
    }

    #[test]
    fn changed_rect_none_when_equal() {
        assert_eq!(changed_rect(&img(), &img(), 4), None);
    }

    #[test]
    fn changed_rect_bounds_all_differences() {
        let mut b = img();
        b[(4 + 2) * 4 + 1] = 99; // pixel (2, 1)
        assert_eq!(changed_rect(&img(), &b, 4), Some((2, 1, 1, 1)));
        b[(2 * 4 + 1) * 4 + 3] = 99; // pixel (1, 2)
        b[3 * 4] = 99; // pixel (3, 0)
        assert_eq!(changed_rect(&img(), &b, 4), Some((1, 0, 3, 3)));
    }

    #[test]
    fn copy_rect_extracts_rows() {
        let mut out = vec![7; 3];
        copy_rect(&img(), 4, (1, 1, 2, 2), &mut out);
        let want: Vec<u8> = [5u8, 6, 9, 10].iter().flat_map(|i| [*i; 4]).collect();
        assert_eq!(out, want);
    }

    #[test]
    fn image_dims_reject_oversize() {
        assert_eq!(image_dims(10, 20).ok(), Some((10, 20)));
        assert!(image_dims(70_000, 1).is_err());
        assert!(image_dims(1, 70_000).is_err());
        assert!(image_dims(0, 1).is_err());
    }

    #[test]
    fn xft_dpi_parsed() {
        let db = "Xcursor.size:\t24\nXft.dpi:\t144\nXft.antialias:\t1\n";
        assert_eq!(parse_xft_dpi(db), Some(144.0));
        assert_eq!(parse_xft_dpi("Xft.antialias: 1"), None);
        assert_eq!(parse_xft_dpi("Xft.dpi: bogus"), None);
    }

    const LEFT: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    const RIGHT: Rect = Rect {
        x: 1920,
        y: 0,
        w: 2560,
        h: 1440,
    };

    #[test]
    fn monitor_containing_point() {
        assert_eq!(monitor_at(&[LEFT, RIGHT], 2000, 10), Some(RIGHT));
        assert_eq!(monitor_at(&[LEFT, RIGHT], -5, -5), Some(LEFT));
        assert_eq!(monitor_at(&[], 0, 0), None);
    }

    #[test]
    fn workarea_intersection() {
        // Workarea spanning both monitors minus a 30 px top bar.
        let wa = Rect {
            x: 0,
            y: 30,
            w: 4480,
            h: 1410,
        };
        assert_eq!(
            intersect(RIGHT, wa),
            Some(Rect {
                x: 1920,
                y: 30,
                w: 2560,
                h: 1410
            })
        );
        let far = Rect {
            x: 5000,
            y: 0,
            w: 10,
            h: 10,
        };
        assert_eq!(intersect(LEFT, far), None);
    }

    #[test]
    fn keymap_lookup_both_ways() {
        // keycodes 8.., 2 columns each: 8 = (q, Q), 9 = (space, space)
        let k = Keymap {
            min_keycode: 8,
            per: 2,
            syms: vec![0x71, 0x51, 0x20, 0x20],
        };
        assert_eq!(k.keysym(8), 0x71);
        assert_eq!(k.keysym(9), 0x20);
        assert_eq!(k.keysym(7), 0);
        assert_eq!(k.keysym(50), 0);
        assert_eq!(k.keycode(0x20), Some(9));
        assert_eq!(k.keycode(0x51), Some(8));
        assert_eq!(k.keycode(0xffc9), None);
    }
}
