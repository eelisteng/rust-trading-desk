//! Market data port. UI/CLI depend only on this trait.

use tdesk_core::{Candle, Market, Price, Tick};

pub const BASE_TS: i64 = 1_700_000_000; // 2023-11-14, arbitrary but stable
pub const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Clone, Debug, PartialEq)]
pub struct Instrument {
    pub symbol: String,
    pub name: String,
    pub market: Market,
    /// Reference last price in ticks.
    pub seed_price: Price,
    /// Daily volatility used by the synthetic generator (e.g. 0.018 = 1.8%).
    pub volatility: f64,
    /// Daily drift (e.g. 0.0004).
    pub drift: f64,
}

impl Instrument {
    pub fn label(&self) -> String {
        format!("{} {}", self.symbol, self.name)
    }
}

pub trait MarketDataSource: Send {
    fn instruments(&self) -> &[Instrument];

    /// Deterministic historical daily bars for backtesting. Calling this twice
    /// with the same arguments must yield identical data.
    fn history(&self, symbol: &str, bars: usize) -> Vec<Candle>;

    /// Live bars for charting (the last element is the forming candle).
    fn candles(&self, symbol: &str) -> &[Candle];

    /// Advance one tick and return the new quote.
    fn next_tick(&mut self, symbol: &str) -> Tick;

    fn instrument(&self, symbol: &str) -> Option<&Instrument> {
        self.instruments().iter().find(|i| i.symbol == symbol)
    }

    fn symbols(&self) -> Vec<String> {
        self.instruments().iter().map(|i| i.symbol.clone()).collect()
    }

    fn prev_close(&self, symbol: &str) -> Price {
        self.candles(symbol)
            .iter()
            .rev()
            .nth(1)
            .map(|c| c.close)
            .unwrap_or(1)
    }
}
