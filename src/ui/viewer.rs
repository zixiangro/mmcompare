use eframe::egui;

use crate::core;
use crate::state::AppState;

use super::cell;

const SEP: f32 = 1.0;
const MARGIN: f32 = 6.0;

pub fn image_grid(ui: &mut egui::Ui, state: &mut AppState, loading_count: usize) {
    if loading_count > 0 {
        ui.vertical_centered(|ui| {
            ui.label(format!("Loading {} image(s)...", loading_count));
        });
        ui.ctx().request_repaint();
        return;
    }

    if state.images.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.label(egui::RichText::new("MMCompare").size(24.0).strong());
            ui.add_space(12.0);
            ui.label("Drag images here to view  (max 8)");
            ui.add_space(4.0);
            ui.label("E  EXIF    H  Histogram    L  Local mode");
            ui.add_space(12.0);
            ui.hyperlink_to("Project Homepage", "https://github.com/zixiangro/mmcompare");
        });
        return;
    }

    let n = state.images.len();
    let avail = ui.available_size();
    let sep_color = egui::Color32::from_gray(200);

    let row_layout = match n {
        1..=3 => vec![n],
        4 => vec![2, 2],
        _ => vec![(n + 1) / 2, n / 2],
    };

    let max_cols = *row_layout.iter().max().unwrap_or(&1) as f32;
    let rows = row_layout.len() as f32;
    let inter = MARGIN + SEP + MARGIN;
    let row_h = (avail.y - (rows - 1.0) * SEP) / rows;
    let cell_w = (avail.x - (max_cols - 1.0) * inter) / max_cols;

    // Allocate the entire grid area at once
    let total_h = rows * row_h + (rows - 1.0) * SEP;
    let (_, grid_resp) = ui.allocate_exact_size(egui::vec2(avail.x, total_h), egui::Sense::hover());
    let grid = grid_resp.rect;

    let mut offset = 0;
    for (row_idx, &col_count) in row_layout.iter().enumerate() {
        let row_top = grid.top() + row_idx as f32 * (row_h + SEP);

        // Draw horizontal separator above this row
        if row_idx > 0 {
            let sr = egui::Rect::from_min_size(
                egui::pos2(grid.left(), row_top - SEP),
                egui::vec2(avail.x, SEP),
            );
            ui.painter().rect_filled(sr, 0.0, sep_color);
            ui.allocate_rect(sr, egui::Sense::hover());
        }

        let row_rect =
            egui::Rect::from_min_size(egui::pos2(grid.left(), row_top), egui::vec2(avail.x, row_h));

        // Center cells within the row
        let row_content = col_count as f32 * cell_w + (col_count - 1) as f32 * inter;
        let mut x = row_rect.left() + (avail.x - row_content) / 2.0;

        for i in 0..col_count {
            let img_idx = offset + i;

            if i > 0 {
                x = paint_zone(ui, x, row_rect, MARGIN, None);
                x = paint_zone(ui, x, row_rect, SEP, Some(sep_color));
                x = paint_zone(ui, x, row_rect, MARGIN, None);
            }

            let cell_rect =
                egui::Rect::from_min_size(egui::pos2(x, row_rect.top()), egui::vec2(cell_w, row_h));
            let sense = if state.local_mode {
                egui::Sense::drag()
            } else {
                egui::Sense::hover()
            };
            let resp = ui.interact(cell_rect, ui.next_auto_id(), sense);

            handle_drag(state, &resp, img_idx);
            cell::draw_image(ui, &state.images[img_idx], cell_rect);
            let label = state.avg_y[img_idx]
                .map(core::image::format_cell_label)
                .unwrap_or_default();
            cell::draw_overlay(
                ui,
                cell_rect,
                &state.images[img_idx],
                state.selection,
                &label,
                if state.show_exif {
                    &state.exif[img_idx]
                } else {
                    ""
                },
                if state.show_histogram {
                    &state.histogram[img_idx]
                } else {
                    &[0; 256]
                },
                state.is_dragging(),
            );

            x += cell_w;
        }

        offset += col_count;
    }
}

fn paint_zone(
    ui: &mut egui::Ui,
    x: f32,
    row_rect: egui::Rect,
    width: f32,
    color: Option<egui::Color32>,
) -> f32 {
    let rect = egui::Rect::from_min_size(
        egui::pos2(x, row_rect.top()),
        egui::vec2(width, row_rect.height()),
    );
    if let Some(c) = color {
        ui.painter().rect_filled(rect, 0.0, c);
    }
    ui.allocate_rect(rect, egui::Sense::hover());
    x + width
}

fn handle_drag(state: &mut AppState, resp: &egui::Response, img_idx: usize) {
    if !state.local_mode || resp.dragged_by(egui::PointerButton::Secondary) {
        return;
    }

    if let Some(mouse_pos) = resp.interact_pointer_pos() {
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(norm) =
                cell::mouse_to_norm(mouse_pos, resp.rect, state.images[img_idx].size)
            {
                state.drag_start(norm);
            }
        }

        if resp.dragged_by(egui::PointerButton::Primary) && state.is_dragging() {
            if let Some(norm) =
                cell::mouse_to_norm(mouse_pos, resp.rect, state.images[img_idx].size)
            {
                state.drag_update(norm);
            }
        }
    }

    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        if let Some(sel) = state.drag_end() {
            for (j, img) in state.images.iter().enumerate() {
                state.avg_y[j] = Some(core::image::compute_avg_y(
                    &img.rgba,
                    img.size[0],
                    img.size[1],
                    &sel,
                ));
            }
        }
    }
}
