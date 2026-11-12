//! Adapter-level integration tests (determinism, live tick invariants,
//! rollover detection and CSV round-trip).

use std::collections::BTreeSet;

use tdesk_core::{rules_for, Candle, Market};
use tdesk_data::{CsvSource, MarketDataSource, SyntheticSource, TICKS_PER_BAR};

#[test]
fn every_market_and_symbol_reproduces_identical_history() {
    for market in Market::ALL {
        let a = SyntheticSource::new(market, 2026);
        let b = SyntheticSource::new(market, 2026);
        for inst in a.instruments() {
            assert_eq!(
                a.history(&inst.symbol, 150),
                b.history(&inst.symbol, 150),
                "{} 行情不可复现",
                inst.symbol
            );
            assert_eq!(
                a.history(&inst.symbol, 150),
                a.history(&inst.symbol, 150),
                "重复调用应返回相同序列"
            );
        }
    }
}

#[test]
fn live_ticks_maintain_ohlc_and_volume_invariants() {
    let mut source = SyntheticSource::new(Market::AShare, 42);
    let symbol = "600519.SH";
    let initial_len = source.candles(symbol).len();
    for _ in 0..(TICKS_PER_BAR * 3) {
        let tick = source.next_tick(symbol);
        assert!(tick.last > 0);
        assert!(tick.bid < tick.ask);
        assert!(tick.prev_close > 0);
        let forming = source.candles(symbol).last().unwrap();
        assert!(forming.high >= forming.open.max(forming.close));
        assert!(forming.low <= forming.open.min(forming.close));
        assert!(forming.volume >= 0);
    }
    assert!(
        source.candles(symbol).len() >= initial_len + 2,
        "跨越多个交易日后应新增已完成 K 线"
    );
}

#[test]
fn day_rollover_is_observable_through_prev_close() {
    let mut source = SyntheticSource::new(Market::Hk, 9);
    let symbol = "00700.HK";
    let mut closes = BTreeSet::new();
    for _ in 0..(TICKS_PER_BAR * 4) {
        closes.insert(source.next_tick(symbol).prev_close);
    }
    assert!(
        closes.len() >= 3,
        "前收盘价应随交易日推进而变化，实际 {} 个",
        closes.len()
    );
}

#[test]
fn synthetic_history_survives_a_csv_round_trip() {
    let market = Market::AShare;
    let rules = rules_for(market);
    let source = SyntheticSource::new(market, 77);
    let symbol = "300750.SZ";
    let bars = source.history(symbol, 60);
    let mut csv = String::from("ts,open,high,low,close,volume\n");
    for c in &bars {
        csv.push_str(&format!(
            "{},{:.2},{:.2},{:.2},{:.2},{}\n",
            c.ts,
            rules.price_to_f64(c.open),
            rules.price_to_f64(c.high),
            rules.price_to_f64(c.low),
            rules.price_to_f64(c.close),
            c.volume
        ));
    }
    let parsed = CsvSource::from_csv(&csv, symbol, "宁德时代", market).unwrap();
    let got: &[Candle] = parsed.candles(symbol);
    assert_eq!(got.len(), bars.len());
    for (a, b) in bars.iter().zip(got.iter()) {
        assert_eq!(a, b, "CSV 往返应无损");
    }
    assert_eq!(parsed.history(symbol, 10).len(), 10);
}

#[test]
fn source_ports_expose_consistent_metadata() {
    for market in Market::ALL {
        let source = SyntheticSource::new(market, 1);
        assert_eq!(source.market(), market);
        assert_eq!(source.symbols().len(), source.instruments().len());
        assert!(source.instrument("NO_SUCH").is_none());
        assert!(source.history("NO_SUCH", 10).is_empty());
        assert!(source.prev_close(&source.symbols()[0]) > 0);
    }
}
