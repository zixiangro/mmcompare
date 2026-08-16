# ADR-0004: 前后端分层 + 布局/渲染解耦

> 状态: 已接受（ui 层划分部分被 [ADR-0005](0005-merge-orchestration-into-imlayout.md) 取代） | 日期: 2026-07-31

## 背景

早期迭代中 UI 代码有"越写越肥"的趋势：布局计算、图片绘制、交互编排、数据格式化混在一起，难以测试与复用。需要稳定的模块边界。

## 决策

四个模块各守一条边界（ui 内部两层，2026-08-16 由 ADR-0005 合并）：

```
core/   纯数据处理，零 GUI 依赖（解码、旋转、直方图、统计、标签格式化、EXIF）
state.rs 纯数据结构，零逻辑（只有状态与状态转移的薄方法）
ui/     只读 state 并渲染（不允许直接改业务状态；交互结果写入 state）
        ├── imlayout.rs  统筹所有 imcell：布局、交互编排、加载管线、键盘事件
        └── imcell.rs    单格渲染单元：给定"图片 + 矩形"，画好一张图
```

## 后果

- 正面：core 可脱离 GUI 独立单元测试；布局几何可独立验证；新增渲染效果不动布局，反之亦然。
- 代价：模块间需要传递较多参数（`render_image_cell` / `draw_overlay` 因此超过 clippy 参数阈值，用 `#[allow]` 并注明理由）；ui 层"读 state + 写 state"混在同一帧渲染过程中，交互编排集中在 imlayout，需注意顺序（先收集后应用，如 `PanFeedback`）。
- 风险：`imlayout.rs` 容易吸走太多职责（当前键盘、加载、拖拽、标题、布局都在这里），约 900 行；再膨胀时应优先拆加载管线方法组（ADR-0005）。

## 备选方案

- 单一大 `widgets.rs` 文件：被否决，职责混杂无法测试。
- 引入 ECS/信号系统解耦交互：过度设计，单线程应用不需要。

## 关联

- [ADR-0005 合并编排层与布局引擎为 imlayout](0005-merge-orchestration-into-imlayout.md)
- [ADR-0002 手动精确坐标布局](0002-manual-layout.md)
- [docs/architecture.md](../architecture.md)
