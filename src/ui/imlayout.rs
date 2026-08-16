//! 统筹层：加载管线、键盘事件、窗口标题、网格布局与交互编排。
//!
//! 本模块是应用主体（`MmCompare`），统筹**所有** imcell：布局怎么排、
//! 交互怎么响应、图片怎么加载，全部在这里编排。`imcell` 只负责
//! "给定一个格子矩形，把单张图片画好"，不关心自己在哪、不关心有几个格子。
//!
//! 这是全项目**唯一**允许出现线程原语的地方（ADR-0001）：
//! 解码线程从这里 spawn，也只在 `poll_loading` 收结果。其余模块
//! （state/core/imcell）永远运行在主线程，不需要考虑线程安全。
//!
//! 加载是一个"批次"状态机，状态分散在四个字段里，必须合起来看：
//! - `load_rx`：有值 = 正在加载（`is_loading()` 据此判断）；
//! - `loading_total` / `loading_received`：本批计划数 / 已收到数，相等即批次完成；
//! - `loading_buf`：按加载顺序占位的缓冲，图片到达后立刻上传纹理填入槽位，
//!   批次完成时按序取走（失败的槽位是 `None`，直接跳过）。
//!
//! 布局采用完全手动坐标（ADR-0002）：只算 cell 位置、画分隔线、编排交互，
//! 图片怎么画委托给 `imcell`。交互状态变更集中在帧末统一应用
//! （`PanFeedback` 模式），避免渲染中途改状态。
//!
//! 完整流程见 docs/loading.md 与 docs/layout.md。

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{AppState, CellKind, ImageInfo, MAX_IMAGES};

use super::imcell;

const SEP: f32 = 1.0;
const MARGIN: f32 = 6.0;

type LoadResult = Result<(core::image::DecodedImage, String, [u32; 256]), PathBuf>;

pub struct MmCompare {
    state: AppState,
    load_rx: Option<mpsc::Receiver<(usize, LoadResult)>>,
    loading_total: usize,
    loading_received: usize,
    loading_buf: Vec<Option<ImageInfo>>,
    pending_drops: Vec<PathBuf>,
}

impl Default for MmCompare {
    fn default() -> Self {
        Self {
            state: AppState::new(),
            load_rx: None,
            loading_total: 0,
            loading_received: 0,
            loading_buf: Vec::new(),
            pending_drops: Vec::new(),
        }
    }
}

fn is_image_ext(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp"
        )
    })
}

fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        na.to_lowercase().cmp(&nb.to_lowercase())
    });
}

impl MmCompare {
    fn is_loading(&self) -> bool {
        self.load_rx.is_some()
    }

    fn filter_paths(&mut self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let remaining = MAX_IMAGES
            .saturating_sub(self.state.cell_order.len())
            .saturating_sub(self.loading_total);
        let mut paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| is_image_ext(p) && !self.state.loaded_paths.contains(p))
            .take(remaining)
            .collect();
        sort_paths(&mut paths);
        for p in &paths {
            self.state.loaded_paths.insert(p.clone());
        }
        paths
    }

    pub fn load_startup_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let paths = self.filter_paths(paths);
        if paths.is_empty() {
            return;
        }
        self.spawn_loaders(paths, ctx);
    }

    fn poll_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let paths = dropped.into_iter().filter_map(|f| f.path).collect();
        let paths = self.filter_paths(paths);
        if paths.is_empty() {
            return;
        }
        if self.is_loading() {
            self.pending_drops.extend(paths);
        } else {
            self.spawn_loaders(paths, ctx);
        }
    }

    fn drain_pending_drops(&mut self, ctx: &egui::Context) {
        if self.is_loading() || self.pending_drops.is_empty() {
            return;
        }
        let remaining = MAX_IMAGES
            .saturating_sub(self.state.cell_order.len())
            .saturating_sub(self.loading_total);
        let mut paths = std::mem::take(&mut self.pending_drops);
        paths.truncate(remaining);
        if !paths.is_empty() {
            self.spawn_loaders(paths, ctx);
        }
    }

    fn spawn_loaders(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        self.loading_total = paths.len();
        self.loading_received = 0;
        self.loading_buf = (0..paths.len()).map(|_| None).collect();
        self.state.load_errors.clear();
        let (tx, rx) = mpsc::channel();

        for (i, p) in paths.into_iter().enumerate() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result: LoadResult = (|| {
                    let bytes = std::fs::read(&p).map_err(|e| {
                        log::warn!("read failed {}: {}", p.display(), e);
                        p.clone()
                    })?;
                    let mut img = core::image::decode_image_bytes(&bytes).ok_or_else(|| {
                        log::warn!("decode failed {}", p.display());
                        p.clone()
                    })?;
                    img.path = p.clone();
                    let exif = core::image::extract_exif(&bytes);
                    let histogram = core::image::compute_y_histogram(&img.rgba);
                    Ok((img, exif, histogram))
                })();
                tx.send((i, result)).ok();
            });
        }
        drop(tx);

        self.load_rx = Some(rx);
        ctx.request_repaint();
    }

    fn poll_loading(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.load_rx else {
            return;
        };

        while let Ok((i, result)) = rx.try_recv() {
            match result {
                Ok((img, exif, histogram)) => {
                    let name = img
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("image");
                    let texture = imcell::upload_texture(ctx, &img.rgba, img.size, name);
                    self.loading_buf[i] = Some(ImageInfo {
                        texture,
                        size: img.size,
                        rgba: img.rgba,
                        path: img.path,
                        exif,
                        histogram,
                    });
                }
                Err(path) => {
                    self.state.loaded_paths.remove(&path);
                    self.state.load_errors.push(path);
                }
            }
            self.loading_received += 1;
        }

        if self.loading_received >= self.loading_total {
            let buf = std::mem::take(&mut self.loading_buf);
            let infos: Vec<ImageInfo> = buf.into_iter().flatten().collect();
            self.state.append_standalone_images(infos);

            self.load_rx = None;
            self.loading_total = 0;
            self.loading_received = 0;
            ctx.request_repaint();
        }
    }

    fn rotate_image_cell(&mut self, img_idx: usize, ctx: &egui::Context) {
        let new_info = {
            let info = &self.state.image_cells[img_idx].info;
            imcell::rotate_image(info, ctx)
        };
        self.state.image_cells[img_idx].info = new_info;
        self.state.invalidate_selection_after_rotation(img_idx);
    }
}

impl eframe::App for MmCompare {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut changed = false;
        let all_images = self.state.is_all_images();

        if all_images {
            let num_keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
            ];
            let (p, e, h, nums) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::P),
                    i.key_pressed(egui::Key::E),
                    i.key_pressed(egui::Key::H),
                    num_keys
                        .iter()
                        .enumerate()
                        .filter(|(_, k)| i.key_pressed(**k))
                        .map(|(idx, _)| idx)
                        .collect::<Vec<usize>>(),
                )
            });

            if p {
                self.state.local_mode = !self.state.local_mode;
                changed = true;
                if !self.state.local_mode {
                    for img in &mut self.state.image_cells {
                        img.selection = None;
                        img.avg_stats = None;
                    }
                }
            }
            if e {
                self.state.show_exif = !self.state.show_exif;
                changed = true;
            }
            if h {
                self.state.show_histogram = !self.state.show_histogram;
                changed = true;
            }
            for idx in nums {
                if idx < self.state.cell_order.len() {
                    let CellKind::Image(img_idx) = self.state.cell_order[idx];
                    self.rotate_image_cell(img_idx, ui.ctx());
                }
            }
        }

        if changed {
            let mut flags = String::new();
            if self.state.show_exif {
                flags.push('E');
            }
            if self.state.show_histogram {
                flags.push('H');
            }
            if self.state.local_mode {
                flags.push('P');
            }
            let title = if flags.is_empty() {
                "MMCompare".to_string()
            } else {
                format!("MMCompare - {}", flags)
            };
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title(title));
        }

        self.poll_drops(ui.ctx());
        self.poll_loading(ui.ctx());
        self.drain_pending_drops(ui.ctx());

        egui::CentralPanel::default().show(ui, |ui| {
            image_grid(
                ui,
                &mut self.state,
                self.loading_total - self.loading_received,
            );
        });
    }
}

struct CellSnapshot {
    pos: usize,
    cell_rect: egui::Rect,
    img_size: [usize; 2],
}

struct GridLayout {
    row_layout: Vec<usize>,
    grid: egui::Rect,
    cell_w: f32,
    row_h: f32,
    inter: f32,
}

struct PanFeedback {
    left_dragged: bool,
    drag_delta_acc: [f32; 2],
    snapshots: Vec<CellSnapshot>,
}

pub fn image_grid(ui: &mut egui::Ui, state: &mut AppState, loading_count: usize) {
    if loading_count > 0 {
        ui.ctx().request_repaint();
    }

    if state.cell_order.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            if loading_count > 0 {
                ui.label(format!("Loading {} image(s)...", loading_count));
            } else {
                ui.label(egui::RichText::new("MMCompare").size(24.0).strong());
                ui.add_space(12.0);
                ui.label(format!("Drag images here to view  (max {})", MAX_IMAGES));
                ui.add_space(6.0);
                ui.label("P: Local mode   E: EXIF   H: Histogram");
                ui.label(format!(
                    "1-{}: Rotate   Q: Compare   Ctrl+RMB: Remove   Ctrl+Drag: Reorder",
                    MAX_IMAGES
                ));
                ui.add_space(12.0);
                ui.hyperlink_to("Project Homepage", "https://github.com/zixiangro/mmcompare");
            }
        });
        return;
    }

    let n = state.cell_order.len();
    let avail = ui.available_size();
    let sep_color = egui::Color32::from_gray(200);

    let row_layout = match n {
        1..=3 => vec![n],
        4 => vec![2, 2],
        _ => vec![n.div_ceil(2), n / 2],
    };

    let max_cols = *row_layout.iter().max().unwrap_or(&1) as f32;
    let rows = row_layout.len() as f32;
    let inter = MARGIN + SEP + MARGIN;
    let row_h = (avail.y - (rows - 1.0) * SEP) / rows;
    let cell_w = (avail.x - (max_cols - 1.0) * inter) / max_cols;

    let total_h = rows * row_h + (rows - 1.0) * SEP;
    let (_, grid_resp) = ui.allocate_exact_size(egui::vec2(avail.x, total_h), egui::Sense::hover());
    let layout = GridLayout {
        row_layout,
        grid: grid_resp.rect,
        cell_w,
        row_h,
        inter,
    };

    let ctrl = ui.input(|i| i.modifiers.ctrl);
    let all_images = state.is_all_images();
    let mut feedback = PanFeedback {
        left_dragged: false,
        drag_delta_acc: [0.0, 0.0],
        snapshots: Vec::with_capacity(n),
    };

    let mut offset = 0;
    for (row_idx, &col_count) in layout.row_layout.iter().enumerate() {
        let row_top = layout.grid.top() + row_idx as f32 * (layout.row_h + SEP);

        if row_idx > 0 {
            let sr = egui::Rect::from_min_size(
                egui::pos2(layout.grid.left(), row_top - SEP),
                egui::vec2(avail.x, SEP),
            );
            ui.painter().rect_filled(sr, 0.0, sep_color);
            ui.allocate_rect(sr, egui::Sense::hover());
        }

        let row_content = col_count as f32 * layout.cell_w + (col_count - 1) as f32 * layout.inter;
        let mut x = layout.grid.left() + (avail.x - row_content) / 2.0;

        for i in 0..col_count {
            let cell_pos = offset + i;

            if i > 0 {
                x = paint_zone(ui, x, row_top, layout.row_h, MARGIN, None);
                x = paint_zone(ui, x, row_top, layout.row_h, SEP, Some(sep_color));
                x = paint_zone(ui, x, row_top, layout.row_h, MARGIN, None);
            }

            let cell_rect = egui::Rect::from_min_size(
                egui::pos2(x, row_top),
                egui::vec2(layout.cell_w, layout.row_h),
            );

            let Some(&CellKind::Image(img_idx)) = state.cell_order.get(cell_pos) else {
                x += layout.cell_w;
                continue;
            };

            render_image_cell(
                ui,
                state,
                cell_rect,
                cell_pos,
                img_idx,
                ctrl,
                all_images,
                &layout,
                &mut feedback,
            );

            x += layout.cell_w;
        }

        offset += col_count;
    }

    if all_images && feedback.left_dragged {
        state.pan[0] += feedback.drag_delta_acc[0];
        state.pan[1] += feedback.drag_delta_acc[1];
        apply_clamp_feedback(state, &feedback.snapshots);
    }

    if !state.pending_remove.is_empty() {
        let mut indices: Vec<usize> = state.pending_remove.drain(..).collect();
        indices.sort_unstable();
        indices.reverse();
        for pos in indices {
            if pos < state.cell_order.len() {
                state.remove_cell(pos);
            }
        }
        if state.cell_order.is_empty() {
            state.local_mode = false;
            state.show_exif = false;
            state.show_histogram = false;
            state.zoom = 1.0;
            state.pan = [0.0, 0.0];
            state.pan_offset.clear();
            for img in &mut state.image_cells {
                img.selection = None;
                img.avg_stats = None;
            }
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title("MMCompare".to_string()));
        }
    }

    draw_status_banner(ui, layout.grid, loading_count, &state.load_errors);
}

fn draw_status_banner(
    ui: &mut egui::Ui,
    grid: egui::Rect,
    loading_count: usize,
    errors: &[PathBuf],
) {
    let mut lines: Vec<String> = Vec::new();
    if loading_count > 0 {
        lines.push(format!("Loading {} image(s)...", loading_count));
    }
    for p in errors {
        lines.push(format!("Failed: {}", p.display()));
    }
    if lines.is_empty() {
        return;
    }

    let font = egui::FontId::monospace(12.0);
    let (w, h) = ui.fonts_mut(|f| {
        let mut w = 0.0f32;
        let mut h = 0.0f32;
        for line in &lines {
            let sz = f
                .layout_no_wrap(line.clone(), font.clone(), egui::Color32::WHITE)
                .size();
            w = w.max(sz.x);
            h += sz.y + 3.0;
        }
        (w, h)
    });
    let pad = 8.0;
    let rect = egui::Rect::from_center_size(
        egui::pos2(grid.center().x, grid.top() + pad + h / 2.0),
        egui::vec2(w + pad * 2.0, h + pad),
    );
    ui.painter()
        .rect_filled(rect, 4.0, egui::Color32::from_black_alpha(180));
    ui.painter().text(
        rect.min + egui::vec2(pad, pad / 2.0),
        egui::Align2::LEFT_TOP,
        lines.join("\n"),
        font,
        egui::Color32::WHITE,
    );
    ui.allocate_rect(rect, egui::Sense::hover());
}

#[allow(clippy::too_many_arguments)]
fn render_image_cell(
    ui: &mut egui::Ui,
    state: &mut AppState,
    cell_rect: egui::Rect,
    cell_pos: usize,
    img_idx: usize,
    ctrl: bool,
    all_images: bool,
    layout: &GridLayout,
    feedback: &mut PanFeedback,
) {
    let sense = if ctrl || state.local_mode || state.zoom > 1.0 {
        egui::Sense::drag()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.allocate_rect(cell_rect, sense);

    if ctrl
        && resp.hovered()
        && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary))
    {
        state.pending_remove.push(cell_pos);
    }

    if !state.local_mode && !ctrl && resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            state.zoom = (state.zoom + scroll * 0.005).max(1.0);
        }
    }

    if !state.local_mode && !ctrl && all_images {
        if resp.dragged_by(egui::PointerButton::Primary) {
            let delta = resp.drag_delta();
            feedback.drag_delta_acc[0] += delta.x;
            feedback.drag_delta_acc[1] += delta.y;
            feedback.left_dragged = true;
        }
        if resp.dragged_by(egui::PointerButton::Secondary) {
            let delta = resp.drag_delta();
            state.pan_offset[cell_pos][0] += delta.x;
            state.pan_offset[cell_pos][1] += delta.y;
        }
        if state.zoom <= 1.0 {
            state.pan = [0.0, 0.0];
            state.pan_offset.fill([0.0, 0.0]);
        }
    }

    if ctrl && all_images {
        if resp.drag_started_by(egui::PointerButton::Primary) {
            state.reorder_src = Some(cell_pos);
        }
        if resp.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(src) = state.reorder_src.take()
            && let Some(dst) = find_cell_at(ui.input(|i| i.pointer.hover_pos()), layout)
            && src != dst
        {
            state.swap_cells(src, dst);
        }
    }

    let compare =
        all_images && state.image_cells.len() == 2 && ui.input(|i| i.key_down(egui::Key::Q));
    let draw_idx = if compare && cell_pos == 0 { 1 } else { img_idx };

    let cell_pan = [
        state.pan[0] + state.pan_offset[cell_pos][0],
        state.pan[1] + state.pan_offset[cell_pos][1],
    ];

    handle_drag(state, &resp, img_idx, cell_rect, state.zoom, cell_pan, ctrl);

    let img_size = state.image_cells[draw_idx].info.size;
    feedback.snapshots.push(CellSnapshot {
        pos: cell_pos,
        cell_rect,
        img_size,
    });

    imcell::draw_image(
        ui,
        &state.image_cells[draw_idx].info,
        cell_rect,
        state.zoom,
        cell_pan,
    );

    let cell = &state.image_cells[draw_idx];
    let label = cell
        .avg_stats
        .as_ref()
        .map(core::image::format_cell_label)
        .unwrap_or_default();
    imcell::draw_overlay(
        ui,
        cell_rect,
        &cell.info,
        cell.selection,
        &label,
        if state.show_exif { &cell.info.exif } else { "" },
        if state.show_histogram {
            &cell.info.histogram
        } else {
            &[0; 256]
        },
        state.zoom,
        cell_pan,
        state.is_dragging(),
    );

    if ctrl && state.reorder_src.is_some() {
        let src = state.reorder_src == Some(cell_pos);
        let dst = !src
            && ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|hp| cell_rect.contains(hp));
        if src || dst {
            ui.painter().rect_filled(
                cell_rect,
                0.0,
                if src {
                    egui::Color32::from_rgba_premultiplied(0, 0, 0, 60)
                } else {
                    egui::Color32::from_rgba_premultiplied(0, 0, 0, 40)
                },
            );
        }
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

fn apply_clamp_feedback(state: &mut AppState, snapshots: &[CellSnapshot]) {
    if state.local_mode || state.zoom <= 1.0 {
        return;
    }
    let mut global_adj = [0.0f32, 0.0f32];
    let mut has_global = false;

    for s in snapshots {
        let raw = [
            state.pan[0] + state.pan_offset[s.pos][0],
            state.pan[1] + state.pan_offset[s.pos][1],
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
            let off = state.pan_offset[s.pos][axis];
            if off == 0.0 {
                if d.abs() > global_adj[axis].abs() {
                    global_adj[axis] = d;
                }
                has_global = true;
            } else if off.signum() == d.signum() {
                let new_off = off - d;
                if off.signum() == new_off.signum() || new_off == 0.0 {
                    state.pan_offset[s.pos][axis] = new_off;
                } else {
                    state.pan_offset[s.pos][axis] = 0.0;
                    let rem = d - off;
                    if rem.abs() > global_adj[axis].abs() {
                        global_adj[axis] = rem;
                    }
                    has_global = true;
                }
            } else if d.abs() > global_adj[axis].abs() {
                global_adj[axis] = d;
                has_global = true;
            }
        }
    }
    if has_global {
        state.pan[0] -= global_adj[0];
        state.pan[1] -= global_adj[1];
    }
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

    let img_size = state.image_cells[img_idx].info.size;

    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_start_new(img_idx, norm);
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && state.is_dragging()
        && !state.is_moving_selection()
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_update(norm);
    }
    if resp.drag_started_by(egui::PointerButton::Secondary)
        && state.image_cells[img_idx].selection.is_some()
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_start_move(img_idx, norm);
    }
    if resp.dragged_by(egui::PointerButton::Secondary)
        && state.is_moving_selection()
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_update(norm);
    }

    if resp.drag_stopped_by(egui::PointerButton::Primary)
        && let Some(cell) = state.drag_end()
        && let Some(sel) = state.image_cells[cell].selection
    {
        for img in &mut state.image_cells {
            img.avg_stats = Some(core::image::compute_selection_stats(
                &img.info.rgba,
                img.info.size[0],
                img.info.size[1],
                &sel,
            ));
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Secondary)
        && let Some(cell) = state.drag_end()
        && let Some(sel) = state.image_cells[cell].selection
    {
        let stats = core::image::compute_selection_stats(
            &state.image_cells[cell].info.rgba,
            state.image_cells[cell].info.size[0],
            state.image_cells[cell].info.size[1],
            &sel,
        );
        state.image_cells[cell].avg_stats = Some(stats);
    }
}

fn find_cell_at(pos: Option<egui::Pos2>, layout: &GridLayout) -> Option<usize> {
    let pos = pos?;
    let mut idx = 0usize;
    for (row_idx, &col_count) in layout.row_layout.iter().enumerate() {
        let row_top = layout.grid.top() + row_idx as f32 * (layout.row_h + SEP);
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(layout.grid.left(), row_top),
            egui::vec2(layout.grid.width(), layout.row_h),
        );
        if !row_rect.contains(pos) {
            idx += col_count;
            continue;
        }
        let row_content = col_count as f32 * layout.cell_w + (col_count - 1) as f32 * layout.inter;
        let mut x = row_rect.left() + (layout.grid.width() - row_content) / 2.0;
        for _ in 0..col_count {
            let cr = egui::Rect::from_min_size(
                egui::pos2(x, row_top),
                egui::vec2(layout.cell_w, layout.row_h),
            );
            if cr.contains(pos) {
                return Some(idx);
            }
            x += layout.cell_w + layout.inter;
            idx += 1;
        }
    }
    None
}
