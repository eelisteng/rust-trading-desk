//! Pluggable quantitative strategies.
//!
//! Each strategy implements both [`Strategy`] (for historical replay) and
//! [`LiveStrategy`] (for the paper/live trading loop) from the *same* internal
//! price buffer, so a strategy tuned on history behaves identically in live
//! trading. Strategies only emit an intent (`Buy`/`Sell`); position sizing is
//! delegated to [`crate::live::size_buy`] / [`crate::live::size_sell`] so cash,
//! lot size and participation are handled in one place.

use crate::backtest::{BarContext, Signal, Strategy};
use crate::live::{size_buy, size_sell, LiveContext, LiveStrategy};
use crate::market::MarketRules;
use crate::types::{Money, Price, Qty, Side};

/// Inputs a strategy needs to turn a signal into a sized order.
struct DecideInput {
    position: Qty,
    available: Qty,
    cash: Money,
    price_minor: Money,
    participation: f64,
    lot: Qty,
}

fn sma(closes: &[Price], period: usize) -> Option<f64> {
    if closes.len() < period || period == 0 {
        return None;
    }
    let sum: i64 = closes[closes.len() - period..].iter().sum();
    Some(sum as f64 / period as f64)
}

fn mean_std(closes: &[Price]) -> Option<(f64, f64)> {
    if closes.is_empty() {
        return None;
    }
    let n = closes.len() as f64;
    let mean = closes.iter().map(|&c| c as f64).sum::<f64>() / n;
    let variance = closes.iter().map(|&c| (c as f64 - mean).powi(2)).sum::<f64>() / n;
    Some((mean, variance.sqrt()))
}

// ---------------------------------------------------------------------------
// 双均线金叉/死叉
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DualMaStrategy {
    pub fast: usize,
    pub slow: usize,
    pub participation: f64,
    closes: Vec<Price>,
}

impl DualMaStrategy {
    pub fn new(fast: usize, slow: usize) -> Self {
        Self {
            fast,
            slow,
            participation: 0.30,
            closes: Vec::new(),
        }
    }

    fn decide(&self, input: &DecideInput) -> Option<Signal> {
        let h = &self.closes;
        if h.len() < self.slow + 2 {
            return None;
        }
        let fast_now = sma(h, self.fast)?;
        let slow_now = sma(h, self.slow)?;
        let prev = &h[..h.len() - 1];
        let fast_prev = sma(prev, self.fast)?;
        let slow_prev = sma(prev, self.slow)?;
        let golden = fast_prev <= slow_prev && fast_now > slow_now;
        let death = fast_prev >= slow_prev && fast_now < slow_now;
        if golden && input.position == 0 {
            let qty = size_buy(input.cash, input.price_minor, input.participation, input.lot);
            if qty > 0 {
                return Some(Signal {
                    side: Side::Buy,
                    qty,
                });
            }
        }
        if death && input.available > 0 {
            return Some(Signal {
                side: Side::Sell,
                qty: size_sell(input.available),
            });
        }
        None
    }
}

impl Strategy for DualMaStrategy {
    fn name(&self) -> &str {
        "双均线 金叉/死叉"
    }

    fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal> {
        self.closes.push(ctx.candle.close);
        let rules: &dyn MarketRules = ctx.rules;
        let input = DecideInput {
            position: ctx.position,
            available: ctx.available,
            cash: ctx.cash,
            price_minor: rules.price_to_minor(ctx.candle.close),
            participation: self.participation,
            lot: rules.lot_size(ctx.symbol).max(1),
        };
        self.decide(&input)
    }

    fn reset(&mut self) {
        self.closes.clear();
    }
}

impl LiveStrategy for DualMaStrategy {
    fn name(&self) -> &str {
        "双均线 金叉/死叉"
    }

    fn on_quote(&mut self, ctx: &LiveContext<'_>) -> Option<Signal> {
        self.closes.push(ctx.price);
        let input = DecideInput {
            position: ctx.position,
            available: ctx.available,
            cash: ctx.cash,
            price_minor: ctx.rules.price_to_minor(ctx.price),
            participation: self.participation,
            lot: ctx.rules.lot_size(ctx.symbol).max(1),
        };
        self.decide(&input)
    }

    fn reset(&mut self) {
        self.closes.clear();
    }
}

// ---------------------------------------------------------------------------
// N 日高低点突破（动量/趋势）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BreakoutStrategy {
    pub lookback: usize,
    pub participation: f64,
    closes: Vec<Price>,
}

impl BreakoutStrategy {
    pub fn new(lookback: usize) -> Self {
        Self {
            lookback: lookback.max(2),
            participation: 0.30,
            closes: Vec::new(),
        }
    }

    fn decide(&self, input: &DecideInput) -> Option<Signal> {
        let h = &self.closes;
        if h.len() < self.lookback + 2 {
            return None;
        }
        let window = &h[..h.len() - 1];
        let start = window.len().saturating_sub(self.lookback);
        let ref_window = &window[start..];
        let high = *ref_window.iter().max().unwrap();
        let low = *ref_window.iter().min().unwrap();
        let current = h[h.len() - 1];
        if current > high && input.position == 0 {
            let qty = size_buy(input.cash, input.price_minor, input.participation, input.lot);
            if qty > 0 {
                return Some(Signal {
                    side: Side::Buy,
                    qty,
                });
            }
        }
        if current < low && input.available > 0 {
            return Some(Signal {
                side: Side::Sell,
                qty: size_sell(input.available),
            });
        }
        None
    }
}

impl Strategy for BreakoutStrategy {
    fn name(&self) -> &str {
        "N日突破（动量）"
    }

    fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal> {
        self.closes.push(ctx.candle.close);
        let rules: &dyn MarketRules = ctx.rules;
        let input = DecideInput {
            position: ctx.position,
            available: ctx.available,
            cash: ctx.cash,
            price_minor: rules.price_to_minor(ctx.candle.close),
            participation: self.participation,
            lot: rules.lot_size(ctx.symbol).max(1),
        };
        self.decide(&input)
    }

    fn reset(&mut self) {
        self.closes.clear();
    }
}

impl LiveStrategy for BreakoutStrategy {
    fn name(&self) -> &str {
        "N日突破（动量）"
    }

    fn on_quote(&mut self, ctx: &LiveContext<'_>) -> Option<Signal> {
        self.closes.push(ctx.price);
        let input = DecideInput {
            position: ctx.position,
            available: ctx.available,
            cash: ctx.cash,
            price_minor: ctx.rules.price_to_minor(ctx.price),
            participation: self.participation,
            lot: ctx.rules.lot_size(ctx.symbol).max(1),
        };
        self.decide(&input)
    }

    fn reset(&mut self) {
        self.closes.clear();
    }
}

// ---------------------------------------------------------------------------
// 均值回归（布林/标准差）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MeanReversionStrategy {
    pub window: usize,
    pub entry_z: f64,
    pub participation: f64,
    closes: Vec<Price>,
}

impl MeanReversionStrategy {
    pub fn new(window: usize, entry_z: f64) -> Self {
        Self {
            window: window.max(2),
            entry_z: entry_z.max(0.1),
            participation: 0.30,
            closes: Vec::new(),
        }
    }

    fn decide(&self, input: &DecideInput) -> Option<Signal> {
        let h = &self.closes;
        if h.len() < self.window {
            return None;
        }
        let window = &h[h.len() - self.window..];
        let (mean, std) = mean_std(window)?;
        if std <= 0.0 {
            return None;
        }
        let current = h[h.len() - 1];
        let z = (current as f64 - mean) / std;
        // 价格显著低于均值 -> 预期回归向上，买入；显著高于均值 -> 卖出。
        if z <= -self.entry_z && input.position == 0 {
            let qty = size_buy(input.cash, input.price_minor, input.participation, input.lot);
            if qty > 0 {
                return Some(Signal {
                    side: Side::Buy,
                    qty,
                });
            }
        }
        if z >= self.entry_z && input.available > 0 {
            return Some(Signal {
                side: Side::Sell,
                qty: size_sell(input.available),
            });
        }
        None
    }
}

impl Strategy for MeanReversionStrategy {
    fn name(&self) -> &str {
        "均值回归（布林）"
    }

    fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal> {
        self.closes.push(ctx.candle.close);
        let rules: &dyn MarketRules = ctx.rules;
        let input = DecideInput {
            position: ctx.position,
            available: ctx.available,
            cash: ctx.cash,
            price_minor: rules.price_to_minor(ctx.candle.close),
            participation: self.participation,
            lot: rules.lot_size(ctx.symbol).max(1),
        };
        self.decide(&input)
    }

    fn reset(&mut self) {
        self.closes.clear();
    }
}

impl LiveStrategy for MeanReversionStrategy {
    fn name(&self) -> &str {
        "均值回归（布林）"
    }

    fn on_quote(&mut self, ctx: &LiveContext<'_>) -> Option<Signal> {
        self.closes.push(ctx.price);
        let input = DecideInput {
            position: ctx.position,
            available: ctx.available,
            cash: ctx.cash,
            price_minor: ctx.rules.price_to_minor(ctx.price),
            participation: self.participation,
            lot: ctx.rules.lot_size(ctx.symbol).max(1),
        };
        self.decide(&input)
    }

    fn reset(&mut self) {
        self.closes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::run_backtest;
    use crate::market::rules_for;
    use crate::types::{Candle, Market, Price, Symbol, Timestamp};

    fn candles(closes: &[Price]) -> Vec<Candle> {
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

    #[test]
    fn dual_ma_backtest_trades_on_up_then_down_trend() {
        let mut closes = Vec::new();
        let mut p = 12_000;
        for _ in 0..30 {
            p -= 60;
            closes.push(p);
        }
        for _ in 0..30 {
            p += 80;
            closes.push(p);
        }
        for _ in 0..30 {
            p -= 110;
            closes.push(p);
        }
        let bars = candles(&closes);
        let symbol = Symbol::new("TEST");
        let mut strat = DualMaStrategy::new(5, 20);
        let result = run_backtest(&bars, &symbol, rules_for(Market::Us), &mut strat);
        assert!(
            result.metrics.trade_count >= 2,
            "应至少出现一次买入与一次卖出"
        );
    }

    #[test]
    fn breakout_and_mean_reversion_run_without_panicking() {
        let bars = candles(&[10_000_i64; 60]);
        let symbol = Symbol::new("TEST");
        let mut breakout = BreakoutStrategy::new(20);
        let r = run_backtest(&bars, &symbol, rules_for(Market::AShare), &mut breakout);
        assert!(r.metrics.total_return_pct.is_finite());
        let mut revert = MeanReversionStrategy::new(20, 2.0);
        let r = run_backtest(&bars, &symbol, rules_for(Market::AShare), &mut revert);
        assert!(r.metrics.total_return_pct.is_finite());
    }
}

