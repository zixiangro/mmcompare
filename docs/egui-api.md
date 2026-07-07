# egui 0.35 API 备忘

## 相比旧版的 Breaking Changes

| 旧 API | 新 API (0.35) |
|---|---|
| `TopBottomPanel::top(id)` | `Panel::top(id)` |
| `SidePanel::left(id)` | `Panel::left(id)` |
| `menu::bar(ui, \|ui\| ...)` | `MenuBar::new().ui(ui, \|ui\| ...)` |
| `ui.menu_button("File", \|ui\| ...)` | `menu::MenuButton::new("File").ui(ui, \|ui\| ...)` |
| `CentralPanel::default().show(ctx, ...)` | `CentralPanel::default().show(ui, ...)` |
| `ui.close_menu()` | `ui.close()` |
| `ui.drag_released_by(button)` | `ui.drag_stopped_by(button)` |

## 参数类型变化

- `.show()` 的第一个参数从 `&Context` 变为 `&mut Ui`
- `allocate_exact_size` 返回 `(Id, Response)`，rect 通过 `response.rect` 获取
- `Response` 可实现 `Deref<Target=Rect>`，大多数场景可直接当 Rect 用

## 其他

- `MenuButton` 在 `egui::menu::` 模块，不在顶层 `egui::`
- `MenuBar` 在顶层 `egui::`（通过 `containers::*` re-export）
- `rect_stroke()` 多了一个 `StrokeKind` 参数（含 `Inside`/`Outside`）
- `with_drag_and_drop(true)` 可启用文件拖拽支持
- `ctx.input(|i| i.raw.dropped_files.clone())` 获取拖入的文件列表
