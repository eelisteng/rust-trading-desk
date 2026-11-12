//! `tdesk-core` — domain kernel for the paper-trading terminal.
//!
//! No I/O, no system clock, no UI dependencies: everything here can be driven
//! deterministically from tests or a headless CLI.

pub mod account;
pub mod backtest;
pub mod engine;
pub mod error;
pub mod market;
pub mod risk;
pub mod types;

pub use account::{Account, Position};
pub use backtest::{
    run_backtest, run_backtest_with, BacktestConfig, BacktestResult, BarContext, Metrics, Signal,
    SmaCrossStrategy, Strategy,
};
pub use engine::{LiquidityConfig, MatchingEngine, OrderBook};
pub use error::{EngineError, RiskError};
pub use market::{rules_for, FeeSchedule, MarketRules};
pub use types::*;
