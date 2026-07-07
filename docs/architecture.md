# 架构

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
│  poll_drops()   拖拽文件追加图片    │
│  spawn_loaders() 起解码线程        │
│  poll_loading() 收结果 + 上传GPU   │
│  ui()           拼接 viewer       │
└────┬──────────────────────┬────────┘
     │                      │
┌────▼──────┐      ┌────────▼───────┐
│   core/   │      │     ui/        │
│  纯数据处理 │      │   纯界面渲染    │
│           │      │                │
│ image.rs  │      │ viewer.rs 布局  │
│  图片解码  │      │ cell.rs 渲染   │
│  亮度计算  │      │ widgets.rs 预留 │
│  标签格式化│      │                │
└───────────┘      └────────────────┘
```

## 数据流

```
用户拖拽文件
  → app.rs: poll_drops() 取 dropped_files
  → 过滤图片格式 → spawn_loaders(paths, append=true)
  → 子线程解码 → mpsc → 主线程上传 GPU → 显示

用户按 P 键
  → app.rs: toggle state.local_mode
  → viewer.rs: 切换 cell Sense::drag()
  → 拖拽 → cell::mouse_to_norm() 归一化 → state.selection
  → 松手 → core::compute_avg_y() 每图亮度
  → core::format_cell_label() 生成标签文本
  → cell.rs 展示
```

## 模块职责

| 模块 | 职责 | 依赖 |
|---|---|---|
| `main.rs` | 初始化 eframe，创建 MmCompare | eframe, app |
| `app.rs` | 编排，线程管理，键盘事件 | core, state, ui |
| `state.rs` | 数据结构 + 选择状态管理 | egui |
| `core/image.rs` | 纯函数：解码、avg_y、标签格式化 | image crate |
| `ui/viewer.rs` | 网格布局引擎 | egui, state, core, cell |
| `ui/cell.rs` | 图片渲染 + 选择覆盖层 | egui, state, core |
