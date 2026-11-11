//! Built-in visual themes (port of `themes.py`, plus Nord and Amber).

use crate::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Parse `#RRGGBB` (case-insensitive).
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Rgb(p(0)?, p(2)?, p(4)?))
}

pub struct Theme {
    pub name: &'static str,
    pub label: &'static str,
    /// Digit font; "" = keep `config.font_family`.
    pub font_family: &'static str,
    pub color_bg: &'static str,
    pub bg_alpha: u8,
    /// 0 = no border.
    pub border_alpha: u8,
    /// Lighter 1px inner top line (Glass); 0 = off.
    pub top_edge_alpha: u8,
    pub radius: f32,
    pub color_clock: &'static str,
    pub clock_alpha: f32,
    pub color_timer_run: &'static str,
    pub color_timer_pause: &'static str,
    pub color_warn: &'static str,
    pub color_end: &'static str,
    pub color_rest: &'static str,
    pub show_separator: bool,
    pub clock_scale: f32,
    pub timer_scale: f32,
    /// 0 = no shadow.
    pub shadow_alpha: u8,
}

pub static THEMES: [Theme; 6] = [
    Theme {
        name: "classic",
        label: "Classic",
        font_family: "",
        color_bg: "#000000",
        bg_alpha: 185,
        border_alpha: 38,
        top_edge_alpha: 0,
        radius: 13.0,
        color_clock: "#00FF88",
        clock_alpha: 1.0,
        color_timer_run: "#FFFFFF",
        color_timer_pause: "#888888",
        color_warn: "#FF9900",
        color_end: "#FF3333",
        color_rest: "#4FA8FF",
        show_separator: false,
        clock_scale: 1.0,
        timer_scale: 1.25,
        shadow_alpha: 70,
    },
    Theme {
        name: "terminal",
        label: "Terminal",
        font_family: "Monospace",
        color_bg: "#0A0C0B",
        bg_alpha: 199,
        border_alpha: 26,
        top_edge_alpha: 0,
        radius: 14.0,
        color_clock: "#4ADE80",
        clock_alpha: 0.92,
        color_timer_run: "#F4F6F5",
        color_timer_pause: "#6E7672",
        color_warn: "#FBBF24",
        color_end: "#FF3333",
        color_rest: "#60A5FA",
        show_separator: true,
        clock_scale: 0.9,
        timer_scale: 1.3,
        shadow_alpha: 70,
    },
    Theme {
        name: "glass",
        label: "Glass",
        font_family: "Noto Sans",
        color_bg: "#101214",
        bg_alpha: 168,
        border_alpha: 36,
        top_edge_alpha: 56,
        radius: 18.0,
        color_clock: "#E8EDEB",
        clock_alpha: 0.68,
        color_timer_run: "#FFFFFF",
        color_timer_pause: "#8A9490",
        color_warn: "#FBBF24",
        color_end: "#FF3333",
        color_rest: "#93C5FD",
        show_separator: false,
        clock_scale: 0.8,
        timer_scale: 1.4,
        shadow_alpha: 90,
    },
    Theme {
        name: "mono",
        label: "Mono",
        font_family: "Monospace",
        color_bg: "#000000",
        bg_alpha: 217,
        border_alpha: 0,
        top_edge_alpha: 0,
        radius: 10.0,
        color_clock: "#FFFFFF",
        clock_alpha: 0.55,
        color_timer_run: "#FFFFFF",
        color_timer_pause: "#7A7A7A",
        color_warn: "#FBBF24",
        color_end: "#FF3333",
        color_rest: "#B0BEC5",
        show_separator: false,
        clock_scale: 0.62,
        timer_scale: 1.5,
        shadow_alpha: 0,
    },
    Theme {
        name: "nord",
        label: "Nord",
        font_family: "",
        color_bg: "#2E3440",
        bg_alpha: 205,
        border_alpha: 30,
        top_edge_alpha: 0,
        radius: 14.0,
        color_clock: "#88C0D0",
        clock_alpha: 1.0,
        color_timer_run: "#ECEFF4",
        color_timer_pause: "#7B8394",
        color_warn: "#EBCB8B",
        color_end: "#BF616A",
        color_rest: "#81A1C1",
        show_separator: false,
        clock_scale: 0.9,
        timer_scale: 1.3,
        shadow_alpha: 70,
    },
    Theme {
        name: "amber",
        label: "Amber",
        font_family: "Monospace",
        color_bg: "#140C00",
        bg_alpha: 215,
        border_alpha: 0,
        top_edge_alpha: 0,
        radius: 8.0,
        color_clock: "#FFB000",
        clock_alpha: 0.8,
        color_timer_run: "#FFC233",
        color_timer_pause: "#8A6A20",
        color_warn: "#FF7A00",
        color_end: "#FF3B1F",
        color_rest: "#E0A040",
        show_separator: true,
        clock_scale: 0.9,
        timer_scale: 1.35,
        shadow_alpha: 60,
    },
];

/// Look up a theme by name; unknown names fall back to classic.
pub fn get_theme(name: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.name == name).unwrap_or(&THEMES[0])
}

/// Stamp a theme's colors into config and set `config.theme`.
pub fn apply_theme(cfg: &mut Config, name: &str) -> &'static Theme {
    let t = get_theme(name);
    cfg.theme = t.name.into();
    cfg.color_bg = t.color_bg.into();
    cfg.color_clock = t.color_clock.into();
    cfg.color_timer_run = t.color_timer_run.into();
    cfg.color_timer_pause = t.color_timer_pause.into();
    if !t.font_family.is_empty() {
        cfg.font_family = t.font_family.into();
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_builtin_themes_in_order() {
        let names: Vec<_> = THEMES.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            ["classic", "terminal", "glass", "mono", "nord", "amber"]
        );
    }

    #[test]
    fn names_are_unique_and_lookup_by_name() {
        for t in THEMES.iter() {
            assert!(std::ptr::eq(get_theme(t.name), t));
        }
    }

    #[test]
    fn get_theme_fallback() {
        assert!(std::ptr::eq(get_theme("nope"), &THEMES[0]));
        assert!(std::ptr::eq(get_theme("glass"), &THEMES[2]));
    }

    #[test]
    fn classic_values() {
        let t = get_theme("classic");
        assert_eq!(t.color_bg, "#000000");
        assert_eq!(t.bg_alpha, 185);
        assert_eq!(t.border_alpha, 38);
        assert_eq!(t.top_edge_alpha, 0);
        assert_eq!(t.radius, 13.0);
        assert_eq!(t.color_clock, "#00FF88");
        assert_eq!(t.clock_alpha, 1.0);
        assert_eq!(t.color_timer_run, "#FFFFFF");
        assert_eq!(t.color_timer_pause, "#888888");
        assert_eq!(t.color_warn, "#FF9900");
        assert_eq!(t.color_end, "#FF3333");
        assert_eq!(t.color_rest, "#4FA8FF");
        assert!(!t.show_separator);
        assert_eq!(t.clock_scale, 1.0);
        assert_eq!(t.timer_scale, 1.25);
        assert_eq!(t.font_family, "");
        assert_eq!(t.shadow_alpha, 70);
    }

    #[test]
    fn apply_stamps_colors_and_sets_theme() {
        let mut cfg = Config::default();
        apply_theme(&mut cfg, "mono");
        let m = get_theme("mono");
        assert_eq!(cfg.theme, "mono");
        assert_eq!(cfg.color_bg, m.color_bg);
        assert_eq!(cfg.color_clock, m.color_clock);
        assert_eq!(cfg.color_timer_run, m.color_timer_run);
        assert_eq!(cfg.color_timer_pause, m.color_timer_pause);
        assert_eq!(cfg.font_family, "Monospace");
    }

    #[test]
    fn classic_keeps_user_font() {
        let mut cfg = Config {
            font_family: "JetBrains Mono".into(),
            ..Config::default()
        };
        apply_theme(&mut cfg, "classic");
        assert_eq!(cfg.font_family, "JetBrains Mono");
    }

    #[test]
    fn apply_unknown_name_falls_back_to_classic() {
        let mut cfg = Config::default();
        apply_theme(&mut cfg, "does-not-exist");
        assert_eq!(cfg.theme, "classic");
    }

    #[test]
    fn default_config_theme_is_classic() {
        assert_eq!(Config::default().theme, "classic");
    }

    #[test]
    fn all_themes_have_color_rest() {
        let expected = [
            ("classic", "#4FA8FF"),
            ("terminal", "#60A5FA"),
            ("glass", "#93C5FD"),
            ("mono", "#B0BEC5"),
            ("nord", "#81A1C1"),
            ("amber", "#E0A040"),
        ];
        for (name, color) in expected {
            assert_eq!(get_theme(name).color_rest, color);
        }
    }

    #[test]
    fn shadow_alphas() {
        let got: Vec<_> = THEMES.iter().map(|t| t.shadow_alpha).collect();
        assert_eq!(got, [70, 70, 90, 0, 70, 60]);
    }

    #[test]
    fn all_theme_colors_parse() {
        for t in THEMES.iter() {
            for c in [
                t.color_bg,
                t.color_clock,
                t.color_timer_run,
                t.color_timer_pause,
                t.color_warn,
                t.color_end,
                t.color_rest,
            ] {
                assert!(parse_hex(c).is_some(), "{} {c}", t.name);
            }
        }
    }

    #[test]
    fn parse_hex_cases() {
        let c = parse_hex("#00ff88").unwrap();
        assert_eq!((c.0, c.1, c.2), (0, 255, 136));
        let c = parse_hex("#4FA8FF").unwrap();
        assert_eq!((c.0, c.1, c.2), (0x4F, 0xA8, 0xFF));
        assert!(parse_hex("00FF88").is_none());
        assert!(parse_hex("#00FF8").is_none());
        assert!(parse_hex("#GG0000").is_none());
        assert!(parse_hex("#+1+2+3").is_none());
        assert!(parse_hex("").is_none());
    }
}
