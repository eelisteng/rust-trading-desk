//! Price/time-priority order book, matching and pre-trade risk gate.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crate::account::Account;
use crate::error::EngineError;
use crate::market::MarketRules;
use crate::types::{
    Market, Money, Order, OrderId, OrderRequest, OrderStatus, OrderType, Owner, Price, Qty, Side,
    Symbol, Timestamp, Trade, TradeId,
};
use crate::risk;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiquidityConfig {
    /// Number of levels on each side of the mid price.
    pub levels: usize,
    /// Distance between levels, in ticks.
    pub spread_ticks: Price,
    /// Shares offered at each level (rounded to a multiple of the lot size).
    pub qty_per_level: Qty,
}

impl Default for LiquidityConfig {
    fn default() -> Self {
        Self {
            levels: 5,
            spread_ticks: 1,
            qty_per_level: 200,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderBook {
    /// Ascending by price; best bid is the last key.
    pub bids: BTreeMap<Price, VecDeque<OrderId>>,
    /// Ascending by price; best ask is the first key.
    pub asks: BTreeMap<Price, VecDeque<OrderId>>,
}

impl OrderBook {
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    pub fn depth(&self) -> usize {
        self.bids.values().map(VecDeque::len).sum::<usize>()
            + self.asks.values().map(VecDeque::len).sum::<usize>()
    }
}

pub struct MatchingEngine {
    pub rules: Arc<dyn MarketRules>,
    pub account: Account,
    pub books: BTreeMap<String, OrderBook>,
    pub orders: BTreeMap<OrderId, Order>,
    pub trades: Vec<Trade>,
    pub last_prices: BTreeMap<String, Price>,
    pub prev_closes: BTreeMap<String, Price>,
    pub liquidity: LiquidityConfig,
    pub clock: Timestamp,
    pub day_index: u32,
    /// Index of live maker orders per symbol, so refreshing depth is O(levels)
    /// instead of scanning the full order history.
    maker_index: BTreeMap<String, Vec<OrderId>>,
    next_order_id: OrderId,
    next_trade_id: TradeId,
}

impl MatchingEngine {
    pub fn new(rules: Arc<dyn MarketRules>, initial_cash: Money) -> Self {
        Self {
            rules,
            account: Account::new(initial_cash),
            books: BTreeMap::new(),
            orders: BTreeMap::new(),
            trades: Vec::new(),
            last_prices: BTreeMap::new(),
            prev_closes: BTreeMap::new(),
            liquidity: LiquidityConfig::default(),
            clock: 1_700_000_000,
            day_index: 0,
            maker_index: BTreeMap::new(),
            next_order_id: 1,
            next_trade_id: 1,
        }
    }

    pub fn market(&self) -> Market {
        self.rules.market()
    }

    pub fn register_symbol(&mut self, symbol: &str, price: Price) {
        self.books.entry(symbol.to_owned()).or_default();
        self.last_prices.insert(symbol.to_owned(), price);
        self.prev_closes.entry(symbol.to_owned()).or_insert(price);
    }

    /// Mark a new last traded price and revalue the portfolio.
    pub fn set_price(&mut self, symbol: &str, last: Price) {
        self.register_symbol_if_missing(symbol, last);
        self.last_prices.insert(symbol.to_owned(), last);
        let mut prices = BTreeMap::new();
        prices.insert(symbol.to_owned(), last);
        self.account.mark_to_market(&prices);
    }

    fn register_symbol_if_missing(&mut self, symbol: &str, price: Price) {
        if !self.last_prices.contains_key(symbol) {
            self.register_symbol(symbol, price);
        } else {
            self.books.entry(symbol.to_owned()).or_default();
        }
    }

    /// Conservative price used for cash reservation. Market orders are
    /// reserved at +1% of last (capped by the upper price band).
    pub fn estimate_execution_price(&self, req: &OrderRequest) -> Price {
        match req.kind {
            OrderType::Limit => req.price,
            OrderType::Market => {
                let symbol = req.symbol.as_str();
                let last = self
                    .last_prices
                    .get(symbol)
                    .copied()
                    .unwrap_or(req.price.max(1));
                let prev_close = self.prev_closes.get(symbol).copied().unwrap_or(last);
                let (_, upper) = self.rules.limit_range(prev_close);
                let estimate = (last as f64 * 1.01).round() as Price;
                estimate.clamp(1, upper.max(1))
            }
        }
    }

    pub fn book(&self, symbol: &str) -> Option<&OrderBook> {
        self.books.get(symbol)
    }

    pub fn user_orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values().filter(|o| o.owner == Owner::User)
    }

    pub fn user_trades(&self) -> impl Iterator<Item = &Trade> {
        self.trades.iter().filter(|t| t.user_side.is_some())
    }

    pub fn active_user_order_count(&self) -> usize {
        self.user_orders().filter(|o| o.is_active()).count()
    }

    /// Risk gate → match → settle. On rejection a `Rejected` order is kept for
    /// the audit trail but neither the book nor the account changes.
    pub fn submit_user_order(&mut self, req: OrderRequest) -> Result<OrderId, EngineError> {
        let symbol = req.symbol.as_str().to_owned();
        if !self.last_prices.contains_key(&symbol) {
            return Err(EngineError::UnknownSymbol(req.symbol.clone()));
        }
        if let Err(err) = risk::validate(self, &req) {
            let id = self.alloc_order_id();
            let order = Order {
                id,
                symbol: req.symbol.clone(),
                side: req.side,
                kind: req.kind,
                price: req.price,
                qty: req.qty,
                filled_qty: 0,
                avg_fill_price: 0,
                status: OrderStatus::Rejected,
                owner: Owner::User,
                created_at: self.clock,
                updated_at: self.clock,
                reject_reason: Some(err.message()),
            };
            self.orders.insert(id, order);
            return Err(EngineError::Risk(err));
        }

        let id = self.alloc_order_id();
        let mut order = Order {
            id,
            symbol: req.symbol.clone(),
            side: req.side,
            kind: req.kind,
            price: req.price,
            qty: req.qty,
            filled_qty: 0,
            avg_fill_price: 0,
            status: OrderStatus::Pending,
            owner: Owner::User,
            created_at: self.clock,
            updated_at: self.clock,
            reject_reason: None,
        };
        let trades = self.cross(&mut order);
        self.finalize(order, trades);
        Ok(id)
    }

    /// Synthetic liquidity order. Bypasses risk checks and never settles
    /// against the account unless it crosses a resting user order.
    pub fn submit_maker_order(
        &mut self,
        symbol: &str,
        side: Side,
        price: Price,
        qty: Qty,
    ) -> OrderId {
        self.register_symbol_if_missing(symbol, price);
        let id = self.alloc_order_id();
        let mut order = Order {
            id,
            symbol: Symbol::new(symbol),
            side,
            kind: OrderType::Limit,
            price,
            qty,
            filled_qty: 0,
            avg_fill_price: 0,
            status: OrderStatus::Pending,
            owner: Owner::Maker,
            created_at: self.clock,
            updated_at: self.clock,
            reject_reason: None,
        };
        let trades = self.cross(&mut order);
        self.finalize(order, trades);
        id
    }

    pub fn cancel_order(&mut self, id: OrderId) -> Result<(), EngineError> {
        let (symbol, side, price, status) = match self.orders.get(&id) {
            Some(o) => (o.symbol.as_str().to_owned(), o.side, o.price, o.status),
            None => return Err(EngineError::OrderNotFound(id)),
        };
        if !status.is_active() {
            return Err(EngineError::OrderNotActive(id, status));
        }
        self.remove_from_book(&symbol, side, price, id);
        if let Some(order) = self.orders.get_mut(&id) {
            order.status = OrderStatus::Cancelled;
            order.updated_at = self.clock;
        }
        Ok(())
    }

    /// Rebuild maker depth around `mid`. Resting user orders are preserved and
    /// may be filled if the new depth crosses them.
    pub fn refresh_liquidity(&mut self, symbol: &str, mid: Price) {
        let maker_ids = self.maker_index.remove(symbol).unwrap_or_default();
        for id in maker_ids {
            let Some((side, price, active)) = self
                .orders
                .get(&id)
                .map(|o| (o.side, o.price, o.is_active()))
            else {
                continue;
            };
            if !active {
                continue;
            }
            self.remove_from_book(symbol, side, price, id);
            if let Some(o) = self.orders.get_mut(&id) {
                o.status = OrderStatus::Cancelled;
                o.updated_at = self.clock;
            }
        }

        let lot = self.rules.lot_size(symbol).max(1);
        let step = self.liquidity.spread_ticks.max(1);
        let levels = self.liquidity.levels.max(1);
        let units = (self.liquidity.qty_per_level / lot).max(1);
        let qty = units * lot;
        for i in 1..=levels as i64 {
            let offset = i * step;
            let bid = (mid - offset).max(1);
            let ask = mid.saturating_add(offset);
            self.submit_maker_order(symbol, Side::Buy, bid, qty);
            self.submit_maker_order(symbol, Side::Sell, ask, qty);
        }
    }

    /// Close the trading day: previous close becomes today's last, and T+1
    /// locked shares become sellable.
    pub fn roll_day(&mut self) {
        for (symbol, price) in &self.last_prices {
            self.prev_closes.insert(symbol.clone(), *price);
        }
        self.account.roll_day(self.rules.as_ref());
        self.day_index += 1;
        self.clock += 86_400;
    }

    // ------------------------------------------------------------------ //
    // Matching internals                                                   //
    // ------------------------------------------------------------------ //

    fn alloc_order_id(&mut self) -> OrderId {
        let id = self.next_order_id;
        self.next_order_id += 1;
        id
    }

    fn alloc_trade_id(&mut self) -> TradeId {
        let id = self.next_trade_id;
        self.next_trade_id += 1;
        id
    }

    fn front_order(&self, symbol: &str, resting_side: Side, price: Price) -> Option<OrderId> {
        let book = self.books.get(symbol)?;
        let queue = match resting_side {
            Side::Buy => book.bids.get(&price),
            Side::Sell => book.asks.get(&price),
        }?;
        queue.front().copied()
    }

    fn best_price(&self, symbol: &str, resting_side: Side) -> Option<Price> {
        let book = self.books.get(symbol)?;
        match resting_side {
            Side::Buy => book.best_bid(),
            Side::Sell => book.best_ask(),
        }
    }

    fn pop_front(&mut self, symbol: &str, resting_side: Side, price: Price) {
        let became_empty = match self.books.get_mut(symbol) {
            Some(book) => match resting_side {
                Side::Buy => {
                    if let Some(q) = book.bids.get_mut(&price) {
                        q.pop_front();
                        q.is_empty()
                    } else {
                        false
                    }
                }
                Side::Sell => {
                    if let Some(q) = book.asks.get_mut(&price) {
                        q.pop_front();
                        q.is_empty()
                    } else {
                        false
                    }
                }
            },
            None => return,
        };
        if became_empty {
            if let Some(book) = self.books.get_mut(symbol) {
                match resting_side {
                    Side::Buy => book.bids.remove(&price),
                    Side::Sell => book.asks.remove(&price),
                };
            }
        }
    }

    fn remove_from_book(&mut self, symbol: &str, side: Side, price: Price, id: OrderId) {
        let became_empty = match self.books.get_mut(symbol) {
            Some(book) => {
                let queue = match side {
                    Side::Buy => book.bids.get_mut(&price),
                    Side::Sell => book.asks.get_mut(&price),
                };
                match queue {
                    Some(q) => {
                        q.retain(|x| *x != id);
                        q.is_empty()
                    }
                    None => false,
                }
            }
            None => return,
        };
        if became_empty {
            if let Some(book) = self.books.get_mut(symbol) {
                match side {
                    Side::Buy => book.bids.remove(&price),
                    Side::Sell => book.asks.remove(&price),
                };
            }
        }
    }

    fn insert_resting(&mut self, order: &Order) {
        let book = self.books.entry(order.symbol.as_str().to_owned()).or_default();
        let queue = match order.side {
            Side::Buy => book.bids.entry(order.price).or_default(),
            Side::Sell => book.asks.entry(order.price).or_default(),
        };
        queue.push_back(order.id);
    }

    fn cross(&mut self, taker: &mut Order) -> Vec<Trade> {
        let symbol = taker.symbol.as_str().to_owned();
        let resting_side = taker.side.opposite();
        let mut trades = Vec::new();

        loop {
            if taker.remaining() <= 0 {
                break;
            }
            let Some(price) = self.best_price(&symbol, resting_side) else {
                break;
            };
            let crossable = match taker.kind {
                OrderType::Market => true,
                OrderType::Limit => match taker.side {
                    Side::Buy => taker.price >= price,
                    Side::Sell => taker.price <= price,
                },
            };
            if !crossable {
                break;
            }
            let Some(maker_id) = self.front_order(&symbol, resting_side, price) else {
                self.pop_front(&symbol, resting_side, price);
                continue;
            };
            if maker_id == taker.id {
                break;
            }
            let Some(maker_owner) = self.orders.get(&maker_id).map(|o| o.owner) else {
                self.pop_front(&symbol, resting_side, price);
                continue;
            };
            // Self-trade prevention: cancel the older resting user order.
            if taker.owner == Owner::User && maker_owner == Owner::User {
                self.pop_front(&symbol, resting_side, price);
                if let Some(m) = self.orders.get_mut(&maker_id) {
                    m.status = OrderStatus::Cancelled;
                    m.updated_at = self.clock;
                }
                continue;
            }
            let maker_remaining = self.orders[&maker_id].remaining();
            if maker_remaining <= 0 {
                self.pop_front(&symbol, resting_side, price);
                continue;
            }
            let fill_qty = taker.remaining().min(maker_remaining);
            let user_side = match (taker.owner, maker_owner) {
                (Owner::User, _) => Some(taker.side),
                (_, Owner::User) => Some(resting_side),
                _ => None,
            };
            let fee = match user_side {
                Some(side) => {
                    let notional = self.rules.price_to_minor(price) * fill_qty;
                    self.rules.fees().calculate(side, notional)
                }
                None => 0,
            };
            let trade_id = self.alloc_trade_id();
            let trade = Trade {
                id: trade_id,
                ts: self.clock,
                symbol: Symbol::new(symbol.clone()),
                price,
                qty: fill_qty,
                taker_order_id: taker.id,
                maker_order_id: maker_id,
                taker_side: taker.side,
                user_side,
                fee,
            };
            update_avg_fill(taker, price, fill_qty);
            let maker_filled = {
                let maker = self.orders.get_mut(&maker_id).expect("maker exists");
                update_avg_fill(maker, price, fill_qty);
                maker.updated_at = self.clock;
                if maker.remaining() == 0 {
                    maker.status = OrderStatus::Filled;
                    true
                } else {
                    maker.status = OrderStatus::PartiallyFilled;
                    false
                }
            };
            if maker_filled {
                self.pop_front(&symbol, resting_side, price);
            }
            trades.push(trade);
        }
        trades
    }

    fn finalize(&mut self, mut order: Order, trades: Vec<Trade>) {
        order.updated_at = self.clock;
        if order.filled_qty >= order.qty {
            order.status = OrderStatus::Filled;
        } else if order.kind == OrderType::Market {
            // IOC semantics: any unfilled remainder is cancelled, never rested.
            order.status = OrderStatus::Cancelled;
        } else {
            order.status = if order.filled_qty > 0 {
                OrderStatus::PartiallyFilled
            } else {
                OrderStatus::Pending
            };
        }
        if order.is_active() {
            self.insert_resting(&order);
            if order.owner == Owner::Maker {
                self.maker_index
                    .entry(order.symbol.as_str().to_owned())
                    .or_default()
                    .push(order.id);
            }
        }
        self.orders.insert(order.id, order);
        let rules = self.rules.clone();
        for trade in trades {
            self.account.apply_fill(&trade, rules.as_ref());
            self.trades.push(trade);
        }
    }
}

fn update_avg_fill(order: &mut Order, price: Price, qty: Qty) {
    let total = order.filled_qty + qty;
    order.avg_fill_price = (order.avg_fill_price * order.filled_qty + price * qty) / total;
    order.filled_qty = total;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RiskError;
    use crate::market::rules_for;

    const INITIAL_CASH: Money = 100_000_000; // ¥1,000,000.00

    fn engine(market: Market) -> MatchingEngine {
        let mut e = MatchingEngine::new(rules_for(market), INITIAL_CASH);
        e.register_symbol("TEST", 10_000);
        e
    }

    fn buy(symbol: &str, price: Price, qty: Qty) -> OrderRequest {
        OrderRequest {
            symbol: Symbol::new(symbol),
            side: Side::Buy,
            kind: OrderType::Limit,
            price,
            qty,
        }
    }

    fn sell(symbol: &str, price: Price, qty: Qty) -> OrderRequest {
        OrderRequest {
            symbol: Symbol::new(symbol),
            side: Side::Sell,
            kind: OrderType::Limit,
            price,
            qty,
        }
    }

    #[test]
    fn price_and_time_priority_fill_at_maker_price() {
        let mut e = engine(Market::AShare);
        e.submit_maker_order("TEST", Side::Sell, 10_100, 100);
        e.submit_maker_order("TEST", Side::Sell, 10_000, 100);
        let id = e.submit_user_order(buy("TEST", 10_500, 200)).unwrap();
        let fills: Vec<_> = e.user_trades().collect();
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].price, 10_000); // lower ask first
        assert_eq!(fills[1].price, 10_100);
        assert_eq!(e.orders[&id].status, OrderStatus::Filled);
        assert_eq!(e.orders[&id].avg_fill_price, 10_050);
        assert_eq!(e.book("TEST").unwrap().depth(), 0);
    }

    #[test]
    fn partial_fill_leaves_remainder_resting() {
        let mut e = engine(Market::AShare);
        e.submit_maker_order("TEST", Side::Sell, 10_000, 100);
        let id = e.submit_user_order(buy("TEST", 10_000, 200)).unwrap();
        assert_eq!(e.orders[&id].status, OrderStatus::PartiallyFilled);
        assert_eq!(e.orders[&id].remaining(), 100);
        assert_eq!(e.book("TEST").unwrap().best_bid(), Some(10_000));
        assert_eq!(e.user_trades().count(), 1);
    }

    #[test]
    fn cancel_removes_resting_order_from_book() {
        let mut e = engine(Market::AShare);
        let id = e.submit_user_order(buy("TEST", 9_800, 100)).unwrap();
        assert!(e.orders[&id].is_active());
        e.cancel_order(id).unwrap();
        assert_eq!(e.orders[&id].status, OrderStatus::Cancelled);
        assert_eq!(e.book("TEST").unwrap().depth(), 0);
        assert!(matches!(
            e.cancel_order(id),
            Err(EngineError::OrderNotActive(_, _))
        ));
    }

    #[test]
    fn price_band_rejection_has_no_side_effects() {
        let mut e = engine(Market::AShare);
        let cash_before = e.account.cash;
        let err = e.submit_user_order(buy("TEST", 12_000, 100)).unwrap_err();
        assert!(matches!(err, EngineError::Risk(RiskError::PriceOutOfBand { .. })));
        assert_eq!(e.account.cash, cash_before);
        assert_eq!(e.book("TEST").unwrap().depth(), 0);
        let rejected: Vec<_> = e.user_orders().collect();
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].status, OrderStatus::Rejected);
        assert!(rejected[0].reject_reason.is_some());
    }

    #[test]
    fn lot_size_and_quantity_are_enforced() {
        let mut e = engine(Market::AShare);
        assert!(matches!(
            e.submit_user_order(buy("TEST", 10_000, 150)),
            Err(EngineError::Risk(RiskError::LotSize { .. }))
        ));
        assert!(matches!(
            e.submit_user_order(buy("TEST", 10_000, 0)),
            Err(EngineError::Risk(RiskError::InvalidQuantity))
        ));
    }

    #[test]
    fn insufficient_cash_is_rejected() {
        let mut e = MatchingEngine::new(rules_for(Market::AShare), 1_000);
        e.register_symbol("TEST", 10_000);
        e.submit_maker_order("TEST", Side::Sell, 10_000, 100);
        assert!(matches!(
            e.submit_user_order(buy("TEST", 10_000, 100)),
            Err(EngineError::Risk(RiskError::InsufficientCash { .. }))
        ));
        assert_eq!(e.account.cash, 1_000);
    }

    #[test]
    fn t_plus_1_blocks_same_day_sell_and_releases_next_day() {
        let mut e = engine(Market::AShare);
        e.submit_maker_order("TEST", Side::Sell, 10_000, 100);
        e.submit_user_order(buy("TEST", 10_000, 100)).unwrap();
        assert_eq!(e.account.available("TEST"), 0);
        assert!(matches!(
            e.submit_user_order(sell("TEST", 10_000, 100)),
            Err(EngineError::Risk(RiskError::InsufficientPosition { .. }))
        ));
        e.roll_day();
        assert_eq!(e.account.available("TEST"), 100);
        assert_eq!(e.prev_closes["TEST"], 10_000);
        e.submit_maker_order("TEST", Side::Buy, 10_000, 100);
        e.submit_user_order(sell("TEST", 10_000, 100)).unwrap();
        assert_eq!(e.account.position("TEST").unwrap().qty, 0);
    }

    #[test]
    fn t0_market_allows_immediate_sell() {
        let mut e = engine(Market::Us);
        e.submit_maker_order("TEST", Side::Sell, 10_000, 1);
        e.submit_user_order(buy("TEST", 10_000, 1)).unwrap();
        assert_eq!(e.account.available("TEST"), 1);
        e.submit_maker_order("TEST", Side::Buy, 10_000, 1);
        e.submit_user_order(sell("TEST", 10_000, 1)).unwrap();
        assert_eq!(e.account.position("TEST").unwrap().qty, 0);
    }

    #[test]
    fn self_trade_prevention_cancels_resting_order() {
        let mut e = engine(Market::Us);
        e.submit_maker_order("TEST", Side::Sell, 10_000, 1);
        e.submit_user_order(buy("TEST", 10_000, 1)).unwrap();
        let buy_id = e
            .submit_user_order(buy("TEST", 9_900, 1))
            .unwrap();
        let trades_before = e.trades.len();
        e.submit_user_order(sell("TEST", 9_900, 1)).unwrap();
        assert_eq!(e.trades.len(), trades_before);
        assert_eq!(e.orders[&buy_id].status, OrderStatus::Cancelled);
    }

    #[test]
    fn market_order_fills_immediately_and_cancels_remainder() {
        let mut e = engine(Market::AShare);
        let mut req = buy("TEST", 0, 300);
        req.kind = OrderType::Market;
        e.refresh_liquidity("TEST", 10_000);
        let id = e.submit_user_order(req).unwrap();
        assert_eq!(e.orders[&id].status, OrderStatus::Filled);
        assert_eq!(e.user_trades().count(), 2); // 200 @ 10001 + 100 @ 10002
    }

    #[test]
    fn refresh_liquidity_preserves_user_orders_and_respects_lot() {
        let mut e = engine(Market::AShare);
        let id = e.submit_user_order(buy("TEST", 9_500, 100)).unwrap();
        e.refresh_liquidity("TEST", 10_000);
        assert!(e.orders[&id].is_active());
        assert_eq!(e.book("TEST").unwrap().best_bid(), Some(9_999));
        assert_eq!(e.book("TEST").unwrap().best_ask(), Some(10_001));
        assert_eq!(e.book("TEST").unwrap().depth(), 11); // 10 maker + 1 user
    }

    #[test]
    fn cash_plus_positions_conserves_equity_except_fees() {
        let mut e = engine(Market::AShare);
        let initial = e.account.equity(e.rules.as_ref());
        e.submit_maker_order("TEST", Side::Sell, 10_000, 100);
        e.submit_user_order(buy("TEST", 10_000, 100)).unwrap();
        let after = e.account.equity(e.rules.as_ref());
        assert_eq!(initial - after, e.account.total_fees);
    }
}
