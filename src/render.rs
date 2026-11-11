//! Software overlay renderer: lays out the HUD rows and paints them into a tiny-skia pixmap.
use crate::config::Config;
use crate::layout::Rect;
use crate::text::FontSet;
use crate::themes::{Rgb, Theme};
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
            .find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map_or(Hit::Background, |(h, _)| *h)
    }
}

const SHADOW_LAYERS: u32 = 6;

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

fn plan(view: &View, fonts: &mut FontSet) -> Plan {
    let (cfg, theme, s) = (view.cfg, view.theme, view.scale);
    let fs = cfg.font_size as f32;
    let clock_px = (fs * theme.clock_scale * s).floor().max(1.0);
    let timer_px = (fs * theme.timer_scale * s).floor().max(1.0);
    let mode_px = ((fs * 0.42).floor().max(11.0) * s).floor();
    let gap = px(cfg.row_spacing as f32, s);
    let pad = cfg.padding;
    let pad_top = if cfg.padding_top >= 0 {
        cfg.padding_top
    } else {
        pad
    };
    let inset = if theme.shadow_alpha > 0 {
        px(12.0, s)
    } else {
        0
    };
    let (mx, top) = (px((pad + 4) as f32, s), px(pad_top as f32, s));

    let show_timer = cfg.show_timer;
    let ctrl_on = show_timer && view.show_controls_row;
    let pos = view.controls_pos.clamp(0.0, 1.0);
    let bottom = if ctrl_on {
        (px(8.0, s) as f32 * pos + gap as f32 * (1.0 - pos)).round() as i32
    } else {
        gap
    };

    let clock_h = fonts.ink_height(clock_px).round() as i32;
    let timer_h = fonts.ink_height(timer_px).round() as i32 + 4;
    let mode_h = fonts.ink_height(mode_px).round() as i32;
    let btn_h = px(((cfg.font_size - 15).max(16)) as f32, s);
    let btn_w = [btn_h + px(3.0, s), btn_h + px(3.0, s), btn_h + px(8.0, s)];
    let btn_gap = px(6.0, s);
    let ctrl_inner_h = btn_h + px(2.0, s);

    let mode_ls = 2.0 * s;
    let mode_chars = view.mode_text.chars().count().max(1) as f32;
    let mut content_w = 0f32;
    if cfg.show_clock {
        content_w = content_w.max(fonts.measure(&view.clock, clock_px, true));
    }
    if show_timer {
        content_w = content_w.max(fonts.measure(&view.timer, timer_px, true));
        content_w = content_w
            .max(fonts.measure(&view.mode_text, mode_px, true) + (mode_chars - 1.0) * mode_ls);
    }
    if ctrl_on {
        content_w = content_w.max((btn_w.iter().sum::<i32>() + 2 * btn_gap) as f32);
    }
    let w = content_w.ceil() as i32 + 2 * mx + 2 * inset;
    let panel_w = w - 2 * inset;
    let cx = inset + mx;
    let row_w = panel_w - 2 * mx;

    // Stack the visible rows top to bottom with a uniform gap.
    let mut y = inset + top;
    let mut first = true;
    let mut next = |h: i32| {
        if !first {
            y += gap;
        }
        first = false;
        let at = y;
        y += h;
        at
    };
    let mut clock_baseline = None;
    let mut clock_row = None;
    if cfg.show_clock {
        let at = next(clock_h);
        clock_baseline = Some((at + clock_h) as f32);
        clock_row = Some(rect(cx, at, row_w, clock_h));
    }
    let mut sep_y = None;
    let mut timer_row = None;
    let mut progress_row = None;
    let mut mode_row = None;
    let mut ctrl_row = None;
    let mut mode_baseline = 0.0;
    if show_timer {
        if theme.show_separator {
            sep_y = Some(next(1));
        }
        timer_row = Some(rect(cx, next(timer_h), row_w, timer_h));
        if cfg.progress_style == "line" && view.progress >= 0.0 {
            progress_row = Some(rect(cx, next(px(3.0, s)), row_w, px(3.0, s)));
        }
        let at = next(mode_h);
        mode_baseline = (at + mode_h) as f32;
        mode_row = Some(rect(cx, at, row_w, mode_h));
        if ctrl_on {
            let h = (ctrl_inner_h as f32 * pos).round() as i32;
            ctrl_row = Some(rect(cx, next(h), row_w, h));
        }
    }
    let h = y + bottom + inset;

    // Buttons: centred, full size at the unfolded position.
    let total = btn_w.iter().sum::<i32>() + 2 * btn_gap;
    let mut bx = inset + (panel_w - total) / 2;
    let by = ctrl_row.map_or(0, |r| r.y + (px(2.0, s) as f32 * pos).round() as i32);
    let bh = (btn_h as f32 * pos).round() as i32;
    let mut btn_rects = [rect(0, 0, 0, 0); 3];
    for (i, bw) in btn_w.iter().enumerate() {
        btn_rects[i] = rect(bx, by, *bw, bh);
        bx += bw + btn_gap;
    }

    let mut regions = Vec::new();
    if ctrl_row.is_some() {
        for (hit, r) in [Hit::BtnStart, Hit::BtnReset, Hit::BtnMode]
            .into_iter()
            .zip(btn_rects)
        {
            regions.push((hit, r));
        }
    }
    if let Some(r) = clock_row {
        regions.push((Hit::Clock, r));
    }
    if let Some(r) = timer_row {
        regions.push((Hit::Timer, r));
    }
    if let Some(r) = mode_row {
        regions.push((Hit::Mode, r));
    }
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
    plan(view, fonts).layout
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

fn draw_buttons(view: &View, pl: &Plan, fonts: &mut FontSet, pm: &mut Pixmap) {
    let pos = view.controls_pos.clamp(0.0, 1.0);
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
        let hover = view.hover_btn == Some(hit);
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
            let tw = fonts.measure(view.mode_btn, bpx, false);
            let ink = fonts.ink_height(bpx);
            let x = r.x as f32 + (r.w as f32 - tw) / 2.0;
            let base = r.y as f32 + (r.h as f32 + ink) / 2.0;
            fonts.draw(pm, view.mode_btn, x, base, bpx, fg, false, 0.0);
        } else {
            draw_icon(pm, hit, view.running, r, fg, s);
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

/// Draw one frame into a new pixmap; `None` if no pixmap of the layout's
/// size can be allocated.
pub fn render(view: &View, fonts: &mut FontSet) -> Option<(Pixmap, Layout)> {
    let mut frame = Frame::default();
    let layout = render_into(view, fonts, &mut frame);
    frame.pixmap.map(|pm| (pm, layout))
}

/// A reusable frame: the drawn `pixmap` plus a cache of everything but the
/// progress bar and opacity, so frames where only the bar moved (the common
/// case while running) skip layout, text and panel drawing.
#[derive(Default)]
pub struct Frame {
    /// The last frame drawn; `None` if no pixmap could be allocated.
    pub pixmap: Option<Pixmap>,
    base: Option<Pixmap>,
    key: Option<StaticKey>,
    plan: Option<Plan>,
}

/// The `View` inputs of the static layer (all but the bar's length and the
/// opacity).
#[derive(PartialEq)]
struct StaticKey {
    cfg: Config,
    theme: *const Theme,
    scale: f32,
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
    progress_color: Rgb,
}

impl StaticKey {
    fn matches(&self, v: &View) -> bool {
        self.theme == std::ptr::from_ref(v.theme)
            && self.scale == v.scale
            && self.clock == v.clock
            && self.timer == v.timer
            && self.timer_color == v.timer_color
            && self.timer_px_mul == v.timer_px_mul
            && self.mode_text == v.mode_text
            && self.mode_color == v.mode_color
            && self.show_controls_row == v.show_controls_row
            && self.controls_pos == v.controls_pos
            && self.running == v.running
            && self.mode_btn == v.mode_btn
            && self.hover_btn == v.hover_btn
            && self.progress_shown == (v.progress >= 0.0)
            && self.progress_color == v.progress_color
            && self.cfg == *v.cfg
    }

    fn of(v: &View) -> StaticKey {
        StaticKey {
            cfg: v.cfg.clone(),
            theme: std::ptr::from_ref(v.theme),
            scale: v.scale,
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
            progress_color: v.progress_color,
        }
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

/// Draw one frame into `frame.pixmap`, redrawing the static layer only when
/// a `View` input other than the bar length or opacity changed. Leaves
/// `None` when no pixmap of the layout's size can be allocated; the layout
/// is returned anyway.
pub fn render_into(view: &View, fonts: &mut FontSet, frame: &mut Frame) -> Layout {
    let fresh = match (&frame.key, &frame.plan) {
        (Some(k), Some(_)) if frame.base.is_some() => !k.matches(view),
        _ => true,
    };
    if fresh {
        let pl = plan(view, fonts);
        let (w, h) = (pl.layout.w.max(1) as u32, pl.layout.h.max(1) as u32);
        match &mut frame.base {
            Some(pm) if pm.width() == w && pm.height() == h => pm.fill(Color::TRANSPARENT),
            base => *base = Pixmap::new(w, h),
        }
        if let Some(base) = &mut frame.base {
            draw_static(view, &pl, fonts, base);
            frame.key = Some(StaticKey::of(view));
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
    layout
}

/// Everything but the progress bar fill and the opacity.
fn draw_static(view: &View, pl: &Plan, fonts: &mut FontSet, pm: &mut Pixmap) {
    let (cfg, theme, s) = (view.cfg, view.theme, pl.s);
    let lay = &pl.layout;
    let panel = lay.panel;
    let rad = theme.radius * s;
    let inset = panel.x;
    let pf = (
        panel.x as f32,
        panel.y as f32,
        panel.w as f32,
        panel.h as f32,
    );

    if theme.shadow_alpha > 0 {
        draw_shadow(pm, panel, inset, rad, theme.shadow_alpha);
    }
    let bg = hex(&cfg.color_bg, Rgb(0, 0, 0));
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
        let mx = (cfg.padding + 4) as f32 * s;
        if let Some(rc) = tiny_skia::Rect::from_xywh(pf.0 + mx, y as f32, pf.2 - 2.0 * mx, 1.0) {
            pm.fill_rect(rc, &paint(white(35)), Transform::identity(), None);
        }
    }

    let centre = pf.0 + pf.2 / 2.0;
    let clock_c = color(hex(&cfg.color_clock, Rgb(0, 255, 136)), theme.clock_alpha);
    if let Some(base) = pl.clock_baseline {
        let tw = fonts.measure(&view.clock, pl.clock_px, true);
        let x = centre - tw / 2.0;
        fonts.draw(pm, &view.clock, x, base, pl.clock_px, clock_c, true, 0.0);
    }
    if let Some(row) = pl.timer_row {
        // Pulse: integer px size, scaled about the row centre; other rows stay put.
        let tpx = (pl.timer_px * view.timer_px_mul).round().max(1.0);
        let ink = fonts.ink_height(tpx);
        let base = row.y as f32 + (row.h as f32 + ink) / 2.0;
        let tw = fonts.measure(&view.timer, tpx, true);
        fonts.draw(
            pm,
            &view.timer,
            centre - tw / 2.0,
            base,
            tpx,
            color(view.timer_color, 1.0),
            true,
            0.0,
        );
    }
    if let Some(row) = pl.progress_row {
        let r = (row.x as f32, row.y as f32, row.w as f32, row.h as f32);
        fill_rrect(pm, r, 1.5 * s, &paint(white(30)));
    }
    if pl.mode_row.is_some() {
        let ls = 2.0 * s;
        let n = view.mode_text.chars().count().max(1) as f32;
        let tw = fonts.measure(&view.mode_text, pl.mode_px, true) + (n - 1.0) * ls;
        fonts.draw(
            pm,
            &view.mode_text,
            centre - tw / 2.0,
            pl.mode_baseline,
            pl.mode_px,
            color(view.mode_color, 1.0),
            true,
            ls,
        );
    }
    draw_buttons(view, pl, fonts, pm);
}

/// The progress bar fill (line or border) and the window opacity.
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

    let o = view.opacity.clamp(0.0, 1.0);
    if o < 1.0 {
        // Same rounding as scaling each byte, via a 256-entry table.
        let lut: [u8; 256] = std::array::from_fn(|b| (b as f32 * o).round() as u8);
        for b in pm.data_mut() {
            *b = lut[*b as usize];
        }
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
                        let got = frame.pixmap.as_ref().unwrap();
                        assert!(
                            got.data() == fresh.data(),
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
