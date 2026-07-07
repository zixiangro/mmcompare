# 布局引擎

## 概述

`viewer.rs` 是纯布局引擎，不包含任何图片绘制代码。它负责：
1. 根据图片数量决定行列数
2. 计算统一的 cell 尺寸
3. 摆放分隔线和 margin
4. 委托 `cell.rs` 渲染每个格子的内容

## 常量

| 常量 | 值 | 说明 |
|---|---|---|
| `SEP` | 1.0 | 分隔线粗细 (px) |
| `MARGIN` | 6.0 | 竖线两侧间距 & 窗口边缘留白 (px) |

## 行列规则

| 图片数 | 行数 | 排列 |
|---|---|---|
| 1-4 | 1 | 一行均分 |
| 5 | 2 | 3 + 2 |
| 6 | 2 | 3 + 3 |
| 7 | 2 | 4 + 3 |
| 8 | 2 | 4 + 4 |

## cell 尺寸计算

所有 cell 统一尺寸，以最大列数计算：

```
max_cols = max(所有行的列数)
cell_width  = (available.x - (max_cols - 1) × (MARGIN + SEP + MARGIN)) / max_cols
row_height  = (available.y - (rows - 1) × SEP) / rows
```

## 行内布局

完全手动坐标，不使用 `ui.horizontal` 或 `centered_and_justified`：

1. `allocate_exact_size(available.x, row_height)` 预留整行空间
2. 拿到 `row_rect`
3. 计算行内容总宽：`cols × cell_width + (cols-1) × 13`
4. 起始 x = `row_rect.left() + (available.x - 内容宽) / 2` 实现居中
5. 逐个摆放：margin(6) → sep(1) → margin(6) → cell

## 行间分隔

仅 1px 横线，无 margin：
```
allocate_exact_size(available.x, SEP)
painter().rect_filled()
```

## 为什么不用 egui 自动布局

`ui.horizontal` 在每个元素后自动加 `item_spacing`（包括最后一个），导致右侧不对称。`centered_and_justified` 会消费全部可用高度，导致多行布局失败。手动坐标完全可控。
