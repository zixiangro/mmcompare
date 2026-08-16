# 局部模式 (Local Mode)

> 状态: 稳定 | 更新: 2026-08-16 | 关联: [architecture.md](architecture.md) · [layout.md](layout.md)

## 触发

按 `P` 键切换。`ui/imlayout.rs` 检测 `Key::P`，toggle `state.local_mode`。

## 选择框交互

1. **进入局部模式**：所有 cell 的 `Sense` 从 `hover()` 切换为 `drag()`
2. **拖拽开始**：记录起始鼠标位置，`imcell::mouse_to_norm()` 转为归一化坐标 `[x, y]`（0..1）
3. **拖拽中**：实时更新 `state.selection`，框为**蓝色**
4. **松手**：框确定，计算 RGB 均值，框变为**红色**
5. **右键拖拽**：移动已有选择框（`DragKind::MoveSelection`）

## 归一化坐标

选择框使用归一化坐标 `[x1, y1, x2, y2]`（0..1 范围），相对于图片的实际显示区域。不同尺寸的图片用同一套归一化坐标，框的位置自动保持同步。

### 坐标转换

```
鼠标位置 → cell 坐标 → image_display_rect 坐标 → 归一化 [0..1]

mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan):
  1. image_display_rect() 算图片在 cell 中的实际区域（含缩放/平移）
  2. (mouse - img_rect.min) / img_rect.size → [0..1]
```

## 平均亮度计算

`core::image::compute_selection_stats(rgba, w, h, selection)`：

1. 归一化坐标 → 像素坐标
2. 遍历选择区域内所有像素
3. 分别累加 R / G / B，求均值（归一化到 0..1）

标签文本由 `core::image::format_cell_label(stats)` 生成：
Luma（BT.601 加权）、R/G、B/G、饱和度、RGB 分量。

## 状态结构

```rust
AppState {
    image_cells: Vec<ImageCell>,   // 图片数据（info + selection + avg_stats）
    cell_order: Vec<CellKind>,     // 显示顺序
    local_mode: bool,              // 是否在局部模式
    drag_origin: Option<[f32;2]>,  // 拖拽起始归一化坐标
    drag_cell: Option<usize>,      // 拖拽所在 cell
    drag_kind: Option<DragKind>,   // NewSelection / MoveSelection
}
```

## 模块分工

| 文件 | 职责 |
|---|---|
| `ui/imlayout.rs` | 按 P 切换模式；局部模式下用 `Sense::drag()`，`handle_drag()` 编排拖拽事件 |
| `ui/imcell.rs` | `draw_overlay()` 画框和文字，`mouse_to_norm()` 坐标转换 |
| `state.rs` | `drag_start_new/drag_start_move/drag_update/drag_end` 状态管理 |
| `core/image.rs` | `compute_selection_stats()` 像素采样，`format_cell_label()` 标签格式化 |
