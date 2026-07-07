use eframe::egui;

use crate::core;
use crate::state::{ImageInfo, NormRect};

/// Draw a single image scaled to fit and centered within the given rect.
pub fn draw_image(ui: &mut egui::Ui, img: &ImageInfo, rect: egui::Rect) {
    let scale = (rect.width() / img.size[0] as f32).min(rect.height() / img.size[1] as f32);
    let display_w = img.size[0] as f32 * scale;
    let display_h = img.size[1] as f32 * scale;
    let offset = (
        (rect.width() - display_w) / 2.0,
        (rect.height() - display_h) / 2.0,
    );
    let img_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(offset.0, offset.1),
        egui::vec2(display_w, display_h),
    );

    ui.put(
        img_rect,
        egui::Image::from_texture(egui::load::SizedTexture::new(
            img.texture.id(),
            egui::vec2(display_w, display_h),
        )),
    );
}

/// Returns the rect where the image is actually drawn within the cell.
pub fn image_display_rect(cell_rect: egui::Rect, img_size: [usize; 2]) -> egui::Rect {
    let img_w = img_size[0] as f32;
    let img_h = img_size[1] as f32;
    let scale = (cell_rect.width() / img_w).min(cell_rect.height() / img_h);
    let display_w = img_w * scale;
    let display_h = img_h * scale;
    let offset = (
        (cell_rect.width() - display_w) / 2.0,
        (cell_rect.height() - display_h) / 2.0,
    );
    egui::Rect::from_min_size(
        cell_rect.min + egui::vec2(offset.0, offset.1),
        egui::vec2(display_w, display_h),
    )
}

/// Convert a mouse position within `cell_rect` to normalized image coords [0..1].
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

/// Draw selection overlay and avg Y label.
pub fn draw_overlay(
    ui: &mut egui::Ui,
    cell_rect: egui::Rect,
    img: &ImageInfo,
    selection: Option<NormRect>,
    avg_y: Option<f32>,
    is_dragging: bool,
) {
    let img_rect = image_display_rect(cell_rect, img.size);

    // Selection rectangle
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

    // Stats label anchored to cell bottom-left, pixel-precise.
    if let Some(y) = avg_y {
        let text = core::image::format_cell_label(y);
        let font = egui::FontId::monospace(11.0);
        let anchor = cell_rect.left_bottom();

        for (i, line) in text.lines().rev().enumerate() {
            let pos = anchor - egui::vec2(0.0, i as f32 * 14.0);
            ui.painter().text(
                pos,
                egui::Align2::LEFT_BOTTOM,
                line,
                font.clone(),
                ui.visuals().text_color(),
            );
        }
    }
}
