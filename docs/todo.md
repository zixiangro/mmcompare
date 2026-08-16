# 开发计划：视频对比

> 面向研发协作。状态: 进行中 | 更新: 2026-08-16
> 本文件是视频对比开发的**跨 session 同步载体**：每个 session 开工先读本文件，完成项打勾并刷新日期。

## 架构方案（已确认）

**方案 1：单程序 + Ctrl+Tab 模式切换**。视频与图像同进程，模式间交互独立。
拒绝方案 2（两程序 + 条件编译）：UI 层 90% 共享，cfg 污染模块边界（ADR-0004），
双 binary 全量编译 + 行为漂移。详见 ADR-0007。

## 交互决策（已拍板）

| 项 | 规则 |
|---|---|
| D1 播放 | `Space` 播放/暂停；`Esc` 仅对"从文件夹 cell 打开"的视频返回文件夹视图，直接拖入的视频用 `Ctrl`+右键关闭（同图片模式） |
| D1 箭头 | `←`/`→` 全部视频 ±5s；`↑`/`↓` 全部视频 ±1 帧；`Ctrl`+任意箭头只调**鼠标悬停**的那个视频 |
| D1 字母键 | 视频模式屏蔽 E/H/P 等字母快捷键（那是图片模式专用） |
| D2 隔离 | 视频模式不支持局部模式 P、EXIF E、直方图 H、重排；不与图片交互混用 |
| D3 上限 | `MAX_VIDEOS = 4`（独立常量，不占 `MAX_IMAGES`） |
| D4 缩略图 | 文件夹 cell 视频首帧缩略图：先性能验证，影响不大才做 |
| D5 音频 | 不做。解码只初始化视频流，不碰音频 |

## 里程碑

### M0 基线（完成）

- [x] 重建 dev 分支（含 CI 配置，触发 master + dev）
- [x] 方案 1 架构决策确认

### M1 解码选型 spike（先决任务）

- [x] 解码候选对比：结论进 ADR-0007 备选方案节（symphonia 无 H.264/265；openh264+libde265 自研成本高；子进程违背单二进制）
- [x] dev 引入 ffmpeg-next 9.0（format + software-scaling，裁掉 device/filter/swresample）；三平台 CI 配置完成，**macOS 暂缓**（brew 在 runner 失败，bottle 直下方案已备好）
- [x] `core/video.rs` 雏形：read_info（时长/帧率/尺寸）+ first_frame（swscale 转 RGB24，可降采样）；只初始化视频流（D5）；4 个单测
- [x] 降采样策略：first_frame 最长边限制 + BILINEAR（swscale）
- [x] ADR-0007：已接受（单程序模式切换 + ffmpeg 动态链接 + 交互隔离）
- [x] 本机无 VS 环境的构建链路固化到 .cargo/config.toml（CC=gcc / libclang.dll / mingw 头 + clang 资源目录）
- [x] build.rs：Windows 自动拷 ffmpeg dll 到 target/，cargo run/test 免手动配 PATH
- [x] **验收：dev CI（Windows）绿**——Build + Test 全过（run 31946979109，35 测试）；Linux/macOS CI 暂缓（Linux 编译问题待查、macOS brew 问题待查）

### M2 视频 cell（单视频播放闭环）

- [x] `state.rs`：`CellKind::Video(usize)` + `video_cells` 存储 + 索引约定维护；`MAX_VIDEOS = 4`（D3）
- [x] `classify_paths` 识别视频扩展名（is_video_ext），拖拽/命令行打开；视频批优先切视频模式（M2 简化：清空图片/文件夹，M3 改保留状态切换）
- [x] 解码管线：`spawn_video_worker` 子线程 + mpsc 帧通道（ADR-0001/0003：线程原语只进 imlayout 加载方法组；rx 被主线程丢弃即停止）
- [x] `ui/video.rs`：帧纹理上传、居中绘制（letterbox）、控制条（播放/暂停、进度 seek、时间）
- [x] 播放时钟：帧驱动（worker 按帧率发帧，position 跟随 pts；暂停时 seek/步进由单帧 worker 完成）
- [x] 交互 D1：Space 播放/暂停；`←`/`→` ±5s；`↑`/`↓` 帧步进；`Ctrl`+箭头仅调 hover 视频
- [x] 交互 D2：视频模式屏蔽 E/H/P（is_all_images=false 自然隔离）、无缩放平移/重排
- [x] 失败路径：首帧失败进 `load_errors` + 移除 loaded_paths（重拖重试）；播放中失败标记 cell.failed
- [ ] **验收：GUI 手动验证**（拖入 sample 或真实视频：播放/暂停/seek/步进/进度条/删除）；CI 已跑（44 测试）
- [ ] Esc 返回文件夹视图：M2 无文件夹来源的视频，留 M4（对比对）一并做

### M3 模式切换（Ctrl+Tab）

- [ ] `state.rs`：mode enum（Image/Video），切换各自保留视图状态（zoom/pan/局部模式/选中）
- [ ] 拖拽互斥：视频模式拖入图片 → 横幅提示（沿用模式互斥规则）；Ctrl+Tab 手动切换
- [ ] README 快捷键表草稿（视频段）
- ✅ 验收：双模式来回切无状态串扰；互斥拖拽有提示

### M4 视频对比对（同步播放）

- [ ] 对比对判定扩展：视频模式"每文件夹 ≤1 张" + 同步导航
- [ ] 同步语义：非 Ctrl 箭头 = 全部视频同步调整（帧号对齐）；Space 同步播放/暂停（主从时钟）
- [ ] 复用：Esc 退出恢复文件夹视图、Ctrl+右键门控（对比对禁删）
- ✅ 验收：双视频同步播放/步进与图像对比对行为一致

### M5 性能与收尾

- [ ] 内存上限：帧缓冲上限、降采样、停止即释放
- [ ] D4 性能验证：文件夹 cell 视频首帧缩略图——批量解码成本实测（对比缩略图管线 200 张上限），影响不大则接入（可限制数量/懒加载）
- [ ] 文档收尾：architecture.md 代码地图、loading.md 管线表、product.md 勾选、README 快捷键/FAQ、ADR-0007 转已接受
- [ ] 全量回归：既有 25 测试 + 新增视频测试（core 解码、state 门控、交互隔离）

## 质量门（每个里程碑必过）

```powershell
cargo check --all-targets
cargo clippy --all-targets   # 0 警告
cargo fmt --check
cargo build
cargo test
```

## 相关文档

- [文档地图](README.md) · [产品路线图](product.md) · [架构](architecture.md)
- [ADR-0007](../docs/decisions/0007-video-mode.md)（提案中）
