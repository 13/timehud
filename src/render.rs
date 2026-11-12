//! Software overlay renderer: lays out the HUD rows and paints them into a tiny-skia pixmap.
use crate::config::Config;
use crate::layout::Rect;
use crate::text::FontSet;
use crate::themes::{Rgb, Theme};
use std::sync::atomic::{AtomicBool, Ordering};
use tiny_skia::{
    BlendMode, Color, FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Stroke, StrokeDash,
    Transform,
};

/// Everything needed to draw one frame; built by `app.rs`.
pub struct View<'a> {
    pub cfg: &'a Config,
    pub theme: &'static Theme,
    /// HiDPI factor.
    pub scale: f32,
    pub clock: String,
    pub timer: String,
    pub timer_color: Rgb,
    /// Pulse multiplier, 1.0..1.04.
    pub timer_px_mul: f32,
    pub mode_text: String,
    pub mode_color: Rgb,
    pub show_controls_row: bool,
    /// Fold position 0..1.
    pub controls_pos: f32,
    pub running: bool,
    pub mode_btn: &'static str,
    pub hover_btn: Option<Hit>,
    /// -1 = hidden.
    pub progress: f64,
    pub progress_color: Rgb,
    /// Window opacity x fade-in, baked into alpha.
    pub opacity: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Clock,
    Timer,
    Mode,
    BtnStart,
    BtnReset,
    BtnMode,
    Background,
}

pub struct Layout {
    pub w: i32,
    pub h: i32,
    /// Panel rect inside the shadow inset.
    pub panel: Rect,
    pub regions: Vec<(Hit, Rect)>,
}

impl Layout {
    pub fn hit(&self, x: i32, y: i32) -> Hit {
        self.regions
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map_or(Hit::Background, |(h, _)| *h)
    }
}

const SHADOW_LAYERS: u32 = 6;

/// Whether themes draw their drop shadow (see [`set_shadows`]).
static SHADOWS: AtomicBool = AtomicBool::new(true);

/// Turns the themes' drop shadow (and the window margin it needs) on or
/// off for every later `measure` and `render`. The HUD turns it off while
/// no compositing manager runs: then window alpha is ignored and the
/// shadow would show as a black box. Process-wide, like the display.
pub fn set_shadows(on: bool) {
    SHADOWS.store(on, Ordering::Relaxed);
}

/// A theme compared by identity (themes are statics).
#[derive(Clone, Copy)]
struct ThemeRef(&'static Theme);

impl PartialEq for ThemeRef {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

/// The `View` inputs of the static layer: everything but the bar's length
/// and colour and the opacity. It is both the cache key of [`Frame`] and the
/// only input of [`plan`] and [`draw_static`], so nothing they draw can be
/// missing from the key.
#[derive(Clone, PartialEq)]
struct StaticView {
    theme: ThemeRef,
    /// Draw the theme's shadow ([`set_shadows`]).
    shadow: bool,
    scale: f32,
    /// Not drawn from, but a new family means a different `FontSet`.
    font_family: String,
    font_size: i64,
    row_spacing: i64,
    padding: i64,
    padding_top: i64,
    show_clock: bool,
    show_timer: bool,
    /// `progress_style == "line"` (the bar gets its own row).
    progress_line: bool,
    color_bg: Rgb,
    color_clock: Rgb,
    clock: String,
    timer: String,
    timer_color: Rgb,
    timer_px_mul: f32,
    mode_text: String,
    mode_color: Rgb,
    show_controls_row: bool,
    controls_pos: f32,
    running: bool,
    mode_btn: &'static str,
    hover_btn: Option<Hit>,
    progress_shown: bool,
}

impl StaticView {
    fn of(v: &View) -> StaticView {
        let cfg = v.cfg;
        StaticView {
            theme: ThemeRef(v.theme),
            shadow: SHADOWS.load(Ordering::Relaxed),
            scale: v.scale,
            font_family: cfg.font_family.clone(),
            font_size: cfg.font_size,
            row_spacing: cfg.row_spacing,
            padding: cfg.padding,
            padding_top: cfg.padding_top,
            show_clock: cfg.show_clock,
            show_timer: cfg.show_timer,
            progress_line: cfg.progress_style == "line",
            color_bg: hex(&cfg.color_bg, Rgb(0, 0, 0)),
            color_clock: hex(&cfg.color_clock, Rgb(0, 255, 136)),
            clock: v.clock.clone(),
            timer: v.timer.clone(),
            timer_color: v.timer_color,
            timer_px_mul: v.timer_px_mul,
            mode_text: v.mode_text.clone(),
            mode_color: v.mode_color,
            show_controls_row: v.show_controls_row,
            controls_pos: v.controls_pos,
            running: v.running,
            mode_btn: v.mode_btn,
            hover_btn: v.hover_btn,
            progress_shown: v.progress >= 0.0,
        }
    }

    /// The shadow's alpha; 0 when there is none.
    fn shadow_alpha(&self) -> u8 {
        if self.shadow {
            self.theme.0.shadow_alpha
        } else {
            0
        }
    }

    fn pos(&self) -> f32 {
        self.controls_pos.clamp(0.0, 1.0)
    }

    /// Width of the mode row text, letter spacing included.
    fn mode_width(&self, fonts: &mut FontSet, px: f32, spacing: f32) -> f32 {
        let n = self.mode_text.chars().count().max(1) as f32;
        fonts.measure(&self.mode_text, px, true) + (n - 1.0) * spacing
    }
}

/// Resolved geometry shared by `measure` and `render`.
struct Plan {
    layout: Layout,
    s: f32,
    clock_px: f32,
    timer_px: f32,
    mode_px: f32,
    clock_baseline: Option<f32>,
    timer_row: Option<Rect>,
    sep_y: Option<i32>,
    progress_row: Option<Rect>,
    mode_row: Option<Rect>,
    mode_baseline: f32,
    ctrl_row: Option<Rect>,
    btn_rects: [Rect; 3],
}

fn px(v: f32, s: f32) -> i32 {
    (v * s).round() as i32
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

/// Button sizes at the unfolded position.
struct Buttons {
    h: i32,
    w: [i32; 3],
    gap: i32,
}

impl Buttons {
    fn new(sv: &StaticView) -> Buttons {
        let s = sv.scale;
        let h = px(((sv.font_size - 15).max(16)) as f32, s);
        Buttons {
            h,
            w: [h + px(3.0, s), h + px(3.0, s), h + px(8.0, s)],
            gap: px(6.0, s),
        }
    }

    fn total_w(&self) -> i32 {
        self.w.iter().sum::<i32>() + 2 * self.gap
    }

    /// The three button rects, centred in the panel `x..x + panel_w`, at
    /// `y`, height scaled by the fold position `pos`.
    fn rects(&self, x: i32, panel_w: i32, y: i32, pos: f32) -> [Rect; 3] {
        let mut bx = x + (panel_w - self.total_w()) / 2;
        let bh = (self.h as f32 * pos).round() as i32;
        self.w.map(|bw| {
            let r = rect(bx, y, bw, bh);
            bx += bw + self.gap;
            r
        })
    }
}

/// Stacks rows top to bottom with a uniform gap between them.
struct Stack {
    y: i32,
    gap: i32,
    first: bool,
}

impl Stack {
    /// Top of the next row of height `h`.
    fn next(&mut self, h: i32) -> i32 {
        if !self.first {
            self.y += self.gap;
        }
        self.first = false;
        let at = self.y;
        self.y += h;
        at
    }
}

fn plan(sv: &StaticView, fonts: &mut FontSet) -> Plan {
    let (theme, s) = (sv.theme.0, sv.scale);
    let fs = sv.font_size as f32;
    let clock_px = (fs * theme.clock_scale * s).floor().max(1.0);
    let timer_px = (fs * theme.timer_scale * s).floor().max(1.0);
    let mode_px = ((fs * 0.42).floor().max(11.0) * s).floor();
    let gap = px(sv.row_spacing as f32, s);
    let pad_top = if sv.padding_top >= 0 {
        sv.padding_top
    } else {
        sv.padding
    };
    let inset = if sv.shadow_alpha() > 0 {
        px(12.0, s)
    } else {
        0
    };
    let (mx, top) = (px((sv.padding + 4) as f32, s), px(pad_top as f32, s));

    let show_timer = sv.show_timer;
    let ctrl_on = show_timer && sv.show_controls_row;
    let pos = sv.pos();
    let bottom = if ctrl_on {
        (px(8.0, s) as f32 * pos + gap as f32 * (1.0 - pos)).round() as i32
    } else {
        gap
    };

    let clock_h = fonts.ink_height(clock_px).round() as i32;
    let timer_h = fonts.ink_height(timer_px).round() as i32 + 4;
    let mode_h = fonts.ink_height(mode_px).round() as i32;
    let buttons = Buttons::new(sv);
    let ctrl_inner_h = buttons.h + px(2.0, s);

    let mut content_w = 0f32;
    if sv.show_clock {
        content_w = content_w.max(fonts.measure(&sv.clock, clock_px, true));
    }
    if show_timer {
        content_w = content_w.max(fonts.measure(&sv.timer, timer_px, true));
        content_w = content_w.max(sv.mode_width(fonts, mode_px, 2.0 * s));
    }
    if ctrl_on {
        content_w = content_w.max(buttons.total_w() as f32);
    }
    let w = content_w.ceil() as i32 + 2 * mx + 2 * inset;
    let panel_w = w - 2 * inset;
    let row_w = panel_w - 2 * mx;
    let row = |y: i32, h: i32| rect(inset + mx, y, row_w, h);

    let mut stack = Stack {
        y: inset + top,
        gap,
        first: true,
    };
    let mut clock_baseline = None;
    let mut clock_row = None;
    if sv.show_clock {
        let at = stack.next(clock_h);
        clock_baseline = Some((at + clock_h) as f32);
        clock_row = Some(row(at, clock_h));
    }
    let mut sep_y = None;
    let mut timer_row = None;
    let mut progress_row = None;
    let mut mode_row = None;
    let mut ctrl_row = None;
    let mut mode_baseline = 0.0;
    if show_timer {
        if theme.show_separator {
            sep_y = Some(stack.next(1));
        }
        timer_row = Some(row(stack.next(timer_h), timer_h));
        if sv.progress_line && sv.progress_shown {
            progress_row = Some(row(stack.next(px(3.0, s)), px(3.0, s)));
        }
        let at = stack.next(mode_h);
        mode_baseline = (at + mode_h) as f32;
        mode_row = Some(row(at, mode_h));
        if ctrl_on {
            let h = (ctrl_inner_h as f32 * pos).round() as i32;
            ctrl_row = Some(row(stack.next(h), h));
        }
    }
    let h = stack.y + bottom + inset;

    // Buttons: centred, full size at the unfolded position.
    let by = ctrl_row.map_or(0, |r| r.y + (px(2.0, s) as f32 * pos).round() as i32);
    let btn_rects = buttons.rects(inset, panel_w, by, pos);

    let mut regions = Vec::new();
    if ctrl_row.is_some() {
        regions.extend(
            [Hit::BtnStart, Hit::BtnReset, Hit::BtnMode]
                .into_iter()
                .zip(btn_rects),
        );
    }
    regions.extend(clock_row.map(|r| (Hit::Clock, r)));
    regions.extend(timer_row.map(|r| (Hit::Timer, r)));
    regions.extend(mode_row.map(|r| (Hit::Mode, r)));
    Plan {
        layout: Layout {
            w,
            h,
            panel: rect(inset, inset, panel_w, h - 2 * inset),
            regions,
        },
        s,
        clock_px,
        timer_px,
        mode_px,
        clock_baseline,
        timer_row,
        sep_y,
        progress_row,
        mode_row,
        mode_baseline,
        ctrl_row,
        btn_rects,
    }
}

/// Window size and hit regions, without drawing.
pub fn measure(view: &View, fonts: &mut FontSet) -> Layout {
    plan(&StaticView::of(view), fonts).layout
}

fn color(c: Rgb, a: f32) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn white(alpha: u8) -> Color {
    Color::from_rgba8(255, 255, 255, alpha)
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn hex(s: &str, fallback: Rgb) -> Rgb {
    crate::themes::parse_hex(s).unwrap_or(fallback)
}

const KAPPA: f32 = 0.552_284_8;

/// Rounded rect outline starting at the top-centre and running clockwise.
fn rounded_outline(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = r * KAPPA;
    let (l, t, rt, b) = (x, y, x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(l + w / 2.0, t);
    pb.line_to(rt - r, t);
    pb.cubic_to(rt - r + k, t, rt, t + r - k, rt, t + r);
    pb.line_to(rt, b - r);
    pb.cubic_to(rt, b - r + k, rt - r + k, b, rt - r, b);
    pb.line_to(l + r, b);
    pb.cubic_to(l + r - k, b, l, b - r + k, l, b - r);
    pb.line_to(l, t + r);
    pb.cubic_to(l, t + r - k, l + r - k, t, l + r, t);
    pb.close();
    pb.finish()
}

fn fill_rrect(pm: &mut Pixmap, r: (f32, f32, f32, f32), rad: f32, p: &Paint) {
    if let Some(path) = rounded_outline(r.0, r.1, r.2, r.3, rad) {
        pm.fill_path(&path, p, FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke(pm: &mut Pixmap, path: &Path, p: &Paint, width: f32) {
    let st = Stroke {
        width,
        ..Stroke::default()
    };
    pm.stroke_path(path, p, &st, Transform::identity(), None);
}

fn draw_shadow(pm: &mut Pixmap, panel: Rect, inset: i32, radius: f32, alpha: u8) {
    let a = alpha as f32 / 255.0 / SHADOW_LAYERS as f32 * 1.6;
    let p = paint(Color::from_rgba(0.0, 0.0, 0.0, a.min(1.0)).unwrap_or(Color::BLACK));
    for i in 0..SHADOW_LAYERS {
        let grow = inset as f32 * (1.0 - i as f32 / SHADOW_LAYERS as f32);
        fill_rrect(
            pm,
            (
                panel.x as f32 - grow,
                panel.y as f32 - grow + inset as f32 * 0.15,
                panel.w as f32 + 2.0 * grow,
                panel.h as f32 + 2.0 * grow,
            ),
            radius + grow,
            &p,
        );
    }
    // The panel is translucent: keep the shadow out from underneath it.
    let mut clear = paint(Color::BLACK);
    clear.blend_mode = BlendMode::Clear;
    fill_rrect(
        pm,
        (
            panel.x as f32,
            panel.y as f32,
            panel.w as f32,
            panel.h as f32,
        ),
        radius,
        &clear,
    );
}

fn draw_icon(pm: &mut Pixmap, kind: Hit, running: bool, r: Rect, c: Color, s: f32) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let u = (r.h as f32).min(r.w as f32) * 0.5; // icon half-extent budget
    let p = paint(c);
    let mut pb = PathBuilder::new();
    match kind {
        Hit::BtnStart if running => {
            let (bw, hh, off) = (u * 0.28, u * 0.52, u * 0.2);
            for sx in [-1.0f32, 1.0] {
                let x0 = cx + sx * off - bw / 2.0;
                if let Some(rc) = tiny_skia::Rect::from_xywh(x0, cy - hh, bw, hh * 2.0) {
                    pb.push_rect(rc);
                }
            }
        }
        Hit::BtnStart => {
            let (hw, hh) = (u * 0.45, u * 0.55);
            pb.move_to(cx - hw * 0.7, cy - hh);
            pb.line_to(cx + hw * 1.1, cy);
            pb.line_to(cx - hw * 0.7, cy + hh);
            pb.close();
        }
        _ => {
            // Counter-clockwise circular arrow with a filled head.
            let rad = u * 0.52;
            let steps = 24;
            let (a0, sweep) = (-30f32.to_radians(), 270f32.to_radians());
            for i in 0..=steps {
                let th = a0 - sweep * i as f32 / steps as f32;
                let (x, y) = (cx + rad * th.cos(), cy + rad * th.sin());
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            if let Some(path) = pb.finish() {
                let st = Stroke {
                    width: (1.6 * s).max(1.2),
                    line_cap: LineCap::Butt,
                    ..Stroke::default()
                };
                pm.stroke_path(&path, &p, &st, Transform::identity(), None);
            }
            let th = a0 - sweep;
            let (ex, ey) = (cx + rad * th.cos(), cy + rad * th.sin());
            let (tx, ty) = (th.sin(), -th.cos());
            let (nx, ny) = (th.cos(), th.sin());
            let (hl, hw) = (u * 0.42, u * 0.3);
            let mut hb = PathBuilder::new();
            hb.move_to(ex + tx * hl, ey + ty * hl);
            hb.line_to(ex + nx * hw, ey + ny * hw);
            hb.line_to(ex - nx * hw, ey - ny * hw);
            hb.close();
            if let Some(path) = hb.finish() {
                pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
            }
            return;
        }
    }
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
}

fn draw_buttons(sv: &StaticView, pl: &Plan, fonts: &mut FontSet, pm: &mut Pixmap) {
    let pos = sv.pos();
    if pl.ctrl_row.is_none() || pos <= 0.0 {
        return;
    }
    let s = pl.s;
    let radius = 4.0 * s;
    let hits = [Hit::BtnStart, Hit::BtnReset, Hit::BtnMode];
    for (hit, r) in hits.into_iter().zip(pl.btn_rects) {
        if r.h <= 0 {
            continue;
        }
        let hover = sv.hover_btn == Some(hit);
        let bg = if hover { 38 } else { 18 };
        let a = |v: u8| (v as f32 * pos).round() as u8;
        fill_rrect(
            pm,
            (r.x as f32, r.y as f32, r.w as f32, r.h as f32),
            radius,
            &paint(white(a(bg))),
        );
        if let Some(path) = rounded_outline(
            r.x as f32 + 0.5,
            r.y as f32 + 0.5,
            r.w as f32 - 1.0,
            r.h as f32 - 1.0,
            radius,
        ) {
            stroke(pm, &path, &paint(white(a(35))), 1.0);
        }
        let fg = if hover { 255 } else { 204 };
        let fg = Color::from_rgba8(fg, fg, fg, a(255));
        if hit == Hit::BtnMode {
            let bpx = (10.0 * s).floor().max(1.0);
            let tw = fonts.measure(sv.mode_btn, bpx, false);
            let ink = fonts.ink_height(bpx);
            let x = r.x as f32 + (r.w as f32 - tw) / 2.0;
            let base = r.y as f32 + (r.h as f32 + ink) / 2.0;
            fonts.draw(pm, sv.mode_btn, x, base, bpx, fg, false, 0.0);
        } else {
            draw_icon(pm, hit, sv.running, r, fg, s);
        }
    }
}

fn draw_border_progress(pm: &mut Pixmap, view: &View, panel: Rect, radius: f32) {
    let frac = view.progress.min(1.0) as f32;
    let (x, y) = (panel.x as f32 + 1.0, panel.y as f32 + 1.0);
    let (w, h) = (panel.w as f32 - 2.0, panel.h as f32 - 2.0);
    let Some(path) = rounded_outline(x, y, w, h, radius) else {
        return;
    };
    let r = radius.min(w / 2.0).min(h / 2.0);
    let total = 2.0 * (w - 2.0 * r) + 2.0 * (h - 2.0 * r) + 2.0 * std::f32::consts::PI * r;
    let width = 2.0 * view.scale.max(1.0);
    let mut st = Stroke {
        width,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    if frac < 1.0 {
        st.dash = StrokeDash::new(vec![(total * frac).max(0.01), total + width], 0.0);
    }
    let p = paint(color(view.progress_color, 1.0));
    pm.stroke_path(&path, &p, &st, Transform::identity(), None);
}

/// Draw one frame, opacity applied, into a new pixmap; `None` if no pixmap
/// of the layout's size can be allocated.
pub fn render(view: &View, fonts: &mut FontSet) -> Option<(Pixmap, Layout)> {
    let mut frame = Frame::default();
    let layout = render_into(view, fonts, &mut frame);
    let o = frame.opacity();
    frame.pixmap.map(|mut pm| {
        if let Some(lut) = opacity_lut(o) {
            for b in pm.data_mut() {
                *b = lut[usize::from(*b)];
            }
        }
        (pm, layout)
    })
}

/// Byte table scaling premultiplied channels by the opacity `o` (same
/// rounding as scaling each byte); `None` when `o` is 1 (nothing to do).
pub fn opacity_lut(o: f32) -> Option<[u8; 256]> {
    let o = o.clamp(0.0, 1.0);
    (o < 1.0).then(|| std::array::from_fn(|b| (b as f32 * o).round() as u8))
}

/// A reusable frame: the drawn `pixmap` plus a cache of everything but the
/// progress bar and opacity, so frames where only the bar moved (the common
/// case while running) skip layout, text and panel drawing.
#[derive(Default)]
pub struct Frame {
    /// The last frame drawn, *without* the opacity (see [`Frame::opacity`]);
    /// `None` if no pixmap could be allocated.
    pub pixmap: Option<Pixmap>,
    base: Option<Pixmap>,
    key: Option<StaticView>,
    plan: Option<Plan>,
    opacity: f32,
}

impl Frame {
    /// The last frame's window opacity (0..1), for the presenter to apply
    /// to `pixmap` (only the pixels it uploads, see [`opacity_lut`]).
    pub fn opacity(&self) -> f32 {
        self.opacity
    }
}

/// Copies `src` into `dst`, reusing `dst`'s pixmap when the size matches.
fn copy_into(dst: &mut Option<Pixmap>, src: &Pixmap) {
    match dst {
        Some(d) if d.width() == src.width() && d.height() == src.height() => {
            d.data_mut().copy_from_slice(src.data());
        }
        _ => *dst = Some(src.clone()),
    }
}

/// Draw one frame (bar included, opacity only recorded) into
/// `frame.pixmap`, redrawing the static layer only when
/// a [`StaticView`] input changed. Leaves `None` when no pixmap of the
/// layout's size can be allocated; the layout is returned anyway.
pub fn render_into(view: &View, fonts: &mut FontSet, frame: &mut Frame) -> Layout {
    let sv = StaticView::of(view);
    let cached = frame.base.is_some() && frame.plan.is_some() && frame.key.as_ref() == Some(&sv);
    if !cached {
        let pl = plan(&sv, fonts);
        let (w, h) = (pl.layout.w.max(1) as u32, pl.layout.h.max(1) as u32);
        match &mut frame.base {
            Some(pm) if pm.width() == w && pm.height() == h => pm.fill(Color::TRANSPARENT),
            base => *base = Pixmap::new(w, h),
        }
        if let Some(base) = &mut frame.base {
            draw_static(&sv, &pl, fonts, base);
            frame.key = Some(sv);
        } else {
            frame.key = None;
        }
        frame.plan = Some(pl);
    }
    let pl = frame.plan.as_ref().expect("plan set above");
    let lay = &pl.layout;
    let layout = Layout {
        w: lay.w,
        h: lay.h,
        panel: lay.panel,
        regions: lay.regions.clone(),
    };
    match &frame.base {
        Some(base) => copy_into(&mut frame.pixmap, base),
        None => frame.pixmap = None,
    }
    if let Some(pm) = &mut frame.pixmap {
        draw_dynamic(view, pl, pm);
    }
    frame.opacity = view.opacity.clamp(0.0, 1.0);
    layout
}

/// Everything but the progress bar fill and the opacity.
fn draw_static(sv: &StaticView, pl: &Plan, fonts: &mut FontSet, pm: &mut Pixmap) {
    draw_panel(sv, pl, pm);
    draw_rows(sv, pl, fonts, pm);
    draw_buttons(sv, pl, fonts, pm);
}

/// Shadow, background, border, top edge and separator.
fn draw_panel(sv: &StaticView, pl: &Plan, pm: &mut Pixmap) {
    let (theme, s) = (sv.theme.0, pl.s);
    let panel = pl.layout.panel;
    let rad = theme.radius * s;
    let pf = (
        panel.x as f32,
        panel.y as f32,
        panel.w as f32,
        panel.h as f32,
    );
    if sv.shadow_alpha() > 0 {
        draw_shadow(pm, panel, panel.x, rad, sv.shadow_alpha());
    }
    let bg = sv.color_bg;
    fill_rrect(
        pm,
        pf,
        rad,
        &paint(Color::from_rgba8(bg.0, bg.1, bg.2, theme.bg_alpha)),
    );
    if theme.border_alpha > 0
        && let Some(path) = rounded_outline(pf.0 + 0.5, pf.1 + 0.5, pf.2 - 1.0, pf.3 - 1.0, rad)
    {
        stroke(pm, &path, &paint(white(theme.border_alpha)), 1.0);
    }
    if theme.top_edge_alpha > 0 {
        let mut pb = PathBuilder::new();
        pb.move_to(pf.0 + rad, pf.1 + 1.5);
        pb.line_to(pf.0 + pf.2 - rad, pf.1 + 1.5);
        if let Some(path) = pb.finish() {
            stroke(pm, &path, &paint(white(theme.top_edge_alpha)), 1.0);
        }
    }
    if let Some(y) = pl.sep_y {
        let mx = (sv.padding + 4) as f32 * s;
        if let Some(rc) = tiny_skia::Rect::from_xywh(pf.0 + mx, y as f32, pf.2 - 2.0 * mx, 1.0) {
            pm.fill_rect(rc, &paint(white(35)), Transform::identity(), None);
        }
    }
}

/// Clock, timer, the bar's track and the mode text, centred in the panel.
fn draw_rows(sv: &StaticView, pl: &Plan, fonts: &mut FontSet, pm: &mut Pixmap) {
    let panel = pl.layout.panel;
    let centre = panel.x as f32 + panel.w as f32 / 2.0;
    if let Some(base) = pl.clock_baseline {
        let c = color(sv.color_clock, sv.theme.0.clock_alpha);
        let tw = fonts.measure(&sv.clock, pl.clock_px, true);
        fonts.draw(
            pm,
            &sv.clock,
            centre - tw / 2.0,
            base,
            pl.clock_px,
            c,
            true,
            0.0,
        );
    }
    if let Some(row) = pl.timer_row {
        // Pulse: integer px size, scaled about the row centre; other rows stay put.
        let tpx = (pl.timer_px * sv.timer_px_mul).round().max(1.0);
        let ink = fonts.ink_height(tpx);
        let base = row.y as f32 + (row.h as f32 + ink) / 2.0;
        let tw = fonts.measure(&sv.timer, tpx, true);
        let c = color(sv.timer_color, 1.0);
        fonts.draw(pm, &sv.timer, centre - tw / 2.0, base, tpx, c, true, 0.0);
    }
    if let Some(row) = pl.progress_row {
        let r = (row.x as f32, row.y as f32, row.w as f32, row.h as f32);
        fill_rrect(pm, r, 1.5 * pl.s, &paint(white(30)));
    }
    if pl.mode_row.is_some() {
        let ls = 2.0 * pl.s;
        let tw = sv.mode_width(fonts, pl.mode_px, ls);
        let c = color(sv.mode_color, 1.0);
        let x = centre - tw / 2.0;
        fonts.draw(
            pm,
            &sv.mode_text,
            x,
            pl.mode_baseline,
            pl.mode_px,
            c,
            true,
            ls,
        );
    }
}

/// The progress bar fill (line or border).
fn draw_dynamic(view: &View, pl: &Plan, pm: &mut Pixmap) {
    let s = pl.s;
    if let Some(row) = pl.progress_row {
        let r = (row.x as f32, row.y as f32, row.w as f32, row.h as f32);
        let fw = (r.2 * view.progress.min(1.0) as f32).floor();
        if fw > 0.0 {
            fill_rrect(
                pm,
                (r.0, r.1, fw, r.3),
                1.5 * s,
                &paint(color(view.progress_color, 1.0)),
            );
        }
    }
    if view.cfg.progress_style == "border" && view.progress > 0.0 {
        let rad = view.theme.radius * s;
        draw_border_progress(pm, view, pl.layout.panel, rad);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::get_theme;

    fn mk<'a>(cfg: &'a Config, theme: &str) -> View<'a> {
        View {
            cfg,
            theme: get_theme(theme),
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
        }
    }

    fn fonts() -> FontSet {
        FontSet::bundled()
    }

    #[test]
    fn default_size_reasonable() {
        let cfg = Config::default();
        let l = measure(&mk(&cfg, "classic"), &mut fonts());
        assert!(l.h > 40 && l.h < 300, "h={}", l.h);
        assert!(l.w > 100 && l.w < 500, "w={}", l.w);
    }

    #[test]
    fn fold_shrinks_height() {
        let cfg = Config::default();
        let mut v = mk(&cfg, "classic");
        let full = measure(&v, &mut fonts()).h;
        v.controls_pos = 0.0;
        assert!(measure(&v, &mut fonts()).h < full - 10);
    }

    #[test]
    fn padding_top_override() {
        let mut cfg = Config::default();
        let base = measure(&mk(&cfg, "classic"), &mut fonts()).h;
        cfg.padding_top = 2;
        assert!(measure(&mk(&cfg, "classic"), &mut fonts()).h < base);
    }

    #[test]
    fn pulse_does_not_move_mode_row() {
        let cfg = Config::default();
        let mut v = mk(&cfg, "classic");
        let mode = |v: &View| {
            let l = measure(v, &mut fonts());
            l.regions.iter().find(|(h, _)| *h == Hit::Mode).unwrap().1
        };
        let a = mode(&v);
        v.timer_px_mul = 1.04;
        assert_eq!(a, mode(&v));
    }

    #[test]
    fn hit_regions() {
        let cfg = Config::default();
        let l = measure(&mk(&cfg, "classic"), &mut fonts());
        let t = l.regions.iter().find(|(h, _)| *h == Hit::Timer).unwrap().1;
        assert_eq!(l.hit(t.x + t.w / 2, t.y + t.h / 2), Hit::Timer);
        assert_eq!(l.hit(l.panel.x + 1, l.panel.y + 1), Hit::Background);
    }

    #[test]
    fn render_into_reuses_pixmap() {
        let cfg = Config::default();
        let mut v = mk(&cfg, "classic");
        let mut frame = Frame::default();
        render_into(&v, &mut fonts(), &mut frame);
        let addr = frame.pixmap.as_ref().unwrap().data().as_ptr();
        v.timer = "12:34".into();
        render_into(&v, &mut fonts(), &mut frame);
        let pm = frame.pixmap.as_ref().unwrap();
        assert_eq!(pm.data().as_ptr(), addr, "same-size frame reallocated");

        v.controls_pos = 0.0; // fold: smaller window
        let l = render_into(&v, &mut fonts(), &mut frame);
        let pm = frame.pixmap.as_ref().unwrap();
        assert_eq!((pm.width(), pm.height()), (l.w as u32, l.h as u32));
    }

    /// Frames drawn over the cached static layer equal fresh renders, for
    /// every theme, both bar styles, HiDPI, and changes in and out of the key.
    #[test]
    fn cached_frames_match_fresh() {
        let mut f = fonts();
        for t in crate::themes::THEMES.iter() {
            for style in ["line", "border"] {
                // HiDPI once; debug-build rendering is slow.
                let scales: &[f32] = if t.name == "classic" {
                    &[1.0, 2.0]
                } else {
                    &[1.0]
                };
                for &scale in scales {
                    let mut cfg = Config::default();
                    crate::themes::apply_theme(&mut cfg, t.name);
                    cfg.progress_style = style.into();
                    let mut frame = Frame::default();
                    let steps: [(f64, f32, &str); 4] = [
                        (0.9, 0.88, "24:59"),
                        (0.0, 0.5, "24:59"),
                        (1.0, 1.0, "24:58"),
                        (-1.0, 0.88, "24:58"),
                    ];
                    for (progress, opacity, timer) in steps {
                        let mut v = mk(&cfg, t.name);
                        v.scale = scale;
                        v.running = true;
                        v.progress = progress;
                        v.progress_color = Rgb(0, 200, 100);
                        v.opacity = opacity;
                        v.timer = timer.into();
                        let l = render_into(&v, &mut f, &mut frame);
                        let (fresh, fl) = render(&v, &mut f).unwrap();
                        assert_eq!(frame.opacity(), opacity);
                        let lut = opacity_lut(opacity).unwrap_or(std::array::from_fn(|b| b as u8));
                        let got: Vec<u8> = frame
                            .pixmap
                            .as_ref()
                            .unwrap()
                            .data()
                            .iter()
                            .map(|b| lut[usize::from(*b)])
                            .collect();
                        assert!(
                            got == fresh.data(),
                            "{} {style} x{scale} p={progress}: cached frame differs",
                            t.name
                        );
                        assert_eq!((l.w, l.h, l.panel), (fl.w, fl.h, fl.panel));
                    }
                    cfg.color_bg = "#203040".into(); // config change rebuilds
                    let v = mk(&cfg, t.name);
                    render_into(&v, &mut f, &mut frame);
                    let (fresh, _) = render(&v, &mut f).unwrap();
                    assert!(frame.pixmap.as_ref().unwrap().data() == fresh.data());
                }
            }
        }
    }

    #[test]
    fn shadows_off_drop_the_margin() {
        let cfg = Config::default();
        let v = mk(&cfg, "classic");
        let mut f = fonts();
        let with = StaticView::of(&v);
        assert!(with.shadow, "shadows are on by default");
        let without = StaticView {
            shadow: false,
            ..with.clone()
        };
        let (a, b) = (plan(&with, &mut f).layout, plan(&without, &mut f).layout);
        assert!(a.panel.x > 0);
        assert_eq!((b.panel.x, b.panel.y), (0, 0));
        assert_eq!((b.panel.w, b.panel.h), (a.panel.w, a.panel.h));
        assert_eq!((b.w, b.h), (b.panel.w, b.panel.h));
        // The panel corner is transparent (rounded), the rest of the
        // margin-less frame starts right at the panel.
        let mut pm = Pixmap::new(b.w as u32, b.h as u32).unwrap();
        draw_static(&without, &plan(&without, &mut f), &mut f, &mut pm);
        assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 0);
        let mid = pm.pixel(0, b.h as u32 / 2).unwrap().alpha();
        assert!(mid >= 128, "panel edge alpha {mid}");
    }

    #[test]
    fn corners_transparent_classic_no_shadow_area() {
        let cfg = Config::default();
        let (pm, l) = render(&mk(&cfg, "mono"), &mut fonts()).unwrap();
        assert_eq!(l.panel.x, 0);
        assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 0);
    }

    #[test]
    fn border_progress_half() {
        let cfg = Config {
            progress_style: "border".into(),
            ..Config::default()
        };
        let mut v = mk(&cfg, "mono");
        v.progress = 0.5;
        let (pm, l) = render(&v, &mut fonts()).unwrap();
        let red = |x: i32, y: i32| {
            let p = pm.pixel(x as u32, y as u32).unwrap();
            p.red() > 150 && p.green() < 60
        };
        let p = l.panel;
        assert!(red(p.x + p.w * 3 / 4, p.y + 1));
        assert!(!red(p.x + p.w / 4, p.y + p.h - 2));
    }

    #[test]
    fn opacity_scales_alpha() {
        let cfg = Config::default();
        let mut v = mk(&cfg, "classic");
        let at = |v: &View| {
            let (pm, l) = render(v, &mut fonts()).unwrap();
            pm.pixel((l.panel.x + 3) as u32, (l.panel.y + l.panel.h / 2) as u32)
                .unwrap()
                .alpha() as i32
        };
        let full = at(&v);
        v.opacity = 0.5;
        let half = at(&v);
        assert!(full > 100);
        assert!((half - full / 2).abs() <= 2, "{half} vs {full}");
    }

    #[test]
    fn stopwatch_no_progress() {
        let cfg = Config::default();
        let mut v = mk(&cfg, "classic");
        let none = measure(&v, &mut fonts()).h;
        v.progress = 0.5;
        assert!(measure(&v, &mut fonts()).h > none);
        // border style never adds a row
        let mut b = cfg.clone();
        b.progress_style = "border".into();
        let mut v = mk(&b, "classic");
        v.progress = 0.5;
        assert_eq!(measure(&v, &mut fonts()).h, none);
    }
}
