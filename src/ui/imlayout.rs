//! 统筹层：加载管线、键盘事件、窗口标题、网格布局与交互编排。
//!
//! 本模块是应用主体（`MmCompare`），管理**所有** cell 的编排：布局怎么排、
//! 交互怎么响应、图片怎么加载，都在这里；文件夹 cell 的扫描/缩略图/
//! 打开/导航与渲染收在 `ui/folder`（`FolderManager`），imlayout 只做
//! 编排调用与键盘分发。`imcell` 负责图片 cell 的绘制。
//!
//! 这是全项目**仅有的两个**允许出现线程原语的地方之一（ADR-0001）：
//! 本模块的解码线程（standalone 图片）与 `folder.rs` 的扫描/加载线程，
//! 其余模块永远运行在主线程，不需要考虑线程安全。
//!
//! 加载是一个"批次"状态机：`load_rx` 有值 = 正在加载；
//! `loading_total` / `loading_received` 相等即批次完成；
//! `loading_buf` 按加载顺序占位，批次完成时按序追加到 state。
//! 加载期间收到的拖拽缓存到 `pending_drops`，完成后按序处理。
//!
//! 布局采用完全手动坐标（ADR-0002）。交互状态变更集中在帧末统一应用
//! （`PanFeedback` 模式），避免渲染中途改状态。
//!
//! 完整流程见 docs/loading.md 与 docs/layout.md。

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{AppState, CellKind, FolderAction, ImageInfo, ImageSource, MAX_IMAGES};

use super::{folder, imcell};

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
    folder: folder::FolderManager,
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
            folder: folder::FolderManager::default(),
        }
    }
}

impl MmCompare {
    fn is_busy(&self) -> bool {
        self.load_rx.is_some() || self.folder.is_busy()
    }

    /// 把输入路径分类为图片文件与文件夹（去重、排序、标记、截断名额），
    /// 文件路径立即标记进 `loaded_paths`，避免同批重复入队。
    fn classify_paths(&mut self, paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let remaining = MAX_IMAGES
            .saturating_sub(self.state.cell_order.len())
            .saturating_sub(self.loading_total);
        let mut file_paths = Vec::new();
        let mut folder_paths = Vec::new();
        for p in paths.into_iter().take(remaining) {
            if p.is_dir() {
                if !self.state.folder_cells.iter().any(|fc| fc.dir_path == p) {
                    folder_paths.push(p);
                }
            } else if folder::is_image_ext(&p) && !self.state.loaded_paths.contains(&p) {
                file_paths.push(p);
            }
        }
        folder::sort_paths(&mut file_paths);
        for p in &file_paths {
            self.state.loaded_paths.insert(p.clone());
        }
        (file_paths, folder_paths)
    }

    pub fn load_startup_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let (file_paths, folder_paths) = self.classify_paths(paths);
        if !file_paths.is_empty() {
            self.spawn_loaders(file_paths, ctx);
        }
        if !folder_paths.is_empty() {
            self.folder.queue_scan(folder_paths);
        }
    }

    /// 读取本帧的拖拽事件（egui 的 `dropped_files` 只保留一帧，读走即失）。
    ///
    /// 目录直接交给 folder 的扫描队列（立即接受，不受图片加载进度影响）；
    /// 图片文件在加载中时缓存到 `pending_drops`，空闲后按序启动。
    fn poll_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let paths = dropped.into_iter().filter_map(|f| f.path).collect();
        let (file_paths, folder_paths) = self.classify_paths(paths);
        if !file_paths.is_empty() && self.is_busy() {
            self.pending_drops.extend(file_paths);
        } else if !file_paths.is_empty() {
            self.spawn_loaders(file_paths, ctx);
        }
        if !folder_paths.is_empty() {
            self.folder.queue_scan(folder_paths);
        }
    }

    /// 加载完成后，把缓存中的图片文件启动为新一批（重新分类：
    /// 缓存期间格子可能已被删除/加满，名额变了）。
    fn drain_pending_drops(&mut self, ctx: &egui::Context) {
        if self.is_busy() || self.pending_drops.is_empty() {
            return;
        }
        let paths = std::mem::take(&mut self.pending_drops);
        let (file_paths, _folder_paths) = self.classify_paths(paths);
        if !file_paths.is_empty() {
            self.spawn_loaders(file_paths, ctx);
        }
    }

    /// 启动一批 standalone 图片加载：每张图一个临时线程，
    /// 线程内读文件 → 解码 → EXIF → 直方图（纯 CPU，无共享状态）。
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

    /// 每帧把已完成的解码结果搬进 state，收齐后按序追加。
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
                if idx < self.state.cell_order.len()
                    && let CellKind::Image(img_idx) = self.state.cell_order[idx]
                {
                    self.rotate_image_cell(img_idx, ui.ctx());
                }
            }
        }

        let (space, b_key, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if space || b_key || esc {
            if esc {
                let mut to_close: Vec<usize> = (0..self.state.cell_order.len())
                    .filter(|pos| {
                        matches!(
                            self.state.cell_order[*pos],
                            CellKind::Image(img_idx)
                                if matches!(
                                    self.state.image_cells[img_idx].source,
                                    ImageSource::FromFolder { .. }
                                )
                        )
                    })
                    .collect();
                to_close.sort_unstable();
                for pos in to_close.into_iter().rev() {
                    self.state.close_folder_at_pos(pos);
                }
                changed = true;
            } else if !self.is_busy() {
                // 导航条件：每个文件夹打开 ≤1 张（单文件夹 1 张 / 多文件夹对比
                // 对都响应，同步推进每个文件夹的索引；单文件夹多图不响应）。
                let delta: i32 = if space { 1 } else { -1 };
                let targets = self.state.folder_nav_targets(delta);
                if !targets.is_empty() && self.state.folder_nav_allowed() {
                    self.folder.navigate(ui.ctx(), targets);
                } else if space {
                    for fi in 0..self.state.folder_cells.len() {
                        if !self.state.folder_cells[fi].selected.is_empty() {
                            self.folder.open_selected(&mut self.state, ui.ctx(), fi);
                            break;
                        }
                    }
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
        self.folder.poll_scan(&mut self.state);
        self.folder.poll_loading(&mut self.state, ui.ctx());
        self.drain_pending_drops(ui.ctx());
        self.folder.drain_thumbnails(&mut self.state, ui.ctx());

        egui::CentralPanel::default().show(ui, |ui| {
            let actions = image_grid(
                ui,
                &mut self.state,
                self.loading_total - self.loading_received,
            );
            for action in actions {
                self.folder.handle_action(&mut self.state, action, ui.ctx());
            }
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

pub fn image_grid(
    ui: &mut egui::Ui,
    state: &mut AppState,
    loading_count: usize,
) -> Vec<FolderAction> {
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
                ui.label(format!(
                    "Drag images or folders here to view  (max {})",
                    MAX_IMAGES
                ));
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
        return Vec::new();
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
    let mut folder_actions: Vec<FolderAction> = Vec::new();

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

            let Some(&cell_kind) = state.cell_order.get(cell_pos) else {
                x += layout.cell_w;
                continue;
            };

            match cell_kind {
                CellKind::Image(img_idx) => {
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
                }
                CellKind::Folder(folder_idx) => {
                    // 对比对（2 文件夹各 1 张）禁删：防误删对比图，用 Esc 退出
                    if ctrl
                        && !state.is_compare_pair()
                        && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary))
                        && ui
                            .input(|i| i.pointer.hover_pos())
                            .is_some_and(|hp| cell_rect.contains(hp))
                    {
                        state.pending_remove.push(cell_pos);
                    }
                    let action = folder::render_folder_cell(
                        ui,
                        &mut state.folder_cells[folder_idx],
                        cell_rect,
                    );
                    if !matches!(action, FolderAction::None) {
                        folder_actions.push(action);
                    }
                }
            }

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
    folder_actions
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

    // 对比对（2 文件夹各 1 张）禁删：防误删对比图，用 Esc 退出
    if ctrl
        && !state.is_compare_pair()
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
    // EXIF 摘要第一行显示图片名称，其余照旧（数据层不变，仅展示时拼接）
    let exif_display = if state.show_exif {
        let name = cell
            .info
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image");
        if cell.info.exif.is_empty() {
            name.to_string()
        } else {
            format!("{name}\n{}", cell.info.exif)
        }
    } else {
        String::new()
    };
    imcell::draw_overlay(
        ui,
        cell_rect,
        &cell.info,
        cell.selection,
        &label,
        &exif_display,
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
