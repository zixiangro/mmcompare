# ADR-0002: 手动精确坐标布局

> 状态: 已接受 | 日期: 2026-07-31

## 背景

网格视图需要"等宽 cell + 1px 分隔线 + 行内居中 + 精确留白"。egui 自动布局（`ui.horizontal`、`item_spacing`、`centered_and_justified`）在精确对齐时存在多个 edge case：`horizontal` 在末尾元素后仍加 `item_spacing` 导致右侧不对称；`centered_and_justified` 会消费全部可用高度导致多行布局失败；分隔线与 cell 之间难以做到像素级对齐。

## 决策

`viewer.rs` 采用**完全手动坐标**：

1. `ui.allocate_exact_size` 一次性预留整个网格区域；
2. 用 `pos2`/`Rect::from_min_size` 手工计算每个 cell、分隔线、margin 区的位置；
3. 用 `ui.painter()` 绘制，用 `ui.allocate_rect` / `ui.allocate_exact_size` 声明交互区；
4. 不在布局路径上使用任何自动布局 API。

配套的渲染分工见 ADR-0004（viewer 只算位置，imcell 只负责画）。

## 后果

- 正面：像素级可控、行为可预测，不受 egui 版本布局细节变化影响；布局逻辑可独立阅读与测试（`find_cell_at`、`GridLayout` 均为纯几何计算）。
- 代价：代码冗长（每个 zone 都要显式声明）；新增布局规则（行数/列数）需要手写分支（当前是 `row_layout` 的 match）。
- 风险：`available_size()` 为 0 或负值（窗口极小）时出现除零/负尺寸，当前未防御，属已知边界。

## 备选方案

- `ui.horizontal` + `item_spacing` 微调：被否决，末尾间距与跨行对齐不可控。
- `egui_extras::Grid`：被否决，Grid 面向表单式布局，不支持自定义分隔线、行内居中与不等宽行。
- 每 cell 独立 `CentralPanel`/`Area`：被否决，交互与渲染顺序难管理。

## 关联

- [ADR-0004 模块分离](0004-module-separation.md)
- [docs/layout.md](../layout.md)
