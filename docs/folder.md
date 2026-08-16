# 文件夹 cell

> 状态: 稳定 | 更新: 2026-08-16 | 关联: [architecture.md](architecture.md) · [layout.md](layout.md)

## 概述

拖入目录（或命令行传目录）会创建一个**文件夹 cell**（`CellKind::Folder`），
与图片 cell 共用同一网格布局。文件夹 cell 展示目录内的图片条目，
支持列表 / 缩略图双视图、多选、滚动与右键菜单。

管理职责在 `ui/imlayout.rs`（imlayout 统筹所有 cell，含文件夹 cell），
渲染在 `ui/imcell.rs`（`render_folder_cell`，与图片渲染同属单格渲染单元）。

## 数据流

```
拖入目录 / 命令行传目录
  → imlayout: classify_paths() 分离文件与目录（目录按 dir_path 去重）
  → scan_folders(): 子线程 read_dir → 过滤 → 排序 → poll_scan() 登记 FolderCell 入格
  → 缩略图排队 pending_thumbnails → 加载空闲时 spawn_loaders(LoadTarget::Thumbnails)
  → 子线程：read → decode_thumbnail_bytes(64×64) → mpsc
  → poll_loading: 按槽位 push thumbnails（失败槽位 None，与 entries 对齐）
```

## 交互

| 操作 | 行为 |
|---|---|
| 单击条目 | 选择（Ctrl 多选 / Shift 范围选） |
| 双击条目 | 打开为图片 cell（文件夹 cell 从网格隐藏） |
| 右键菜单 | Open image / Open file location / Remove from list / Open selected / Toggle view |
| 滚轮 | 滚动条目列表（或缩略图网格） |
| `Space` / `B` | 打开的文件夹图片切换到 上一张 / 下一张 |
| `Esc` | 关闭文件夹图片，恢复文件夹 cell 显示 |
| `Space`（有选中条目时） | 打开全部选中条目 |
| `Ctrl` + 右键 | 删除文件夹 cell 本身 |

## 打开 / 导航

- **打开**：`OpenImage` → 异步加载全图 → `open_folder_entry`：
  新图片 cell 入 `cell_order`，文件夹 cell 隐藏（`open_entry` 记录当前条目）。
- **导航**（Space/B）：`folder_nav_target` 只计算目标条目，加载完成后
  `apply_navigated_image` 一次性替换图片内容并作废选区（避免半更新状态）。
- **关闭**（Esc）：`close_folder_at_pos` 删除图片 cell 并恢复文件夹 cell。
- 打开/导航与缩略图共用同一加载管线（`LoadTarget::OpenEntry` /
  `OpenEntries` / `Navigate`），加载中收到的请求排队或忽略，不互相覆盖。

## 已知限制

- 删除文件夹 cell 时，其已打开的图片降级为独立图片（失去导航，图片保留）；
- 缩略图解码失败静默跳过（槽位为 `None`），不弹横幅；
- 目录扫描在子线程执行（`scan_folders`），主线程只登记结果；
  扫描期间拖入的新目录进入 `pending_drops`，扫描批次完成后按序处理。
