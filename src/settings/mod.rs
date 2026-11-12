//! Settings and save-preset windows: separate processes that talk to the HUD
//! over IPC. Only this module touches eframe/egui, so the HUD never loads GL.

mod presets;
mod save_preset;

pub use save_preset::run_save_preset;

use crate::app::custom_pos;
use crate::config::{
    COUNTDOWN_RANGE, Config, FONT_SIZE_RANGE, MARGIN_RANGE, Mode, OPACITY_RANGE, PADDING_RANGE,
    REST_RANGE, ROUNDS_RANGE, ROW_SPACING_RANGE, SOUND_SECS_RANGE, WORK_RANGE, config_path,
};
use crate::hotkey::{duplicate_combos, egui_to_combo, format_combo, is_modifier_key};
use crate::ipc::{Event, Request, socket_path};
use crate::layout::POSITIONS;
use crate::menu::position_label;
use crate::settings_session::adopt_hud_config;
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

const NOT_SENT: &str = "Cannot reach TimeHUD: the change was not applied.";
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

/// Integer spin box with an inclusive `(lo, hi)` range (one of the
/// `config` range constants) and unit (QSpinBox stand-in).
pub(crate) fn spin(
    ui: &mut Ui,
    value: &mut i64,
    range: (i64, i64),
    suffix: &str,
) -> egui::Response {
    ui.add(
        egui::DragValue::new(value)
            .range(range.0..=range.1)
            .suffix(suffix)
            .speed(0.25)
            .clamp_existing_to_range(false)
            .min_size(Vec2::new(90.0, 0.0)),
    )
}

/// [`spin`] whose lowest value is a special setting shown as `special`
/// (`-1` = "same as padding", `0` = "off"); other values show with `unit`.
fn sentinel_spin(
    ui: &mut Ui,
    value: &mut i64,
    range: (i64, i64),
    unit: &'static str,
    special: &'static str,
) -> egui::Response {
    let lo = range.0 as f64;
    ui.add(
        egui::DragValue::new(value)
            .range(range.0..=range.1)
            .speed(0.25)
            .clamp_existing_to_range(false)
            .min_size(Vec2::new(90.0, 0.0))
            .custom_formatter(move |v, _| {
                if v <= lo {
                    special.into()
                } else {
                    format!("{v} {unit}")
                }
            })
            .custom_parser(move |s| parse_sentinel(s, lo, unit, special)),
    )
}

/// Typed text of a [`sentinel_spin`]: a number (unit optional), or any
/// prefix of `special` (e.g. "same", "off") for the sentinel `lo`.
fn parse_sentinel(s: &str, lo: f64, unit: &str, special: &str) -> Option<f64> {
    let t = s.trim();
    let t = t.strip_suffix(unit).unwrap_or(t).trim();
    if let Ok(v) = t.parse() {
        return Some(v);
    }
    let special = special.to_lowercase();
    (!t.is_empty() && special.starts_with(&t.to_lowercase())).then_some(lo)
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
    let Ok(mut stream) = socket_path().and_then(UnixStream::connect) else {
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
        Ok(Event::Config { config, tab, .. }) => {
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

/// Why a key press during hotkey capture did not become the hotkey.
#[derive(Clone, Debug, PartialEq)]
enum CaptureNote {
    /// No Ctrl/Alt (and not a function key).
    NeedsModifier,
    /// No X keysym known for this key.
    Unsupported(String),
}

/// What a key press during capture does.
#[derive(Clone, Debug, PartialEq)]
enum Captured {
    /// A lone modifier: keep waiting for the rest of the combo.
    Pending,
    Rejected(CaptureNote),
    Combo(String),
}

fn capture_key(key: Key, physical: Option<Key>, m: egui::Modifiers) -> Captured {
    if is_modifier_key(key.name()) {
        Captured::Pending
    } else if !capture_allowed(key, m) {
        Captured::Rejected(CaptureNote::NeedsModifier)
    } else if let Some(c) = combo_from_key(key, physical, m) {
        Captured::Combo(c)
    } else {
        Captured::Rejected(CaptureNote::Unsupported(key.name().to_string()))
    }
}

/// Whether a captured key press may become a global hotkey: it needs Ctrl or
/// Alt (Super is not reported on Linux), except bare function keys. A plain
/// letter or Space would be grabbed globally and stop reaching every app.
fn capture_allowed(key: Key, m: egui::Modifiers) -> bool {
    let name = key.name();
    let is_fkey = name
        .strip_prefix('F')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    m.ctrl || m.alt || is_fkey
}

fn combo_from_key(key: Key, physical: Option<Key>, m: egui::Modifiers) -> Option<String> {
    [Some(key), physical]
        .into_iter()
        .flatten()
        .find_map(|k| egui_to_combo(k.name(), m.ctrl, m.shift, m.alt, false))
        .map(|c| format_combo(&c))
}

/// A config string field, for tables of editable fields.
type Field = fn(&mut Config) -> &mut String;

/// The hotkeys tab: label, example combo, config field.
const HOTKEYS: [(&str, &str, Field); 3] = [
    ("Start / pause", "Ctrl+Shift+Space", |c| {
        &mut c.hotkey_toggle
    }),
    ("Reset", "Ctrl+Shift+R", |c| &mut c.hotkey_reset),
    ("Show / hide", "Ctrl+Shift+H", |c| &mut c.hotkey_visibility),
];

fn hotkey_field(cfg: &mut Config, i: usize) -> &mut String {
    (HOTKEYS[i].2)(cfg)
}

/// The colors grid, two per row: label, config field.
const COLORS: [(&str, Field); 4] = [
    ("Background", |c| &mut c.color_bg),
    ("Clock", |c| &mut c.color_clock),
    ("Timer running", |c| &mut c.color_timer_run),
    ("Timer paused", |c| &mut c.color_timer_pause),
];

// ── The settings app ────────────────────────────────────────────────────────

struct SettingsApp {
    link: Link,
    cfg: Config,
    /// What the HUD last received from us.
    sent: Config,
    /// HUD values as of the start, kept current by HUD pushes for keys not
    /// edited here (`settings_session`).
    base: Config,
    /// Revision of the last HUD push received, echoed in Preview/Commit so
    /// the HUD can tell values sent before a push from edits.
    rev: u64,
    section: Section,
    presets: presets::PresetsState,
    /// Hotkey problems: the HUD's grab report, or local ones (see `hotkeys`).
    hotkey_errors: Vec<String>,
    /// The last other error the HUD reported, shown in the button bar.
    status: Option<String>,
    capturing: Option<usize>,
    /// Why the last key pressed while capturing was not taken.
    capture_note: Option<CaptureNote>,
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
            base: cfg.clone(),
            rev: 0,
            cfg,
            section,
            hotkey_errors: Vec::new(),
            status: None,
            capturing: None,
            capture_note: None,
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
                Incoming::Event(Event::HotkeyStatus { errors }) => {
                    self.hotkey_errors = errors;
                }
                Incoming::Event(Event::Error { message }) => {
                    self.status = Some(message);
                }
                Incoming::Event(Event::Config { config, rev, .. }) => {
                    self.adopt(&config);
                    self.rev = rev;
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

    /// The HUD changed its config (menu, wheel, drag, ...): take its values
    /// for every key not edited here, keeping the user's unsaved edits.
    fn adopt(&mut self, hud: &serde_json::Value) {
        let mut local = self.cfg.to_value();
        let mut base = self.base.to_value();
        let mut sent = self.sent.to_value();
        for key in adopt_hud_config(&mut local, &mut base, hud) {
            // Already the HUD's: no need to send it back.
            if let (Some(m), Some(v)) = (sent.as_object_mut(), local.get(&key)) {
                m.insert(key, v.clone());
            }
        }
        self.cfg = Config::from_value(&local);
        self.base = Config::from_value(&base);
        self.sent = Config::from_value(&sent);
    }

    /// Feeds the next key press to the hotkey being captured.
    fn capture_keys(&mut self, ctx: &egui::Context) {
        let Some(i) = self.capturing else {
            return;
        };
        let mut result: Option<Option<String>> = None;
        let mut note: Option<CaptureNote> = None;
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
                        } else {
                            match capture_key(*key, *physical_key, *modifiers) {
                                Captured::Pending => {}
                                // Keep capturing, say why.
                                Captured::Rejected(why) => note = Some(why),
                                Captured::Combo(c) => result = Some(Some(c)),
                            }
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
        if note.is_some() {
            self.capture_note = note;
        }
        if let Some(r) = result {
            self.capturing = None;
            self.capture_note = None;
            if let Some(combo) = r {
                let field = hotkey_field(&mut self.cfg, i);
                if *field != combo {
                    *field = combo;
                    // A new hotkey set: the HUD reports its errors afresh.
                    self.hotkey_errors.clear();
                }
            }
        }
    }

    /// Sends `req`; on failure shows why in the button bar and returns false.
    fn send(&mut self, req: &Request) -> bool {
        let ok = self.link.send(req);
        if !ok {
            self.status = Some(NOT_SENT.into());
        } else if self.status.as_deref() == Some(NOT_SENT) {
            self.status = None;
        }
        ok
    }

    /// OK / Cancel: send and close. A Commit that did not go out keeps the
    /// window open (with the error shown) so the edits are not lost.
    fn finish(&mut self, ctx: &egui::Context, req: Request) {
        if !self.send(&req) && matches!(req, Request::Commit { .. }) {
            return;
        }
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
                spin(ui, &mut cfg.font_size, FONT_SIZE_RANGE, " px");
                ui.end_row();

                ui.label("Opacity");
                let mut pct = (cfg.opacity * 100.0).round() as i64;
                let (lo, hi) = OPACITY_RANGE;
                let pct_range = (lo * 100.0).round() as i64..=(hi * 100.0).round() as i64;
                if ui
                    .add(egui::Slider::new(&mut pct, pct_range).suffix("%"))
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
                    for row in COLORS.chunks(2) {
                        for &(label, field) in row {
                            let field = field(cfg);
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
                let current = if custom_pos(cfg).is_some() {
                    "Custom (dragged)".to_string()
                } else {
                    position_label(&cfg.position)
                };
                egui::ComboBox::from_id_salt("position")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for p in POSITIONS {
                            if ui
                                .selectable_label(cfg.position == p, position_label(p))
                                .clicked()
                            {
                                cfg.position = p.into();
                                cfg.custom_x = -1;
                                cfg.custom_y = -1;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Screen corner or edge; dragging the HUD overrides it");
                ui.end_row();

                ui.label("Screen margin");
                let dragged = custom_pos(cfg).is_some();
                ui.add_enabled_ui(!dragged, |ui| {
                    spin(ui, &mut cfg.margin, MARGIN_RANGE, " px")
                        .on_hover_text("Distance from the screen edge")
                        .on_disabled_hover_text("Pick a position to use the margin again");
                });
                ui.end_row();

                ui.label("Padding");
                spin(ui, &mut cfg.padding, PADDING_RANGE, " px")
                    .on_hover_text("Space between the window border and the text");
                ui.end_row();

                ui.label("Row spacing");
                spin(ui, &mut cfg.row_spacing, ROW_SPACING_RANGE, " px")
                    .on_hover_text("Uniform gap between clock, timer, mode label and controls");
                ui.end_row();
            });
        });
        ui.add_space(10.0);
        card(ui, "Show", |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut cfg.show_clock, "Clock");
                ui.add_enabled_ui(cfg.show_clock, |ui| {
                    choice(ui, "clock-format", &mut cfg.clock_format, &CLOCK_FORMATS)
                        .on_hover_text("Clock format");
                });
            });
            if ui.checkbox(&mut cfg.show_timer, "Timer").changed() && !cfg.show_timer {
                cfg.show_controls = false;
            }
            ui.add_enabled_ui(cfg.show_timer, |ui| {
                ui.checkbox(&mut cfg.show_controls, "Start / pause / reset buttons");
            });
        });
        ui.add_space(10.0);
        card(ui, "Behavior", |ui| {
            ui.checkbox(&mut cfg.show_tray_icon, "Tray icon");
            ui.checkbox(&mut cfg.click_through, "Click-through")
                .on_hover_text("Mouse clicks pass through the HUD to the windows below");
            if lockout_risk(cfg) {
                warning(
                    ui,
                    "With click-through on and no tray icon, the HUD cannot be clicked: \
                     turn click-through off again via `timehud settings`.",
                );
            }
        });
    }

    fn timer(&mut self, ui: &mut Ui) {
        let cfg = &mut self.cfg;
        let before = (
            cfg.timer_mode,
            cfg.countdown_duration,
            cfg.interval_work,
            cfg.interval_rest,
            cfg.interval_rounds,
        );
        card(ui, "Mode", |ui| {
            form(ui, "timer-mode", |ui| {
                ui.label("Default mode");
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(cfg.timer_mode.label())
                    .show_ui(ui, |ui| {
                        for m in Mode::ALL {
                            ui.selectable_value(&mut cfg.timer_mode, m, m.label());
                        }
                    });
                ui.end_row();

                ui.label("Progress");
                choice(ui, "progress", &mut cfg.progress_style, &PROGRESS_STYLES)
                    .on_hover_text("How a countdown or interval shows the time left");
                ui.end_row();
            });
            ui.checkbox(
                &mut cfg.notify_on_finish,
                "Desktop notification when a countdown or interval session ends",
            )
            .on_hover_text("Sent with notify-send");
        });
        ui.add_space(10.0);
        card(ui, "Countdown", |ui| {
            form(ui, "timer-cd", |ui| {
                ui.label("Duration");
                ui.horizontal(|ui| {
                    spin(ui, &mut cfg.countdown_duration, COUNTDOWN_RANGE, " s")
                        .on_hover_text("Countdown duration in seconds (e.g. 300 = 5 min)");
                    note(ui, countdown_note(cfg.countdown_duration));
                });
                ui.end_row();
            });
            ui.checkbox(
                &mut cfg.auto_restart_countdown,
                "Start again automatically when it reaches zero",
            );
        });
        ui.add_space(10.0);
        card(ui, "Interval", |ui| {
            form(ui, "timer-iv", |ui| {
                ui.label("Work");
                spin(ui, &mut cfg.interval_work, WORK_RANGE, " s");
                ui.end_row();
                ui.label("Rest");
                spin(ui, &mut cfg.interval_rest, REST_RANGE, " s");
                ui.end_row();
                ui.label("Rounds");
                ui.horizontal(|ui| {
                    spin(ui, &mut cfg.interval_rounds, ROUNDS_RANGE, "");
                    let total = interval_session_secs(
                        cfg.interval_work,
                        cfg.interval_rest,
                        cfg.interval_rounds,
                    );
                    note(ui, format!("{} total", countdown_note(total)));
                });
                ui.end_row();
            });
        });
        let after = (
            cfg.timer_mode,
            cfg.countdown_duration,
            cfg.interval_work,
            cfg.interval_rest,
            cfg.interval_rounds,
        );
        if after != before {
            // Mode or durations edited by hand: the preset no longer applies.
            cfg.active_preset.clear();
        }
    }

    fn sound(&mut self, ui: &mut Ui) {
        let cfg = &mut self.cfg;
        card(ui, "", |ui| {
            ui.checkbox(
                &mut cfg.sound_enabled,
                RichText::new("Play sounds")
                    .size(15.0)
                    .color(Color32::WHITE),
            )
            .on_hover_text("Master switch: off mutes every beep");
        });
        ui.add_space(10.0);
        ui.add_enabled_ui(cfg.sound_enabled, |ui| {
            card(ui, "Beeps", |ui| {
                ui.checkbox(
                    &mut cfg.alert_last_5_seconds,
                    "Short beeps in the last 5 seconds of a countdown or phase",
                );
                ui.add_space(4.0);
                form(ui, "sound-form", |ui| {
                    ui.label("Beep every");
                    sentinel_spin(ui, &mut cfg.sound_interval, SOUND_SECS_RANGE, "s", "off")
                        .on_hover_text(
                            "A beep every N seconds of running stopwatch or countdown \
                             (not in interval or cycling timers)",
                        );
                    ui.end_row();

                    ui.label("Warning beep");
                    ui.add_enabled_ui(cfg.sound_interval > 0, |ui| {
                        sentinel_spin(
                            ui,
                            &mut cfg.sound_alert_before,
                            SOUND_SECS_RANGE,
                            "s before",
                            "off",
                        )
                        .on_hover_text("A double short beep N seconds before each periodic beep");
                    });
                    ui.end_row();
                });
            });
            ui.add_space(10.0);
            card(ui, "Custom sound", |ui| {
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
                            .hint_text("Empty = built-in beep")
                            .desired_width(ui.available_width()),
                    );
                });
                note(
                    ui,
                    "Plays instead of every built-in beep. WAV, MP3, OGG or FLAC.",
                );
            });
        });
    }

    /// The hotkeys' current combos, in [`HOTKEYS`] order.
    fn hotkey_values(&mut self) -> Vec<String> {
        (0..HOTKEYS.len())
            .map(|i| hotkey_field(&mut self.cfg, i).clone())
            .collect()
    }

    fn hotkeys(&mut self, ui: &mut Ui) {
        let before = self.hotkey_values();
        let dup = duplicate_combos(&before.iter().map(String::as_str).collect::<Vec<_>>());
        card(ui, "Shortcuts", |ui| {
            form(ui, "hotkeys", |ui| {
                for (i, (label, hint, _)) in HOTKEYS.iter().enumerate() {
                    ui.label(*label);
                    ui.horizontal(|ui| {
                        let capturing = self.capturing == Some(i);
                        let value = hotkey_field(&mut self.cfg, i).clone();
                        let text = if capturing && let Some(why) = &self.capture_note {
                            match why {
                                CaptureNote::NeedsModifier => {
                                    RichText::new("Add Ctrl or Alt…").color(WARN)
                                }
                                CaptureNote::Unsupported(k) => {
                                    RichText::new(format!("Unsupported key: {k}")).color(WARN)
                                }
                            }
                        } else if capturing {
                            RichText::new("Press keys…").color(ACCENT)
                        } else if value.is_empty() {
                            RichText::new(format!("e.g. {hint}")).color(WEAK).italics()
                        } else if dup.contains(&i) {
                            RichText::new(value.as_str()).strong().color(ERROR)
                        } else {
                            RichText::new(value.as_str()).strong()
                        };
                        let mut btn = egui::Button::new(text).min_size(Vec2::new(200.0, 28.0));
                        if capturing {
                            btn = btn.stroke(Stroke::new(1.5, ACCENT));
                        } else if dup.contains(&i) {
                            btn = btn.stroke(Stroke::new(1.0, ERROR.gamma_multiply(0.7)));
                        }
                        let r = ui.add(btn).on_hover_text(if capturing {
                            "Press a combo with Ctrl or Alt (or a bare F-key); Esc cancels"
                        } else {
                            "Click, then press the key combination"
                        });
                        if r.clicked() {
                            self.capturing = if capturing { None } else { Some(i) };
                            self.capture_note = None;
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
            if !dup.is_empty() {
                ui.add_space(4.0);
                let names: Vec<&str> = dup.iter().map(|&i| HOTKEYS[i].0).collect();
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{} share one combo; only one of them can work.",
                            names.join(" and ")
                        ))
                        .color(ERROR),
                    )
                    .wrap(),
                );
            }
            if !self.hotkey_errors.is_empty() {
                ui.add_space(4.0);
                Frame::new()
                    .fill(ERROR.gamma_multiply(0.12))
                    .stroke(Stroke::new(1.0, ERROR.gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for err in &self.hotkey_errors {
                            ui.label(RichText::new(err.as_str()).color(ERROR));
                        }
                    });
            }
        });
        ui.add_space(10.0);
        warning(ui, HOTKEY_WARNING);
        if self.hotkey_values() != before {
            self.hotkey_errors.clear();
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
            ui.label(RichText::new("Clock, stopwatch, countdown & interval timer.").color(WEAK));
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

/// A string setting's `(value, label)` pairs, and the value the HUD treats
/// an unknown one as.
struct Choices {
    items: &'static [(&'static str, &'static str)],
    unknown_as: &'static str,
}

/// `clock_format` (unknown renders like 24h, see `format::clock_text`).
const CLOCK_FORMATS: Choices = Choices {
    items: &[
        ("24h", "24-hour  13:05:09"),
        ("24h-short", "24-hour  13:05"),
        ("12h", "12-hour  01:05:09 PM"),
    ],
    unknown_as: "24h",
};

/// `progress_style` (unknown draws no progress, see `render::View`).
const PROGRESS_STYLES: Choices = Choices {
    items: &[
        ("line", "Bar under the timer"),
        ("border", "Around the window"),
        ("off", "Off"),
    ],
    unknown_as: "off",
};

/// Label for `value` in `table`; an unknown value shows what it acts as.
fn choice_label(table: &Choices, value: &str) -> &'static str {
    let find = |v: &str| table.items.iter().find(|(x, _)| *x == v).map(|e| e.1);
    find(value).or_else(|| find(table.unknown_as)).unwrap_or("")
}

/// Combo box over a [`Choices`] table, editing `value`.
fn choice(ui: &mut Ui, id: &str, value: &mut String, table: &Choices) -> egui::Response {
    egui::ComboBox::from_id_salt(id)
        .selected_text(choice_label(table, value))
        .show_ui(ui, |ui| {
            for (v, label) in table.items {
                if ui.selectable_label(value == v, *label).clicked() {
                    *value = (*v).into();
                }
            }
        })
        .response
}

/// Length of an interval session: `rounds` work phases with a rest between
/// each pair (the engine ends after the last work phase, no trailing rest).
pub(crate) fn interval_session_secs(work: i64, rest: i64, rounds: i64) -> i64 {
    let rounds = rounds.max(1);
    work.saturating_mul(rounds)
        .saturating_add(rest.saturating_mul(rounds - 1))
}

/// Click-through with no tray icon leaves no mouse path back to the HUD.
fn lockout_risk(cfg: &Config) -> bool {
    cfg.click_through && !cfg.show_tray_icon
}

/// An amber callout; text between backticks shows as code.
fn warning(ui: &mut Ui, text: &str) {
    Frame::new()
        .fill(WARN.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, WARN.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (i, part) in text.split('`').enumerate() {
                    let t = RichText::new(part);
                    ui.label(if i % 2 == 1 {
                        t.code().color(ACCENT)
                    } else {
                        t.color(TEXT)
                    });
                }
            });
        });
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_events(&ctx);
        self.capture_keys(&ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            // Window × = Cancel.
            // Closing anyway: a HUD we cannot reach has nothing to restore.
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
                        let rev = self.rev;
                        self.finish(&ctx, Request::Commit { config, rev });
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(Vec2::new(84.0, 30.0)))
                        .clicked()
                    {
                        self.finish(&ctx, Request::Cancel);
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        match &self.status {
                            Some(msg) => {
                                let r = ui.add(
                                    egui::Label::new(
                                        RichText::new(msg.as_str()).size(12.0).color(ERROR),
                                    )
                                    .truncate()
                                    .sense(Sense::click()),
                                );
                                if r.on_hover_text(format!("{msg}\n(click to dismiss)"))
                                    .clicked()
                                {
                                    self.status = None;
                                }
                            }
                            None => {
                                note(ui, "Changes preview live on the HUD.");
                            }
                        }
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
            let req = Request::Preview {
                config: self.cfg.to_value(),
                rev: self.rev,
            };
            // Unsent edits stay pending (`sent` unchanged) and go out with
            // the next successful message.
            if self.send(&req) {
                self.sent = self.cfg.clone();
            }
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
    fn capture_requires_ctrl_or_alt_except_function_keys() {
        use egui::Modifiers;
        let shift = Modifiers::SHIFT;
        assert!(!capture_allowed(Key::A, Modifiers::NONE));
        assert!(!capture_allowed(Key::Space, Modifiers::NONE));
        assert!(!capture_allowed(Key::A, shift));
        assert!(capture_allowed(Key::A, Modifiers::CTRL));
        assert!(capture_allowed(Key::Space, Modifiers::ALT | shift));
        assert!(capture_allowed(Key::F5, Modifiers::NONE));
        assert!(capture_allowed(Key::F12, shift));
    }

    #[test]
    fn choice_labels() {
        let clock = |v| choice_label(&CLOCK_FORMATS, v);
        assert_eq!(clock("12h"), "12-hour  01:05:09 PM");
        assert_eq!(clock("24h-short"), "24-hour  13:05");
        // Unknown values show what the HUD does with them.
        assert_eq!(clock("bogus"), clock("24h"));
        let progress = |v| choice_label(&PROGRESS_STYLES, v);
        assert_eq!(progress("border"), "Around the window");
        assert_eq!(progress("bogus"), "Off");
    }

    #[test]
    fn choice_tables_cover_their_fallbacks() {
        for t in [&CLOCK_FORMATS, &PROGRESS_STYLES] {
            assert!(t.items.iter().any(|(v, _)| *v == t.unknown_as));
        }
        // The progress styles the HUD draws are all offered.
        for v in ["line", "border", "off"] {
            assert!(PROGRESS_STYLES.items.iter().any(|(x, _)| *x == v));
        }
        assert_eq!(
            choice_label(&CLOCK_FORMATS, Config::default().clock_format.as_str()),
            "24-hour  13:05:09"
        );
    }

    #[test]
    fn interval_total_has_no_trailing_rest() {
        // 3 rounds: W R W R W (engine::tests, "no trailing rest").
        assert_eq!(interval_session_secs(40, 20, 3), 3 * 40 + 2 * 20);
        assert_eq!(interval_session_secs(40, 20, 1), 40);
        assert_eq!(interval_session_secs(i64::MAX, 1, 2), i64::MAX);
    }

    #[test]
    fn lockout_needs_click_through_without_tray() {
        let cfg = |click_through, show_tray_icon| Config {
            click_through,
            show_tray_icon,
            ..Config::default()
        };
        assert!(lockout_risk(&cfg(true, false)));
        assert!(!lockout_risk(&cfg(true, true)));
        assert!(!lockout_risk(&cfg(false, false)));
    }

    #[test]
    fn capture_classifies_key_presses() {
        use egui::Modifiers;
        let ctrl = Modifiers::CTRL;
        assert_eq!(capture_key(Key::ControlLeft, None, ctrl), Captured::Pending);
        assert_eq!(
            capture_key(Key::A, None, Modifiers::NONE),
            Captured::Rejected(CaptureNote::NeedsModifier)
        );
        assert_eq!(
            capture_key(Key::Minus, None, ctrl),
            Captured::Combo("Ctrl+Minus".into())
        );
        assert_eq!(
            capture_key(Key::F20, None, Modifiers::NONE),
            Captured::Combo("F20".into())
        );
        assert_eq!(
            capture_key(Key::LaunchApp1, None, ctrl),
            Captured::Rejected(CaptureNote::Unsupported("LaunchApp1".into()))
        );
    }

    #[test]
    fn tab_names_map_to_sections() {
        assert_eq!(Section::from_tab("presets"), Some(Section::Presets));
        assert_eq!(Section::from_tab("Hotkeys"), Some(Section::Hotkeys));
        assert_eq!(Section::from_tab("nope"), None);
    }

    #[test]
    fn sentinel_text_parses() {
        let p = |s| parse_sentinel(s, -1.0, "px", "same as padding");
        assert_eq!(p("12 px"), Some(12.0));
        assert_eq!(p(" 7px "), Some(7.0));
        assert_eq!(p("3"), Some(3.0));
        assert_eq!(p("same as padding"), Some(-1.0));
        assert_eq!(p("Same"), Some(-1.0));
        assert_eq!(p("bogus"), None);
        assert_eq!(p(""), None);
        let p = |s| parse_sentinel(s, 0.0, "s", "off");
        assert_eq!(p("off"), Some(0.0));
        assert_eq!(p("90 s"), Some(90.0));
    }

    #[test]
    fn field_tables_point_at_their_fields() {
        let mut cfg = Config::default();
        for (i, (_, _, field)) in HOTKEYS.iter().enumerate() {
            *field(&mut cfg) = format!("F{i}");
        }
        assert_eq!(
            (
                cfg.hotkey_toggle.as_str(),
                cfg.hotkey_reset.as_str(),
                cfg.hotkey_visibility.as_str()
            ),
            ("F0", "F1", "F2")
        );
        for (label, field) in COLORS {
            *field(&mut cfg) = label.into();
        }
        assert_eq!(cfg.color_bg, "Background");
        assert_eq!(cfg.color_timer_pause, "Timer paused");
    }

    #[test]
    fn color_hex_round_trip() {
        assert_eq!(rgb_to_hex(hex_to_rgb("#00ff88")), "#00FF88");
        assert_eq!(hex_to_rgb("junk"), [255, 255, 255]);
    }
}
