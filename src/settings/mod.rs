//! Settings and save-preset windows: separate processes that talk to the HUD
//! over IPC. Only this module touches eframe/egui, so the HUD never loads GL.

mod presets;
mod save_preset;

pub use save_preset::run_save_preset;

use crate::config::{Config, Mode, config_path};
use crate::hotkey::{egui_to_combo, format_combo};
use crate::ipc::{Event, Request, socket_path};
use crate::layout::POSITIONS;
use crate::themes::{THEMES, apply_theme, parse_hex};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Key, Layout, Margin, RichText,
    Sense, Stroke, Ui, Vec2, ViewportCommand,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

// ── Look ────────────────────────────────────────────────────────────────────

pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x00, 0xFF, 0x88);
const ACCENT_DARK: Color32 = Color32::from_rgb(0x00, 0xAA, 0x55);
const BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
const SIDEBAR: Color32 = Color32::from_rgb(0x16, 0x16, 0x18);
const CARD: Color32 = Color32::from_rgb(0x24, 0x24, 0x27);
const FIELD: Color32 = Color32::from_rgb(0x2C, 0x2C, 0x2E);
const BORDER: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3C);
const TEXT: Color32 = Color32::from_rgb(0xE5, 0xE5, 0xEA);
const WEAK: Color32 = Color32::from_rgb(0x8E, 0x8E, 0x93);
const ERROR: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x6B);
const WARN: Color32 = Color32::from_rgb(0xFF, 0xB8, 0x4D);
const ROUND: u8 = 8;

const REPO_URL: &str = "https://github.com/ben/timehud";
const HOTKEY_WARNING: &str = "Grabbed combos stop reaching other apps (e.g. Ctrl+Shift+R in browsers). Tip: bind `timehud toggle` in your window manager instead.";

/// Dark visuals with the classic accent; shared by both windows.
pub(crate) fn apply_style(ctx: &egui::Context) {
    // Ubuntu-Light lacks arrows (→ ↑ ≡); fall back to the bundled Hack.
    let mut fonts = egui::FontDefinitions::default();
    if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        prop.push("Hack".into());
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = BG;
        v.extreme_bg_color = FIELD;
        v.faint_bg_color = CARD;
        v.hyperlink_color = ACCENT;
        v.error_fg_color = ERROR;
        v.warn_fg_color = WARN;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.slider_trailing_fill = true;
        v.window_corner_radius = CornerRadius::same(ROUND);
        v.menu_corner_radius = CornerRadius::same(ROUND);
        v.window_stroke = Stroke::new(1.0, BORDER);
        let r = CornerRadius::same(ROUND);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CARD;
        w.noninteractive.weak_bg_fill = CARD;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;
        for (wv, fill, stroke) in [
            (&mut w.inactive, FIELD, BORDER),
            (&mut w.hovered, Color32::from_rgb(0x3A, 0x3A, 0x3C), WEAK),
            (&mut w.active, Color32::from_rgb(0x48, 0x48, 0x4A), ACCENT),
            (&mut w.open, Color32::from_rgb(0x3A, 0x3A, 0x3C), BORDER),
        ] {
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.bg_stroke = Stroke::new(1.0, stroke);
            wv.corner_radius = r;
            wv.expansion = 0.0;
        }
        w.inactive.fg_stroke = Stroke::new(2.0, TEXT);
        w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
        w.active.fg_stroke = Stroke::new(1.5, ACCENT);
        w.open.fg_stroke = Stroke::new(1.0, TEXT);
        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(8.0, 8.0);
        s.button_padding = Vec2::new(12.0, 5.0);
        s.interact_size = Vec2::new(40.0, 26.0);
        s.combo_width = 190.0;
        s.slider_width = 160.0;
        s.text_edit_width = 190.0;
        s.icon_width = 16.0;
        s.icon_width_inner = 10.0;
        s.icon_spacing = 8.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    });
}

/// A titled, rounded group of controls.
pub(crate) fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, Color32::from_rgb(0x30, 0x30, 0x33)))
        .corner_radius(CornerRadius::same(ROUND))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(
                    RichText::new(title.to_uppercase())
                        .size(11.0)
                        .color(WEAK)
                        .strong(),
                );
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

/// Two-column label/control grid inside a card.
fn form<R>(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([18.0, 10.0])
        .min_col_width(120.0)
        .show(ui, add)
        .inner
}

fn note(ui: &mut Ui, text: impl Into<String>) -> egui::Response {
    ui.label(RichText::new(text.into()).size(12.0).color(WEAK))
}

/// Integer spin box with range and unit (QSpinBox stand-in).
pub(crate) fn spin(
    ui: &mut Ui,
    value: &mut i64,
    range: std::ops::RangeInclusive<i64>,
    suffix: &str,
) -> egui::Response {
    ui.add(
        egui::DragValue::new(value)
            .range(range)
            .suffix(suffix)
            .speed(0.25)
            .clamp_existing_to_range(false)
            .min_size(Vec2::new(90.0, 0.0)),
    )
}

/// `→ 1h 05m 00s` note under the countdown spin (`_update_cd_note`).
pub fn countdown_note(secs: i64) -> String {
    let (m, s) = (secs.div_euclid(60), secs.rem_euclid(60));
    let (h, m) = (m / 60, m % 60);
    let txt = if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    };
    format!("\u{2192} {txt}")
}

// ── Sections ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Display,
    Timer,
    Presets,
    Sound,
    Hotkeys,
    About,
}

const SECTIONS: [(Section, &str); 6] = [
    (Section::Display, "Display"),
    (Section::Timer, "Timer"),
    (Section::Presets, "Presets"),
    (Section::Sound, "Sound"),
    (Section::Hotkeys, "Hotkeys"),
    (Section::About, "About"),
];

impl Section {
    fn from_tab(tab: &str) -> Option<Section> {
        SECTIONS
            .iter()
            .find(|(_, label)| label.eq_ignore_ascii_case(tab.trim()))
            .map(|(s, _)| *s)
    }

    fn title(self) -> &'static str {
        SECTIONS
            .iter()
            .find(|(s, _)| *s == self)
            .map_or("", |e| e.1)
    }

    fn subtitle(self) -> &'static str {
        match self {
            Section::Display => "Theme, size, colors and what the overlay shows",
            Section::Timer => "Default mode and timer lengths",
            Section::Presets => "Quick timers for the right-click menu",
            Section::Sound => "Beeps and alert sound",
            Section::Hotkeys => "Global keyboard shortcuts",
            Section::About => "",
        }
    }
}

/// Small line-art icon for a sidebar entry, centred on `c` (16 px box).
fn paint_icon(p: &egui::Painter, s: Section, c: egui::Pos2, color: Color32) {
    use egui::{Pos2, Rect, StrokeKind, pos2, vec2};
    let st = Stroke::new(1.5, color);
    let line = |a: Pos2, b: Pos2| p.line_segment([a, b], st);
    match s {
        Section::Display => {
            let r = Rect::from_center_size(c - vec2(0.0, 2.0), vec2(16.0, 11.0));
            p.rect_stroke(r, CornerRadius::same(2), st, StrokeKind::Middle);
            line(pos2(c.x, r.bottom()), pos2(c.x, r.bottom() + 3.0));
            line(
                pos2(c.x - 4.0, r.bottom() + 3.5),
                pos2(c.x + 4.0, r.bottom() + 3.5),
            );
        }
        Section::Timer => paint_stopwatch(p, c + vec2(0.0, 1.0), 6.5, color),
        Section::Presets => {
            for dy in [-5.0, 0.0, 5.0] {
                p.circle_filled(pos2(c.x - 6.0, c.y + dy), 1.3, color);
                line(pos2(c.x - 3.0, c.y + dy), pos2(c.x + 7.0, c.y + dy));
            }
        }
        Section::Sound => {
            let pts = vec![
                pos2(c.x - 7.0, c.y - 2.5),
                pos2(c.x - 4.0, c.y - 2.5),
                pos2(c.x + 0.5, c.y - 6.5),
                pos2(c.x + 0.5, c.y + 6.5),
                pos2(c.x - 4.0, c.y + 2.5),
                pos2(c.x - 7.0, c.y + 2.5),
            ];
            p.add(egui::Shape::closed_line(pts, st));
            line(pos2(c.x + 3.5, c.y - 2.5), pos2(c.x + 4.5, c.y));
            line(pos2(c.x + 4.5, c.y), pos2(c.x + 3.5, c.y + 2.5));
            line(pos2(c.x + 6.0, c.y - 5.0), pos2(c.x + 7.5, c.y));
            line(pos2(c.x + 7.5, c.y), pos2(c.x + 6.0, c.y + 5.0));
        }
        Section::Hotkeys => {
            let r = Rect::from_center_size(c, vec2(17.0, 11.0));
            p.rect_stroke(r, CornerRadius::same(2), st, StrokeKind::Middle);
            for x in [-4.5, -1.5, 1.5, 4.5] {
                p.circle_filled(pos2(c.x + x, c.y - 2.0), 0.9, color);
            }
            line(pos2(c.x - 4.0, c.y + 2.5), pos2(c.x + 4.0, c.y + 2.5));
        }
        Section::About => {
            p.circle_stroke(c, 7.5, st);
            p.circle_filled(pos2(c.x, c.y - 3.5), 1.1, color);
            line(pos2(c.x, c.y - 1.0), pos2(c.x, c.y + 4.0));
        }
    }
}

/// Stopwatch outline: dial, crown and a hand at ten past.
fn paint_stopwatch(p: &egui::Painter, c: egui::Pos2, r: f32, color: Color32) {
    use egui::{Rect, pos2, vec2};
    let w = (r / 5.0).max(1.5);
    let st = Stroke::new(w, color);
    p.circle_stroke(c, r, st);
    let top = c.y - r;
    p.line_segment([pos2(c.x, top), pos2(c.x, top - r * 0.22)], st);
    p.rect_filled(
        Rect::from_center_size(pos2(c.x, top - r * 0.3), vec2(r * 0.55, w * 1.1)),
        CornerRadius::same(1),
        color,
    );
    p.line_segment([c, pos2(c.x + r * 0.45, c.y - r * 0.45)], st);
    p.circle_filled(c, w * 0.9, color);
}

// ── IPC link ────────────────────────────────────────────────────────────────

enum Incoming {
    Event(Event),
    Disconnected,
}

struct Link {
    stream: UnixStream,
    rx: Receiver<Incoming>,
}

impl Link {
    fn send(&mut self, req: &Request) -> bool {
        let mut line = match serde_json::to_vec(req) {
            Ok(l) => l,
            Err(_) => return false,
        };
        line.push(b'\n');
        self.stream.write_all(&line).is_ok()
    }
}

enum Startup {
    Ready {
        stream: UnixStream,
        reader: BufReader<UnixStream>,
        cfg: Box<Config>,
        tab: Option<String>,
    },
    NotRunning,
    /// Another settings window owns the session (the HUD focused it).
    AlreadyOpen,
}

/// Connect, say hello and wait (2 s) for the HUD's `Config`.
fn handshake() -> Startup {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return Startup::NotRunning;
    };
    let timeout = Some(Duration::from_secs(2));
    let ok = stream.set_read_timeout(timeout).is_ok()
        && stream.set_write_timeout(timeout).is_ok()
        && serde_json::to_vec(&Request::SettingsHello)
            .map(|mut l| {
                l.push(b'\n');
                l
            })
            .is_ok_and(|l| stream.write_all(&l).is_ok());
    if !ok {
        return Startup::NotRunning;
    }
    let Ok(read_half) = stream.try_clone() else {
        return Startup::NotRunning;
    };
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    if !matches!(reader.read_line(&mut line), Ok(n) if n > 0) {
        return Startup::NotRunning;
    }
    match serde_json::from_str::<Event>(line.trim_end()) {
        Ok(Event::Config { config, tab }) => {
            let _ = reader.get_ref().set_read_timeout(None);
            Startup::Ready {
                stream,
                reader,
                cfg: Box::new(Config::from_value(&config)),
                tab,
            }
        }
        Ok(Event::Error { .. }) => Startup::AlreadyOpen,
        _ => Startup::NotRunning,
    }
}

/// Reads HUD events until EOF; wakes the UI for each.
fn spawn_reader(mut reader: BufReader<UnixStream>, tx: Sender<Incoming>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            let msg = match reader.read_line(&mut line) {
                Ok(0) | Err(_) => Incoming::Disconnected,
                Ok(_) => match serde_json::from_str::<Event>(line.trim_end()) {
                    Ok(ev) => Incoming::Event(ev),
                    Err(_) => continue,
                },
            };
            let done = matches!(msg, Incoming::Disconnected);
            if tx.send(msg).is_err() {
                return;
            }
            ctx.request_repaint();
            if done {
                return;
            }
        }
    });
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// `fc-list` families, sorted, with the generic aliases first.
fn font_families() -> Vec<String> {
    let mut fams: Vec<String> = Command::new("fc-list")
        .args([":", "family"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    fams.sort_by_key(|s| s.to_lowercase());
    fams.dedup();
    let mut out: Vec<String> = ["Monospace", "Sans", "Serif"].map(String::from).into();
    out.extend(fams);
    out
}

fn in_path(prog: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(d.join(prog))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

/// zenity or kdialog, whichever exists.
fn file_picker() -> Option<&'static str> {
    ["zenity", "kdialog"].into_iter().find(|p| in_path(p))
}

fn run_picker(prog: &str, current: &str) -> Option<String> {
    let start = if current.is_empty() {
        std::env::var("HOME").unwrap_or_default() + "/"
    } else {
        current.to_string()
    };
    let mut cmd = Command::new(prog);
    if prog == "zenity" {
        cmd.args([
            "--file-selection",
            "--title=Select alert sound",
            "--file-filter=Audio files | *.wav *.mp3 *.ogg *.flac",
            "--file-filter=All files | *",
        ])
        .arg(format!("--filename={start}"));
    } else {
        cmd.args([
            "--getopenfilename",
            &start,
            "*.wav *.mp3 *.ogg *.flac|Audio files",
        ]);
    }
    let out = cmd.stderr(Stdio::null()).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !path.is_empty()).then_some(path)
}

fn hex_to_rgb(s: &str) -> [u8; 3] {
    parse_hex(s).map_or([255, 255, 255], |c| [c.0, c.1, c.2])
}

fn rgb_to_hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// A captured key press as a combo string, or None if the key is unusable.
fn combo_from_key(key: Key, physical: Option<Key>, m: egui::Modifiers) -> Option<String> {
    [Some(key), physical]
        .into_iter()
        .flatten()
        .find_map(|k| egui_to_combo(k.name(), m.ctrl, m.shift, m.alt, false))
        .map(|c| format_combo(&c))
}

const HOTKEYS: [(&str, &str); 3] = [
    ("Start / pause", "Ctrl+Shift+Space"),
    ("Reset", "Ctrl+Shift+R"),
    ("Show / hide", "Ctrl+Shift+H"),
];

fn hotkey_field(cfg: &mut Config, i: usize) -> &mut String {
    match i {
        0 => &mut cfg.hotkey_toggle,
        1 => &mut cfg.hotkey_reset,
        _ => &mut cfg.hotkey_visibility,
    }
}

// ── The settings app ────────────────────────────────────────────────────────

struct SettingsApp {
    link: Link,
    cfg: Config,
    /// What the HUD last received from us.
    sent: Config,
    section: Section,
    presets: presets::PresetsState,
    hotkey_error: Option<String>,
    capturing: Option<usize>,
    fonts: Arc<OnceLock<Vec<String>>>,
    font_filter: String,
    picker: Option<&'static str>,
    picked: Option<Receiver<Option<String>>>,
    closing: bool,
}

impl SettingsApp {
    fn new(
        ctx: &egui::Context,
        stream: UnixStream,
        reader: BufReader<UnixStream>,
        cfg: Config,
        section: Section,
    ) -> SettingsApp {
        let (tx, rx) = channel();
        spawn_reader(reader, tx, ctx.clone());
        let fonts = Arc::new(OnceLock::new());
        let f = fonts.clone();
        std::thread::spawn(move || {
            let _ = f.set(font_families());
        });
        SettingsApp {
            link: Link { stream, rx },
            presets: presets::PresetsState::new(&cfg),
            sent: cfg.clone(),
            cfg,
            section,
            hotkey_error: None,
            capturing: None,
            fonts,
            font_filter: String::new(),
            picker: file_picker(),
            picked: None,
            closing: false,
        }
    }

    fn poll_events(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.link.rx.try_recv() {
            match msg {
                Incoming::Event(Event::Focus { tab }) => {
                    if let Some(s) = tab.as_deref().and_then(Section::from_tab) {
                        self.section = s;
                    }
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                Incoming::Event(Event::Error { message }) => {
                    self.hotkey_error = Some(message);
                }
                Incoming::Event(_) => {}
                Incoming::Disconnected => {
                    // The HUD quit: nothing left to preview against.
                    self.closing = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        if let Some(rx) = &self.picked
            && let Ok(res) = rx.try_recv()
        {
            if let Some(path) = res {
                self.cfg.sound_file = path;
            }
            self.picked = None;
        }
    }

    /// Feeds the next key press to the hotkey being captured.
    fn capture_keys(&mut self, ctx: &egui::Context) {
        let Some(i) = self.capturing else {
            return;
        };
        let mut result: Option<Option<String>> = None;
        ctx.input_mut(|input| {
            let mods = input.modifiers;
            input.events.retain(|ev| {
                if result.is_some() {
                    return true;
                }
                match ev {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if *key == Key::Escape && !modifiers.any() {
                            result = Some(None);
                        } else if let Some(c) = combo_from_key(*key, *physical_key, *modifiers) {
                            result = Some(Some(c));
                        }
                        false
                    }
                    egui::Event::Key { .. } | egui::Event::Text(_) => false,
                    // Ctrl+C/X/V arrive as clipboard events, not keys.
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => {
                        let k = match ev {
                            egui::Event::Copy => Key::C,
                            egui::Event::Cut => Key::X,
                            _ => Key::V,
                        };
                        result = combo_from_key(k, None, mods).map(Some);
                        false
                    }
                    _ => true,
                }
            });
        });
        if let Some(r) = result {
            self.capturing = None;
            if let Some(combo) = r {
                let field = hotkey_field(&mut self.cfg, i);
                if *field != combo {
                    *field = combo;
                    // A new hotkey set: the HUD reports its errors afresh.
                    self.hotkey_error = None;
                }
            }
        }
    }

    fn finish(&mut self, ctx: &egui::Context, req: Request) {
        self.link.send(&req);
        self.closing = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    // ── Section views ──

    fn display(&mut self, ui: &mut Ui) {
        let cfg = &mut self.cfg;
        card(ui, "Appearance", |ui| {
            form(ui, "display-look", |ui| {
                ui.label("Theme");
                let current = THEMES
                    .iter()
                    .find(|t| t.name == cfg.theme)
                    .map_or(cfg.theme.clone(), |t| t.label.to_string());
                egui::ComboBox::from_id_salt("theme")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for t in &THEMES {
                            if ui.selectable_label(cfg.theme == t.name, t.label).clicked()
                                && cfg.theme != t.name
                            {
                                apply_theme(cfg, t.name);
                            }
                        }
                    });
                ui.end_row();

                ui.label("Font family");
                let fonts = self.fonts.get();
                let filter = &mut self.font_filter;
                egui::ComboBox::from_id_salt("font")
                    .selected_text(cfg.font_family.as_str())
                    .height(260.0)
                    .show_ui(ui, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(filter)
                                .hint_text("Filter…")
                                .desired_width(f32::INFINITY),
                        );
                        let Some(fonts) = fonts else {
                            ui.label("Loading fonts…");
                            return;
                        };
                        let f = filter.to_lowercase();
                        for fam in fonts.iter().filter(|x| x.to_lowercase().contains(&f)) {
                            if ui
                                .selectable_label(cfg.font_family == *fam, fam.as_str())
                                .clicked()
                            {
                                cfg.font_family = fam.clone();
                            }
                        }
                    });
                ui.end_row();

                ui.label("Font size");
                spin(ui, &mut cfg.font_size, 12..=80, " px");
                ui.end_row();

                ui.label("Opacity");
                let mut pct = (cfg.opacity * 100.0).round() as i64;
                if ui
                    .add(egui::Slider::new(&mut pct, 20..=100).suffix("%"))
                    .changed()
                {
                    cfg.opacity = pct as f64 / 100.0;
                }
                ui.end_row();
            });
        });
        ui.add_space(10.0);
        card(ui, "Colors", |ui| {
            egui::Grid::new("display-colors")
                .num_columns(2)
                .spacing([18.0, 10.0])
                .min_col_width(150.0)
                .show(ui, |ui| {
                    for row in [
                        [("Background", 0), ("Clock", 1)],
                        [("Timer running", 2), ("Timer paused", 3)],
                    ] {
                        for (label, which) in row {
                            let field = match which {
                                0 => &mut cfg.color_bg,
                                1 => &mut cfg.color_clock,
                                2 => &mut cfg.color_timer_run,
                                _ => &mut cfg.color_timer_pause,
                            };
                            ui.horizontal(|ui| {
                                let mut rgb = hex_to_rgb(field);
                                if egui::color_picker::color_edit_button_srgb(ui, &mut rgb)
                                    .on_hover_text(field.as_str())
                                    .changed()
                                {
                                    *field = rgb_to_hex(rgb);
                                }
                                ui.label(label);
                            });
                        }
                        ui.end_row();
                    }
                });
        });
        ui.add_space(10.0);
        card(ui, "Layout", |ui| {
            form(ui, "display-layout", |ui| {
                ui.label("Position");
                egui::ComboBox::from_id_salt("position")
                    .selected_text(cfg.position.as_str())
                    .show_ui(ui, |ui| {
                        for p in POSITIONS {
                            if ui.selectable_label(cfg.position == p, p).clicked() {
                                cfg.position = p.into();
                                cfg.custom_x = -1;
                                cfg.custom_y = -1;
                            }
                        }
                    });
                ui.end_row();

                ui.label("Padding");
                spin(ui, &mut cfg.padding, 2..=40, " px")
                    .on_hover_text("Space between the window border and the text");
                ui.end_row();

                ui.label("Top padding");
                ui.add(
                    egui::DragValue::new(&mut cfg.padding_top)
                        .range(-1..=40)
                        .speed(0.25)
                        .clamp_existing_to_range(false)
                        .min_size(Vec2::new(90.0, 0.0))
                        .custom_formatter(|v, _| {
                            if v < 0.0 {
                                "same as padding".into()
                            } else {
                                format!("{v} px")
                            }
                        })
                        .custom_parser(|s| {
                            let t = s.trim().trim_end_matches("px").trim();
                            if t.starts_with("same") {
                                Some(-1.0)
                            } else {
                                t.parse().ok()
                            }
                        }),
                )
                .on_hover_text("Top padding override; 'same as padding' follows the value above");
                ui.end_row();

                ui.label("Row spacing");
                spin(ui, &mut cfg.row_spacing, 0..=40, " px")
                    .on_hover_text("Uniform gap between clock, timer, mode label and controls");
                ui.end_row();
            });
        });
        ui.add_space(10.0);
        card(ui, "Show", |ui| {
            ui.checkbox(&mut cfg.show_clock, "System clock  (HH:MM:SS)");
            if ui
                .checkbox(&mut cfg.show_timer, "Timer  (stopwatch / countdown)")
                .changed()
                && !cfg.show_timer
            {
                cfg.show_controls = false;
            }
            ui.checkbox(&mut cfg.show_controls, "Timer controls (start/stop/reset)");
            ui.checkbox(&mut cfg.show_tray_icon, "System tray icon");
            ui.checkbox(
                &mut cfg.click_through,
                "Click-through (mouse passes to windows below)",
            );
        });
    }

    fn timer(&mut self, ui: &mut Ui) {
        let cfg = &mut self.cfg;
        let before = (
            cfg.countdown_duration,
            cfg.interval_work,
            cfg.interval_rest,
            cfg.interval_rounds,
        );
        card(ui, "Mode", |ui| {
            form(ui, "timer-mode", |ui| {
                ui.label("Default mode");
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(mode_name(cfg.timer_mode))
                    .show_ui(ui, |ui| {
                        for m in [Mode::Stopwatch, Mode::Countdown, Mode::Interval] {
                            ui.selectable_value(&mut cfg.timer_mode, m, mode_name(m));
                        }
                    });
                ui.end_row();

                ui.label("Progress bar");
                egui::ComboBox::from_id_salt("progress")
                    .selected_text(cfg.progress_style.as_str())
                    .show_ui(ui, |ui| {
                        for p in ["line", "border", "off"] {
                            if ui.selectable_label(cfg.progress_style == p, p).clicked() {
                                cfg.progress_style = p.into();
                            }
                        }
                    })
                    .response
                    .on_hover_text(
                        "line = bar under the timer, border = progress traced around the window",
                    );
                ui.end_row();
            });
        });
        ui.add_space(10.0);
        card(ui, "Countdown", |ui| {
            form(ui, "timer-cd", |ui| {
                ui.label("Duration");
                ui.horizontal(|ui| {
                    spin(ui, &mut cfg.countdown_duration, 5..=86400, " s")
                        .on_hover_text("Countdown duration in seconds (e.g. 300 = 5 min)");
                    note(ui, countdown_note(cfg.countdown_duration));
                });
                ui.end_row();
            });
            ui.checkbox(
                &mut cfg.auto_restart_countdown,
                "Restart countdown automatically",
            );
        });
        ui.add_space(10.0);
        card(ui, "Interval", |ui| {
            form(ui, "timer-iv", |ui| {
                ui.label("Work");
                spin(ui, &mut cfg.interval_work, 5..=3600, " s");
                ui.end_row();
                ui.label("Rest");
                spin(ui, &mut cfg.interval_rest, 0..=3600, " s");
                ui.end_row();
                ui.label("Rounds");
                spin(ui, &mut cfg.interval_rounds, 1..=99, "");
                ui.end_row();
            });
        });
        let after = (
            cfg.countdown_duration,
            cfg.interval_work,
            cfg.interval_rest,
            cfg.interval_rounds,
        );
        if after != before {
            // Edited by hand: the active preset no longer applies.
            cfg.active_preset.clear();
        }
    }

    fn sound(&mut self, ui: &mut Ui) {
        let cfg = &mut self.cfg;
        card(ui, "Alerts", |ui| {
            ui.checkbox(&mut cfg.sound_enabled, "Enable periodic sound alerts");
            ui.checkbox(
                &mut cfg.alert_last_5_seconds,
                "Alert last 5 seconds with short beeps (countdown)",
            );
            ui.add_space(4.0);
            form(ui, "sound-form", |ui| {
                ui.label("Alert every");
                ui.add(
                    egui::DragValue::new(&mut cfg.sound_interval)
                        .range(0..=3600)
                        .speed(0.25)
                        .clamp_existing_to_range(false)
                        .min_size(Vec2::new(90.0, 0.0))
                        .custom_formatter(|v, _| {
                            if v <= 0.0 {
                                "off".into()
                            } else {
                                format!("{v} s")
                            }
                        })
                        .custom_parser(|s| {
                            let t = s.trim().trim_end_matches('s').trim();
                            if t == "off" {
                                Some(0.0)
                            } else {
                                t.parse().ok()
                            }
                        }),
                )
                .on_hover_text("Play a beep every N seconds of active timer (off = never)");
                ui.end_row();

                ui.label("Double beep before");
                ui.horizontal(|ui| {
                    spin(ui, &mut cfg.sound_alert_before, 0..=3600, " s")
                        .on_hover_text("Play a double short beep N seconds before the main alert");
                    note(ui, "0 to disable");
                });
                ui.end_row();
            });
        });
        ui.add_space(10.0);
        card(ui, "Sound file", |ui| {
            let row = Vec2::new(ui.available_width(), 28.0);
            ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                if let Some(prog) = self.picker {
                    let busy = self.picked.is_some();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Browse…"))
                        .clicked()
                    {
                        let (tx, rx) = channel();
                        let current = cfg.sound_file.clone();
                        let ctx = ui.ctx().clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(run_picker(prog, &current));
                            ctx.request_repaint();
                        });
                        self.picked = Some(rx);
                    }
                }
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.sound_file)
                        .hint_text("Leave empty to use built-in beep")
                        .desired_width(ui.available_width()),
                );
            });
            note(ui, "WAV, MP3, OGG or FLAC.");
        });
    }

    fn hotkeys(&mut self, ui: &mut Ui) {
        let before = (
            self.cfg.hotkey_toggle.clone(),
            self.cfg.hotkey_reset.clone(),
            self.cfg.hotkey_visibility.clone(),
        );
        card(ui, "Shortcuts", |ui| {
            form(ui, "hotkeys", |ui| {
                for (i, (label, hint)) in HOTKEYS.iter().enumerate() {
                    ui.label(*label);
                    ui.horizontal(|ui| {
                        let capturing = self.capturing == Some(i);
                        let value = hotkey_field(&mut self.cfg, i).clone();
                        let text = if capturing {
                            RichText::new("Press keys…").color(ACCENT)
                        } else if value.is_empty() {
                            RichText::new(format!("e.g. {hint}")).color(WEAK).italics()
                        } else {
                            RichText::new(value.as_str()).strong()
                        };
                        let mut btn = egui::Button::new(text).min_size(Vec2::new(200.0, 28.0));
                        if capturing {
                            btn = btn.stroke(Stroke::new(1.5, ACCENT));
                        }
                        let r = ui.add(btn).on_hover_text(if capturing {
                            "Press the combo; Esc cancels"
                        } else {
                            "Click, then press the key combination"
                        });
                        if r.clicked() {
                            self.capturing = if capturing { None } else { Some(i) };
                            r.surrender_focus();
                        }
                        if !value.is_empty()
                            && ui
                                .add(egui::Button::new("×").min_size(Vec2::new(28.0, 28.0)))
                                .on_hover_text("Clear")
                                .clicked()
                        {
                            hotkey_field(&mut self.cfg, i).clear();
                            self.capturing = None;
                        }
                    });
                    ui.end_row();
                }
            });
            if let Some(err) = &self.hotkey_error {
                ui.add_space(4.0);
                Frame::new()
                    .fill(ERROR.gamma_multiply(0.12))
                    .stroke(Stroke::new(1.0, ERROR.gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(err.as_str()).color(ERROR));
                    });
            }
        });
        ui.add_space(10.0);
        Frame::new()
            .fill(WARN.gamma_multiply(0.08))
            .stroke(Stroke::new(1.0, WARN.gamma_multiply(0.35)))
            .corner_radius(CornerRadius::same(ROUND))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("Heads-up: ").strong().color(WARN));
                    let (head, rest) = HOTKEY_WARNING
                        .split_once('`')
                        .unwrap_or((HOTKEY_WARNING, ""));
                    let (code, tail) = rest.split_once('`').unwrap_or(("", rest));
                    ui.label(RichText::new(head).color(TEXT));
                    ui.label(RichText::new(code).code().color(ACCENT));
                    ui.label(RichText::new(tail).color(TEXT));
                });
            });
        let after = (
            self.cfg.hotkey_toggle.clone(),
            self.cfg.hotkey_reset.clone(),
            self.cfg.hotkey_visibility.clone(),
        );
        if after != before {
            self.hotkey_error = None;
        }
    }

    fn about(&mut self, ui: &mut Ui) {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
            paint_stopwatch(
                ui.painter(),
                rect.center() + Vec2::new(0.0, 4.0),
                26.0,
                ACCENT,
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let w = 190.0;
                ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                ui.label(
                    RichText::new("TimeHUD")
                        .size(24.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(24.0)
                        .strong()
                        .color(ACCENT),
                );
            });
            ui.add_space(6.0);
            ui.label(RichText::new("A lightweight, always-on-top system overlay.").color(WEAK));
            ui.label(RichText::new("Clock, Stopwatch & Countdown.").color(WEAK));
            ui.add_space(14.0);
            if ui.link(RichText::new(REPO_URL).color(ACCENT)).clicked() {
                let _ = Command::new("xdg-open")
                    .arg(REPO_URL)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        });
        ui.add_space(28.0);
        card(ui, "Config file", |ui| {
            ui.label(
                RichText::new(config_path().display().to_string())
                    .monospace()
                    .color(TEXT),
            );
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(
                RichText::new("TimeHUD")
                    .size(18.0)
                    .strong()
                    .color(Color32::WHITE),
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Settings").size(12.0).color(WEAK));
        });
        ui.add_space(14.0);
        for (s, label) in SECTIONS {
            let selected = self.section == s;
            let (rect, r) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
            let p = ui.painter();
            if selected {
                p.rect_filled(rect, CornerRadius::same(ROUND), ACCENT.gamma_multiply(0.14));
                p.rect_filled(
                    egui::Rect::from_min_size(
                        rect.min + Vec2::new(0.0, 8.0),
                        Vec2::new(3.0, rect.height() - 16.0),
                    ),
                    CornerRadius::same(2),
                    ACCENT,
                );
            } else if r.hovered() {
                p.rect_filled(rect, CornerRadius::same(ROUND), FIELD);
            }
            let color = if selected {
                ACCENT
            } else if r.hovered() {
                Color32::WHITE
            } else {
                TEXT
            };
            paint_icon(p, s, rect.left_center() + Vec2::new(22.0, 0.0), color);
            p.text(
                rect.left_center() + Vec2::new(42.0, 0.0),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(14.5),
                if selected { Color32::WHITE } else { color },
            );
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.section = s;
                self.capturing = None;
            }
            ui.add_space(2.0);
        }
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Stopwatch => "stopwatch",
        Mode::Countdown => "countdown",
        Mode::Interval => "interval",
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_events(&ctx);
        self.capture_keys(&ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            // Window × = Cancel.
            self.link.send(&Request::Cancel);
            self.closing = true;
        }

        egui::Panel::bottom("buttons")
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ok = egui::Button::new(RichText::new("OK").strong().color(Color32::WHITE))
                        .fill(ACCENT_DARK)
                        .min_size(Vec2::new(84.0, 30.0));
                    if ui.add(ok).clicked() {
                        let config = self.cfg.to_value();
                        self.finish(&ctx, Request::Commit { config });
                    }
                    if ui
                        .add(egui::Button::new("Apply").min_size(Vec2::new(84.0, 30.0)))
                        .clicked()
                    {
                        self.link.send(&Request::Commit {
                            config: self.cfg.to_value(),
                        });
                        self.sent = self.cfg.clone();
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(Vec2::new(84.0, 30.0)))
                        .clicked()
                    {
                        self.finish(&ctx, Request::Cancel);
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        note(ui, "Changes preview live on the HUD.");
                    });
                });
            });

        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(168.0)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(Margin::symmetric(10, 0)),
            )
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin {
                left: 22,
                right: 22,
                top: 18,
                bottom: 0,
            }))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(self.section.title())
                        .size(22.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                let sub = self.section.subtitle();
                if !sub.is_empty() {
                    note(ui, sub);
                }
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.section {
                            Section::Display => self.display(ui),
                            Section::Timer => self.timer(ui),
                            Section::Presets => {
                                presets::section(ui, &mut self.presets, &mut self.cfg)
                            }
                            Section::Sound => self.sound(ui),
                            Section::Hotkeys => self.hotkeys(ui),
                            Section::About => self.about(ui),
                        }
                        ui.add_space(16.0);
                    });
            });

        if !self.closing && self.cfg != self.sent {
            self.link.send(&Request::Preview {
                config: self.cfg.to_value(),
            });
            self.sent = self.cfg.clone();
        }
    }
}

// ── "TimeHUD is not running" ────────────────────────────────────────────────

struct NotRunning;

impl eframe::App for NotRunning {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(20)))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("TimeHUD is not running")
                        .size(17.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                note(ui, "Start timehud first, then open its settings.");
                ui.add_space(14.0);
                ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
                    ui.add_space(4.0);
                    if ui
                        .add(egui::Button::new("Close").min_size(Vec2::new(84.0, 30.0)))
                        .clicked()
                    {
                        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                    }
                });
            });
    }
}

pub(crate) fn native_options(
    title: &str,
    size: [f32; 2],
    resizable: bool,
) -> eframe::NativeOptions {
    let mut vp = egui::ViewportBuilder::default()
        .with_title(title)
        .with_app_id("timehud")
        .with_inner_size(size)
        .with_resizable(resizable);
    if resizable {
        vp = vp.with_min_inner_size([560.0, 420.0]);
    }
    eframe::NativeOptions {
        viewport: vp,
        ..Default::default()
    }
}

pub fn run_settings(tab: Option<String>) -> anyhow::Result<()> {
    let result = match handshake() {
        Startup::AlreadyOpen => return Ok(()),
        Startup::NotRunning => eframe::run_native(
            "TimeHUD Settings",
            native_options("TimeHUD Settings", [360.0, 140.0], false),
            Box::new(|cc| {
                apply_style(&cc.egui_ctx);
                Ok(Box::new(NotRunning))
            }),
        ),
        Startup::Ready {
            stream,
            reader,
            cfg,
            tab: hud_tab,
        } => {
            let section = hud_tab
                .or(tab)
                .as_deref()
                .and_then(Section::from_tab)
                .unwrap_or(Section::Display);
            eframe::run_native(
                "TimeHUD Settings",
                native_options("TimeHUD Settings", [640.0, 520.0], true),
                Box::new(move |cc| {
                    apply_style(&cc.egui_ctx);
                    Ok(Box::new(SettingsApp::new(
                        &cc.egui_ctx,
                        stream,
                        reader,
                        *cfg,
                        section,
                    )))
                }),
            )
        }
    };
    result.map_err(|e| anyhow::anyhow!("settings window: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_note_matches_python() {
        assert_eq!(countdown_note(45), "\u{2192} 45s");
        assert_eq!(countdown_note(300), "\u{2192} 5m 00s");
        assert_eq!(countdown_note(3725), "\u{2192} 1h 02m 05s");
    }

    #[test]
    fn tab_names_map_to_sections() {
        assert_eq!(Section::from_tab("presets"), Some(Section::Presets));
        assert_eq!(Section::from_tab("Hotkeys"), Some(Section::Hotkeys));
        assert_eq!(Section::from_tab("nope"), None);
    }

    #[test]
    fn color_hex_round_trip() {
        assert_eq!(rgb_to_hex(hex_to_rgb("#00ff88")), "#00FF88");
        assert_eq!(hex_to_rgb("junk"), [255, 255, 255]);
    }
}
