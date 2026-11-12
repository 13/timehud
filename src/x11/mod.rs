//! X11 platform layer: connection, ARGB windows, presenting pixmaps, keymap.
//!
//! The connection selects every root event the HUD needs, once:
//! `SubstructureNotify` so the event loop sees other top-level windows
//! map/restack (see [`overlay::restack_needed`]), `StructureNotify` for root
//! size changes and the tray's `MANAGER` client messages, and
//! `PropertyChange` for `_NET_WORKAREA`, the current desktop and
//! `RESOURCE_MANAGER` (see [`X::changes_screens`], [`X::changes_scale`]).

pub mod grabs;
pub mod overlay;
pub mod popup;
pub mod tooltip;
pub mod tray;

use crate::layout::Rect;
use anyhow::{Context, bail};
use std::cell::{Cell, RefCell};
use std::os::fd::{AsRawFd, RawFd};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
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
        _NET_WM_WINDOW_TYPE_TOOLTIP,
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
    scale: Cell<f32>,
    /// The keyboard mapping ([`X::reload_keymap`]).
    keymap: RefCell<Keymap>,
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
            &ChangeWindowAttributesAux::new().event_mask(
                EventMask::SUBSTRUCTURE_NOTIFY
                    | EventMask::STRUCTURE_NOTIFY
                    | EventMask::PROPERTY_CHANGE,
            ),
        )?;
        let x = X {
            conn,
            screen_num,
            root,
            argb_visual,
            colormap,
            atoms,
            scale: Cell::new(1.0),
            keymap: RefCell::default(),
            bgra: RefCell::default(),
        };
        x.reload_scale();
        x.reload_keymap();
        x.conn.flush()?;
        Ok(x)
    }

    pub fn fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }

    /// `Xft.dpi / 96` as last read by [`X::reload_scale`] (at connect and
    /// when the root `RESOURCE_MANAGER` property changes).
    pub fn scale(&self) -> f32 {
        self.scale.get()
    }

    /// Re-reads `Xft.dpi / 96` from the root `RESOURCE_MANAGER` property
    /// (default 1.0); true if the scale changed.
    pub fn reload_scale(&self) -> bool {
        let scale = self
            .conn
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
            .map_or(1.0, |dpi| dpi / 96.0);
        self.scale.replace(scale) != scale
    }

    /// Pointer position in root coordinates; `(0, 0)` if the query fails.
    pub fn pointer(&self) -> (i32, i32) {
        self.conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or((0, 0), |r| (r.root_x.into(), r.root_y.into()))
    }

    /// Asks for RandR screen, CRTC and output changes (monitors added,
    /// removed, moved or resized), which change [`X::screens`]; the core
    /// root events are selected by [`X::connect`].
    pub fn watch_screens(&self) -> anyhow::Result<()> {
        use x11rb::protocol::randr::NotifyMask;
        // RandR delivers 1.2+ events only to clients that announced 1.2+.
        if self.conn.randr_query_version(1, 5)?.reply().is_ok() {
            self.conn.randr_select_input(
                self.root,
                NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE | NotifyMask::OUTPUT_CHANGE,
            )?;
        }
        Ok(())
    }

    /// Whether a root event may change [`X::screens`].
    pub fn changes_screens(&self, ev: &Event) -> bool {
        match ev {
            Event::ConfigureNotify(e) => e.window == self.root,
            Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => true,
            Event::PropertyNotify(e) => {
                e.window == self.root
                    && (e.atom == self.atoms._NET_WORKAREA
                        || e.atom == self.atoms._NET_CURRENT_DESKTOP)
            }
            _ => false,
        }
    }

    /// Whether a root event may change [`X::scale`].
    pub fn changes_scale(&self, ev: &Event) -> bool {
        matches!(ev, Event::PropertyNotify(e)
            if e.window == self.root && e.atom == u32::from(AtomEnum::RESOURCE_MANAGER))
    }

    /// The current screen size, RandR monitors and `_NET_WORKAREA`.
    pub fn screens(&self) -> Screens {
        let size = self
            .conn
            .get_geometry(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|g| (i32::from(g.width), i32::from(g.height)))
            .unwrap_or_else(|| {
                let screen = &self.conn.setup().roots[self.screen_num];
                (
                    screen.width_in_pixels.into(),
                    screen.height_in_pixels.into(),
                )
            });
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
        Screens {
            size,
            monitors,
            workarea: self.net_workarea(),
        }
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
        self.put_bgra(win, gc, 32, &buf, (0, 0, w, h))?;
        self.conn.flush()?;
        Ok(())
    }

    /// Uploads `canvas.frame` to `win` at (0, 0), sending only the bounding
    /// box of the pixels that changed since the last upload (everything after
    /// [`Canvas::invalidate`], a size or an opacity change). Only that box is
    /// converted to BGRA, with the frame's opacity applied on the way.
    /// Nothing to do without a pixmap.
    pub fn present_canvas(&self, win: u32, gc: u32, c: &mut Canvas) -> anyhow::Result<()> {
        let Some(r) = c.next_upload() else {
            return Ok(());
        };
        let sent = self.put_bgra(win, gc, 32, &c.sub, r);
        if sent.is_err() {
            c.invalidate();
        }
        sent?;
        self.conn.flush()?;
        Ok(())
    }

    /// PutImage of tightly packed 32 bpp BGRA (or BGRX for `depth` 24)
    /// `data` covering `(x, y, w, h)` of `win`, in row bands that fit the
    /// server's maximum request size.
    fn put_bgra(
        &self,
        win: u32,
        gc: u32,
        depth: u8,
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
                depth,
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
        *self.keymap.borrow_mut() = Keymap {
            min_keycode: min,
            per: r.keysyms_per_keycode,
            syms: r.keysyms,
        };
    }

    /// Unshifted (first column) keysym of a keycode; 0 if unknown.
    pub fn keysym(&self, keycode: u8) -> u32 {
        self.keymap.borrow().keysym(keycode)
    }

    /// The keycode producing `keysym` and its column (1 = needs Shift);
    /// see [`Keymap::keycode`].
    pub fn keycode(&self, keysym: u32) -> Option<(u8, u8)> {
        self.keymap.borrow().keycode(keysym)
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
        self.set_names(win, name)?;
        let c = &self.conn;
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

    /// Sets `WM_CLASS` "timehud", `WM_NAME` and `_NET_WM_NAME` to `name`.
    fn set_names(&self, win: u32, name: &str) -> anyhow::Result<()> {
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
        Ok(())
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
    pub frame: crate::render::Frame,
    /// The frame's RGBA (before opacity) as last uploaded; empty when unknown.
    shown: Vec<u8>,
    shown_w: usize,
    /// The opacity `shown` was uploaded with.
    shown_opacity: f32,
    /// BGRA staging buffer for the uploaded rect.
    sub: Vec<u8>,
}

impl Canvas {
    /// The rect of the frame to upload, its BGRA (opacity applied) left in
    /// `sub`, and records it as shown; `None` if nothing changed.
    fn next_upload(&mut self) -> Option<(usize, usize, usize, usize)> {
        let pm = self.frame.pixmap.as_ref()?;
        let (w, h) = (pm.width() as usize, pm.height() as usize);
        let src = pm.data();
        let opacity = self.frame.opacity();
        let known =
            self.shown_w == w && self.shown.len() == src.len() && self.shown_opacity == opacity;
        let r = if known {
            changed_rect(&self.shown, src, w)?
        } else {
            (0, 0, w, h)
        };
        let lut = crate::render::opacity_lut(opacity);
        convert_rect(src, w, r, lut.as_ref(), &mut self.sub);
        if known {
            copy_rect_into(src, w, r, &mut self.shown);
        } else {
            self.shown.clear();
            self.shown.extend_from_slice(src);
        }
        self.shown_w = w;
        self.shown_opacity = opacity;
        Some(r)
    }

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

/// Converts rect `(x, y, w, h)` of a premultiplied RGBA image, `stride_px`
/// pixels wide, into `dst` as tightly packed BGRA rows, each byte mapped
/// through `lut` (the opacity, [`crate::render::opacity_lut`]) if given.
fn convert_rect(
    src: &[u8],
    stride_px: usize,
    (x, y, w, h): (usize, usize, usize, usize),
    lut: Option<&[u8; 256]>,
    dst: &mut Vec<u8>,
) {
    dst.clear();
    dst.reserve(w * h * 4);
    for row in src.chunks_exact(stride_px * 4).skip(y).take(h) {
        let px = row[x * 4..(x + w) * 4].as_chunks::<4>().0;
        match lut {
            None => px.iter().for_each(|&[r, g, b, a]| dst.extend([b, g, r, a])),
            Some(l) => px.iter().for_each(|&[r, g, b, a]| {
                dst.extend([b, g, r, a].map(|c| l[usize::from(c)]));
            }),
        }
    }
}

/// Copies rect `(x, y, w, h)` of `src` into the same place of `dst`, both
/// images of 4-byte pixels `stride_px` wide.
fn copy_rect_into(
    src: &[u8],
    stride_px: usize,
    (x, y, w, h): (usize, usize, usize, usize),
    dst: &mut [u8],
) {
    let stride = stride_px * 4;
    let rows = src.chunks_exact(stride).zip(dst.chunks_exact_mut(stride));
    for (s, d) in rows.skip(y).take(h) {
        d[x * 4..(x + w) * 4].copy_from_slice(&s[x * 4..(x + w) * 4]);
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

/// Monitor layout and work area, cached by the HUD (refreshed on the events
/// [`X::changes_screens`] reports) so placing windows needs no round trips.
#[derive(Clone, Debug, PartialEq)]
pub struct Screens {
    /// Root window size.
    pub size: (i32, i32),
    /// RandR monitors (may be empty, e.g. without RandR).
    pub monitors: Vec<Rect>,
    /// The current desktop's `_NET_WORKAREA`, if the WM sets one.
    pub workarea: Option<Rect>,
}

impl Screens {
    fn full(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: self.size.0,
            h: self.size.1,
        }
    }

    /// The monitor containing the point (else the first, else the whole
    /// screen), intersected with the work area.
    pub fn work_area_at(&self, x: i32, y: i32) -> Rect {
        let monitor = monitor_at(&self.monitors, x, y).unwrap_or(self.full());
        self.workarea
            .and_then(|wa| intersect(monitor, wa))
            .unwrap_or(monitor)
    }

    /// Whether the point lies on some monitor (on the screen if none known).
    pub fn on_screen(&self, x: i32, y: i32) -> bool {
        if self.monitors.is_empty() {
            contains(self.full(), x, y)
        } else {
            self.monitors.iter().any(|m| contains(*m, x, y))
        }
    }

    /// Where the HUD starts: the work area of the monitor holding the saved
    /// custom position, so a HUD saved on another monitor stays there; the
    /// one under the pointer without a custom position or when the saved one
    /// is on no monitor now (`pointer` is only queried then).
    pub fn start_area(
        &self,
        custom: Option<(i32, i32)>,
        pointer: impl FnOnce() -> (i32, i32),
    ) -> Rect {
        match custom {
            Some((x, y)) if self.on_screen(x, y) => self.work_area_at(x, y),
            _ => {
                let (x, y) = pointer();
                self.work_area_at(x, y)
            }
        }
    }
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
#[derive(Default)]
struct Keymap {
    min_keycode: u8,
    per: u8,
    syms: Vec<u32>,
}

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

    /// The keycode producing `keysym` and its column: 0 = unshifted,
    /// 1 = with Shift. Unshifted keys are preferred; columns 2 and up (other
    /// groups, AltGr levels) are not used. Letters match either case and
    /// always report column 0, so "Ctrl+A" means the plain A key.
    fn keycode(&self, keysym: u32) -> Option<(u8, u8)> {
        let per = usize::from(self.per).max(1);
        let cols = per.min(2);
        let letter = match keysym {
            0x61..=0x7a => Some((keysym, keysym - 0x20)),
            0x41..=0x5a => Some((keysym + 0x20, keysym)),
            _ => None,
        };
        let find = |col: usize, want: &dyn Fn(u32) -> bool| {
            self.syms
                .chunks(per)
                .position(|row| row.get(col).is_some_and(|s| want(*s)))
                .and_then(|i| u8::try_from(i + usize::from(self.min_keycode)).ok())
        };
        if let Some((lower, upper)) = letter {
            let is_letter = |s: u32| s == lower || s == upper;
            return (0..cols)
                .find_map(|col| find(col, &is_letter))
                .map(|k| (k, 0));
        }
        (0..cols).find_map(|col| find(col, &|s| s == keysym).map(|k| (k, col as u8)))
    }
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
    fn convert_rect_extracts_rows() {
        let mut out = vec![7; 3];
        convert_rect(&img(), 4, (1, 1, 2, 2), None, &mut out);
        let want: Vec<u8> = [5u8, 6, 9, 10].iter().flat_map(|i| [*i; 4]).collect();
        assert_eq!(out, want);
    }

    #[test]
    fn convert_rect_swaps_and_applies_opacity() {
        // 2x1 RGBA image; convert only the second pixel.
        let src = [1, 2, 3, 4, 10, 20, 30, 40];
        let mut out = Vec::new();
        convert_rect(&src, 2, (1, 0, 1, 1), None, &mut out);
        assert_eq!(out, [30, 20, 10, 40]);
        let lut = crate::render::opacity_lut(0.5).unwrap();
        convert_rect(&src, 2, (0, 0, 2, 1), Some(&lut), &mut out);
        // Same result as converting everything, then scaling every byte.
        let mut full = Vec::new();
        rgba_to_bgra(&src, &mut full);
        let scaled: Vec<u8> = full.iter().map(|b| lut[usize::from(*b)]).collect();
        assert_eq!(out, scaled);
        assert_eq!(out[4..], [15, 10, 5, 20]);
    }

    /// Uploads applied to a simulated window always reproduce the frame
    /// converted in full, while sending only the changed rect.
    #[test]
    fn canvas_uploads_reproduce_full_frames() {
        use crate::config::Config;
        use crate::render::{View, render_into};
        use crate::themes::{Rgb, get_theme};
        let cfg = Config {
            progress_style: "line".into(),
            ..Config::default()
        };
        let mut fonts = crate::text::FontSet::bundled();
        let mut c = Canvas::default();
        let mut window: Vec<u8> = Vec::new();
        // (progress, opacity, timer, expect a full upload)
        let steps = [
            (0.5, 1.0, "05:00", true),
            (0.5, 1.0, "05:00", false),
            (0.6, 1.0, "05:00", false),
            (0.6, 0.5, "05:00", true),
            (0.7, 0.5, "04:59", false),
            (0.7, 1.0, "04:59", true),
        ];
        for (i, (progress, opacity, timer, full)) in steps.into_iter().enumerate() {
            let view = View {
                cfg: &cfg,
                theme: get_theme("classic"),
                scale: 1.0,
                clock: "12:00:00".into(),
                timer: timer.into(),
                timer_color: Rgb(255, 255, 255),
                timer_px_mul: 1.0,
                mode_text: "COUNTDOWN  5:00".into(),
                mode_color: Rgb(102, 102, 102),
                show_controls_row: true,
                controls_pos: 1.0,
                running: true,
                mode_btn: "CD",
                hover_btn: None,
                progress,
                progress_color: Rgb(0, 200, 100),
                opacity,
            };
            render_into(&view, &mut fonts, &mut c.frame);
            let pm = c.frame.pixmap.as_ref().unwrap();
            let (w, h) = (pm.width() as usize, pm.height() as usize);
            let mut want = Vec::new();
            rgba_to_bgra(pm.data(), &mut want);
            if let Some(lut) = crate::render::opacity_lut(opacity) {
                want.iter_mut().for_each(|b| *b = lut[usize::from(*b)]);
            }
            let r = c.next_upload();
            if i == 1 {
                assert_eq!(r, None, "unchanged frame uploads nothing");
            } else {
                let r = r.unwrap();
                assert_eq!(r == (0, 0, w, h), full, "step {i}: {r:?}");
                window.resize(w * h * 4, 0);
                let (x, y, rw, _) = r;
                for (row, data) in c.sub.chunks_exact(rw * 4).enumerate() {
                    let at = ((y + row) * w + x) * 4;
                    window[at..at + rw * 4].copy_from_slice(data);
                }
            }
            assert!(window == want, "step {i}: window differs from the frame");
        }
    }

    #[test]
    fn copy_rect_into_updates_only_the_rect() {
        let src: Vec<u8> = (0..12u8).flat_map(|i| [i + 100; 4]).collect();
        let mut dst = img();
        copy_rect_into(&src, 4, (1, 1, 2, 2), &mut dst);
        for i in 0..12u8 {
            let (x, y) = (i % 4, i / 4);
            let inside = (1..3).contains(&x) && (1..3).contains(&y);
            let want = if inside { i + 100 } else { i };
            assert_eq!(dst[usize::from(i) * 4..][..4], [want; 4], "pixel {i}");
        }
        let mut all = img();
        copy_rect_into(&src, 4, (0, 0, 4, 3), &mut all);
        assert_eq!(changed_rect(&all, &src, 4), None);
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

    fn two_monitors() -> Screens {
        Screens {
            size: (4480, 1440),
            monitors: vec![LEFT, RIGHT],
            workarea: None,
        }
    }

    #[test]
    fn start_area_follows_the_saved_position() {
        let s = two_monitors();
        let no_pointer = || panic!("pointer queried although the position is known");
        // Saved on the right monitor, pointer on the left: stay right.
        assert_eq!(s.start_area(Some((2500, 100)), no_pointer), RIGHT);
        assert_eq!(s.start_area(Some((10, 10)), no_pointer), LEFT);
        // No custom position: the monitor under the pointer.
        assert_eq!(s.start_area(None, || (2000, 10)), RIGHT);
        // Saved on a monitor that is gone (or in a gap): pointer's monitor.
        assert_eq!(s.start_area(Some((9000, 100)), || (2000, 10)), RIGHT);
        assert_eq!(s.start_area(Some((100, 1300)), || (2000, 10)), RIGHT);
    }

    #[test]
    fn screens_without_monitors_use_the_current_size() {
        let s = Screens {
            size: (3000, 2000),
            monitors: Vec::new(),
            workarea: Some(Rect {
                x: 0,
                y: 30,
                w: 3000,
                h: 1970,
            }),
        };
        assert_eq!(
            s.work_area_at(5, 5),
            Rect {
                x: 0,
                y: 30,
                w: 3000,
                h: 1970
            }
        );
        assert!(s.on_screen(2999, 1999));
        assert!(!s.on_screen(3000, 10));
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
        assert_eq!(k.keycode(0x20), Some((9, 0)));
        assert_eq!(k.keycode(0x51), Some((8, 0)));
        assert_eq!(k.keycode(0x71), Some((8, 0)));
        assert_eq!(k.keycode(0xffc9), None);
    }

    /// US-like rows: 10 = (equal, plus), 11 = (bracketleft, braceleft),
    /// 12 = (semicolon, colon), 13 = (KP_Add), 14 = (a, A, ä, Ä, at, ...).
    fn us_keymap() -> Keymap {
        let rows: [[u32; 6]; 5] = [
            [0x3d, 0x2b, 0x3d, 0x2b, 0, 0],
            [0x5b, 0x7b, 0x5b, 0x7b, 0, 0],
            [0x3b, 0x3a, 0x3b, 0x3a, 0, 0],
            [0xffab, 0xffab, 0xffab, 0xffab, 0, 0],
            [0x61, 0x41, 0xe4, 0xc4, 0x40, 0],
        ];
        Keymap {
            min_keycode: 10,
            per: 6,
            syms: rows.concat(),
        }
    }

    #[test]
    fn keycode_reports_the_shift_column() {
        let k = us_keymap();
        assert_eq!(k.keycode(0x2b), Some((10, 1)), "plus needs Shift");
        assert_eq!(k.keycode(0x3d), Some((10, 0)));
        assert_eq!(k.keycode(0x7b), Some((11, 1)), "braceleft needs Shift");
        assert_eq!(k.keycode(0x3a), Some((12, 1)), "colon needs Shift");
        assert_eq!(k.keycode(0xffab), Some((13, 0)));
        assert_eq!(k.keycode(0x41), Some((14, 0)), "letters never imply Shift");
    }

    #[test]
    fn keycode_prefers_unshifted_and_ignores_other_groups_and_levels() {
        let mut k = us_keymap();
        // A second key with plus unshifted (like a German layout) wins.
        k.syms.extend([0x2b, 0x2a, 0, 0, 0, 0]);
        assert_eq!(k.keycode(0x2b), Some((15, 0)));
        // ä (group 2) and @ (AltGr level) are only in columns >= 2.
        assert_eq!(k.keycode(0xe4), None);
        assert_eq!(k.keycode(0x40), None);
    }
}
