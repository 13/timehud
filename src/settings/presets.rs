//! Presets section: the editor's pure model plus its egui view.

use super::{
    ACCENT, ACCENT_DARK, CARD, ERROR, FIELD, TEXT, WEAK, card, countdown_note, form, note, spin,
};
use crate::config::{
    COUNTDOWN_RANGE, Config, Preset, PresetRules, REST_RANGE, ROUNDS_RANGE, WORK_RANGE,
    interval_preset_rounds, valid_presets,
};
use crate::format::fmt_seconds;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, RichText, Sense, Stroke, Ui, Vec2,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetKind {
    Countdown,
    Interval,
    Stopwatch,
}

/// The editor fields below the preset list (the `_presets_tab` widgets).
#[derive(Clone, Debug, PartialEq)]
pub struct PresetEditor {
    pub name: String,
    pub kind: PresetKind,
    /// Countdown duration, seconds.
    pub duration: i64,
    pub iv_work: i64,
    pub iv_rest: i64,
    /// Interval rounds; the preset stores `total` = (work + rest) × rounds.
    pub iv_rounds: i64,
    /// The loaded preset's `total`, kept while it still yields `iv_rounds`
    /// (a total that is no round multiple must not drift on Update).
    pub iv_total: Option<i64>,
    pub sw_work: i64,
    pub sw_rest: i64,
    pub boundary: bool,
    pub halfway: bool,
    pub last5: bool,
}

impl PresetEditor {
    /// Fresh editor; the sound checkboxes start from the config (`_load_values`).
    pub fn for_config(cfg: &Config) -> PresetEditor {
        PresetEditor {
            name: String::new(),
            kind: PresetKind::Countdown,
            duration: 300,
            iv_work: 45,
            iv_rest: 15,
            iv_rounds: 10,
            iv_total: None,
            sw_work: 45,
            sw_rest: 15,
            boundary: cfg.phase_beeps,
            halfway: cfg.halfway_beep,
            last5: cfg.alert_last_5_seconds,
        }
    }
}

/// The preset the editor describes (`_preset_add`); the name is trimmed.
pub fn preset_from_editor(e: &PresetEditor) -> Result<(Preset, PresetRules), String> {
    let name = e.name.trim().to_string();
    if name.is_empty() {
        return Err("Enter a name first.".into());
    }
    let preset = match e.kind {
        PresetKind::Countdown => Preset::Countdown {
            name,
            duration: e.duration,
        },
        PresetKind::Interval => {
            if e.iv_work < 1 || e.iv_rest < 0 || e.iv_rounds < 1 {
                return Err("Work and rounds must be at least 1.".into());
            }
            Preset::Interval {
                name,
                work: e.iv_work,
                rest: e.iv_rest,
                total: interval_total(e),
            }
        }
        PresetKind::Stopwatch => Preset::Stopwatch {
            name,
            work: e.sw_work,
            rest: e.sw_rest,
        },
    };
    let rules = PresetRules {
        last5: Some(e.last5),
        boundary: Some(e.boundary),
        halfway: Some(e.halfway),
    };
    Ok((preset, rules))
}

/// Editor fields for a selected preset (`_preset_selected`). Missing rules
/// default to boundary on, halfway off, last-5 = `last5_default`.
pub fn editor_from_preset(p: &Preset, r: &PresetRules, last5_default: bool) -> PresetEditor {
    let mut e = PresetEditor {
        name: p.name().to_string(),
        boundary: r.boundary.unwrap_or(true),
        halfway: r.halfway.unwrap_or(false),
        last5: r.last5.unwrap_or(last5_default),
        ..PresetEditor::for_config(&Config::default())
    };
    match p {
        Preset::Countdown { duration, .. } => {
            e.kind = PresetKind::Countdown;
            e.duration = *duration;
        }
        Preset::Interval {
            work, rest, total, ..
        } => {
            e.kind = PresetKind::Interval;
            e.iv_work = *work;
            e.iv_rest = *rest;
            e.iv_rounds = interval_preset_rounds(*work, *rest, *total);
            e.iv_total = Some(*total);
        }
        Preset::Stopwatch { work, rest, .. } => {
            e.kind = PresetKind::Stopwatch;
            e.sw_work = *work;
            e.sw_rest = *rest;
        }
    }
    e
}

/// The interval `total` to store: the loaded one while it still gives the
/// edited rounds (and covers a work phase), else (work + rest) × rounds.
pub fn interval_total(e: &PresetEditor) -> i64 {
    match e.iv_total {
        Some(t)
            if t >= e.iv_work && interval_preset_rounds(e.iv_work, e.iv_rest, t) == e.iv_rounds =>
        {
            t
        }
        _ => (e.iv_work + e.iv_rest) * e.iv_rounds.max(1),
    }
}

// ── List operations ─────────────────────────────────────────────────────────
//
// They work on the raw `cfg.presets` values: entries the editor did not
// touch stay verbatim (unknown keys, malformed entries and all), and an
// edited one only has its known keys rewritten (`Preset::merge_into`).

/// A valid preset, its rules and its index into the raw `cfg.presets`.
pub type Entry = (Preset, PresetRules, usize);

/// The presets the HUD would offer, in order.
pub fn entries(cfg: &Config) -> Vec<Entry> {
    valid_presets(&cfg.presets)
}

/// What the editor's main button does for `selected` (a raw index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveAction {
    Add,
    Update,
    Rename,
}

fn find_by_name<'a>(list: &'a [Entry], name: &str) -> Option<&'a Entry> {
    list.iter().find(|(p, _, _)| p.name() == name)
}

pub fn save_action(cfg: &Config, selected: Option<usize>, e: &PresetEditor) -> SaveAction {
    let list = entries(cfg);
    let name = e.name.trim();
    match selected.and_then(|i| list.iter().find(|(_, _, raw)| *raw == i)) {
        Some((p, _, _)) if p.name() == name || name.is_empty() => SaveAction::Update,
        Some(_) => SaveAction::Rename,
        None if find_by_name(&list, name).is_some() => SaveAction::Update,
        None => SaveAction::Add,
    }
}

/// Saves the editor into `cfg.presets`: the selected entry is updated in
/// place (renamed if the name changed, refused if another preset has the new
/// name); without a selection a preset of the same name is updated, else one
/// is appended. When the edited preset is the active one, its values are
/// applied to the live config and `active_preset` follows a rename.
/// Returns the raw index of the saved entry.
pub fn save_editor(
    cfg: &mut Config,
    selected: Option<usize>,
    e: &PresetEditor,
) -> Result<usize, String> {
    let (preset, rules) = preset_from_editor(e)?;
    let list = entries(cfg);
    let current = selected.and_then(|i| list.iter().find(|(_, _, raw)| *raw == i));
    let clash = list
        .iter()
        .find(|(p, _, raw)| p.name() == preset.name() && Some(*raw) != selected);
    let target = match (current, clash) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "A preset named \u{201c}{}\u{201d} already exists; pick another name.",
                preset.name()
            ));
        }
        (Some(c), None) => Some(c),
        (None, other) => other,
    };
    let index = match target {
        Some((old, _, raw)) => {
            let was_active = !cfg.active_preset.is_empty() && cfg.active_preset == old.name();
            preset.merge_into(&rules, &mut cfg.presets[*raw]);
            if was_active {
                preset.apply_to(&rules, cfg);
            }
            *raw
        }
        None => {
            cfg.presets.push(preset.to_value(&rules));
            cfg.presets.len() - 1
        }
    };
    Ok(index)
}

/// Removes the raw entry `raw`; ends the preset if it was the active one.
pub fn remove(cfg: &mut Config, raw: usize) {
    if raw >= cfg.presets.len() {
        return;
    }
    let removed = cfg.presets.remove(raw);
    if let Some((p, _, _)) = valid_presets(std::slice::from_ref(&removed)).first()
        && cfg.active_preset == p.name()
    {
        cfg.active_preset.clear();
    }
}

/// Moves item `from` so it lands before the item currently at `to`
/// (`to == len` = end). Returns its new index.
pub fn move_item<T>(list: &mut Vec<T>, from: usize, to: usize) -> usize {
    let item = list.remove(from);
    let to = if to > from { to - 1 } else { to };
    let to = to.min(list.len());
    list.insert(to, item);
    to
}

// ── View ────────────────────────────────────────────────────────────────────

pub struct PresetsState {
    pub editor: PresetEditor,
    /// Raw index into `cfg.presets` and the name it had when selected.
    pub selected: Option<(usize, String)>,
    pub message: Option<String>,
}

impl PresetsState {
    pub fn new(cfg: &Config) -> PresetsState {
        PresetsState {
            editor: PresetEditor::for_config(cfg),
            selected: None,
            message: None,
        }
    }

    /// The selected raw index, re-found by name if the list changed under
    /// us (e.g. the HUD saved a preset); dropped if the preset is gone.
    fn resolve(&mut self, list: &[Entry]) -> Option<usize> {
        let (raw, name) = self.selected.as_ref()?;
        let found = list
            .iter()
            .find(|(p, _, r)| r == raw && p.name() == name)
            .or_else(|| find_by_name(list, name))
            .map(|(_, _, r)| *r);
        match (found, self.selected.as_mut()) {
            (Some(r), Some(s)) => s.0 = r,
            _ => self.selected = None,
        }
        found
    }

    fn select(&mut self, cfg: &Config, raw: usize) {
        self.selected = entries(cfg)
            .into_iter()
            .find(|(_, _, r)| *r == raw)
            .map(|(p, _, r)| (r, p.name().to_string()));
    }
}

/// Drag payload: index of the dragged preset.
struct Dragged(usize);

fn detail(p: &Preset) -> String {
    match p {
        Preset::Countdown { duration, .. } => fmt_seconds(*duration as f64),
        Preset::Interval {
            work, rest, total, ..
        } => format!(
            "{work}/{rest} \u{d7}{}",
            interval_preset_rounds(*work, *rest, *total)
        ),
        Preset::Stopwatch { work, rest, .. } => format!("{work}/{rest} \u{2191}"),
    }
}

fn kind_name(k: PresetKind) -> &'static str {
    match k {
        PresetKind::Countdown => "countdown",
        PresetKind::Interval => "interval",
        PresetKind::Stopwatch => "stopwatch",
    }
}

pub fn section(ui: &mut Ui, st: &mut PresetsState, cfg: &mut Config) {
    let list = entries(cfg);
    let selected = st.resolve(&list);

    card(ui, "Presets", |ui| {
        if list.is_empty() {
            ui.label(RichText::new("No presets yet. Add one below.").color(WEAK));
            return;
        }
        let row_h = 30.0;
        let (zone, dropped) = ui.dnd_drop_zone::<Dragged, Option<usize>>(
            Frame::new()
                .fill(FIELD)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(4)),
            |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                let mut target = None;
                let dragging = egui::DragAndDrop::has_payload_of_type::<Dragged>(ui.ctx());
                for (i, (p, rules, raw)) in list.iter().enumerate() {
                    // One widget that senses both: with `dnd_drag_source` a
                    // drag-only widget on top swallowed the row's clicks.
                    let row = {
                        let (rect, r) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), row_h),
                            Sense::click_and_drag(),
                        );
                        r.dnd_set_drag_payload(Dragged(i));
                        let selected = selected == Some(*raw);
                        let painter = ui.painter();
                        if selected {
                            painter.rect_filled(
                                rect,
                                CornerRadius::same(6),
                                ACCENT.gamma_multiply(0.16),
                            );
                            painter.rect_stroke(
                                rect,
                                CornerRadius::same(6),
                                Stroke::new(1.0, ACCENT.gamma_multiply(0.6)),
                                egui::StrokeKind::Inside,
                            );
                        } else if r.hovered() {
                            painter.rect_filled(rect, CornerRadius::same(6), CARD);
                        }
                        painter.text(
                            rect.left_center() + Vec2::new(10.0, 0.0),
                            Align2::LEFT_CENTER,
                            "\u{2261}",
                            FontId::proportional(16.0),
                            WEAK,
                        );
                        painter.text(
                            rect.left_center() + Vec2::new(30.0, 0.0),
                            Align2::LEFT_CENTER,
                            p.name(),
                            FontId::proportional(14.0),
                            if selected { Color32::WHITE } else { TEXT },
                        );
                        painter.text(
                            rect.right_center() - Vec2::new(10.0, 0.0),
                            Align2::RIGHT_CENTER,
                            detail(p),
                            FontId::monospace(13.0),
                            if selected { ACCENT } else { WEAK },
                        );
                        r.on_hover_cursor(egui::CursorIcon::Grab)
                    };
                    if row.clicked() {
                        st.selected = Some((*raw, p.name().to_string()));
                        st.editor = editor_from_preset(p, rules, cfg.alert_last_5_seconds);
                        st.message = None;
                    }
                    let rect = row.rect;
                    if dragging
                        && let Some(pos) = ui.ctx().pointer_hover_pos()
                        && pos.y >= rect.top() - 1.0
                        && pos.y < rect.bottom() + 1.0
                    {
                        let before = pos.y < rect.center().y;
                        target = Some(if before { i } else { i + 1 });
                        let y = if before {
                            rect.top() - 1.0
                        } else {
                            rect.bottom() + 1.0
                        };
                        ui.painter()
                            .hline(rect.x_range(), y, Stroke::new(2.0, ACCENT));
                    }
                }
                target
            },
        );
        if let (Some(from), Some(to)) = (dropped, zone.inner)
            && let Some((_, _, raw_from)) = list.get(from.0)
        {
            // Raw indices: malformed entries keep their place among the rest.
            let raw_to = list.get(to).map_or(cfg.presets.len(), |e| e.2);
            move_item(&mut cfg.presets, *raw_from, raw_to);
            // `resolve` re-finds the selection by name next frame.
        }
        ui.add_space(2.0);
        note(ui, "Drag to reorder \u{b7} click to edit");
    });

    ui.add_space(10.0);
    let mut saved = None;
    card(ui, "Editor", |ui| {
        let e = &mut st.editor;
        form(ui, "preset-editor", |ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut e.name)
                    .hint_text("Name")
                    .desired_width(220.0),
            );
            ui.end_row();

            ui.label("Type");
            egui::ComboBox::from_id_salt("preset-kind")
                .selected_text(kind_name(e.kind))
                .show_ui(ui, |ui| {
                    for k in [
                        PresetKind::Countdown,
                        PresetKind::Interval,
                        PresetKind::Stopwatch,
                    ] {
                        ui.selectable_value(&mut e.kind, k, kind_name(k));
                    }
                });
            ui.end_row();

            match e.kind {
                PresetKind::Countdown => {
                    ui.label("Duration");
                    ui.horizontal(|ui| {
                        spin(ui, &mut e.duration, COUNTDOWN_RANGE, " s");
                        note(ui, countdown_note(e.duration));
                    });
                    ui.end_row();
                }
                PresetKind::Interval => {
                    ui.label("Work / rest");
                    ui.horizontal(|ui| {
                        spin(ui, &mut e.iv_work, WORK_RANGE, " s work");
                        spin(ui, &mut e.iv_rest, REST_RANGE, " s rest");
                    });
                    ui.end_row();
                    ui.label("Rounds");
                    ui.horizontal(|ui| {
                        spin(ui, &mut e.iv_rounds, ROUNDS_RANGE, " rounds");
                        let total = interval_total(e);
                        note(ui, format!("\u{2192} {} total", fmt_seconds(total as f64)));
                    });
                    ui.end_row();
                }
                PresetKind::Stopwatch => {
                    ui.label("Work / rest");
                    ui.horizontal(|ui| {
                        spin(ui, &mut e.sw_work, WORK_RANGE, " s work");
                        spin(ui, &mut e.sw_rest, REST_RANGE, " s rest");
                    });
                    ui.end_row();
                }
            }

            ui.label("Beeps");
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.checkbox(&mut e.boundary, "phase-end beep")
                    .on_hover_text("Long beep when a work/rest phase ends");
                ui.checkbox(&mut e.halfway, "halfway double")
                    .on_hover_text("Fast double beep at half of each work phase");
                ui.checkbox(&mut e.last5, "last-5 beeps")
                    .on_hover_text("Short beeps at 5..1 before a phase/countdown ends");
            });
            ui.end_row();
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (label, hint) = match save_action(cfg, selected, e) {
                SaveAction::Add => ("Add", "Add a new preset"),
                SaveAction::Update => ("Update", "Save the changes to this preset"),
                SaveAction::Rename => ("Rename", "Rename the selected preset and save its changes"),
            };
            let add = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                .fill(ACCENT_DARK)
                .min_size(Vec2::new(84.0, 28.0));
            if ui.add(add).on_hover_text(hint).clicked() {
                match save_editor(cfg, selected, e) {
                    Ok(raw) => {
                        st.message = None;
                        saved = Some(raw);
                    }
                    Err(msg) => st.message = Some(msg),
                }
            }
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new("Remove").min_size(Vec2::new(84.0, 28.0)),
                )
                .clicked()
                && let Some(raw) = selected
            {
                remove(cfg, raw);
                st.selected = None;
                st.message = None;
            }
            if ui
                .add(egui::Button::new("New").min_size(Vec2::new(64.0, 28.0)))
                .on_hover_text("Clear the editor")
                .clicked()
            {
                st.selected = None;
                st.message = None;
                *e = PresetEditor::for_config(cfg);
            }
        });
        if let Some(msg) = &st.message {
            ui.add(egui::Label::new(RichText::new(msg.as_str()).color(ERROR)).wrap());
        }
    });

    if let Some(raw) = saved {
        st.select(cfg, raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;
    use serde_json::json;

    fn editor(kind: PresetKind) -> PresetEditor {
        PresetEditor {
            name: "x".into(),
            kind,
            duration: 300,
            iv_work: 45,
            iv_rest: 15,
            iv_rounds: 10,
            iv_total: None,
            sw_work: 45,
            sw_rest: 15,
            boundary: true,
            halfway: false,
            last5: false,
        }
    }

    #[test]
    fn interval_editor_rounds_round_trip() {
        let e = PresetEditor {
            name: " hiit ".into(),
            ..editor(PresetKind::Interval)
        };
        let (p, r) = preset_from_editor(&e).unwrap();
        assert_eq!(
            p,
            Preset::Interval {
                name: "hiit".into(),
                work: 45,
                rest: 15,
                total: 600
            }
        );
        let back = editor_from_preset(&p, &r, false);
        assert_eq!(back.kind, PresetKind::Interval);
        assert_eq!((back.iv_work, back.iv_rest, back.iv_rounds), (45, 15, 10));
        assert_eq!(back.name, "hiit");
    }

    #[test]
    fn interval_total_survives_an_unchanged_update() {
        // 20/10 x5 = 150 s used to load as "2 min" and save back as 120 s.
        let p = Preset::Interval {
            name: "t".into(),
            work: 20,
            rest: 10,
            total: 150,
        };
        let mut e = editor_from_preset(&p, &PresetRules::default(), false);
        assert_eq!(e.iv_rounds, 5);
        assert_eq!(preset_from_editor(&e).unwrap().0, p);
        // A total that is no multiple of a round is kept too (630 = 10 rounds).
        let odd = Preset::Interval {
            name: "t".into(),
            work: 45,
            rest: 15,
            total: 630,
        };
        let e2 = editor_from_preset(&odd, &PresetRules::default(), false);
        assert_eq!(preset_from_editor(&e2).unwrap().0, odd);
        // Editing the rounds (or a phase so the rounds change) recomputes it.
        e.iv_rounds = 6;
        assert_eq!(interval_total(&e), 180);
        e.iv_rounds = 5;
        e.iv_work = 25;
        assert_eq!(interval_total(&e), 175);
    }

    #[test]
    fn countdown_editor_round_trip() {
        let e = PresetEditor {
            name: "Plank".into(),
            duration: 90,
            halfway: true,
            last5: true,
            ..editor(PresetKind::Countdown)
        };
        let (p, r) = preset_from_editor(&e).unwrap();
        assert_eq!(
            p,
            Preset::Countdown {
                name: "Plank".into(),
                duration: 90
            }
        );
        assert_eq!(
            r,
            PresetRules {
                last5: Some(true),
                boundary: Some(true),
                halfway: Some(true)
            }
        );
        let back = editor_from_preset(&p, &r, false);
        assert_eq!(back, PresetEditor { ..e });
    }

    #[test]
    fn stopwatch_editor_round_trip() {
        let e = PresetEditor {
            sw_work: 60,
            sw_rest: 0,
            ..editor(PresetKind::Stopwatch)
        };
        let (p, r) = preset_from_editor(&e).unwrap();
        assert_eq!(
            p,
            Preset::Stopwatch {
                name: "x".into(),
                work: 60,
                rest: 0
            }
        );
        assert_eq!(editor_from_preset(&p, &r, false), e);
    }

    #[test]
    fn empty_name_rejected() {
        let e = PresetEditor {
            name: "  ".into(),
            ..editor(PresetKind::Countdown)
        };
        assert!(preset_from_editor(&e).is_err());
    }

    #[test]
    fn missing_rules_use_python_defaults() {
        let p = Preset::Countdown {
            name: "a".into(),
            duration: 60,
        };
        let e = editor_from_preset(&p, &PresetRules::default(), true);
        assert!(e.boundary && !e.halfway && e.last5);
    }

    fn names(cfg: &Config) -> Vec<String> {
        entries(cfg)
            .iter()
            .map(|(p, _, _)| p.name().to_string())
            .collect()
    }

    fn cfg_with(presets: Vec<serde_json::Value>) -> Config {
        Config {
            presets,
            ..Config::default()
        }
    }

    fn countdown(name: &str, duration: i64) -> PresetEditor {
        PresetEditor {
            name: name.into(),
            duration,
            ..editor(PresetKind::Countdown)
        }
    }

    #[test]
    fn save_without_selection_updates_same_name_or_appends() {
        let mut cfg = Config::default(); // "1 min", "5 min"
        assert_eq!(
            save_action(&cfg, None, &countdown("1 min", 61)),
            SaveAction::Update
        );
        assert_eq!(save_editor(&mut cfg, None, &countdown("1 min", 61)), Ok(0));
        assert_eq!(cfg.presets[0]["duration"], 61);
        assert_eq!(save_action(&cfg, None, &countdown("c", 5)), SaveAction::Add);
        assert_eq!(save_editor(&mut cfg, None, &countdown("c", 5)), Ok(2));
        assert_eq!(names(&cfg), ["1 min", "5 min", "c"]);
    }

    #[test]
    fn renaming_the_selected_preset_keeps_its_place() {
        let mut cfg = Config::default();
        let e = countdown("one", 60);
        assert_eq!(save_action(&cfg, Some(0), &e), SaveAction::Rename);
        assert_eq!(save_editor(&mut cfg, Some(0), &e), Ok(0));
        assert_eq!(names(&cfg), ["one", "5 min"]);
        // Taking another preset's name is refused, changing nothing.
        let before = cfg.presets.clone();
        let err = save_editor(&mut cfg, Some(0), &countdown("5 min", 60)).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(cfg.presets, before);
    }

    #[test]
    fn updating_the_active_preset_applies_it_and_follows_a_rename() {
        let mut cfg = Config::default();
        cfg.presets[1] = json!({"name": "5 min", "duration": 300});
        Preset::Countdown {
            name: "5 min".into(),
            duration: 300,
        }
        .apply_to(&PresetRules::default(), &mut cfg);
        assert_eq!(cfg.active_preset, "5 min");
        save_editor(&mut cfg, Some(1), &countdown("6 min", 360)).unwrap();
        assert_eq!(cfg.active_preset, "6 min");
        assert_eq!(cfg.timer_mode, Mode::Countdown);
        assert_eq!(cfg.countdown_duration, 360);
        // Editing an inactive preset leaves the live setup alone.
        save_editor(&mut cfg, Some(0), &countdown("1 min", 30)).unwrap();
        assert_eq!(cfg.countdown_duration, 360);
        assert_eq!(cfg.active_preset, "6 min");
    }

    #[test]
    fn edits_keep_unknown_keys_and_untouched_entries_verbatim() {
        let mut cfg = cfg_with(vec![
            json!({"name": "a", "duration": 60, "icon": "tea"}),
            json!({"name": "broken", "duration": "soon"}),
            json!({"name": "b", "duration": 30.0, "note": 1}),
            json!({"name": "c", "duration": 10, "color": "#fff"}),
        ]);
        let untouched = cfg.presets.clone();
        // "b" has a float duration: not a valid preset, kept as is.
        assert_eq!(names(&cfg), ["a", "c"]);
        save_editor(&mut cfg, Some(0), &countdown("a2", 90)).unwrap();
        assert_eq!(
            cfg.presets[0],
            json!({"name": "a2", "duration": 90, "icon": "tea",
            "last5": false, "boundary": true, "halfway": false})
        );
        assert_eq!(cfg.presets[1..], untouched[1..]);
        remove(&mut cfg, 3);
        assert_eq!(cfg.presets[1..], untouched[1..3]);
    }

    #[test]
    fn remove_ends_the_active_preset() {
        let mut cfg = Config {
            active_preset: "5 min".into(),
            ..Config::default()
        };
        remove(&mut cfg, 0);
        assert_eq!(cfg.active_preset, "5 min");
        remove(&mut cfg, 0);
        assert_eq!(cfg.active_preset, "");
        assert!(cfg.presets.is_empty());
    }

    #[test]
    fn move_item_down_and_up() {
        let mut list = vec!["a", "b", "c"];
        assert_eq!(move_item(&mut list, 0, 3), 2);
        assert_eq!(list, ["b", "c", "a"]);
        assert_eq!(move_item(&mut list, 2, 0), 0);
        assert_eq!(list, ["a", "b", "c"]);
        assert_eq!(move_item(&mut list, 0, 1), 0); // dropped onto itself
        assert_eq!(list, ["a", "b", "c"]);
    }

    #[test]
    fn selection_is_refound_by_name_after_the_list_changes() {
        let mut cfg = Config::default();
        let mut st = PresetsState::new(&cfg);
        st.select(&cfg, 1);
        assert_eq!(st.selected, Some((1, "5 min".into())));
        move_item(&mut cfg.presets, 1, 0);
        assert_eq!(st.resolve(&entries(&cfg)), Some(0));
        remove(&mut cfg, 0);
        assert_eq!(st.resolve(&entries(&cfg)), None);
        assert_eq!(st.selected, None);
    }

    #[test]
    fn new_interval_writes_python_shape() {
        let mut cfg = Config::default();
        let e = PresetEditor {
            name: "hiit".into(),
            kind: PresetKind::Interval,
            ..PresetEditor::for_config(&cfg)
        };
        save_editor(&mut cfg, None, &e).unwrap();
        assert_eq!(
            cfg.presets[2],
            json!({"name": "hiit", "type": "interval", "work": 45, "rest": 15,
                "total": 600, "last5": false, "boundary": true, "halfway": false})
        );
    }
}
