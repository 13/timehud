//! Context menu model (shared by the overlay's right-click and the tray).

/// What a menu entry does; `App::perform` executes it.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    OpenSettings,
    ApplyStopwatch,
    /// Index into the raw `config.presets` list.
    ApplyPreset(usize),
    SavePreset,
    ManagePresets,
    SetTheme(&'static str),
    ToggleClickThrough,
    SetOpacity(f64),
    SetPosition(&'static str),
    ToggleVisibility,
    Quit,
}

use crate::config::{Config, Mode, Preset, valid_presets};
use crate::format::preset_menu_label;
use crate::layout::{POSITIONS, Rect};
use crate::text::FontSet;
use crate::themes::{THEMES, Theme};

/// One menu entry; submenus own their children.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Action {
        label: String,
        action: MenuAction,
        checked: Option<bool>,
        enabled: bool,
    },
    Submenu {
        label: String,
        items: Vec<MenuItem>,
    },
    Separator,
}

fn action(label: impl Into<String>, action: MenuAction) -> MenuItem {
    MenuItem::Action {
        label: label.into(),
        action,
        checked: None,
        enabled: true,
    }
}

fn check(label: impl Into<String>, action: MenuAction, on: bool) -> MenuItem {
    MenuItem::Action {
        label: label.into(),
        action,
        checked: Some(on),
        enabled: true,
    }
}

fn submenu(label: &str, items: Vec<MenuItem>) -> MenuItem {
    MenuItem::Submenu {
        label: label.into(),
        items,
    }
}

/// "top-left" -> "Top Left" (Python `str.title`).
fn position_label(p: &str) -> String {
    p.split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn presets_menu(cfg: &Config) -> MenuItem {
    let mut items = vec![check(
        "Stopwatch",
        MenuAction::ApplyStopwatch,
        cfg.timer_mode == Mode::Stopwatch,
    )];
    let presets = valid_presets(&cfg.presets);
    if !presets.is_empty() {
        items.push(MenuItem::Separator);
        for (p, _, idx) in &presets {
            let mode = match p {
                Preset::Interval { .. } => Mode::Interval,
                Preset::Stopwatch { .. } => Mode::Stopwatch,
                Preset::Countdown { .. } => Mode::Countdown,
            };
            let on = cfg.timer_mode == mode && cfg.active_preset == p.name();
            items.push(check(
                preset_menu_label(p),
                MenuAction::ApplyPreset(*idx),
                on,
            ));
        }
    }
    items.push(MenuItem::Separator);
    items.push(MenuItem::Action {
        label: "Save current as preset…".into(),
        action: MenuAction::SavePreset,
        checked: None,
        enabled: cfg.timer_mode != Mode::Stopwatch || cfg.stopwatch_work > 0,
    });
    items.push(action("Manage presets…", MenuAction::ManagePresets));
    submenu("Presets", items)
}

fn opacity_menu(cfg: &Config) -> MenuItem {
    let current = ((cfg.opacity * 100.0).round() as i64).clamp(0, 100);
    let mut items: Vec<MenuItem> = [30, 50, 70, 85, 95, 100]
        .into_iter()
        .map(|pct| {
            check(
                format!("{pct}%"),
                MenuAction::SetOpacity(pct as f64 / 100.0),
                pct == current,
            )
        })
        .collect();
    if ![30, 50, 70, 85, 95, 100].contains(&current) {
        items.push(check(
            format!("Current: {current}%"),
            MenuAction::SetOpacity(current as f64 / 100.0),
            true,
        ));
    }
    submenu("Opacity", items)
}

/// Port of `menus.populate_context_menu`.
pub fn build_menu(cfg: &Config, include_window_actions: bool) -> Vec<MenuItem> {
    let mut m = vec![
        action("Settings…", MenuAction::OpenSettings),
        MenuItem::Separator,
        presets_menu(cfg),
        submenu(
            "Theme",
            THEMES
                .iter()
                .map(|t| check(t.label, MenuAction::SetTheme(t.name), t.name == cfg.theme))
                .collect(),
        ),
        MenuItem::Separator,
    ];
    if include_window_actions {
        let label = if cfg.click_through {
            "Click-Through: ON"
        } else {
            "Click-Through: OFF"
        };
        m.push(action(label, MenuAction::ToggleClickThrough));
    }
    m.push(opacity_menu(cfg));
    m.push(submenu(
        "Position",
        POSITIONS
            .iter()
            .map(|p| {
                check(
                    position_label(p),
                    MenuAction::SetPosition(p),
                    *p == cfg.position,
                )
            })
            .collect(),
    ));
    if include_window_actions {
        m.push(MenuItem::Separator);
        m.push(action("Show/Hide Overlay", MenuAction::ToggleVisibility));
    }
    m.push(action("Quit", MenuAction::Quit));
    m
}

// ---- rendering ----

const FONT_PX: f32 = 13.0;
const BORDER: f32 = 1.0;
const RADIUS: f32 = 6.0;
const PANEL_PAD_Y: f32 = 4.0;
const ITEM_PAD_X: f32 = 22.0;
const ITEM_PAD_Y: f32 = 6.0;
const SEP_MARGIN_Y: f32 = 3.0;
const SEP_MARGIN_X: f32 = 6.0;
const CHEVRON_GAP: f32 = 12.0;
const TIP_PAD_X: f32 = 10.0;

pub struct MenuLayout {
    pub w: i32,
    pub h: i32,
    /// Row rectangle and item index per row (`None` for a separator).
    pub rows: Vec<(Rect, Option<usize>)>,
}

fn sc(v: f32, scale: f32) -> i32 {
    (v * scale).round() as i32
}

fn font_px(scale: f32) -> f32 {
    (FONT_PX * scale).round().max(1.0)
}

fn label_of(item: &MenuItem) -> Option<&str> {
    match item {
        MenuItem::Action { label, .. } | MenuItem::Submenu { label, .. } => Some(label),
        MenuItem::Separator => None,
    }
}

pub fn layout_menu(items: &[MenuItem], fonts: &mut FontSet, scale: f32) -> MenuLayout {
    let px = font_px(scale);
    let border = sc(BORDER, scale).max(1);
    let text_w = items
        .iter()
        .filter_map(label_of)
        .map(|l| fonts.measure(l, px, false))
        .fold(0.0_f32, f32::max);
    let chevron = if items.iter().any(|i| matches!(i, MenuItem::Submenu { .. })) {
        fonts.measure("›", px, false) + CHEVRON_GAP * scale
    } else {
        0.0
    };
    let w = (text_w + chevron).ceil() as i32 + 2 * sc(ITEM_PAD_X, scale) + 2 * border;
    let row_h = (px * 1.2).round() as i32 + 2 * sc(ITEM_PAD_Y, scale);
    let sep_h = border + 2 * sc(SEP_MARGIN_Y, scale);
    let mut y = border + sc(PANEL_PAD_Y, scale);
    let mut rows = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let (h, idx) = match item {
            MenuItem::Separator => (sep_h, None),
            _ => (row_h, Some(i)),
        };
        rows.push((
            Rect {
                x: border,
                y,
                w: w - 2 * border,
                h,
            },
            idx,
        ));
        y += h;
    }
    MenuLayout {
        w,
        h: y + sc(PANEL_PAD_Y, scale) + border,
        rows,
    }
}

fn rgba(hex: u32, a: u8) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, a)
}

fn paint(c: tiny_skia::Color) -> tiny_skia::Paint<'static> {
    let mut p = tiny_skia::Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    const K: f32 = 0.552_284_8;
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = r * K;
    let mut b = tiny_skia::PathBuilder::new();
    b.move_to(x + r, y);
    b.line_to(x + w - r, y);
    b.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    b.line_to(x + w, y + h - r);
    b.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    b.line_to(x + r, y + h);
    b.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    b.line_to(x, y + r);
    b.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    b.close();
    b.finish()
}

/// Size of a one-line tooltip in the menu's look.
pub fn tooltip_size(text: &str, fonts: &mut FontSet, scale: f32) -> (i32, i32) {
    let px = font_px(scale);
    let border = sc(BORDER, scale).max(1);
    let w = fonts.measure(text, px, false).ceil() as i32 + 2 * sc(TIP_PAD_X, scale) + 2 * border;
    let h = (px * 1.2).round() as i32 + 2 * sc(ITEM_PAD_Y, scale) + 2 * border;
    (w, h)
}

/// A one-line tooltip: the menu's panel, border and text colours.
pub fn render_tooltip(text: &str, fonts: &mut FontSet, scale: f32) -> tiny_skia::Pixmap {
    let (w, h) = tooltip_size(text, fonts, scale);
    let mut pm = tiny_skia::Pixmap::new(w.max(1) as u32, h.max(1) as u32).expect("tooltip pixmap");
    let id = tiny_skia::Transform::identity();
    let border = sc(BORDER, scale).max(1) as f32;
    let (wf, hf) = (w as f32, h as f32);
    if let Some(p) = rounded_rect(0.0, 0.0, wf, hf, RADIUS * scale) {
        let fill = paint(rgba(0x1A1A1A, 245));
        pm.fill_path(&p, &fill, tiny_skia::FillRule::Winding, id, None);
    }
    if let Some(p) = rounded_rect(
        border / 2.0,
        border / 2.0,
        wf - border,
        hf - border,
        RADIUS * scale - border / 2.0,
    ) {
        let stroke = tiny_skia::Stroke {
            width: border,
            ..Default::default()
        };
        pm.stroke_path(&p, &paint(rgba(0x333333, 255)), &stroke, id, None);
    }
    let px = font_px(scale);
    let baseline = (hf + fonts.ink_height(px)) / 2.0;
    let x = border + sc(TIP_PAD_X, scale) as f32;
    fonts.draw(
        &mut pm,
        text,
        x,
        baseline,
        px,
        rgba(0xCCCCCC, 255),
        false,
        0.0,
    );
    pm
}

pub fn render_menu(
    items: &[MenuItem],
    layout: &MenuLayout,
    hover: Option<usize>,
    _theme: &Theme,
    fonts: &mut FontSet,
    scale: f32,
) -> tiny_skia::Pixmap {
    let mut pm = tiny_skia::Pixmap::new(layout.w.max(1) as u32, layout.h.max(1) as u32)
        .expect("menu pixmap");
    let id = tiny_skia::Transform::identity();
    let border = sc(BORDER, scale).max(1) as f32;
    let (wf, hf) = (layout.w as f32, layout.h as f32);
    if let Some(p) = rounded_rect(0.0, 0.0, wf, hf, RADIUS * scale) {
        pm.fill_path(
            &p,
            &paint(rgba(0x1A1A1A, 245)),
            tiny_skia::FillRule::Winding,
            id,
            None,
        );
    }
    if let Some(p) = rounded_rect(
        border / 2.0,
        border / 2.0,
        wf - border,
        hf - border,
        RADIUS * scale - border / 2.0,
    ) {
        let stroke = tiny_skia::Stroke {
            width: border,
            ..Default::default()
        };
        pm.stroke_path(&p, &paint(rgba(0x333333, 255)), &stroke, id, None);
    }

    let px = font_px(scale);
    let ink = fonts.ink_height(px);
    let pad_x = ITEM_PAD_X * scale;
    for (rect, idx) in &layout.rows {
        let (x, y, w, h) = (rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32);
        let Some(i) = *idx else {
            let m = SEP_MARGIN_X * scale;
            let sy = y + (h - border) / 2.0;
            if let Some(r) = tiny_skia::Rect::from_xywh(x + m, sy, w - 2.0 * m, border) {
                pm.fill_rect(r, &paint(rgba(0x333333, 255)), id, None);
            }
            continue;
        };
        let item = &items[i];
        let enabled = !matches!(item, MenuItem::Action { enabled: false, .. });
        let hovered = hover == Some(i) && enabled;
        if hovered && let Some(r) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            pm.fill_rect(r, &paint(rgba(0x2E2E2E, 255)), id, None);
        }
        let fg = if hovered { 0xFFFFFF } else { 0xCCCCCC };
        let a = if enabled { 255 } else { 102 };
        let color = rgba(fg, a);
        let label = label_of(item).unwrap_or_default();
        let baseline = y + (h + ink) / 2.0;
        fonts.draw(&mut pm, label, x + pad_x, baseline, px, color, false, 0.0);
        match item {
            MenuItem::Action {
                checked: Some(true),
                ..
            } => {
                // Small tick centred in the left padding.
                let (cx, cy) = (x + pad_x / 2.0, y + h / 2.0);
                let s = px * 0.3;
                let mut b = tiny_skia::PathBuilder::new();
                b.move_to(cx - s, cy);
                b.line_to(cx - s * 0.3, cy + s * 0.75);
                b.line_to(cx + s, cy - s * 0.7);
                if let Some(p) = b.finish() {
                    let stroke = tiny_skia::Stroke {
                        width: (1.5 * scale).max(1.0),
                        line_cap: tiny_skia::LineCap::Round,
                        line_join: tiny_skia::LineJoin::Round,
                        ..Default::default()
                    };
                    pm.stroke_path(&p, &paint(color), &stroke, id, None);
                }
            }
            MenuItem::Submenu { .. } => {
                let cw = fonts.measure("›", px, false);
                fonts.draw(
                    &mut pm,
                    "›",
                    x + w - pad_x / 2.0 - cw,
                    baseline,
                    px,
                    color,
                    false,
                    0.0,
                );
            }
            _ => {}
        }
    }
    pm
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| label_of(i).unwrap_or("─")).collect()
    }

    fn sub<'a>(m: &'a [MenuItem], name: &str) -> &'a [MenuItem] {
        m.iter()
            .find_map(|i| match i {
                MenuItem::Submenu { label, items } if label == name => Some(items.as_slice()),
                _ => None,
            })
            .unwrap()
    }

    fn checked(i: &MenuItem) -> Option<bool> {
        match i {
            MenuItem::Action { checked, .. } => *checked,
            _ => None,
        }
    }

    #[test]
    fn menu_structure_with_window_actions() {
        let m = build_menu(&Config::default(), true);
        assert_eq!(
            labels(&m),
            [
                "Settings…",
                "─",
                "Presets",
                "Theme",
                "─",
                "Click-Through: OFF",
                "Opacity",
                "Position",
                "─",
                "Show/Hide Overlay",
                "Quit"
            ]
        );
    }

    #[test]
    fn menu_without_window_actions() {
        let m = build_menu(&Config::default(), false);
        let l = labels(&m);
        assert!(!l.iter().any(|s| s.starts_with("Click-Through")));
        assert!(!l.contains(&"Show/Hide Overlay"));
        assert_eq!(l.last(), Some(&"Quit"));
    }

    #[test]
    fn click_through_on_label() {
        let cfg = Config {
            click_through: true,
            ..Config::default()
        };
        assert!(labels(&build_menu(&cfg, true)).contains(&"Click-Through: ON"));
    }

    #[test]
    fn save_disabled_for_plain_stopwatch() {
        let cfg = Config {
            timer_mode: Mode::Stopwatch,
            stopwatch_work: 0,
            ..Config::default()
        };
        let m = build_menu(&cfg, true);
        let save = sub(&m, "Presets")
            .iter()
            .find(|i| label_of(i) == Some("Save current as preset…"))
            .unwrap();
        assert!(matches!(save, MenuItem::Action { enabled: false, .. }));
        let cfg = Config {
            timer_mode: Mode::Countdown,
            ..cfg
        };
        let m = build_menu(&cfg, true);
        let save = sub(&m, "Presets")
            .iter()
            .find(|i| label_of(i) == Some("Save current as preset…"))
            .unwrap();
        assert!(matches!(save, MenuItem::Action { enabled: true, .. }));
    }

    #[test]
    fn active_preset_checked() {
        let cfg = Config {
            timer_mode: Mode::Countdown,
            active_preset: "Plank".into(),
            presets: vec![
                json!("junk"),
                json!({"name": "Plank", "duration": 60}),
                json!({"name": "Other", "duration": 30}),
            ],
            ..Config::default()
        };
        let m = build_menu(&cfg, true);
        let p = sub(&m, "Presets");
        let plank = p
            .iter()
            .find(|i| label_of(i).is_some_and(|l| l.starts_with("Plank ")))
            .unwrap();
        assert_eq!(checked(plank), Some(true));
        assert!(matches!(
            plank,
            MenuItem::Action {
                action: MenuAction::ApplyPreset(1),
                ..
            }
        ));
        let other = p
            .iter()
            .find(|i| label_of(i).unwrap_or("").starts_with("Other"))
            .unwrap();
        assert_eq!(checked(other), Some(false));
        assert_eq!(checked(&p[0]), Some(false)); // Stopwatch
    }

    #[test]
    fn opacity_current_entry_when_unmatched() {
        let cfg = Config {
            opacity: 0.42,
            ..Config::default()
        };
        let m = build_menu(&cfg, true);
        let op = sub(&m, "Opacity");
        assert_eq!(op.len(), 7);
        let last = op.last().unwrap();
        assert_eq!(label_of(last), Some("Current: 42%"));
        assert_eq!(checked(last), Some(true));
        let m = build_menu(
            &Config {
                opacity: 0.85,
                ..Config::default()
            },
            true,
        );
        let op = sub(&m, "Opacity");
        assert_eq!(op.len(), 6);
        assert_eq!(op.iter().filter(|i| checked(i) == Some(true)).count(), 1);
    }

    #[test]
    fn theme_items_six_one_checked() {
        let m = build_menu(&Config::default(), true);
        let t = sub(&m, "Theme");
        assert_eq!(t.len(), 6);
        assert_eq!(t.iter().filter(|i| checked(i) == Some(true)).count(), 1);
    }

    #[test]
    fn position_labels_titlecased() {
        let m = build_menu(&Config::default(), true);
        assert_eq!(labels(sub(&m, "Position"))[0], "Top Left");
        assert_eq!(labels(sub(&m, "Position"))[5], "Bottom Center");
    }

    #[test]
    fn layout_rows_match_items() {
        let m = build_menu(&Config::default(), true);
        let mut fonts = FontSet::bundled();
        let l = layout_menu(&m, &mut fonts, 1.0);
        assert_eq!(l.rows.len(), m.len());
        let mut bottom = 0;
        for (i, (r, idx)) in l.rows.iter().enumerate() {
            assert_eq!(*idx, (m[i] != MenuItem::Separator).then_some(i));
            assert!(r.y >= bottom && r.x + r.w <= l.w);
            bottom = r.y + r.h;
        }
        assert!(bottom < l.h);
        let l2 = layout_menu(&m, &mut fonts, 2.0);
        assert!(l2.w > l.w && l2.h > l.h);
    }
}
