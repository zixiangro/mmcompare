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
│  start_open()   开对话框 + 起线程   │
│  poll_drops()   拖拽文件追加图片    │
│  poll_loading() 收线程结果 + 上传GPU│
│  ui()           拼接 menu + viewer │
└────┬──────────────────────┬────────┘
     │                      │
┌────▼──────┐      ┌────────▼───────┐
│   core/   │      │     ui/        │
│  纯数据处理 │      │   纯界面渲染    │
│           │      │                │
│ image.rs  │      │ menu.rs 菜单栏  │
│  图片解码  │      │ viewer.rs 布局  │
│           │      │ cell.rs 渲染   │
│           │      │ widgets.rs 预留 │
└───────────┘      └────────────────┘
```

## 数据流

```
用户操作
  │
  ├── File > Open
  │     → menu.rs 设 pending_open = true
  │     → app.rs: 文件对话框 → spawn_loaders(paths, append=false)
  │
  ├── 拖拽文件
  │     → app.rs: poll_drops() 取 dropped_files
  │     → 过滤图片格式 → spawn_loaders(paths, append=true)
  │
  └── 按 P 键
        → app.rs: toggle state.local_mode
        → viewer.rs: 切换 cell Sense::drag()
        → 拖拽 → cell::mouse_to_norm() 归一化 → state.selection
        → 松手 → cell::compute_avg_y() 每图亮度
```

## 模块职责

| 模块 | 职责 | 依赖 |
|---|---|---|
| `main.rs` | 初始化 eframe，创建 MmCompare | eframe, app |
| `app.rs` | 编排，线程管理，键盘事件 | core, state, ui |
| `state.rs` | 数据结构 + 选择状态管理 | egui |
| `core/image.rs` | 纯函数，图片解码 | image crate |
| `ui/menu.rs` | 菜单栏渲染 | egui |
| `ui/viewer.rs` | 网格布局引擎 | egui, state, cell |
| `ui/cell.rs` | 图片渲染 + 选择覆盖层 + 亮度计算 | egui, state |
