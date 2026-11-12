# tdesk · Rust 股票交易终端（模拟盘）

> ⚠️ **本项目是本地纸面交易 / 教学演示系统。行情由确定性算法合成，不连接任何券商或交易所，不产生真实资金流动，不构成投资建议。**

一个用 **Rust + egui/eframe** 写的桌面股票交易终端：行情 K 线、下单撤单、撮合结算、风控、持仓盈亏与策略回测的完整闭环，全部离线可运行。

## 快速开始

```bash
# 需要 Rust 1.80+（本项目在 1.99.1 上验证）
cargo run -p tdesk-app          # 打开桌面窗口
cargo run -p tdesk-cli          # 无 GUI 端到端演示 + 断言 + 性能基准
cargo test --workspace          # 全部单元 / 集成 / UI 冒烟测试
```

macOS 首次启动会自动加载系统 `Arial Unicode.ttf` 以显示中文，无需额外配置。

## 功能

| 模块 | 内容 |
| --- | --- |
| 多市场 | A 股 / 美股 / 港股可切换：涨跌颜色、T+0/T+1、涨跌停、最小交易单位、费率全部由 `MarketRules` 驱动 |
| 行情 | 14 只标的、确定性合成日 K 与逐 tick 实时推进；K 线 / 成交量 / MA5 / MA20 / 十字光标 OHLC 提示 |
| 交易 | 限价单、市价单（IOC）、价格时间优先撮合、部分成交、撤单、自成交防范、成交回报 |
| 风控 | 数量 / 整手 / 价格 / 涨跌停 / 可用资金 / 可卖数量 / 单笔集中度校验，拒绝时账户零副作用 |
| 账户 | 现金、持仓成本、T+1 锁定与释放、浮盈浮亏、已实现盈亏、手续费、总权益恒等式 |
| 回测 | `Strategy` trait + SMA(5,20) 金叉死叉；T 日收盘出信号、T+1 开盘成交；权益曲线、收益、回撤、胜率、夏普 |
| 程序化策略 | `Strategy`/`LiveStrategy` 双 trait 同构：**双均线金叉死叉**、**N 日突破（动量）**、**均值回归（布林）**；同一套代码既回测又驱动实时纸面下单 |
| 自动交易回路 | `Broker` 端口 + `LiveTrader`：行情→策略→规模→下单→结算，纸面（`PaperBroker`）与实盘（`LiveBroker` 骨架）同构；CLI 演示 3000 tick 自动买卖闭环 |
| 工程 | Cargo workspace 分层、`cargo clippy` 无告警、UI 可在无窗口环境渲染并测试 |

## 架构

```
crates/tdesk-app   egui 桌面终端（表现层）
crates/tdesk-cli   无头演示与基准（表现层）
crates/tdesk-data  MarketDataSource → SyntheticSource / CsvSource（适配层）
crates/tdesk-core  MarketRules / MatchingEngine / Account / risk / backtest
                    + Broker(PaperBroker/LiveBroker) / LiveTrader / strategies（领域层，零外部依赖）
```

依赖严格单向 `app|cli → data → core`。价格用最小价位整数 `Price`，金额用货币最小单位 `Money`，全流程无浮点对账误差。

详细文档：

- [需求分析](docs/01-requirements.md)
- [架构设计](docs/02-architecture.md)
- [测试报告](docs/03-test-report.md)
- [运行手册](docs/04-runbook.md)

## 常见操作

| 想做什么 | 怎么点 |
| --- | --- |
| 换市场 | 顶栏 `A股·沪深 / 美股 / 港股`（会重置模拟账户） |
| 换标的 | 左侧自选列表点代码 |
| 下单 | 右侧选买/卖、限价/市价，填价格数量，或点 `25% / 50% / 全部` 自动算量，再提交 |
| 撤单 | 底部「委托」标签页，活动单右侧「撤单」 |
| 看 T+1 | 底部「持仓」页的 `可卖 / T+1锁定`；点顶栏「推进一日」释放 |
| 跑回测 | 底部「回测」页 → `运行 SMA(5,20) 回测` |

## 接入真实券商（实盘交易）

本工程已把"下单"抽象为 **`Broker` trait**，并把原有模拟撮合封装为 `PaperBroker`。这意味着**交易逻辑（策略、风控、规模、结算）在纸面与实盘之间完全复用**，接真实券商只需新增一个 `impl Broker`。

> ⚠️ **关于「东方财富证券 API」**：东方财富证券**不提供面向散户的官方交易 API**。市面上的"程序化下单"到东财/同花顺/广发等券商，只有三条非官方路径——量化平台网关（QMT / Ptrade / 掘金）、客户端自动化（easytrader 风格逆向协议或 UI 操控）、或行情/交易 Web 接口逆向。后两类**违反券商用户协议、有封号与合规风险，且涉及真实资金**。
>
> 因此 `LiveBroker` 目前是**刻意留空的骨架**：任何 `submit` 都会显式返回 `BrokerError::NotConfigured`。真实通道的落地必须经过：①确定合规可用的通道并拿到凭据；②凭据隔离（环境变量/密钥库，绝不进仓库）；③审计日志与限速；④先接 `PaperBroker` 跑通再切 `LiveBroker`。这一步与模拟盘物理隔离，建议作为独立任务评估。

当前可直接运行的自动交易是**纸面闭环**：`cargo run -p tdesk-cli` 的第 7 节会驱动 `SyntheticSource` 行情 + `PaperBroker` + `DualMaStrategy`，自动按双均线金叉/死叉买卖 3000 个 tick。

## 测试

```bash
cargo test --workspace                 # 全部测试
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p tdesk-app     # 产物 target/release/tdesk-app
```

## 已知简化

港股价位阶梯统一为 0.01；市价单按 IOC 处理；无分红送转、融券、集合竞价、真实行情与实盘接口。详见需求文档第 8 节。
