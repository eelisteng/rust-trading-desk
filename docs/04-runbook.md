# 运行手册 — `tdesk`

## 1. 环境准备

| 项 | 要求 | 本机验证结果 |
| --- | --- | --- |
| OS | macOS（原生窗口）/ Linux / Windows 亦可 | macOS x86_64 |
| Rust | stable ≥ 1.80 | rustc 1.99.1 / cargo 1.99.1 |
| 原生构建 | Xcode Command Line Tools（macOS） | `/Library/Developer/CommandLineTools` |
| 中文字体 | `/System/Library/Fonts/Supplemental/Arial Unicode.ttf` | 存在，22 MB，启动时自动加载 |
| 网络 | 仅首次 `cargo` 拉取依赖需要；运行时完全离线 | 已验证 |

若本机没有 Rust：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
source "$HOME/.cargo/env"
```

## 2. 构建与验证

```bash
cd rust-trading-desk

cargo check --workspace --all-targets                 # 编译检查（快）
cargo test --workspace                                # 全部测试
cargo clippy --workspace --all-targets -- -D warnings  # 静态检查
cargo build --release -p tdesk-app                    # 发布构建
./target/release/tdesk-app                            # 直接运行二进制
```

首次构建会编译 egui/eframe 约 300+ 个依赖，耗时数分钟；之后增量编译为秒级。

## 3. 启动桌面终端

```bash
cargo run -p tdesk-app
```

窗口默认 1440×900，最小 1100×700。顶栏固定显示 **「模拟盘 · 非实盘」**。

### 建议的完整验收路径

1. **选标的**：左侧「自选列表」点 `300750.SZ`，中央 K 线切换，右侧下单面板价格自动带出。
2. **买入**：右侧选 `买入` + `限价单`，价格填略高于卖一，数量点 `25%`，点「提交买入委托」。
3. **查看成交**：底部「成交」页出现成交回报，含成交价、数量、手续费、对手单号。
4. **T+1 验证**：底部「持仓」页 `可卖=0`、`T+1锁定=100`；此时提交卖出会被风控拒绝并显示中文原因。
5. **撤单**：先在低价挂一笔限价买单，到「委托」页点「撤单」，状态变「已撤销」。
6. **推进一日**：顶栏「推进一日」，「持仓」页可卖数量释放；再挂卖单成交平仓。
7. **风控演示**：价格填到涨停价之外，或数量填 `150`（非整手），被拒绝且资金不变。
8. **回测**：底部「回测」页点「运行 SMA(5,20) 回测」，查看权益曲线与指标。
9. **换市场**：顶栏切到「美股」，确认涨色变绿、最小单位变 1 股、T+0 即时可卖；再切「港股」确认红涨绿跌与 200 股/手（01810.HK）。
10. **主题**：点「☀ 明亮 / 🌙 深色」切换配色，涨跌色仍按市场规则保持正确。

> 顶栏「推进一日」用于手动演示 T+1；行情播放时每 24 tick 也会自动换日并释放锁定持仓。

## 4. 无头演示（CI / 无窗口环境）

```bash
cargo run -p tdesk-cli
```

输出 6 个段落：多市场规则表、确定性行情、A 股 T+1 全链路（含风控拒绝、撤单、平仓）、回测指标、CSV 适配器、1000 tick 性能基准。脚本内含 `assert!`，任何环节破坏账户恒等式都会 panic 退出。

## 5. 扩展指南

### 增加一个新市场

1. 在 `crates/tdesk-core/src/types.rs` 的 `Market` 增加枚举值并加入 `Market::ALL`。
2. 在 `market.rs` 实现 `MarketRules`（`tick_minor / lot_size / price_limit_ratio / settlement_lag_days / rise_is_red / fees`）。
3. 在 `rules_for()` 注册；在 `tdesk-data/src/synthetic.rs::instrument_list()` 增加标的清单。
4. UI 顶栏自动出现该市场，涨跌色、T+N、涨跌停、费率全部自动生效，无需改 UI 代码。

### 接入真实历史行情

实现 `MarketDataSource` trait（4 个方法），或直接用内置 `CsvSource`：

```rust
let src = CsvSource::from_csv(csv_text, "600519.SH", "贵州茅台", Market::AShare)?;
let bars = src.history("600519.SH", 250);   // 交给 run_backtest
```

CSV 格式：`ts,open,high,low,close,volume`，价格为十进制字符串，支持表头与 `#` 注释。

### 增加策略

```rust
struct MyStrategy;
impl Strategy for MyStrategy {
    fn name(&self) -> &str { "my-strategy" }
    fn on_bar(&mut self, ctx: &BarContext<'_>) -> Option<Signal> {
        // 只能访问 ctx.history[..=ctx.index]，天然无未来函数
        None
    }
}
let result = run_backtest(&bars, &symbol, rules_for(Market::AShare), &mut MyStrategy);
```

## 6. 故障排查

| 现象 | 原因 / 处理 |
| --- | --- |
| 中文显示为方块 | 系统字体路径不存在；`setup_fonts` 会依次尝试 3 个 macOS 字体并在控制台打印警告。可自行在 `theme.rs::CANDIDATES` 加入本机字体路径。 |
| 首次 `cargo run` 很慢 | 正常，egui/winit/glow 依赖较多；用 `cargo build --release` 只做一次。 |
| 窗口打不开 / 报 OpenGL 错误 | eframe 0.31 默认 glow 渲染；macOS 需 Xcode CLT。Linux 需 `libxkbcommon`/X11 开发库。 |
| 下单一直被拒 | 看右侧消息区中文原因；常见是超出涨跌停区间、非整手、可用资金不足、T+1 可卖为 0。 |
| 市价单「已撤销」 | 首版市价单按 IOC 处理：未成交余量自动撤销，不是错误。 |
| 回测收益看起来很好 | 行情是几何布朗运动合成数据，结论不可外推；请通过 `CsvSource` 换真实历史数据。 |
| 想自动截一帧 UI | `EFRAME_SCREENSHOT_TO=/abs/path/frame.png cargo run -p tdesk-app --features screenshot`（eframe 内部钩子，第 2 帧后自动退出）。注意部分 macOS GL 驱动回读帧缓冲会得到无效图像，此时以 `cargo test -p tdesk-app` 的无头渲染测试为准。 |

## 7. 文件地图

```
Cargo.toml                          workspace 定义
README.md                           项目入口
docs/01..04                         需求 / 架构 / 测试 / 运行
crates/tdesk-core/src/
  types.rs        Price/Money/Order/Trade/Candle/Tick/Quote
  market.rs       MarketRules + A股/美股/港股 + FeeSchedule
  account.rs      Position / Account / T+1 结算
  risk.rs         前置只读风控
  engine.rs       订单簿 / 价格时间优先撮合 / 做市商流动性
  backtest.rs     Strategy / SMA 金叉 / 指标
  error.rs        RiskError / EngineError（含中文原因）
crates/tdesk-core/tests/integration.rs
crates/tdesk-data/src/
  rng.rs          SplitMix64 + Box-Muller（确定性）
  source.rs       MarketDataSource / Instrument
  synthetic.rs    合成日K + tick 流 + 换日
  csv.rs          CSV 适配器
crates/tdesk-data/tests/data_tests.rs
crates/tdesk-cli/src/main.rs         无头端到端演示
crates/tdesk-app/src/
  main.rs         eframe 启动 + 字体
  app.rs          TradingApp 状态、render、交互
  charts.rs       K线 / 成交量 / MA / 权益曲线 painter
  panels.rs       卡片 / 表格组件
  theme.rs        明暗主题、市场涨跌配色、中文字体
crates/tdesk-app/tests/ui_smoke.rs  无窗口渲染测试
```
