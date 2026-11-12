use tdesk_app::{theme::setup_fonts, TradingApp};
use tdesk_core::Market;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("tdesk · Rust 模拟交易终端（模拟盘）")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "tdesk",
        options,
        Box::new(|cc| {
            if !setup_fonts(&cc.egui_ctx) {
                eprintln!("警告：未找到可用的中文字体，界面中文可能显示为方块。");
            }
            Ok(Box::new(TradingApp::new(Market::AShare)))
        }),
    )
}
