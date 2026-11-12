//! Market rule abstraction. Adding a market only requires a new implementation
//! of [`MarketRules`] plus an entry in [`rules_for`].

use std::fmt;
use std::sync::Arc;

use crate::types::{Market, Money, Price, Qty, Side};

/// All fees are computed in currency minor units. Rates are `f64` by nature
/// (they are percentages) but every resulting amount is rounded to an integer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeeSchedule {
    pub commission_rate: f64,
    pub commission_min: Money,
    pub stamp_duty_rate_sell: f64,
    pub transfer_fee_rate: f64,
}

impl FeeSchedule {
    pub const ZERO: Self = Self {
        commission_rate: 0.0,
        commission_min: 0,
        stamp_duty_rate_sell: 0.0,
        transfer_fee_rate: 0.0,
    };

    fn mul(amount: Money, rate: f64) -> Money {
        (amount as f64 * rate).round() as Money
    }

    pub fn calculate(&self, side: Side, amount: Money) -> Money {
        let commission = Self::mul(amount, self.commission_rate).max(self.commission_min);
        let stamp = match side {
            Side::Sell => Self::mul(amount, self.stamp_duty_rate_sell),
            Side::Buy => 0,
        };
        let transfer = Self::mul(amount, self.transfer_fee_rate);
        commission + stamp + transfer
    }

    pub fn label(&self) -> String {
        format!(
            "佣金{:.4}% · 印花(卖){:.4}% · 过户{:.4}%",
            self.commission_rate * 100.0,
            self.stamp_duty_rate_sell * 100.0,
            self.transfer_fee_rate * 100.0
        )
    }
}

pub trait MarketRules: Send + Sync + fmt::Debug {
    fn market(&self) -> Market;
    fn currency(&self) -> &'static str;
    fn currency_symbol(&self) -> &'static str;
    /// Value of one price tick, in currency minor units (1 = 0.01).
    fn tick_minor(&self) -> Money;
    fn lot_size(&self, symbol: &str) -> Qty;
    /// `Some(0.10)` means +/-10% from previous close; `None` means no band.
    fn price_limit_ratio(&self) -> Option<f64>;
    /// 1 for T+1, 0 for T+0.
    fn settlement_lag_days(&self) -> u8;
    /// Chinese convention: red for rising prices.
    fn rise_is_red(&self) -> bool;
    fn fees(&self) -> FeeSchedule;

    fn price_to_f64(&self, price: Price) -> f64 {
        (price * self.tick_minor()) as f64 / 100.0
    }

    fn price_from_f64(&self, value: f64) -> Price {
        (value * 100.0 / self.tick_minor() as f64).round() as Price
    }

    fn price_to_minor(&self, price: Price) -> Money {
        price * self.tick_minor()
    }

    fn money_to_f64(&self, money: Money) -> f64 {
        money as f64 / 100.0
    }

    fn format_price(&self, price: Price) -> String {
        format!("{:.2}", self.price_to_f64(price))
    }

    #[allow(clippy::manual_is_multiple_of)] // keep compatibility with Rust < 1.87
    fn format_money(&self, money: Money) -> String {
        let negative = money < 0;
        let abs = money.unsigned_abs();
        let whole = abs / 100;
        let frac = abs % 100;
        let mut grouped = String::new();
        let digits = whole.to_string();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        format!(
            "{}{}{}.{:02}",
            if negative { "-" } else { "" },
            self.currency_symbol(),
            grouped,
            frac
        )
    }

    fn format_signed_money(&self, money: Money) -> String {
        if money > 0 {
            format!("+{}", self.format_money(money))
        } else {
            self.format_money(money)
        }
    }

    fn limit_range(&self, prev_close: Price) -> (Price, Price) {
        match self.price_limit_ratio() {
            Some(ratio) => {
                let lower = (prev_close as f64 * (1.0 - ratio)).round() as Price;
                let upper = (prev_close as f64 * (1.0 + ratio)).round() as Price;
                (lower.max(1), upper.max(lower.max(1)))
            }
            None => (1, Price::MAX / 4),
        }
    }

    /// Maximum fraction of total equity a single order may represent.
    fn max_order_notional_pct(&self) -> f64 {
        0.50
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AShareRules;

impl MarketRules for AShareRules {
    fn market(&self) -> Market {
        Market::AShare
    }
    fn currency(&self) -> &'static str {
        "CNY"
    }
    fn currency_symbol(&self) -> &'static str {
        "¥"
    }
    fn tick_minor(&self) -> Money {
        1
    }
    fn lot_size(&self, _symbol: &str) -> Qty {
        100
    }
    fn price_limit_ratio(&self) -> Option<f64> {
        Some(0.10)
    }
    fn settlement_lag_days(&self) -> u8 {
        1
    }
    fn rise_is_red(&self) -> bool {
        true
    }
    fn fees(&self) -> FeeSchedule {
        FeeSchedule {
            commission_rate: 0.0003,
            commission_min: 500,
            stamp_duty_rate_sell: 0.0005,
            transfer_fee_rate: 0.00001,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UsRules;

impl MarketRules for UsRules {
    fn market(&self) -> Market {
        Market::Us
    }
    fn currency(&self) -> &'static str {
        "USD"
    }
    fn currency_symbol(&self) -> &'static str {
        "$"
    }
    fn tick_minor(&self) -> Money {
        1
    }
    fn lot_size(&self, _symbol: &str) -> Qty {
        1
    }
    fn price_limit_ratio(&self) -> Option<f64> {
        None
    }
    fn settlement_lag_days(&self) -> u8 {
        0
    }
    fn rise_is_red(&self) -> bool {
        false
    }
    fn fees(&self) -> FeeSchedule {
        FeeSchedule::ZERO
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HkRules;

impl MarketRules for HkRules {
    fn market(&self) -> Market {
        Market::Hk
    }
    fn currency(&self) -> &'static str {
        "HKD"
    }
    fn currency_symbol(&self) -> &'static str {
        "HK$"
    }
    fn tick_minor(&self) -> Money {
        1
    }
    fn lot_size(&self, symbol: &str) -> Qty {
        match symbol {
            "01810.HK" => 200,
            "03690.HK" => 100,
            _ => 100,
        }
    }
    fn price_limit_ratio(&self) -> Option<f64> {
        None
    }
    fn settlement_lag_days(&self) -> u8 {
        0
    }
    fn rise_is_red(&self) -> bool {
        true
    }
    fn fees(&self) -> FeeSchedule {
        FeeSchedule {
            commission_rate: 0.0003,
            commission_min: 0,
            stamp_duty_rate_sell: 0.001,
            transfer_fee_rate: 0.0,
        }
    }
}

pub fn rules_for(market: Market) -> Arc<dyn MarketRules> {
    match market {
        Market::AShare => Arc::new(AShareRules),
        Market::Us => Arc::new(UsRules),
        Market::Hk => Arc::new(HkRules),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_limit_band_is_rounded_to_ticks() {
        let r = AShareRules;
        let (lo, hi) = r.limit_range(1000);
        assert_eq!(lo, 900);
        assert_eq!(hi, 1100);
        assert_eq!(r.limit_range(1685).1, 1854); // 1685 * 1.1 = 1853.5 -> 1854
    }

    #[test]
    fn us_market_has_no_band_and_t_plus_0() {
        let r = UsRules;
        assert_eq!(r.price_limit_ratio(), None);
        assert_eq!(r.settlement_lag_days(), 0);
        assert!(!r.rise_is_red());
        let (lo, _) = r.limit_range(1000);
        assert_eq!(lo, 1);
    }

    #[test]
    fn a_share_minimum_commission_applies() {
        let r = AShareRules;
        // 100 shares @ 10.00 = 100000 cents => 0.03% = 30 cents, below 500 minimum
        assert_eq!(r.fees().calculate(Side::Buy, 100_000), 501);
    }

    #[test]
    fn stamp_duty_only_on_sell() {
        let hk = HkRules;
        assert_eq!(hk.fees().calculate(Side::Buy, 100_000), 30);
        assert_eq!(hk.fees().calculate(Side::Sell, 100_000), 130);
    }

    #[test]
    fn money_formatting_is_grouped_and_signed() {
        let r = AShareRules;
        assert_eq!(r.format_money(123_456_789), "¥1,234,567.89");
        assert_eq!(r.format_money(-250), "-¥2.50");
        assert_eq!(r.format_signed_money(250), "+¥2.50");
        assert_eq!(r.format_price(168_500), "1685.00");
    }

    #[test]
    fn price_roundtrip_is_stable() {
        for market in Market::ALL {
            let r = rules_for(market);
            for p in [1_i64, 999, 168_500, 12_345] {
                assert_eq!(r.price_from_f64(r.price_to_f64(p)), p);
            }
        }
    }
}
