//! Strategy replay. Signals are produced on bar `i` close and filled on bar
//! `i+1` open, so strategies can never observe future prices.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::account::Account;
use crate::market::MarketRules;
use crate::types::{pct, Candle, Money, Qty, Side, Symbol, Trade};

pub const DEFAULT_INITIAL_CASH: Money = 100_000_000; // ¥1,000,000.00

#[derive(Clone, Copy, Debug)]
pub struct BacktestConfig {
    pub initial_cash: Money,
    /// Fraction of cash a buy signal is allowed to deploy.
    pub participation: f64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            initial_cash: DEFAULT_INITIAL_CASH,
            participation: 0.30,
        }
    }
}

pub struct BarContext<'a> {
    pub index: usize,
    pub symbol: &'a str,
    pub candle: &'a Candle,
    /// All bars up to and including `index` — never beyond.
    pub history: &'a [Candle],
    pub position: Qty,
    pub available: Qty,
    pub cash: Money,
    pub equity: Money,
    pub rules: &'a dyn MarketRules,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signal {
    pub side: Side,
    pub qty: Qty,
}

pub trait Strategy {
    fn name(&self) -> &str;
    fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal>;
    fn reset(&mut self) {}
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    pub initial_equity: Money,
    pub final_equity: Money,
    pub total_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub win_rate_pct: f64,
    pub trade_count: usize,
    pub closed_trades: usize,
    pub sharpe: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BacktestResult {
    pub strategy: String,
    pub equity_curve: Vec<Money>,
    pub trades: Vec<Trade>,
    pub metrics: Metrics,
}

#[derive(Clone, Debug)]
pub struct SmaCrossStrategy {
    pub fast: usize,
    pub slow: usize,
}

impl SmaCrossStrategy {
    pub fn new(fast: usize, slow: usize) -> Self {
        Self { fast, slow }
    }
}

fn sma(candles: &[Candle], period: usize) -> Option<f64> {
    if candles.len() < period || period == 0 {
        return None;
    }
    let sum: i64 = candles[candles.len() - period..]
        .iter()
        .map(|c| c.close)
        .sum();
    Some(sum as f64 / period as f64)
}

impl Strategy for SmaCrossStrategy {
    fn name(&self) -> &str {
        "SMA 金叉/死叉"
    }

    fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal> {
        let h = ctx.history;
        if h.len() < self.slow + 2 {
            return None;
        }
        let (fast, slow) = (sma(h, self.fast)?, sma(h, self.slow)?);
        let prev = &h[..h.len() - 1];
        let (prev_fast, prev_slow) = (sma(prev, self.fast)?, sma(prev, self.slow)?);
        let golden_cross = prev_fast <= prev_slow && fast > slow;
        let death_cross = prev_fast >= prev_slow && fast < slow;

        if golden_cross && ctx.position == 0 {
            let lot = ctx.rules.lot_size(ctx.symbol).max(1);
            let budget = ctx.cash as f64 * 0.30;
            let price = ctx.rules.price_to_minor(ctx.candle.close).max(1) as f64;
            let raw = (budget / price) as Qty;
            let qty = (raw / lot) * lot;
            if qty > 0 {
                return Some(Signal {
                    side: Side::Buy,
                    qty,
                });
            }
        }
        if death_cross && ctx.available > 0 {
            return Some(Signal {
                side: Side::Sell,
                qty: ctx.available,
            });
        }
        None
    }
}

pub fn run_backtest(
    candles: &[Candle],
    symbol: &Symbol,
    rules: Arc<dyn MarketRules>,
    strategy: &mut dyn Strategy,
) -> BacktestResult {
    run_backtest_with(candles, symbol, rules, strategy, BacktestConfig::default())
}

pub fn run_backtest_with(
    candles: &[Candle],
    symbol: &Symbol,
    rules: Arc<dyn MarketRules>,
    strategy: &mut dyn Strategy,
    config: BacktestConfig,
) -> BacktestResult {
    strategy.reset();
    let mut account = Account::new(config.initial_cash);
    let mut pending: Option<Signal> = None;
    let mut equity_curve = Vec::with_capacity(candles.len());
    let mut trades: Vec<Trade> = Vec::new();
    let mut wins = 0usize;
    let mut closed = 0usize;
    let lot = rules.lot_size(symbol.as_str()).max(1);

    for (index, bar) in candles.iter().enumerate() {
        // 1) Execute the signal generated on the *previous* bar at this open.
        if let Some(signal) = pending.take() {
            let price = bar.open;
            let price_minor = rules.price_to_minor(price);
            let mut qty = (signal.qty / lot) * lot;
            if signal.side == Side::Buy && price_minor > 0 {
                let affordable = ((account.cash as f64 * config.participation) / price_minor as f64)
                    as Qty;
                qty = qty.min((affordable / lot) * lot);
            }
            if signal.side == Side::Sell {
                qty = qty.min(account.available(symbol.as_str()));
            }
            if qty > 0 {
                let fee = rules
                    .fees()
                    .calculate(signal.side, price_minor * qty);
                let before = account.realized_pnl();
                let trade = Trade {
                    id: trades.len() as u64 + 1,
                    ts: bar.ts,
                    symbol: symbol.clone(),
                    price,
                    qty,
                    taker_order_id: 0,
                    maker_order_id: 0,
                    taker_side: signal.side,
                    user_side: Some(signal.side),
                    fee,
                };
                account.apply_fill(&trade, rules.as_ref());
                if signal.side == Side::Sell {
                    closed += 1;
                    if account.realized_pnl() > before {
                        wins += 1;
                    }
                }
                trades.push(trade);
            }
        }

        // 2) Mark to market at this bar's close.
        let mut prices = BTreeMap::new();
        prices.insert(symbol.as_str().to_owned(), bar.close);
        account.mark_to_market(&prices);
        equity_curve.push(account.equity(rules.as_ref()));

        // 3) Strategies only see history up to this bar.
        let position = account.position(symbol.as_str()).map(|p| p.qty).unwrap_or(0);
        let ctx = BarContext {
            index,
            symbol: symbol.as_str(),
            candle: bar,
            history: &candles[..=index],
            position,
            available: account.available(symbol.as_str()),
            cash: account.cash,
            equity: account.equity(rules.as_ref()),
            rules: rules.as_ref(),
        };
        pending = strategy.on_bar(&ctx);

        // 4) Settle T+1 for the next bar.
        account.roll_day(rules.as_ref());
    }

    let metrics = compute_metrics(&equity_curve, config.initial_cash, wins, closed, trades.len());
    BacktestResult {
        strategy: strategy.name().to_owned(),
        equity_curve,
        trades,
        metrics,
    }
}

fn compute_metrics(
    equity: &[Money],
    initial: Money,
    wins: usize,
    closed: usize,
    trade_count: usize,
) -> Metrics {
    let final_equity = equity.last().copied().unwrap_or(initial);
    let mut peak = initial;
    let mut max_dd = 0.0_f64;
    let mut returns = Vec::with_capacity(equity.len().saturating_sub(1));
    for (i, &value) in equity.iter().enumerate() {
        peak = peak.max(value);
        if peak > 0 {
            max_dd = max_dd.max((peak - value) as f64 / peak as f64 * 100.0);
        }
        if i > 0 {
            let prev = equity[i - 1];
            if prev != 0 {
                returns.push((value - prev) as f64 / prev as f64);
            }
        }
    }
    let mean = if returns.is_empty() {
        0.0
    } else {
        returns.iter().sum::<f64>() / returns.len() as f64
    };
    let variance = if returns.len() < 2 {
        0.0
    } else {
        returns
            .iter()
            .map(|r| (r - mean) * (r - mean))
            .sum::<f64>()
            / (returns.len() - 1) as f64
    };
    let std = variance.sqrt();
    let sharpe = if std > 0.0 {
        mean / std * (252.0_f64).sqrt()
    } else {
        0.0
    };
    Metrics {
        initial_equity: initial,
        final_equity,
        total_return_pct: pct((final_equity - initial) as f64, initial as f64),
        max_drawdown_pct: max_dd,
        win_rate_pct: if closed == 0 {
            0.0
        } else {
            wins as f64 / closed as f64 * 100.0
        },
        trade_count,
        closed_trades: closed,
        sharpe,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::rules_for;
    use crate::types::{Market, Price, Timestamp};

    fn bars(closes: &[Price]) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &close)| Candle {
                ts: 1_700_000_000 + i as Timestamp * 86_400,
                open: close,
                high: close + 10,
                low: close - 10,
                close,
                volume: 10_000,
            })
            .collect()
    }

    struct AlwaysBuyOnFirstBar;
    impl Strategy for AlwaysBuyOnFirstBar {
        fn name(&self) -> &str {
            "test-buy"
        }
        fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal> {
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

    #[test]
    fn signal_on_bar_i_fills_at_bar_i_plus_1_open() {
        let candles = bars(&[10_000, 11_000, 12_000]);
        let symbol = Symbol::new("TEST");
        let mut strategy = AlwaysBuyOnFirstBar;
        let result = run_backtest(&candles, &symbol, rules_for(Market::AShare), &mut strategy);
        assert_eq!(result.trades.len(), 1);
        assert_eq!(result.trades[0].price, 11_000); // not bar 0 open
        assert_eq!(result.trades[0].ts, candles[1].ts);
    }

    #[test]
    fn flat_series_produces_no_trades_and_zero_drawdown() {
        let candles = bars(&vec![10_000; 40]);
        let symbol = Symbol::new("TEST");
        let mut strategy = SmaCrossStrategy::new(5, 20);
        let result = run_backtest(&candles, &symbol, rules_for(Market::AShare), &mut strategy);
        assert!(result.trades.is_empty());
        assert_eq!(result.equity_curve.len(), 40);
        assert_eq!(result.metrics.max_drawdown_pct, 0.0);
        assert_eq!(result.metrics.total_return_pct, 0.0);
    }

    #[test]
    fn uptrend_generates_trades_and_positive_return() {
        let mut closes = vec![10_000_i64; 22];
        for i in 0..40 {
            closes.push(10_000 + i * 150);
        }
        let candles = bars(&closes);
        let symbol = Symbol::new("TEST");
        let mut strategy = SmaCrossStrategy::new(5, 20);
        let result = run_backtest(&candles, &symbol, rules_for(Market::AShare), &mut strategy);
        assert!(!result.trades.is_empty(), "golden cross should trigger a buy");
        assert!(result.metrics.total_return_pct > 0.0);
        assert!(result.metrics.max_drawdown_pct >= 0.0);
        assert!(result.metrics.sharpe.is_finite());
    }

    #[test]
    fn metrics_handle_empty_equity_curve() {
        let m = compute_metrics(&[], 100_000, 0, 0, 0);
        assert_eq!(m.final_equity, 100_000);
        assert_eq!(m.total_return_pct, 0.0);
        assert_eq!(m.sharpe, 0.0);
    }
}
