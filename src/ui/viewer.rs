use eframe::egui;

use crate::core;
use crate::state::AppState;

use super::imcell;

const SEP: f32 = 1.0;
const MARGIN: f32 = 6.0;

struct CellSnapshot {
    idx: usize,
    cell_rect: egui::Rect,
    img_size: [usize; 2],
}

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
            ui.label("1-8: Rotate image");
            ui.label("R-click drag: Pan single / Move selection");
            ui.label("Ctrl: Delete / Reorder");
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
    let mut left_dragged = false;
    let mut drag_delta_acc = [0.0f32, 0.0f32];
    let mut snapshots: Vec<CellSnapshot> = Vec::with_capacity(n);

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

        let row_content = col_count as f32 * cell_w + (col_count - 1) as f32 * inter;
        let mut x = grid.left() + (avail.x - row_content) / 2.0;

        for i in 0..col_count {
            let img_idx = offset + i;

            if i > 0 {
                x = paint_zone(ui, x, row_top, row_h, MARGIN, None);
                x = paint_zone(ui, x, row_top, row_h, SEP, Some(sep_color));
                x = paint_zone(ui, x, row_top, row_h, MARGIN, None);
            }

            let cell_rect =
                egui::Rect::from_min_size(egui::pos2(x, row_top), egui::vec2(cell_w, row_h));

            let sense = if ctrl {
                egui::Sense::drag()
            } else if state.local_mode || state.zoom > 1.0 {
                egui::Sense::drag()
            } else {
                egui::Sense::hover()
            };
            let resp = ui.allocate_rect(cell_rect, sense);

            if ctrl {
                let s = 14.0;
                let btn = egui::Rect::from_min_size(
                    cell_rect.right_top() + egui::vec2(-s - 4.0, 4.0),
                    egui::vec2(s, s),
                );
                if ui.allocate_rect(btn, egui::Sense::click()).clicked() {
                    state.pending_remove.push(img_idx);
                }
            }

            // Zoom
            if !state.local_mode && !ctrl && resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    state.zoom = (state.zoom + scroll * 0.005).max(1.0);
                }
            }

            // Pan
            if !state.local_mode && !ctrl {
                if resp.dragged_by(egui::PointerButton::Primary) {
                    let delta = resp.drag_delta();
                    drag_delta_acc[0] += delta.x;
                    drag_delta_acc[1] += delta.y;
                    left_dragged = true;
                }
                if resp.dragged_by(egui::PointerButton::Secondary) {
                    let delta = resp.drag_delta();
                    state.pan_offset[img_idx][0] += delta.x;
                    state.pan_offset[img_idx][1] += delta.y;
                }
                if state.zoom <= 1.0 {
                    state.pan = [0.0, 0.0];
                    state.pan_offset.fill([0.0, 0.0]);
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

            // Q swap
            let compare = state.images.len() == 2 && ui.input(|i| i.key_down(egui::Key::Q));
            let draw_idx = if compare && img_idx == 0 { 1 } else { img_idx };

            let cell_pan = [
                state.pan[0] + state.pan_offset[img_idx][0],
                state.pan[1] + state.pan_offset[img_idx][1],
            ];

            handle_drag(
                state, &resp, draw_idx, cell_rect, state.zoom, cell_pan, ctrl,
            );

            // ── Draw image ────────────────────────────────
            snapshots.push(CellSnapshot {
                idx: img_idx,
                cell_rect,
                img_size: state.images[draw_idx].size,
            });

            imcell::draw_image(ui, &state.images[draw_idx], cell_rect, state.zoom, cell_pan);

            // Ctrl visual overlays
            let reorder_active = ctrl && state.reorder_src.is_some();
            if reorder_active {
                let src = state.reorder_src == Some(img_idx);
                let dst = !src
                    && ui
                        .input(|i| i.pointer.hover_pos())
                        .map_or(false, |hp| cell_rect.contains(hp));
                if src {
                    ui.painter().rect_filled(
                        cell_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 60),
                    );
                } else if dst {
                    ui.painter().rect_filled(
                        cell_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 40),
                    );
                }
            }
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
            }

            let label = state.avg_stats[draw_idx]
                .as_ref()
                .map(|s| core::image::format_cell_label(s))
                .unwrap_or_default();
            imcell::draw_overlay(
                ui,
                cell_rect,
                &state.images[draw_idx],
                state.selection[draw_idx],
                &label,
                if state.show_exif {
                    &state.exif[draw_idx]
                } else {
                    ""
                },
                if state.show_histogram {
                    &state.histogram[draw_idx]
                } else {
                    &[0; 256]
                },
                state.zoom,
                cell_pan,
                state.is_dragging(),
            );

            x += cell_w;
        }

        offset += col_count;
    }

    if left_dragged {
        state.pan[0] += drag_delta_acc[0];
        state.pan[1] += drag_delta_acc[1];
        apply_clamp_feedback(state, &snapshots);
    }

    if !state.pending_remove.is_empty() {
        let mut indices: Vec<usize> = state.pending_remove.drain(..).collect();
        indices.sort_unstable();
        indices.reverse();
        for idx in indices {
            if idx < state.images.len() {
                state.remove_image(idx);
            }
        }
        state.selection.fill(None);
        state.avg_stats.fill(None);
    }
}

fn apply_clamp_feedback(state: &mut AppState, snapshots: &[CellSnapshot]) {
    if state.local_mode || state.zoom <= 1.0 {
        return;
    }

    let mut global_adj = [0.0f32, 0.0f32];
    let mut has_global = false;

    for s in snapshots {
        let raw = [
            state.pan[0] + state.pan_offset[s.idx][0],
            state.pan[1] + state.pan_offset[s.idx][1],
        ];
        let eff = imcell::clamp_pan(raw, s.cell_rect, s.img_size, state.zoom);
        let diff = [raw[0] - eff[0], raw[1] - eff[1]];

        if diff[0] == 0.0 && diff[1] == 0.0 {
            continue;
        }

        for axis in 0..2 {
            let d = diff[axis];
            if d == 0.0 {
                continue;
            }
            let off = state.pan_offset[s.idx][axis];
            if off == 0.0 {
                if d.abs() > global_adj[axis].abs() {
                    global_adj[axis] = d;
                }
                has_global = true;
            } else if off.signum() == d.signum() {
                let new_off = off - d;
                if off.signum() == new_off.signum() || new_off == 0.0 {
                    state.pan_offset[s.idx][axis] = new_off;
                } else {
                    state.pan_offset[s.idx][axis] = 0.0;
                    let rem = d - off;
                    if rem.abs() > global_adj[axis].abs() {
                        global_adj[axis] = rem;
                    }
                    has_global = true;
                }
            } else {
                if d.abs() > global_adj[axis].abs() {
                    global_adj[axis] = d;
                }
                has_global = true;
            }
        }
    }

    if has_global {
        state.pan[0] -= global_adj[0];
        state.pan[1] -= global_adj[1];
    }
}

fn paint_zone(
    ui: &mut egui::Ui,
    x: f32,
    row_top: f32,
    row_h: f32,
    width: f32,
    color: Option<egui::Color32>,
) -> f32 {
    let rect = egui::Rect::from_min_size(egui::pos2(x, row_top), egui::vec2(width, row_h));
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
    cell_rect: egui::Rect,
    zoom: f32,
    pan: [f32; 2],
    ctrl: bool,
) {
    if !state.local_mode || ctrl {
        return;
    }

    let Some(mouse_pos) = resp.interact_pointer_pos() else {
        return;
    };

    let img_size = state.images[img_idx].size;

    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan) {
            state.drag_start_new(img_idx, norm);
        }
    }

    if resp.dragged_by(egui::PointerButton::Primary)
        && state.is_dragging()
        && !state.is_moving_selection()
    {
        if let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan) {
            state.drag_update(norm);
        }
    }

    if resp.drag_started_by(egui::PointerButton::Secondary) {
        if state.selection[img_idx].is_some() {
            if let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan) {
                state.drag_start_move(img_idx, norm);
            }
        }
    }
    if resp.dragged_by(egui::PointerButton::Secondary) && state.is_moving_selection() {
        if let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan) {
            state.drag_update(norm);
        }
    }

    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        if let Some(cell) = state.drag_end() {
            if let Some(sel) = state.selection[cell] {
                for (j, img) in state.images.iter().enumerate() {
                    state.avg_stats[j] = Some(core::image::compute_selection_stats(
                        &img.rgba,
                        img.size[0],
                        img.size[1],
                        &sel,
                    ));
                }
            }
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Secondary) {
        if let Some(cell) = state.drag_end() {
            if let Some(sel) = state.selection[cell] {
                let img = &state.images[cell];
                state.avg_stats[cell] = Some(core::image::compute_selection_stats(
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
