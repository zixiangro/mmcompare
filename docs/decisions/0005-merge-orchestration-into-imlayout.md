# ADR-0005: 合并编排层与布局引擎为 imlayout

> 状态: 已接受 | 日期: 2026-08-16

## 背景

ADR-0004 把 ui 层拆成 app.rs（编排）+ viewer.rs（布局引擎）+ imcell.rs（渲染单元）三层。实践中发现前两层的边界价值低于阅读成本：

- 整个应用专为图片服务，`viewer` 作为"通用布局引擎"的抽象从未被复用；
- 交互时序（`PanFeedback` 帧末统一应用）与加载/键盘时序（快捷键先于渲染）耦合在跨模块调用里，阅读顺序 ≠ 执行顺序；
- 三个文件互相传参（`render_image_cell` 9 个参数），边界本身成了负担。

## 决策

把 `app.rs` 与 `ui/viewer.rs` 合并为 `ui/imlayout.rs`（image layout，统筹层），ui/ 内部从三层压成两层：

```
ui/imlayout.rs  统筹所有 imcell：加载管线、键盘事件、窗口标题、布局计算、交互编排
ui/imcell.rs    单格渲染单元：给定"图片 + 矩形"，画好一张图
```

`imcell` 保持"单格服务"不变：不关心自己在哪、不关心有几个格子、不碰业务状态（返回值由 imlayout 写回）。`imlayout` 是唯一统筹者，也是线程原语唯一合法出现处（ADR-0001 的隔离位置从 `app.rs` 移到 `imlayout.rs`，决策本身不变）。

## 后果

- 正面：交互时序与加载时序同处一文件，阅读顺序即执行顺序；减少一层间接调用与跨模块参数传递。
- 代价：`imlayout.rs` 约 900 行，超过 ADR-0004 的"~400 行应拆分"旧阈值。后续再膨胀时，优先拆出**加载管线方法组**（`spawn_loaders` / `poll_loading` / `poll_drops` / `drain_pending_drops` 自成一组），而非恢复 viewer。
- 风险：`imlayout` 同时拥有线程原语与 UI 交互，评审时需确认线程代码仍物理隔离在加载方法组内（ADR-0001）。

## 备选方案

- 保持三层（app / viewer / imcell）：被否决，边界价值低于阅读成本。
- 只改名不合并（viewer → imlayout）：被否决，无法消除跨模块时序耦合。
- 全部合并进一个 `app.rs`：被否决，`imcell` 单格单元与统筹层职责差异大，合并会丢失"单格可独立阅读"的好处。

## 关联

- 部分取代 [ADR-0004](0004-module-separation.md) 的 ui 层划分（core / state 边界不变）
- [ADR-0001](0001-single-threaded-model.md)（线程隔离位置迁移）
- [docs/architecture.md](../architecture.md)
