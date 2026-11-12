//! Live (paper or real) trading loop.
//!
//! Ties a [`MarketDataSource`](crate::types)-style quote feed, a
//! [`LiveStrategy`] and a [`Broker`] together: each quote is handed to the
//! strategy, and any signal becomes a sized order submitted to the broker. The
//! loop is identical for paper and real trading — only the [`Broker`] changes.

use std::sync::Arc;

use crate::backtest::Signal;
use crate::broker::{Broker, BrokerError, BrokerOrderId, BrokerPosition};
use crate::market::MarketRules;
use crate::types::{Money, OrderRequest, OrderType, Price, Qty, Side, Symbol, Tick};

/// View of the market + account handed to a strategy on every quote.
pub struct LiveContext<'a> {
    pub symbol: &'a str,
    pub price: Price,
    pub prev_close: Price,
    pub position: Qty,
    pub available: Qty,
    pub cash: Money,
    pub rules: &'a dyn MarketRules,
}

/// A strategy that reacts to live quotes. Implemented by every strategy in
/// [`crate::strategies`]; the same implementation is reused for backtesting via
/// the [`crate::backtest::Strategy`] trait.
pub trait LiveStrategy: Send {
    fn name(&self) -> &str;
    fn on_quote(&mut self, ctx: &LiveContext<'_>) -> Option<Signal>;
    fn reset(&mut self) {}
}

/// What the loop did in response to a quote.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveAction {
    /// Strategy produced no order.
    NoSignal,
    /// An order was accepted by the broker.
    OrderSubmitted(BrokerOrderId),
    /// The broker rejected the order.
    OrderRejected(BrokerError),
    /// A signal fired but sizing produced zero shares (e.g. insufficient cash).
    Insufficient,
}

/// Size a buy so it never exceeds `participation` of cash, rounded to a lot.
pub fn size_buy(cash: Money, price_minor: Money, participation: f64, lot: Qty) -> Qty {
    if price_minor <= 0 || lot <= 0 {
        return 0;
    }
    let budget = (cash as f64 * participation.max(0.0)).max(0.0);
    let raw = (budget / price_minor as f64) as Qty;
    (raw / lot) * lot
}

/// Size a sell as the full currently-available quantity.
pub fn size_sell(available: Qty) -> Qty {
    available.max(0)
}

/// Drives strategy signals into a broker, one quote at a time.
pub struct LiveTrader<B: Broker> {
    pub broker: B,
    pub rules: Arc<dyn MarketRules>,
    pub strategy: Box<dyn LiveStrategy>,
    pub participation: f64,
}

impl<B: Broker> LiveTrader<B> {
    pub fn new(
        rules: Arc<dyn MarketRules>,
        broker: B,
        strategy: Box<dyn LiveStrategy>,
        participation: f64,
    ) -> Self {
        Self {
            broker,
            rules,
            strategy,
            participation,
        }
    }

    /// Feed one quote. Updates the broker's price, asks the strategy for a
    /// signal, and if one fires submits a (marketable) order. Returns the
    /// action taken so callers can log / persist it.
    pub fn on_quote(&mut self, symbol: &str, tick: &Tick) -> Vec<LiveAction> {
        let price = tick.last;
        let prev_close = tick.prev_close;
        self.broker.set_price(symbol, price);
        let position: BrokerPosition = self.broker.position(symbol);
        let ctx = LiveContext {
            symbol,
            price,
            prev_close,
            position: position.qty,
            available: position.available,
            cash: self.broker.cash(),
            rules: self.rules.as_ref(),
        };
        let sig = self.strategy.on_quote(&ctx);
        let mut actions = Vec::new();
        match sig {
            None => actions.push(LiveAction::NoSignal),
            Some(signal) => {
                let lot = self.rules.lot_size(symbol).max(1);
                let qty = match signal.side {
                    Side::Buy => size_buy(
                        self.broker.cash(),
                        self.rules.price_to_minor(price),
                        self.participation,
                        lot,
                    ),
                    Side::Sell => position.available,
                };
                if qty <= 0 {
                    actions.push(LiveAction::Insufficient);
                } else {
                    let req = OrderRequest {
                        symbol: Symbol::new(symbol),
                        side: signal.side,
                        kind: OrderType::Market,
                        price,
                        qty,
                    };
                    match self.broker.submit(req) {
                        Ok(id) => actions.push(LiveAction::OrderSubmitted(id)),
                        Err(e) => actions.push(LiveAction::OrderRejected(e)),
                    }
                }
            }
        }
        actions
    }

    /// Advance the session day (T+1 release, etc.).
    pub fn roll_day(&mut self) {
        self.broker.roll_day();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::PaperBroker;
    use crate::market::rules_for;
    use crate::strategies::DualMaStrategy;
    use crate::types::Market;

    fn tick(last: Price, prev_close: Price) -> Tick {
        Tick {
            ts: 0,
            last,
            bid: last - 1,
            ask: last + 1,
            volume: 100,
            prev_close,
        }
    }

    #[test]
    fn live_paper_loop_buys_on_uptrend_and_sells_on_downtrend() {
        let rules = rules_for(Market::Us); // T+0 so sells can happen same day
        let mut trader = LiveTrader::new(
            rules.clone(),
            PaperBroker::new(rules.clone(), 100_000_000),
            Box::new(DualMaStrategy::new(5, 20)),
            0.30,
        );
        let mut price = 12_000;
        // Sustained decline so the slow MA is firmly below the fast MA only after
        // a real upturn; no premature signal during the down-leg.
        for _ in 0..30 {
            price -= 60;
            trader.on_quote("AAPL", &tick(price, 12_000));
        }
        assert_eq!(
            trader.broker.position("AAPL").qty,
            0,
            "no position should open during a downtrend"
        );
        // Upturn: fast MA crosses above slow MA -> golden cross -> buy.
        for _ in 0..30 {
            price += 80;
            trader.on_quote("AAPL", &tick(price, 12_000));
        }
        assert!(
            trader.broker.position("AAPL").qty > 0,
            "golden cross should have opened a position"
        );
        // Sharp decline: fast MA crosses back below slow MA -> death cross -> sell.
        for _ in 0..30 {
            price -= 110;
            trader.on_quote("AAPL", &tick(price, 12_000));
        }
        assert_eq!(
            trader.broker.position("AAPL").qty,
            0,
            "death cross should have closed the position"
        );
    }

    #[test]
    fn sizing_respects_cash_and_lot() {
        let lot = 100;
        assert_eq!(size_buy(1_000_000, 1_000, 0.30, lot), 300);
        assert_eq!(size_buy(50_000, 1_000, 0.30, lot), 0); // 15000 < 1 lot
        assert_eq!(size_sell(250), 250);
    }
}
