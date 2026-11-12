//! Deterministic synthetic market: daily GBM bars plus an intraday tick stream.

use std::collections::BTreeMap;

use tdesk_core::{rules_for, Candle, Market, Price, Tick};

use crate::rng::{fnv1a, Rng};
use crate::source::{Instrument, MarketDataSource, BASE_TS, SECONDS_PER_DAY};

pub const HISTORY_BARS: usize = 120;
pub const TICKS_PER_BAR: u32 = 24;
const MAX_LIVE_BARS: usize = 240;

pub fn instrument_list(market: Market) -> Vec<Instrument> {
    match market {
        Market::AShare => vec![
            Instrument { symbol: "600519.SH".into(), name: "贵州茅台".into(), market, seed_price: 168_500, volatility: 0.016, drift: 0.0003 },
            Instrument { symbol: "000001.SZ".into(), name: "平安银行".into(), market, seed_price: 1_180, volatility: 0.014, drift: 0.0002 },
            Instrument { symbol: "300750.SZ".into(), name: "宁德时代".into(), market, seed_price: 24_500, volatility: 0.023, drift: 0.0005 },
            Instrument { symbol: "601318.SH".into(), name: "中国平安".into(), market, seed_price: 5_230, volatility: 0.015, drift: 0.0002 },
            Instrument { symbol: "000858.SZ".into(), name: "五粮液".into(), market, seed_price: 13_800, volatility: 0.018, drift: 0.0003 },
        ],
        Market::Us => vec![
            Instrument { symbol: "AAPL".into(), name: "Apple".into(), market, seed_price: 22_750, volatility: 0.014, drift: 0.0004 },
            Instrument { symbol: "MSFT".into(), name: "Microsoft".into(), market, seed_price: 42_300, volatility: 0.013, drift: 0.0004 },
            Instrument { symbol: "NVDA".into(), name: "NVIDIA".into(), market, seed_price: 13_800, volatility: 0.028, drift: 0.0007 },
            Instrument { symbol: "TSLA".into(), name: "Tesla".into(), market, seed_price: 24_800, volatility: 0.030, drift: 0.0003 },
            Instrument { symbol: "AMZN".into(), name: "Amazon".into(), market, seed_price: 18_600, volatility: 0.017, drift: 0.0004 },
        ],
        Market::Hk => vec![
            Instrument { symbol: "00700.HK".into(), name: "腾讯控股".into(), market, seed_price: 41_200, volatility: 0.019, drift: 0.0003 },
            Instrument { symbol: "09988.HK".into(), name: "阿里巴巴-W".into(), market, seed_price: 8_900, volatility: 0.022, drift: 0.0002 },
            Instrument { symbol: "03690.HK".into(), name: "美团-W".into(), market, seed_price: 17_800, volatility: 0.025, drift: 0.0002 },
            Instrument { symbol: "01810.HK".into(), name: "小米集团-W".into(), market, seed_price: 2_750, volatility: 0.021, drift: 0.0005 },
        ],
    }
}

/// Generate `bars` daily candles from a stable per-symbol seed. Prices are
/// rescaled so the final close lands on the instrument's reference price.
pub fn generate_history(inst: &Instrument, seed: u64, bars: usize) -> Vec<Candle> {
    if bars == 0 {
        return Vec::new();
    }
    let mut rng = Rng::new(fnv1a(&inst.symbol) ^ seed);
    let mut price = (inst.seed_price as f64 * 0.86).round().max(1.0) as Price;
    let mut out = Vec::with_capacity(bars);
    for i in 0..bars {
        let shock = rng.normal() * inst.volatility;
        let open = price;
        let close = (open as f64 * (1.0 + inst.drift + shock))
            .round()
            .max(1.0) as Price;
        let wick = (open as f64 * inst.volatility * rng.range(0.1, 0.8))
            .round()
            .max(1.0) as Price;
        let high = open.max(close) + wick;
        let low = (open.min(close) - wick).max(1);
        let volume = rng.int_range(80_000, 420_000);
        out.push(Candle {
            ts: BASE_TS + i as i64 * SECONDS_PER_DAY,
            open,
            high,
            low,
            close,
            volume,
        });
        price = close;
    }
    let last_close = out.last().map(|c| c.close).unwrap_or(1).max(1);
    let factor = inst.seed_price as f64 / last_close as f64;
    for c in out.iter_mut() {
        c.open = (c.open as f64 * factor).round().max(1.0) as Price;
        c.high = (c.high as f64 * factor).round().max(1.0) as Price;
        c.low = (c.low as f64 * factor).round().max(1.0) as Price;
        c.close = (c.close as f64 * factor).round().max(1.0) as Price;
        c.high = c.high.max(c.open).max(c.close);
        c.low = c.low.min(c.open).min(c.close).max(1);
    }
    if let Some(last) = out.last_mut() {
        last.close = inst.seed_price;
        last.high = last.high.max(last.close);
        last.low = last.low.min(last.close).max(1);
    }
    out
}

#[derive(Clone, Debug)]
pub struct SymbolState {
    pub rng: Rng,
    pub last: Price,
    pub prev_close: Price,
    pub candles: Vec<Candle>,
    pub ticks_in_bar: u32,
    pub ts: i64,
}

pub struct SyntheticSource {
    market: Market,
    seed: u64,
    instruments: Vec<Instrument>,
    states: BTreeMap<String, SymbolState>,
}

impl SyntheticSource {
    pub fn new(market: Market, seed: u64) -> Self {
        let instruments = instrument_list(market);
        let mut states = BTreeMap::new();
        for inst in &instruments {
            let mut candles = generate_history(inst, seed, HISTORY_BARS);
            let prev_close = candles.last().map(|c| c.close).unwrap_or(inst.seed_price);
            let ts = candles.last().map(|c| c.ts).unwrap_or(BASE_TS) + SECONDS_PER_DAY;
            candles.push(Candle::new(ts, prev_close, 0));
            states.insert(
                inst.symbol.clone(),
                SymbolState {
                    rng: Rng::new(fnv1a(&inst.symbol) ^ seed ^ 0xA5A5_1234_5678_9ABC),
                    last: prev_close,
                    prev_close,
                    candles,
                    ticks_in_bar: 0,
                    ts,
                },
            );
        }
        Self {
            market,
            seed,
            instruments,
            states,
        }
    }

    pub fn market(&self) -> Market {
        self.market
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn state(&self, symbol: &str) -> Option<&SymbolState> {
        self.states.get(symbol)
    }
}

impl MarketDataSource for SyntheticSource {
    fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    fn history(&self, symbol: &str, bars: usize) -> Vec<Candle> {
        match self.instruments.iter().find(|i| i.symbol == symbol) {
            Some(inst) => generate_history(inst, self.seed, bars),
            None => Vec::new(),
        }
    }

    fn candles(&self, symbol: &str) -> &[Candle] {
        static EMPTY: &[Candle] = &[];
        self.states
            .get(symbol)
            .map(|s| s.candles.as_slice())
            .unwrap_or(EMPTY)
    }

    fn next_tick(&mut self, symbol: &str) -> Tick {
        let inst = self
            .instruments
            .iter()
            .find(|i| i.symbol == symbol)
            .cloned();
        let Some(inst) = inst else {
            return Tick {
                ts: BASE_TS,
                last: 1,
                bid: 1,
                ask: 1,
                volume: 0,
                prev_close: 1,
            };
        };
        let Some(st) = self.states.get_mut(symbol) else {
            return Tick {
                ts: BASE_TS,
                last: inst.seed_price,
                bid: inst.seed_price,
                ask: inst.seed_price,
                volume: 0,
                prev_close: inst.seed_price,
            };
        };

        let per_tick_vol = inst.volatility / (TICKS_PER_BAR as f64).sqrt();
        let shock = st.rng.normal() * per_tick_vol;
        let drift = inst.drift / TICKS_PER_BAR as f64;
        let raw = (st.last as f64 * (1.0 + drift + shock)).round().max(1.0) as Price;
        let (lower, upper) = rules_for(inst.market).limit_range(st.prev_close);
        let last = raw.clamp(lower, upper);
        st.last = last;

        let spread = (inst.seed_price as f64 * 0.0002).round().max(1.0) as Price;
        let bid = (last - spread).max(1);
        let ask = last + spread;
        let volume = st.rng.int_range(100, 3_000);
        if let Some(forming) = st.candles.last_mut() {
            forming.update(last, volume);
        }
        st.ticks_in_bar += 1;

        if st.ticks_in_bar >= TICKS_PER_BAR {
            let closed = *st.candles.last().expect("forming candle");
            st.prev_close = closed.close;
            st.ts = closed.ts + SECONDS_PER_DAY;
            st.candles.push(Candle::new(st.ts, closed.close, 0));
            st.ticks_in_bar = 0;
            if st.candles.len() > MAX_LIVE_BARS {
                st.candles.remove(0);
            }
        }

        Tick {
            ts: st.ts,
            last,
            bid,
            ask,
            volume,
            prev_close: st.prev_close,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_deterministic_and_ohlc_consistent() {
        let inst = instrument_list(Market::AShare)[0].clone();
        let a = generate_history(&inst, 2026, 120);
        let b = generate_history(&inst, 2026, 120);
        assert_eq!(a, b);
        assert_eq!(a.len(), 120);
        for c in &a {
            assert!(c.high >= c.open.max(c.close));
            assert!(c.low <= c.open.min(c.close));
            assert!(c.low > 0);
            assert!(c.volume > 0);
            assert!(c.close > 0);
        }
        assert_eq!(a.last().unwrap().close, inst.seed_price);
    }

    #[test]
    fn different_seeds_produce_different_paths() {
        let inst = instrument_list(Market::Us)[0].clone();
        let a = generate_history(&inst, 1, 60);
        let b = generate_history(&inst, 2, 60);
        assert_ne!(a, b);
    }

    #[test]
    fn all_markets_have_instruments() {
        for market in Market::ALL {
            let list = instrument_list(market);
            assert!(!list.is_empty());
            assert!(list.iter().all(|i| i.market == market && i.seed_price > 0));
        }
    }
}
