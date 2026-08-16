# ADR-0007: 视频对比——单程序模式切换 + ffmpeg 解码

> 状态: 已接受 | 日期: 2026-08-16

## 背景（Context）

视频对比是 product.md 路线图的下一阶段（M1-M4）。开发前有两个层面的决策：

1. **程序形态**：视频与图像做成单程序（Ctrl+Tab 模式切换）还是两个程序（条件编译）？
2. **解码选型**：Rust 生态没有纯 Rust 的 H.264/H.265 解码器（symphonia 0.6.1 的
   `all-codecs` 只有音频与实验性 VP8/VP9），主流相机/手机视频（H.264/H.265）必须依赖
   ffmpeg 系；而 ffmpeg 绑定在"最少系统依赖"约束下如何落地？

约束：尽量少动系统环境（不装 VS/vcpkg/系统包）、单二进制分发（product.md）、
线程模型沿用 ADR-0001/0003。

## 决策（Decision）

**程序形态：方案 1——单程序，Ctrl+Tab 切换视频/图像模式。**
UI 层 90% 共享（布局/键盘/拖拽/管线/文件夹/横幅），两程序方案要么 cfg 撒点污染
模块边界（ADR-0004），要么双 binary 全量编译 + 行为漂移。模式独立交互用**模块边界**
保证：新增 `ui/video.rs`（视频 cell 渲染 + 播放交互），imlayout 按 mode 分发；
状态转移仍收在 state.rs 一处。

**解码：ffmpeg-next 9.0（绑定 FFmpeg 9.x），动态链接。**
- 只启用 `format` + `software-scaling` features（裁掉 device/filter/swresample，无音频）
- 只初始化视频流（D5）
- Windows: gyan.dev full_build-shared 9.0.1（MSVC .lib + include，GPL 构建）；
  运行 dll 由 build.rs 拷到 target/ 目录；**CI 已验证（Build + Test 全绿）**
- Linux: **暂缓**。BtbN linux64-lgpl-shared 下载/解压已通，bindgen 编译失败原因待查
- macOS: **暂缓**。brew install ffmpeg 在 Actions runner 上 6s 即失败（原因未明）；后备方案：Homebrew bottle 直下（ffmpeg 9.0.1，arm64_sequoia/tahoe digest 已备）
- 本机开发环境（无 VS）：CC=gcc + mingw 头文件 + pip libclang.dll + clang 内置头
  资源目录，全部配置在 `.cargo/config.toml`（force=false，CI 可覆盖）

**交互隔离（D1-D5）**：视频模式独立快捷键（Space 播放/暂停、←→ ±5s、↑↓ 帧步进、
Ctrl+箭头只调 hover 视频）；屏蔽 E/H/P 与重排；MAX_VIDEOS=4；Esc 仅文件夹打开时
返回；不做音频。详见 docs/todo.md。

## 后果（Consequences）

- 正面：模式间交互完全独立不互相污染；ffmpeg 白送容器/时间戳/seek 全链路；
  视频 cell 复用现有网格布局/平移缩放/文件夹子系统/加载管线模式。
- 代价：
  - 二进制 +ffmpeg dll（Windows 分发需带 4 个 dll，~80MB）
  - 许可：Windows 用 gyan GPL 构建（full_build-shared 含 x264 等 GPL 组件），
    分发受 GPL 约束——个人工具接受；后续可换 LGPL 构建（自己裁剪编译）降级
  - 构建环境复杂：本机无 VS 时需 CC/libclang/mingw 头文件三重配置（已固化到
    .cargo/config.toml，新机器照抄）
- 风险：
  - Linux/macOS CI 未验证（暂缓；Linux bindgen 编译失败、macOS brew 失败均待查）
  - 静态链接裁剪（体积优化）留待后续；当前动态链接 dll 分发

## 备选方案（Alternatives）

1. **两程序 + 条件编译**：拒绝。UI 层共享度极高，cfg 污染边界，双份行为漂移，
   CI 矩阵 ×2，用户双入口。
2. **openh264 + libde265 + mp4parse 组合**：体积小但容器/时间戳/seek 全自研
   （800-1500 行 + 调试），B 帧重排与帧级同步是对比对正确性地基，自研风险最贵；
   libde265 绑定冷门（0.2.0 个人维护）。
3. **symphonia**：纯 Rust 零依赖，但无 H.264/H.265 解码器，打不开主流视频。
4. **子进程调 ffmpeg**：违背单二进制分发，帧传输开销大。
5. **ffmpeg 静态链接**：运行期零依赖最理想，但 Windows 无现成 MSVC 静态 dev 包
   （BtbN win64 不带 include/lib），自编译违背"不折腾编译"；留作后续优化项。

## 关联

- 开发计划：docs/todo.md（M1-M5 里程碑与验收）
- 产品路线图：docs/product.md（视频对比 M1-M4）
- 线程模型：ADR-0001（解码线程物理隔离）、ADR-0003（重计算子线程 + 主线程纹理上传）
- 模块边界：ADR-0004（core/state/ui 分层）、ADR-0006（folder.rs 拆分，video.rs 同构）
