use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::image::AvgStats;

pub struct ImageInfo {
    pub texture: eframe::egui::TextureHandle,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub path: PathBuf,
}

pub struct ImageCell {
    pub info: ImageInfo,
    pub selection: Option<NormRect>,
    pub avg_stats: Option<AvgStats>,
    pub exif: String,
    pub histogram: [u32; 256],
}

impl ImageCell {
    fn from_info(info: ImageInfo, exif: String, histogram: [u32; 256]) -> Self {
        Self {
            info,
            selection: None,
            avg_stats: None,
            exif,
            histogram,
        }
    }
}

#[derive(Clone, Copy)]
pub enum CellKind {
    Image(usize), // index into AppState.image_cells
}

pub type NormRect = [f32; 4];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DragKind {
    NewSelection,
    MoveSelection,
}

pub struct AppState {
    pub image_cells: Vec<ImageCell>,
    /// Display order of all cells.
    pub cell_order: Vec<CellKind>,

    pub local_mode: bool,
    pub show_exif: bool,
    pub show_histogram: bool,

    /// Global zoom (image cells only).
    pub zoom: f32,
    /// Global pan (left-drag, image cells only).
    pub pan: [f32; 2],
    /// Per-cell pan offset, indexed by cell_order position.
    pub pan_offset: Vec<[f32; 2]>,

    pub loaded_paths: HashSet<PathBuf>,
    pub reorder_src: Option<usize>,
    pub pending_remove: Vec<usize>,

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
            drag_origin: None,
            drag_cell: None,
            drag_kind: None,
        }
    }

    /// Add image cells (from drag/drop or command line).
    pub fn append_standalone_images(
        &mut self,
        infos: Vec<ImageInfo>,
        exif: Vec<String>,
        histogram: Vec<[u32; 256]>,
    ) {
        let start = self.image_cells.len();
        for (i, info) in infos.into_iter().enumerate() {
            self.image_cells
                .push(ImageCell::from_info(info, exif[i].clone(), histogram[i]));
            self.cell_order.push(CellKind::Image(start + i));
            self.pan_offset.push([0.0, 0.0]);
        }
    }

    /// Remove a cell from cell_order.
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

    /// Whether there is at least one cell to render.
    pub fn is_all_images(&self) -> bool {
        self.cell_order
            .iter()
            .all(|c| matches!(c, CellKind::Image(_)))
            && !self.cell_order.is_empty()
    }
}
