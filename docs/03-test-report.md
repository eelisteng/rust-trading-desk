# 测试报告 — `tdesk` v0.1.0

| 项 | 结果 |
| --- | --- |
| 测试日期 | 2026-11-12 |
| 环境 | macOS x86_64 · rustc 1.99.1 · cargo 1.99.1 · Xcode CLT |
| 总用例数 | **53** |
| 通过 / 失败 | **53 / 0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过，0 告警 |
| `cargo build --release -p tdesk-app` | 通过，产物 `target/release/tdesk-app`（5.0 MB） |
| `cargo run -p tdesk-cli` | 通过，全部断言成立 |
| 原生窗口启动 | 通过（eframe/glow 创建窗口并完成两帧渲染） |

---

## 1. 测试分层

```
第 4 层  UI 冒烟测试      tdesk-app/tests/ui_smoke.rs       3 例（无窗口 egui 渲染）
第 3 层  端到端演示       tdesk-cli/src/main.rs             6 段脚本 + assert!
第 2 层  跨模块集成测试   tdesk-core/tests/integration.rs   7 例
                          tdesk-data/tests/data_tests.rs    5 例
第 1 层  单元测试         tdesk-core/src/*.rs              27 例
                          tdesk-data/src/*.rs              11 例
```

领域层不依赖系统时间、网络、窗口，因此第 1/2/3 层完全可在 CI 无图形环境运行；第 4 层借助 `egui::Context::run(RawInput::default(), …)` 在纯内存里布局与绘制一帧。

## 2. 用例清单与结果

### 2.1 `tdesk-core` 单元测试（27 例，全部通过）

| 模块 | 覆盖点 |
| --- | --- |
| `market` (6) | 涨跌停取整（`1685×1.1→1854`）、无涨跌停市场、A 股最低佣金 ¥5、印花税仅卖出、金额千分位与符号格式化、价格 `f64↔tick` 往返一致 |
| `account` (5) | 买卖结算与已实现盈亏、T+1 锁定/释放与 T+0 即时可卖、部分平仓成本余额归零、市值/浮盈/权益恒等式、做市商成交不影响账户 |
| `engine` (10) | 价格优先、时间优先、maker 价成交、部分成交挂单、撤单、涨跌停拒绝零副作用、整手与数量、资金不足、T+1 拦截与释放、T+0 即时卖出、自成交防范、市价单 IOC、盘口刷新保留用户单、权益守恒（差额=手续费） |
| `backtest` (4) | 信号 T 日收盘 / 成交 T+1 开盘、平坦序列零交易零回撤、上升趋势产生正收益、空权益曲线不 panic |
| `rng` / 合计 | 见 2.3 |

### 2.2 `tdesk-core` 集成测试（7 例，全部通过）

| 用例 | 断言 |
| --- | --- |
| `full_day_cycle_is_consistent_for_every_market` | 三个市场各自完成「买入 → 校验 T+N 可卖 → 换日 → 卖出平仓 → 权益=现金」，成交数=2 |
| `risk_matrix_rejects_without_mutating_state` | 数量为 0 / 非整手 / 价格为 0 / 涨停外 / 跌停外 / 无持仓卖出 / 资金不足 7 类拒绝，现金与订单簿深度均不变 |
| `price_time_priority_handles_multiple_levels_and_partial_fills` | 成交价序列 `[10000, 10000, 10100]`，同价位按挂单先后 `b→c→a`，加权均价 10033 |
| `market_order_reserves_cash_conservatively_and_fills_ioc` | 市价单 300 股全部成交，`权益 = 期初 − 已付名义额 − 手续费 + 盯市市值` |
| `backtest_does_not_use_future_prices` | 策略内断言 `history.len() == index+1`，首根产生信号、第二根开盘 11000 成交 |
| `fee_differences_are_applied_by_market` | A 股/港股卖出费 > 买入费，美股全 0 |
| `equity_identity_holds_after_marking_to_market` | 4 个不同盯市价下 `现金 + 数量×价格 = 总权益` |

### 2.3 `tdesk-data` 单元 + 集成（16 例，全部通过）

| 用例组 | 覆盖点 |
| --- | --- |
| `rng` (4) | 同种子序列一致、异种子不同、正态分布有限值、符号哈希稳定 |
| `synthetic` (3) | 120 根日 K 确定性 + `high ≥ max(o,c)` / `low ≤ min(o,c)` / 正值、异种子路径不同、三市场标的元数据完整 |
| `csv` (4) | 表头/注释跳过、十进制价格转 tick、尾部 N 根、回放循环与 prev_close、畸形行拒绝 |
| 集成 (5) | 三市场全部标的行情逐字段可复现、200+ tick 后 OHLC 与量恒等式、换日前收盘可观测、合成历史 → CSV → 解析无损往返、端口元数据一致性 |

### 2.4 `tdesk-app` UI 冒烟（3 例，全部通过）

| 用例 | 断言 |
| --- | --- |
| `renders_one_complete_frame_without_a_window` | 一帧产生非空 `FullOutput.shapes`，无 panic / 布局溢出 |
| `scripted_interaction_covers_order_backtest_and_market_switch` | 渲染 → 快捷算量 → 下单 → 换日 → 回测 → 切美股 → 切港股，状态正确落地 |
| `every_market_renders_with_its_own_palette` | A 股 / 美股 / 港股各渲染一帧并套用各自涨跌配色 |

> 中文字体在测试中也通过 `setup_fonts()` 加载，验证 `Arial Unicode.ttf` 可被 egui 解析。

## 3. 端到端 CLI 实测输出（摘要）

```
1 · 多市场规则
A股  T+1 红涨绿跌 100股/手 涨跌停 [90.00, 110.00]  佣金0.0300% 印花(卖)0.0500%
美股 T+0 绿涨红跌 1股/手   无涨跌停限制          费率全 0
港股 T+0 红涨绿跌 100股/手 无涨跌停限制          佣金0.0300% 印花(卖)0.1000%

2 · 确定性行情      5 只标的 / 160 根日K / 首 1403.19 末 1685.00 / 复现校验通过
3 · 风控→撮合→结算  初始 ¥1,000,000.00
   风控拒绝 · 委托价 218500 超出涨跌停区间 [151650, 185350]
   买入成交 · 1685.01 × 100，手续费 ¥52.24，T+1 可卖 0
   T+1 拦截 · 可卖数量不足：拟卖 100，可卖 0
   换日后卖出 · 平仓完成 | 已实现盈亏 -¥189.49 | 总权益 ¥999,810.27
4 · 回测            180 根K线 / 6 笔成交 / 3 次平仓
   总收益 +0.24% | 最大回撤 1.69% | 胜率 33.3% | 夏普 0.13
5 · CSV 适配器      3 根解析 · 回放最新价 10.20 前收 10.00 量 1000
6 · 性能基准        1000 tick / 55.33 ms → 0.055 ms/tick / 10000 笔订单
结论：全部断言通过
```

> 回测收益来自合成随机行情，仅用于验证链路正确性，不代表策略有效性。

## 4. 性能

| 指标 | 实测 | 验收阈值 | 结论 |
| --- | --- | --- | --- |
| 单 tick（含 5 标的行情推进 + 10 档盘口刷新） | 0.055 ms | < 16 ms（一帧预算） | 通过，余量约 290× |
| 1000 tick 生成订单数 | 10 000 | — | 内存/索引增长受控（maker 索引每 tick 清空） |
| 53 个测试总耗时 | < 0.2 s（不含编译） | — | 通过 |
| Release 二进制 | 5.0 MB | — | 通过 |

## 5. 缺陷与修复记录

| # | 现象 | 原因 | 修复 |
| --- | --- | --- | --- |
| 1 | `full_day_cycle` 在卖出后持仓仍为 100 | 测试里做市商买单一挂出就与遗留的做市商卖单自相成交，导致用户卖单无对手盘 | 测试将做市商买单低挂 1 tick；并通过用例记录该行为属订单簿正常语义 |
| 2 | 多市场「涨跌停」显示 `23058430092136940.00` | 无涨跌停市场 `limit_range` 返回上界哨兵值，被直接格式化 | 展示层按 `price_limit_ratio().is_none()` 显示「无涨跌停限制」，CLI 与 UI 同步修正 |
| 3 | `rustc 1.99` 浮点字面量回退告警 | `Stroke::new(1.0, …)` 推断 f64 | 全部显式写成 `1.0_f32` 等 |
| 4 | clippy `inconsistent_digit_grouping` / `manual_clamp` / `manual_is_multiple_of` | 测试字面量与新版本 lint | 统一下划线分组、改用 `clamp`、对兼容旧编译器的分组循环加局部 allow |

## 6. 未覆盖 / 后续补充

- 真实券商/行情接口联调（首版明确排除）。
- 长时间（>10 万 tick）内存增长与订单归档策略。
- 跨平台（Windows/Linux）原生窗口与字体路径验证；当前仅在 macOS 实测。
- 视觉回归：eframe 内置 `__screenshot` 钩子在本机 GL 驱动下回读为无效帧缓冲，故改用「无头一帧渲染 + shapes 非空」作为 UI 冒烟依据。仓库保留 `screenshot` feature 供其他平台使用。
- 属性测试（proptest / 随机订单流不变式）尚未引入。

## 7. 复现命令

```bash
cd rust-trading-desk
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p tdesk-cli
cargo build --release -p tdesk-app && ./target/release/tdesk-app
```
