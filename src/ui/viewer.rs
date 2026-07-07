use eframe::egui;

use crate::state::ImageInfo;

use super::cell;

/// Separator line thickness.
const SEP: f32 = 1.0;
/// Margin on each side of vertical separators, and from window edges.
const MARGIN: f32 = 6.0;

/// Layout N images in a grid.
///
/// This function is purely a layout coordinator:
/// - Decides row/column counts based on image count
/// - Computes uniform cell sizes across all rows
/// - Positions separators and margins
/// - Delegates each cell's rendering to `cell::draw_image`
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
    let row_height = (available.y - (rows - 1.0) * SEP) / rows;
    let cell_width = (available.x - (max_cols - 1.0) * (MARGIN + SEP + MARGIN)) / max_cols;

    let mut offset = 0;
    for (row_idx, &col_count) in row_layout.iter().enumerate() {
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

        // Center cells within the row
        let inter_cell = MARGIN + SEP + MARGIN;
        let row_content = col_count as f32 * cell_width + (col_count - 1) as f32 * inter_cell;
        let mut x = row_rect.left() + (available.x - row_content) / 2.0;

        let row_images = &images[offset..offset + col_count];
        for (i, img) in row_images.iter().enumerate() {
            if i > 0 {
                x = draw_vertical_separator(ui, x, row_rect, sep_color);
            }

            let cell_rect = egui::Rect::from_min_size(
                egui::pos2(x, row_rect.top()),
                egui::vec2(cell_width, row_height),
            );
            cell::draw_image(ui, img, cell_rect);
            ui.allocate_rect(cell_rect, egui::Sense::hover());
            x += cell_width;
        }

        offset += col_count;
    }
}

/// Draw a vertical separator with margin on both sides. Returns the new x position after the separator.
fn draw_vertical_separator(
    ui: &mut egui::Ui,
    x: f32,
    row_rect: egui::Rect,
    color: egui::Color32,
) -> f32 {
    let mut x = x;

    // Margin left
    let mr = egui::Rect::from_min_size(
        egui::pos2(x, row_rect.top()),
        egui::vec2(MARGIN, row_rect.height()),
    );
    ui.allocate_rect(mr, egui::Sense::hover());
    x += MARGIN;

    // Separator line
    let sr = egui::Rect::from_min_size(
        egui::pos2(x, row_rect.top()),
        egui::vec2(SEP, row_rect.height()),
    );
    ui.painter().rect_filled(sr, 0.0, color);
    ui.allocate_rect(sr, egui::Sense::hover());
    x += SEP;

    // Margin right
    let mr = egui::Rect::from_min_size(
        egui::pos2(x, row_rect.top()),
        egui::vec2(MARGIN, row_rect.height()),
    );
    ui.allocate_rect(mr, egui::Sense::hover());
    x += MARGIN;

    x
}
