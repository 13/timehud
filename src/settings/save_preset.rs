//! The "Save current as preset…" name prompt.

use super::{ACCENT_DARK, BG, apply_style, native_options};
use crate::ipc::{Request, send, socket_path};
use eframe::egui::{
    self, Align, Color32, Frame, Key, Layout, Margin, RichText, Ui, Vec2, ViewportCommand,
};

struct SavePreset {
    name: String,
    focused: bool,
}

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
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let escape = ui.input(|i| i.key_pressed(Key::Escape));
                let mut ok = enter;
                let mut cancel = escape;
                ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
                    let okb = egui::Button::new(RichText::new("OK").strong().color(Color32::WHITE))
                        .fill(ACCENT_DARK)
                        .min_size(Vec2::new(76.0, 28.0));
                    ok |= ui.add(okb).clicked();
                    cancel |= ui
                        .add(egui::Button::new("Cancel").min_size(Vec2::new(76.0, 28.0)))
                        .clicked();
                });
                if ok {
                    let name = self.name.trim();
                    if name.is_empty() {
                        return;
                    }
                    if let Err(e) = send(&socket_path(), &Request::SavePreset { name: name.into() })
                    {
                        eprintln!("timehud: cannot save preset: {e}");
                    }
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                } else if cancel {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            });
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
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("save-preset window: {e}"))
}
