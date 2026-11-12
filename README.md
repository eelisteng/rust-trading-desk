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
| 工程 | Cargo workspace 分层、`cargo clippy` 无告警、UI 可在无窗口环境渲染并测试 |

## 架构

```
crates/tdesk-app   egui 桌面终端（表现层）
crates/tdesk-cli   无头演示与基准（表现层）
crates/tdesk-data  MarketDataSource → SyntheticSource / CsvSource（适配层）
crates/tdesk-core  MarketRules / MatchingEngine / Account / risk / backtest（领域层，零外部依赖）
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

## 测试

```bash
cargo test --workspace                 # 全部测试
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p tdesk-app     # 产物 target/release/tdesk-app
```

## 已知简化

港股价位阶梯统一为 0.01；市价单按 IOC 处理；无分红送转、融券、集合竞价、真实行情与实盘接口。详见需求文档第 8 节。
