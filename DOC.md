# mmcompare 项目知识库

## 启动

```powershell
cargo run
```

窗口 800×600，浅色主题。菜单栏 File → Open 选择图片（支持多选，最多 8 张）。

## 架构分层

```
┌──────────────────────────────────────────────────┐
│                   main.rs                        │
│           初始化 eframe，创建 MmCompare            │
└────────────────────┬─────────────────────────────┘
                     │
┌────────────────────▼─────────────────────────────┐
│                   app.rs                         │
│  编排层：连接 core ↔ state ↔ ui                   │
│  - start_open(): 开文件对话框 + 起解码线程          │
│  - poll_loading(): 每帧收线程结果 + 上传 GPU 纹理   │
│  - ui(): 拼接 menu + viewer                      │
└──────┬─────────────────────────────────┬─────────┘
       │                                 │
┌──────▼──────┐                  ┌───────▼─────────┐
│   core/     │                  │     ui/         │
│  纯数据处理   │                  │   纯界面渲染      │
│             │                  │                 │
│ image.rs:   │                  │ menu.rs:        │
│   图片解码   │                  │   菜单栏         │
│             │                  │                 │
│             │                  │ viewer.rs:      │
│             │                  │   图片网格布局    │
│             │                  │                 │
│             │                  │ widgets.rs:     │
│             │                  │   通用组件(预留)  │
└──────┬──────┘                  └───────┬─────────┘
       │                                 │
       │         ┌───────────┐           │
       └────────►│ state.rs  │◄──────────┘
                 │ 共享数据   │
                 │ ImageInfo │
                 │ AppState  │
                 └───────────┘
```

## 数据流

### 图片打开流程

```
用户点击 File → Open
  → menu.rs 设 pending_open = true
  → app.rs 检测到 flag:
      1. rfd::FileDialog 弹出（主线程阻塞，等待用户选择）
      2. 用户选完，最多取 8 个路径
      3. 每个路径 spawn 一个线程 → 读文件 → 解码 → mpsc::send
      4. drop(tx)，确保 rx 在所有线程结束后不阻塞
  → 每帧 poll_loading():
      5. try_recv 收线程结果
      6. 收齐后：主线程 load_texture 上传 GPU
      7. 更新 state.images → 自动重绘
```

### 状态读取（纯读）

```
ui::menu::menu_bar(ui, &mut pending_open)     ← 只写 pending_open
ui::viewer::image_grid(ui, &state.images, ..) ← 只读 images
```

## 布局算法

### cell 尺寸计算（所有 cell 统一）

```
max_cols = 所有行中最大列数
cell_width  = (available.x - (max_cols - 1) * 13) / max_cols    // 13 = 6+1+6
row_height  = (available.y - (rows - 1) * 1) / rows              // 1 = sep
```

### 行内布局（手动坐标）

对每一行：
1. `allocate_exact_size(available.x, row_height)` 预留整行空间，拿到 `row_rect`
2. 计算 `row_content_width = col_count * cell_width + (col_count-1) * 13`
3. `x = row_rect.left() + (available.x - row_content_width) / 2` 居中起始
4. 逐个摆放：`margin(6px)` → `sep(1px)` → `margin(6px)` → `cell`

### 行间分隔

仅 1px 横线，无 margin，通过 `allocate_exact_size(available.x, 1)` + `painter().rect_filled()` 绘制。

## 图片绘制

`draw_cell(ui, img, cell_rect)`：
1. 计算 fit-in-rect 缩放比：`min(cell_w / img_w, cell_h / img_h)`
2. 图片在 cell 内居中
3. `ui.put(img_rect, Image::from_texture(SizedTexture::new(id, display_size)))`

## 依赖说明

| 包 | 版本 | 用途 |
|---|---|---|
| eframe | 0.35 | 窗口框架，wgpu 后端 |
| egui_extras | 0.35 | 图片加载器安装（`install_image_loaders`） |
| env_logger | 0.11.11 | 日志输出 |
| image | 0.25 | 图片格式解码（PNG, JPEG, WebP 等） |
| rfd | 0.17 | 原生文件对话框 |

## 线程模型

单线程为主，只有在 `app.rs::start_open()` 中临时 spawn 子线程解码图片。

```
主线程                   子线程 1        子线程 2        子线程 N
  │                        │              │              │
  ├─ 文件对话框(阻塞)        │              │              │
  ├─ spawn ────────────────►│              │              │
  ├─ spawn ───────────────────────────────►│              │
  ├─ spawn ──────────────────────────────────────────────►│
  ├─ drop(tx)               │              │              │
  │                        │              │              │
  ├─ try_recv ◄────────────┤ 读文件+解码    │              │
  ├─ try_recv ◄────────────┼──────────────┤ 读文件+解码   │
  ├─ try_recv ◄────────────┼──────────────┼──────────────┤ 读文件+解码
  │                        x              x              x (线程结束)
  ├─ 收齐 → 上传纹理 → 显示
```

无 `Arc<Mutex<>>`，无全局线程池，跨线程通信仅通过 `mpsc::channel`。

## 后续扩展方向

- `ui/widgets.rs`：状态栏、缩放滑块等通用组件
- `core/`：图片对比算法、缓存管理
- `state.rs`：对比结果、用户设置
- 键盘快捷键（← → 切换、Delete 移除、R 重置等）
