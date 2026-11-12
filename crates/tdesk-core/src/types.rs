//! Domain value types.
//!
//! Money is always expressed in the currency's minor unit (cents) and price in
//! integer ticks. Floating point is only used for display and ratios.

use std::fmt;

/// Price as an integer number of ticks (tick size is defined by [`crate::market::MarketRules`]).
pub type Price = i64;
/// Share quantity, always a positive integer for orders.
pub type Qty = i64;
/// Amount of money in currency minor units (cents).
pub type Money = i64;
/// Unix timestamp in seconds.
pub type Timestamp = i64;
pub type OrderId = u64;
pub type TradeId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Market {
    AShare,
    Us,
    Hk,
}

impl Market {
    pub const ALL: [Market; 3] = [Market::AShare, Market::Us, Market::Hk];

    pub fn code(self) -> &'static str {
        match self {
            Market::AShare => "CN",
            Market::Us => "US",
            Market::Hk => "HK",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Market::AShare => "A股 · 沪深",
            Market::Us => "美股",
            Market::Hk => "港股",
        }
    }
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(pub String);

impl Symbol {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Side::Buy => "买入",
            Side::Sell => "卖出",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrderType {
    Limit,
    /// First version behaves as IOC: any unfilled remainder is cancelled.
    Market,
}

impl OrderType {
    pub fn label(self) -> &'static str {
        match self {
            OrderType::Limit => "限价",
            OrderType::Market => "市价",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrderStatus {
    Pending,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

impl OrderStatus {
    pub fn label(self) -> &'static str {
        match self {
            OrderStatus::Pending => "已挂起",
            OrderStatus::PartiallyFilled => "部分成交",
            OrderStatus::Filled => "全部成交",
            OrderStatus::Cancelled => "已撤销",
            OrderStatus::Rejected => "已拒绝",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, OrderStatus::Pending | OrderStatus::PartiallyFilled)
    }
}

/// Who owns an order. Maker orders are synthetic liquidity and never settle
/// against the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    User,
    Maker,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    pub id: OrderId,
    pub symbol: Symbol,
    pub side: Side,
    pub kind: OrderType,
    /// Ticks. Ignored for market orders.
    pub price: Price,
    pub qty: Qty,
    pub filled_qty: Qty,
    pub avg_fill_price: Price,
    pub status: OrderStatus,
    pub owner: Owner,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub reject_reason: Option<String>,
}

impl Order {
    pub fn remaining(&self) -> Qty {
        self.qty - self.filled_qty
    }

    pub fn is_active(&self) -> bool {
        self.status.is_active()
    }

    pub fn fill_ratio(&self) -> f64 {
        if self.qty == 0 {
            0.0
        } else {
            self.filled_qty as f64 / self.qty as f64
        }
    }
}

/// A request created by the UI / CLI before it enters the matching engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderRequest {
    pub symbol: Symbol,
    pub side: Side,
    pub kind: OrderType,
    pub price: Price,
    pub qty: Qty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trade {
    pub id: TradeId,
    pub ts: Timestamp,
    pub symbol: Symbol,
    pub price: Price,
    pub qty: Qty,
    pub taker_order_id: OrderId,
    pub maker_order_id: OrderId,
    pub taker_side: Side,
    /// Side from the account's point of view. `None` for maker-to-maker trades.
    pub user_side: Option<Side>,
    pub fee: Money,
}

impl Trade {
    pub fn notional_minor(&self, tick_minor: Money) -> Money {
        self.price * tick_minor * self.qty
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candle {
    pub ts: Timestamp,
    pub open: Price,
    pub high: Price,
    pub low: Price,
    pub close: Price,
    pub volume: Qty,
}

impl Candle {
    pub fn new(ts: Timestamp, open: Price, volume: Qty) -> Self {
        Self {
            ts,
            open,
            high: open,
            low: open,
            close: open,
            volume,
        }
    }

    pub fn update(&mut self, price: Price, volume: Qty) {
        self.high = self.high.max(price);
        self.low = self.low.min(price);
        self.close = price;
        self.volume += volume;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    pub ts: Timestamp,
    pub last: Price,
    pub bid: Price,
    pub ask: Price,
    pub volume: Qty,
    pub prev_close: Price,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    pub symbol: Symbol,
    pub last: Price,
    pub prev_close: Price,
    pub change: Price,
    pub change_pct: f64,
    pub high: Price,
    pub low: Price,
    pub volume: Qty,
}

impl Quote {
    pub fn from_candles(symbol: Symbol, candles: &[Candle], prev_close: Price) -> Self {
        let last = candles.last().map(|c| c.close).unwrap_or(prev_close);
        let (high, low, volume) = candles.iter().fold((last, last, 0_i64), |(h, l, v), c| {
            (h.max(c.high), l.min(c.low), v + c.volume)
        });
        let change = last - prev_close;
        let change_pct = if prev_close == 0 {
            0.0
        } else {
            change as f64 / prev_close as f64 * 100.0
        };
        Self {
            symbol,
            last,
            prev_close,
            change,
            change_pct,
            high,
            low,
            volume,
        }
    }
}

/// Percentage helper used by UI and metrics.
pub fn pct(numerator: f64, denominator: f64) -> f64 {
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator * 100.0
    }
}
