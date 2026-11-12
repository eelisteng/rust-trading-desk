use std::sync::Arc;

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, Visuals};
use tdesk_core::MarketRules;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub up: Color32,
    pub down: Color32,
    pub flat: Color32,
    pub accent: Color32,
    pub warn: Color32,
    pub grid: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub panel: Color32,
    pub histogram: Color32,
    pub ma_fast: Color32,
    pub ma_slow: Color32,
}

impl Palette {
    pub fn new(dark: bool, rules: &dyn MarketRules) -> Self {
        let red = Color32::from_rgb(0xF2, 0x4B, 0x53);
        let green = Color32::from_rgb(0x18, 0xB8, 0x7C);
        let (up, down) = if rules.rise_is_red() {
            (red, green)
        } else {
            (green, red)
        };
        if dark {
            Self {
                up,
                down,
                flat: Color32::from_rgb(0x8B, 0x94, 0xA6),
                accent: Color32::from_rgb(0x4C, 0x9A, 0xFF),
                warn: Color32::from_rgb(0xFF, 0xB0, 0x2E),
                grid: Color32::from_rgb(0x2A, 0x31, 0x3E),
                text: Color32::from_rgb(0xE6, 0xEB, 0xF2),
                muted: Color32::from_rgb(0x8B, 0x94, 0xA6),
                panel: Color32::from_rgb(0x18, 0x1E, 0x28),
                histogram: Color32::from_rgb(0x3A, 0x45, 0x56),
                ma_fast: Color32::from_rgb(0xFF, 0xC4, 0x53),
                ma_slow: Color32::from_rgb(0xA0, 0x7B, 0xFF),
            }
        } else {
            Self {
                up,
                down,
                flat: Color32::from_rgb(0x6B, 0x72, 0x80),
                accent: Color32::from_rgb(0x1D, 0x6F, 0xEB),
                warn: Color32::from_rgb(0xB7, 0x6E, 0x00),
                grid: Color32::from_rgb(0xE1, 0xE6, 0xEC),
                text: Color32::from_rgb(0x1A, 0x20, 0x2A),
                muted: Color32::from_rgb(0x6B, 0x72, 0x80),
                panel: Color32::from_rgb(0xF1, 0xF4, 0xF8),
                histogram: Color32::from_rgb(0xC7, 0xD0, 0xDC),
                ma_fast: Color32::from_rgb(0xDB, 0x8B, 0x00),
                ma_slow: Color32::from_rgb(0x7B, 0x4D, 0xE0),
            }
        }
    }

    /// Chinese convention handled by the market rules, never hardcoded in UI.
    pub fn change_color(&self, change: f64) -> Color32 {
        if change > 0.0 {
            self.up
        } else if change < 0.0 {
            self.down
        } else {
            self.flat
        }
    }
}

pub fn apply_visuals(ctx: &Context, dark: bool) {
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = if dark {
        Color32::from_rgb(0x12, 0x17, 0x20)
    } else {
        Color32::from_rgb(0xFA, 0xFB, 0xFD)
    };
    ctx.set_visuals(visuals);
}

/// egui's bundled fonts have no CJK glyphs. Load a system font on macOS and
/// prepend it to both font families so Chinese labels render correctly.
pub fn setup_fonts(ctx: &Context) -> bool {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "cjk".to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&family) {
                    list.insert(0, "cjk".to_owned());
                }
            }
            ctx.set_fonts(fonts);
            return true;
        }
    }
    false
}
