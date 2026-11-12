//! Pluggable broker abstraction.
//!
//! The existing in-process matching engine becomes the **paper** backend
//! ([`PaperBroker`]); a real exchange/broker connects by implementing the same
//! [`Broker`] trait (see [`LiveBroker`] for the intentional stub). Nothing in
//! the strategy or trading-loop code depends on *which* broker is wired in, so
//! live trading can be developed and tested entirely against the paper backend.

use std::sync::Arc;

use crate::account::Account;
use crate::engine::MatchingEngine;
use crate::error::EngineError;
use crate::market::MarketRules;
use crate::types::{
    Money, OrderRequest, OrderStatus, OrderType, Price, Qty, Side,
};

/// Order id returned by a [`Broker`], kept distinct from the engine's internal
/// id space so adapters can map to a remote order id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BrokerOrderId(pub u64);

/// A position snapshot as seen by the trading loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerPosition {
    pub symbol: String,
    pub qty: Qty,
    pub available: Qty,
}

/// A live (resting or partially filled) order snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerOrderView {
    pub id: BrokerOrderId,
    pub symbol: String,
    pub side: Side,
    pub kind: OrderType,
    pub price: Price,
    pub qty: Qty,
    pub filled_qty: Qty,
    pub status: OrderStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BrokerError {
    /// Rejected/cancelled by the underlying engine (paper backend).
    Engine(EngineError),
    /// The live adapter is not wired up yet.
    NotConfigured(&'static str),
    /// Transport/session failure for a real broker.
    Connection(String),
}

impl BrokerError {
    pub fn message(&self) -> String {
        match self {
            BrokerError::Engine(e) => e.message(),
            BrokerError::NotConfigured(reason) => format!("实盘适配器未配置：{reason}"),
            BrokerError::Connection(msg) => format!("连接失败：{msg}"),
        }
    }
}

/// The seam between the trading loop and whatever actually executes orders.
///
/// All methods are safe to call every tick; a real adapter would batch network
/// calls internally. `set_price` / `roll_day` have no-op defaults because a live
/// feed pushes prices itself and session roll is handled by the exchange.
pub trait Broker {
    fn name(&self) -> &str;

    fn submit(&mut self, req: OrderRequest) -> Result<BrokerOrderId, BrokerError>;

    fn cancel(&mut self, id: BrokerOrderId) -> Result<(), BrokerError>;

    fn last_price(&self, symbol: &str) -> Option<Price>;

    fn cash(&self) -> Money;

    fn position(&self, symbol: &str) -> BrokerPosition;

    fn positions(&self) -> Vec<BrokerPosition>;

    fn pending_orders(&self) -> Vec<BrokerOrderView>;

    fn set_price(&mut self, _symbol: &str, _price: Price) {}

    fn roll_day(&mut self) {}
}

/// Paper-trading backend: the existing deterministic matching engine.
///
/// This *is* the simulated book the UI and CLI already use, exposed through the
/// [`Broker`] trait so the live trading loop can treat paper and real trading
/// uniformly. Liquidity is refreshed around each new price so marketable orders
/// fill immediately.
pub struct PaperBroker {
    engine: MatchingEngine,
}

impl PaperBroker {
    pub fn new(rules: Arc<dyn MarketRules>, initial_cash: Money) -> Self {
        Self {
            engine: MatchingEngine::new(rules, initial_cash),
        }
    }

    /// Direct access for the UI / CLI that still drive the engine by hand.
    pub fn engine(&self) -> &MatchingEngine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut MatchingEngine {
        &mut self.engine
    }
}

impl Broker for PaperBroker {
    fn name(&self) -> &str {
        "纸面模拟（内置撮合引擎）"
    }

    fn submit(&mut self, req: OrderRequest) -> Result<BrokerOrderId, BrokerError> {
        self.engine
            .submit_user_order(req)
            .map(BrokerOrderId)
            .map_err(BrokerError::Engine)
    }

    fn cancel(&mut self, id: BrokerOrderId) -> Result<(), BrokerError> {
        self.engine.cancel_order(id.0).map_err(BrokerError::Engine)
    }

    fn last_price(&self, symbol: &str) -> Option<Price> {
        self.engine.last_prices.get(symbol).copied()
    }

    fn cash(&self) -> Money {
        self.engine.account.cash
    }

    fn position(&self, symbol: &str) -> BrokerPosition {
        let account: &Account = &self.engine.account;
        BrokerPosition {
            symbol: symbol.to_owned(),
            qty: account.position(symbol).map(|p| p.qty).unwrap_or(0),
            available: account.available(symbol),
        }
    }

    fn positions(&self) -> Vec<BrokerPosition> {
        self.engine
            .account
            .positions
            .iter()
            .map(|(symbol, p)| BrokerPosition {
                symbol: symbol.clone(),
                qty: p.qty,
                available: p.available,
            })
            .collect()
    }

    fn pending_orders(&self) -> Vec<BrokerOrderView> {
        self.engine
            .user_orders()
            .filter(|o| o.is_active())
            .map(|o| BrokerOrderView {
                id: BrokerOrderId(o.id),
                symbol: o.symbol.as_str().to_owned(),
                side: o.side,
                kind: o.kind,
                price: o.price,
                qty: o.qty,
                filled_qty: o.filled_qty,
                status: o.status,
            })
            .collect()
    }

    fn set_price(&mut self, symbol: &str, price: Price) {
        self.engine.set_price(symbol, price);
        self.engine.refresh_liquidity(symbol, price);
    }

    fn roll_day(&mut self) {
        self.engine.roll_day();
    }
}

/// Connection parameters for a real broker adapter. Currently unused — kept so
/// the shape of a real integration is explicit and so future adapters have a
/// place to read credentials (which must come from configuration, never
/// hardcoded).
#[derive(Clone, Debug, Default)]
pub struct LiveBrokerConfig {
    pub channel: &'static str,
    pub api_base: String,
    pub account: String,
}

impl LiveBrokerConfig {
    pub fn describe(&self) -> String {
        format!(
            "channel={} api_base={} account={}",
            self.channel, self.api_base, self.account
        )
    }
}

/// Intentional stub for a real exchange/broker.
///
/// Implementing this properly requires a concrete channel (e.g. a quant-platform
/// gateway such as QMT / Ptrade / 掘金, or a client-automation layer such as
/// easytrader). None of those are wired here on purpose: real-money automation
/// carries legal, compliance and financial risk and must be a deliberate,
/// reviewed step. Until then every `submit` fails loudly with
/// [`BrokerError::NotConfigured`].
pub struct LiveBroker {
    pub config: LiveBrokerConfig,
}

impl LiveBroker {
    pub fn new() -> Self {
        Self {
            config: LiveBrokerConfig {
                channel: "未配置",
                api_base: String::new(),
                account: String::new(),
            },
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.config.api_base.is_empty()
    }
}

impl Default for LiveBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl Broker for LiveBroker {
    fn name(&self) -> &str {
        "实盘券商（未配置）"
    }

    fn submit(&mut self, _req: OrderRequest) -> Result<BrokerOrderId, BrokerError> {
        Err(BrokerError::NotConfigured(
            "请接入具体通道（QMT / Ptrade / 掘金 量化网关，或 东财/同花顺 客户端自动化），并实现 Broker trait",
        ))
    }

    fn cancel(&mut self, _id: BrokerOrderId) -> Result<(), BrokerError> {
        Err(BrokerError::NotConfigured(
            "实盘券商适配器未实现：取消委托需由具体通道实现",
        ))
    }

    fn last_price(&self, _symbol: &str) -> Option<Price> {
        None
    }

    fn cash(&self) -> Money {
        0
    }

    fn position(&self, symbol: &str) -> BrokerPosition {
        BrokerPosition {
            symbol: symbol.to_owned(),
            qty: 0,
            available: 0,
        }
    }

    fn positions(&self) -> Vec<BrokerPosition> {
        Vec::new()
    }

    fn pending_orders(&self) -> Vec<BrokerOrderView> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::rules_for;
    use crate::types::{Market, Symbol};

    fn paper() -> PaperBroker {
        PaperBroker::new(rules_for(Market::AShare), 100_000_000)
    }

    #[test]
    fn paper_broker_fills_a_market_order_against_refreshed_liquidity() {
        let mut b = paper();
        b.set_price("600519.SH", 168_500);
        let _id = b
            .submit(OrderRequest {
                symbol: Symbol::new("600519.SH"),
                side: Side::Buy,
                kind: OrderType::Market,
                price: 168_500,
                qty: 100,
            })
            .unwrap();
        let trades = &b.engine().trades;
        assert_eq!(trades.len(), 1);
        let t = &trades[0];
        // Marketable buy fills at the ask (last + 1 tick), not at the last price.
        assert_eq!(t.price, 168_501);
        assert_eq!(b.position("600519.SH").qty, 100);
        assert_eq!(b.cash(), 100_000_000 - t.price * t.qty - b.engine().account.total_fees);
        // A-share T+1: just bought, nothing sellable yet.
        assert_eq!(b.position("600519.SH").available, 0);
        assert_eq!(b.pending_orders().len(), 0);
    }

    #[test]
    fn paper_broker_t_plus_1_releases_shares_after_roll_day() {
        let mut b = paper();
        b.set_price("600519.SH", 168_500);
        b.submit(OrderRequest {
            symbol: Symbol::new("600519.SH"),
            side: Side::Buy,
            kind: OrderType::Market,
            price: 168_500,
            qty: 100,
        })
        .unwrap();
        b.roll_day();
        assert_eq!(b.position("600519.SH").available, 100);
    }

    #[test]
    fn live_broker_is_unconfigured_and_rejects_everything() {
        let mut b = LiveBroker::new();
        assert!(!b.is_configured());
        let err = b
            .submit(OrderRequest {
                symbol: Symbol::new("600519.SH"),
                side: Side::Buy,
                kind: OrderType::Market,
                price: 168_500,
                qty: 100,
            })
            .unwrap_err();
        assert!(matches!(err, BrokerError::NotConfigured(_)));
        assert_eq!(b.last_price("600519.SH"), None);
        assert_eq!(b.positions().len(), 0);
        assert!(!b.config.describe().is_empty());
    }
}
