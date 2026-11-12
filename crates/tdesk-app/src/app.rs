//! Application state and UI. `render` is deliberately independent of
//! `eframe::App` so it can be executed headlessly from an integration test.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, Context, RawInput, RichText};
use tdesk_core::{
    run_backtest, rules_for, BacktestResult, Candle, Market, MarketRules, MatchingEngine, Metrics,
    Money, Order, OrderId, OrderRequest, OrderStatus, OrderType, Price, Qty, Side,
    SmaCrossStrategy, Symbol, Trade,
};
use tdesk_data::{Instrument, MarketDataSource, SyntheticSource};

use crate::charts;
use crate::panels;
use crate::theme::{self, Palette};

const INITIAL_CASH: Money = 100_000_000; // ¥1,000,000.00
const DATA_SEED: u64 = 20_261_112;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Orders,
    Trades,
    Positions,
    Account,
    Backtest,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Orders,
        Tab::Trades,
        Tab::Positions,
        Tab::Account,
        Tab::Backtest,
    ];
    fn label(self) -> &'static str {
        match self {
            Tab::Orders => "委托",
            Tab::Trades => "成交",
            Tab::Positions => "持仓",
            Tab::Account => "资金",
            Tab::Backtest => "回测",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Notice {
    text: String,
    is_error: bool,
}

#[derive(Debug)]
struct Actions {
    market: Option<Market>,
    selected: Option<String>,
    toggle_play: bool,
    toggle_dark: bool,
    roll_day: bool,
    reset: bool,
    submit: bool,
    cancel: Option<OrderId>,
    quick: Option<f32>,
    run_backtest: bool,
    tab: Tab,
    show_ma: bool,
    visible_bars: usize,
}

pub struct TradingApp {
    pub market: Market,
    pub rules: Arc<dyn MarketRules>,
    pub source: SyntheticSource,
    pub engine: MatchingEngine,
    pub selected: String,
    pub playing: bool,
    pub speed: f32,
    pub dark: bool,
    pub tab: Tab,
    pub order_side: Side,
    pub order_kind: OrderType,
    pub price_input: String,
    pub qty_input: String,
    pub visible_bars: usize,
    pub show_ma: bool,
    pub backtest: Option<BacktestResult>,
    notice: Notice,
    applied_dark: Option<bool>,
    prev_close_seen: BTreeMap<String, Price>,
    last_frame_time: Option<f64>,
    accumulator: f64,
}

impl TradingApp {
    fn build(
        market: Market,
        dark: bool,
        tab: Tab,
        playing: bool,
        speed: f32,
        notice: Notice,
    ) -> Self {
        let rules = rules_for(market);
        let source = SyntheticSource::new(market, DATA_SEED);
        let instruments: Vec<Instrument> = source.instruments().to_vec();
        let mut engine = MatchingEngine::new(rules.clone(), INITIAL_CASH);
        let mut prev_close_seen = BTreeMap::new();
        for inst in &instruments {
            engine.register_symbol(&inst.symbol, inst.seed_price);
            prev_close_seen.insert(inst.symbol.clone(), inst.seed_price);
        }
        let first = &instruments[0];
        let selected = first.symbol.clone();
        engine.refresh_liquidity(&selected, first.seed_price);
        Self {
            market,
            rules,
            source,
            engine,
            selected,
            playing,
            speed,
            dark,
            tab,
            order_side: Side::Buy,
            order_kind: OrderType::Limit,
            price_input: format!("{:.2}", rules_for(market).price_to_f64(first.seed_price)),
            qty_input: rules_for(market).lot_size(&first.symbol).to_string(),
            visible_bars: 80,
            show_ma: true,
            backtest: None,
            notice,
            applied_dark: None,
            prev_close_seen,
            last_frame_time: None,
            accumulator: 0.0,
        }
    }

    pub fn new(market: Market) -> Self {
        Self::build(
            market,
            true,
            Tab::Orders,
            true,
            2.0,
            Notice {
                text: "模拟盘已就绪 · 行情为确定性合成数据，非实盘交易".into(),
                is_error: false,
            },
        )
    }

    pub fn switch_market(&mut self, market: Market) {
        let notice = Notice {
            text: format!("已切换至 {} · 账户与持仓已重置", market.label()),
            is_error: false,
        };
        *self = Self::build(
            market,
            self.dark,
            self.tab,
            self.playing,
            self.speed,
            notice,
        );
    }

    pub fn reset_account(&mut self) {
        let notice = Notice {
            text: "模拟账户已重置：资金恢复初始值，委托与持仓清空".into(),
            is_error: false,
        };
        *self = Self::build(
            self.market,
            self.dark,
            self.tab,
            self.playing,
            self.speed,
            notice,
        );
    }

    pub fn roll_day(&mut self) {
        self.engine.roll_day();
        for (symbol, price) in &self.engine.last_prices {
            self.prev_close_seen.insert(symbol.clone(), *price);
        }
        self.notice = Notice {
            text: format!("已进入交易日 D{} · T+1 持仓已释放", self.engine.day_index),
            is_error: false,
        };
    }

    fn set_notice(&mut self, text: impl Into<String>, is_error: bool) {
        self.notice = Notice {
            text: text.into(),
            is_error,
        };
    }

    /// Advance the simulated market based on wall-clock frame time.
    pub fn advance_market(&mut self, ctx: &Context) {
        let now = ctx.input(|i| i.time);
        let dt = match self.last_frame_time {
            Some(previous) => (now - previous).clamp(0.0, 0.25),
            None => 0.0,
        };
        self.last_frame_time = Some(now);
        if !self.playing {
            return;
        }
        self.accumulator += dt * self.speed as f64;
        let mut steps = 0;
        let mut day_rolled = false;
        while self.accumulator >= 1.0 && steps < 8 {
            self.accumulator -= 1.0;
            steps += 1;
            let symbols: Vec<String> = self.source.symbols();
            let mut ticks = Vec::with_capacity(symbols.len());
            for symbol in &symbols {
                let tick = self.source.next_tick(symbol);
                let seen = self
                    .prev_close_seen
                    .get(symbol)
                    .copied()
                    .unwrap_or(tick.prev_close);
                if seen != tick.prev_close {
                    day_rolled = true;
                }
                ticks.push((symbol.clone(), tick));
            }
            for (symbol, tick) in ticks {
                self.engine.set_price(&symbol, tick.last);
                self.prev_close_seen.insert(symbol, tick.prev_close);
            }
        }
        if day_rolled {
            self.engine.roll_day();
        }
        if steps > 0 {
            let mid = self
                .engine
                .last_prices
                .get(&self.selected)
                .copied()
                .unwrap_or(1);
            self.engine.refresh_liquidity(&self.selected, mid);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }

    fn parse_order_inputs(&self) -> Result<(Price, Qty), String> {
        let price = if self.order_kind == OrderType::Market {
            0
        } else {
            let value = self
                .price_input
                .trim()
                .parse::<f64>()
                .map_err(|_| "委托价格格式不正确".to_string())?;
            if value <= 0.0 {
                return Err("委托价格必须大于 0".to_string());
            }
            self.rules.price_from_f64(value)
        };
        let qty = self
            .qty_input
            .trim()
            .parse::<Qty>()
            .map_err(|_| "委托数量必须是整数".to_string())?;
        Ok((price, qty))
    }

    pub fn submit_order(&mut self) {
        let (price, qty) = match self.parse_order_inputs() {
            Ok(v) => v,
            Err(e) => {
                self.set_notice(e, true);
                return;
            }
        };
        let request = OrderRequest {
            symbol: Symbol::new(self.selected.clone()),
            side: self.order_side,
            kind: self.order_kind,
            price,
            qty,
        };
        match self.engine.submit_user_order(request) {
            Ok(id) => self.set_notice(
                format!(
                    "委托 #{id} 已提交 · {} {} {} 股",
                    self.order_side.label(),
                    self.selected,
                    qty
                ),
                false,
            ),
            Err(err) => self.set_notice(format!("下单被拒绝：{}", err.message()), true),
        }
    }

    pub fn cancel(&mut self, id: OrderId) {
        match self.engine.cancel_order(id) {
            Ok(()) => self.set_notice(format!("委托 #{id} 已撤销"), false),
            Err(err) => self.set_notice(format!("撤单失败：{}", err.message()), true),
        }
    }

    pub fn quick_qty(&mut self, fraction: f32) {
        let lot = self.rules.lot_size(&self.selected).max(1);
        let last = self
            .engine
            .last_prices
            .get(&self.selected)
            .copied()
            .unwrap_or(1);
        let raw = match self.order_side {
            Side::Buy => {
                let price_minor = self.rules.price_to_minor(last).max(1);
                ((self.engine.account.cash as f64 / price_minor as f64) * fraction as f64) as Qty
            }
            Side::Sell => {
                (self.engine.account.available(&self.selected) as f64 * fraction as f64) as Qty
            }
        };
        self.qty_input = ((raw / lot) * lot).max(0).to_string();
    }

    pub fn run_backtest_now(&mut self) {
        let bars = self.source.history(&self.selected, 180);
        let mut strategy = SmaCrossStrategy::new(5, 20);
        let result = run_backtest(
            &bars,
            &Symbol::new(self.selected.clone()),
            self.rules.clone(),
            &mut strategy,
        );
        self.set_notice(
            format!(
                "回测完成 · {} 笔成交 · 总收益 {:.2}% · 最大回撤 {:.2}%",
                result.metrics.trade_count,
                result.metrics.total_return_pct,
                result.metrics.max_drawdown_pct
            ),
            false,
        );
        self.backtest = Some(result);
    }

    /// Draw one full frame. Safe to call with a bare `egui::Context` (no window).
    pub fn render(&mut self, ctx: &Context) {
        if self.applied_dark != Some(self.dark) {
            theme::apply_visuals(ctx, self.dark);
            self.applied_dark = Some(self.dark);
        }
        let palette = Palette::new(self.dark, self.rules.as_ref());
        let rules = self.rules.clone();
        let instruments: Vec<Instrument> = self.source.instruments().to_vec();
        let candles: Vec<Candle> = self.source.candles(&self.selected).to_vec();
        let selected = self.selected.clone();
        let selected_name = self
            .source
            .instrument(&selected)
            .map(|i| i.name.clone())
            .unwrap_or_default();
        let prev_close = self
            .engine
            .prev_closes
            .get(&selected)
            .copied()
            .unwrap_or(1);
        let (lower, upper) = rules.limit_range(prev_close);
        let band_text = if rules.price_limit_ratio().is_some() {
            format!("{} ~ {}", rules.format_price(lower), rules.format_price(upper))
        } else {
            "无涨跌停限制".to_owned()
        };
        let last_price = self
            .engine
            .last_prices
            .get(&selected)
            .copied()
            .unwrap_or(1);
        let (best_bid, best_ask) = self
            .engine
            .book(&selected)
            .map(|b| (b.best_bid(), b.best_ask()))
            .unwrap_or((None, None));
        let account = self.engine.account.clone();
        let equity = account.equity(rules.as_ref());
        let market_value = account.market_value(rules.as_ref());
        let unrealized = account.unrealized_pnl(rules.as_ref());
        let realized = account.realized_pnl();
        let day_index = self.engine.day_index;
        let active_orders = self.engine.active_user_order_count();
        let notice = self.notice.clone();
        let quote_rows: Vec<(String, String, Price, f64, Price)> = instruments
            .iter()
            .map(|i| {
                let last = self
                    .engine
                    .last_prices
                    .get(&i.symbol)
                    .copied()
                    .unwrap_or(i.seed_price);
                let prev = self
                    .engine
                    .prev_closes
                    .get(&i.symbol)
                    .copied()
                    .unwrap_or(last);
                let change = last - prev;
                let pct = if prev == 0 {
                    0.0
                } else {
                    change as f64 / prev as f64 * 100.0
                };
                (i.symbol.clone(), i.name.clone(), last, pct, change)
            })
            .collect();
        let mut orders: Vec<Order> = self.engine.user_orders().cloned().collect();
        orders.reverse();
        let mut trades: Vec<Trade> = self.engine.user_trades().cloned().collect();
        trades.reverse();
        let equity_curve: Vec<Money> = self
            .backtest
            .as_ref()
            .map(|r| r.equity_curve.clone())
            .unwrap_or_default();
        let metrics: Option<Metrics> = self.backtest.as_ref().map(|r| r.metrics.clone());

        let mut actions = Actions {
            market: None,
            selected: None,
            toggle_play: false,
            toggle_dark: false,
            roll_day: false,
            reset: false,
            submit: false,
            cancel: None,
            quick: None,
            run_backtest: false,
            tab: self.tab,
            show_ma: self.show_ma,
            visible_bars: self.visible_bars,
        };
        let playing = self.playing;
        let mut speed = self.speed;
        let mut price_input = self.price_input.clone();
        let mut qty_input = self.qty_input.clone();
        let mut order_side = self.order_side;
        let mut order_kind = self.order_kind;
        let mut selected_local = selected.clone();

        // ------------------------------- top bar ----------------------------
        egui::TopBottomPanel::top("tdesk_top")
            .exact_height(46.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("tdesk")
                            .size(18.0)
                            .strong()
                            .color(palette.accent),
                    );
                    ui.label(
                        RichText::new("模拟盘 · 非实盘")
                            .size(10.0)
                            .strong()
                            .color(palette.warn),
                    );
                    ui.separator();
                    for m in Market::ALL {
                        if ui
                            .selectable_label(self.market == m, RichText::new(m.label()).size(11.0))
                            .clicked()
                        {
                            actions.market = Some(m);
                        }
                    }
                    ui.separator();
                    if ui
                        .button(if playing { "⏸ 暂停" } else { "▶ 播放" })
                        .clicked()
                    {
                        actions.toggle_play = true;
                    }
                    ui.add(
                        egui::Slider::new(&mut speed, 1.0..=10.0)
                            .text("tick/s")
                            .clamping(egui::SliderClamping::Always),
                    );
                    if ui.button("推进一日").clicked() {
                        actions.roll_day = true;
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.button(if self.dark { "☀ 明亮" } else { "🌙 深色" }).clicked() {
                                actions.toggle_dark = true;
                            }
                            if ui.button("重置账户").clicked() {
                                actions.reset = true;
                            }
                            ui.label(
                                RichText::new(format!(
                                    "交易日 D{} · 活动委托 {}",
                                    day_index, active_orders
                                ))
                                .size(10.0)
                                .color(palette.muted),
                            );
                        },
                    );
                });
            });

        // ------------------------------ watchlist ---------------------------
        egui::SidePanel::left("tdesk_watchlist")
            .exact_width(228.0)
            .resizable(false)
            .show(ctx, |ui| {
                panels::section_label(ui, "自选列表", &palette);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (symbol, name, last, change_pct, change) in &quote_rows {
                            let is_selected = *symbol == selected_local;
                            let color = palette.change_color(*change as f64);
                            egui::Frame::NONE
                                .fill(if is_selected {
                                    palette.histogram
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .corner_radius(4.0)
                                .inner_margin(egui::vec2(6.0, 4.0))
                                .show(ui, |ui| {
                                    let width = ui.available_width();
                                    ui.set_width(width);
                                    if ui
                                        .selectable_label(
                                            is_selected,
                                            RichText::new(symbol.as_str()).size(12.0).strong(),
                                        )
                                        .clicked()
                                    {
                                        actions.selected = Some(symbol.clone());
                                        selected_local = symbol.clone();
                                    }
                                    ui.label(
                                        RichText::new(name.as_str()).size(10.0).color(palette.muted),
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(rules.format_price(*last))
                                                .size(13.0)
                                                .strong()
                                                .color(color),
                                        );
                                        ui.label(
                                            RichText::new(format!("{:+.2}%", change_pct))
                                                .size(11.0)
                                                .color(color),
                                        );
                                    });
                                });
                            ui.separator();
                        }
                    });
                ui.add_space(6.0);
                panels::section_label(ui, "市场规则", &palette);
                ui.label(
                    RichText::new(format!(
                        "{} · {}\nT+{} · {}股/手\n{}\n涨跌停 {}\n{}",
                        rules.currency(),
                        if rules.rise_is_red() {
                            "红涨绿跌"
                        } else {
                            "绿涨红跌"
                        },
                        rules.settlement_lag_days(),
                        rules.lot_size(&selected),
                        rules.fees().label(),
                        band_text,
                        if best_bid.is_some() { "盘口流动性已就绪" } else { "等待刷新盘口" }
                    ))
                    .size(10.0)
                    .color(palette.muted),
                );
            });

        // ---------------------------- order entry ---------------------------
        egui::SidePanel::right("tdesk_order")
            .exact_width(302.0)
            .resizable(false)
            .show(ctx, |ui| {
                panels::section_label(ui, &format!("下单 · {}", selected), &palette);
                ui.horizontal(|ui| {
                    for (side, label) in [(Side::Buy, "买入"), (Side::Sell, "卖出")] {
                        let color = if side == Side::Buy {
                            palette.up
                        } else {
                            palette.down
                        };
                        let selected_side = order_side == side;
                        if ui
                            .add_sized(
                                [136.0, 28.0],
                                egui::Button::new(
                                    RichText::new(label).strong().color(if selected_side {
                                        Color32::WHITE
                                    } else {
                                        color
                                    }),
                                )
                                .fill(if selected_side {
                                    color
                                } else {
                                    palette.panel
                                }),
                            )
                            .clicked()
                        {
                            order_side = side;
                            actions.quick = None;
                        }
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut order_kind, OrderType::Limit, "限价单");
                    ui.selectable_value(&mut order_kind, OrderType::Market, "市价单");
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("价格").size(11.0).color(palette.muted));
                    ui.add_enabled(
                        order_kind == OrderType::Limit,
                        egui::TextEdit::singleline(&mut price_input).desired_width(150.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("数量").size(11.0).color(palette.muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut qty_input).desired_width(150.0),
                    );
                });
                ui.horizontal(|ui| {
                    for (label, fraction) in [("25%", 0.25_f32), ("50%", 0.5_f32), ("全部", 1.0_f32)] {
                        if ui.button(label).clicked() {
                            actions.quick = Some(fraction);
                        }
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "买一 {} / 卖一 {}\n可卖 {} 股 · 可用 {}\n涨跌停 {}",
                        best_bid.map(|p| rules.format_price(p)).unwrap_or_else(|| "—".into()),
                        best_ask.map(|p| rules.format_price(p)).unwrap_or_else(|| "—".into()),
                        account.available(&selected),
                        rules.format_money(account.cash),
                        band_text,
                    ))
                    .size(10.0)
                    .color(palette.muted),
                );
                ui.add_space(8.0);
                let submit_color = if order_side == Side::Buy {
                    palette.up
                } else {
                    palette.down
                };
                if ui
                    .add_sized(
                        [ui.available_width(), 34.0],
                        egui::Button::new(
                            RichText::new(format!("提交{}委托", order_side.label()))
                                .strong()
                                .size(14.0)
                                .color(Color32::WHITE),
                        )
                        .fill(submit_color),
                    )
                    .clicked()
                {
                    actions.submit = true;
                }
                ui.add_space(6.0);
                egui::Frame::NONE
                    .fill(palette.panel)
                    .corner_radius(4.0)
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&notice.text)
                                .size(10.0)
                                .color(if notice.is_error {
                                    palette.down
                                } else {
                                    palette.accent
                                }),
                        );
                    });
                ui.add_space(10.0);
                panels::section_label(ui, "账户概览", &palette);
                ui.columns(2, |cols| {
                    panels::metric_card(
                        &mut cols[0],
                        "总权益",
                        &rules.format_money(equity),
                        palette.text,
                        &palette,
                    );
                    panels::metric_card(
                        &mut cols[1],
                        "可用资金",
                        &rules.format_money(account.cash),
                        palette.text,
                        &palette,
                    );
                    panels::metric_card(
                        &mut cols[0],
                        "持仓市值",
                        &rules.format_money(market_value),
                        palette.text,
                        &palette,
                    );
                    panels::metric_card(
                        &mut cols[1],
                        "浮动盈亏",
                        &rules.format_signed_money(unrealized),
                        palette.change_color(unrealized as f64),
                        &palette,
                    );
                    panels::metric_card(
                        &mut cols[0],
                        "已实现盈亏",
                        &rules.format_signed_money(realized),
                        palette.change_color(realized as f64),
                        &palette,
                    );
                    panels::metric_card(
                        &mut cols[1],
                        "累计手续费",
                        &rules.format_money(account.total_fees),
                        palette.warn,
                        &palette,
                    );
                });
            });

        // ------------------------------ bottom ------------------------------
        egui::TopBottomPanel::bottom("tdesk_bottom")
            .resizable(true)
            .default_height(238.0)
            .min_height(150.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for tab in Tab::ALL {
                        if ui
                            .selectable_label(actions.tab == tab, tab.label())
                            .clicked()
                        {
                            actions.tab = tab;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut actions.show_ma, "显示 MA5 / MA20");
                        ui.add(
                            egui::Slider::new(&mut actions.visible_bars, 30..=160)
                                .text("K线根数"),
                        );
                    });
                });
                ui.separator();
                match actions.tab {
                    Tab::Orders => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                egui::Grid::new("orders_grid")
                                    .striped(true)
                                    .spacing([12.0, 4.0])
                                    .show(ui, |ui| {
                                        for h in [
                                            "单号", "标的", "方向", "类型", "委托价", "数量", "已成交",
                                            "状态", "操作",
                                        ] {
                                            ui.label(
                                                RichText::new(h).size(10.0).strong().color(palette.muted),
                                            );
                                        }
                                        ui.end_row();
                                        if orders.is_empty() {
                                            ui.label(RichText::new("暂无委托").color(palette.muted));
                                            ui.end_row();
                                        }
                                        for o in &orders {
                                            let color = match o.status {
                                                OrderStatus::Rejected => palette.down,
                                                OrderStatus::Filled => palette.up,
                                                OrderStatus::Pending
                                                | OrderStatus::PartiallyFilled => palette.accent,
                                                OrderStatus::Cancelled => palette.muted,
                                            };
                                            for cell in [
                                                format!("#{}", o.id),
                                                o.symbol.as_str().to_owned(),
                                                o.side.label().to_owned(),
                                                o.kind.label().to_owned(),
                                                if o.kind == OrderType::Market {
                                                    "市价".to_owned()
                                                } else {
                                                    rules.format_price(o.price)
                                                },
                                                o.qty.to_string(),
                                                o.filled_qty.to_string(),
                                                o.status.label().to_owned(),
                                            ] {
                                                ui.label(
                                                    RichText::new(cell).size(11.0).color(color),
                                                );
                                            }
                                            if o.is_active() {
                                                if ui.button("撤单").clicked() {
                                                    actions.cancel = Some(o.id);
                                                }
                                            } else {
                                                ui.label(
                                                    RichText::new("—").size(11.0).color(palette.muted),
                                                );
                                            }
                                            ui.end_row();
                                            if let Some(reason) = &o.reject_reason {
                                                ui.label(
                                                    RichText::new(format!("↳ {reason}"))
                                                        .size(10.0)
                                                        .color(palette.down),
                                                );
                                                for _ in 0..8 {
                                                    ui.label("");
                                                }
                                                ui.end_row();
                                            }
                                        }
                                    });
                            });
                    }
                    Tab::Trades => {
                        let rows: Vec<Vec<(String, Color32)>> = trades
                            .iter()
                            .map(|t| {
                                let color = if t.taker_side == Side::Buy {
                                    palette.up
                                } else {
                                    palette.down
                                };
                                vec![
                                    (format!("#{}", t.id), palette.text),
                                    (t.symbol.as_str().to_owned(), palette.text),
                                    (t.taker_side.label().to_owned(), color),
                                    (rules.format_price(t.price), color),
                                    (t.qty.to_string(), palette.text),
                                    (rules.format_money(t.fee), palette.warn),
                                    (format!("t={}", t.ts), palette.muted),
                                    (format!("对手 #{}", t.maker_order_id), palette.muted),
                                ]
                            })
                            .collect();
                        panels::table(
                            ui,
                            "trades_table",
                            &["成交号", "标的", "方向", "成交价", "数量", "手续费", "时间", "对手单"],
                            &rows,
                            &palette,
                        );
                    }
                    Tab::Positions => {
                        let rows: Vec<Vec<(String, Color32)>> = account
                            .positions
                            .values()
                            .map(|p| {
                                let pnl = p.unrealized_pnl(rules.as_ref());
                                let color = palette.change_color(pnl as f64);
                                vec![
                                    (p.symbol.as_str().to_owned(), palette.text),
                                    (p.qty.to_string(), palette.text),
                                    (p.available.to_string(), palette.accent),
                                    (p.locked_today.to_string(), palette.warn),
                                    (rules.format_price(p.avg_cost().max(0)), palette.text),
                                    (rules.format_price(p.last_price), palette.text),
                                    (rules.format_money(p.market_value(rules.as_ref())), palette.text),
                                    (rules.format_signed_money(pnl), color),
                                    (rules.format_signed_money(p.realized_pnl), color),
                                ]
                            })
                            .collect();
                        panels::table(
                            ui,
                            "positions_table",
                            &[
                                "标的", "持仓", "可卖", "T+1锁定", "成本价", "最新价", "市值", "浮盈", "已实现",
                            ],
                            &rows,
                            &palette,
                        );
                    }
                    Tab::Account => {
                        ui.horizontal_wrapped(|ui| {
                            for (label, value, color) in [
                                ("期初资金", rules.format_money(account.initial_cash), palette.text),
                                ("可用资金", rules.format_money(account.cash), palette.text),
                                ("持仓市值", rules.format_money(market_value), palette.text),
                                ("总权益", rules.format_money(equity), palette.accent),
                                (
                                    "浮动盈亏",
                                    rules.format_signed_money(unrealized),
                                    palette.change_color(unrealized as f64),
                                ),
                                (
                                    "已实现盈亏",
                                    rules.format_signed_money(realized),
                                    palette.change_color(realized as f64),
                                ),
                                ("累计手续费", rules.format_money(account.total_fees), palette.warn),
                                (
                                    "总收益率",
                                    format!(
                                        "{:+.2}%",
                                        (equity - account.initial_cash) as f64
                                            / account.initial_cash as f64
                                            * 100.0
                                    ),
                                    palette.change_color((equity - account.initial_cash) as f64),
                                ),
                            ] {
                                panels::metric_card(ui, label, &value, color, &palette);
                            }
                        });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "账户恒等式：现金 {} + 持仓市值 {} = 总权益 {}   ·   交易日 D{}",
                                rules.format_money(account.cash),
                                rules.format_money(market_value),
                                rules.format_money(equity),
                                day_index
                            ))
                            .size(11.0)
                            .color(palette.muted),
                        );
                    }
                    Tab::Backtest => {
                        ui.horizontal(|ui| {
                            if ui.button("▶ 运行 SMA(5,20) 回测").clicked() {
                                actions.run_backtest = true;
                            }
                            if let Some(m) = &metrics {
                                ui.label(
                                    RichText::new(format!(
                                        "成交 {} 笔 · 总收益 {:+.2}% · 最大回撤 {:.2}% · 胜率 {:.1}% · 夏普 {:.2}",
                                        m.trade_count,
                                        m.total_return_pct,
                                        m.max_drawdown_pct,
                                        m.win_rate_pct,
                                        m.sharpe
                                    ))
                                    .size(11.0)
                                    .color(palette.text),
                                );
                            } else {
                                ui.label(
                                    RichText::new("信号 T 日收盘产生，T+1 开盘成交（无未来函数）")
                                        .size(10.0)
                                        .color(palette.muted),
                                );
                            }
                        });
                        ui.add_space(4.0);
                        charts::equity_chart(ui, &equity_curve, rules.as_ref(), &palette);
                    }
                }
            });

        // ------------------------------- chart ------------------------------
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{selected}  {selected_name}"))
                        .size(15.0)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "最新 {} · 前收 {}",
                        rules.format_price(last_price),
                        rules.format_price(prev_close)
                    ))
                    .size(11.0)
                    .color(palette.muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("左键悬停查看 OHLC · 颜色随市场规则切换")
                            .size(10.0)
                            .color(palette.muted),
                    );
                });
            });
            charts::candle_chart(
                ui,
                &candles,
                rules.as_ref(),
                &palette,
                actions.visible_bars,
                actions.show_ma,
                &selected,
            );
        });

        // --------------------------- write back -----------------------------
        self.playing = playing;
        self.speed = speed;
        self.price_input = price_input;
        self.qty_input = qty_input;
        self.order_side = order_side;
        self.order_kind = order_kind;
        self.tab = actions.tab;
        self.show_ma = actions.show_ma;
        self.visible_bars = actions.visible_bars;
        if actions.toggle_play {
            self.playing = !self.playing;
        }
        if actions.toggle_dark {
            self.dark = !self.dark;
        }
        if let Some(symbol) = actions.selected {
            self.select_symbol(&symbol);
        }
        if actions.roll_day {
            self.roll_day();
        }
        if actions.reset {
            self.reset_account();
        }
        if let Some(m) = actions.market {
            if m != self.market {
                self.switch_market(m);
                return;
            }
        }
        if let Some(fraction) = actions.quick {
            self.quick_qty(fraction);
        }
        if let Some(id) = actions.cancel {
            self.cancel(id);
        }
        if actions.submit {
            self.submit_order();
        }
        if actions.run_backtest {
            self.run_backtest_now();
        }
    }

    fn select_symbol(&mut self, symbol: &str) {
        let Some(inst) = self.source.instrument(symbol).cloned() else {
            return;
        };
        let last = self
            .engine
            .last_prices
            .get(symbol)
            .copied()
            .unwrap_or(inst.seed_price);
        self.selected = symbol.to_owned();
        self.price_input = self.rules.format_price(last);
        self.qty_input = self.rules.lot_size(symbol).to_string();
        self.engine.refresh_liquidity(symbol, last);
        self.backtest = None;
        self.set_notice(format!("已切换标的 {symbol}"), false);
    }
}

impl eframe::App for TradingApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.advance_market(ctx);
        self.render(ctx);
    }
}

/// Helper used by the headless smoke test to exercise a whole interaction
/// sequence without opening a native window.
pub fn scripted_smoke_test(app: &mut TradingApp, ctx: &Context) {
    let _ = ctx.run(RawInput::default(), |ctx| app.render(ctx));
    app.quick_qty(0.25);
    app.submit_order();
    assert!(!app.engine.orders.is_empty(), "submitting should create an order");
    app.roll_day();
    app.run_backtest_now();
    assert!(app.backtest.is_some(), "backtest should produce a result");
    app.switch_market(Market::Us);
    let _ = ctx.run(RawInput::default(), |ctx| app.render(ctx));
    app.switch_market(Market::Hk);
    let _ = ctx.run(RawInput::default(), |ctx| app.render(ctx));
    assert_eq!(app.market, Market::Hk);
}
