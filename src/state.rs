//! 全局状态：图片数据、显示顺序、交互状态。
//!
//! 设计约束（ADR-0004）：
//! - 本模块是**纯数据结构**：只有状态与状态转移的薄方法，不含业务/渲染逻辑；
//! - 全部字段都是普通值、没有锁——多线程只发生在 imlayout.rs 的加载管线，
//!   线程间通过 mpsc 传自有数据，写入 state 的时机永远在主线程（ADR-0001）。
//!
//! 索引约定（多处必须保持一致，否则会静默画错图）：
//! - `image_cells` / `folder_cells` 是实际存储，删除元素时下标会移动；
//! - `cell_order` 是**显示顺序**，元素是 `CellKind::Image(usize)` 或
//!   `CellKind::Folder(usize)`，usize 分别是两个存储 vec 的下标；
//! - `pan_offset` 与 `cell_order` 同长度、同顺序，按"格子"而非"cell"索引，
//!   重排（swap）后平移量跟着格子走而不是跟着 cell 走。

use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::image::AvgStats;

pub const MAX_IMAGES: usize = 8;

/// 文件夹 cell 渲染层上报的用户意图，由 imlayout 统一处理。
#[derive(Clone)]
pub enum FolderAction {
    None,
    OpenImage(usize),
    OpenFolder(PathBuf),
    Remove(usize),
    OpenSelected,
    ToggleView,
    /// 无修饰单击：所有文件夹 cell 联动选中同索引（双栏对比的基准操作）。
    SelectSynced(usize),
}

pub struct ImageInfo {
    pub texture: eframe::egui::TextureHandle,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub path: PathBuf,
    pub exif: String,
    pub histogram: [u32; 256],
}

/// 图片的来源：独立拖入，或从文件夹 cell 打开。
#[derive(Clone, Copy)]
pub enum ImageSource {
    Standalone,
    FromFolder { folder_idx: usize, entry_idx: usize },
}

pub struct ImageCell {
    pub info: ImageInfo,
    pub source: ImageSource,
    pub selection: Option<NormRect>,
    pub avg_stats: Option<AvgStats>,
}

impl ImageCell {
    fn from_info(info: ImageInfo, source: ImageSource) -> Self {
        Self {
            info,
            source,
            selection: None,
            avg_stats: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum FolderView {
    List,
    Thumbnail,
}

/// 一个文件夹 cell：目录内容 + 视图状态 + 缩略图。
///
/// `thumbnails` 与 `entries` 等长对齐（失败槽位为 `None`），
/// 保证渲染时按索引取缩略图不会错位。
pub struct FolderCell {
    pub dir_path: PathBuf,
    pub entries: Vec<PathBuf>,
    pub selected: HashSet<usize>,
    pub view_mode: FolderView,
    pub scroll_offset: f32,
    pub thumbnails: Vec<Option<eframe::egui::TextureHandle>>,
    pub open_entry: Option<usize>,
    /// 请求滚动到条目（恢复视图后由渲染层调整 `scroll_offset` 并清除）：
    /// 退出对比时导航位置在视口外，只高亮看不到。
    pub scroll_to: Option<usize>,
}

#[derive(Clone, Copy)]
pub enum CellKind {
    Image(usize),
    Folder(usize),
}

pub type NormRect = [f32; 4];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DragKind {
    NewSelection,
    MoveSelection,
}

pub struct AppState {
    pub image_cells: Vec<ImageCell>,
    pub folder_cells: Vec<FolderCell>,
    pub cell_order: Vec<CellKind>,

    pub local_mode: bool,
    pub show_exif: bool,
    pub show_histogram: bool,

    pub zoom: f32,
    pub pan: [f32; 2],
    pub pan_offset: Vec<[f32; 2]>,

    pub loaded_paths: HashSet<PathBuf>,
    pub reorder_src: Option<usize>,
    pub pending_remove: Vec<usize>,
    pub load_errors: Vec<PathBuf>,

    drag_origin: Option<[f32; 2]>,
    drag_cell: Option<usize>,
    drag_kind: Option<DragKind>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            image_cells: Vec::new(),
            folder_cells: Vec::new(),
            cell_order: Vec::new(),
            local_mode: false,
            show_exif: false,
            show_histogram: false,
            zoom: 1.0,
            pan: [0.0, 0.0],
            pan_offset: Vec::new(),
            loaded_paths: HashSet::new(),
            reorder_src: None,
            pending_remove: Vec::new(),
            load_errors: Vec::new(),
            drag_origin: None,
            drag_cell: None,
            drag_kind: None,
        }
    }

    pub fn append_standalone_images(&mut self, infos: Vec<ImageInfo>) {
        let start = self.image_cells.len();
        for (i, info) in infos.into_iter().enumerate() {
            self.image_cells
                .push(ImageCell::from_info(info, ImageSource::Standalone));
            self.cell_order.push(CellKind::Image(start + i));
            self.pan_offset.push([0.0, 0.0]);
        }
    }

    /// 把文件夹条目打开为图片 cell。
    ///
    /// 打开限制：**多文件夹（≥2）时每文件夹限 1 张**——重复打开同文件夹
    /// 条目自动替换已有图片（不新增 cell）；**单文件夹可开多张**（唯一
    /// 支持多图的场景）。网格满时仍允许打开**可见**文件夹的条目
    /// （打开后文件夹隐藏，净增 0 格）；打开后从网格隐藏该文件夹 cell。
    pub fn open_folder_entry(&mut self, folder_idx: usize, entry_idx: usize, info: ImageInfo) {
        if self.folder_cells.len() >= 2
            && let Some(pos) = self.image_cells.iter().position(|c| {
                matches!(
                    c.source,
                    ImageSource::FromFolder {
                        folder_idx: fi,
                        ..
                    } if fi == folder_idx
                )
            })
        {
            let cell = &mut self.image_cells[pos];
            cell.info = info;
            cell.selection = None;
            cell.avg_stats = None;
            cell.source = ImageSource::FromFolder {
                folder_idx,
                entry_idx,
            };
            self.folder_cells[folder_idx].open_entry = Some(entry_idx);
            return;
        }
        // 打开后：图片 +1，文件夹若可见则隐藏 -1，净增 = 1 - 可见性
        let folder_visible = self
            .cell_order
            .iter()
            .any(|c| matches!(c, CellKind::Folder(fi) if *fi == folder_idx));
        if self.cell_order.len() + 1 - usize::from(folder_visible) > MAX_IMAGES {
            return;
        }
        let img_idx = self.image_cells.len();
        self.image_cells.push(ImageCell::from_info(
            info,
            ImageSource::FromFolder {
                folder_idx,
                entry_idx,
            },
        ));
        // 图片占据被隐藏文件夹的**原位置**：左文件夹的图在左 cell，右同理
        // （依次打开时顺序也始终与文件夹对应）。
        let fpos = self
            .cell_order
            .iter()
            .position(|c| matches!(c, CellKind::Folder(fi) if *fi == folder_idx));
        if let Some(fpos) = fpos {
            self.cell_order[fpos] = CellKind::Image(img_idx);
            self.pan_offset[fpos] = [0.0, 0.0];
        } else {
            self.cell_order.push(CellKind::Image(img_idx));
            self.pan_offset.push([0.0, 0.0]);
        }
        self.folder_cells[folder_idx].open_entry = Some(entry_idx);
    }

    /// 导航（Space/B）的目标条目：所有打开的文件夹图片各自的下一条/上一条。
    /// 返回 (图片下标, 文件夹下标, 新条目下标, 路径)，由 imlayout 统一加载。
    pub fn folder_nav_targets(&self, delta: i32) -> Vec<(usize, usize, usize, PathBuf)> {
        let mut targets = Vec::new();
        for (img_idx, cell) in self.image_cells.iter().enumerate() {
            let ImageSource::FromFolder {
                folder_idx,
                entry_idx,
            } = cell.source
            else {
                continue;
            };
            let Some(folder) = self.folder_cells.get(folder_idx) else {
                continue;
            };
            if folder.entries.is_empty() {
                continue; // 防御：空条目时 clamp(0, -1) 会 panic
            }
            let new_idx =
                (entry_idx as i32 + delta).clamp(0, folder.entries.len() as i32 - 1) as usize;
            if new_idx != entry_idx {
                targets.push((
                    img_idx,
                    folder_idx,
                    new_idx,
                    folder.entries[new_idx].clone(),
                ));
            }
        }
        targets
    }

    /// 每个文件夹打开的图片数（事件响应的输入之一）。
    pub fn folder_open_counts(&self) -> Vec<usize> {
        let mut counts = vec![0usize; self.folder_cells.len()];
        for cell in &self.image_cells {
            if let ImageSource::FromFolder { folder_idx, .. } = cell.source
                && let Some(c) = counts.get_mut(folder_idx)
            {
                *c += 1;
            }
        }
        counts
    }

    /// 导航条件：至少一个文件夹图片，且**每个文件夹打开 ≤1 张**——
    /// 同一文件夹多图时索引语义歧义，不响应 Space/B。
    pub fn folder_nav_allowed(&self) -> bool {
        let counts = self.folder_open_counts();
        !counts.is_empty() && counts.iter().all(|&c| c <= 1) && counts.contains(&1)
    }

    /// 对比对：恰好两个文件夹、各打开 1 张。此时**禁删图**（防误删对比图），
    /// 用 Esc 退出对比恢复文件夹视图。
    pub fn is_compare_pair(&self) -> bool {
        self.folder_cells.len() == 2 && self.folder_open_counts() == [1, 1]
    }

    /// 导航加载完成后替换图片内容，并作废选区（像素已变）。
    pub fn apply_navigated_image(
        &mut self,
        img_idx: usize,
        folder_idx: usize,
        entry_idx: usize,
        info: ImageInfo,
    ) {
        let cell = &mut self.image_cells[img_idx];
        cell.info = info;
        cell.selection = None;
        cell.avg_stats = None;
        cell.source = ImageSource::FromFolder {
            folder_idx,
            entry_idx,
        };
        self.folder_cells[folder_idx].open_entry = Some(entry_idx);
    }

    /// 关闭文件夹图片（Esc）：删除图片 cell，文件夹 cell 由
    /// `remove_cell` 的 Image 分支自动恢复。
    pub fn close_folder_at_pos(&mut self, cell_pos: usize) {
        let Some(&CellKind::Image(img_idx)) = self.cell_order.get(cell_pos) else {
            return;
        };
        if matches!(
            self.image_cells[img_idx].source,
            ImageSource::FromFolder { .. }
        ) {
            self.remove_cell(cell_pos);
        }
    }

    pub fn remove_cell(&mut self, cell_order_pos: usize) {
        let Some(&cell_kind) = self.cell_order.get(cell_order_pos) else {
            return;
        };
        // 待恢复的文件夹 (下标, 原位)：从文件夹打开的图片删除后，
        // 文件夹恢复到图片占据的原位置（保持左右对应）。
        let mut restore: Option<(usize, usize)> = None;
        match cell_kind {
            CellKind::Image(img_idx) => {
                let was_folder_image = matches!(
                    self.image_cells[img_idx].source,
                    ImageSource::FromFolder { .. }
                );
                let folder_idx = match self.image_cells[img_idx].source {
                    ImageSource::FromFolder { folder_idx, .. } => Some(folder_idx),
                    _ => None,
                };
                self.loaded_paths
                    .remove(&self.image_cells[img_idx].info.path);
                self.image_cells.remove(img_idx);
                for entry in &mut self.cell_order {
                    if let CellKind::Image(idx) = entry
                        && *idx > img_idx
                    {
                        *idx -= 1;
                    }
                }
                if was_folder_image && let Some(folder_idx) = folder_idx {
                    // 该文件夹还有其他打开图时保持隐藏，全部关完才恢复原位
                    let still_open = self.image_cells.iter().any(|c| {
                        matches!(
                            c.source,
                            ImageSource::FromFolder {
                                folder_idx: fi,
                                ..
                            } if fi == folder_idx
                        )
                    });
                    if !still_open && let Some(folder) = self.folder_cells.get_mut(folder_idx) {
                        // 退出位置记忆：导航后的当前条目写回选中，恢复视图后
                        // 仍高亮/可继续打开退出时的图片；同时请求滚动到该条目
                        if let Some(entry) = folder.open_entry {
                            folder.selected.clear();
                            folder.selected.insert(entry);
                            folder.scroll_to = Some(entry);
                        }
                        folder.open_entry = None;
                        restore = Some((folder_idx, cell_order_pos));
                    }
                }
            }
            CellKind::Folder(folder_idx) => {
                for cell in &mut self.image_cells {
                    match cell.source {
                        ImageSource::FromFolder {
                            folder_idx: fi,
                            entry_idx: _,
                        } if fi == folder_idx => cell.source = ImageSource::Standalone,
                        ImageSource::FromFolder {
                            folder_idx: fi,
                            entry_idx,
                        } if fi > folder_idx => {
                            cell.source = ImageSource::FromFolder {
                                folder_idx: fi - 1,
                                entry_idx,
                            };
                        }
                        _ => {}
                    }
                }
                self.folder_cells.remove(folder_idx);
                for entry in &mut self.cell_order {
                    if let CellKind::Folder(idx) = entry
                        && *idx > folder_idx
                    {
                        *idx -= 1;
                    }
                }
            }
        }
        self.cell_order.remove(cell_order_pos);
        self.pan_offset.remove(cell_order_pos);
        if let Some((folder_idx, pos)) = restore {
            self.cell_order.insert(pos, CellKind::Folder(folder_idx));
            self.pan_offset.insert(pos, [0.0, 0.0]);
        }
    }

    pub fn drag_start_new(&mut self, cell: usize, norm: [f32; 2]) {
        for img in &mut self.image_cells {
            img.selection = None;
            img.avg_stats = None;
        }
        self.drag_origin = Some(norm);
        self.drag_cell = Some(cell);
        self.drag_kind = Some(DragKind::NewSelection);
    }

    pub fn drag_start_move(&mut self, cell: usize, norm: [f32; 2]) {
        self.drag_origin = Some(norm);
        self.drag_cell = Some(cell);
        self.drag_kind = Some(DragKind::MoveSelection);
    }

    pub fn drag_update(&mut self, norm: [f32; 2]) -> bool {
        match self.drag_kind {
            Some(DragKind::NewSelection) => {
                let origin = self.drag_origin.unwrap();
                let rect = [
                    origin[0].min(norm[0]),
                    origin[1].min(norm[1]),
                    origin[0].max(norm[0]),
                    origin[1].max(norm[1]),
                ];
                for img in &mut self.image_cells {
                    img.selection = Some(rect);
                }
                true
            }
            Some(DragKind::MoveSelection) => {
                if let (Some(cell), Some(origin)) = (self.drag_cell, self.drag_origin) {
                    let dx = norm[0] - origin[0];
                    let dy = norm[1] - origin[1];
                    if let Some(sel) = &mut self.image_cells[cell].selection {
                        sel[0] = (sel[0] + dx).clamp(0.0, 1.0);
                        sel[1] = (sel[1] + dy).clamp(0.0, 1.0);
                        sel[2] = (sel[2] + dx).clamp(0.0, 1.0);
                        sel[3] = (sel[3] + dy).clamp(0.0, 1.0);
                        if sel[0] >= sel[2] {
                            sel[0] = sel[2] - 0.001;
                        }
                        if sel[1] >= sel[3] {
                            sel[1] = sel[3] - 0.001;
                        }
                    }
                    self.drag_origin = Some(norm);
                }
                true
            }
            None => false,
        }
    }

    pub fn drag_end(&mut self) -> Option<usize> {
        let cell = self.drag_cell;
        self.drag_origin = None;
        self.drag_cell = None;
        self.drag_kind = None;
        cell
    }

    #[inline]
    pub fn is_dragging(&self) -> bool {
        self.drag_origin.is_some()
    }

    #[inline]
    pub fn is_moving_selection(&self) -> bool {
        self.drag_kind == Some(DragKind::MoveSelection)
    }

    pub fn swap_cells(&mut self, a: usize, b: usize) {
        self.cell_order.swap(a, b);
        self.pan_offset.swap(a, b);
    }

    pub fn invalidate_selection_after_rotation(&mut self, img_idx: usize) {
        if self.local_mode {
            for img in &mut self.image_cells {
                img.selection = None;
                img.avg_stats = None;
            }
        } else {
            let cell = &mut self.image_cells[img_idx];
            cell.selection = None;
            cell.avg_stats = None;
        }
    }

    pub fn is_all_images(&self) -> bool {
        self.cell_order
            .iter()
            .all(|c| matches!(c, CellKind::Image(_)))
            && !self.cell_order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    fn make_info(ctx: &egui::Context, name: &str) -> ImageInfo {
        let tex = ctx.load_texture(
            name,
            egui::ColorImage::new([4, 4], vec![egui::Color32::WHITE; 16]),
            egui::TextureOptions::default(),
        );
        ImageInfo {
            texture: tex,
            size: [4, 4],
            rgba: vec![255; 64],
            path: PathBuf::from(name),
            exif: String::new(),
            histogram: [0; 256],
        }
    }

    fn add_folder(state: &mut AppState, n: usize) -> usize {
        let idx = state.folder_cells.len();
        state.folder_cells.push(FolderCell {
            dir_path: PathBuf::from(format!("dir{idx}")),
            entries: (0..n)
                .map(|i| PathBuf::from(format!("dir{idx}/img{i}.png")))
                .collect(),
            selected: HashSet::new(),
            view_mode: FolderView::List,
            scroll_offset: 0.0,
            thumbnails: vec![None; n],
            open_entry: None,
            scroll_to: None,
        });
        state.cell_order.push(CellKind::Folder(idx));
        state.pan_offset.push([0.0, 0.0]);
        idx
    }

    fn image_pos(state: &AppState, img_idx: usize) -> usize {
        state
            .cell_order
            .iter()
            .position(|c| matches!(c, CellKind::Image(i) if *i == img_idx))
            .expect("image cell in order")
    }

    #[test]
    fn single_folder_allows_multi_open_but_no_nav() {
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fi = add_folder(&mut s, 5);
        s.open_folder_entry(fi, 0, make_info(&ctx, "a0"));
        s.open_folder_entry(fi, 1, make_info(&ctx, "a1"));
        assert_eq!(s.image_cells.len(), 2, "单文件夹可开多张");
        assert_eq!(s.folder_open_counts(), [2]);
        assert!(!s.folder_nav_allowed(), "同文件夹多图不响应导航");
        assert!(!s.is_compare_pair());
    }

    #[test]
    fn single_folder_one_image_nav_allowed() {
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fi = add_folder(&mut s, 5);
        s.open_folder_entry(fi, 0, make_info(&ctx, "a0"));
        assert!(s.folder_nav_allowed(), "单文件夹 1 张可导航");
        assert_eq!(s.folder_nav_targets(1).len(), 1);
    }

    #[test]
    fn multi_folder_limits_one_per_folder() {
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fa = add_folder(&mut s, 5);
        let fb = add_folder(&mut s, 5);
        s.open_folder_entry(fa, 0, make_info(&ctx, "a0"));
        s.open_folder_entry(fa, 1, make_info(&ctx, "a1"));
        assert_eq!(s.image_cells.len(), 1, "多文件夹时重复打开自动替换");
        assert!(s.folder_nav_allowed());
        assert!(!s.is_compare_pair(), "单侧开图不是对比对");
        s.open_folder_entry(fb, 0, make_info(&ctx, "b0"));
        assert!(s.is_compare_pair(), "2 文件夹各 1 张构成对比对");
        assert!(s.folder_nav_allowed());
        assert_eq!(s.folder_nav_targets(1).len(), 2, "对比对同步索引两张");
    }

    #[test]
    fn delete_down_to_one_restores_nav() {
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fi = add_folder(&mut s, 5);
        s.open_folder_entry(fi, 0, make_info(&ctx, "a0"));
        s.open_folder_entry(fi, 1, make_info(&ctx, "a1"));
        // 删一张 → 剩 1 张 → 导航恢复
        s.remove_cell(image_pos(&s, 0));
        assert_eq!(s.image_cells.len(), 1);
        assert!(s.folder_nav_allowed(), "删到 1 张恢复导航");
    }

    #[test]
    fn delete_all_restores_folder_cell() {
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fi = add_folder(&mut s, 5);
        s.open_folder_entry(fi, 0, make_info(&ctx, "a0"));
        s.open_folder_entry(fi, 1, make_info(&ctx, "a1"));
        s.remove_cell(image_pos(&s, 0));
        assert!(
            !s.cell_order
                .iter()
                .any(|c| matches!(c, CellKind::Folder(f) if *f == fi)),
            "还有打开图时文件夹保持隐藏"
        );
        s.remove_cell(image_pos(&s, 0));
        assert!(
            s.cell_order
                .iter()
                .any(|c| matches!(c, CellKind::Folder(f) if *f == fi)),
            "最后一张被删后文件夹恢复"
        );
    }

    #[test]
    fn compare_pair_remove_forbidden_by_ui_rule() {
        // is_compare_pair 本身是 UI 层禁删的判定输入，这里验证状态判定
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fa = add_folder(&mut s, 5);
        let fb = add_folder(&mut s, 5);
        s.open_folder_entry(fa, 0, make_info(&ctx, "a0"));
        s.open_folder_entry(fb, 0, make_info(&ctx, "b0"));
        assert!(s.is_compare_pair());
        // 破坏对比对（删掉一侧）→ 可删恢复
        s.remove_cell(image_pos(&s, 0));
        assert!(!s.is_compare_pair());
    }
    #[test]
    fn open_at_full_grid_when_folder_visible() {
        // 网格满（8 格）但文件夹可见：打开条目应成功（文件夹隐藏腾 1 格，净 0 变化）
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fi = add_folder(&mut s, 5);
        let infos: Vec<ImageInfo> = (0..7).map(|i| make_info(&ctx, &format!("s{i}"))).collect();
        s.append_standalone_images(infos);
        assert_eq!(s.cell_order.len(), 8, "1 folder + 7 standalone 满格");
        s.open_folder_entry(fi, 0, make_info(&ctx, "f0"));
        assert_eq!(s.image_cells.len(), 8, "打开成功");
        assert_eq!(s.cell_order.len(), 8, "folder 隐藏，格数不变");
    }

    #[test]
    fn open_rejected_when_folder_hidden_at_full_grid() {
        // 单文件夹开满 8 张（folder 已隐藏）后再开：应拒绝（净增 1 → 超限）
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fi = add_folder(&mut s, 20);
        for i in 0..8 {
            s.open_folder_entry(fi, i, make_info(&ctx, &format!("f{i}")));
        }
        assert_eq!(s.image_cells.len(), 8);
        assert!(
            !s.cell_order
                .iter()
                .any(|c| matches!(c, CellKind::Folder(f) if *f == fi)),
            "folder 已隐藏"
        );
        s.open_folder_entry(fi, 8, make_info(&ctx, "f8"));
        assert_eq!(s.image_cells.len(), 8, "第 9 张被拒");
    }
    #[test]
    fn open_image_keeps_folder_position() {
        // 左文件夹的图占据左 cell（原文件夹位置），依次打开也不串位
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fa = add_folder(&mut s, 5);
        let fb = add_folder(&mut s, 5);
        s.open_folder_entry(fa, 0, make_info(&ctx, "a0"));
        assert!(
            matches!(s.cell_order[0], CellKind::Image(_)),
            "A 的图占据 A 的原位置（左 cell）"
        );
        assert!(
            matches!(s.cell_order[1], CellKind::Folder(f) if f == fb),
            "B 的文件夹仍在右 cell"
        );
        s.open_folder_entry(fb, 0, make_info(&ctx, "b0"));
        assert_eq!(s.cell_order.len(), 2);
        // 左 cell 是 A 的图（source folder_idx == fa）
        let CellKind::Image(a_img) = s.cell_order[0] else {
            panic!("left cell is image");
        };
        assert!(
            matches!(s.image_cells[a_img].source, ImageSource::FromFolder { folder_idx, .. } if folder_idx == fa),
            "左 cell = 左文件夹的图"
        );
    }

    #[test]
    fn remove_restores_folder_to_original_position() {
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fa = add_folder(&mut s, 5);
        let fb = add_folder(&mut s, 5);
        s.open_folder_entry(fa, 0, make_info(&ctx, "a0"));
        s.open_folder_entry(fb, 0, make_info(&ctx, "b0"));
        // 删左图 → 左 folder 恢复到原位 0
        s.remove_cell(0);
        assert!(
            matches!(s.cell_order[0], CellKind::Folder(f) if f == fa),
            "A 恢复原位"
        );
        assert!(matches!(s.cell_order[1], CellKind::Image(_)), "B 图仍在右");
    }
    #[test]
    fn esc_restores_selection_to_navigated_position() {
        // 打开 → 导航 → Esc：文件夹选中回到退出时的图片位置
        let ctx = egui::Context::default();
        let mut s = AppState::new();
        let fa = add_folder(&mut s, 10);
        s.open_folder_entry(fa, 2, make_info(&ctx, "a2"));
        // 模拟 Space 导航到第 5 张
        s.apply_navigated_image(0, fa, 5, make_info(&ctx, "a5"));
        assert_eq!(s.folder_cells[fa].open_entry, Some(5));
        // Esc 关闭
        let pos = s
            .cell_order
            .iter()
            .position(|c| matches!(c, CellKind::Image(_)))
            .unwrap();
        s.close_folder_at_pos(pos);
        assert!(
            s.folder_cells[fa].selected.contains(&5),
            "选中回到退出时的图片位置"
        );
        assert_eq!(
            s.folder_cells[fa].scroll_to,
            Some(5),
            "请求滚动到退出位置（渲染层据此定位到第二条）"
        );
        assert_eq!(s.folder_cells[fa].open_entry, None, "open_entry 已清空");
        assert!(
            s.cell_order
                .iter()
                .any(|c| matches!(c, CellKind::Folder(f) if *f == fa)),
            "文件夹恢复"
        );
    }
}
