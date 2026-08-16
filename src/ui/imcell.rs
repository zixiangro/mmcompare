//! 单格渲染单元：给定"图片 + 矩形"，把一张图画好。
//!
//! 不关心自己在哪、不关心有几个格子、不碰业务状态——需要改 state 的
//! 操作（旋转、纹理重建）只返回数据，由 imlayout（统筹层）写回。

use eframe::egui;

use crate::core;
use crate::state::{ImageInfo, NormRect};

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
