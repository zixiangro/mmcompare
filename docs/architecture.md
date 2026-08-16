# 研发文档：架构设计与思路

> 面向研发与需要理解代码结构的新读者。状态：2026-08-16

## 一句话概览

一个 egui 单窗口应用：**状态（state）与渲染（ui）分离，一切重计算在后台线程，
主线程只做状态转移与纹理上传**。所有内容都是"格子"（cell）——图片 cell、文件夹 cell——由统一的网格布局编排。

```
┌─────────────┐    ┌──────────────────────────┐
│   main.rs   │    │        state.rs           │
│  入口/初始化  │───▶│   纯数据：cells、模式、   │
└─────────────┘    │   视图状态、索引约定       │
                   └────────────┬─────────────┘
                                │ 只读 / 薄方法转移
                   ┌────────────▼─────────────┐
                   │      ui/ 层（编排+渲染）    │
                   │                          │
                   │ imlayout.rs  统筹：布局、  │
                   │   键盘、图片加载管线、模式  │
                   │ folder.rs    文件夹子系统：│
                   │   扫描/缩略图/打开/导航/渲染 │
                   │ imcell.rs    图片 cell 绘制│
                   └────────────┬─────────────┘
                                │ 纯函数调用
                   ┌────────────▼─────────────┐
                   │      core/image.rs       │
                   │  解码/缩略图/旋转/直方图/   │
                   │  统计/EXIF（零 GUI 依赖）  │
                   └──────────────────────────┘
```

## 代码地图（阅读顺序）

| 文件 | 职责 | 阅读要点 |
|---|---|---|
| `src/main.rs` | 入口：env_logger、命令行参数、窗口创建、主题 | 只做初始化，无业务逻辑 |
| `src/state.rs` | **全局状态**：`image_cells`/`folder_cells`（实际存储）、`cell_order`（显示顺序）、视图开关、拖拽/删除的瞬时状态 | 文件头部的**索引约定**必须先读；所有跨帧引用（加载批次、队列）都可能受"删除导致下标移动"影响 |
| `src/ui/imlayout.rs` | 应用主体 `MmCompare`：键盘事件、standalone 图片加载管线、网格布局 `image_grid`、交互编排（拖拽/重排/删除/缩放） | `ui()` 每帧顺序是刻意的：**键盘 → 管线轮询 → 渲染** |
| `src/ui/folder.rs` | 文件夹子系统 `FolderManager`：目录扫描、缩略图分批调度、打开/导航加载管线、`FolderAction` 处理、文件夹 cell 渲染 | 三条独立管线（`scan_rx`/`thumb_rx`/`load_rx`）；调度策略：新目录插队、轮转、`THUMB_LIMIT` 上限 |
| `src/ui/imcell.rs` | 图片 cell 的纯绘制：居中、覆盖层、直方图、纹理重建、旋转封装 | 不碰 state，返回数据由调用方写回 |
| `src/ui/video.rs` | 视频 cell 的纯绘制：帧居中（letterbox）、控制条（播放/暂停、进度 seek、时间） | 不碰 state，交互意图以 `VideoAction` 返回由 imlayout 写回 |
| `src/core/image.rs` | 纯像素算法：解码、缩略图（JPEG 降采样）、旋转、直方图、选区统计、EXIF | 无 GUI 类型，可独立单测 |
| `src/core/video.rs` | 视频解码封装：`VideoDecoder`（seek 解码 / 连续播放解码）、首帧提取、降采样 | 无 GUI 类型；解码线程只在本模块与 imlayout 的 worker 之间 |

## 核心概念

### 1. Cell（格子）

一切显示内容都是 cell，网格布局（`image_grid`）只认识两类：

```rust
enum CellKind { Image(usize), Folder(usize), Video(usize) }  // usize = 存储 vec 的下标
```

- `image_cells` / `folder_cells`：**实际存储**，删除元素下标会移动；
- `cell_order`：**显示顺序**，与 `pan_offset` 等长同序；
- **索引约定（不变量）**：三处必须一致——删除/重排时同步维护，否则静默画错图。

### 2. 两种模式（互斥）

文件模式 / 文件夹模式，由状态推导（`folder_cells`、图片、加载批次），入口 `classify_paths` 统一过滤。设计动机：两种模式的交互规则（导航、删除、打开）完全不同，混合会互相干扰。

### 3. 事件门控（对比对规则）

交互响应是"文件夹数 × 各文件夹打开图数"的函数（`state.rs`）：

| 状态 | 导航 Space/B | 删除 Ctrl+RMB |
|---|---|---|
| 单文件夹多图 | 不响应 | 响应 |
| 每文件夹 ≤1 张 | 响应（同步索引） | — |
| 对比对（2 文件夹各 1 张） | 响应 | **禁用**（Esc 退出） |

### 4. 加载管线

四条独立批次管线（各持 mpsc，互不阻塞）：

| 管线 | 位置 | 载荷 | 特性 |
|---|---|---|---|
| standalone 图片 | imlayout | 全图+EXIF+直方图 | 收齐按序 append |
| 目录扫描 | folder | 路径列表 | 队列接续（`queue_scan`） |
| 缩略图 | folder | 64px 图 | 每批 8 张、插队+轮转、上限 200 |
| 打开/导航 | folder | 全图 | OpenEntry/OpenEntries/NavigateMany |
| 视频首帧 | imlayout | info + 首帧（≤1280 降采样） | 收齐按序 append |
| 视频播放/seek | imlayout | RGB 帧（按 path 关联 cell） | 每视频一个 worker；rx 丢弃即停 |

**线程隔离（ADR-0001）**：线程原语只允许出现在 imlayout 与 folder 的加载/扫描方法组；其余模块纯主线程。

### 5. 帧循环顺序（imlayout::ui）

```
键盘（改 state）→ poll_drops（拖拽）→ poll_loading（standalone）
→ folder.poll_scan / poll_loading / poll_thumbnails
→ drain_pending_drops / drain_thumbnails（队列接续）→ 渲染
```

渲染前所有异步结果已落库；交互反馈（`PanFeedback`）帧末统一应用，避免渲染中途改状态。

## 关键设计决策（ADR）

| 决策 | 内容 | 文件 |
|---|---|---|
| 0001 | 单线程心智模型，多线程物理隔离 | decisions/0001 |
| 0002 | 完全手动坐标布局（不用 egui 自动布局） | decisions/0002 |
| 0003 | 重 CPU 计算下沉子线程，纹理上传留主线程 | decisions/0003 |
| 0004 | 前后端分层（core/state/ui） | decisions/0004 |
| 0005 | 编排层与布局引擎合并为 imlayout | decisions/0005 |
| 0006 | 文件夹管理拆分独立模块 folder.rs | decisions/0006 |

## 防御性设计（review 沉淀）

- **跨帧引用带 dir 校验**：加载批次目标、缩略图队列都携带目标目录路径，加载完成时比对——文件夹在加载期间被删除/索引移动时，结果丢弃而非写错对象；
- 空条目、拖拽期间删除等边缘路径有越界防御；
- egui 0.35 API 坑（`Sense::drag()` 不含 CLICK 等）记录在 [egui-api.md](egui-api.md)。

## 测试策略

- `core`：纯算法测试（缩略图颜色/尺寸/格式回退）；
- `state`：状态机测试（打开限制、对比对判定、删除恢复、网格边界）——用 headless `egui::Context` 构造纹理；
- `folder`：调度语义测试（插队、轮转、上限、漂移丢弃）；
- `imlayout`：模式互斥测试（含窗口期）——用真实临时文件系统。
- 运行：`cargo test`（25 个）。

## 相关文档

- [文档地图](README.md)
- [加载管线细节](loading.md) · [布局算法](layout.md) · [文件夹子系统](folder.md) · [局部模式](local-mode.md)
