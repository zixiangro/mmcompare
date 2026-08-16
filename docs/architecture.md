# 系统架构

> 状态: 稳定 | 更新: 2026-08-16 | 关联: [ADR-0004](decisions/0004-module-separation.md) · [ADR-0005](decisions/0005-merge-orchestration-into-imlayout.md)

## 分层

```
┌────────────┐     ┌──────────────┐
│   main.rs  │     │   state.rs   │
│  入口初始化  │     │  全局共享数据  │
└─────┬──────┘     └──────┬───────┘
      │                   │
┌─────▼───────────────────▼─────────┐
│        ui/imlayout.rs             │
│  统筹层：图片加载、键盘、布局编排  │
│                                  │
│  classify_paths()  分离文件/目录  │
│  spawn_loaders()   图片解码线程   │
│  poll_loading()    收结果+追加    │
│  poll_drops()      拖拽/文件夹    │
│  image_grid()      布局+交互编排   │
│  folder: FolderManager 编排调用   │
└────┬──────────────────────┬──────┘
     │                      │
┌────▼──────┐      ┌────────▼───────┐
│   core/   │      │     ui/        │
│  纯数据处理 │      │                │
│           │      │ folder.rs      │
│ image.rs  │      │ 扫描/缩略图/    │
│  解码/缩略图│      │ 打开/导航/渲染  │
│  旋转/直方图│      │                │
│  统计/标签│      │ imcell.rs      │
│  EXIF     │      │ 图片 cell 渲染  │
└───────────┘      └────────────────┘
```

## 模块职责

| 模块 | 职责 | 依赖 | 约束 |
|---|---|---|---|
| `main.rs` | 初始化 eframe，创建 MmCompare | eframe, ui | 不写业务逻辑 |
| `ui/imlayout.rs` | 统筹层：图片加载管线、键盘、标题、网格布局、交互编排；持有 FolderManager 做文件夹编排 | core, state, imcell, folder | 线程代码物理隔离在加载方法组内（ADR-0001） |
| `ui/folder.rs` | 文件夹 cell：扫描/缩略图/打开/导航/渲染（FolderManager + render_folder_cell） | core, state, imcell | 线程代码物理隔离在加载/扫描方法组内（ADR-0001/0006）；渲染不碰全局业务状态 |
| `state.rs` | 数据结构 + 状态转移薄方法（含 FolderCell / ImageSource） | egui | 无逻辑，仅状态操作 |
| `core/image.rs` | 纯函数：解码、缩略图、旋转、直方图、RGB 统计、标签格式化、EXIF | image, nom-exif, jpeg-decoder | 禁止任何 GUI 类型；可脱离 GUI 单测 |
| `ui/imcell.rs` | 图片 cell 渲染：居中绘制、覆盖层、纹理重建、旋转封装 | egui, state, core | 只画/只算，不碰业务状态（返回结果由 imlayout 应用） |

## 数据流

### 图片加载

```
用户拖拽文件
  → imlayout: poll_drops() 取 dropped_files
  → classify_paths() 分离文件/目录 → spawn_loaders(Standalone)
  → 子线程：读文件 → decode → EXIF → 直方图 → mpsc
  → 主线程：逐张上传 GPU → 收齐 append → 显示
```

详见 [loading.md](loading.md)（管线、载荷类型、失败处理）。

### 文件夹 cell

```
用户拖入目录
  → folder.rs: scan_folders() 子线程扫描 → poll_scan() 登记 FolderCell 入格
  → 缩略图批次排队（每批 ≤8 张）→ 列表/缩略图渲染（render_folder_cell）
  → 双击条目 → open_entry() 异步加载 → open_folder_entry()
  → 图片入格、文件夹 cell 隐藏；对比对（≥2 个文件夹图片）Space/B 同步索引、Esc 恢复
```

详见 [folder.md](folder.md)。

### 局部模式

```
用户按 P 键
  → imlayout: toggle state.local_mode
  → image_grid: 切换 cell Sense::drag()
  → 拖拽 → imcell::mouse_to_norm() 归一化 → state.selection
  → 松手 → core::compute_selection_stats() 每图 RGB 均值
  → core::format_cell_label() 生成标签文本 → imcell::draw_overlay() 展示
```

详见 [local-mode.md](local-mode.md)。

### 渲染

```
imlayout 计算 GridLayout（行列/尺寸/分隔线）
  → 逐 cell: render_image_cell() 编排交互（缩放/平移/重排/选择）
  → imcell::draw_image() 居中绘制 → imcell::draw_overlay() 覆盖层
  → 帧末: PanFeedback 统一应用到 state.pan / pan_offset
```

详见 [layout.md](layout.md)。

## 架构原则速查

| 原则 | 位置 | 理由 |
|---|---|---|
| 单线程心智模型 | ADR-0001 | egui 状态必须主线程；无锁无竞态 |
| 手动精确坐标 | ADR-0002 | 自动布局无法满足像素级对齐 |
| 加载管线分工 | ADR-0003 | 重 CPU 计算下沉子线程，纹理上传留主线程 |
| 模块边界 | ADR-0004/0005 | core 可单测、imcell 单格职责单一、imlayout 统筹所有 imcell |

## 已知边界与代价

- `ImageInfo.rgba` 全分辨率常驻内存（选择框统计/旋转需要）。
- 旋转、直方图重算仍在主线程全图遍历，大图单帧开销存在（加载管线已优化，交互路径未优化）。
- `available_size()` 为极小值时布局除零风险未防御。
