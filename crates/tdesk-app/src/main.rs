//! Native GUI entry point.
//!
//! Documentation captures are reproducible and fully offline: set
//! `TDESK_CAPTURE` to a comma-separated preset spec and `TDESK_CAPTURE_TO`
//! to a .png path. The app renders a deterministic state, asks egui for a
//! framebuffer readback (before the buffer swap), writes the PNG and exits.
//! No screen-recording permission is needed. Normal runs are unaffected.

use std::{env, path::PathBuf};

use egui::{Context, Event, UserData, ViewportCommand};
use tdesk_app::{theme::setup_fonts, Tab, TradingApp};
use tdesk_core::Market;
use tdesk_data::MarketDataSource;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("tdesk · Rust 模拟交易终端（模拟盘）")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    let spec = env::var("TDESK_CAPTURE").ok();
    let capture_to = env::var("TDESK_CAPTURE_TO").ok().map(PathBuf::from);
    eframe::run_native(
        "tdesk",
        options,
        Box::new(move |cc| {
            if !setup_fonts(&cc.egui_ctx) {
                eprintln!("警告：未找到可用的中文字体，界面中文可能显示为方块。");
            }
            let app = match spec.as_deref() {
                Some(spec) => capture_app(spec),
                None => TradingApp::new(Market::AShare),
            };
            match capture_to {
                Some(path) => Ok(Box::new(CaptureApp::new(app, path))),
                None => Ok(Box::new(app)),
            }
        }),
    )
}

/// Wraps [`TradingApp`] for one-off documentation captures.
///
/// Uses egui's `ViewportCommand::Screenshot`, which reads the framebuffer
/// *before* `swap_buffers`. eframe's `__screenshot` hook reads *after* the
/// swap and returns an empty buffer on macOS, so it cannot be used here.
struct CaptureApp {
    inner: TradingApp,
    output: PathBuf,
    frames: u32,
    requested: bool,
    done: bool,
}

impl CaptureApp {
    fn new(inner: TradingApp, output: PathBuf) -> Self {
        Self {
            inner,
            output,
            frames: 0,
            requested: false,
            done: false,
        }
    }
}

impl eframe::App for CaptureApp {
    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        self.inner.update(ctx, frame);
        self.frames += 1;

        let screenshot = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });

        if let Some(image) = screenshot {
            if !self.done {
                self.done = true;
                let [width, height] = image.size;
                let mut rgba = Vec::with_capacity(width * height * 4);
                for pixel in &image.pixels {
                    rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
                }
                image::save_buffer(
                    &self.output,
                    &rgba,
                    width as u32,
                    height as u32,
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap_or_else(|err| panic!("无法写入截图 {:?}: {err}", self.output));
                eprintln!(
                    "已保存截图 {}x{} -> {}",
                    width,
                    height,
                    self.output.display()
                );
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            return;
        }

        // Let the window settle (resize/fonts) before asking for a readback.
        if !self.requested && self.frames >= 3 {
            self.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
        }
        ctx.request_repaint();
    }
}

/// Build a deterministic, fully populated UI state for docs screenshots and
/// visual regression checks. Never used in a normal run.
///
/// Spec format: `<market>,<theme>,<tab>,<ticks>,<orders>,<backtest>`
/// e.g. `ashare,dark,orders,140,3,0`
///   market:  ashare | us | hk          theme: dark | light
///   tab:     orders | trades | positions | account | backtest
///   ticks:   number of synthetic ticks to advance (deterministic, seed fixed)
///   orders:  number of crossing limit buys to place (0..4)
///   backtest: 1 to pre-run the SMA(5,20) backtest
fn capture_app(spec: &str) -> TradingApp {
    let parts: Vec<String> = spec
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    let field = |i: usize, default: &str| -> String {
        parts
            .get(i)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };

    let market = match field(0, "ashare").as_str() {
        "us" => Market::Us,
        "hk" => Market::Hk,
        _ => Market::AShare,
    };
    let dark = field(1, "dark") != "light";
    let tab = match field(2, "orders").as_str() {
        "trades" => Tab::Trades,
        "positions" => Tab::Positions,
        "account" => Tab::Account,
        "backtest" => Tab::Backtest,
        _ => Tab::Orders,
    };
    let ticks: usize = field(3, "140").parse().unwrap_or(140).min(4000);
    let orders: usize = field(4, "0").parse().unwrap_or(0).min(4);
    let with_backtest = matches!(field(5, "0").as_str(), "1" | "true" | "yes");

    let mut app = TradingApp::new(market);
    app.dark = dark;
    app.tab = tab;

    // Replay deterministic synthetic ticks so the chart shows real history.
    let symbols = app.source.symbols();
    for _ in 0..ticks {
        let mut quotes = Vec::with_capacity(symbols.len());
        for symbol in &symbols {
            let tick = app.source.next_tick(symbol);
            quotes.push((symbol.clone(), tick.last));
        }
        for (symbol, last) in quotes {
            app.engine.set_price(&symbol, last);
        }
    }
    let selected = app.selected.clone();
    let mid = app
        .engine
        .last_prices
        .get(&selected)
        .copied()
        .unwrap_or(1);
    app.engine.refresh_liquidity(&selected, mid);

    // Crossing limit buys populate Orders / Trades / Positions with real fills.
    if orders > 0 {
        let ask = app
            .engine
            .book(&selected)
            .and_then(|b| b.best_ask())
            .unwrap_or(mid + 1);
        app.price_input = format!("{:.2}", app.rules.price_to_f64(ask));
        let lot = app.rules.lot_size(&selected);
        for i in 0..orders {
            app.qty_input = (lot * (i as i64 + 1)).to_string();
            app.submit_order();
        }
        app.roll_day(); // release T+1 so the position is sellable
    }
    if with_backtest {
        app.run_backtest_now();
    }
    app
}
