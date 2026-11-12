//! `tdesk-core` — domain kernel for the paper-trading terminal.
//!
//! No I/O, no system clock, no UI dependencies: everything here can be driven
//! deterministically from tests or a headless CLI.

pub mod account;
pub mod backtest;
pub mod broker;
pub mod engine;
pub mod error;
pub mod live;
pub mod market;
pub mod risk;
pub mod strategies;
pub mod types;

pub use account::{Account, Position};
pub use backtest::{
    run_backtest, run_backtest_with, BacktestConfig, BacktestResult, BarContext, Metrics, Signal,
    SmaCrossStrategy, Strategy,
};
pub use broker::{
    Broker, BrokerError, BrokerOrderId, BrokerOrderView, BrokerPosition, LiveBroker,
    LiveBrokerConfig, PaperBroker,
};
pub use engine::{LiquidityConfig, MatchingEngine, OrderBook};
pub use error::{EngineError, RiskError};
pub use live::{size_buy, size_sell, LiveAction, LiveContext, LiveStrategy, LiveTrader};
pub use market::{rules_for, FeeSchedule, MarketRules};
pub use strategies::{BreakoutStrategy, DualMaStrategy, MeanReversionStrategy};
pub use types::*;
