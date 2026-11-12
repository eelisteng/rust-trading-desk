use std::fmt;

use crate::types::{OrderId, OrderStatus, Qty, Symbol};

#[derive(Clone, Debug, PartialEq)]
pub enum RiskError {
    InvalidQuantity,
    LotSize { qty: Qty, lot: Qty },
    InvalidPrice,
    PriceOutOfBand { price: i64, lower: i64, upper: i64 },
    InsufficientCash { required: i64, available: i64 },
    InsufficientPosition { requested: Qty, available: Qty },
    Concentration { notional_pct: f64, max_pct: f64 },
}

impl RiskError {
    /// Human readable Chinese explanation, shown directly in the UI.
    pub fn message(&self) -> String {
        match self {
            RiskError::InvalidQuantity => "委托数量必须大于 0".to_string(),
            RiskError::LotSize { qty, lot } => {
                format!("数量 {qty} 不是最小交易单位 {lot} 的整数倍")
            }
            RiskError::InvalidPrice => "限价必须大于 0".to_string(),
            RiskError::PriceOutOfBand {
                price,
                lower,
                upper,
            } => format!("委托价 {price} 超出涨跌停区间 [{lower}, {upper}]（单位：最小价位）"),
            RiskError::InsufficientCash { required, available } => format!(
                "可用资金不足：需要 {required} 分，当前 {available} 分（含预估费用）"
            ),
            RiskError::InsufficientPosition {
                requested,
                available,
            } => {
                format!("可卖数量不足：拟卖 {requested}，可卖 {available}（注意 T+1 限制）")
            }
            RiskError::Concentration {
                notional_pct,
                max_pct,
            } => format!(
                "单笔金额占总权益 {notional_pct:.1}%，超过集中度上限 {max_pct:.0}%"
            ),
        }
    }
}

impl fmt::Display for RiskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for RiskError {}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineError {
    Risk(RiskError),
    OrderNotFound(OrderId),
    OrderNotActive(OrderId, OrderStatus),
    UnknownSymbol(Symbol),
}

impl EngineError {
    pub fn message(&self) -> String {
        match self {
            EngineError::Risk(r) => r.message(),
            EngineError::OrderNotFound(id) => format!("订单 #{id} 不存在"),
            EngineError::OrderNotActive(id, status) => {
                format!("订单 #{id} 当前状态为 {}，不可撤销", status.label())
            }
            EngineError::UnknownSymbol(s) => format!("未知标的 {s}"),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for EngineError {}

impl From<RiskError> for EngineError {
    fn from(value: RiskError) -> Self {
        EngineError::Risk(value)
    }
}
