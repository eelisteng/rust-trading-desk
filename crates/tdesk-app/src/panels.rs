//! Small reusable egui widgets shared by the terminal panels.

use egui::{Color32, RichText, Ui};

use crate::theme::Palette;

pub fn metric_card(ui: &mut Ui, label: &str, value: &str, color: Color32, palette: &Palette) {
    egui::Frame::NONE
        .fill(palette.panel)
        .corner_radius(6.0)
        .inner_margin(egui::vec2(10.0, 8.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(10.0).color(palette.muted));
                ui.label(RichText::new(value).size(15.0).strong().color(color));
            });
        });
}

pub fn section_label(ui: &mut Ui, text: &str, palette: &Palette) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(12.0).strong().color(palette.text));
    ui.separator();
}

/// A compact striped table. Each cell is `(text, color)`.
pub fn table(
    ui: &mut Ui,
    id: &str,
    headers: &[&str],
    rows: &[Vec<(String, Color32)>],
    palette: &Palette,
) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new(id)
                .striped(true)
                .min_col_width(70.0)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for header in headers {
                        ui.label(
                            RichText::new(*header)
                                .size(10.0)
                                .strong()
                                .color(palette.muted),
                        );
                    }
                    ui.end_row();
                    if rows.is_empty() {
                        ui.label(
                            RichText::new("暂无记录")
                                .size(11.0)
                                .color(palette.muted),
                        );
                        ui.end_row();
                        return;
                    }
                    for row in rows {
                        for (text, color) in row {
                            ui.label(RichText::new(text).size(11.0).color(*color));
                        }
                        ui.end_row();
                    }
                });
        });
}
