//! XEmbed system tray icon (freedesktop System Tray protocol).
//!
//! The icon window is created unmapped, announced to the selection owner of
//! `_NET_SYSTEM_TRAY_S<screen>` with `REQUEST_DOCK`, and mapped by the tray
//! itself. If the tray advertises a 32-bit `_NET_SYSTEM_TRAY_VISUAL` the icon
//! is drawn with full alpha; otherwise the window uses a `ParentRelative`
//! background and only pixels with alpha >= 128 are painted (clip mask), so
//! there is no halo against the unknown tray background.

use super::X;
use crate::layout::Rect;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, BackPixmap, ClientMessageEvent, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    EventMask, ImageFormat, ImageOrder, PropMode, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

const ICONS: [(u32, &[u8]); 7] = [
    (16, include_bytes!("../../assets/icons/16.png")),
    (24, include_bytes!("../../assets/icons/24.png")),
    (32, include_bytes!("../../assets/icons/32.png")),
    (48, include_bytes!("../../assets/icons/48.png")),
    (64, include_bytes!("../../assets/icons/64.png")),
    (128, include_bytes!("../../assets/icons/128.png")),
    (256, include_bytes!("../../assets/icons/256.png")),
];

const REQUEST_DOCK: u32 = 0;
const DEFAULT_SIZE: u32 = 24;

/// Pixel size of the embedded PNG used for `size`: the smallest one that is
/// at least `size`, else the largest.
pub fn source_size(size: u32) -> u32 {
    pick(size).0
}

fn pick(size: u32) -> (u32, &'static [u8]) {
    ICONS
        .iter()
        .copied()
        .find(|(s, _)| *s >= size)
        .unwrap_or(ICONS[ICONS.len() - 1])
}

/// The icon scaled (bicubic) to `size` x `size`.
pub fn icon_for_size(size: u32) -> Pixmap {
    let size = size.max(1);
    let (src_size, png) = pick(size);
    let src = Pixmap::decode_png(png).expect("embedded icon PNG is valid");
    if src_size == size {
        return src;
    }
    let mut out = Pixmap::new(size, size).expect("non-zero size");
    let scale = size as f32 / src.width() as f32;
    let paint = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    out.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &paint,
        Transform::from_scale(scale, scale),
        None,
    );
    out
}

/// A `w` x `h` tray slot: the icon at the slot's smaller side, centred,
/// transparent around it.
pub fn slot_icon(w: u32, h: u32) -> Pixmap {
    let (w, h) = (w.max(1), h.max(1));
    let size = w.min(h);
    let icon = icon_for_size(size);
    if (w, h) == (size, size) {
        return icon;
    }
    let Some(mut out) = Pixmap::new(w, h) else {
        return icon;
    };
    out.draw_pixmap(
        ((w - size) / 2) as i32,
        ((h - size) / 2) as i32,
        icon.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    out
}

/// The last slot image, rebuilt only when the slot size changes (decoding and
/// rescaling the PNG on every Expose is wasted work).
#[derive(Default)]
struct SlotCache {
    img: Option<Pixmap>,
}

impl SlotCache {
    fn get(&mut self, w: u32, h: u32) -> &Pixmap {
        let (w, h) = (w.max(1), h.max(1));
        match &self.img {
            Some(p) if (p.width(), p.height()) == (w, h) => {}
            _ => self.img = Some(slot_icon(w, h)),
        }
        self.img.get_or_insert_with(|| slot_icon(w, h))
    }
}

#[derive(Debug, PartialEq)]
pub enum TrayEvent {
    /// Left click.
    Activate,
    /// Right click, in root coordinates.
    Menu { root_x: i32, root_y: i32 },
    /// The pointer entered the icon; its rectangle in root coordinates.
    Enter { icon: Rect },
    /// The pointer left the icon.
    Leave,
}

struct Docked {
    win: Window,
    gc: u32,
    /// Colormap we created for a foreign ARGB visual (freed with the window).
    own_colormap: Option<u32>,
    argb: bool,
    /// Window size (width, height) the tray gave us.
    slot: (u32, u32),
    icon: SlotCache,
}

pub struct Tray {
    selection: u32,
    owner: Window,
    visible: bool,
    docked: Option<Docked>,
}

impl Tray {
    pub fn new(x: &X) -> anyhow::Result<Tray> {
        let name = format!("_NET_SYSTEM_TRAY_S{}", x.screen_num);
        let selection = x.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        // MANAGER announcements arrive as StructureNotify client messages on
        // the root; keep whatever mask the connection already selected.
        let current = x
            .conn
            .get_window_attributes(x.root)?
            .reply()?
            .your_event_mask;
        x.conn.change_window_attributes(
            x.root,
            &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                .event_mask(current | EventMask::STRUCTURE_NOTIFY),
        )?;
        let mut t = Tray {
            selection,
            owner: 0,
            visible: true,
            docked: None,
        };
        t.find_owner(x);
        t.dock(x);
        x.conn.flush()?;
        Ok(t)
    }

    /// Whether a tray manager currently owns the selection.
    pub fn has_owner(&self) -> bool {
        self.owner != 0
    }

    /// Whether an icon window currently exists (docked or awaiting embedding).
    pub fn is_docked(&self) -> bool {
        self.docked.is_some()
    }

    pub fn set_visible(&mut self, x: &X, on: bool) {
        if on == self.visible {
            return;
        }
        self.visible = on;
        if on {
            self.dock(x);
        } else {
            self.undock(x);
        }
        let _ = x.conn.flush();
    }

    pub fn handle(&mut self, x: &X, ev: &Event) -> Option<TrayEvent> {
        match ev {
            Event::ClientMessage(e)
                if e.window == x.root
                    && e.type_ == x.atoms.MANAGER
                    && e.format == 32
                    && e.data.as_data32()[1] == self.selection =>
            {
                self.owner = e.data.as_data32()[2];
                self.watch_owner(x);
                // A new manager wants fresh clients.
                self.undock(x);
                self.dock(x);
            }
            Event::DestroyNotify(e) if self.owner != 0 && e.window == self.owner => {
                self.owner = 0;
                self.undock(x);
            }
            Event::DestroyNotify(e) if self.docked.as_ref().is_some_and(|d| d.win == e.window) => {
                // The tray tore our window down; wait for the next MANAGER.
                if let Some(d) = self.docked.take() {
                    release(x, &d, false);
                }
            }
            Event::ConfigureNotify(e) => {
                if let Some(d) = self.docked.as_mut().filter(|d| d.win == e.window) {
                    let slot = (u32::from(e.width).max(1), u32::from(e.height).max(1));
                    if slot != d.slot {
                        d.slot = slot;
                        self.repaint(x);
                    }
                }
            }
            Event::Expose(e) if e.count == 0 => {
                if self.docked.as_ref().is_some_and(|d| d.win == e.window) {
                    self.repaint(x);
                }
            }
            Event::EnterNotify(e) => {
                let d = self.docked.as_ref().filter(|d| d.win == e.event)?;
                // The pointer's root and window positions give the icon's origin.
                return Some(TrayEvent::Enter {
                    icon: Rect {
                        x: i32::from(e.root_x) - i32::from(e.event_x),
                        y: i32::from(e.root_y) - i32::from(e.event_y),
                        w: d.slot.0 as i32,
                        h: d.slot.1 as i32,
                    },
                });
            }
            Event::LeaveNotify(e) if self.docked.as_ref().is_some_and(|d| d.win == e.event) => {
                return Some(TrayEvent::Leave);
            }
            Event::ButtonPress(e) if self.docked.as_ref().is_some_and(|d| d.win == e.event) => {
                return match e.detail {
                    1 => Some(TrayEvent::Activate),
                    3 => Some(TrayEvent::Menu {
                        root_x: e.root_x.into(),
                        root_y: e.root_y.into(),
                    }),
                    _ => None,
                };
            }
            _ => {}
        }
        None
    }

    fn find_owner(&mut self, x: &X) {
        self.owner = x
            .conn
            .get_selection_owner(self.selection)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or(0, |r| r.owner);
        self.watch_owner(x);
    }

    fn watch_owner(&mut self, x: &X) {
        if self.owner == 0 {
            return;
        }
        let ok = x
            .conn
            .change_window_attributes(
                self.owner,
                &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .ok()
            .is_some_and(|c| c.check().is_ok());
        if !ok {
            self.owner = 0;
        }
    }

    /// The 32-bit visual the tray asks icons to use, if any.
    fn tray_visual(&self, x: &X) -> Option<u32> {
        let r = x
            .conn
            .get_property(
                false,
                self.owner,
                x.atoms._NET_SYSTEM_TRAY_VISUAL,
                AtomEnum::VISUALID,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        let id = r.value32()?.next()?;
        x.conn.setup().roots[x.screen_num]
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| &d.visuals)
            .any(|v| v.visual_id == id)
            .then_some(id)
    }

    fn dock(&mut self, x: &X) {
        if !self.visible || self.owner == 0 || self.docked.is_some() {
            return;
        }
        match self.create_icon_window(x) {
            Ok(d) => {
                let win = d.win;
                self.docked = Some(d);
                self.repaint(x);
                let msg = ClientMessageEvent::new(
                    32,
                    self.owner,
                    x.atoms._NET_SYSTEM_TRAY_OPCODE,
                    [0, REQUEST_DOCK, win, 0, 0],
                );
                let _ = x
                    .conn
                    .send_event(false, self.owner, EventMask::NO_EVENT, msg);
                let _ = x.conn.flush();
            }
            Err(e) => eprintln!("warning: cannot create tray icon window: {e:#}"),
        }
    }

    fn create_icon_window(&self, x: &X) -> anyhow::Result<Docked> {
        let screen = &x.conn.setup().roots[x.screen_num];
        let size = DEFAULT_SIZE;
        let win = x.conn.generate_id()?;
        let events = EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::ENTER_WINDOW
            | EventMask::LEAVE_WINDOW;
        let mut own_colormap = None;
        let argb = if let Some(vis) = self.tray_visual(x) {
            let cmap = if vis == x.argb_visual {
                x.colormap
            } else {
                let c = x.conn.generate_id()?;
                x.conn.create_colormap(
                    x11rb::protocol::xproto::ColormapAlloc::NONE,
                    c,
                    x.root,
                    vis,
                )?;
                own_colormap = Some(c);
                c
            };
            x.conn.create_window(
                32,
                win,
                x.root,
                0,
                0,
                size as u16,
                size as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                vis,
                &CreateWindowAux::new()
                    .background_pixel(0)
                    .border_pixel(0)
                    .colormap(cmap)
                    .event_mask(events),
            )?;
            true
        } else {
            x.conn.create_window(
                screen.root_depth,
                win,
                x.root,
                0,
                0,
                size as u16,
                size as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .background_pixmap(BackPixmap::PARENT_RELATIVE)
                    .event_mask(events),
            )?;
            false
        };
        let gc = x.conn.generate_id()?;
        x.conn
            .create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0))?;
        x.conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"timehud\0timehud\0",
        )?;
        x.conn.change_property8(
            PropMode::REPLACE,
            win,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"TimeHUD",
        )?;
        // XEmbed info: version 0, flags XEMBED_MAPPED.
        x.conn.change_property32(
            PropMode::REPLACE,
            win,
            x.atoms._XEMBED_INFO,
            x.atoms._XEMBED_INFO,
            &[0, 1],
        )?;
        Ok(Docked {
            win,
            gc,
            own_colormap,
            argb,
            slot: (size, size),
            icon: SlotCache::default(),
        })
    }

    fn undock(&mut self, x: &X) {
        if let Some(d) = self.docked.take() {
            release(x, &d, true);
        }
        let _ = x.conn.flush();
    }

    fn repaint(&mut self, x: &X) {
        let Some(d) = &mut self.docked else { return };
        let (win, gc, argb) = (d.win, d.gc, d.argb);
        let icon = d.icon.get(d.slot.0, d.slot.1);
        let res = if argb {
            x.present(win, gc, icon)
        } else {
            paint_thresholded(x, win, gc, icon)
        };
        if let Err(e) = res {
            eprintln!("warning: tray icon paint failed: {e:#}");
        }
    }
}

fn release(x: &X, d: &Docked, destroy: bool) {
    let _ = x.conn.free_gc(d.gc);
    if destroy {
        let _ = x.conn.destroy_window(d.win);
    }
    if let Some(c) = d.own_colormap {
        let _ = x.conn.free_colormap(c);
    }
}

/// Paints only pixels with alpha >= 128 onto a cleared (ParentRelative)
/// window, using a 1-bit clip mask and an opaque root-depth image.
fn paint_thresholded(x: &X, win: Window, gc: u32, icon: &Pixmap) -> anyhow::Result<()> {
    let setup = x.conn.setup();
    let screen = &setup.roots[x.screen_num];
    let depth = screen.root_depth;
    let bpp = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == depth)
        .map_or(0, |f| f.bits_per_pixel);
    x.conn.clear_area(false, win, 0, 0, 0, 0)?;
    if bpp != 32 {
        x.conn.flush()?;
        return Ok(()); // unsupported root format: leave the window blank
    }
    let (w, h) = (icon.width() as usize, icon.height() as usize);
    let (mask, _) = threshold_mask(
        icon,
        setup.bitmap_format_scanline_pad,
        setup.bitmap_format_bit_order == ImageOrder::LSB_FIRST,
    );
    let clip = x.conn.generate_id()?;
    x.conn.create_pixmap(1, clip, win, w as u16, h as u16)?;
    let mgc = x.conn.generate_id()?;
    x.conn.create_gc(mgc, clip, &CreateGCAux::new())?;
    x.conn.put_image(
        // XY_PIXMAP copies bits literally; XY_BITMAP would colour 1-bits with the
        // GC foreground (default 0) and invert the mask.
        ImageFormat::XY_PIXMAP,
        clip,
        mgc,
        w as u16,
        h as u16,
        0,
        0,
        0,
        1,
        &mask,
    )?;
    // Demultiplied, opaque BGRX pixels (LSBFirst, 32 bpp).
    let mut buf = Vec::with_capacity(w * h * 4);
    for &[r, g, b, a] in icon.data().as_chunks::<4>().0 {
        let un = |c: u8| {
            if a == 0 {
                0
            } else {
                (u32::from(c) * 255 / u32::from(a)).min(255) as u8
            }
        };
        buf.extend_from_slice(&[un(b), un(g), un(r), 0]);
    }
    x.conn.change_gc(
        gc,
        &x11rb::protocol::xproto::ChangeGCAux::new().clip_mask(clip),
    )?;
    let stride = w * 4;
    let rows = (x.conn.maximum_request_bytes().saturating_sub(64) / stride).clamp(1, h);
    for y0 in (0..h).step_by(rows) {
        let n = rows.min(h - y0);
        x.conn.put_image(
            ImageFormat::Z_PIXMAP,
            win,
            gc,
            w as u16,
            n as u16,
            0,
            y0 as i16,
            0,
            depth,
            &buf[y0 * stride..(y0 + n) * stride],
        )?;
    }
    x.conn.change_gc(
        gc,
        &x11rb::protocol::xproto::ChangeGCAux::new().clip_mask(0u32),
    )?;
    x.conn.free_pixmap(clip)?;
    x.conn.free_gc(mgc)?;
    x.conn.flush()?;
    Ok(())
}

/// 1-bit XYBitmap rows (padded to `pad` bits) of "alpha >= 128" pixels.
fn threshold_mask(icon: &Pixmap, pad: u8, lsb_first: bool) -> (Vec<u8>, usize) {
    let (w, h) = (icon.width() as usize, icon.height() as usize);
    let pitch = w.div_ceil(usize::from(pad)) * usize::from(pad) / 8;
    let mut out = vec![0u8; pitch * h];
    for (i, px) in icon.pixels().iter().enumerate() {
        if px.alpha() >= 128 {
            let (row, col) = (i / w, i % w);
            let bit = if lsb_first { col % 8 } else { 7 - col % 8 };
            out[row * pitch + col / 8] |= 1 << bit;
        }
    }
    (out, pitch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_sizes() {
        assert_eq!(source_size(22), 24);
        assert_eq!(source_size(24), 24);
        assert_eq!(source_size(300), 256);
        assert_eq!(source_size(1), 16);
        let p = icon_for_size(22);
        assert_eq!((p.width(), p.height()), (22, 22));
        assert!(p.pixels().iter().any(|px| px.alpha() > 0));
        assert_eq!(icon_for_size(300).width(), 300);
    }

    #[test]
    fn slot_icon_centred_in_non_square_slot() {
        let p = slot_icon(40, 24);
        assert_eq!((p.width(), p.height()), (40, 24));
        let icon = icon_for_size(24);
        // The 24 px icon sits at x = 8..32: columns outside are transparent
        // and the icon's pixels appear shifted by the offset.
        for y in 0..24 {
            for x in (0..8).chain(32..40) {
                assert_eq!(p.pixel(x, y).unwrap().alpha(), 0, "({x}, {y})");
            }
            for x in 0..24 {
                assert_eq!(p.pixel(x + 8, y), icon.pixel(x, y));
            }
        }
        let tall = slot_icon(22, 30);
        assert_eq!((tall.width(), tall.height()), (22, 30));
        assert_eq!(tall.pixel(11, 3).unwrap().alpha(), 0);
        assert_eq!(tall.pixel(11, 4), icon_for_size(22).pixel(11, 0));
    }

    #[test]
    fn slot_icon_square_is_plain_icon() {
        assert!(slot_icon(22, 22).data() == icon_for_size(22).data());
    }

    #[test]
    fn slot_cache_reuses_until_resized() {
        let mut c = SlotCache::default();
        let a = c.get(30, 22).data().as_ptr();
        assert_eq!(c.get(30, 22).data().as_ptr(), a);
        assert_eq!(c.get(24, 24).width(), 24);
    }

    #[test]
    fn threshold_mask_bits() {
        let mut p = Pixmap::new(10, 2).unwrap();
        // Premultiplied pixels: alpha 200 opaque-ish at x=0 and x=9, 100 at x=1.
        let px = |a: u8| tiny_skia::PremultipliedColorU8::from_rgba(a, a, a, a).unwrap();
        p.pixels_mut()[0] = px(200);
        p.pixels_mut()[1] = px(100);
        p.pixels_mut()[9] = px(128);
        let (m, pitch) = threshold_mask(&p, 32, true);
        assert_eq!(pitch, 4);
        assert_eq!(m[0], 0b0000_0001);
        assert_eq!(m[1], 0b0000_0010);
        let (m, _) = threshold_mask(&p, 32, false);
        assert_eq!(m[0], 0b1000_0000);
        assert_eq!(m[1], 0b0100_0000);
    }

    #[test]
    #[ignore = "needs an X display (DISPLAY=:0) with a system tray"]
    fn smoke_tray_docks() {
        let x = X::connect().unwrap();
        let mut t = Tray::new(&x).unwrap();
        eprintln!("owner = {:#x}, docked = {}", t.owner, t.is_docked());
        assert!(t.has_owner(), "no system tray manager on this display");
        assert!(t.is_docked());
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(3) {
            while let Ok(Some(ev)) = x.conn.poll_for_event() {
                let _ = t.handle(&x, &ev);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Centre (icon body) must differ from the corner (transparent -> bar bg).
        let d = t.docked.as_ref().unwrap();
        let (win, size) = (d.win, d.slot);
        let geo = x.conn.get_geometry(win).unwrap().reply().unwrap();
        let img = x
            .conn
            .get_image(ImageFormat::Z_PIXMAP, win, 0, 0, geo.width, geo.height, !0)
            .unwrap()
            .reply()
            .unwrap();
        let bpp = img.data.len() / (usize::from(geo.width) * usize::from(geo.height));
        let px = |cx: u16, cy: u16| {
            let i = (usize::from(cy) * usize::from(geo.width) + usize::from(cx)) * bpp;
            img.data[i..i + 3].to_vec()
        };
        let (centre, corner) = (px(geo.width / 2, geo.height / 2), px(0, 0));
        eprintln!("slot {size:?} centre {centre:?} corner {corner:?}");
        assert_ne!(centre, corner, "icon body not painted");
        t.set_visible(&x, false);
        assert!(!t.is_docked());
        x.conn.get_input_focus().unwrap().reply().unwrap();
    }
}
