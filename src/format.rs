use crate::config::{Config, Mode, Preset, interval_preset_rounds};
use crate::engine::Phase;
use chrono::{NaiveTime, Timelike};

/// Format seconds as MM:SS, or HH:MM:SS from one hour up (hours unbounded).
pub fn fmt_seconds(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, rem) = (s / 3600, s % 3600);
    let (m, sec) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h:02}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

/// Clock label text: "24h" HH:MM:SS, "24h-short" HH:MM, "12h" hh:MM:SS AM/PM.
pub fn clock_text(now: NaiveTime, format: &str) -> String {
    let (h, m, s) = (now.hour(), now.minute(), now.second());
    match format {
        "24h-short" => format!("{h:02}:{m:02}"),
        "12h" => {
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            let ampm = if h < 12 { "AM" } else { "PM" };
            format!("{h12:02}:{m:02}:{s:02} {ampm}")
        }
        _ => format!("{h:02}:{m:02}:{s:02}"),
    }
}

/// Seconds to show on the timer label: countdown/interval round up.
pub fn shown_seconds(mode: Mode, display: f64) -> f64 {
    match mode {
        Mode::Countdown | Mode::Interval => (display - 1e-9).ceil(),
        Mode::Stopwatch => display,
    }
}

/// Static mode label (port of overlay `_refresh_mode_label`).
pub fn mode_label(cfg: &Config) -> String {
    let preset = cfg.active_preset.to_uppercase();
    match cfg.timer_mode {
        Mode::Stopwatch => {
            if cfg.stopwatch_work > 0 {
                let base = format!("{}s/{}s ↑", cfg.stopwatch_work, cfg.stopwatch_rest);
                if preset.is_empty() {
                    format!("STOPWATCH {base}")
                } else {
                    format!("{preset} · {base}")
                }
            } else if preset.is_empty() {
                "STOPWATCH".into()
            } else {
                format!("{preset} · STOPWATCH")
            }
        }
        Mode::Interval => {
            let base = format!(
                "{}s/{}s ×{}",
                cfg.interval_work, cfg.interval_rest, cfg.interval_rounds
            );
            if preset.is_empty() {
                format!("INTERVAL {base}")
            } else {
                format!("{preset} · {base}")
            }
        }
        Mode::Countdown => {
            let dur = fmt_seconds(cfg.countdown_duration as f64);
            if preset.is_empty() {
                format!("COUNTDOWN  {dur}")
            } else {
                format!("{preset}  ·  {dur}")
            }
        }
    }
}

/// Live phase label, e.g. "WORK 3/8" (endless: "WORK 3").
pub fn phase_label(phase: Phase, round: i64, rounds: i64) -> String {
    let name = match phase {
        Phase::None => return String::new(),
        Phase::Work => "WORK",
        Phase::Rest => "REST",
    };
    if rounds > 0 {
        format!("{name} {round}/{rounds}")
    } else {
        format!("{name} {round}")
    }
}

/// Text of the mode button.
pub fn mode_button_text(mode: Mode) -> &'static str {
    match mode {
        Mode::Stopwatch => "SW",
        Mode::Countdown => "CD",
        Mode::Interval => "IV",
    }
}

/// Presets sub-menu label (port of `menus.preset_menu_label`).
pub fn preset_menu_label(p: &Preset) -> String {
    match p {
        Preset::Interval {
            name,
            work,
            rest,
            total,
        } => format!(
            "{name} {work}/{rest} ×{}",
            interval_preset_rounds(*work, *rest, *total)
        ),
        Preset::Stopwatch { name, work, rest } => format!("{name} {work}/{rest} ↑"),
        Preset::Countdown { name, duration } => {
            format!("{name} {}", fmt_seconds(*duration as f64))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod test_fmt_seconds {
        use super::*;

        #[test]
        fn under_a_minute() {
            assert_eq!(fmt_seconds(59.0), "00:59");
        }

        #[test]
        fn minutes() {
            assert_eq!(fmt_seconds(61.0), "01:01");
            assert_eq!(fmt_seconds(300.0), "05:00");
        }

        #[test]
        fn hour_boundary() {
            assert_eq!(fmt_seconds(3599.0), "59:59");
            assert_eq!(fmt_seconds(3600.0), "01:00:00");
        }

        #[test]
        fn large() {
            assert_eq!(fmt_seconds(86399.0), "23:59:59");
        }

        #[test]
        fn negative_clamps_to_zero() {
            assert_eq!(fmt_seconds(-3.0), "00:00");
        }

        #[test]
        fn fmt_seconds_over_100_hours() {
            assert_eq!(fmt_seconds(360000.0), "100:00:00");
        }
    }

    mod test_labels {
        use super::*;

        fn cfg(mode: Mode) -> Config {
            Config {
                timer_mode: mode,
                ..Config::default()
            }
        }

        #[test]
        fn clock_text_formats() {
            let t = NaiveTime::from_hms_opt(13, 5, 9).unwrap();
            assert_eq!(clock_text(t, "24h"), "13:05:09");
            assert_eq!(clock_text(t, "24h-short"), "13:05");
            assert_eq!(clock_text(t, "12h"), "01:05:09 PM");
            assert_eq!(clock_text(t, "bogus"), "13:05:09");
        }

        #[test]
        fn clock_text_12h_midnight_and_noon() {
            let m = NaiveTime::from_hms_opt(0, 7, 0).unwrap();
            let n = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
            assert_eq!(clock_text(m, "12h"), "12:07:00 AM");
            assert_eq!(clock_text(n, "12h"), "12:00:00 PM");
        }

        #[test]
        fn shown_seconds_ceils_countdown() {
            assert_eq!(shown_seconds(Mode::Countdown, 4.2), 5.0);
            assert_eq!(shown_seconds(Mode::Countdown, 5.0), 5.0);
            assert_eq!(shown_seconds(Mode::Interval, 4.2), 5.0);
            assert_eq!(shown_seconds(Mode::Stopwatch, 4.2), 4.2);
        }

        #[test]
        fn mode_label_countdown() {
            let mut c = cfg(Mode::Countdown);
            c.countdown_duration = 300;
            assert_eq!(mode_label(&c), "COUNTDOWN  05:00");
            c.active_preset = "Tea".into();
            assert_eq!(mode_label(&c), "TEA  ·  05:00");
        }

        #[test]
        fn mode_label_interval() {
            let mut c = cfg(Mode::Interval);
            c.interval_work = 40;
            c.interval_rest = 20;
            c.interval_rounds = 8;
            assert_eq!(mode_label(&c), "INTERVAL 40s/20s ×8");
            c.active_preset = "Tabata".into();
            assert_eq!(mode_label(&c), "TABATA · 40s/20s ×8");
        }

        #[test]
        fn mode_label_stopwatch_plain() {
            let mut c = cfg(Mode::Stopwatch);
            assert_eq!(mode_label(&c), "STOPWATCH");
            c.active_preset = "Run".into();
            assert_eq!(mode_label(&c), "RUN · STOPWATCH");
        }

        #[test]
        fn mode_label_stopwatch_cycling() {
            let mut c = cfg(Mode::Stopwatch);
            c.stopwatch_work = 60;
            c.stopwatch_rest = 30;
            assert_eq!(mode_label(&c), "STOPWATCH 60s/30s ↑");
            c.active_preset = "Run".into();
            assert_eq!(mode_label(&c), "RUN · 60s/30s ↑");
        }

        #[test]
        fn phase_labels() {
            assert_eq!(phase_label(Phase::Work, 3, 8), "WORK 3/8");
            assert_eq!(phase_label(Phase::Rest, 2, 8), "REST 2/8");
            assert_eq!(phase_label(Phase::Work, 3, 0), "WORK 3");
            assert_eq!(phase_label(Phase::None, 1, 8), "");
        }

        #[test]
        fn mode_button_texts() {
            assert_eq!(mode_button_text(Mode::Stopwatch), "SW");
            assert_eq!(mode_button_text(Mode::Countdown), "CD");
            assert_eq!(mode_button_text(Mode::Interval), "IV");
        }

        #[test]
        fn preset_menu_label_interval() {
            let p = Preset::Interval {
                name: "Tabata".into(),
                work: 20,
                rest: 10,
                total: 240,
            };
            assert_eq!(preset_menu_label(&p), "Tabata 20/10 ×8");
        }

        #[test]
        fn preset_menu_label_stopwatch() {
            let p = Preset::Stopwatch {
                name: "Run".into(),
                work: 60,
                rest: 30,
            };
            assert_eq!(preset_menu_label(&p), "Run 60/30 ↑");
        }

        #[test]
        fn preset_menu_label_countdown() {
            let p = Preset::Countdown {
                name: "Tea".into(),
                duration: 300,
            };
            assert_eq!(preset_menu_label(&p), "Tea 05:00");
        }
    }
}
