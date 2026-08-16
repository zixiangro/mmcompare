# AGENTS.md — mmcompare

> 工程手册：本文件是仓库入口，包含项目速览、工程规范与文档导航。
> 详细技术内容一律在 [docs/README.md](docs/README.md)（文档地图），本文件不重复展开。

## 1. 项目概览

图片对比桌面应用（egui/eframe）：多图查看（1-8 张）、选择框同步亮度对比、缩放/平移、EXIF 摘要、直方图、旋转、重排、删除、双图对比。

| 领域 | 选型 |
|---|---|
| GUI | eframe 0.35 (wgpu) + egui 0.35 |
| 图片解码 | `image` 0.25 |
| EXIF | nom-exif 3.6 |
| 日志 | env_logger + log |
| 线程 | 仅 `std::thread::spawn` + `std::sync::mpsc` |

## 2. 文档导航

**入口：[docs/README.md](docs/README.md)**（文档矩阵 + 维护规则）。结构：

```
docs/
├── README.md          # 文档地图：索引 + 状态矩阵 + 变更触发点
├── architecture.md    # 系统架构：分层、数据流、模块职责
├── loading.md         # 图片加载管线（线程模型、失败处理）
├── layout.md          # 布局引擎：网格算法、坐标计算
├── folder.md          # 文件夹 cell：扫描、缩略图、打开/导航
├── local-mode.md      # 局部模式：选择框、归一化坐标
├── egui-api.md        # egui 0.35 API 差异备忘（参考）
└── decisions/         # ADR 架构决策记录（含模板）
    ├── 0001-single-threaded-model.md   # 单线程心智模型
    ├── 0002-manual-layout.md           # 手动坐标布局
    ├── 0003-loading-pipeline.md        # 加载管线分工
    ├── 0004-module-separation.md       # 模块边界
    └── 0005-merge-orchestration-into-imlayout.md  # 编排层与布局引擎合并为 imlayout
```

## 3. 工程规范

### 3.1 架构分层（ADR-0004）

```
core/   纯数据处理，零 GUI 依赖（解码、旋转、直方图、统计、标签、EXIF）
state.rs 纯数据结构，只有状态与状态转移薄方法
ui/     只读 state 渲染；交互结果写入 state，不直接改业务状态
        ├── imlayout.rs  统筹所有 cell（图片 + 文件夹）：加载管线、键盘事件、标题、
        │                网格布局、交互编排、文件夹扫描/打开/导航
        │                （唯一允许线程原语的模块）
        └── imcell.rs    单格渲染单元：给定"格子矩形 + cell 数据"，画好一个 cell
                        （图片居中绘制/覆盖层/旋转，文件夹列表/缩略图/菜单）
```

### 3.2 线程模型（ADR-0001）

- 除图片加载外全部在主线程运行；多线程代码**物理隔离**在 `imlayout.rs` 的 `spawn_loaders`/`poll_loading`/`poll_drops`/`drain_pending_drops`。
- 子线程用完即弃，线程间仅 `mpsc::channel`。**禁止 `Arc<Mutex<>>`、`RwLock`、线程池**。
- 重 CPU 计算（解码、EXIF、直方图）必须放子线程；主线程只做纹理上传（ADR-0003）。

### 3.3 布局与渲染解耦（ADR-0002/0004）

- `imlayout.rs` 统筹层：只算 cell 位置、画分隔线、编排交互、跑加载管线、管理文件夹 cell。不关心单个 cell 怎么画。
- `imcell.rs` 单格渲染单元：cell → 屏幕的一切（图片的居中绘制/纹理重建/旋转封装/覆盖层，文件夹的列表/缩略图/右键菜单）。不关心自己在哪、不碰 state。
- 像素算法（旋转/直方图/统计）在 `core`；`imcell` 封装"cell 操作"但不碰 state（返回值由 `imlayout` 写回）；选区失效等状态转移归 `state.rs`。
- 完全手动坐标（`allocate_exact_size` → `pos2` → `painter`/`allocate_rect`），不用自动布局。

### 3.4 代码风格

- 模块/函数/变量：蛇形命名；常量：`SCREAMING_SNAKE`；公共项不带 doc comment。
- **注释只保留文件头部的模块说明（`//!`）**：代码内不写行内/函数注释；需要解释的"为什么"（意图、约束、权衡）一律写进 docs/（或文件头部），避免注释与代码双份维护。
- 交互状态变更集中在帧末统一应用（参考 `imlayout.rs` 的 `PanFeedback` 模式），避免渲染中途改状态。
- clippy 0 警告、`cargo fmt` 通过是提交前提。纯绘制函数参数超限用 `#[allow(clippy::too_many_arguments)]`，不强行拆结构。

### 3.5 错误处理

- 用户可见错误：进 `state.load_errors`（横幅展示），失败路径从 `loaded_paths` 移除以支持重拖重试。
- 调试信息：`log::warn!` / `log::debug!`（env_logger 已初始化）。
- 第三方解析器可能 panic（nom-exif）：用 `catch_unwind` 兜底，返回空结果。

### 3.6 依赖管理

- 新增依赖需在 PR 说明理由；优先 `std` 与已有依赖。
- `egui`/`eframe`/`egui_extras` 版本必须三者一致（当前 0.35）。
- 升级 egui 后同步检查 [docs/egui-api.md](docs/egui-api.md)。

### 3.7 关键常量

| 常量 | 值 | 说明 |
|---|---|---|
| `MAX_IMAGES` (state.rs) | 8 | 图片硬上限，同时决定数字键旋转数量 |
| `SEP` (imlayout.rs) | 1.0 | 分隔线粗细 |
| `MARGIN` (imlayout.rs) | 6.0 | 竖线两侧间距 & 窗口边缘留白 |

## 4. 开发工作流

1. **改代码**：遵循第 3 节规范。
2. **验证**（提交前必须全部通过）：
   ```powershell
   cargo check --all-targets
   cargo clippy --all-targets   # 0 警告
   cargo fmt --check
   cargo build
   ```
3. **同步文档**：按 [docs/README.md](docs/README.md) 的"变更触发点"检查命中项并更新；涉及架构决策先更新对应 ADR（模板在 [docs/decisions/README.md](docs/decisions/README.md)）。
4. **提交**：短主语（≤50 字符，祈使句，不加句号），必要时 72 列正文说明"为什么"。参考个人 AGENTS.md 的 commit 规范。

## 5. 快捷键与交互速查

| 键 | 功能 |
|---|---|
| `P` | 局部模式：拖拽框选（归一化同步所有 cell）；右键移动选择框 |
| `E` / `H` | 切换 EXIF 摘要 / 直方图显示 |
| `1-8` | 旋转对应位置图片（顺时针 90°，仅独立拖入的图片） |
| `Q` | 按住：两张图时互换显示（对比） |
| `Space` / `B` | 文件夹打开的图片：上一张 / 下一张 |
| `Esc` | 关闭文件夹打开的图片，恢复文件夹视图 |
| 滚轮 | 全局缩放（图片模式下）；文件夹 cell 内为滚动条目 |
| 左键拖拽 | 缩放 >1 时全局平移；局部模式下为框选 |
| 右键拖拽 | 单 cell 平移 |
| `Ctrl` + 右键 | 删除当前 cell（图片或文件夹） |
| `Ctrl` + 左键拖拽 | 重排 |

拖入图片**或文件夹**均可；文件夹 cell 内双击条目打开图片，右键菜单管理条目。

## 6. 布局规则

| 图片数 | 布局 |
|---|---|
| 1-3 | 单行等宽（左右 6px margin） |
| 4 | 2 + 2 |
| 5 / 6 | 3+2 / 3+3 |
| 7 / 8 | 4+3 / 4+4 |

所有 cell 统一尺寸（按最大列数），行内居中，行间仅 1px 分隔线。
加载中网格正常渲染，顶部叠加半透明状态横幅（进度 + 失败列表），不整屏遮挡。
