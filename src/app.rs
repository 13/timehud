//! Platform-free HUD controller (port of `overlay.py`): turns input events and
//! clock ticks into state changes, animations and `Effect`s for the platform layer.
//!
//! Positions are tracked for the visible panel (Python's window); the window
//! itself is larger by the theme's shadow inset (`Layout::panel.x/y`).
use crate::anim::{Ease, Tween, lerp_rgb};
use crate::config::{Config, Mode, Preset, PresetRules, interval_preset_rounds, valid_presets};
use crate::engine::{ClockFn, Phase, State, TimerEngine};
use crate::format::{
    clock_text, fmt_seconds, mode_button_text, mode_label, phase_label, shown_seconds,
};
use crate::layout::{Rect, clamp_into, preset_position, resize_anchor_delta, snap_drag};
use crate::menu::MenuAction;
use crate::render::{Hit, Layout, View, measure, render};
use crate::text::FontSet;
use crate::themes::{Rgb, apply_theme, get_theme, parse_hex};
use std::cell::RefCell;

mod events;
#[cfg(test)]
mod tests;

pub use events::{Button, Effect, Input, Key};

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
    line_fraction: f64,
    border_fraction: f64,
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
    visible: bool,
    // Geometry
    panel_pos: (i32, i32),
    layout: Layout,
    // Settings sync (port of `_sync_*`)
    last_cd_duration: i64,
    last_interval_cfg: (i64, i64, i64),
    last_show_tray_icon: bool,
}

fn hex(s: &str, fallback: Rgb) -> Rgb {
    parse_hex(s).unwrap_or(fallback)
}

fn to_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Font family the HUD draws with: theme font, else config font, else Monospace.
fn resolved_family(cfg: &Config) -> String {
    let theme = get_theme(&cfg.theme);
    [theme.font_family, cfg.font_family.as_str()]
        .into_iter()
        .find(|f| !f.is_empty())
        .unwrap_or("Monospace")
        .to_string()
}

fn interval_cfg(cfg: &Config) -> (i64, i64, i64) {
    (cfg.interval_work, cfg.interval_rest, cfg.interval_rounds)
}

/// Qt `QRect::center()` on one axis.
fn qt_center(pos: i32, len: i32) -> f64 {
    f64::from((2 * pos + len - 1) / 2)
}

fn copy_layout(l: &Layout) -> Layout {
    Layout {
        w: l.w,
        h: l.h,
        panel: l.panel,
        regions: l.regions.clone(),
    }
}

fn is_button(h: Hit) -> bool {
    matches!(h, Hit::BtnStart | Hit::BtnReset | Hit::BtnMode)
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
            line_fraction: -1.0,
            border_fraction: -1.0,
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
            visible: true,
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
            last_cd_duration: cfg.countdown_duration,
            last_interval_cfg: interval_cfg(&cfg),
            last_show_tray_icon: cfg.show_tray_icon,
            cfg,
        };
        app.update_timer(now);
        app.layout = app.measure(now);
        let p = app.layout.panel;
        app.panel_pos = if app.has_custom_pos() {
            // A saved position from another monitor setup must stay reachable.
            clamp_into(
                to_i32(app.cfg.custom_x),
                to_i32(app.cfg.custom_y),
                p.w,
                p.h,
                area,
            )
        } else {
            app.preset_pos()
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
        if !self.visible {
            return false;
        }
        let pulsing = self.pulse_start.is_some_and(|s| now - s < PULSE);
        let gliding = self.progress_glide.as_ref().is_some_and(|g| !g.done(now));
        // While running, the progress bar reads engine time every frame.
        let live_progress = self.engine.running() && self.progress_target >= 0.0;
        !self.fade_in.done(now)
            || !self.color_anim.done(now)
            || !self.controls.done(now)
            || pulsing
            || gliding
            || live_progress
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
        self.cfg.custom_x >= 0 && self.cfg.custom_y >= 0
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
    pub fn frame(&mut self, now: f64) -> (tiny_skia::Pixmap, Layout) {
        self.poll(now);
        let (pm, layout) = {
            let view = self.view(now);
            render(&view, &mut self.fonts.borrow_mut())
        };
        self.apply_layout(copy_layout(&layout));
        (pm, layout)
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
        if self.cfg.show_clock {
            self.clock = clock_text(wall, &self.cfg.clock_format);
        }
        let mut fx = self.update_timer(now);
        if before != (self.clock.len(), self.timer.len(), self.mode_text.clone()) {
            fx.extend(self.relayout(now));
        }
        fx.push(Effect::Redraw);
        fx
    }

    fn update_timer(&mut self, now: f64) -> Vec<Effect> {
        if !self.cfg.show_timer {
            return Vec::new();
        }
        let r = self.engine.tick(&self.cfg);
        let theme = get_theme(&self.cfg.theme);
        let countdown = self.cfg.timer_mode == Mode::Countdown;
        self.timer = fmt_seconds(shown_seconds(self.cfg.timer_mode, r.display));

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
        self.set_timer_color(&color, animate, now);

        let bar = if matches!(r.state, State::Warn | State::End) {
            theme.color_warn
        } else if r.phase == Phase::Rest {
            theme.color_rest
        } else {
            theme.color_clock
        };
        self.bar_color = hex(bar, Rgb(255, 255, 255));
        let style = self.cfg.progress_style.as_str();
        self.line_fraction = if style == "line" { r.progress } else { -1.0 };
        self.border_fraction = if style == "border" { r.progress } else { -1.0 };
        self.set_progress(self.line_fraction.max(self.border_fraction), now);

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
        if !r.beeps.is_empty() && countdown && r.display <= FINAL_SECONDS {
            self.pulse_start = Some(now);
        }
        if r.finished && self.cfg.notify_on_finish {
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
        if new == self.cfg.timer_mode {
            return Vec::new();
        }
        self.cfg.active_preset.clear(); // manual mode change ends the preset
        self.engine.set_mode(&mut self.cfg, new);
        self.refresh_mode_label();
        let mut fx = vec![Effect::SaveSoon];
        fx.extend(self.sync(now));
        fx
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
                self.refresh_mode_label();
                let mut fx = vec![Effect::SaveSoon];
                fx.extend(self.sync(now));
                fx
            }
            MenuAction::ApplyPreset(index) => self.apply_preset(*index, now),
            MenuAction::SetTheme(name) => {
                apply_theme(&mut self.cfg, name);
                self.restyle();
                let mut fx = vec![Effect::SaveSoon];
                fx.extend(self.sync(now));
                fx
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
            MenuAction::Quit => vec![Effect::SaveNow, Effect::Quit],
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
        let cfg = &mut self.cfg;
        match &preset {
            Preset::Stopwatch { work, rest, .. } => {
                cfg.timer_mode = Mode::Stopwatch;
                cfg.stopwatch_work = *work;
                cfg.stopwatch_rest = *rest;
            }
            Preset::Interval {
                work, rest, total, ..
            } => {
                cfg.timer_mode = Mode::Interval;
                cfg.interval_work = *work;
                cfg.interval_rest = *rest;
                cfg.interval_rounds = interval_preset_rounds(*work, *rest, *total);
                self.last_interval_cfg = interval_cfg(cfg);
            }
            Preset::Countdown { duration, .. } => {
                cfg.timer_mode = Mode::Countdown;
                cfg.countdown_duration = *duration;
                self.last_cd_duration = *duration;
            }
        }
        // Optional per-preset sound rules override the global ones
        if let Some(last5) = rules.last5 {
            cfg.alert_last_5_seconds = last5;
        }
        // Phase-end long beep defaults ON; halfway double defaults OFF
        cfg.phase_beeps = rules.boundary.unwrap_or(true);
        cfg.halfway_beep = rules.halfway.unwrap_or(false);
        cfg.active_preset = preset.name().to_string();
        self.engine.reset(&self.cfg);
        self.refresh_mode_label();
        let mut fx = vec![Effect::SaveSoon];
        fx.extend(self.sync(now));
        fx
    }

    /// Save the current timer setup as a preset (same name overwrites).
    pub fn save_preset(&mut self, name: &str) -> Vec<Effect> {
        let name = name.trim();
        if name.is_empty() {
            return Vec::new();
        }
        let cfg = &self.cfg;
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
        // Like Python, malformed entries are dropped; kept ones stay verbatim.
        let mut presets: Vec<_> = valid_presets(&cfg.presets)
            .into_iter()
            .filter(|(p, _, _)| p.name() != name)
            .map(|(_, _, i)| cfg.presets[i].clone())
            .collect();
        presets.push(preset.to_value(&rules));
        self.cfg.presets = presets;
        self.cfg.active_preset = name.into();
        self.refresh_mode_label();
        vec![Effect::SaveSoon, Effect::Redraw]
    }

    /// Live-apply a config edited in Settings (port of `update_ui` + `_sync_*`).
    /// Not saved here: the settings flow commits with `SaveNow`.
    pub fn preview_config(&mut self, cfg: Config, now: f64) -> Vec<Effect> {
        self.poll(now);
        let old = std::mem::replace(&mut self.cfg, cfg);
        let mut fx = Vec::new();
        // A manual duration change ends the preset, unless the edit already
        // changed `active_preset` itself (e.g. Cancel restoring a snapshot).
        if self.cfg.countdown_duration != old.countdown_duration
            && self.cfg.active_preset == old.active_preset
        {
            self.cfg.active_preset.clear();
        }
        if self.cfg.timer_mode != old.timer_mode {
            self.engine.reset(&self.cfg);
        }
        if !self.cfg.show_timer {
            fx.extend(self.reset_timer(now));
        }
        // Countdown duration: stopped engines reload it (stays idle).
        if self.cfg.countdown_duration != self.last_cd_duration {
            self.last_cd_duration = self.cfg.countdown_duration;
            if !self.engine.running() {
                self.engine.adjust_countdown(&self.cfg, 0.0);
            }
        }
        // Interval settings: reset a stopped interval engine.
        let iv = interval_cfg(&self.cfg);
        if iv != self.last_interval_cfg {
            self.last_interval_cfg = iv;
            if self.cfg.timer_mode == Mode::Interval && !self.engine.running() {
                self.engine.reset(&self.cfg);
            }
        }
        self.restyle();
        if !self.cfg.show_timer {
            self.line_fraction = -1.0;
            self.border_fraction = -1.0;
            self.set_progress(-1.0, now);
        }
        if self.cfg.show_timer && self.cfg.show_controls {
            self.reset_controls_fold(now);
        }
        if self.cfg.show_tray_icon != self.last_show_tray_icon {
            self.last_show_tray_icon = self.cfg.show_tray_icon;
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
                ..
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
                Key::Q if ctrl => vec![Effect::SaveNow, Effect::Quit],
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
        let hover = Some(self.layout.hit(x, y)).filter(|h| is_button(*h));
        if hover != self.hover {
            self.hover = hover;
            fx.push(Effect::Redraw);
        }
        fx
    }

    fn wheel(&mut self, x: i32, y: i32, up: bool, now: f64) -> Vec<Effect> {
        let dir = if up { -1 } else { 1 };
        match self.layout.hit(x, y) {
            Hit::Clock => {
                let step = if up { 2 } else { -2 };
                let size = (self.cfg.font_size + step).clamp(10, 120);
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

    /// Countdown duration ±`step` seconds from the mode label (never below 1 s).
    fn wheel_duration(&mut self, step: i64, up: bool, now: f64) -> Vec<Effect> {
        let delta = if up {
            step
        } else {
            -step.min(self.cfg.countdown_duration - 1)
        };
        if delta != 0 {
            self.cfg.countdown_duration += delta;
            self.last_cd_duration = self.cfg.countdown_duration;
            self.cfg.active_preset.clear();
            self.engine.adjust_countdown(&self.cfg, delta as f64);
        }
        self.refresh_mode_label();
        let mut fx = vec![Effect::SaveSoon];
        fx.extend(self.sync(now));
        fx
    }
}
