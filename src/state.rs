//! 全局状态：图片数据、显示顺序、交互状态。
//!
//! 设计约束（ADR-0004）：
//! - 本模块是**纯数据结构**：只有状态与状态转移的薄方法，不含业务/渲染逻辑；
//! - 全部字段都是普通值、没有锁——多线程只发生在 imlayout.rs 的加载管线，
//!   线程间通过 mpsc 传自有数据，写入 state 的时机永远在主线程（ADR-0001）。
//!
//! 索引约定（三处必须保持一致，否则会静默画错图）：
//! - `image_cells` 是图片的**实际存储**，删除元素时下标会移动；
//! - `cell_order` 是**显示顺序**，元素是 `CellKind::Image(usize)`，
//!   其中的 usize 是 `image_cells` 的下标；
//! - `pan_offset` 与 `cell_order` 同长度、同顺序，按"格子"而非"图片"索引，
//!   重排（swap）后平移量跟着格子走而不是跟着图片走。

use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::image::AvgStats;

pub const MAX_IMAGES: usize = 8;

pub struct ImageInfo {
    pub texture: eframe::egui::TextureHandle,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub path: PathBuf,
    pub exif: String,
    pub histogram: [u32; 256],
}

pub struct ImageCell {
    pub info: ImageInfo,
    pub selection: Option<NormRect>,
    pub avg_stats: Option<AvgStats>,
}

impl ImageCell {
    fn from_info(info: ImageInfo) -> Self {
        Self {
            info,
            selection: None,
            avg_stats: None,
        }
    }
}

#[derive(Clone, Copy)]
pub enum CellKind {
    Image(usize),
}

pub type NormRect = [f32; 4];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DragKind {
    NewSelection,
    MoveSelection,
}

pub struct AppState {
    pub image_cells: Vec<ImageCell>,
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
            self.image_cells.push(ImageCell::from_info(info));
            self.cell_order.push(CellKind::Image(start + i));
            self.pan_offset.push([0.0, 0.0]);
        }
    }

    pub fn remove_cell(&mut self, cell_order_pos: usize) {
        let Some(&CellKind::Image(img_idx)) = self.cell_order.get(cell_order_pos) else {
            return;
        };
        self.loaded_paths
            .remove(&self.image_cells[img_idx].info.path);
        self.image_cells.remove(img_idx);
        for CellKind::Image(idx) in &mut self.cell_order {
            if *idx > img_idx {
                *idx -= 1;
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
