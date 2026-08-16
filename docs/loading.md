# 图片加载管线

> 状态: 稳定 | 更新: 2026-08-16 | 关联: [ADR-0001](decisions/0001-single-threaded-model.md) · [ADR-0003](decisions/0003-loading-pipeline.md)

## 原则

单线程心智模型为主（ADR-0001）。多线程代码**完全隔离**在 `ui/imlayout.rs` 中，仅用于图片加载。无 `Arc<Mutex<>>`，无全局线程池。

## 管线总览

```
拖拽 / 命令行传参（文件或目录）
  │
  ├─ 目录: ui/folder.rs（FolderManager 管线）
  │     queue_scan() 排队（不受图片加载进度影响）→ scan_folders() 子线程 read_dir
  │     → poll_scan() 登记入格（扫描批次完成后自动接续队列）
  │     → 缩略图**独立管线**（thumb_rx）：新目录插队（最近优先）、分批轮转
  │       （≤8 张/批，每文件夹上限 THUMB_LIMIT），与打开/导航互不阻塞
  │     → 打开/导航走 load_rx 管线的 OpenEntry/OpenEntries/NavigateMany
  │
  ├─ imlayout.rs 管线（standalone 图片）
  │     spawn_loaders() → 子线程：读文件 → decode → EXIF → 直方图 → mpsc
  │     → poll_loading() 逐张上传 GPU → 收齐 append
  │
  └─ 两条管线各自独立批次（`load_rx`），互不覆盖；
     加载期间新到的图片文件进入 pending_drops，空闲后按序处理；
     新目录始终立即接受（queue_scan 内部排队）
```

加载期间新到的拖拽进入 `pending_drops`，待缩略图目录进入 `pending_thumbnails`，
当前批次完成后由 `drain_pending_drops` / `drain_pending_thumbnails` 按序启动。
目录扫描与图片解码是两个独立批次（`scan_rx` / `load_rx`），可并行。

## 关键类型

```rust
type LoadResult = Result<(DecodedImage, String, [u32; 256]), PathBuf>;
//                       ├── 解码图   ├─ EXIF 摘要 ├─ Y 直方图
//                                                                     └─ 失败路径
```

- `DecodedImage`：`rgba` + `size` + `path`，不含原始字节（EXIF 在线程内消费完即弃）。
- 主线程只做：纹理上传（必须主线程）、错误记账、按序追加。

## 并发与容量

- 单批 ≤ `MAX_IMAGES`（8），由 `filter_paths` 的 `MAX_IMAGES - cell_order.len() - loading_total` 控制。
- 每批全部完成后才启动下一批：`pending_drops` 中的路径由 `drain_pending_drops()` 在加载结束后处理。
- 加载期间 `image_grid` 正常渲染已有图片，顶部横幅显示进度与失败列表，不整屏遮挡。

## 失败处理

| 场景 | 处理 |
|---|---|
| 读文件失败 | `log::warn` + `Err(path)` → 横幅显示 `Failed: <path>` |
| 解码失败 | 同上 |
| EXIF 解析 panic | `catch_unwind` 兜底，返回空串（不影响加载） |

失败路径从 `loaded_paths` 移除，用户重新拖入即可重试。横幅保留到下一次加载开始（`spawn_loaders` 清空 `load_errors`）。

## 内存说明

- 解码期峰值：同时解码的图片数 × 图片大小（含原始字节）。
- 常驻：`ImageInfo.rgba` 全分辨率保留（选择框统计、旋转需要），8 × 50MP ≈ 1.6GB 为已知代价（ADR-0003）。

## 排查清单

- 加载无响应：确认 `spawn_loaders` 里 `ctx.request_repaint()` 已调用（线程完成后靠重绘收结果）。
- 拖拽不生效：确认 `egui::ViewportBuilder::with_drag_and_drop(true)`（main.rs）与 `i.raw.dropped_files` 读取。
- 主线程卡顿：检查是否把全图遍历（直方图/统计/EXIF）写回了主线程——应在子线程或仅作用于选择区域。
