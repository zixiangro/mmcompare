use eframe::egui;

use crate::state::AppState;

use super::cell;

/// Separator line thickness.
const SEP: f32 = 1.0;
/// Margin on each side of vertical separators, and from window edges.
const MARGIN: f32 = 6.0;

/// Layout N images in a grid. Handles drag-to-select in local mode.
pub fn image_grid(ui: &mut egui::Ui, state: &mut AppState, loading_count: usize) {
    if loading_count > 0 {
        ui.centered_and_justified(|ui| {
            ui.label(format!("Loading {} image(s)...", loading_count));
        });
        ui.ctx().request_repaint();
        return;
    }

    if state.images.is_empty() {
        let hint = if state.local_mode {
            "No images loaded. Press P to exit local mode."
        } else {
            "No images loaded. File \u{2192} Open or drag images here."
        };
        ui.centered_and_justified(|ui| {
            ui.label(hint);
        });
        return;
    }

    let n = state.images.len();
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
            let (_, sr) =
                ui.allocate_exact_size(egui::vec2(available.x, SEP), egui::Sense::hover());
            ui.painter().rect_filled(sr.rect, 0.0, sep_color);
        }

        let (_, row_resp) =
            ui.allocate_exact_size(egui::vec2(available.x, row_height), egui::Sense::hover());
        let row_rect = row_resp.rect;

        let inter_cell = MARGIN + SEP + MARGIN;
        let row_content = col_count as f32 * cell_width + (col_count - 1) as f32 * inter_cell;
        let mut x = row_rect.left() + (available.x - row_content) / 2.0;

        for i in 0..col_count {
            let img_idx = offset + i;

            if i > 0 {
                x = draw_vertical_separator(ui, x, row_rect, sep_color);
            }

            let cell_rect = egui::Rect::from_min_size(
                egui::pos2(x, row_rect.top()),
                egui::vec2(cell_width, row_height),
            );

            // Sense depends on mode: draggable in local mode, hover otherwise
            let sense = if state.local_mode {
                egui::Sense::drag()
            } else {
                egui::Sense::hover()
            };
            let resp = ui.allocate_rect(cell_rect, sense);

            // Handle drag interaction
            if state.local_mode && !resp.dragged_by(egui::PointerButton::Secondary) {
                if let Some(mouse_pos) = resp.interact_pointer_pos() {
                    if resp.drag_started_by(egui::PointerButton::Primary) {
                        if let Some(norm) =
                            cell::mouse_to_norm(mouse_pos, cell_rect, state.images[img_idx].size)
                        {
                            state.drag_start(norm);
                        }
                    }

                    if resp.dragged_by(egui::PointerButton::Primary) && state.is_dragging() {
                        if let Some(norm) =
                            cell::mouse_to_norm(mouse_pos, cell_rect, state.images[img_idx].size)
                        {
                            state.drag_update(norm);
                        }
                    }
                }

                if resp.drag_stopped_by(egui::PointerButton::Primary) {
                    if let Some(sel) = state.drag_end() {
                        // Compute avg Y for all images
                        for (j, img) in state.images.iter().enumerate() {
                            state.avg_y[j] = Some(cell::compute_avg_y(img, sel));
                        }
                    }
                }
            }

            // Draw image + overlay
            cell::draw_image(ui, &state.images[img_idx], cell_rect);
            cell::draw_overlay(
                ui,
                cell_rect,
                &state.images[img_idx],
                state.selection,
                state.avg_y[img_idx],
                state.is_dragging(),
            );

            x += cell_width;
        }

        offset += col_count;
    }
}

fn draw_vertical_separator(
    ui: &mut egui::Ui,
    x: f32,
    row_rect: egui::Rect,
    color: egui::Color32,
) -> f32 {
    let mut x = x;

    let mr = egui::Rect::from_min_size(
        egui::pos2(x, row_rect.top()),
        egui::vec2(MARGIN, row_rect.height()),
    );
    ui.allocate_rect(mr, egui::Sense::hover());
    x += MARGIN;

    let sr = egui::Rect::from_min_size(
        egui::pos2(x, row_rect.top()),
        egui::vec2(SEP, row_rect.height()),
    );
    ui.painter().rect_filled(sr, 0.0, color);
    ui.allocate_rect(sr, egui::Sense::hover());
    x += SEP;

    let mr = egui::Rect::from_min_size(
        egui::pos2(x, row_rect.top()),
        egui::vec2(MARGIN, row_rect.height()),
    );
    ui.allocate_rect(mr, egui::Sense::hover());
    x += MARGIN;

    x
}
