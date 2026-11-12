//! Debug: render every theme (and a few states) to target/snap-<name>.png.
use timehud::config::Config;
use timehud::render::{View, render};
use timehud::text::FontSet;
use timehud::themes::{THEMES, parse_hex};

fn main() {
    for t in THEMES.iter() {
        for (tag, style, progress, running, ctrl, scale) in [
            ("", "line", 0.6, true, 1.0, 1.0),
            ("-border", "border", 0.6, false, 1.0, 1.0),
            ("-fold", "line", -1.0, false, 0.0, 1.0),
            ("-2x", "line", 0.35, true, 1.0, 2.0),
        ] {
            let mut cfg = Config::default();
            timehud::themes::apply_theme(&mut cfg, t.name);
            cfg.progress_style = style.into();
            let family = if t.font_family.is_empty() {
                &cfg.font_family
            } else {
                t.font_family
            };
            let mut fonts = FontSet::load(family);
            let view = View {
                cfg: &cfg,
                theme: t,
                shadow: true,
                scale,
                clock: "14:32:07".into(),
                timer: "24:59".into(),
                timer_color: parse_hex(t.color_timer_run).unwrap(),
                timer_px_mul: 1.0,
                mode_text: "COUNTDOWN  25:00".into(),
                mode_color: parse_hex("#666666").unwrap(),
                show_controls_row: true,
                controls_pos: ctrl,
                running,
                mode_btn: "CD",
                hover_btn: if tag.is_empty() {
                    Some(timehud::render::Hit::BtnReset)
                } else {
                    None
                },
                progress,
                progress_color: parse_hex(t.color_timer_run).unwrap(),
                opacity: 1.0,
            };
            let (pm, _) = render(&view, &mut fonts).expect("pixmap");
            // Composite over mid-grey so translucency/shadow is visible.
            let mut bg = tiny_skia::Pixmap::new(pm.width(), pm.height()).unwrap();
            bg.fill(tiny_skia::Color::from_rgba8(90, 100, 120, 255));
            bg.draw_pixmap(
                0,
                0,
                pm.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
            bg.save_png(format!("target/snap-{}{tag}.png", t.name))
                .unwrap();
        }
    }

    snap_menu();
}

fn snap_menu() {
    use timehud::menu::{MenuAction, MenuItem, layout_menu, render_menu};
    let it = |label: &str, checked, enabled| MenuItem::Action {
        label: label.into(),
        action: MenuAction::Quit,
        checked,
        enabled,
    };
    let items = vec![
        it("Settings…", None, true),
        MenuItem::Separator,
        MenuItem::Submenu {
            label: "Presets".into(),
            items: vec![],
        },
        it("Opacity 70%", Some(true), true),
        it("Save current as preset…", None, false),
        it("Click-Through: OFF", None, true),
        MenuItem::Separator,
        it("Quit", None, true),
    ];
    let mut fonts = FontSet::bundled();
    for scale in [1.0, 2.0] {
        let l = layout_menu(&items, &mut fonts, scale);
        let pm = render_menu(&items, &l, Some(5), &mut fonts, scale);
        let mut bg = tiny_skia::Pixmap::new(pm.width() + 20, pm.height() + 20).unwrap();
        bg.fill(tiny_skia::Color::from_rgba8(90, 100, 120, 255));
        bg.draw_pixmap(
            10,
            10,
            pm.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
        let name = if scale == 1.0 { "menu" } else { "menu-2x" };
        bg.save_png(format!("target/snap-{name}.png")).unwrap();
    }
}
