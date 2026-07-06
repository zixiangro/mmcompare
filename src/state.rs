use std::path::PathBuf;

use eframe::egui;

pub struct ImageInfo {
    pub texture: egui::TextureHandle,
    /// Original dimensions [width, height].
    pub size: [usize; 2],
    #[allow(dead_code)]
    pub path: PathBuf,
}

pub struct AppState {
    pub images: Vec<ImageInfo>,
}

impl AppState {
    pub fn new() -> Self {
        Self { images: Vec::new() }
    }

    pub fn set_images(&mut self, images: Vec<ImageInfo>) {
        self.images = images;
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.images.clear();
    }
}
