# AGENTS.md — mmcompare

## 项目概述

图片对比工具，基于 egui/eframe 的桌面应用。
当前阶段：简单的多图查看器。

## 技术栈

- **GUI**: eframe 0.35 (wgpu 后端) + egui 0.35
- **图片解码**: `image` crate 0.25
- **文件对话框**: `rfd` 0.17
- **日志**: env_logger
- **线程**: 仅 `std::thread::spawn` + `std::sync::mpsc`

## 项目结构

```
src/
├── main.rs          # 入口，初始化 eframe
├── app.rs           # eframe::App 实现 + 编排层
├── state.rs         # 全局状态（ImageInfo, AppState）
├── core/
│   ├── mod.rs
│   └── image.rs     # 图片解码（纯函数，线程安全）
├── ui/
    ├── mod.rs
    ├── menu.rs      # 菜单栏
    ├── viewer.rs    # 布局引擎：算位置 + 分隔线
    ├── cell.rs      # 图片渲染：居中画图
    └── widgets.rs   # 通用组件（预留）
```

## 架构原则

### 1. 单线程心智模型
除了图片解码，全部在主线程运行。多线程代码**物理隔离**在 `app.rs` 的 `start_open` + `poll_loading` 两个方法中。子线程用完即弃，线程间仅通过 `mpsc::channel` 通信。无 `Arc<Mutex<>>`、无全局线程池。

### 2. 前后端分层
- **ui/**: 只读取 state，渲染界面
- **core/**: 纯数据处理，不含任何 GUI 依赖
- **app.rs**: 薄编排层，串联 core → state → ui
- **state.rs**: 纯数据结构，不含逻辑

### 3. 布局与渲染解耦
- **viewer.rs**: 布局引擎，只负责计算 cell 位置、画分隔线、控制间距。不关心图片怎么画。
- **cell.rs**: 渲染单元，只负责"给我一个图片+矩形，我居中画出来"。不关心自己在哪里。

### 4. 手动精确坐标
egui 的自动布局（`ui.horizontal`、`item_spacing`、`centered_and_justified`）在需要精确对齐时有各种 edge case。当前 `viewer.rs` 采用完全手动布局：`allocate_exact_size` 预留空间 → `Rect::from_min_size` 计算位置 → `ui.painter()` / `ui.put()` 精确绘制。

## 关键常量

| 常量 | 值 | 说明 |
|---|---|---|
| `SEP` (viewer.rs) | 1.0 | 分隔线粗细 |
| `MARGIN` (viewer.rs) | 6.0 | 竖线两侧间距 & 窗口边缘留白 |

## 图片加载流程

```
用户点击 Open → 文件对话框(主线程,阻塞) → 每张图起一个临时线程解码
→ 主线程每帧 try_recv 收图片 → 收齐后上传 GPU 纹理 → 显示
```

## 布局规则

| 图片数 | 布局 |
|---|---|
| 1-4 | 单行，等宽均分，左右 6px margin |
| 5 | 首行 3 + 次行 2，1px 横线分隔 |
| 6 | 首行 3 + 次行 3 |
| 7 | 首行 4 + 次行 3 |
| 8 | 首行 4 + 次行 4 |

所有 cell 统一尺寸（按最大列数计算），行内居中，行间距仅 1px 分隔线无 margin。

## egui 0.35 API 注意事项

相比旧版有较多 breaking changes：
- `TopBottomPanel` → `Panel::top()` / `Panel::bottom()`
- `SidePanel` → `Panel::left()` / `Panel::right()`
- `menu::bar()` → `MenuBar::new().ui(ui, ...)`
- `menu_button()` → `menu::MenuButton::new().ui(ui, ...)`
- `.show(ctx, ...)` → `.show(ui, ...)` （参数从 `&Context` 变为 `&mut Ui`）
- `ui.close_menu()` → `ui.close()`
- `allocate_exact_size` 返回 `(Id, Response)`，rect 通过 `response.rect` 获取
