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
            ui.add_space(6.0);
            ui.label("Q: Compare 2 images");
            ui.label("P: Local mode");
            ui.label("E: Show EXIF");
            ui.label("H: Show histogram");
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

    let total_h = rows * row_h + (rows - 1.0) * SEP;
    let (_, grid_resp) = ui.allocate_exact_size(egui::vec2(avail.x, total_h), egui::Sense::hover());
    let grid = grid_resp.rect;

    let ctrl = ui.input(|i| i.modifiers.ctrl);

    let mut offset = 0;
    for (row_idx, &col_count) in row_layout.iter().enumerate() {
        let row_top = grid.top() + row_idx as f32 * (row_h + SEP);

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

            let sense = if ctrl {
                egui::Sense::drag()
            } else if state.local_mode || state.zoom > 1.0 {
                egui::Sense::drag()
            } else {
                egui::Sense::hover()
            };
            let resp = ui.allocate_rect(cell_rect, sense);

            // Ctrl close button (after cell alloc)
            if ctrl {
                let s = 14.0;
                let btn = egui::Rect::from_min_size(
                    cell_rect.right_top() + egui::vec2(-s - 4.0, 4.0),
                    egui::vec2(s, s),
                );
                ui.painter().rect_filled(
                    btn,
                    2.0,
                    egui::Color32::from_rgba_premultiplied(200, 50, 50, 200),
                );
                let w = egui::Color32::WHITE;
                ui.painter().line_segment(
                    [
                        btn.left_top() + egui::vec2(3.0, 3.0),
                        btn.right_bottom() - egui::vec2(3.0, 3.0),
                    ],
                    egui::Stroke::new(2.0, w),
                );
                ui.painter().line_segment(
                    [
                        btn.right_top() + egui::vec2(-3.0, 3.0),
                        btn.left_bottom() + egui::vec2(3.0, -3.0),
                    ],
                    egui::Stroke::new(2.0, w),
                );
                if ui.allocate_rect(btn, egui::Sense::click()).clicked() {
                    state.pending_remove.push(img_idx);
                }
            }

            // Zoom/pan (non-local, non-ctrl)
            if !state.local_mode && !ctrl {
                if resp.hovered() {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.0 {
                        state.zoom = (state.zoom + scroll * 0.005).max(1.0);
                    }
                }
                if resp.dragged_by(egui::PointerButton::Primary) {
                    let delta = resp.drag_delta();
                    state.pan[0] += delta.x;
                    state.pan[1] += delta.y;
                }
                if state.zoom <= 1.0 {
                    state.pan = [0.0, 0.0];
                }
            }

            // Ctrl reorder
            if ctrl {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    state.reorder_src = Some(img_idx);
                }
                if resp.drag_stopped_by(egui::PointerButton::Primary) {
                    if let Some(src) = state.reorder_src.take() {
                        let hover_pos = ui.input(|i| i.pointer.hover_pos());
                        if let Some(dst) =
                            find_cell_at(hover_pos, &row_layout, grid, cell_w, row_h, inter)
                        {
                            if src != dst {
                                state.swap_images(src, dst);
                            }
                        }
                    }
                }
            }

            // Reorder highlight (check global hover during drag)
            if ctrl && state.reorder_src.is_some() {
                if state.reorder_src == Some(img_idx) {
                    ui.painter().rect_filled(
                        cell_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(0, 120, 255, 60),
                    );
                } else if let Some(hp) = ui.input(|i| i.pointer.hover_pos()) {
                    if cell_rect.contains(hp) {
                        ui.painter().rect_filled(
                            cell_rect,
                            0.0,
                            egui::Color32::from_rgba_premultiplied(255, 200, 0, 40),
                        );
                    }
                }
            }

            // Q-key swap
            let compare = state.images.len() == 2 && ui.input(|i| i.key_down(egui::Key::Q));
            let draw_idx = if compare && img_idx == 0 { 1 } else { img_idx };

            handle_drag(state, &resp, img_idx, state.zoom, state.pan);
            cell::draw_image(
                ui,
                &state.images[draw_idx],
                cell_rect,
                state.zoom,
                state.pan,
            );
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
                state.zoom,
                state.pan,
                state.is_dragging(),
            );

            x += cell_w;
        }

        offset += col_count;
    }

    // Process removals
    if !state.pending_remove.is_empty() {
        let mut indices: Vec<usize> = state.pending_remove.drain(..).collect();
        indices.sort_unstable();
        indices.reverse();
        for idx in indices {
            if idx < state.images.len() {
                state.remove_image(idx);
            }
        }
        state.selection = None;
        state.avg_y.fill(None);
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

fn handle_drag(
    state: &mut AppState,
    resp: &egui::Response,
    img_idx: usize,
    zoom: f32,
    pan: [f32; 2],
) {
    if !state.local_mode || resp.dragged_by(egui::PointerButton::Secondary) {
        return;
    }

    if let Some(mouse_pos) = resp.interact_pointer_pos() {
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(norm) =
                cell::mouse_to_norm(mouse_pos, resp.rect, state.images[img_idx].size, zoom, pan)
            {
                state.drag_start(norm);
            }
        }

        if resp.dragged_by(egui::PointerButton::Primary) && state.is_dragging() {
            if let Some(norm) =
                cell::mouse_to_norm(mouse_pos, resp.rect, state.images[img_idx].size, zoom, pan)
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

fn find_cell_at(
    pos: Option<egui::Pos2>,
    row_layout: &[usize],
    grid: egui::Rect,
    cell_w: f32,
    row_h: f32,
    inter: f32,
) -> Option<usize> {
    let pos = pos?;
    let mut idx = 0usize;
    for (row_idx, &col_count) in row_layout.iter().enumerate() {
        let row_top = grid.top() + row_idx as f32 * (row_h + SEP);
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(grid.left(), row_top),
            egui::vec2(grid.width(), row_h),
        );
        if !row_rect.contains(pos) {
            idx += col_count;
            continue;
        }
        let row_content = col_count as f32 * cell_w + (col_count - 1) as f32 * inter;
        let mut x = row_rect.left() + (grid.width() - row_content) / 2.0;
        for _ in 0..col_count {
            let cell_rect =
                egui::Rect::from_min_size(egui::pos2(x, row_top), egui::vec2(cell_w, row_h));
            if cell_rect.contains(pos) {
                return Some(idx);
            }
            x += cell_w + inter;
            idx += 1;
        }
    }
    None
}
