# 架构决策记录（ADR）

> 状态: 稳定 | 更新: 2026-08-16

## 什么是 ADR

ADR（Architecture Decision Record）把"为什么代码长这样"写下来。代码回答 *what*，ADR 回答 *why*。
本项目的 ADR 简短实用：每条记录一个决策的背景、选择、后果与备选方案，防止未来重构无意破坏当初的权衡。

## 索引

| 编号 | 决策 | 状态 |
|---|---|---|
| [0001](0001-single-threaded-model.md) | 单线程心智模型，多线程物理隔离在 ui/imlayout.rs | 已接受 |
| [0002](0002-manual-layout.md) | 手动精确坐标布局，不用 egui 自动布局 | 已接受 |
| [0003](0003-loading-pipeline.md) | 解码管线：子线程纯 CPU 计算 + 主线程纹理上传 | 已接受 |
| [0004](0004-module-separation.md) | 前后端分层 + 布局/渲染解耦 | 已接受（ui 划分被 0005 取代） |
| [0005](0005-merge-orchestration-into-imlayout.md) | 合并编排层与布局引擎为 imlayout（统筹所有 cell） | 已接受（文件夹部分被 0006 取代） |
| [0006](0006-folder-module-separation.md) | 文件夹管理拆分独立模块 folder.rs | 已接受 |
| [0007](0007-video-mode.md) | 视频对比：单程序模式切换 + ffmpeg 解码 | 已接受 |

## 模板

新增决策时复制以下骨架到 `docs/decisions/NNNN-short-name.md`：

```markdown
# ADR-NNNN: 决策标题

> 状态: 提案中 / 已接受 / 已废弃 | 日期: YYYY-MM-DD

## 背景（Context）
为什么需要这个决策？当时面临什么问题？

## 决策（Decision）
我们选择怎么做？一句话说清楚。

## 后果（Consequences）
- 正面：……
- 代价：……
- 风险：……

## 备选方案（Alternatives）
考虑过哪些其他方案？为什么放弃？

## 关联
- 相关代码位置 / 相关 ADR / 相关文档
```

## 规则

- 每条 ADR 只记录**一个**决策。
- 决策被推翻时，不要改写历史：在新 ADR 里声明"废弃 000X"，旧记录保留。
- 写代码前先确认是否有未记录的隐含决策；有则补 ADR。
