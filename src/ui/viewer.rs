use eframe::egui;

use crate::state::ImageInfo;

/// Separator line thickness.
const SEP: f32 = 1.0;
/// Margin on each side of vertical separators, and from window edges.
const MARGIN: f32 = 6.0;

/// Layout 1-8 images in a grid.
/// - 1-4 images: single row, equal width per image.
/// - 5-8 images: two rows; first row gets ceil(n/2), second gets floor(n/2).
/// All cells have the same area.
pub fn image_grid(ui: &mut egui::Ui, images: &[ImageInfo], loading_count: usize) {
    if loading_count > 0 {
        ui.centered_and_justified(|ui| {
            ui.label(format!("Loading {} image(s)...", loading_count));
        });
        ui.ctx().request_repaint();
        return;
    }

    if images.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("No images loaded. File \u{2192} Open to load images (max 8).");
        });
        return;
    }

    let n = images.len();
    let available = ui.available_size();
    let sep_color = egui::Color32::from_gray(200);

    let row_layout: Vec<usize> = if n <= 4 {
        vec![n]
    } else {
        vec![(n + 1) / 2, n / 2]
    };

    let max_cols = *row_layout.iter().max().unwrap_or(&1) as f32;
    let rows = row_layout.len() as f32;
    let inter_row = SEP;
    let row_height = (available.y - (rows - 1.0) * inter_row) / rows;
    let cell_width = (available.x - (max_cols - 1.0) * (MARGIN + SEP + MARGIN)) / max_cols;

    let mut offset = 0;
    for (row_idx, &col_count) in row_layout.iter().enumerate() {
        // Inter-row gap + separator (skip for first row)
        if row_idx > 0 {
            // Horizontal separator between rows (no margin)
            let (_, sr) =
                ui.allocate_exact_size(egui::vec2(available.x, SEP), egui::Sense::hover());
            ui.painter().rect_filled(sr.rect, 0.0, sep_color);
        }

        // Allocate the row
        let (_, row_resp) =
            ui.allocate_exact_size(egui::vec2(available.x, row_height), egui::Sense::hover());
        let row_rect = row_resp.rect;

        // Center cells + separators within the row
        let inter_cell = MARGIN + SEP + MARGIN; // 13px between cells
        let row_content = col_count as f32 * cell_width + (col_count - 1) as f32 * inter_cell;
        let mut x = row_rect.left() + (available.x - row_content) / 2.0;

        let row_images = &images[offset..offset + col_count];
        for (i, img) in row_images.iter().enumerate() {
            if i > 0 {
                // Margin left of separator
                let mr = egui::Rect::from_min_size(
                    egui::pos2(x, row_rect.top()),
                    egui::vec2(MARGIN, row_height),
                );
                ui.allocate_rect(mr, egui::Sense::hover());
                x += MARGIN;

                // Separator
                let sr = egui::Rect::from_min_size(
                    egui::pos2(x, row_rect.top()),
                    egui::vec2(SEP, row_height),
                );
                ui.painter().rect_filled(sr, 0.0, sep_color);
                ui.allocate_rect(sr, egui::Sense::hover());
                x += SEP;

                // Margin right of separator
                let mr = egui::Rect::from_min_size(
                    egui::pos2(x, row_rect.top()),
                    egui::vec2(MARGIN, row_height),
                );
                ui.allocate_rect(mr, egui::Sense::hover());
                x += MARGIN;
            }

            // Cell
            let cr = egui::Rect::from_min_size(
                egui::pos2(x, row_rect.top()),
                egui::vec2(cell_width, row_height),
            );
            draw_cell(ui, img, cr);
            ui.allocate_rect(cr, egui::Sense::hover());
            x += cell_width;
        }

        offset += col_count;
    }
}

fn draw_cell(ui: &mut egui::Ui, img: &ImageInfo, cell_rect: egui::Rect) {
    let img_w = img.size[0] as f32;
    let img_h = img.size[1] as f32;
    let scale = (cell_rect.width() / img_w).min(cell_rect.height() / img_h);
    let display = egui::vec2(img_w * scale, img_h * scale);

    let offset = (cell_rect.size() - display) / 2.0;
    let img_rect = egui::Rect::from_min_size(cell_rect.min + offset, display);

    ui.put(
        img_rect,
        egui::Image::from_texture(egui::load::SizedTexture::new(img.texture.id(), display)),
    );
}
