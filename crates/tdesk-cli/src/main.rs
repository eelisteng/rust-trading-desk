//! Headless end-to-end demonstration. Verifies the whole domain chain without
//! opening a window: data -> risk -> matching -> settlement -> backtest.

use std::time::Instant;

use tdesk_core::{
    run_backtest, rules_for, Market, MatchingEngine, OrderRequest, OrderType, Side, SmaCrossStrategy,
    Symbol,
};
use tdesk_data::{CsvSource, MarketDataSource, SyntheticSource};

const CASH: i64 = 100_000_000; // ¥1,000,000.00
const SEED: u64 = 20_261_112;

fn section(title: &str) {
    println!("\n{}", "=".repeat(72));
    println!("  {title}");
    println!("{}", "=".repeat(72));
}

fn main() {
    println!("tdesk-cli · Rust 模拟交易内核端到端演示（无 GUI / 完全离线）");
    demo_market_rules();
    demo_data();
    demo_trading();
    demo_backtest();
    demo_csv();
    benchmark();
    section("结论");
    println!("全部断言通过：撮合、风控、T+1 结算、账户守恒、回测与数据适配器均正常。");
    println!("提示：本系统为模拟盘，行情为确定性合成数据，不构成任何投资建议。");
}

fn demo_market_rules() {
    section("1 · 多市场规则");
    for market in Market::ALL {
        let r = rules_for(market);
        let (lo, hi) = r.limit_range(10_000);
        let band = if r.price_limit_ratio().is_some() {
            format!("[{}, {}]", r.format_price(lo), r.format_price(hi))
        } else {
            "无限制".to_string()
        };
        println!(
            "{:<12} {:<5} {:<4} T+{} 涨跌色: {:<8} 最小单位: {:<4} 涨跌停: {}  {}",
            r.market().label(),
            r.currency(),
            r.currency_symbol(),
            r.settlement_lag_days(),
            if r.rise_is_red() { "红涨绿跌" } else { "绿涨红跌" },
            r.lot_size("600519.SH"),
            band,
            r.fees().label()
        );
    }
    let cn = rules_for(Market::AShare);
    assert_eq!(cn.limit_range(10_000), (9_000, 11_000));
    assert_eq!(rules_for(Market::Us).price_limit_ratio(), None);
    assert_eq!(rules_for(Market::Hk).lot_size("01810.HK"), 200);
    assert_eq!(cn.format_money(123_456_789), "¥1,234,567.89");
}

fn demo_data() {
    section("2 · 确定性行情");
    let source = SyntheticSource::new(Market::AShare, SEED);
    let symbol = "600519.SH";
    let a = source.history(symbol, 160);
    let b = source.history(symbol, 160);
    assert_eq!(a, b, "同种子行情必须完全可复现");
    assert_eq!(a.len(), 160);
    assert!(a.iter().all(|c| c.high >= c.open.max(c.close) && c.low <= c.open.min(c.close)));
    let c = source.history("AAPL", 160);
    assert!(c.is_empty(), "A 股数据源不应包含美股标的");
    println!(
        "标的数 {} | {} 共 {} 根日K | 首 {} 末 {} | 复现校验通过",
        source.instruments().len(),
        symbol,
        a.len(),
        rules_for(Market::AShare).format_price(a[0].close),
        rules_for(Market::AShare).format_price(a[a.len() - 1].close)
    );
}

fn demo_trading() {
    section("3 · 风控 → 撮合 → 结算（A 股 T+1 全链路）");
    let rules = rules_for(Market::AShare);
    let source = SyntheticSource::new(Market::AShare, SEED);
    let symbol = Symbol::new("600519.SH");
    let price = source.instrument(symbol.as_str()).unwrap().seed_price;

    let mut engine = MatchingEngine::new(rules.clone(), CASH);
    for inst in source.instruments() {
        engine.register_symbol(&inst.symbol, inst.seed_price);
    }
    engine.refresh_liquidity(symbol.as_str(), price);
    println!(
        "初始 {}",
        rules.format_money(engine.account.equity(rules.as_ref()))
    );
    println!(
        "盘口 买一 {} / 卖一 {}",
        rules.format_price(engine.book(symbol.as_str()).unwrap().best_bid().unwrap()),
        rules.format_price(engine.book(symbol.as_str()).unwrap().best_ask().unwrap())
    );

    // A rejected order must not touch cash or the book.
    let bad = OrderRequest {
        symbol: symbol.clone(),
        side: Side::Buy,
        kind: OrderType::Limit,
        price: price + 50_000,
        qty: 100,
    };
    let err = engine.submit_user_order(bad).unwrap_err();
    println!("风控拒绝 · {}", err.message());
    assert_eq!(engine.account.cash, CASH);

    let equity_before = engine.account.equity(rules.as_ref());
    let buy_id = engine
        .submit_user_order(OrderRequest {
            symbol: symbol.clone(),
            side: Side::Buy,
            kind: OrderType::Limit,
            price: price + 5,
            qty: 100,
        })
        .unwrap();
    assert_eq!(engine.user_trades().count(), 1);
    assert_eq!(engine.account.available(symbol.as_str()), 0);
    assert_eq!(
        equity_before - engine.account.equity(rules.as_ref()),
        engine.account.total_fees,
        "成交前后权益差应恰好等于手续费"
    );
    println!(
        "买入成交 · {} @ {} × 100，手续费 {}，T+1 可卖 {}",
        engine.trades[0].id,
        rules.format_price(engine.trades[0].price),
        rules.format_money(engine.trades[0].fee),
        engine.account.available(symbol.as_str())
    );

    let sell_err = engine
        .submit_user_order(OrderRequest {
            symbol: symbol.clone(),
            side: Side::Sell,
            kind: OrderType::Limit,
            price,
            qty: 100,
        })
        .unwrap_err();
    println!("T+1 拦截 · {}", sell_err.message());

    let resting_id = engine
        .submit_user_order(OrderRequest {
            symbol: symbol.clone(),
            side: Side::Buy,
            kind: OrderType::Limit,
            price: price - 1_000,
            qty: 100,
        })
        .unwrap();
    engine.cancel_order(resting_id).unwrap();
    println!("挂单撤单 · 订单 #{} 已撤销", resting_id);

    engine.roll_day();
    assert_eq!(engine.account.available(symbol.as_str()), 100);
    engine.submit_maker_order(symbol.as_str(), Side::Buy, price, 100);
    engine
        .submit_user_order(OrderRequest {
            symbol: symbol.clone(),
            side: Side::Sell,
            kind: OrderType::Limit,
            price,
            qty: 100,
        })
        .unwrap();
    assert_eq!(engine.account.position(symbol.as_str()).unwrap().qty, 0);
    println!(
        "换日后卖出 · 平仓完成 | 已实现盈亏 {} | 累计手续费 {} | 总权益 {}",
        rules.format_signed_money(engine.account.realized_pnl()),
        rules.format_money(engine.account.total_fees),
        rules.format_money(engine.account.equity(rules.as_ref()))
    );
    assert!(engine.orders[&buy_id].status == tdesk_core::OrderStatus::Filled);
}

fn demo_backtest() {
    section("4 · 策略回测（信号 T 日收盘，成交 T+1 开盘）");
    let rules = rules_for(Market::AShare);
    let source = SyntheticSource::new(Market::AShare, SEED);
    let symbol = Symbol::new("600519.SH");
    let bars = source.history(symbol.as_str(), 180);
    let mut strategy = SmaCrossStrategy::new(5, 20);
    let result = run_backtest(&bars, &symbol, rules.clone(), &mut strategy);
    assert_eq!(result.equity_curve.len(), bars.len());
    assert!(result.metrics.total_return_pct.is_finite());
    assert!(result.metrics.max_drawdown_pct >= 0.0);
    println!(
        "策略 {} | K线 {} | 成交 {} 笔 | 平仓 {} 次",
        result.strategy,
        bars.len(),
        result.metrics.trade_count,
        result.metrics.closed_trades
    );
    println!(
        "期初 {} → 期末 {} | 总收益 {:.2}% | 最大回撤 {:.2}% | 胜率 {:.1}% | 夏普 {:.2}",
        rules.format_money(result.metrics.initial_equity),
        rules.format_money(result.metrics.final_equity),
        result.metrics.total_return_pct,
        result.metrics.max_drawdown_pct,
        result.metrics.win_rate_pct,
        result.metrics.sharpe
    );
}

fn demo_csv() {
    section("5 · CSV 行情适配器");
    let csv = "\
# ts,open,high,low,close,volume
ts,open,high,low,close,volume
1700000000,10.00,10.50,9.80,10.20,1000
1700086400,10.20,10.80,10.10,10.60,1200
1700172800,10.60,10.90,10.30,10.40,900
";
    let mut src = CsvSource::from_csv(csv, "TEST", "测试标的", Market::AShare).unwrap();
    assert_eq!(src.candles("TEST").len(), 3);
    let tick = src.next_tick("TEST");
    println!(
        "解析 {} 根 | 回放 tick 最新价 {} 前收 {} 量 {}",
        src.candles("TEST").len(),
        rules_for(Market::AShare).format_price(tick.last),
        rules_for(Market::AShare).format_price(tick.prev_close),
        tick.volume
    );
}

fn benchmark() {
    section("6 · 无头性能基准（1000 tick，含盘口刷新）");
    let mut source = SyntheticSource::new(Market::Us, 7);
    let mut engine = MatchingEngine::new(rules_for(Market::Us), CASH);
    engine.register_symbol("AAPL", 22_750);
    let start = Instant::now();
    for _ in 0..1000 {
        let tick = source.next_tick("AAPL");
        engine.set_price("AAPL", tick.last);
        engine.refresh_liquidity("AAPL", tick.last);
    }
    let elapsed = start.elapsed();
    let total_ms = elapsed.as_secs_f64() * 1000.0;
    let ms_per_tick = total_ms / 1000.0;
    println!(
        "1000 tick 耗时 {:.2} ms → {:.3} ms/tick | 订单 {} 笔",
        total_ms,
        ms_per_tick,
        engine.orders.len()
    );
    assert!(
        ms_per_tick < 16.0,
        "单 tick 处理应低于一帧预算（16ms）"
    );
}
