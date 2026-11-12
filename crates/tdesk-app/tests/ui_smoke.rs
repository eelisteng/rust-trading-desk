//! Headless UI smoke tests: egui can lay out and paint every panel without a
//! native window, which keeps the UI verifiable in CI.

use egui::{Context, RawInput};
use tdesk_app::{scripted_smoke_test, theme::setup_fonts, TradingApp};
use tdesk_core::Market;

fn context() -> Context {
    let ctx = Context::default();
    setup_fonts(&ctx);
    ctx
}

#[test]
fn renders_one_complete_frame_without_a_window() {
    let ctx = context();
    let mut app = TradingApp::new(Market::AShare);
    let output = ctx.run(RawInput::default(), |ctx| app.render(ctx));
    assert!(!output.shapes.is_empty(), "a frame should produce paint shapes");
    assert_eq!(app.market, Market::AShare);
}

#[test]
fn scripted_interaction_covers_order_backtest_and_market_switch() {
    let ctx = context();
    let mut app = TradingApp::new(Market::AShare);
    scripted_smoke_test(&mut app, &ctx);
    assert_eq!(app.market, Market::Hk);
}

#[test]
fn every_market_renders_with_its_own_palette() {
    let ctx = context();
    for market in Market::ALL {
        let mut app = TradingApp::new(market);
        let output = ctx.run(RawInput::default(), |ctx| app.render(ctx));
        assert!(!output.shapes.is_empty(), "{market} should render");
    }
}
