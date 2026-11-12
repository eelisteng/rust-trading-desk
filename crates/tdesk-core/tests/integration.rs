//! Cross-module integration tests: risk gate -> order book -> settlement ->
//! T+1 roll -> mark-to-market, for every supported market.

use tdesk_core::*;

const CASH: Money = 100_000_000;

fn request(side: Side, kind: OrderType, price: Price, qty: Qty) -> OrderRequest {
    OrderRequest {
        symbol: Symbol::new("TEST"),
        side,
        kind,
        price,
        qty,
    }
}

#[test]
fn full_day_cycle_is_consistent_for_every_market() {
    for market in Market::ALL {
        let rules = rules_for(market);
        let lot = rules.lot_size("TEST");
        let mut engine = MatchingEngine::new(rules.clone(), CASH);
        engine.register_symbol("TEST", 10_000);

        // Buy from a resting maker ask.
        engine.submit_maker_order("TEST", Side::Sell, 10_000, lot * 2);
        engine
            .submit_user_order(request(Side::Buy, OrderType::Limit, 10_000, lot))
            .unwrap();
        assert_eq!(engine.account.position("TEST").unwrap().qty, lot);
        let expected_available = if rules.settlement_lag_days() == 0 {
            lot
        } else {
            0
        };
        assert_eq!(
            engine.account.available("TEST"),
            expected_available,
            "{} T+N 可卖数量错误",
            market.label()
        );

        let cash_after_buy = engine.account.cash;
        engine.roll_day();
        assert_eq!(engine.account.available("TEST"), lot);

        // Sell into a resting maker bid. The bid is placed one tick below the
        // leftover maker ask so that it is not consumed by a maker-maker cross.
        engine.submit_maker_order("TEST", Side::Buy, 9_999, lot);
        engine
            .submit_user_order(request(Side::Sell, OrderType::Limit, 9_999, lot))
            .unwrap();
        assert_eq!(engine.account.position("TEST").unwrap().qty, 0);
        assert_eq!(engine.account.market_value(rules.as_ref()), 0);
        assert_eq!(
            engine.account.equity(rules.as_ref()),
            engine.account.cash
        );
        assert!(engine.account.cash > cash_after_buy);
        assert_eq!(engine.user_trades().count(), 2);
    }
}

#[test]
fn risk_matrix_rejects_without_mutating_state() {
    let rules = rules_for(Market::AShare);
    let mut engine = MatchingEngine::new(rules.clone(), CASH);
    engine.register_symbol("TEST", 10_000);
    let baseline_cash = engine.account.cash;
    let baseline_book_depth = engine.book("TEST").unwrap().depth();

    let cases = [
        ("数量为 0", request(Side::Buy, OrderType::Limit, 10_000, 0)),
        ("非整手", request(Side::Buy, OrderType::Limit, 10_000, 150)),
        ("价格为 0", request(Side::Buy, OrderType::Limit, 0, 100)),
        ("涨停外", request(Side::Buy, OrderType::Limit, 12_000, 100)),
        ("跌停外", request(Side::Buy, OrderType::Limit, 8_000, 100)),
        ("无持仓卖出", request(Side::Sell, OrderType::Limit, 10_000, 100)),
    ];
    for (label, req) in cases {
        let err = engine
            .submit_user_order(req)
            .unwrap_err_or_panic(label);
        assert!(
            matches!(err, EngineError::Risk(_)),
            "{label} 应由风控拒绝，实际 {err:?}"
        );
        assert_eq!(engine.account.cash, baseline_cash, "{label} 不应改变现金");
        assert_eq!(
            engine.book("TEST").unwrap().depth(),
            baseline_book_depth,
            "{label} 不应改变订单簿"
        );
    }

    let mut tiny = MatchingEngine::new(rules, 1_000);
    tiny.register_symbol("TEST", 10_000);
    let err = tiny
        .submit_user_order(request(Side::Buy, OrderType::Limit, 10_000, 100))
        .unwrap_err();
    assert!(matches!(
        err,
        EngineError::Risk(RiskError::InsufficientCash { .. })
    ));
    assert_eq!(tiny.account.cash, 1_000);
}

trait UnwrapErrOrPanic<T> {
    fn unwrap_err_or_panic(self, label: &str) -> EngineError;
}

impl<T> UnwrapErrOrPanic<T> for Result<T, EngineError> {
    fn unwrap_err_or_panic(self, label: &str) -> EngineError {
        match self {
            Ok(_) => panic!("{label} 本应被拒绝，却下单成功"),
            Err(e) => e,
        }
    }
}

#[test]
fn price_time_priority_handles_multiple_levels_and_partial_fills() {
    let rules = rules_for(Market::AShare);
    let mut engine = MatchingEngine::new(rules.clone(), CASH);
    engine.register_symbol("TEST", 10_000);

    let a = engine.submit_maker_order("TEST", Side::Sell, 10_100, 100);
    let b = engine.submit_maker_order("TEST", Side::Sell, 10_000, 100);
    let c = engine.submit_maker_order("TEST", Side::Sell, 10_000, 100);
    let id = engine
        .submit_user_order(request(Side::Buy, OrderType::Limit, 10_100, 300))
        .unwrap();

    let prices: Vec<Price> = engine.user_trades().map(|t| t.price).collect();
    assert_eq!(prices, vec![10_000, 10_000, 10_100]);
    let ids: Vec<OrderId> = engine.user_trades().map(|t| t.maker_order_id).collect();
    assert_eq!(ids, vec![b, c, a], "同一价位必须按时间优先");
    assert_eq!(engine.orders[&id].status, OrderStatus::Filled);
    assert_eq!(engine.orders[&id].avg_fill_price, 10_033); // (10000*2 + 10100)/3
    assert_eq!(engine.book("TEST").unwrap().depth(), 0);
}

#[test]
fn market_order_reserves_cash_conservatively_and_fills_ioc() {
    let rules = rules_for(Market::AShare);
    let mut engine = MatchingEngine::new(rules.clone(), CASH);
    engine.register_symbol("TEST", 10_000);
    engine.refresh_liquidity("TEST", 10_000);
    let id = engine
        .submit_user_order(request(Side::Buy, OrderType::Market, 0, 300))
        .unwrap();
    assert_eq!(engine.orders[&id].status, OrderStatus::Filled);
    let total: Qty = engine.user_trades().map(|t| t.qty).sum();
    assert_eq!(total, 300);
    assert!(engine.account.cash < CASH);
    let paid: Money = engine
        .user_trades()
        .map(|t| rules.price_to_minor(t.price) * t.qty)
        .sum();
    let mark = 10_050;
    engine.set_price("TEST", mark);
    let expected = CASH - paid - engine.account.total_fees
        + total * rules.price_to_minor(mark);
    assert_eq!(engine.account.equity(rules.as_ref()), expected);
}

#[test]
fn backtest_does_not_use_future_prices() {
    struct BuyAtFirstBar;
    impl Strategy for BuyAtFirstBar {
        fn name(&self) -> &str {
            "no-lookahead"
        }
        fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal> {
            assert_eq!(ctx.history.len(), ctx.index + 1, "策略不可看到未来 K 线");
            if ctx.index == 0 {
                Some(Signal {
                    side: Side::Buy,
                    qty: 100,
                })
            } else {
                None
            }
        }
    }
    let candle = |ts: i64, price: Price| Candle {
        ts,
        open: price,
        high: price,
        low: price,
        close: price,
        volume: 1_000,
    };
    let bars = vec![
        candle(1, 10_000),
        candle(2, 11_000),
        candle(3, 12_000),
    ];
    let result = run_backtest(
        &bars,
        &Symbol::new("TEST"),
        rules_for(Market::AShare),
        &mut BuyAtFirstBar,
    );
    assert_eq!(result.trades.len(), 1);
    assert_eq!(result.trades[0].price, 11_000);
    assert!(result.metrics.total_return_pct > 0.0);
}

#[test]
fn fee_differences_are_applied_by_market() {
    for market in Market::ALL {
        let rules = rules_for(market);
        let fee = rules.fees().calculate(Side::Sell, 1_000_000);
        match market {
            Market::AShare => assert!(fee > 500),
            Market::Us => assert_eq!(fee, 0),
            Market::Hk => assert!(fee > 300),
        }
        assert!(rules.fees().calculate(Side::Buy, 1_000_000) <= fee);
    }
}

#[test]
fn equity_identity_holds_after_marking_to_market() {
    let rules = rules_for(Market::AShare);
    let mut engine = MatchingEngine::new(rules.clone(), CASH);
    engine.register_symbol("TEST", 10_000);
    engine.submit_maker_order("TEST", Side::Sell, 10_000, 100);
    engine
        .submit_user_order(request(Side::Buy, OrderType::Limit, 10_000, 100))
        .unwrap();
    for price in [9_800, 10_000, 10_250, 11_000] {
        engine.set_price("TEST", price);
        let pos = engine.account.position("TEST").unwrap();
        assert_eq!(
            engine.account.equity(rules.as_ref()),
            engine.account.cash + pos.qty * rules.price_to_minor(price)
        );
    }
}
