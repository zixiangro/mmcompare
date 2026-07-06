# mmcompare

图片查看 / 对比工具，基于 [egui](https://github.com/emilk/egui) 的桌面应用。

## 运行

```powershell
cargo run
```

## 功能

- **多图查看**：支持 1-8 张图片同时显示
- **自适应布局**：1-4 张单行均分，5-8 张双行排列
- **并行加载**：图片解码在后台线程并行执行，不阻塞 UI

## 布局

| 数量 | 排列 |
|---|---|
| 1-4 | 单行等宽 |
| 5 | 首行 3 + 次行 2 |
| 6 | 首行 3 + 次行 3 |
| 7 | 首行 4 + 次行 3 |
| 8 | 首行 4 + 次行 4 |

## 技术栈

- **GUI**: eframe 0.35 + egui 0.35 (wgpu)
- **图片**: `image` 0.25
- **对话框**: `rfd` 0.17

## 项目结构

```
src/
├── main.rs          # 入口
├── app.rs           # 编排层
├── state.rs         # 共享状态
├── core/
│   └── image.rs     # 图片解码
└── ui/
    ├── menu.rs      # 菜单栏
    ├── viewer.rs    # 网格布局
    └── widgets.rs   # 通用组件
```

详见 [`DOC.md`](DOC.md) 和 [`AGENTS.md`](AGENTS.md)。
