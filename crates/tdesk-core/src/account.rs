//! Cash ledger, positions and settlement.
//!
//! All amounts are minor units. A position keeps its *total* cost basis rather
//! than an average price so that repeated partial closes never accumulate
//! rounding error.

use std::collections::BTreeMap;

use crate::market::MarketRules;
use crate::types::{Money, Price, Qty, Side, Symbol, Trade};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub symbol: Symbol,
    pub qty: Qty,
    /// Shares that can be sold right now (T+1 aware).
    pub available: Qty,
    /// Shares bought today that are still locked by T+1.
    pub locked_today: Qty,
    /// Total remaining cost including buy-side fees.
    pub cost_basis: Money,
    pub realized_pnl: Money,
    pub last_price: Price,
}

impl Position {
    pub fn new(symbol: Symbol) -> Self {
        Self {
            symbol,
            qty: 0,
            available: 0,
            locked_today: 0,
            cost_basis: 0,
            realized_pnl: 0,
            last_price: 0,
        }
    }

    pub fn avg_cost(&self) -> Money {
        if self.qty == 0 {
            0
        } else {
            self.cost_basis / self.qty
        }
    }

    pub fn market_value(&self, rules: &dyn MarketRules) -> Money {
        self.qty * rules.price_to_minor(self.last_price)
    }

    pub fn unrealized_pnl(&self, rules: &dyn MarketRules) -> Money {
        self.qty * rules.price_to_minor(self.last_price) - self.cost_basis
    }

    pub fn apply_buy(
        &mut self,
        price: Price,
        qty: Qty,
        fee: Money,
        rules: &dyn MarketRules,
    ) {
        let amount = rules.price_to_minor(price) * qty;
        self.qty += qty;
        self.cost_basis += amount + fee;
        if rules.settlement_lag_days() == 0 {
            self.available += qty;
        } else {
            self.locked_today += qty;
        }
        self.last_price = price;
    }

    pub fn apply_sell(
        &mut self,
        price: Price,
        qty: Qty,
        fee: Money,
        rules: &dyn MarketRules,
    ) {
        let amount = rules.price_to_minor(price) * qty;
        let avg = self.avg_cost();
        let cost = avg * qty;
        self.realized_pnl += amount - fee - cost;
        self.qty -= qty;
        self.cost_basis -= cost;
        self.available = (self.available - qty).max(0);
        self.last_price = price;
        if self.qty == 0 {
            // Clear integer-division residue when the position is fully closed.
            self.cost_basis = 0;
        }
    }

    pub fn roll_day(&mut self, rules: &dyn MarketRules) {
        if rules.settlement_lag_days() > 0 && self.locked_today > 0 {
            self.available += self.locked_today;
            self.locked_today = 0;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub cash: Money,
    pub initial_cash: Money,
    pub positions: BTreeMap<String, Position>,
    pub total_fees: Money,
    pub day_index: u32,
}

impl Account {
    pub fn new(initial_cash: Money) -> Self {
        Self {
            cash: initial_cash,
            initial_cash,
            positions: BTreeMap::new(),
            total_fees: 0,
            day_index: 0,
        }
    }

    pub fn position(&self, symbol: &str) -> Option<&Position> {
        self.positions.get(symbol)
    }

    pub fn available(&self, symbol: &str) -> Qty {
        self.position(symbol).map(|p| p.available).unwrap_or(0)
    }

    pub fn realized_pnl(&self) -> Money {
        self.positions.values().map(|p| p.realized_pnl).sum()
    }

    pub fn market_value(&self, rules: &dyn MarketRules) -> Money {
        self.positions
            .values()
            .map(|p| p.market_value(rules))
            .sum()
    }

    pub fn unrealized_pnl(&self, rules: &dyn MarketRules) -> Money {
        self.positions
            .values()
            .map(|p| p.unrealized_pnl(rules))
            .sum()
    }

    pub fn equity(&self, rules: &dyn MarketRules) -> Money {
        self.cash + self.market_value(rules)
    }

    /// Mark every position at the given price map (missing symbols keep their
    /// last price). Call before computing equity / risk.
    pub fn mark_to_market(&mut self, prices: &BTreeMap<String, Price>) {
        for (symbol, pos) in self.positions.iter_mut() {
            if let Some(p) = prices.get(symbol) {
                pos.last_price = *p;
            }
        }
    }

    pub fn apply_fill(&mut self, trade: &Trade, rules: &dyn MarketRules) {
        let Some(side) = trade.user_side else {
            return;
        };
        let amount = rules.price_to_minor(trade.price) * trade.qty;
        match side {
            Side::Buy => self.cash -= amount + trade.fee,
            Side::Sell => self.cash += amount - trade.fee,
        }
        self.total_fees += trade.fee;

        let pos = self
            .positions
            .entry(trade.symbol.as_str().to_owned())
            .or_insert_with(|| Position::new(trade.symbol.clone()));
        match side {
            Side::Buy => pos.apply_buy(trade.price, trade.qty, trade.fee, rules),
            Side::Sell => pos.apply_sell(trade.price, trade.qty, trade.fee, rules),
        }
    }

    pub fn roll_day(&mut self, rules: &dyn MarketRules) {
        for pos in self.positions.values_mut() {
            pos.roll_day(rules);
        }
        self.day_index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::{rules_for, HkRules, MarketRules};
    use crate::types::Market;

    fn trade(side: Side, price: Price, qty: Qty, fee: Money) -> Trade {
        Trade {
            id: 1,
            ts: 0,
            symbol: Symbol::new("600519.SH"),
            price,
            qty,
            taker_order_id: 1,
            maker_order_id: 2,
            taker_side: side,
            user_side: Some(side),
            fee,
        }
    }

    #[test]
    fn buy_then_sell_settles_cash_and_realized_pnl() {
        let rules = rules_for(Market::AShare);
        let mut account = Account::new(100_000_000);
        account.apply_fill(&trade(Side::Buy, 10_000, 100, 500), rules.as_ref());
        assert_eq!(account.cash, 100_000_000 - 1_000_000 - 500);
        assert_eq!(account.position("600519.SH").unwrap().qty, 100);
        assert_eq!(account.position("600519.SH").unwrap().available, 0); // T+1

        account.apply_fill(&trade(Side::Sell, 11_000, 100, 500), rules.as_ref());
        let pos = account.position("600519.SH").unwrap();
        assert_eq!(pos.qty, 0);
        assert_eq!(pos.cost_basis, 0);
        // revenue 1,100,000 - sell fee 500 - basis (1,000,000 + 500) = 99,000
        assert_eq!(pos.realized_pnl, 99_000);
        assert_eq!(account.equity(rules.as_ref()), 100_000_000 + 99_000);
        assert_eq!(account.total_fees, 1000);
    }

    #[test]
    fn t_plus_1_releases_on_roll_day_and_t0_does_not_lock() {
        let cn = rules_for(Market::AShare);
        let mut cn_account = Account::new(100_000_000);
        cn_account.apply_fill(&trade(Side::Buy, 10_000, 100, 500), cn.as_ref());
        assert_eq!(cn_account.available("600519.SH"), 0);
        cn_account.roll_day(cn.as_ref());
        assert_eq!(cn_account.available("600519.SH"), 100);
        assert_eq!(cn_account.day_index, 1);

        let hk: Box<dyn MarketRules> = Box::new(HkRules);
        let mut hk_account = Account::new(100_000_000);
        hk_account.apply_fill(&trade(Side::Buy, 10_000, 100, 500), hk.as_ref());
        assert_eq!(hk_account.available("600519.SH"), 100);
    }

    #[test]
    fn partial_close_keeps_cost_basis_consistent() {
        let rules = rules_for(Market::Us);
        let mut account = Account::new(100_000_000);
        account.apply_fill(&trade(Side::Buy, 5_000, 10, 0), rules.as_ref());
        account.apply_fill(&trade(Side::Sell, 6_000, 4, 0), rules.as_ref());
        let pos = account.position("600519.SH").unwrap();
        assert_eq!(pos.qty, 6);
        assert_eq!(pos.cost_basis, 30_000);
        assert_eq!(pos.available, 6);
        assert_eq!(pos.realized_pnl, 4_000);
    }

    #[test]
    fn equity_equals_cash_plus_marked_market_value() {
        let rules = rules_for(Market::AShare);
        let mut account = Account::new(100_000_000);
        account.apply_fill(&trade(Side::Buy, 10_000, 100, 500), rules.as_ref());
        let mut prices = BTreeMap::new();
        prices.insert("600519.SH".to_string(), 10_500);
        account.mark_to_market(&prices);
        assert_eq!(account.market_value(rules.as_ref()), 1_050_000);
        assert_eq!(account.unrealized_pnl(rules.as_ref()), 49_500);
        assert_eq!(account.equity(rules.as_ref()), account.cash + 1_050_000);
    }

    #[test]
    fn maker_only_trade_has_no_account_effect() {
        let rules = rules_for(Market::AShare);
        let mut account = Account::new(100_000_000);
        let mut t = trade(Side::Buy, 10_000, 100, 500);
        t.user_side = None;
        account.apply_fill(&t, rules.as_ref());
        assert_eq!(account.cash, 100_000_000);
        assert!(account.positions.is_empty());
    }
}
