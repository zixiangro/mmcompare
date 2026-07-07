# 局部模式 (Local Mode)

## 触发

按 `P` 键切换。`app.rs` 检测 `Key::P`，toggle `state.local_mode`。

## 选择框交互

1. **进入局部模式**：所有 cell 的 `Sense` 从 `hover()` 切换为 `drag()`
2. **拖拽开始**：记录起始鼠标位置，`cell::mouse_to_norm()` 转为归一化坐标 `[x, y]`（0..1）
3. **拖拽中**：实时更新 `state.selection`，框为**蓝色**
4. **松手**：框确定，计算平均亮度，框变为**红色**

## 归一化坐标

选择框使用归一化坐标 `[x1, y1, x2, y2]`（0..1 范围），相对于图片的实际显示区域。不同尺寸的图片用同一套归一化坐标，框的位置自动保持同步。

### 坐标转换

```
鼠标位置 → cell 坐标 → image_display_rect 坐标 → 归一化 [0..1]

mouse_to_norm(mouse_pos, cell_rect, img_size):
  1. image_display_rect() 算图片在 cell 中的实际区域
  2. (mouse - img_rect.min) / img_rect.size → [0..1]
```

## 平均亮度计算

`cell::compute_avg_y(img, selection)`：

1. 归一化坐标 → 像素坐标
2. 遍历选择区域内所有像素
3. Y = 0.299R + 0.587G + 0.114B (BT.601 luma)
4. 求均值

## 状态结构

```rust
AppState {
    images: Vec<ImageInfo>,     // 图片数据（含 rgba 像素）
    local_mode: bool,           // 是否在局部模式
    selection: Option<[f32;4]>, // 归一化选择框
    avg_y: Vec<Option<f32>>,    // 每张图的平均亮度
    drag_origin: Option<[f32;2]>, // 拖拽起始归一化坐标
}
```

## 模块分工

| 文件 | 职责 |
|---|---|
| `app.rs` | 按 P 切换模式 |
| `viewer.rs` | 局部模式下用 `Sense::drag()`，处理拖拽事件 |
| `cell.rs` | `draw_overlay()` 画框和文字，`compute_avg_y()` 像素采样，`mouse_to_norm()` 坐标转换 |
| `state.rs` | `drag_start/drag_update/drag_end` 状态管理 |
