# 系统架构

> 状态: 稳定 | 更新: 2026-07-31 | 关联: [ADR-0004](decisions/0004-module-separation.md)

## 分层

```
┌────────────┐     ┌──────────────┐
│   main.rs  │     │   state.rs   │
│  入口初始化  │     │  全局共享数据  │
└─────┬──────┘     └──────┬───────┘
      │                   │
┌─────▼───────────────────▼─────────┐
│            app.rs                  │
│  编排层：串联 core ↔ state ↔ ui    │
│                                    │
│  filter_paths()  过滤/排序/截断    │
│  spawn_loaders() 起解码线程        │
│  poll_loading()  收结果 + 上传GPU  │
│  poll_drops()    拖拽文件追加图片  │
│  drain_pending_drops() 处理缓存    │
│  ui()            拼接 viewer       │
└────┬──────────────────────┬────────┘
     │                      │
┌────▼──────┐      ┌────────▼───────┐
│   core/   │      │     ui/        │
│  纯数据处理 │      │   纯界面渲染    │
│           │      │                │
│ image.rs  │      │ viewer.rs 布局  │
│  解码/旋转 │      │ imcell.rs 渲染  │
│  直方图   │      │                │
│  统计/标签│      │                │
│  EXIF     │      │                │
└───────────┘      └────────────────┘
```

## 模块职责

| 模块 | 职责 | 依赖 | 约束 |
|---|---|---|---|
| `main.rs` | 初始化 eframe，创建 MmCompare | eframe, app | 不写业务逻辑 |
| `app.rs` | 编排：加载管线、键盘事件、标题、拖拽 | core, state, ui | 唯一允许出现线程原语的模块（ADR-0001）；超过 ~400 行应拆分 |
| `state.rs` | 数据结构 + 状态转移薄方法 | egui | 无逻辑，仅状态操作（append/remove/swap/drag） |
| `core/image.rs` | 纯函数：解码、旋转、直方图、RGB 统计、标签格式化、EXIF | image, nom-exif | 禁止任何 GUI 类型；可脱离 GUI 单测 |
| `ui/viewer.rs` | 布局引擎 + 交互编排 | egui, state, core, imcell | 只算位置/画分隔线，不关心图片怎么画 |
| `ui/imcell.rs` | 渲染单元：居中画图、选择覆盖层、标签/直方图 | egui, state, core | 只画，不关心自己在哪、不处理输入 |

## 数据流

### 图片加载

```
用户拖拽文件
  → app.rs: poll_drops() 取 dropped_files
  → filter_paths() 过滤格式 → spawn_loaders(paths, append=true)
  → 子线程：读文件 → decode → EXIF → 直方图 → mpsc
  → 主线程：逐张上传 GPU → 收齐 append → 显示
```

详见 [loading.md](loading.md)（管线、载荷类型、失败处理）。

### 局部模式

```
用户按 P 键
  → app.rs: toggle state.local_mode
  → viewer.rs: 切换 cell Sense::drag()
  → 拖拽 → imcell::mouse_to_norm() 归一化 → state.selection
  → 松手 → core::compute_selection_stats() 每图 RGB 均值
  → core::format_cell_label() 生成标签文本 → imcell.rs 展示
```

详见 [local-mode.md](local-mode.md)。

### 渲染

```
viewer.rs 计算 GridLayout（行列/尺寸/分隔线）
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
| 模块边界 | ADR-0004 | core 可单测、ui 职责单一、app 薄编排 |

## 已知边界与代价

- `ImageInfo.rgba` 全分辨率常驻内存（选择框统计/旋转需要）。
- 旋转、直方图重算仍在主线程全图遍历，大图单帧开销存在（加载管线已优化，交互路径未优化）。
- `available_size()` 为极小值时布局除零风险未防御。
