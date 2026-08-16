//! 单格渲染单元：给定"格子矩形 + cell 数据"，把一个格子画好。
//!
//! 支持两种 cell：
//! - 图片 cell：居中绘制、选择覆盖层、标签/直方图/EXIF、纹理重建、旋转封装；
//! - 文件夹 cell：列表/缩略图双视图、滚动、选择、右键菜单。
//!
//! 不关心自己在哪、不关心有几个格子、不碰业务状态——需要改 state 的
//! 操作（旋转、纹理重建、打开条目）只返回数据或 `FolderAction`，
//! 由 imlayout（统筹层）写回。

use std::cell::RefCell;
use std::rc::Rc;

use eframe::egui;

use crate::core;
use crate::state::{FolderAction, FolderCell, FolderView, ImageInfo, NormRect};

const ROW_H: f32 = 52.0;
const THUMB_SIZE: f32 = 44.0;
const THUMB_PAD: f32 = 6.0;
const GRID_CELL: f32 = 110.0;
const GRID_PAD: f32 = 8.0;

pub fn upload_texture(
    ctx: &egui::Context,
    rgba: &[u8],
    size: [usize; 2],
    name: &str,
) -> egui::TextureHandle {
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba);
    ctx.load_texture(name, color_image, egui::TextureOptions::default())
}

pub fn rotate_image(info: &ImageInfo, ctx: &egui::Context) -> ImageInfo {
    let (rgba, size) = core::image::rotate_rgba_90_cw(&info.rgba, info.size[0], info.size[1]);
    let histogram = core::image::compute_y_histogram(&rgba);
    let name = info
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image");
    let texture = upload_texture(ctx, &rgba, size, name);
    ImageInfo {
        texture,
        size,
        rgba,
        path: info.path.clone(),
        exif: info.exif.clone(),
        histogram,
    }
}

pub fn draw_image(ui: &mut egui::Ui, img: &ImageInfo, rect: egui::Rect, zoom: f32, pan: [f32; 2]) {
    let img_rect = image_display_rect(rect, img.size, zoom, pan);
    ui.painter().with_clip_rect(rect).image(
        img.texture.id(),
        img_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

pub fn image_display_rect(
    cell_rect: egui::Rect,
    img_size: [usize; 2],
    zoom: f32,
    pan: [f32; 2],
) -> egui::Rect {
    let img_w = img_size[0] as f32;
    let img_h = img_size[1] as f32;
    let scale = (cell_rect.width() / img_w).min(cell_rect.height() / img_h) * zoom;
    let dw = img_w * scale;
    let dh = img_h * scale;
    let cw = cell_rect.width();
    let ch = cell_rect.height();
    let ox = if dw > cw {
        ((cw - dw) / 2.0 + pan[0]).clamp(cw - dw, 0.0)
    } else {
        (cw - dw) / 2.0
    };
    let oy = if dh > ch {
        ((ch - dh) / 2.0 + pan[1]).clamp(ch - dh, 0.0)
    } else {
        (ch - dh) / 2.0
    };
    egui::Rect::from_min_size(cell_rect.min + egui::vec2(ox, oy), egui::vec2(dw, dh))
}

pub fn clamp_pan(
    raw: [f32; 2],
    cell_rect: egui::Rect,
    img_size: [usize; 2],
    zoom: f32,
) -> [f32; 2] {
    let img_w = img_size[0] as f32;
    let img_h = img_size[1] as f32;
    let scale = (cell_rect.width() / img_w).min(cell_rect.height() / img_h) * zoom;
    let dw = img_w * scale;
    let dh = img_h * scale;
    let cw = cell_rect.width();
    let ch = cell_rect.height();
    let cx = (cw - dw) / 2.0;
    let cy = (ch - dh) / 2.0;
    let clamped_x = if dw > cw {
        (cx + raw[0]).clamp(cw - dw, 0.0)
    } else {
        cx
    };
    let clamped_y = if dh > ch {
        (cy + raw[1]).clamp(ch - dh, 0.0)
    } else {
        cy
    };
    [clamped_x - cx, clamped_y - cy]
}

pub fn mouse_to_norm(
    mouse_pos: egui::Pos2,
    cell_rect: egui::Rect,
    img_size: [usize; 2],
    zoom: f32,
    pan: [f32; 2],
) -> Option<[f32; 2]> {
    if !cell_rect.contains(mouse_pos) {
        return None;
    }
    let img_rect = image_display_rect(cell_rect, img_size, zoom, pan);
    Some([
        ((mouse_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0),
        ((mouse_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0),
    ])
}

#[allow(clippy::too_many_arguments)]
pub fn draw_overlay(
    ui: &mut egui::Ui,
    cell_rect: egui::Rect,
    img: &ImageInfo,
    selection: Option<NormRect>,
    avg_y_label: &str,
    exif: &str,
    histogram: &[u32; 256],
    zoom: f32,
    pan: [f32; 2],
    is_dragging: bool,
) {
    let img_rect = image_display_rect(cell_rect, img.size, zoom, pan);

    if let Some(sel) = selection {
        let (x1, y1) = (
            img_rect.min.x + sel[0] * img_rect.width(),
            img_rect.min.y + sel[1] * img_rect.height(),
        );
        let (x2, y2) = (
            img_rect.min.x + sel[2] * img_rect.width(),
            img_rect.min.y + sel[3] * img_rect.height(),
        );
        let sel_rect = egui::Rect::from_min_max(egui::pos2(x1, y1), egui::pos2(x2, y2));
        let color = if is_dragging {
            egui::Color32::from_rgb(0, 180, 255)
        } else {
            egui::Color32::from_rgb(255, 80, 80)
        };
        ui.painter().with_clip_rect(cell_rect).rect_stroke(
            sel_rect,
            0.0,
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Inside,
        );
    }

    draw_label(ui, cell_rect, exif, egui::Align2::LEFT_TOP, 10.0);
    draw_histogram(ui, cell_rect, histogram);
    draw_label(ui, cell_rect, avg_y_label, egui::Align2::LEFT_BOTTOM, 11.0);
}

fn draw_label(
    ui: &mut egui::Ui,
    cell_rect: egui::Rect,
    text: &str,
    align: egui::Align2,
    font_size: f32,
) {
    if text.is_empty() {
        return;
    }
    let font = egui::FontId::monospace(font_size);
    let lines: Vec<&str> = text.lines().collect();
    let bg = egui::Color32::from_black_alpha(160);
    let white = egui::Color32::WHITE;

    let mut y_cursor: f32;
    if align == egui::Align2::LEFT_TOP {
        y_cursor = cell_rect.top();
        for line in &lines {
            let sz = ui.fonts_mut(|f| {
                f.layout_no_wrap(line.to_string(), font.clone(), white)
                    .size()
            });
            let bg_r = egui::Rect::from_min_size(
                egui::pos2(cell_rect.left(), y_cursor),
                egui::vec2(sz.x + 6.0, sz.y + 4.0),
            );
            ui.painter().rect_filled(bg_r, 0.0, bg);
            ui.painter().text(
                bg_r.min + egui::vec2(3.0, 2.0),
                egui::Align2::LEFT_TOP,
                line,
                font.clone(),
                white,
            );
            y_cursor += sz.y + 4.0;
        }
    } else {
        y_cursor = cell_rect.bottom();
        for line in lines.iter().rev() {
            let sz = ui.fonts_mut(|f| {
                f.layout_no_wrap(line.to_string(), font.clone(), white)
                    .size()
            });
            y_cursor -= sz.y + 4.0;
            let bg_r = egui::Rect::from_min_size(
                egui::pos2(cell_rect.left(), y_cursor),
                egui::vec2(sz.x + 6.0, sz.y + 4.0),
            );
            ui.painter().rect_filled(bg_r, 0.0, bg);
            ui.painter().text(
                bg_r.min + egui::vec2(3.0, 2.0),
                egui::Align2::LEFT_TOP,
                line,
                font.clone(),
                white,
            );
        }
    }
}

fn draw_histogram(ui: &mut egui::Ui, cell_rect: egui::Rect, hist: &[u32; 256]) {
    if hist.iter().all(|&c| c == 0) {
        return;
    }
    let hist_w = 80.0;
    let hist_h = 48.0;
    let rect = egui::Rect::from_min_size(
        cell_rect.right_top() + egui::vec2(-hist_w - 2.0, 2.0),
        egui::vec2(hist_w, hist_h),
    );
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(160));

    let max_count = *hist.iter().max().unwrap_or(&1).max(&1) as f32;
    let bar_w = hist_w / 256.0;
    for (i, &count) in hist.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let h = (count as f32 / max_count * hist_h).max(0.5);
        let x = rect.left() + i as f32 * bar_w;
        let bar = egui::Rect::from_min_size(
            egui::pos2(x, rect.bottom() - h),
            egui::vec2(bar_w.max(0.5), h),
        );
        ui.painter().rect_filled(bar, 0.0, egui::Color32::WHITE);
    }
}

pub fn render_folder_cell(
    ui: &mut egui::Ui,
    folder: &mut FolderCell,
    cell_rect: egui::Rect,
) -> FolderAction {
    ui.painter()
        .rect_filled(cell_rect, 0.0, egui::Color32::from_gray(248));
    ui.allocate_rect(cell_rect, egui::Sense::hover());

    if folder.entries.is_empty() {
        ui.painter().text(
            cell_rect.center(),
            egui::Align2::CENTER_CENTER,
            "No images in folder",
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(150),
        );
        return FolderAction::None;
    }

    let hover_pos = ui.input(|i| i.pointer.hover_pos());
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    let shift = ui.input(|i| i.modifiers.shift);
    let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);

    if matches!(folder.view_mode, FolderView::List) {
        render_list(ui, folder, cell_rect, hover_pos, ctrl, shift, scroll_delta)
    } else {
        render_grid(ui, folder, cell_rect, hover_pos, ctrl, shift, scroll_delta)
    }
}

fn render_list(
    ui: &mut egui::Ui,
    folder: &mut FolderCell,
    cell_rect: egui::Rect,
    hover_pos: Option<egui::Pos2>,
    ctrl: bool,
    shift: bool,
    scroll_delta: f32,
) -> FolderAction {
    let total_h = folder.entries.len() as f32 * ROW_H;
    let max_scroll = (total_h - cell_rect.height()).max(0.0);
    folder.scroll_offset = (folder.scroll_offset - scroll_delta).clamp(0.0, max_scroll);
    let base_y = cell_rect.top() - folder.scroll_offset;
    let menu_action = Rc::new(RefCell::new(None));
    let mut result = FolderAction::None;

    for i in 0..folder.entries.len() {
        let y = base_y + i as f32 * ROW_H;
        if y + ROW_H < cell_rect.top() || y > cell_rect.bottom() {
            continue;
        }
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(cell_rect.left(), y),
            egui::vec2(cell_rect.width(), ROW_H),
        );
        let is_hovered = hover_pos.is_some_and(|hp| row_rect.contains(hp));
        let is_selected = folder.selected.contains(&i);

        let bg = if is_selected {
            egui::Color32::from_rgb(200, 220, 255)
        } else if is_hovered {
            egui::Color32::from_gray(225)
        } else {
            egui::Color32::from_gray(248)
        };
        ui.painter().rect_filled(row_rect, 0.0, bg);

        let thumb_rect = egui::Rect::from_min_size(
            row_rect.left_center() + egui::vec2(THUMB_PAD, -THUMB_SIZE / 2.0),
            egui::vec2(THUMB_SIZE, THUMB_SIZE),
        );
        if let Some(tex) = folder.thumbnails.get(i).and_then(|t| t.as_ref()) {
            let fit = fit_rect(thumb_rect, tex.size_vec2());
            ui.painter().image(
                tex.id(),
                fit,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let name = folder.entries[i]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("???");
        let tc = if is_selected {
            egui::Color32::from_gray(20)
        } else {
            egui::Color32::from_gray(60)
        };
        ui.painter().text(
            row_rect.left_center() + egui::vec2(THUMB_SIZE + THUMB_PAD * 2.0, -6.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(12.0),
            tc,
        );

        let row_id = ui.make_persistent_id(format!("frow_{}", i));
        let row_resp = ui.interact(row_rect, row_id, egui::Sense::click());
        if row_resp.clicked() {
            let sel = folder.selected.contains(&i);
            if ctrl {
                if sel {
                    folder.selected.remove(&i);
                } else {
                    folder.selected.insert(i);
                }
            } else if shift {
                let last = folder.selected.iter().max().copied().unwrap_or(i);
                let (lo, hi) = if i < last { (i, last) } else { (last, i) };
                for j in lo..=hi {
                    folder.selected.insert(j);
                }
            } else {
                folder.selected.clear();
                folder.selected.insert(i);
            }
        }
        if row_resp.double_clicked() {
            result = FolderAction::OpenImage(i);
        }

        let entry_c = folder.entries[i].clone();
        let sc = folder.selected.len();
        let ac = menu_action.clone();
        if !ctrl {
            row_resp.context_menu(move |ui| {
                context_menu(ui, i, entry_c, sc, ac);
            });
        }
    }

    if matches!(result, FolderAction::None)
        && let Some(a) = menu_action.borrow_mut().take()
    {
        result = a;
    }
    result
}

fn render_grid(
    ui: &mut egui::Ui,
    folder: &mut FolderCell,
    cell_rect: egui::Rect,
    hover_pos: Option<egui::Pos2>,
    ctrl: bool,
    shift: bool,
    scroll_delta: f32,
) -> FolderAction {
    let avail_w = cell_rect.width() - GRID_PAD;
    let cols = (avail_w / (GRID_CELL + GRID_PAD)).max(1.0) as usize;
    let rows = folder.entries.len().div_ceil(cols);
    let grid_h = rows as f32 * (GRID_CELL + GRID_PAD) + GRID_PAD;
    let max_scroll = (grid_h - cell_rect.height()).max(0.0);
    folder.scroll_offset = (folder.scroll_offset - scroll_delta).clamp(0.0, max_scroll);
    let base_y = cell_rect.top() - folder.scroll_offset + GRID_PAD;
    let start_x = cell_rect.left()
        + (avail_w - (cols as f32 * GRID_CELL + (cols - 1) as f32 * GRID_PAD)) / 2.0
        + GRID_PAD;
    let menu_action = Rc::new(RefCell::new(None));
    let mut result = FolderAction::None;

    for i in 0..folder.entries.len() {
        let col = i % cols;
        let row = i / cols;
        let x = start_x + col as f32 * (GRID_CELL + GRID_PAD);
        let y = base_y + row as f32 * (GRID_CELL + GRID_PAD);
        if y + GRID_CELL < cell_rect.top() || y > cell_rect.bottom() {
            continue;
        }
        let gc = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(GRID_CELL, GRID_CELL));
        let is_hovered = hover_pos.is_some_and(|hp| gc.contains(hp));
        let is_selected = folder.selected.contains(&i);

        let bg = if is_selected {
            egui::Color32::from_rgb(200, 220, 255)
        } else if is_hovered {
            egui::Color32::from_gray(225)
        } else {
            egui::Color32::from_gray(248)
        };
        ui.painter().rect_filled(gc, 0.0, bg);

        let thumb = gc.shrink(4.0);
        if let Some(tex) = folder.thumbnails.get(i).and_then(|t| t.as_ref()) {
            ui.painter().image(
                tex.id(),
                thumb,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let name = folder.entries[i]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("???");
        let dname = if name.len() > 14 {
            format!("{}…", &name[..13])
        } else {
            name.to_string()
        };
        let tc = if is_selected {
            egui::Color32::from_gray(20)
        } else {
            egui::Color32::from_gray(60)
        };
        ui.painter().text(
            gc.center_bottom() + egui::vec2(0.0, 2.0),
            egui::Align2::CENTER_TOP,
            &dname,
            egui::FontId::proportional(10.0),
            tc,
        );

        let row_id = ui.make_persistent_id(format!("fgrid_{}", i));
        let resp = ui.interact(gc, row_id, egui::Sense::click());
        if resp.clicked() {
            let sel = folder.selected.contains(&i);
            if ctrl {
                if sel {
                    folder.selected.remove(&i);
                } else {
                    folder.selected.insert(i);
                }
            } else if shift {
                let last = folder.selected.iter().max().copied().unwrap_or(i);
                let (lo, hi) = if i < last { (i, last) } else { (last, i) };
                for j in lo..=hi {
                    folder.selected.insert(j);
                }
            } else {
                folder.selected.clear();
                folder.selected.insert(i);
            }
        }
        if resp.double_clicked() {
            result = FolderAction::OpenImage(i);
        }

        let ec = folder.entries[i].clone();
        let sc = folder.selected.len();
        let ac = menu_action.clone();
        if !ctrl {
            resp.context_menu(move |ui| {
                context_menu(ui, i, ec, sc, ac);
            });
        }
    }

    if matches!(result, FolderAction::None)
        && let Some(a) = menu_action.borrow_mut().take()
    {
        result = a;
    }
    result
}

fn context_menu(
    ui: &mut egui::Ui,
    i: usize,
    entry: std::path::PathBuf,
    selected_count: usize,
    action: Rc<RefCell<Option<FolderAction>>>,
) {
    if ui.button("Open image").clicked() {
        *action.borrow_mut() = Some(FolderAction::OpenImage(i));
        ui.close();
    }
    if ui.button("Open file location").clicked() {
        *action.borrow_mut() = Some(FolderAction::OpenFolder(entry));
        ui.close();
    }
    if ui.button("Remove from list").clicked() {
        *action.borrow_mut() = Some(FolderAction::Remove(i));
        ui.close();
    }
    if selected_count > 0 {
        ui.separator();
        if ui
            .button(format!("Open selected ({})", selected_count))
            .clicked()
        {
            *action.borrow_mut() = Some(FolderAction::OpenSelected);
            ui.close();
        }
    }
    ui.separator();
    if ui.button("Toggle thumbnail/list view").clicked() {
        *action.borrow_mut() = Some(FolderAction::ToggleView);
        ui.close();
    }
}

fn fit_rect(outer: egui::Rect, img_size: egui::Vec2) -> egui::Rect {
    let scale = (outer.width() / img_size.x).min(outer.height() / img_size.y);
    let w = img_size.x * scale;
    let h = img_size.y * scale;
    egui::Rect::from_min_size(
        egui::pos2(outer.center().x - w / 2.0, outer.center().y - h / 2.0),
        egui::vec2(w, h),
    )
}
