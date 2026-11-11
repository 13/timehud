//! Presets section: the editor's pure model plus its egui view.

use super::{
    ACCENT, ACCENT_DARK, CARD, ERROR, FIELD, TEXT, WEAK, card, countdown_note, form, note, spin,
};
use crate::config::{Config, Preset, PresetRules, interval_preset_rounds, valid_presets};
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
    /// Interval total time, minutes.
    pub iv_total_min: i64,
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
            iv_total_min: 10,
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
            let total = e.iv_total_min * 60;
            if total < e.iv_work {
                return Err("Total time must be at least one work phase.".into());
            }
            Preset::Interval {
                name,
                work: e.iv_work,
                rest: e.iv_rest,
                total,
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
            // Python's round(): halves go to the even neighbour.
            e.iv_total_min = ((*total as f64 / 60.0).round_ties_even() as i64).max(1);
        }
        Preset::Stopwatch { work, rest, .. } => {
            e.kind = PresetKind::Stopwatch;
            e.sw_work = *work;
            e.sw_rest = *rest;
        }
    }
    e
}

// ── List operations ─────────────────────────────────────────────────────────

pub type Entry = (Preset, PresetRules);

/// The valid presets of the config (malformed raw entries are dropped on the
/// next write, as Python's add/remove do).
pub fn entries(cfg: &Config) -> Vec<Entry> {
    valid_presets(&cfg.presets)
        .into_iter()
        .map(|(p, r, _)| (p, r))
        .collect()
}

fn store(cfg: &mut Config, list: &[Entry]) {
    cfg.presets = list.iter().map(|(p, r)| p.to_value(r)).collect();
}

/// Replaces the preset of the same name in place, else appends. Returns its index.
pub fn upsert(list: &mut Vec<Entry>, entry: Entry) -> usize {
    match list.iter().position(|(p, _)| p.name() == entry.0.name()) {
        Some(i) => {
            list[i] = entry;
            i
        }
        None => {
            list.push(entry);
            list.len() - 1
        }
    }
}

/// Moves item `from` so it lands before the item currently at `to`
/// (`to == len` = end). Returns its new index.
pub fn move_item(list: &mut Vec<Entry>, from: usize, to: usize) -> usize {
    let item = list.remove(from);
    let to = if to > from { to - 1 } else { to };
    let to = to.min(list.len());
    list.insert(to, item);
    to
}

// ── View ────────────────────────────────────────────────────────────────────

pub struct PresetsState {
    pub editor: PresetEditor,
    pub selected: Option<usize>,
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
    let mut list = entries(cfg);
    if st.selected.is_some_and(|i| i >= list.len()) {
        st.selected = None;
    }
    let mut changed = false;

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
                for (i, (p, _)) in list.iter().enumerate() {
                    let id = egui::Id::unique(("timehud-preset-row", i));
                    let row = ui.dnd_drag_source(id, Dragged(i), |ui| {
                        let (rect, r) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), row_h),
                            Sense::click(),
                        );
                        let selected = st.selected == Some(i);
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
                        r
                    });
                    if row.inner.clicked() {
                        st.selected = Some(i);
                        st.editor = editor_from_preset(p, &list[i].1, cfg.alert_last_5_seconds);
                        st.message = None;
                    }
                    let rect = row.response.rect;
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
        if let (Some(from), Some(to)) = (dropped, zone.inner) {
            let from = from.0;
            let new_idx = move_item(&mut list, from, to);
            if st.selected == Some(from) {
                st.selected = Some(new_idx);
            } else {
                st.selected = None;
            }
            changed = true;
        }
        ui.add_space(2.0);
        note(ui, "Drag to reorder \u{b7} click to edit");
    });

    ui.add_space(10.0);
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
                        spin(ui, &mut e.duration, 1..=24 * 3600, " s");
                        note(ui, countdown_note(e.duration));
                    });
                    ui.end_row();
                }
                PresetKind::Interval => {
                    ui.label("Work / rest");
                    ui.horizontal(|ui| {
                        spin(ui, &mut e.iv_work, 5..=3600, " s work");
                        spin(ui, &mut e.iv_rest, 0..=3600, " s rest");
                    });
                    ui.end_row();
                    ui.label("Total");
                    ui.horizontal(|ui| {
                        spin(ui, &mut e.iv_total_min, 1..=24 * 60, " min total");
                        let rounds =
                            interval_preset_rounds(e.iv_work, e.iv_rest, e.iv_total_min * 60);
                        note(ui, format!("\u{2192} {rounds} rounds"));
                    });
                    ui.end_row();
                }
                PresetKind::Stopwatch => {
                    ui.label("Work / rest");
                    ui.horizontal(|ui| {
                        spin(ui, &mut e.sw_work, 5..=3600, " s work");
                        spin(ui, &mut e.sw_rest, 0..=3600, " s rest");
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
            let exists = list.iter().any(|(p, _)| p.name() == e.name.trim());
            let label = if exists { "Update" } else { "Add" };
            let add = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                .fill(ACCENT_DARK)
                .min_size(Vec2::new(84.0, 28.0));
            if ui.add(add).clicked() {
                match preset_from_editor(e) {
                    Ok(entry) => {
                        st.selected = Some(upsert(&mut list, entry));
                        st.message = None;
                        changed = true;
                    }
                    Err(msg) => st.message = Some(msg),
                }
            }
            let can_remove = st.selected.is_some();
            if ui
                .add_enabled(
                    can_remove,
                    egui::Button::new("Remove").min_size(Vec2::new(84.0, 28.0)),
                )
                .clicked()
                && let Some(i) = st.selected.take()
            {
                let (removed, _) = list.remove(i);
                if cfg.active_preset == removed.name() {
                    cfg.active_preset.clear();
                }
                st.message = None;
                changed = true;
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
            if let Some(msg) = &st.message {
                ui.label(RichText::new(msg.as_str()).color(ERROR));
            }
        });
    });

    if changed {
        store(cfg, &list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(kind: PresetKind) -> PresetEditor {
        PresetEditor {
            name: "x".into(),
            kind,
            duration: 300,
            iv_work: 45,
            iv_rest: 15,
            iv_total_min: 10,
            sw_work: 45,
            sw_rest: 15,
            boundary: true,
            halfway: false,
            last5: false,
        }
    }

    #[test]
    fn interval_editor_total_minutes_round_trip() {
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
        assert_eq!(
            (back.iv_work, back.iv_rest, back.iv_total_min),
            (45, 15, 10)
        );
        assert_eq!(back.name, "hiit");
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
    fn interval_total_shorter_than_work_rejected() {
        // 1 min total < 90 s work would be dropped by valid_presets.
        let e = PresetEditor {
            iv_work: 90,
            iv_total_min: 1,
            ..editor(PresetKind::Interval)
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

    #[test]
    fn total_rounds_half_to_even_like_python() {
        let p = |total| Preset::Interval {
            name: "a".into(),
            work: 30,
            rest: 0,
            total,
        };
        let r = PresetRules::default();
        assert_eq!(editor_from_preset(&p(90), &r, false).iv_total_min, 2);
        assert_eq!(editor_from_preset(&p(150), &r, false).iv_total_min, 2);
        assert_eq!(editor_from_preset(&p(30), &r, false).iv_total_min, 1);
    }

    fn entry(name: &str) -> Entry {
        (
            Preset::Countdown {
                name: name.into(),
                duration: 60,
            },
            PresetRules::default(),
        )
    }

    fn names(list: &[Entry]) -> Vec<&str> {
        list.iter().map(|(p, _)| p.name()).collect()
    }

    #[test]
    fn upsert_replaces_in_place_or_appends() {
        let mut list = vec![entry("a"), entry("b")];
        let updated = (
            Preset::Countdown {
                name: "a".into(),
                duration: 5,
            },
            PresetRules::default(),
        );
        assert_eq!(upsert(&mut list, updated.clone()), 0);
        assert_eq!(list[0], updated);
        assert_eq!(upsert(&mut list, entry("c")), 2);
        assert_eq!(names(&list), ["a", "b", "c"]);
    }

    #[test]
    fn move_item_down_and_up() {
        let mut list = vec![entry("a"), entry("b"), entry("c")];
        assert_eq!(move_item(&mut list, 0, 3), 2);
        assert_eq!(names(&list), ["b", "c", "a"]);
        assert_eq!(move_item(&mut list, 2, 0), 0);
        assert_eq!(names(&list), ["a", "b", "c"]);
        assert_eq!(move_item(&mut list, 0, 1), 0); // dropped onto itself
        assert_eq!(names(&list), ["a", "b", "c"]);
    }

    #[test]
    fn section_writes_python_shape() {
        let mut cfg = Config::default();
        let mut list = entries(&cfg);
        upsert(
            &mut list,
            preset_from_editor(&PresetEditor {
                name: "hiit".into(),
                kind: PresetKind::Interval,
                ..PresetEditor::for_config(&cfg)
            })
            .unwrap(),
        );
        store(&mut cfg, &list);
        assert_eq!(
            cfg.presets[2],
            serde_json::json!({"name": "hiit", "type": "interval", "work": 45, "rest": 15,
                "total": 600, "last5": false, "boundary": true, "halfway": false})
        );
    }
}
