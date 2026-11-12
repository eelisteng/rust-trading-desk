//! CSV adapter: `ts,open,high,low,close,volume` with decimal prices.
//! Header lines, blank lines and `#` comments are ignored.

use std::collections::BTreeMap;

use tdesk_core::{rules_for, Candle, Market, Price, Qty, Tick};

use crate::source::{Instrument, MarketDataSource, BASE_TS, SECONDS_PER_DAY};

pub struct CsvSource {
    instruments: Vec<Instrument>,
    bars: BTreeMap<String, Vec<Candle>>,
    cursors: BTreeMap<String, usize>,
}

impl CsvSource {
    pub fn from_csv(
        csv: &str,
        symbol: &str,
        name: &str,
        market: Market,
    ) -> Result<Self, String> {
        let rules = rules_for(market);
        let mut candles = Vec::new();
        for (line_no, raw) in csv.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            let parts: Vec<&str> = line.split(',').map(str::trim).collect();
            if parts.len() < 6 {
                return Err(format!("第 {} 行字段不足 6 个: {}", line_no + 1, line));
            }
            // Skip a header row (first column is not a number).
            if parts[0].parse::<i64>().is_err() {
                continue;
            }
            let parse_i64 = |s: &str, field: &str| -> Result<i64, String> {
                s.parse::<i64>()
                    .map_err(|_| format!("第 {} 行 {} 无法解析为整数: {}", line_no + 1, field, s))
            };
            let parse_price = |s: &str, field: &str| -> Result<Price, String> {
                let v = s.parse::<f64>().map_err(|_| {
                    format!("第 {} 行 {} 无法解析为价格: {}", line_no + 1, field, s)
                })?;
                if v <= 0.0 {
                    return Err(format!("第 {} 行 {} 价格必须为正", line_no + 1, field));
                }
                Ok(rules.price_from_f64(v))
            };
            let candle = Candle {
                ts: parse_i64(parts[0], "ts")?,
                open: parse_price(parts[1], "open")?,
                high: parse_price(parts[2], "high")?,
                low: parse_price(parts[3], "low")?,
                close: parse_price(parts[4], "close")?,
                volume: parse_i64(parts[5], "volume")? as Qty,
            };
            if candle.high < candle.open.max(candle.close)
                || candle.low > candle.open.min(candle.close)
                || candle.low <= 0
            {
                return Err(format!("第 {} 行 OHLC 关系不合法", line_no + 1));
            }
            candles.push(candle);
        }
        if candles.is_empty() {
            return Err("未解析到有效数据行".to_string());
        }
        let seed_price = candles.last().map(|c| c.close).unwrap_or(1);
        let instrument = Instrument {
            symbol: symbol.to_owned(),
            name: name.to_owned(),
            market,
            seed_price,
            volatility: 0.0,
            drift: 0.0,
        };
        let mut bars = BTreeMap::new();
        let mut cursors = BTreeMap::new();
        bars.insert(symbol.to_owned(), candles);
        cursors.insert(symbol.to_owned(), 0usize);
        Ok(Self {
            instruments: vec![instrument],
            bars,
            cursors,
        })
    }
}

impl MarketDataSource for CsvSource {
    fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    fn history(&self, symbol: &str, bars: usize) -> Vec<Candle> {
        let all = self.bars.get(symbol).map(Vec::as_slice).unwrap_or(&[]);
        let start = all.len().saturating_sub(bars);
        all[start..].to_vec()
    }

    fn candles(&self, symbol: &str) -> &[Candle] {
        static EMPTY: &[Candle] = &[];
        self.bars.get(symbol).map(Vec::as_slice).unwrap_or(EMPTY)
    }

    fn next_tick(&mut self, symbol: &str) -> Tick {
        let empty = Tick {
            ts: BASE_TS,
            last: 1,
            bid: 1,
            ask: 1,
            volume: 0,
            prev_close: 1,
        };
        let Some(bars) = self.bars.get(symbol) else {
            return empty;
        };
        if bars.is_empty() {
            return empty;
        }
        let cursor = self.cursors.entry(symbol.to_owned()).or_insert(0);
        let idx = (*cursor).min(bars.len() - 1);
        let bar = bars[idx];
        let prev_close = if idx > 0 {
            bars[idx - 1].close
        } else {
            bar.open
        };
        *cursor = if idx + 1 >= bars.len() { 0 } else { idx + 1 };
        let spread = (bar.close as f64 * 0.0002).round().max(1.0) as Price;
        Tick {
            ts: bar.ts + SECONDS_PER_DAY,
            last: bar.close,
            bid: (bar.close - spread).max(1),
            ask: bar.close + spread,
            volume: bar.volume,
            prev_close,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "\
# sample data
ts,open,high,low,close,volume
1700000000,10.00,10.50,9.80,10.20,1000
1700086400,10.20,10.80,10.10,10.60,1200
1700172800,10.60,10.90,10.30,10.40,900
";

    #[test]
    fn parses_header_comments_and_decimal_prices() {
        let src = CsvSource::from_csv(CSV, "TEST", "测试", Market::AShare).unwrap();
        let bars = src.candles("TEST");
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].open, 1000);
        assert_eq!(bars[1].close, 1060);
        assert_eq!(src.instrument("TEST").unwrap().seed_price, 1040);
    }

    #[test]
    fn history_returns_last_n_bars() {
        let src = CsvSource::from_csv(CSV, "TEST", "测试", Market::AShare).unwrap();
        assert_eq!(src.history("TEST", 2).len(), 2);
        assert_eq!(src.history("TEST", 99).len(), 3);
    }

    #[test]
    fn replay_wraps_and_reports_prev_close() {
        let mut src = CsvSource::from_csv(CSV, "TEST", "测试", Market::AShare).unwrap();
        let t0 = src.next_tick("TEST");
        assert_eq!(t0.last, 1020);
        assert_eq!(t0.prev_close, 1000);
        let t1 = src.next_tick("TEST");
        assert_eq!(t1.last, 1060);
        assert_eq!(t1.prev_close, 1020);
        let _ = src.next_tick("TEST");
        assert_eq!(src.next_tick("TEST").last, 1020); // wrapped
    }

    #[test]
    fn invalid_rows_are_rejected() {
        assert!(CsvSource::from_csv("ts,open,high,low,close,volume\n", "T", "T", Market::Us).is_err());
        assert!(CsvSource::from_csv("1,10,9,11,10,100\n", "T", "T", Market::Us).is_err());
        assert!(CsvSource::from_csv("1,10,10,10\n", "T", "T", Market::Us).is_err());
    }
}
