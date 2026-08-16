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

    /// 把文件夹条目打开为图片 cell，同时从网格隐藏该文件夹 cell。
    pub fn open_folder_entry(&mut self, folder_idx: usize, entry_idx: usize, info: ImageInfo) {
        let img_idx = self.image_cells.len();
        self.image_cells.push(ImageCell::from_info(
            info,
            ImageSource::FromFolder {
                folder_idx,
                entry_idx,
            },
        ));
        self.cell_order.push(CellKind::Image(img_idx));
        self.pan_offset.push([0.0, 0.0]);
        self.folder_cells[folder_idx].open_entry = Some(entry_idx);
        if let Some(fpos) = self
            .cell_order
            .iter()
            .position(|c| matches!(c, CellKind::Folder(fi) if *fi == folder_idx))
        {
            self.cell_order.remove(fpos);
            self.pan_offset.remove(fpos);
        }
    }

    /// 文件夹导航（Space/B）的目标条目：只计算不改状态，
    /// 由 imlayout 加载完成后统一写入（`apply_navigated_image`）。
    pub fn folder_nav_target(&self, img_idx: usize, delta: i32) -> Option<(usize, usize, PathBuf)> {
        let cell = self.image_cells.get(img_idx)?;
        let ImageSource::FromFolder {
            folder_idx,
            entry_idx,
        } = cell.source
        else {
            return None;
        };
        let folder = self.folder_cells.get(folder_idx)?;
        let new_idx = (entry_idx as i32 + delta).clamp(0, folder.entries.len() as i32 - 1) as usize;
        if new_idx == entry_idx {
            return None;
        }
        Some((folder_idx, new_idx, folder.entries[new_idx].clone()))
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

    /// Esc：关闭文件夹图片，恢复文件夹 cell 显示。
    pub fn close_folder_at_pos(&mut self, cell_pos: usize) {
        let Some(&CellKind::Image(img_idx)) = self.cell_order.get(cell_pos) else {
            return;
        };
        let ImageSource::FromFolder { folder_idx, .. } = self.image_cells[img_idx].source else {
            return;
        };
        self.remove_cell(cell_pos);
        self.folder_cells[folder_idx].open_entry = None;
        self.show_folder_cell(folder_idx);
    }

    fn show_folder_cell(&mut self, folder_idx: usize) {
        if !self
            .cell_order
            .iter()
            .any(|c| matches!(c, CellKind::Folder(fi) if *fi == folder_idx))
        {
            self.cell_order.push(CellKind::Folder(folder_idx));
            self.pan_offset.push([0.0, 0.0]);
        }
    }

    pub fn remove_cell(&mut self, cell_order_pos: usize) {
        let Some(&cell_kind) = self.cell_order.get(cell_order_pos) else {
            return;
        };
        match cell_kind {
            CellKind::Image(img_idx) => {
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
