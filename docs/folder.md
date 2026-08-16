# 文件夹 cell

> 状态: 稳定 | 更新: 2026-08-16 | 关联: [architecture.md](architecture.md) · [layout.md](layout.md)

## 概述

拖入目录（或命令行传目录）会创建一个**文件夹 cell**（`CellKind::Folder`），
与图片 cell 共用同一网格布局。文件夹 cell 展示目录内的图片条目，
支持列表 / 缩略图双视图、多选、滚动与右键菜单。

文件夹的一切（管理 + 渲染）收在 `ui/folder.rs`（`FolderManager`），
imlayout 只做编排调用；图片 cell 渲染在 `ui/imcell.rs`。

## 双文件夹对比

拖入**两个文件夹**即构成对比对：

1. 两个文件夹 cell 并列展示（网格布局）；
2. 每个文件夹同时最多打开 **1 张**图片——再次双击同一文件夹的条目时
   自动**替换**已打开的图片（不新增 cell）；
3. 两文件夹各打开一张后，两张图片并列展示，文件夹 cell 隐藏；
4. `Space` / `B` **同步索引**：两个文件夹各自前进/后退到下一张；
   只有对比对存在（打开的文件夹图片 ≥ 2）时才响应导航，
   单文件夹打开 1 张时按 Space/B 无操作（避免歧义）；
5. `Esc` 或 `Ctrl`+右键删除图片后，对应文件夹 cell 自动恢复。

## 数据流

```
拖入目录 / 命令行传目录
  → imlayout: classify_paths() 分离文件与目录（目录按 dir_path 去重）
  → folder.rs: scan_folders() 子线程 read_dir → 过滤 → 排序
  → poll_scan() 登记 FolderCell 入格
  → 缩略图排队 pending_thumbnails → 加载空闲时**分批**（每次 ≤8 张）
  → 子线程：read → 缩略图解码（JPEG 走解码器级降采样）→ mpsc
  → poll_loading: 按槽位写入 thumbnails（失败槽位 None，与 entries 对齐）
```

## 交互

| 操作 | 行为 |
|---|---|
| 单击条目 | 选择（Ctrl 多选 / Shift 范围选） |
| 双击条目 | 打开为图片 cell（每文件夹同时最多 1 张，重复打开替换；文件夹 cell 从网格隐藏） |
| 右键菜单 | Open image / Open file location / Remove from list / Open selected / Toggle view |
| 滚轮 | 滚动条目列表（或缩略图网格） |
| `Space` / `B` | 对比对同步导航：所有打开的文件夹图片各自上一张 / 下一张 |
| `Esc` | 关闭文件夹图片，恢复文件夹 cell 显示 |
| `Space`（有选中条目时） | 打开全部选中条目（按网格名额截断） |
| `Ctrl` + 右键 | 删除文件夹 cell 本身 |

## 打开 / 导航

- **打开**：`open_entry` → 异步加载全图 → `open_folder_entry`：
  每文件夹同时最多 1 张（已有则替换），文件夹 cell 隐藏（`open_entry` 记录当前条目）。
- **同步导航**（Space/B）：`folder_nav_targets` 计算**所有**打开的文件夹图片
  各自的下一条/上一条，`NavigateMany` 一次批次加载，完成后逐张替换并作废选区。
- **关闭**（Esc / Ctrl+右键删除）：`remove_cell` 的 Image 分支自动恢复文件夹 cell。
- 打开/导航与缩略图共用同一加载管线（`LoadTarget::OpenEntry` /
  `OpenEntries` / `NavigateMany`），加载中收到的请求忽略，不互相覆盖。

## 已知限制

- 删除文件夹 cell 时，其已打开的图片降级为独立图片（失去导航，图片保留）；
- 缩略图解码失败静默跳过（槽位为 `None`），不弹横幅；
- 目录扫描在子线程执行（`scan_folders`），主线程只登记结果；
  扫描期间拖入的新目录进入 `pending_drops`，扫描批次完成后按序处理。

## 海量目录的缩略图策略

- **分批限并发**：每次最多 8 张同时解码（`THUMB_BATCH`），队列按
  (文件夹, 偏移) 推进；峰值内存 ≈ 8 × (文件字节 + 解码缓冲)，不随目录大小增长。
- **JPEG 解码器级降采样**：`jpeg_decoder::scale` 只解码需要的 DCT 块
  （因子 1/8·1/4·1/2），单张解码内存约为全解码的 1/60；
  其余格式（PNG/WebP 等）全解码后用链式半采样快速缩略。
- 缩略图**保持宽高比**（最长边 64px），列表/网格渲染按比例绘制，不变形。
- 打开多张选中条目按剩余网格名额截断（≤ `MAX_IMAGES` 格）。
