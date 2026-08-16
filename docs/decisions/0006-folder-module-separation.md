# ADR-0006: 文件夹管理拆分独立模块 folder.rs

> 状态: 已接受 | 日期: 2026-08-16

## 背景

ADR-0005 合并后，文件夹的扫描、缩略图队列、打开/导航、渲染全部堆在 `imlayout.rs`
（约 1300 行），与图片 cell 的编排混在一起；`imcell.rs` 同时承担图片与文件夹两种
渲染（约 800 行）。文件夹功能是独立子系统（目录扫描、缩略图分批、对比导航），
与图片 cell 的编排没有共享状态，混放只会互相干扰。

## 决策

新增 `ui/folder.rs`，文件夹的一切收拢到本模块：

```
ui/folder.rs    文件夹 cell：扫描（子线程）、缩略图分批队列、打开/导航加载管线、
                FolderAction 处理、渲染（列表/缩略图/右键菜单）
ui/imlayout.rs  统筹层：图片 cell 加载管线、键盘、布局编排；持有 FolderManager，
                只做编排调用（扫描请求、导航请求、action 分发）
ui/imcell.rs    图片 cell 渲染（瘦身回图片专用）
```

边界：

- `FolderManager` 拥有自己的扫描/加载管线（`scan_rx` / `load_rx`）与缩略图队列，
  通过参数接收 `&mut AppState` 与 `ctx`，不依赖 imlayout 内部；
- imlayout 通过 `folder.scan_folders` / `folder.poll_scan` / `folder.drain_thumbnails` /
  `folder.navigate` / `folder.handle_action` 编排，线程代码物理隔离在
  folder.rs 的加载/扫描方法组内（ADR-0001 的隔离位置扩展到本模块）；
- 工具函数 `is_image_ext` / `sort_paths` 移到 folder.rs 并 `pub(crate)` 共享。

## 后果

- 正面：imlayout 回到编排职责（约 800 行）；文件夹子系统可独立阅读/演进；
  图片与文件夹的加载管线互不干扰（各自 mpsc 批次）。
- 代价：`FolderManager` 的加载管线与 imlayout 的 standalone 管线结构相似
  （重复的批次状态机），暂不泛化——两套载荷不同，泛化收益低于抽象成本。
- 风险：`FolderManager` 直接操作 `state`（与 imlayout 同层），评审时确认
  线程代码仍在文件夹方法组内。

## 备选方案

- 保持现状（全部在 imlayout）：被否决，1300 行单文件无法维护。
- folder 渲染留在 imcell：被否决，imcell 职责膨胀且与 FolderCell 类型绑定
  的渲染逻辑分散两处。
- FolderManager 委托 imlayout 加载（线程仍在 imlayout）：被否决，需要
  回调/反向依赖，接口比自带管线更绕。

## 关联

- 修订 [ADR-0001](0001-single-threaded-model.md)（线程隔离位置扩展到 ui/folder.rs）
- 部分修订 [ADR-0005](0005-merge-orchestration-into-imlayout.md)（imlayout 不再"管理文件夹的一切"）
- [docs/folder.md](../folder.md)
