use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::image::AvgStats;

pub struct ImageInfo {
    pub texture: eframe::egui::TextureHandle,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub path: PathBuf,
}

pub type NormRect = [f32; 4];

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    NewSelection,
    MoveSelection,
}

pub struct AppState {
    pub images: Vec<ImageInfo>,
    pub local_mode: bool,
    pub show_exif: bool,
    pub show_histogram: bool,
    pub selection: Vec<Option<NormRect>>,
    pub avg_stats: Vec<Option<AvgStats>>,
    pub exif: Vec<String>,
    pub histogram: Vec<[u32; 256]>,
    pub zoom: f32,
    pub pan: [f32; 2],
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
            images: Vec::new(),
            local_mode: false,
            show_exif: false,
            show_histogram: false,
            selection: Vec::new(),
            avg_stats: Vec::new(),
            exif: Vec::new(),
            histogram: Vec::new(),
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

    pub fn append_images(&mut self, images: Vec<ImageInfo>) {
        let old_len = self.images.len();
        let new_len = old_len + images.len();
        self.images.extend(images);
        self.avg_stats.resize(new_len, None);
        self.selection.resize(new_len, None);
        self.pan_offset.resize(new_len, [0.0, 0.0]);
        for img in &self.images[old_len..] {
            self.loaded_paths.insert(img.path.clone());
        }
        self.avg_stats.fill(None);
        self.selection.fill(None);
    }

    pub fn drag_start_new(&mut self, cell: usize, norm: [f32; 2]) {
        self.selection.iter_mut().for_each(|s| *s = None);
        self.avg_stats.fill(None);
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
                for s in &mut self.selection {
                    *s = Some(rect);
                }
                true
            }
            Some(DragKind::MoveSelection) => {
                if let (Some(cell), Some(origin)) = (self.drag_cell, self.drag_origin) {
                    let dx = norm[0] - origin[0];
                    let dy = norm[1] - origin[1];
                    if let Some(sel) = &mut self.selection[cell] {
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

    pub fn swap_images(&mut self, a: usize, b: usize) {
        self.images.swap(a, b);
        self.avg_stats.swap(a, b);
        self.exif.swap(a, b);
        self.histogram.swap(a, b);
        self.selection.swap(a, b);
        self.pan_offset.swap(a, b);
    }

    pub fn remove_image(&mut self, idx: usize) {
        self.loaded_paths.remove(&self.images[idx].path);
        self.images.remove(idx);
        self.avg_stats.remove(idx);
        self.exif.remove(idx);
        self.histogram.remove(idx);
        self.selection.remove(idx);
        self.pan_offset.remove(idx);
    }
}
