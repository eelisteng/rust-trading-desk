//! Market data adapters for `tdesk`. All sources implement
//! [`MarketDataSource`] so the UI never depends on a concrete generator.

pub mod csv;
pub mod rng;
pub mod source;
pub mod synthetic;

pub use csv::CsvSource;
pub use rng::{fnv1a, Rng};
pub use source::{Instrument, MarketDataSource, BASE_TS, SECONDS_PER_DAY};
pub use synthetic::{
    generate_history, instrument_list, SyntheticSource, HISTORY_BARS, TICKS_PER_BAR,
};
