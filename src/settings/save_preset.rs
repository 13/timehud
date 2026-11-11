//! The "Save current as preset…" name prompt.

use super::{ACCENT_DARK, BG, ERROR, apply_style, native_options};
use crate::ipc::{Event, Request, send, socket_path};
use eframe::egui::{
    self, Align, Color32, Frame, Key, Layout, Margin, RichText, Ui, Vec2, ViewportCommand,
};

struct SavePreset {
    name: String,
    focused: bool,
    /// The HUD refused the preset; shown instead of closing.
    error: Option<String>,
    /// Height of the panel content last frame.
    content_h: f32,
}

impl SavePreset {
    /// Resizes the (non-resizable) window to last frame's content height,
    /// so a wrapped error message is not cut off.
    fn fit_height(&mut self, ui: &Ui) {
        let ctx = ui.ctx();
        let want = (self.content_h + 2.0 * MARGIN).max(MIN_H).round();
        let have = ctx.input(|i| i.viewport().inner_rect.map(|r| r.height()));
        if self.content_h > 0.0 && have.is_some_and(|h| (h - want).abs() > 1.0) {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(WIDTH, want)));
        }
    }
}

const WIDTH: f32 = 320.0;
const MIN_H: f32 = 120.0;
const MARGIN: f32 = 16.0;

impl eframe::App for SavePreset {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                ui.label(RichText::new("Preset name").color(Color32::WHITE));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .hint_text("e.g. Plank 60s")
                        .desired_width(f32::INFINITY),
                );
                if !self.focused {
                    r.request_focus();
                    self.focused = true;
                }
                if let Some(err) = &self.error {
                    // Long messages wrap; the window grows to fit below.
                    ui.add(egui::Label::new(RichText::new(err.as_str()).color(ERROR)).wrap());
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let escape = ui.input(|i| i.key_pressed(Key::Escape));
                let mut ok = enter;
                let mut cancel = escape;
                let row = Vec2::new(ui.available_width(), 28.0);
                ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                    let okb = egui::Button::new(RichText::new("OK").strong().color(Color32::WHITE))
                        .fill(ACCENT_DARK)
                        .min_size(Vec2::new(76.0, 28.0));
                    ok |= ui.add(okb).clicked();
                    cancel |= ui
                        .add(egui::Button::new("Cancel").min_size(Vec2::new(76.0, 28.0)))
                        .clicked();
                });
                self.content_h = ui.min_rect().height();
                if ok {
                    let name = self.name.trim();
                    if name.is_empty() {
                        return;
                    }
                    match socket_path()
                        .and_then(|p| send(&p, &Request::SavePreset { name: name.into() }))
                    {
                        Ok(Event::Error { message }) => {
                            self.error = Some(message);
                            ctx.request_repaint();
                            return;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            use std::io::ErrorKind::{ConnectionRefused, NotFound};
                            self.error = Some(match e.kind() {
                                NotFound | ConnectionRefused => "TimeHUD is not running.".into(),
                                _ => format!("Cannot reach TimeHUD: {e}"),
                            });
                            ctx.request_repaint();
                            return;
                        }
                    }
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                } else if cancel {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            });
        self.fit_height(ui);
    }
}

pub fn run_save_preset() -> anyhow::Result<()> {
    eframe::run_native(
        "TimeHUD",
        native_options("Save preset", [320.0, 120.0], false),
        Box::new(|cc| {
            apply_style(&cc.egui_ctx);
            Ok(Box::new(SavePreset {
                name: String::new(),
                focused: false,
                content_h: 0.0,
                error: None,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("save-preset window: {e}"))
}
