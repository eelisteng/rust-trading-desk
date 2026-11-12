//! Pre-trade risk checks. The validator is strictly read-only: a rejected
//! request leaves both the account and the order book untouched.

use crate::engine::MatchingEngine;
use crate::error::RiskError;
use crate::types::{OrderRequest, OrderType, Side};

pub fn validate(engine: &MatchingEngine, req: &OrderRequest) -> Result<(), RiskError> {
    let rules = engine.rules.as_ref();
    let symbol = req.symbol.as_str();

    if req.qty <= 0 {
        return Err(RiskError::InvalidQuantity);
    }
    let lot = rules.lot_size(symbol);
    if lot > 0 && req.qty % lot != 0 {
        return Err(RiskError::LotSize {
            qty: req.qty,
            lot,
        });
    }

    let prev_close = engine
        .prev_closes
        .get(symbol)
        .copied()
        .unwrap_or(engine.last_prices.get(symbol).copied().unwrap_or(req.price));
    let (lower, upper) = rules.limit_range(prev_close);
    if req.kind == OrderType::Limit {
        if req.price <= 0 {
            return Err(RiskError::InvalidPrice);
        }
        if req.price < lower || req.price > upper {
            return Err(RiskError::PriceOutOfBand {
                price: req.price,
                lower,
                upper,
            });
        }
    }

    let est_price = engine.estimate_execution_price(req);
    let notional = rules.price_to_minor(est_price) * req.qty;
    let fee = rules.fees().calculate(req.side, notional);
    match req.side {
        Side::Buy => {
            let required = notional + fee;
            if required > engine.account.cash {
                return Err(RiskError::InsufficientCash {
                    required,
                    available: engine.account.cash,
                });
            }
        }
        Side::Sell => {
            let available = engine.account.available(symbol);
            if req.qty > available {
                return Err(RiskError::InsufficientPosition {
                    requested: req.qty,
                    available,
                });
            }
        }
    }

    // Portfolio-level guard is evaluated last so that the most actionable
    // reason (cash / position) is reported first.
    let equity = engine.account.equity(rules).max(1);
    let notional_pct = notional as f64 / equity as f64 * 100.0;
    let max_pct = rules.max_order_notional_pct() * 100.0;
    if notional_pct > max_pct {
        return Err(RiskError::Concentration {
            notional_pct,
            max_pct,
        });
    }
    Ok(())
}
