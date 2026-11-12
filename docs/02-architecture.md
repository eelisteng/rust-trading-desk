# 架构设计说明书 — `tdesk`

## 1. 架构总览

采用**分层 + 六边形（端口与适配器）**混合：核心域完全不依赖 UI 与数据源实现，外部能力通过 trait 注入。

```
┌──────────────────────────────────────────────────────────────┐
│  表现层  tdesk-app (eframe/egui)      tdesk-cli (headless)      │
│   panels.rs / charts.rs / theme.rs / app.rs                      │
└───────────────┬──────────────────────────┬───────────────────┘
                │ 只依赖 trait 与领域类型      │
┌───────────────▼──────────────────────────▼───────────────────┐
│  适配层  tdesk-data                                             │
│   MarketDataSource ── SyntheticSource / CsvSource               │
│   SplitMix64 RNG（确定性、零外部依赖）                            │
└───────────────────────────┬──────────────────────────────────┘
                            │
┌───────────────────────────▼──────────────────────────────────┐
│  领域层  tdesk-core（无 UI、无 IO、无系统时间）                    │
│   types / market / account / risk / engine / backtest / error    │
│   MarketRules ◄── AShareRules / UsRules / HkRules                │
│   Strategy    ◄── SmaCrossStrategy                              │
└──────────────────────────────────────────────────────────────┘
```

依赖方向严格单向：`app|cli → data → core`。`core` 的 `Cargo.toml` 只有标准库。

## 2. Crate 划分

| Crate | 职责 | 关键公开类型 | 外部依赖 |
| --- | --- | --- | --- |
| `tdesk-core` | 领域模型、撮合、结算、风控、回测、**可插拔 Broker、实时交易回路、量化策略** | `MatchingEngine`, `Account`, `MarketRules`, `Strategy`, `run_backtest`, `Broker`, `PaperBroker`, `LiveTrader`, `LiveStrategy`, `DualMaStrategy`, `BreakoutStrategy`, `MeanReversionStrategy` | 无 |
| `tdesk-data` | 行情端口与实现 | `MarketDataSource`, `SyntheticSource`, `CsvSource`, `Instrument` | `tdesk-core` |
| `tdesk-app` | 桌面 GUI | `TradingApp::render`, `CandleChart` | `egui`, `eframe`, 上述二者 |
| `tdesk-cli` | 无头端到端演示与基准 | `main` | `tdesk-core`, `tdesk-data` |

## 3. 关键设计决策（ADR）

### ADR-1：整数价格与整数金额

- `Price = i64`，单位是「最小价位整数倍」（tick）。`A 股 1685.00` → `168500`。
- `Money = i64`，单位是「货币最小单位」（分）。
- 换算：`price_minor = Price × MarketRules::tick_minor()`；名义金额 `= Price × tick_minor × Qty`。
- **理由**：避免 `f64` 累加误差导致资金对账不平；撮合簿用 `BTreeMap<Price, …>` 可直接获得价格优先，无需浮点比较。
- **代价**：展示层需 `format_price` 转换；港股价位阶梯暂简化。

### ADR-2：`MarketRules` trait 而非枚举分支

```rust
pub trait MarketRules: Send + Sync {
    fn market(&self) -> Market;
    fn tick_minor(&self) -> Money;
    fn lot_size(&self, symbol: &str) -> Qty;
    fn price_limit_ratio(&self) -> Option<f64>;
    fn settlement_lag_days(&self) -> u8;
    fn rise_is_red(&self) -> bool;
    fn fees(&self) -> FeeSchedule;
    fn limit_range(&self, prev_close: Price) -> (Price, Price);
    // …格式化、换算等默认方法
}
```

- 撮合引擎持有 `Arc<dyn MarketRules>`；新增市场只需新增实现 + 注册到 `rules_for()`。
- 涨跌颜色以 `ColorScheme::up_color(rules)` 统一计算，UI 不做 `if market == …`。

### ADR-3：统一订单簿 + 做市商流动性

```
bids: BTreeMap<Price, VecDeque<OrderId>>   // 升序，最优 = next_back()
asks: BTreeMap<Price, VecDeque<OrderId>>   // 升序，最优 = next()
orders: BTreeMap<OrderId, Order>           // 用户单与做市商单同簿
```

- 每次 tick 调 `refresh_liquidity(symbol, mid)`：撤销该标的全部 Maker 挂单，在 mid±1..±N 档重挂；用户挂单保留。
- 买单吃 `asks` 中 `ask ≤ 限价` 的档位；卖单吃 `bids` 中 `bid ≥ 限价` 的档位；成交价取 **maker 挂单价**。
- 同账户自成交：撤销较旧的对手挂单，不生成 Trade。
- **理由**：同一套撮合代码同时服务 UI 实时模拟与自动化测试，避免"演示版假撮合"。

### ADR-4：风控前置、账户零副作用

`submit_user_order` 顺序：`构造请求 → risk::validate（只读） → 写入订单 → 撮合 → 结算`。校验全部在写入前完成，任何 `RiskError` 返回时账户与订单簿完全不变。

### ADR-5：UI 渲染与 `eframe::App` 解耦

```rust
impl TradingApp {
    pub fn render(&mut self, ctx: &egui::Context) { /* 所有面板 */ }
}
impl eframe::App for TradingApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.advance_market(ctx);
        self.render(ctx);
    }
}
```

测试可用 `egui::Context::default().run(RawInput::default(), |ctx| app.render(ctx))` 在**无窗口**环境渲染一帧，捕获 panic 与布局溢出。

### ADR-6：回测无未来函数

信号在第 `i` 根 K 线**收盘后**产生，成交在第 `i+1` 根 **开盘价**，费用/T+N 结算复用 `Account` 与 `FeeSchedule`。每根 K 线视为一个交易日，逐日 `roll_day()`。

### ADR-7：可插拔 `Broker` 端口 + 统一策略框架（纸面/实盘同构）

```
            行情 tick (MarketDataSource / 真实行情适配器)
                          │  Tick { last, bid, ask, prev_close }
                          ▼
                   LiveTrader::on_quote(symbol, &tick)
                          │  ① broker.set_price(...)  ② strategy.on_quote(ctx) → Option<Signal>
                          ▼
             规模 sizing：size_buy / size_sell（现金·lot·participation 单一出处）
                          │
                          ▼
                  broker.submit(OrderRequest)  ──►  Broker trait
                          │                     PaperBroker ── 内置撮合引擎（模拟盘）
                          │                     LiveBroker  ── 实盘适配器骨架（未配置）
                          ▼
                  成交回报 → 账户结算 → 持仓/盈亏
```

- **`Broker` trait** 是下单/撤单/查持仓/查资金的唯一出口，与撮合引擎解耦。`PaperBroker` 直接包裹现有 `MatchingEngine`，因此"原有模拟交易"原样保留并升格为可切换后端；`set_price` 时刷新做市商盘口，使市价单可立即成交。
- **`LiveBroker` 是刻意的空壳**：东方财富证券等**没有面向散户的官方交易 API**，真实下单需走量化平台网关（QMT / Ptrade / 掘金）或客户端自动化（easytrader 风格），均存在合规/封号/实盘资金风险。故实盘适配器**默认不实现**，任何 `submit` 都显式返回 `BrokerError::NotConfigured`，须经明确评估与凭据隔离后再落地。
- **策略同构**：每个策略同时实现 `Strategy`（回测）与 `LiveStrategy`（实时），复用同一段内部价格缓冲与 `sma`/`mean_std` 计算，保证"回测里调好的参数，实盘纸面行为一致"。首版内置三类：`DualMaStrategy`（双均线金叉死叉）、`BreakoutStrategy`（N 日高低点突破·动量）、`MeanReversionStrategy`（均值回归·布林）。
- **纸面优先**：`LiveTrader` 对纸面与实盘完全一致，仅 `Broker` 实现不同；策略与风控逻辑在仿真环境跑通后再接真实通道，避免真金白银试错。

## 4. 核心流程

### 4.1 下单时序

```
UI 下单面板
   │ OrderRequest { symbol, side, kind, price, qty }
   ▼
MatchingEngine::submit_user_order
   ├─ risk::validate(engine, req)  ──失败──► Err(RiskError) → UI 显示中文原因
   ├─ 生成 OrderId / Order(owner=User, status=Pending)
   ├─ cross(&mut order)  价格时间优先，产出 Vec<Trade>
   │    └─ 每个 Trade 计算费用、更新 maker/taker 数量与状态
   ├─ 剩余量：限价单 → 挂入订单簿；市价单 → 撤销剩余
   ├─ orders.insert / trades.extend
   └─ account.apply_fills(&trades, rules, day_index)
            ├─ Buy : cash -= 名义额+费；qty/available/locked_today += …
            └─ Sell: cash += 名义额-费；结转已实现盈亏；available -= …
```

### 4.2 行情 tick 时序

```
SyntheticSource::next_tick(symbol)
   ├─ 价格随机游走 → Tick { last, bid, ask, volume }
   ├─ 更新当前未完成 Candle（high/low/close/volume）
   └─ 满 N tick → 收盘并压入 history
        ▼
TradingApp::advance_market
   ├─ engine.set_price(symbol, last, prev_close)
   ├─ engine.refresh_liquidity(symbol, last)
   │      └─ 可能触发用户挂单成交（价格穿越限价）
   └─ engine.refresh_positions_mark_to_market()
```

### 4.3 换日（T+1）

`engine.roll_day()` → `prev_close = last`；`account.roll_day()` → `available += locked_today; locked_today = 0`；`day_index += 1`。涨跌停区间随之按新前收盘重算。

### 4.4 实时自动交易回路（纸面/实盘同构）

```
每收到一个行情 tick：
  LiveTrader::on_quote(symbol, &Tick)
    1. broker.set_price(symbol, tick.last)         // 纸面：刷新盘口，使市价单可成交
    2. ctx = LiveContext{ price, prev_close, position, available, cash, rules }
    3. signal = strategy.on_quote(&ctx)            // 双均线/突破/均值回归 → Option<Signal>
    4. qty = size_buy(cash, price_minor, participation, lot)  // 或 size_sell(available)
    5. broker.submit(OrderRequest{ kind: Market, … })  → LiveAction::{OrderSubmitted|Rejected|Insufficient|NoSignal}
    6. 可选 trader.roll_day() 推进交易日（触发 T+1 释放）
```

- 回路与撮合引擎、风控完全解耦；接入真实券商只需新增一个 `impl Broker`（见 ADR-7）。
- 规模计算在 `live::size_buy/size_sell` 单点收口，所有策略与回测共用，避免现金/手数/仓位比例分散在多处。

## 5. 状态机

```
                ┌──────────── 撤单 / 市价单剩余撤销 ───────────► Cancelled
                │
 Pending ──部分成交──► PartiallyFilled ──全部成交──► Filled
    │                        │
    └──────全部成交──────────┘
    │
    └─ 风控失败 ─► Rejected（记录 reject_reason，不入簿）
```

## 6. 模块接口速查

| 接口 | 签名 | 说明 |
| --- | --- | --- |
| 规则工厂 | `rules_for(Market) -> Arc<dyn MarketRules>` | 返回对应市场实现 |
| 行情端口 | `MarketDataSource::history(&self, &str, usize) -> Vec<Candle>` | 确定性历史 |
| 行情端口 | `MarketDataSource::next_tick(&mut self, &str) -> Tick` | 推进一个 tick |
| 撮合 | `MatchingEngine::submit_user_order(OrderRequest) -> Result<OrderId, EngineError>` | 风控 + 撮合 + 结算 |
| 撮合 | `MatchingEngine::cancel_order(OrderId) -> Result<(), EngineError>` | 仅活动单可撤 |
| 撮合 | `MatchingEngine::refresh_liquidity(&str, Price)` | 重挂做市商档位 |
| 结算 | `Account::apply_fill(&Trade, &dyn MarketRules, u32)` | 更新现金/持仓/盈亏 |
| 结算 | `Account::equity(&BTreeMap<String, Price>, &dyn MarketRules) -> Money` | 现金 + 市值 |
| 风控 | `risk::validate(&MatchingEngine, &OrderRequest) -> Result<(), RiskError>` | 只读校验 |
| 回测 | `run_backtest(&[Candle], &Symbol, Arc<dyn MarketRules>, &mut dyn Strategy) -> BacktestResult` | 权益曲线 + 指标 |
| 经纪商 | `Broker::submit(OrderRequest) -> Result<BrokerOrderId, BrokerError>` | 下单出口（纸面/实盘同构） |
| 经纪商 | `PaperBroker::new(Arc<dyn MarketRules>, Money)` | 包裹 `MatchingEngine` 的模拟后台 |
| 实盘回路 | `LiveTrader::on_quote(&mut self, &str, &Tick) -> Vec<LiveAction>` | 行情→策略→下单→回报 |
| 策略 | `Strategy::on_bar(&mut self, &BarContext) -> Option<Signal>` | 回测信号 |
| 策略 | `LiveStrategy::on_quote(&mut self, &LiveContext) -> Option<Signal>` | 实时信号（与回测同构） |
| 策略 | `DualMaStrategy` / `BreakoutStrategy` / `MeanReversionStrategy` | 三类内置量化策略，均实现双 trait |

## 7. 线程与并发

首版**单线程**：行情推进、撮合、渲染均在 `eframe` 主循环内完成，无锁、无通道、无 `unsafe`。
理由：模拟盘吞吐需求极低，单线程让状态可预测、测试可复现。若后续接入真实行情，扩展点在 `MarketDataSource` 后加 `mpsc::Receiver<Tick>` 适配器，领域层无需改动。

## 8. 目录结构

```
rust-trading-desk/
├── Cargo.toml                  # workspace
├── README.md
├── docs/
│   ├── 01-requirements.md
│   ├── 02-architecture.md
│   ├── 03-test-report.md
│   └── 04-runbook.md
└── crates/
    ├── tdesk-core/{src,tests}
    ├── tdesk-data/{src,tests}
    ├── tdesk-cli/src/main.rs
    └── tdesk-app/{src,tests}
```

## 9. 演进路线

| 阶段 | 内容 |
| --- | --- |
| v0.2 | `CsvSource` 接真实历史数据；订单/账户 JSON 快照持久化；K 线周期切换 |
| v0.3 | `HttpQuoteSource` 适配器 + `tokio` 后台行情线程（`mpsc` 注入） |
| v0.4 | 策略插件化、参数网格寻优、多标的组合回测 |
| v0.5 | 券商 API 适配器：已预留 `Broker` 端口与 `LiveBroker` 空壳；落地 `LiveBroker` 真实实现需经合规评估、凭据隔离、审计日志，并与模拟盘物理隔离（详见 ADR-7） |
