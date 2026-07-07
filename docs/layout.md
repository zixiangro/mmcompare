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
| 1-3 | 1 | 一行均分 |
| 4 | 2 | 2 + 2 |
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

完全手动坐标：

1. `allocate_exact_size(available.x, row_height)` 预留整行空间，拿到 `row_rect`
2. 计算行内容总宽：`cols × cell_width + (cols-1) × (MARGIN + SEP + MARGIN)`
3. 起始 x = `row_rect.left() + (available.x - 内容宽) / 2` 实现居中
4. `paint_zone()` 逐个摆放：margin → sep → margin → cell

`paint_zone` 用 `Rect::from_min_size(pos2(x, row_rect.top()), vec2(width, row_height))` 精确定位，用 `ui.painter().rect_filled()` 绘制分隔线，用 `ui.allocate_rect()` 标记占用。

## 行间分隔

仅 1px 横线，无 margin：
```
allocate_exact_size(available.x, SEP)
painter().rect_filled(response.rect)
```

## 交互

cell 交互用 `ui.interact(cell_rect, id, Sense::drag())`，直接指定 `cell_rect` 作为交互区域，不依赖 egui 的自动布局。

## 为什么不用 egui 自动布局

`ui.horizontal` 在每个元素后自动加 `item_spacing`（包括最后一个），导致右侧不对称。`centered_and_justified` 会消费全部可用高度，导致多行布局失败。手动坐标完全可控。
