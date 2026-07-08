use std::path::PathBuf;

use eframe::egui;

pub struct ImageInfo {
    pub texture: egui::TextureHandle,
    /// Original dimensions [width, height].
    pub size: [usize; 2],
    /// Raw RGBA pixels for sampling (avg Y computation).
    pub rgba: Vec<u8>,
    #[allow(dead_code)]
    pub path: PathBuf,
}

/// Normalized selection rectangle [x1, y1, x2, y2] in 0..1 range.
pub type NormRect = [f32; 4];

pub struct AppState {
    pub images: Vec<ImageInfo>,
    /// Whether local mode is active (toggled by 'L').
    pub local_mode: bool,
    /// Show EXIF overlay (toggled by 'E').
    pub show_exif: bool,
    /// Show histogram overlay (toggled by 'H').
    pub show_histogram: bool,
    /// The current selection (normalized coords), if any.
    pub selection: Option<NormRect>,
    /// Per-image average Y brightness in the selection region.
    pub avg_y: Vec<Option<f32>>,
    /// Per-image EXIF text (multi-line).
    pub exif: Vec<String>,
    /// Per-image 256-bin Y histogram.
    pub histogram: Vec<[u32; 256]>,
    /// Per-image zoom level (1.0 = default fit).
    pub zoom: f32,
    /// Pan offset in pixels (non-local mode).
    pub pan: [f32; 2],
    /// Drag origin in normalized coords, set on drag start.
    drag_origin: Option<[f32; 2]>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            images: Vec::new(),
            local_mode: false,
            show_exif: false,
            show_histogram: false,
            selection: None,
            avg_y: Vec::new(),
            exif: Vec::new(),
            histogram: Vec::new(),
            zoom: 1.0,
            pan: [0.0, 0.0],
            drag_origin: None,
        }
    }

    pub fn set_images(&mut self, images: Vec<ImageInfo>) {
        self.avg_y.resize(images.len(), None);
        self.selection = None;
        self.images = images;
    }

    pub fn append_images(&mut self, images: Vec<ImageInfo>) {
        let old_len = self.images.len();
        self.images.extend(images);
        self.avg_y.resize(self.images.len(), None);
        self.avg_y[..old_len].fill(None);
        self.selection = None;
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.images.clear();
        self.avg_y.clear();
        self.exif.clear();
        self.histogram.clear();
        self.selection = None;
    }

    /// Start a selection drag at normalized position.
    pub fn drag_start(&mut self, norm: [f32; 2]) {
        self.selection = None;
        self.drag_origin = Some(norm);
    }

    /// Update selection during drag. Returns true if changed.
    pub fn drag_update(&mut self, norm: [f32; 2]) -> bool {
        let Some(origin) = self.drag_origin else {
            return false;
        };
        let rect = [
            origin[0].min(norm[0]),
            origin[1].min(norm[1]),
            origin[0].max(norm[0]),
            origin[1].max(norm[1]),
        ];
        self.selection = Some(rect);
        true
    }

    /// Finish drag. Returns the new selection rect, if any.
    pub fn drag_end(&mut self) -> Option<NormRect> {
        self.drag_origin = None;
        self.selection
    }

    #[inline]
    pub fn is_dragging(&self) -> bool {
        self.drag_origin.is_some()
    }
}
