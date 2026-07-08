use eframe::egui;

use crate::state::{ImageInfo, NormRect};

pub fn draw_image(ui: &mut egui::Ui, img: &ImageInfo, rect: egui::Rect) {
    let img_rect = image_display_rect(rect, img.size);
    ui.put(
        img_rect,
        egui::Image::from_texture(egui::load::SizedTexture::new(
            img.texture.id(),
            img_rect.size(),
        )),
    );
}

pub fn image_display_rect(cell_rect: egui::Rect, img_size: [usize; 2]) -> egui::Rect {
    let img_w = img_size[0] as f32;
    let img_h = img_size[1] as f32;
    let scale = (cell_rect.width() / img_w).min(cell_rect.height() / img_h);
    let offset = (
        (cell_rect.width() - img_w * scale) / 2.0,
        (cell_rect.height() - img_h * scale) / 2.0,
    );
    egui::Rect::from_min_size(
        cell_rect.min + egui::vec2(offset.0, offset.1),
        egui::vec2(img_w * scale, img_h * scale),
    )
}

pub fn mouse_to_norm(
    mouse_pos: egui::Pos2,
    cell_rect: egui::Rect,
    img_size: [usize; 2],
) -> Option<[f32; 2]> {
    let img_rect = image_display_rect(cell_rect, img_size);
    if !img_rect.contains(mouse_pos) {
        return None;
    }
    Some([
        (mouse_pos.x - img_rect.min.x) / img_rect.width(),
        (mouse_pos.y - img_rect.min.y) / img_rect.height(),
    ])
}

pub fn draw_overlay(
    ui: &mut egui::Ui,
    cell_rect: egui::Rect,
    img: &ImageInfo,
    selection: Option<NormRect>,
    avg_y_label: &str,
    exif: &str,
    histogram: &[u32; 256],
    is_dragging: bool,
) {
    let img_rect = image_display_rect(cell_rect, img.size);

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
        ui.painter().rect_stroke(
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
