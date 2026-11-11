use super::*;
use serde_json::json;
use std::cell::Cell;
use std::rc::Rc;

const AREA: Rect = Rect {
    x: 0,
    y: 0,
    w: 1920,
    h: 1080,
};

struct Harness {
    app: App,
    t: Rc<Cell<f64>>,
}

impl Harness {
    fn now(&self) -> f64 {
        self.t.get()
    }
    fn advance(&self, secs: f64) {
        self.t.set(self.t.get() + secs);
    }
    fn tick(&mut self) -> Vec<Effect> {
        let now = self.now();
        self.app.tick(now, wall())
    }
    fn input(&mut self, input: Input) -> Vec<Effect> {
        let now = self.now();
        self.app.handle(input, now)
    }
    fn region(&self, hit: Hit) -> Rect {
        self.app
            .layout
            .regions
            .iter()
            .find(|(h, _)| *h == hit)
            .unwrap_or_else(|| panic!("no {hit:?} region"))
            .1
    }
    /// Left press in the middle of `hit` (window-local and root coordinates).
    fn press(&mut self, hit: Hit, button: Button) -> Vec<Effect> {
        let r = self.region(hit);
        let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
        let (wx, wy) = self.app.window_pos();
        self.input(Input::Press {
            x,
            y,
            root_x: wx + x,
            root_y: wy + y,
            button,
        })
    }
    fn wheel_at(&mut self, hit: Hit, frac_x: f64, up: bool) -> Vec<Effect> {
        let r = self.region(hit);
        let x = r.x + (f64::from(r.w) * frac_x) as i32;
        self.input(Input::Wheel {
            x,
            y: r.y + r.h / 2,
            up,
        })
    }
}

fn wall() -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap()
}

fn mk(cfg: Config) -> Harness {
    let t = Rc::new(Cell::new(1000.0));
    let c = t.clone();
    let app = App::new(
        cfg,
        Box::new(move || c.get()),
        FontSet::bundled(),
        1.0,
        AREA,
        1000.0,
    );
    Harness { app, t }
}

fn countdown(duration: i64) -> Config {
    Config {
        timer_mode: Mode::Countdown,
        countdown_duration: duration,
        ..Config::default()
    }
}

fn with_presets(presets: Vec<serde_json::Value>) -> Config {
    Config {
        presets,
        ..Config::default()
    }
}

// ── TestPresets ──────────────────────────────────────────────────────

#[test]
fn countdown_preset() {
    let mut h = mk(with_presets(vec![
        json!({"name": "5 min", "duration": 300}),
    ]));
    let now = h.now();
    h.app.perform(&MenuAction::ApplyPreset(0), now);
    assert_eq!(h.app.cfg.timer_mode, Mode::Countdown);
    assert_eq!(h.app.cfg.countdown_duration, 300);
    assert_eq!(h.app.cfg.active_preset, "5 min");
    assert!(h.app.mode_text().contains("5 MIN"), "{}", h.app.mode_text());
}

#[test]
fn interval_preset_rounds_from_total() {
    let mut h = mk(with_presets(vec![
        json!({"name": "hiit", "type": "interval", "work": 45, "rest": 15, "total": 600}),
    ]));
    let now = h.now();
    h.app.perform(&MenuAction::ApplyPreset(0), now);
    assert_eq!(h.app.cfg.timer_mode, Mode::Interval);
    assert_eq!(h.app.cfg.interval_rounds, 10);
}

#[test]
fn cycling_stopwatch_preset() {
    let mut h = mk(with_presets(vec![
        json!({"name": "up", "type": "stopwatch", "work": 45, "rest": 15}),
    ]));
    let now = h.now();
    h.app.perform(&MenuAction::ApplyPreset(0), now);
    assert_eq!(h.app.cfg.stopwatch_work, 45);
    h.app.toggle_timer(now);
    h.advance(50.0);
    h.tick();
    assert!(
        h.app.mode_text().contains("REST 1"),
        "{}",
        h.app.mode_text()
    );
}

#[test]
fn mode_toggle_clears_preset() {
    let mut h = mk(with_presets(vec![
        json!({"name": "5 min", "duration": 300}),
    ]));
    let now = h.now();
    h.app.perform(&MenuAction::ApplyPreset(0), now);
    let fx = h.press(Hit::BtnMode, Button::Left);
    assert_eq!(h.app.cfg.active_preset, "");
    assert_eq!(h.app.cfg.timer_mode, Mode::Interval);
    assert!(fx.contains(&Effect::SaveSoon));
}

#[test]
fn sound_toggles_applied_and_legacy_ignored() {
    let mut h = mk(Config {
        sound_interval: 90,
        ..with_presets(vec![
            json!({"name": "hiit", "type": "interval", "work": 45, "rest": 15,
                   "total": 600, "last5": true, "boundary": false, "halfway": true}),
            json!({"name": "legacy", "duration": 60, "every": 0, "before": 5}),
        ])
    });
    let now = h.now();
    h.app.perform(&MenuAction::ApplyPreset(0), now);
    assert!(h.app.cfg.alert_last_5_seconds);
    assert!(!h.app.cfg.phase_beeps);
    assert!(h.app.cfg.halfway_beep);
    // legacy every/before keys and plain presets leave sound_interval alone
    h.app.perform(&MenuAction::ApplyPreset(1), now);
    assert_eq!(h.app.cfg.sound_interval, 90);
    assert!(h.app.cfg.phase_beeps); // defaults restored
    assert!(!h.app.cfg.halfway_beep);
}

// ── TestBeepLabelAlignment ───────────────────────────────────────────

#[test]
fn shorts_on_displayed_5_to_1_long_on_zero() {
    let mut h = mk(Config {
        sound_enabled: true,
        alert_last_5_seconds: true,
        ..countdown(6)
    });
    let now = h.now();
    h.app.toggle_timer(now);
    let (mut shorts, mut longs) = (Vec::new(), Vec::new());
    for _ in 0..65 {
        h.advance(0.1);
        for fx in h.tick() {
            if let Effect::Beep(b) = fx {
                let label = h.app.timer_text().to_string();
                if b.short {
                    shorts.push(label);
                } else if !b.double {
                    longs.push(label);
                }
            }
        }
    }
    assert_eq!(shorts, ["00:05", "00:04", "00:03", "00:02", "00:01"]);
    assert_eq!(longs, ["00:00"]);
}

#[test]
fn label_starts_at_full_duration() {
    let mut h = mk(countdown(30));
    let now = h.now();
    h.app.toggle_timer(now);
    h.advance(0.1);
    h.tick();
    assert_eq!(h.app.timer_text(), "00:30");
}

// ── TestProgressStyles ───────────────────────────────────────────────

#[test]
fn line_border_off() {
    let mut h = mk(countdown(300));
    h.app.cfg.progress_style = "line".into();
    h.tick();
    assert_eq!(h.app.line_fraction, 1.0);
    assert_eq!(h.app.border_fraction, -1.0);
    h.app.cfg.progress_style = "border".into();
    h.tick();
    assert_eq!(h.app.line_fraction, -1.0);
    assert_eq!(h.app.border_fraction, 1.0);
    h.app.cfg.progress_style = "off".into();
    h.tick();
    assert_eq!(h.app.border_fraction, -1.0);
}

// ── Settings live preview ────────────────────────────────────────────

#[test]
fn countdown_duration_sync_keeps_idle() {
    let mut h = mk(countdown(300));
    assert!(h.app.engine.is_idle(&h.app.cfg));
    let mut edited = h.app.cfg.clone();
    edited.countdown_duration += 60;
    let now = h.now();
    h.app.preview_config(edited, now);
    assert!(
        h.app.engine.is_idle(&h.app.cfg),
        "duration edit must not fake-start the timer"
    );
}

#[test]
fn zero_sound_interval_survives_settings() {
    let mut h = mk(Config {
        sound_interval: 0,
        ..Config::default()
    });
    let mut edited = h.app.cfg.clone();
    edited.font_size += 2;
    let now = h.now();
    h.app.preview_config(edited, now);
    assert_eq!(h.app.cfg.sound_interval, 0);
    assert_eq!(h.app.cfg.font_size, 32);
}

#[test]
fn buttons_fit_after_font_growth() {
    let mut h = mk(Config::default());
    let now = h.now();
    h.app.fade_controls_to(0.0, now);
    h.advance(0.5);
    h.app.frame(h.now());
    assert_eq!(h.app.controls_pos(h.now()), 0.0);
    let now = h.now();
    h.app.fade_controls_to(1.0, now);
    h.advance(0.5);
    h.app.frame(h.now());
    assert_eq!(h.app.controls_pos(h.now()), 1.0);
    let mut edited = h.app.cfg.clone();
    edited.font_size = 60;
    let now = h.now();
    h.app.preview_config(edited, now);
    let (_, layout) = h.app.frame(now);
    let p = layout.panel;
    for hit in [Hit::BtnStart, Hit::BtnReset, Hit::BtnMode] {
        let r = h.region(hit);
        assert!(r.h >= 60 - 15, "{hit:?} height {}", r.h);
        assert!(
            r.x >= p.x && r.y >= p.y && r.x + r.w <= p.x + p.w && r.y + r.h <= p.y + p.h,
            "{hit:?} {r:?} outside panel {p:?}"
        );
    }
}

// ── Input ────────────────────────────────────────────────────────────

#[test]
fn double_click_resets() {
    let mut h = mk(Config::default());
    h.press(Hit::Timer, Button::Left);
    assert!(h.app.engine.running());
    h.advance(0.2);
    h.press(Hit::Timer, Button::Left);
    assert!(!h.app.engine.running());
    assert_eq!(h.app.engine.elapsed(), 0.0);
}

#[test]
fn slow_second_click_toggles() {
    let mut h = mk(Config::default());
    h.press(Hit::Timer, Button::Left);
    h.advance(0.6);
    h.press(Hit::Timer, Button::Left);
    assert!(!h.app.engine.running());
    assert!((h.app.engine.elapsed() - 0.6).abs() < 1e-9);
}

#[test]
fn wheel_on_mode_right_edge_adjusts_countdown() {
    let mut h = mk(Config {
        active_preset: "5 min".into(),
        ..countdown(300)
    });
    let fx = h.wheel_at(Hit::Mode, 0.9, true);
    assert_eq!(h.app.cfg.countdown_duration, 301);
    assert_eq!(h.app.cfg.active_preset, "");
    assert!(fx.contains(&Effect::SaveSoon));
    assert_eq!(h.app.engine.remaining(), 301.0);
    // middle band steps by a minute
    h.wheel_at(Hit::Mode, 0.6, true);
    assert_eq!(h.app.cfg.countdown_duration, 361);
    // left part cycles the mode instead
    h.wheel_at(Hit::Mode, 0.2, false);
    assert_eq!(h.app.cfg.timer_mode, Mode::Interval);
    assert_eq!(h.app.cfg.countdown_duration, 361);
}

#[test]
fn wheel_down_never_below_one() {
    let mut h = mk(countdown(1));
    h.wheel_at(Hit::Mode, 0.9, false);
    assert_eq!(h.app.cfg.countdown_duration, 1);
    let mut h = mk(countdown(30));
    h.wheel_at(Hit::Mode, 0.6, false);
    assert_eq!(h.app.cfg.countdown_duration, 1);
    h.wheel_at(Hit::Mode, 0.6, false);
    assert_eq!(h.app.cfg.countdown_duration, 1);
}

#[test]
fn wheel_up_stops_at_max_countdown() {
    use crate::config::COUNTDOWN_RANGE;
    let mut h = mk(countdown(COUNTDOWN_RANGE.1 - 30));
    h.wheel_at(Hit::Mode, 0.6, true);
    assert_eq!(h.app.cfg.countdown_duration, COUNTDOWN_RANGE.1);
}

#[test]
fn preview_config_clamps_out_of_range_values() {
    let mut h = mk(Config::default());
    let edited = Config {
        font_size: 5000,
        countdown_duration: 0,
        ..h.app.cfg.clone()
    };
    let now = h.now();
    h.app.preview_config(edited, now);
    assert_eq!(h.app.cfg.font_size, 120);
    assert_eq!(h.app.cfg.countdown_duration, 1);
}

#[test]
fn wheel_on_clock_changes_font_size() {
    let mut h = mk(Config::default());
    let fx = h.wheel_at(Hit::Clock, 0.5, true);
    assert_eq!(h.app.cfg.font_size, 32);
    assert!(fx.contains(&Effect::SaveSoon));
    h.wheel_at(Hit::Clock, 0.5, false);
    assert_eq!(h.app.cfg.font_size, 30);

    let mut h = mk(Config {
        font_size: 120,
        ..Config::default()
    });
    h.wheel_at(Hit::Clock, 0.5, true);
    assert_eq!(h.app.cfg.font_size, 120);
    let mut h = mk(Config {
        font_size: 10,
        ..Config::default()
    });
    h.wheel_at(Hit::Clock, 0.5, false);
    assert_eq!(h.app.cfg.font_size, 10);
}

#[test]
fn wheel_on_timer_cycles_mode() {
    let mut h = mk(Config::default());
    h.wheel_at(Hit::Timer, 0.5, false);
    assert_eq!(h.app.cfg.timer_mode, Mode::Countdown);
    h.wheel_at(Hit::Timer, 0.5, true);
    assert_eq!(h.app.cfg.timer_mode, Mode::Stopwatch);
    h.wheel_at(Hit::Timer, 0.5, true);
    assert_eq!(h.app.cfg.timer_mode, Mode::Interval);
}

#[test]
fn right_click_opens_menu() {
    let mut h = mk(Config::default());
    let fx = h.input(Input::Press {
        x: 5,
        y: 5,
        root_x: 100,
        root_y: 200,
        button: Button::Right,
    });
    assert!(fx.contains(&Effect::OpenMenu {
        root_x: 100,
        root_y: 200,
        include_window_actions: true
    }));
    let mut h = mk(Config {
        show_tray_icon: false,
        ..Config::default()
    });
    let fx = h.press(Hit::Timer, Button::Right);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::OpenMenu {
            include_window_actions: false,
            ..
        }
    )));
    assert!(!h.app.engine.running());
}

#[test]
fn drag_saves_custom_position() {
    let mut h = mk(Config::default());
    let (wx, wy) = h.app.window_pos();
    let p = h.app.layout.panel;
    let (lx, ly) = (p.x + 2, p.y + 2);
    assert_eq!(h.app.layout.hit(lx, ly), Hit::Background);
    h.input(Input::Press {
        x: lx,
        y: ly,
        root_x: wx + lx,
        root_y: wy + ly,
        button: Button::Left,
    });
    let fx = h.input(Input::Motion {
        x: lx,
        y: ly,
        root_x: wx + lx - 500,
        root_y: wy + ly + 300,
    });
    assert!(fx.contains(&Effect::Move {
        x: wx - 500,
        y: wy + 300
    }));
    assert_eq!(h.app.window_pos(), (wx - 500, wy + 300));
    let fx = h.input(Input::Release {
        root_x: wx + lx - 500,
        root_y: wy + ly + 300,
        button: Button::Left,
    });
    assert!(fx.contains(&Effect::SaveSoon));
    // custom position is the visible panel's top-left
    assert_eq!(
        (h.app.cfg.custom_x, h.app.cfg.custom_y),
        (i64::from(wx - 500 + p.x), i64::from(wy + 300 + p.y))
    );
}

#[test]
fn click_without_drag_keeps_preset_position() {
    let mut h = mk(Config::default());
    let fx = h.press(Hit::Clock, Button::Left);
    assert!(fx.is_empty(), "{fx:?}");
    h.input(Input::Release {
        root_x: 0,
        root_y: 0,
        button: Button::Left,
    });
    assert_eq!((h.app.cfg.custom_x, h.app.cfg.custom_y), (-1, -1));
}

#[test]
fn keys() {
    let mut h = mk(Config::default());
    h.input(Input::Key {
        key: Key::Space,
        ctrl: false,
    });
    assert!(h.app.engine.running());
    h.input(Input::Key {
        key: Key::R,
        ctrl: false,
    });
    assert!(!h.app.engine.running());
    let fx = h.input(Input::Key {
        key: Key::Escape,
        ctrl: false,
    });
    assert!(fx.contains(&Effect::SetVisible(false)));
    assert!(
        h.input(Input::Key {
            key: Key::Q,
            ctrl: false
        })
        .is_empty()
    );
    let fx = h.input(Input::Key {
        key: Key::Q,
        ctrl: true,
    });
    assert_eq!(fx, [Effect::SaveNow, Effect::Quit]);
}

// ── Animations / cadence ─────────────────────────────────────────────

#[test]
fn fold_after_leave_when_not_idle() {
    let mut h = mk(Config::default());
    let now = h.now();
    h.app.toggle_timer(now);
    h.input(Input::Enter);
    h.input(Input::Leave);
    h.advance(1.9);
    h.tick();
    assert_eq!(h.app.controls_pos(h.now()), 1.0);
    h.advance(0.6);
    h.tick();
    let now = h.now();
    assert!(h.app.animating(now) || h.app.controls_pos(now) == 0.0);
    assert!(h.app.controls_pos(now) < 1.0);
    h.advance(1.0);
    assert_eq!(h.app.controls_pos(h.now()), 0.0);
    // hovering again unfolds
    h.input(Input::Enter);
    h.advance(0.5);
    assert_eq!(h.app.controls_pos(h.now()), 1.0);
}

#[test]
fn stays_unfolded_when_idle() {
    let mut h = mk(Config::default());
    h.input(Input::Leave);
    h.advance(3.0);
    h.tick();
    assert_eq!(h.app.controls_pos(h.now()), 1.0);
    assert!(!h.app.animating(h.now()));
}

#[test]
fn idle_wakeup_aligns_to_second() {
    let mut h = mk(Config::default());
    h.advance(1.0); // fade-in done
    h.tick();
    assert!(!h.app.animating(h.now()));
    assert!((h.app.next_wakeup(h.now(), 0.3) - 0.7).abs() < 1e-9);
}

#[test]
fn running_wakeup_100ms() {
    let mut h = mk(Config::default());
    let now = h.now();
    h.app.toggle_timer(now);
    assert!((h.app.next_wakeup(now, 0.3) - 0.1).abs() < 1e-9);
}

#[test]
fn hidden_hud_does_not_animate() {
    let mut h = mk(countdown(60));
    let now = h.now();
    h.app.toggle_timer(now);
    h.advance(1.0);
    h.tick();
    assert!(h.app.animating(h.now()), "running progress bar animates");
    h.app.set_visible(false);
    assert!(!h.app.animating(h.now()));
    assert!(
        (h.app.next_wakeup(h.now(), 0.5) - 0.1).abs() < 1e-9,
        "beeps keep ticking"
    );
}

#[test]
fn fade_in_reaches_opacity() {
    let h = mk(Config::default());
    assert_eq!(h.app.opacity(h.now()), 0.0);
    assert!(h.app.animating(h.now()));
    h.advance(0.3);
    assert!((h.app.opacity(h.now()) - 0.88).abs() < 1e-9);
}

#[test]
fn pulse_on_final_beeps() {
    let mut h = mk(Config {
        alert_last_5_seconds: true,
        ..countdown(10)
    });
    let now = h.now();
    h.app.toggle_timer(now);
    h.advance(5.05);
    let fx = h.tick();
    assert!(fx.iter().any(|e| matches!(e, Effect::Beep(b) if b.short)));
    h.advance(0.09);
    let mul = h.app.timer_px_mul(h.now());
    assert!(mul > 1.03 && mul <= 1.04, "{mul}");
    h.advance(0.2);
    assert_eq!(h.app.timer_px_mul(h.now()), 1.0);
}

#[test]
fn notify_on_finish_effect() {
    for (notify, expected) in [(true, 1), (false, 0)] {
        let mut h = mk(Config {
            notify_on_finish: notify,
            ..countdown(2)
        });
        let now = h.now();
        h.app.toggle_timer(now);
        let mut notes = Vec::new();
        for _ in 0..30 {
            h.advance(0.1);
            notes.extend(h.tick().into_iter().filter_map(|e| match e {
                Effect::Notify(s) => Some(s),
                _ => None,
            }));
        }
        assert_eq!(notes.len(), expected);
        if notify {
            assert_eq!(notes[0], "Countdown finished");
        }
    }
}

#[test]
fn notify_skipped_on_auto_restart() {
    // Python parity: finish handling only when `finished && !restarted`.
    let mut h = mk(Config {
        notify_on_finish: true,
        auto_restart_countdown: true,
        ..countdown(2)
    });
    let now = h.now();
    h.app.toggle_timer(now);
    let mut notes = 0;
    for _ in 0..70 {
        h.advance(0.1);
        notes += h
            .tick()
            .iter()
            .filter(|e| matches!(e, Effect::Notify(_)))
            .count();
    }
    assert!(h.app.engine.running(), "still cycling");
    assert_eq!(notes, 0);
}

#[test]
fn startup_clamps_offscreen_custom_pos() {
    // e.g. saved on a monitor that is no longer connected (-1 means "preset")
    let h = mk(Config {
        custom_x: 5000,
        custom_y: 3000,
        ..Config::default()
    });
    let (wx, wy) = h.app.window_pos();
    let p = h.app.layout.panel;
    assert_eq!((wx + p.x, wy + p.y), (AREA.w - p.w, AREA.h - p.h));
}

#[test]
fn preset_position_top_right() {
    let h = mk(Config::default());
    let (wx, wy) = h.app.window_pos();
    let p = h.app.layout.panel;
    assert_eq!((wx + p.x, wy + p.y), (AREA.w - p.w - 15, 15));
}

#[test]
fn menu_actions() {
    let mut h = mk(Config::default());
    let now = h.now();
    let fx = h.app.perform(&MenuAction::ToggleClickThrough, now);
    assert!(h.app.cfg.click_through);
    assert!(fx.contains(&Effect::SetClickThrough(true)) && fx.contains(&Effect::SaveSoon));
    let fx = h.app.perform(&MenuAction::ManagePresets, now);
    assert_eq!(
        fx,
        [Effect::OpenSettings {
            tab: Some("presets".into())
        }]
    );
    assert_eq!(
        h.app.perform(&MenuAction::SavePreset, now),
        [Effect::OpenSavePreset]
    );
    assert_eq!(
        h.app.perform(&MenuAction::Quit, now),
        [Effect::SaveNow, Effect::Quit]
    );
    h.app.perform(&MenuAction::SetOpacity(0.5), now);
    assert_eq!(h.app.cfg.opacity, 0.5);
    h.app.cfg.custom_x = 100;
    h.app.cfg.custom_y = 100;
    let fx = h.app.perform(&MenuAction::SetPosition("bottom-left"), now);
    assert_eq!((h.app.cfg.custom_x, h.app.cfg.custom_y), (-1, -1));
    assert!(fx.iter().any(|e| matches!(e, Effect::Move { .. })));
    let p = h.app.layout.panel;
    let (wx, wy) = h.app.window_pos();
    assert_eq!((wx + p.x, wy + p.y + p.h), (15, 1080 - 15));
    h.app.font_loader = |_| FontSet::bundled();
    h.app.perform(&MenuAction::SetTheme("glass"), now);
    assert_eq!(h.app.cfg.theme, "glass");
    assert_eq!(h.app.font_family, "Noto Sans");
    assert_eq!(h.app.toggle_visibility(), [Effect::SetVisible(false)]);
    assert_eq!(
        h.app.perform(&MenuAction::ToggleVisibility, now),
        [Effect::SetVisible(true)]
    );
}

#[test]
fn save_preset_overwrites_same_name() {
    let mut h = mk(Config {
        presets: vec![
            json!({"name": "a", "duration": 60, "custom": 1}),
            json!({"name": "b", "duration": 120}),
        ],
        ..countdown(90)
    });
    let fx = h.app.save_preset("  b ").unwrap();
    assert!(fx.contains(&Effect::SaveSoon));
    assert_eq!(h.app.cfg.active_preset, "b");
    assert_eq!(
        h.app.cfg.presets,
        [
            json!({"name": "a", "duration": 60, "custom": 1}),
            json!({"name": "b", "duration": 90, "last5": false, "boundary": true, "halfway": false}),
        ]
    );
    assert!(h.app.save_preset("   ").is_err());
}

#[test]
fn save_preset_rejects_what_the_loader_would_drop() {
    // A plain stopwatch (work 0) is no valid preset: nothing is stored and
    // the active preset stays as it was.
    let mut h = mk(Config {
        timer_mode: Mode::Stopwatch,
        stopwatch_work: 0,
        active_preset: "old".into(),
        ..with_presets(vec![json!({"name": "a", "duration": 60})])
    });
    let before = h.app.cfg.clone();
    assert!(h.app.save_preset("plain").is_err());
    assert_eq!(h.app.cfg, before);
    // A cycling stopwatch is fine.
    h.app.cfg.stopwatch_work = 30;
    h.app.cfg.stopwatch_rest = 10;
    h.app.save_preset("cyc").unwrap();
    assert_eq!(h.app.cfg.active_preset, "cyc");
    assert_eq!(h.app.cfg.presets.len(), 2);
}

#[test]
fn preview_mode_change_clears_preset() {
    let mut h = mk(with_presets(vec![
        json!({"name": "TEA", "type": "interval", "work": 40, "rest": 20, "total": 480}),
    ]));
    let now = h.now();
    h.app.perform(&MenuAction::ApplyPreset(0), now);
    assert_eq!(h.app.cfg.active_preset, "TEA");
    let mut edited = h.app.cfg.clone();
    edited.timer_mode = Mode::Countdown;
    h.app.preview_config(edited, now);
    assert_eq!(h.app.cfg.active_preset, "");
    assert!(!h.app.mode_text().contains("TEA"), "{}", h.app.mode_text());
    // A config that sets mode and preset together (Cancel restoring both)
    // keeps its preset.
    let mut restored = h.app.cfg.clone();
    restored.timer_mode = Mode::Interval;
    restored.active_preset = "TEA".into();
    h.app.preview_config(restored, now);
    assert_eq!(h.app.cfg.active_preset, "TEA");
}

#[test]
fn preview_toggles_tray_and_resets_interval() {
    let mut h = mk(Config {
        timer_mode: Mode::Interval,
        ..Config::default()
    });
    let now = h.now();
    h.app.toggle_timer(now);
    h.advance(3.0);
    h.app.toggle_timer(h.now()); // paused mid-phase
    let mut edited = h.app.cfg.clone();
    edited.interval_work = 30;
    edited.show_tray_icon = false;
    let fx = h.app.preview_config(edited, h.now());
    assert!(fx.contains(&Effect::SetTrayVisible(false)));
    assert!(h.app.engine.is_idle(&h.app.cfg));
    assert_eq!(h.app.engine.remaining(), 30.0);
}

#[test]
fn pause_reset_and_mode_change_update_color_immediately() {
    let mut h = mk(countdown(60));
    let pause = h.app.cfg.color_timer_pause.clone();
    let run = h.app.cfg.color_timer_run.clone();
    let now = h.now();
    h.app.toggle_timer(now);
    assert_eq!(h.app.color_target, run, "start shows the run color at once");
    h.advance(2.0);
    h.tick();
    let now = h.now();
    h.app.toggle_timer(now);
    assert_eq!(
        h.app.color_target, pause,
        "pause must not wait for the next tick"
    );
    assert!((h.app.progress_target - 58.0 / 60.0).abs() < 1e-9);

    h.app.toggle_timer(h.now());
    h.advance(1.0);
    h.tick();
    h.press(Hit::BtnReset, Button::Left);
    assert_eq!(h.app.color_target, pause);
    assert_eq!(h.app.progress_target, 1.0);

    h.app.toggle_timer(h.now());
    h.advance(1.0);
    h.tick();
    h.press(Hit::BtnMode, Button::Left);
    assert_eq!(h.app.cfg.timer_mode, Mode::Interval);
    assert_eq!(h.app.color_target, pause);
}

// ── Frame deadlines ──────────────────────────────────────────────────

/// Running 5-min countdown, fade-in and color fade over, controls unfolded.
fn running_countdown() -> Harness {
    let mut h = mk(countdown(300));
    let now = h.now();
    h.app.toggle_timer(now);
    h.advance(1.05);
    h.tick();
    h
}

#[test]
fn live_progress_frames_are_sparse() {
    let mut h = running_countdown();
    h.app.frame(h.now()); // layout with the progress row
    let now = h.now();
    let next = h.app.next_frame(now).expect("running bar still advances");
    assert!(next - now >= 0.5, "line bar frame after {} s", next - now);
}

#[test]
fn border_progress_frames_follow_perimeter() {
    let mut h = running_countdown();
    let mut cfg = h.app.cfg.clone();
    cfg.progress_style = "border".into();
    let now = h.now();
    h.app.preview_config(cfg, now);
    h.advance(0.5); // let the fold reset settle
    h.tick();
    h.app.frame(h.now());
    let now = h.now();
    let dt = h.app.next_frame(now).unwrap() - now;
    // Longer path than the line: more frequent, but far from 60 fps.
    assert!((FRAME..0.5).contains(&dt), "border frame after {dt} s");
}

#[test]
fn tweens_frame_at_full_rate() {
    let mut h = running_countdown();
    h.input(Input::Enter);
    h.input(Input::Leave);
    h.advance(2.1); // fold starts at 2.0 s
    h.tick();
    let now = h.now();
    let dt = h.app.next_frame(now).unwrap() - now;
    assert!((dt - FRAME).abs() < 1e-9, "fold frame after {dt} s");
}

#[test]
fn paused_needs_no_frames() {
    let mut h = mk(countdown(300));
    h.advance(1.0); // fade-in over
    h.tick();
    assert_eq!(h.app.next_frame(h.now()), None);
    h.app.set_visible(false);
    let now = h.now();
    h.app.toggle_timer(now);
    assert_eq!(h.app.next_frame(h.now()), None, "hidden: no frames");
}

#[test]
fn running_tick_redraws_only_on_visible_change() {
    let mut h = running_countdown();
    h.advance(0.1); // same displayed second
    assert!(!h.tick().contains(&Effect::Redraw));
    h.advance(1.0); // next second: the timer text changes
    assert!(h.tick().contains(&Effect::Redraw));
}
