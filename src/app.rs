//! Platform-free HUD controller (port of `overlay.py`): turns input events and
//! clock ticks into state changes, animations and `Effect`s for the platform layer.
//!
//! Positions are tracked for the visible panel (Python's window); the window
//! itself is larger by the theme's shadow inset (`Layout::panel.x/y`).
use crate::anim::{Ease, Tween, lerp_rgb};
use crate::config::{
    COUNTDOWN_RANGE, Config, FONT_SIZE_RANGE, Mode, Preset, PresetRules, active_preset_matches,
    valid_presets,
};
use crate::engine::{ClockFn, Phase, State, TimerEngine};
use crate::format::{
    clock_text, fmt_seconds, mode_button_text, mode_label, phase_label, shown_seconds,
};
use crate::layout::{Rect, clamp_into, preset_position, resize_anchor_delta, snap_drag};
use crate::menu::{MenuAction, preset_savable};
use crate::render::{Frame, Hit, Layout, View, measure, render_into};
use crate::text::FontSet;
use crate::themes::{Rgb, apply_theme, get_theme, parse_hex};
use std::cell::RefCell;

mod events;
#[cfg(test)]
mod tests;

pub use events::{Button, Cursor, Effect, Input, Key, Status};

/// Qt's default double-click interval.
const DOUBLE_CLICK: f64 = 0.4;
const COLOR_FADE: f64 = 0.2;
const FOLD: f64 = 0.4;
/// Controls fold this long after the pointer leaves (unless idle).
const FOLD_DELAY: f64 = 2.0;
const PULSE: f64 = 0.18;
const PULSE_GAIN: f64 = 0.04;
const FADE_IN: f64 = 0.3;
/// Progress glide between adjacent targets while not running.
const GLIDE: f64 = 0.12;
const RUN_TICK: f64 = 0.1;
/// Countdown's final seconds: crisp color flips and beep pulses.
const FINAL_SECONDS: f64 = 6.5;
const MODE_GREY: Rgb = Rgb(0x99, 0x99, 0x99);
/// Frame interval while a tween animates (~60 fps).
pub const FRAME: f64 = 0.016;
/// Shortest interval between frames that only advance the progress bar
/// (~24 fps): smooth enough for a half-pixel step, far cheaper than `FRAME`.
pub const PROGRESS_FRAME: f64 = 0.042;
const MODES: [Mode; 3] = [Mode::Stopwatch, Mode::Countdown, Mode::Interval];

struct Drag {
    /// Pointer offset from the panel's top-left.
    off: (i32, i32),
    moved: bool,
}

pub struct App {
    pub cfg: Config,
    pub engine: TimerEngine,
    /// Loads the HUD font when the theme/config font family changes.
    pub font_loader: fn(&str) -> FontSet,
    fonts: RefCell<FontSet>,
    font_family: String,
    scale: f32,
    area: Rect,
    // Labels
    clock: String,
    timer: String,
    mode_text: String,
    /// Live WORK/REST label currently shown ("" = static mode label).
    interval_label: String,
    mode_color: Option<Rgb>,
    // Timer color fade (tween runs 0 → 1 from `color_from` to `color_to`)
    color_target: String,
    color_from: Rgb,
    color_to: Rgb,
    color_anim: Tween,
    // Progress (targets from the last tick; -1 = hidden)
    progress_target: f64,
    progress_glide: Option<Tween>,
    bar_color: Rgb,
    // Fold / pulse / fade-in
    controls: Tween,
    fold_at: Option<f64>,
    pulse_start: Option<f64>,
    fade_in: Tween,
    // Input
    drag: Option<Drag>,
    last_timer_press: Option<f64>,
    hover: Option<Hit>,
    /// The cursor last set on the overlay window.
    cursor: Cursor,
    visible: bool,
    /// Draw the theme's shadow ([`App::set_shadows`]).
    shadows: bool,
    // Geometry
    panel_pos: (i32, i32),
    layout: Layout,
}

fn hex(s: &str, fallback: Rgb) -> Rgb {
    parse_hex(s).unwrap_or(fallback)
}

fn to_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Font family the HUD draws with: theme font, else config font, else Monospace.
pub fn resolved_family(cfg: &Config) -> String {
    let theme = get_theme(&cfg.theme);
    [theme.font_family, cfg.font_family.as_str()]
        .into_iter()
        .find(|f| !f.is_empty())
        .unwrap_or("Monospace")
        .to_string()
}

/// The saved (dragged) panel position, if any; `-1` means a preset position.
pub fn custom_pos(cfg: &Config) -> Option<(i32, i32)> {
    (cfg.custom_x >= 0 && cfg.custom_y >= 0).then(|| (to_i32(cfg.custom_x), to_i32(cfg.custom_y)))
}

/// Qt `QRect::center()` on one axis.
fn qt_center(pos: i32, len: i32) -> f64 {
    f64::from((2 * pos + len - 1) / 2)
}

fn is_button(h: Hit) -> bool {
    matches!(h, Hit::BtnStart | Hit::BtnReset | Hit::BtnMode)
}

/// The pointer shape over `hit` (Python: PointingHandCursor on the timer,
/// mode label and buttons). Click-through windows get no pointer events,
/// so they keep the default.
pub fn cursor_for(hit: Hit, click_through: bool) -> Cursor {
    match hit {
        _ if click_through => Cursor::Default,
        Hit::Timer | Hit::Mode | Hit::BtnStart | Hit::BtnReset | Hit::BtnMode => Cursor::Hand,
        Hit::Clock | Hit::Background => Cursor::Default,
    }
}

impl App {
    pub fn new(
        cfg: Config,
        clock: ClockFn,
        fonts: FontSet,
        scale: f32,
        area: Rect,
        now: f64,
    ) -> App {
        let engine = TimerEngine::new(&cfg, clock);
        let pause = hex(&cfg.color_timer_pause, Rgb(0x88, 0x88, 0x88));
        let mut app = App {
            engine,
            font_loader: FontSet::load,
            fonts: RefCell::new(fonts),
            font_family: resolved_family(&cfg),
            scale,
            area,
            clock: "--:--:--".into(),
            timer: "00:00".into(),
            mode_text: mode_label(&cfg),
            interval_label: String::new(),
            mode_color: None,
            color_target: String::new(),
            color_from: pause,
            color_to: pause,
            color_anim: Tween::new(1.0, 1.0, now, 0.0, Ease::Linear),
            progress_target: -1.0,
            progress_glide: None,
            bar_color: hex(get_theme(&cfg.theme).color_clock, Rgb(0, 255, 136)),
            controls: Tween::new(1.0, 1.0, now, 0.0, Ease::InOutCubic),
            fold_at: None,
            pulse_start: None,
            fade_in: Tween::new(0.0, cfg.opacity, now, FADE_IN, Ease::OutCubic),
            drag: None,
            last_timer_press: None,
            hover: None,
            cursor: Cursor::Default,
            visible: true,
            shadows: true,
            panel_pos: (0, 0),
            layout: Layout {
                w: 0,
                h: 0,
                panel: Rect {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                },
                regions: Vec::new(),
            },
            cfg,
        };
        app.update_timer(now);
        app.layout = app.measure(now);
        let p = app.layout.panel;
        app.panel_pos = match custom_pos(&app.cfg) {
            // A saved position from another monitor setup must stay reachable.
            Some((x, y)) => clamp_into(x, y, p.w, p.h, area),
            None => app.preset_pos(),
        };
        app
    }

    // ══ Queries ═════════════════════════════════════════════════════════════

    /// Top-left of the window in root coordinates.
    pub fn window_pos(&self) -> (i32, i32) {
        (
            self.panel_pos.0 - self.layout.panel.x,
            self.panel_pos.1 - self.layout.panel.y,
        )
    }

    pub fn timer_text(&self) -> &str {
        &self.timer
    }

    /// The timer's state for `timehud status` and the tray tooltip.
    pub fn status(&self) -> Status {
        let mode = self.cfg.timer_mode;
        let state = if self.engine.running() {
            "running"
        } else if self.engine.is_idle(&self.cfg) {
            "idle"
        } else {
            "paused"
        };
        Status {
            mode: mode.name().into(),
            state: state.into(),
            // As of the last tick (hidden timers tick too).
            display: self.timer.clone(),
            label: self.mode_text.clone(),
            preset: self.cfg.active_preset.clone(),
            visible: self.visible,
        }
    }

    pub fn mode_text(&self) -> &str {
        &self.mode_text
    }

    /// Fold position: 1 = controls shown, 0 = folded away.
    pub fn controls_pos(&self, now: f64) -> f64 {
        self.controls.value(now)
    }

    /// Window opacity including the startup fade-in.
    pub fn opacity(&self, now: f64) -> f64 {
        if self.fade_in.done(now) {
            self.cfg.opacity
        } else {
            self.fade_in.value(now)
        }
    }

    /// True while a frame-by-frame animation is visible (never while hidden).
    pub fn animating(&self, now: f64) -> bool {
        self.next_frame(now).is_some()
    }

    /// When the next animation frame is due (never while hidden): `now +
    /// FRAME` while a tween runs; while only the live progress bar moves, when
    /// its drawn length next changes by about half a device pixel; `None`
    /// when the picture only changes through ticks and actions.
    pub fn next_frame(&self, now: f64) -> Option<f64> {
        if !self.visible {
            return None;
        }
        let running = self.engine.running();
        let pulsing = self.pulse_start.is_some_and(|s| now - s < PULSE);
        // While running the bar reads engine time; the glide is unused then.
        let gliding = !running && self.progress_glide.as_ref().is_some_and(|g| !g.done(now));
        if !self.fade_in.done(now)
            || !self.color_anim.done(now)
            || !self.controls.done(now)
            || pulsing
            || gliding
        {
            return Some(now + FRAME);
        }
        if running && self.progress_target >= 0.0 {
            return Some(now + self.progress_frame_interval());
        }
        None
    }

    /// Seconds for the running bar to move ~0.5 device px in the phase it is
    /// draining now (at least `PROGRESS_FRAME`).
    fn progress_frame_interval(&self) -> f64 {
        let cfg = &self.cfg;
        // The bar moves 1/phase per second.
        let Some(phase) = self.engine.phase_length(cfg) else {
            return PROGRESS_FRAME;
        };
        let p = self.layout.panel;
        // Upper bounds of the drawn length: the panel width for the line, the
        // panel outline for the border.
        let length = if cfg.progress_style == "border" {
            2 * (p.w + p.h)
        } else {
            p.w
        };
        (0.5 * phase / f64::from(length.max(1))).max(PROGRESS_FRAME)
    }

    /// Seconds until `tick` is next needed: 100 ms while running, else the next
    /// wall-clock second (`wall_frac` = fraction of the current second elapsed),
    /// or earlier for a pending auto-fold. Animations are driven by `animating`.
    pub fn next_wakeup(&self, now: f64, wall_frac: f64) -> f64 {
        let mut t = if self.engine.running() {
            RUN_TICK
        } else {
            1.0 - wall_frac.clamp(0.0, 1.0)
        };
        if let Some(at) = self.fold_at {
            t = t.min((at - now).max(0.0));
        }
        t
    }

    fn has_custom_pos(&self) -> bool {
        custom_pos(&self.cfg).is_some()
    }

    fn preset_pos(&self) -> (i32, i32) {
        let p = self.layout.panel;
        preset_position(
            &self.cfg.position,
            self.area,
            p.w,
            p.h,
            to_i32(self.cfg.margin),
        )
    }

    fn timer_color(&self, now: f64) -> Rgb {
        lerp_rgb(self.color_from, self.color_to, self.color_anim.value(now))
    }

    fn timer_px_mul(&self, now: f64) -> f64 {
        match self.pulse_start {
            Some(s) if now >= s && now - s < PULSE => {
                let t = (now - s) / PULSE;
                1.0 + PULSE_GAIN * (1.0 - (2.0 * t - 1.0).abs())
            }
            _ => 1.0,
        }
    }

    fn progress_shown(&self, now: f64) -> f64 {
        if self.progress_target < 0.0 {
            -1.0
        } else if self.engine.running() {
            self.engine.progress_now(&self.cfg)
        } else {
            self.progress_glide
                .as_ref()
                .map_or(self.progress_target, |g| g.value(now))
        }
    }

    fn view(&self, now: f64) -> View<'_> {
        View {
            cfg: &self.cfg,
            theme: get_theme(&self.cfg.theme),
            shadow: self.shadows,
            scale: self.scale,
            clock: self.clock.clone(),
            timer: self.timer.clone(),
            timer_color: self.timer_color(now),
            timer_px_mul: self.timer_px_mul(now) as f32,
            mode_text: self.mode_text.clone(),
            mode_color: self.mode_color.unwrap_or(MODE_GREY),
            show_controls_row: self.cfg.show_controls,
            controls_pos: self.controls_pos(now) as f32,
            running: self.engine.running(),
            mode_btn: mode_button_text(self.cfg.timer_mode),
            hover_btn: self.hover.filter(|h| is_button(*h)),
            progress: self.progress_shown(now),
            progress_color: self.bar_color,
            opacity: self.opacity(now) as f32,
        }
    }

    // ══ Frame / layout ══════════════════════════════════════════════════════

    /// Render the current state. The size may change between frames (fold
    /// animation); callers apply `window_pos()` and the layout size after each.
    pub fn frame(&mut self, now: f64) -> (Option<tiny_skia::Pixmap>, Layout) {
        let mut frame = Frame::default();
        let layout = self.frame_into(now, &mut frame);
        (frame.pixmap, layout)
    }

    /// `frame`, drawing into the reusable `canvas` (see `render_into`).
    pub fn frame_into(&mut self, now: f64, canvas: &mut Frame) -> Layout {
        self.poll(now);
        let layout = {
            let view = self.view(now);
            render_into(&view, &mut self.fonts.borrow_mut(), canvas)
        };
        self.apply_layout(layout.clone());
        layout
    }

    fn measure(&self, now: f64) -> Layout {
        measure(&self.view(now), &mut self.fonts.borrow_mut())
    }

    /// Re-measure after a state change; re-anchors the window on resize.
    fn relayout(&mut self, now: f64) -> Vec<Effect> {
        let layout = self.measure(now);
        self.apply_layout(layout)
    }

    fn apply_layout(&mut self, layout: Layout) -> Vec<Effect> {
        let old = (self.layout.w, self.layout.h, self.layout.panel);
        let old_window = self.window_pos();
        self.layout = layout;
        let p = self.layout.panel;
        let mut fx = Vec::new();
        if (old.0, old.1) != (self.layout.w, self.layout.h) {
            fx.push(Effect::Resize);
        }
        if (old.2.w, old.2.h) != (p.w, p.h) {
            if !self.has_custom_pos() {
                // Preset positions re-anchor exactly.
                self.panel_pos = self.preset_pos();
            } else {
                // Dragged: keep the nearer edge / center fixed per axis.
                let (x, y) = self.panel_pos;
                let a = self.area;
                let (dx, dy) = resize_anchor_delta(
                    qt_center(x, p.w),
                    qt_center(y, p.h),
                    qt_center(a.x, a.w),
                    qt_center(a.y, a.h),
                    p.w - old.2.w,
                    p.h - old.2.h,
                );
                self.panel_pos = (x + dx, y + dy);
            }
        }
        let (x, y) = self.window_pos();
        if (x, y) != old_window {
            fx.push(Effect::Move { x, y });
        }
        fx
    }

    /// Turns the theme's drop shadow (and the window margin it needs) on or
    /// off from the next frame; the HUD turns it off while no compositing
    /// manager runs. The panel stays put; the window moves and resizes
    /// around it.
    pub fn set_shadows(&mut self, on: bool) {
        self.shadows = on;
    }

    /// The monitor's work area changed.
    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        let p = self.layout.panel;
        self.panel_pos = if self.has_custom_pos() {
            clamp_into(self.panel_pos.0, self.panel_pos.1, p.w, p.h, area)
        } else {
            self.preset_pos()
        };
    }

    /// The display scale (`Xft.dpi / 96`) changed: re-measure at the new
    /// size, then re-anchor and keep the window inside the work area.
    pub fn set_scale(&mut self, scale: f32, now: f64) -> Vec<Effect> {
        if scale == self.scale {
            return Vec::new();
        }
        self.scale = scale;
        let mut fx = self.relayout(now);
        let before = self.window_pos();
        self.set_area(self.area);
        if self.window_pos() != before && !fx.iter().any(|e| matches!(e, Effect::Move { .. })) {
            fx.push(self.move_effect());
        }
        fx.push(Effect::Redraw);
        fx
    }

    fn move_effect(&self) -> Effect {
        let (x, y) = self.window_pos();
        Effect::Move { x, y }
    }

    /// Engine tick + relayout + redraw, after a state-changing action.
    fn sync(&mut self, now: f64) -> Vec<Effect> {
        let mut fx = self.update_timer(now);
        fx.extend(self.relayout(now));
        fx.push(Effect::Redraw);
        fx
    }

    /// After a timer-setup change: fresh mode label, save, then `sync`.
    fn changed(&mut self, now: f64) -> Vec<Effect> {
        self.refresh_mode_label();
        let mut fx = vec![Effect::SaveSoon];
        fx.extend(self.sync(now));
        fx
    }

    // ══ Tick (port of `_update`) ════════════════════════════════════════════

    /// Start pending timed transitions (auto-fold) whose deadline has passed.
    fn poll(&mut self, now: f64) {
        if let Some(at) = self.fold_at
            && now >= at
        {
            self.fold_at = None;
            self.fade_controls_to(0.0, at);
        }
    }

    pub fn tick(&mut self, now: f64, wall: chrono::NaiveTime) -> Vec<Effect> {
        self.poll(now);
        let before = (self.clock.len(), self.timer.len(), self.mode_text.clone());
        let shown_before = self.shown_state();
        if self.cfg.show_clock {
            self.clock = clock_text(wall, &self.cfg.clock_format);
        }
        let mut fx = self.update_timer(now);
        if before != (self.clock.len(), self.timer.len(), self.mode_text.clone()) {
            fx.extend(self.relayout(now));
        }
        // Running: ticks come every 100 ms, but the picture changes about once
        // a second (the moving bar is driven by `next_frame`).
        if !fx.is_empty() || self.shown_state() != shown_before {
            fx.push(Effect::Redraw);
        }
        fx
    }

    /// What a tick can change on screen outside of animations.
    fn shown_state(&self) -> (String, String, String, Option<Rgb>, String, Rgb, f64) {
        // While running, the bar follows engine time, not the tick's target.
        let progress = if self.engine.running() {
            self.progress_target.signum()
        } else {
            self.progress_target
        };
        // A hidden timer's text and label are not drawn.
        let shown = |s: &String| {
            if self.cfg.show_timer {
                s.clone()
            } else {
                String::new()
            }
        };
        (
            self.clock.clone(),
            shown(&self.timer),
            shown(&self.mode_text),
            self.mode_color,
            self.color_target.clone(),
            self.bar_color,
            progress,
        )
    }

    /// Engine tick: beeps, finish notification and labels always (a hidden
    /// timer keeps running); `show_timer` only gates what is drawn of it
    /// (color, progress, pulse).
    fn update_timer(&mut self, now: f64) -> Vec<Effect> {
        let r = self.engine.tick(&self.cfg);
        let theme = get_theme(&self.cfg.theme);
        let countdown = self.cfg.timer_mode == Mode::Countdown;
        self.timer = fmt_seconds(shown_seconds(self.cfg.timer_mode, r.display));
        let shown = self.cfg.show_timer;

        // Crisp flashes during the countdown's final seconds; fade otherwise
        let animate = r.state != State::End && !(countdown && r.display <= FINAL_SECONDS);
        let color = if r.phase == Phase::Rest && r.state == State::Run {
            theme.color_rest.to_string()
        } else {
            match r.state {
                State::Run => self.cfg.color_timer_run.clone(),
                State::Pause => self.cfg.color_timer_pause.clone(),
                State::Warn => theme.color_warn.to_string(),
                State::End => theme.color_end.to_string(),
            }
        };
        if shown {
            self.set_timer_color(&color, animate, now);
        }

        let bar = if matches!(r.state, State::Warn | State::End) {
            theme.color_warn
        } else if r.phase == Phase::Rest {
            theme.color_rest
        } else {
            theme.color_clock
        };
        self.bar_color = hex(bar, Rgb(255, 255, 255));
        let bar_style = matches!(self.cfg.progress_style.as_str(), "line" | "border");
        self.set_progress(if shown && bar_style { r.progress } else { -1.0 }, now);

        if r.phase != Phase::None && !self.engine.is_idle(&self.cfg) {
            let label = phase_label(r.phase, r.round, r.rounds);
            if label != self.interval_label {
                self.interval_label = label.clone();
                self.mode_text = label;
                let c = if r.phase == Phase::Rest {
                    theme.color_rest
                } else {
                    theme.color_clock
                };
                self.mode_color = Some(hex(c, MODE_GREY));
            }
        } else {
            self.interval_label.clear();
        }

        let mut fx: Vec<Effect> = r.beeps.iter().map(|b| Effect::Beep(*b)).collect();
        if shown && !r.beeps.is_empty() && countdown && r.display <= FINAL_SECONDS {
            self.pulse_start = Some(now);
        }
        if r.finished && !r.restarted && self.cfg.notify_on_finish {
            let text = if self.cfg.timer_mode == Mode::Interval {
                "Interval session finished"
            } else {
                "Countdown finished"
            };
            fx.push(Effect::Notify(text.into()));
        }
        fx
    }

    fn set_timer_color(&mut self, color: &str, animate: bool, now: f64) {
        if color == self.color_target {
            return;
        }
        self.color_target = color.to_string();
        let c = hex(color, Rgb(255, 255, 255));
        if animate {
            self.color_from = self.timer_color(now);
            self.color_anim = Tween::new(0.0, 1.0, now, COLOR_FADE, Ease::Linear);
        } else {
            self.color_from = c;
            self.color_anim = Tween::new(1.0, 1.0, now, 0.0, Ease::Linear);
        }
        self.color_to = c;
    }

    /// New progress target; glides between adjacent values, snaps on jumps.
    fn set_progress(&mut self, fraction: f64, now: f64) {
        if fraction == self.progress_target {
            return;
        }
        let previous = self.progress_shown(now);
        self.progress_target = fraction;
        self.progress_glide =
            if fraction < 0.0 || previous < 0.0 || (fraction - previous).abs() > 0.2 {
                None
            } else {
                Some(Tween::new(previous, fraction, now, GLIDE, Ease::Linear))
            };
    }

    fn refresh_mode_label(&mut self) {
        self.interval_label.clear(); // force live label re-sync on next tick
        self.mode_color = None;
        self.mode_text = mode_label(&self.cfg);
    }

    /// Port of `_apply_styles`' non-geometry part: drop the pulse, reload the font.
    fn restyle(&mut self) {
        self.pulse_start = None;
        let family = resolved_family(&self.cfg);
        if family != self.font_family {
            *self.fonts.borrow_mut() = (self.font_loader)(&family);
            self.font_family = family;
        }
    }

    // ══ Fold ════════════════════════════════════════════════════════════════

    fn fade_controls_to(&mut self, target: f64, now: f64) {
        if self.controls.target() == target {
            return;
        }
        let from = self.controls_pos(now);
        self.controls = Tween::new(from, target, now, FOLD, Ease::InOutCubic);
    }

    /// Instantly restore the unfolded state (settings toggled controls on).
    fn reset_controls_fold(&mut self, now: f64) {
        self.fold_at = None;
        self.controls = Tween::new(1.0, 1.0, now, 0.0, Ease::InOutCubic);
    }

    // ══ Actions ═════════════════════════════════════════════════════════════

    /// Start if stopped, pause if running.
    /// Refreshes color/progress at once: paused HUDs only tick once per second.
    pub fn toggle_timer(&mut self, now: f64) -> Vec<Effect> {
        self.engine.toggle(&self.cfg);
        let mut fx = self.update_timer(now);
        fx.push(Effect::Redraw);
        fx
    }

    /// Starts the timer unless it already runs (`timehud start`).
    pub fn start_timer(&mut self, now: f64) -> Vec<Effect> {
        if self.engine.running() {
            return Vec::new();
        }
        self.toggle_timer(now)
    }

    /// Pauses the timer if it runs (`timehud pause`).
    pub fn pause_timer(&mut self, now: f64) -> Vec<Effect> {
        if !self.engine.running() {
            return Vec::new();
        }
        self.toggle_timer(now)
    }

    /// Switches to `mode` (stopped, at its start) and ends the preset, like
    /// the mode button; a no-op if already in that mode.
    pub fn set_mode(&mut self, mode: Mode, now: f64) -> Vec<Effect> {
        self.poll(now);
        if mode == self.cfg.timer_mode {
            return Vec::new();
        }
        self.cfg.active_preset.clear(); // manual mode change ends the preset
        self.engine.set_mode(&mut self.cfg, mode);
        self.changed(now)
    }

    /// `timehud set`: a countdown of `secs`, stopped at its start. Not a
    /// preset, so `active_preset` is cleared.
    pub fn set_countdown(&mut self, secs: i64, now: f64) -> Result<Vec<Effect>, String> {
        self.poll(now);
        let (lo, hi) = COUNTDOWN_RANGE;
        if !(lo..=hi).contains(&secs) {
            return Err(format!(
                "a countdown must last {lo} s to {}",
                fmt_seconds(hi as f64)
            ));
        }
        self.cfg.countdown_duration = secs;
        self.cfg.active_preset.clear();
        self.engine.set_mode(&mut self.cfg, Mode::Countdown);
        Ok(self.changed(now))
    }

    /// `timehud preset NAME`: applies the preset of that name (exact match
    /// first, else ignoring case).
    pub fn apply_preset_named(&mut self, name: &str, now: f64) -> Result<Vec<Effect>, String> {
        self.poll(now);
        let list = valid_presets(&self.cfg.presets);
        let name = name.trim();
        let index = list
            .iter()
            .find(|(p, _, _)| p.name() == name)
            .or_else(|| {
                list.iter()
                    .find(|(p, _, _)| p.name().eq_ignore_ascii_case(name))
            })
            .map(|(_, _, i)| *i)
            .ok_or_else(|| {
                let names: Vec<&str> = list.iter().map(|(p, _, _)| p.name()).collect();
                if names.is_empty() {
                    format!("no preset named '{name}' (there are no presets)")
                } else {
                    format!("no preset named '{name}' (have: {})", names.join(", "))
                }
            })?;
        Ok(self.apply_preset(index, now))
    }

    /// Stop and zero the timer.
    pub fn reset_timer(&mut self, now: f64) -> Vec<Effect> {
        self.engine.reset(&self.cfg);
        self.refresh_mode_label();
        self.sync(now)
    }

    pub fn toggle_visibility(&mut self) -> Vec<Effect> {
        self.set_visible(!self.visible)
    }

    pub fn set_visible(&mut self, v: bool) -> Vec<Effect> {
        self.visible = v;
        vec![Effect::SetVisible(v)]
    }

    /// Switch mode by `dir` steps through stopwatch → countdown → interval.
    fn step_mode(&mut self, dir: i32, now: f64) -> Vec<Effect> {
        let cur = MODES
            .iter()
            .position(|m| *m == self.cfg.timer_mode)
            .unwrap_or(0) as i32;
        let new = MODES[(cur + dir).rem_euclid(MODES.len() as i32) as usize];
        self.set_mode(new, now)
    }

    pub fn perform(&mut self, action: &MenuAction, now: f64) -> Vec<Effect> {
        self.poll(now);
        match action {
            MenuAction::OpenSettings => vec![Effect::OpenSettings { tab: None }],
            MenuAction::ManagePresets => vec![Effect::OpenSettings {
                tab: Some("presets".into()),
            }],
            MenuAction::SavePreset => vec![Effect::OpenSavePreset],
            MenuAction::ApplyStopwatch => {
                self.cfg.active_preset.clear(); // plain stopwatch, no preset
                self.cfg.stopwatch_work = 0;
                self.cfg.stopwatch_rest = 0;
                self.engine.set_mode(&mut self.cfg, Mode::Stopwatch);
                self.changed(now)
            }
            MenuAction::ApplyPreset(index) => self.apply_preset(*index, now),
            MenuAction::SetTheme(name) => {
                apply_theme(&mut self.cfg, name);
                self.restyle();
                self.changed(now)
            }
            MenuAction::ToggleClickThrough => {
                self.cfg.click_through = !self.cfg.click_through;
                vec![
                    Effect::SetClickThrough(self.cfg.click_through),
                    Effect::SaveSoon,
                ]
            }
            MenuAction::SetOpacity(v) => {
                self.cfg.opacity = *v;
                vec![Effect::SaveSoon, Effect::Redraw]
            }
            MenuAction::SetPosition(preset) => {
                self.cfg.position = (*preset).to_string();
                self.cfg.custom_x = -1;
                self.cfg.custom_y = -1;
                self.panel_pos = self.preset_pos();
                vec![self.move_effect(), Effect::SaveSoon]
            }
            MenuAction::ToggleVisibility => self.toggle_visibility(),
            MenuAction::Quit => vec![Effect::Quit], // shutdown saves
        }
    }

    /// Apply `config.presets[index]` (raw index; malformed entries are ignored).
    fn apply_preset(&mut self, index: usize, now: f64) -> Vec<Effect> {
        let Some((preset, rules, _)) = valid_presets(&self.cfg.presets)
            .into_iter()
            .find(|(_, _, i)| *i == index)
        else {
            return Vec::new();
        };
        preset.apply_to(&rules, &mut self.cfg);
        self.engine.reset(&self.cfg);
        self.changed(now)
    }

    /// Save the current timer setup as a preset (same name overwrites).
    /// Fails, changing nothing, for an empty name or a setup the config
    /// loader would discard (e.g. a plain stopwatch).
    pub fn save_preset(&mut self, name: &str) -> Result<Vec<Effect>, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("the preset name is empty".into());
        }
        let cfg = &self.cfg;
        if !preset_savable(cfg) {
            return Err("a plain stopwatch cannot be saved as a preset".into());
        }
        let preset = match cfg.timer_mode {
            Mode::Interval => Preset::Interval {
                name: name.into(),
                work: cfg.interval_work,
                rest: cfg.interval_rest,
                total: (cfg.interval_work + cfg.interval_rest) * cfg.interval_rounds,
            },
            Mode::Stopwatch => Preset::Stopwatch {
                name: name.into(),
                work: cfg.stopwatch_work,
                rest: cfg.stopwatch_rest,
            },
            Mode::Countdown => Preset::Countdown {
                name: name.into(),
                duration: cfg.countdown_duration,
            },
        };
        // Capture the current sound rules with the preset
        let rules = PresetRules {
            last5: Some(cfg.alert_last_5_seconds),
            boundary: Some(cfg.phase_beeps),
            halfway: Some(cfg.halfway_beep),
        };
        let value = preset.to_value(&rules);
        if valid_presets(std::slice::from_ref(&value)).is_empty() {
            return Err("the current timer setup is not a valid preset".into());
        }
        // Like Python, malformed entries are dropped; kept ones stay verbatim.
        let mut presets: Vec<_> = valid_presets(&cfg.presets)
            .into_iter()
            .filter(|(p, _, _)| p.name() != name)
            .map(|(_, _, i)| cfg.presets[i].clone())
            .collect();
        presets.push(value);
        self.cfg.presets = presets;
        self.cfg.active_preset = name.into();
        self.refresh_mode_label();
        Ok(vec![Effect::SaveSoon, Effect::Redraw])
    }

    /// Live-apply a config edited in Settings (port of `update_ui` + `_sync_*`).
    /// Not saved here: the settings flow commits with `SaveNow`.
    pub fn preview_config(&mut self, mut cfg: Config, now: f64) -> Vec<Effect> {
        self.poll(now);
        cfg.clamp_ranges();
        let old = std::mem::replace(&mut self.cfg, cfg);
        let mut fx = Vec::new();
        // A manual duration or mode change ends the preset, unless the edit
        // already changed `active_preset` itself (e.g. Cancel restoring it).
        // An edit that keeps the setup the preset describes (Settings
        // updating the active preset itself) keeps it.
        if (self.cfg.countdown_duration != old.countdown_duration
            || self.cfg.timer_mode != old.timer_mode)
            && self.cfg.active_preset == old.active_preset
            && !active_preset_matches(&self.cfg)
        {
            self.cfg.active_preset.clear();
        }
        let interval = |c: &Config| (c.interval_work, c.interval_rest, c.interval_rounds);
        let stopped = !self.engine.running();
        // Back to the start on a mode change or when the timer gets hidden (a
        // hidden timer started via IPC/hotkey keeps running through other
        // edits); a stopped countdown reloads its new duration (stays idle), a
        // stopped interval timer its new phases.
        let needs_reset = self.cfg.timer_mode != old.timer_mode
            || (old.show_timer && !self.cfg.show_timer)
            || (stopped
                && self.cfg.timer_mode == Mode::Countdown
                && self.cfg.countdown_duration != old.countdown_duration)
            || (stopped
                && self.cfg.timer_mode == Mode::Interval
                && interval(&self.cfg) != interval(&old));
        if needs_reset {
            self.engine.reset(&self.cfg);
        } else {
            // Stopwatch cycle edits under a running stopwatch: no replayed beeps.
            let cycle = |c: &Config| (c.stopwatch_work, c.stopwatch_rest);
            if cycle(&self.cfg) != cycle(&old) {
                self.engine.resync_cycle(&self.cfg);
            }
        }
        self.restyle();
        if self.cfg.show_timer && self.cfg.show_controls {
            self.reset_controls_fold(now);
        }
        if self.cfg.show_tray_icon != old.show_tray_icon {
            fx.push(Effect::SetTrayVisible(self.cfg.show_tray_icon));
        }
        if self.cfg.click_through != old.click_through {
            fx.push(Effect::SetClickThrough(self.cfg.click_through));
        }
        self.refresh_mode_label();
        fx.extend(self.sync(now));
        if !self.has_custom_pos() && self.panel_pos != self.preset_pos() {
            self.panel_pos = self.preset_pos();
            fx.push(self.move_effect());
        }
        fx
    }

    // ══ Input (port of eventFilter / mouse*Event / keyPressEvent) ══════════

    pub fn handle(&mut self, input: Input, now: f64) -> Vec<Effect> {
        self.poll(now);
        match input {
            Input::Press {
                x,
                y,
                root_x,
                root_y,
                button,
            } => self.press(x, y, root_x, root_y, button, now),
            Input::Release {
                button: Button::Left,
            } => match self.drag.take() {
                Some(d) if d.moved => {
                    self.cfg.custom_x = i64::from(self.panel_pos.0);
                    self.cfg.custom_y = i64::from(self.panel_pos.1);
                    vec![Effect::SaveSoon]
                }
                _ => Vec::new(),
            },
            Input::Release { .. } => Vec::new(),
            Input::Motion {
                x,
                y,
                root_x,
                root_y,
            } => self.motion(x, y, root_x, root_y),
            Input::Wheel { x, y, up } => self.wheel(x, y, up, now),
            Input::Enter => {
                self.fold_at = None;
                self.fade_controls_to(1.0, now);
                vec![Effect::Redraw]
            }
            Input::Leave => {
                // Keep buttons visible while idle at 00:00 for discoverability
                if !self.engine.is_idle(&self.cfg) {
                    self.fold_at = Some(now + FOLD_DELAY);
                }
                self.hover = None;
                vec![Effect::Redraw]
            }
            Input::Key { key, ctrl } => match key {
                Key::Space => self.toggle_timer(now),
                Key::R => self.reset_timer(now),
                Key::Escape => self.set_visible(false),
                Key::Q if ctrl => vec![Effect::Quit], // shutdown saves
                _ => Vec::new(),
            },
        }
    }

    fn press(
        &mut self,
        x: i32,
        y: i32,
        root_x: i32,
        root_y: i32,
        b: Button,
        now: f64,
    ) -> Vec<Effect> {
        let hit = self.layout.hit(x, y);
        let last_timer_press = self.last_timer_press.take();
        match (b, hit) {
            (Button::Right, _) => vec![Effect::OpenMenu {
                root_x,
                root_y,
                include_window_actions: self.cfg.show_tray_icon,
            }],
            (Button::Middle, Hit::Timer | Hit::Mode) => self.step_mode(1, now),
            (Button::Middle, _) => Vec::new(),
            (Button::Left, Hit::Timer) => {
                // Qt: press toggles; a second press within the interval is the
                // double click, which resets instead.
                if last_timer_press.is_some_and(|t| now - t < DOUBLE_CLICK) {
                    self.reset_timer(now)
                } else {
                    self.last_timer_press = Some(now);
                    self.toggle_timer(now)
                }
            }
            (Button::Left, Hit::Mode) => Vec::new(),
            (Button::Left, Hit::BtnStart) => self.toggle_timer(now),
            (Button::Left, Hit::BtnReset) => self.reset_timer(now),
            (Button::Left, Hit::BtnMode) => self.step_mode(1, now),
            (Button::Left, Hit::Clock | Hit::Background) => {
                self.drag = Some(Drag {
                    off: (root_x - self.panel_pos.0, root_y - self.panel_pos.1),
                    moved: false,
                });
                Vec::new()
            }
        }
    }

    fn motion(&mut self, x: i32, y: i32, root_x: i32, root_y: i32) -> Vec<Effect> {
        let mut fx = Vec::new();
        if let Some(d) = &mut self.drag {
            let p = self.layout.panel;
            let pos = snap_drag(
                root_x - d.off.0,
                root_y - d.off.1,
                p.w,
                p.h,
                self.area,
                to_i32(self.cfg.margin),
            );
            if pos != self.panel_pos {
                d.moved = true;
                self.panel_pos = pos;
                let (x, y) = self.window_pos();
                fx.push(Effect::Move { x, y });
            }
        }
        let hit = self.layout.hit(x, y);
        let hover = Some(hit).filter(|h| is_button(*h));
        if hover != self.hover {
            self.hover = hover;
            fx.push(Effect::Redraw);
        }
        // Only on a change: X keeps the window's cursor until told otherwise.
        // While dragging the panel the pointer keeps its shape.
        let cursor = cursor_for(hit, self.cfg.click_through);
        if cursor != self.cursor && self.drag.is_none() {
            self.cursor = cursor;
            fx.push(Effect::SetCursor(cursor));
        }
        fx
    }

    fn wheel(&mut self, x: i32, y: i32, up: bool, now: f64) -> Vec<Effect> {
        let dir = if up { -1 } else { 1 };
        match self.layout.hit(x, y) {
            Hit::Clock => {
                let step = if up { 2 } else { -2 };
                let size = (self.cfg.font_size + step).clamp(FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1);
                if size == self.cfg.font_size {
                    return Vec::new();
                }
                self.cfg.font_size = size;
                self.restyle();
                let mut fx = self.relayout(now);
                fx.extend([Effect::SaveSoon, Effect::Redraw]);
                fx
            }
            Hit::Mode => {
                let r = self.layout.regions.iter().find(|(h, _)| *h == Hit::Mode);
                let (pos_x, width) =
                    r.map_or((0.0, 1.0), |(_, r)| (f64::from(x - r.x), f64::from(r.w)));
                if self.cfg.timer_mode == Mode::Countdown && pos_x > width * 0.55 {
                    self.wheel_duration(if pos_x > width * 0.8 { 1 } else { 60 }, up, now)
                } else {
                    self.step_mode(dir, now)
                }
            }
            Hit::Timer => self.step_mode(dir, now),
            _ => Vec::new(),
        }
    }

    /// Countdown duration ±`step` seconds from the mode label (within
    /// [`COUNTDOWN_RANGE`]).
    fn wheel_duration(&mut self, step: i64, up: bool, now: f64) -> Vec<Effect> {
        let delta = if up {
            step.min(COUNTDOWN_RANGE.1 - self.cfg.countdown_duration)
                .max(0)
        } else {
            -step.min(self.cfg.countdown_duration - 1)
        };
        if delta == 0 {
            return Vec::new(); // at a range limit
        }
        self.cfg.countdown_duration += delta;
        self.cfg.active_preset.clear();
        self.engine.adjust_countdown(&self.cfg, delta as f64);
        self.changed(now)
    }
}
