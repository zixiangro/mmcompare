use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{AppState, ImageInfo};

use crate::ui;

pub struct MmCompare {
    state: AppState,
    pending_open: bool,
    /// Receiver for background-decoded images (one per image).
    load_rx: Option<mpsc::Receiver<Option<core::image::DecodedImage>>>,
    /// Total number of images being loaded.
    loading_total: usize,
    /// Number already received.
    loading_received: usize,
    /// Accumulated decoded images.
    loading_buf: Vec<core::image::DecodedImage>,
}

impl Default for MmCompare {
    fn default() -> Self {
        Self {
            state: AppState::new(),
            pending_open: false,
            load_rx: None,
            loading_total: 0,
            loading_received: 0,
            loading_buf: Vec::new(),
        }
    }
}

impl MmCompare {
    /// Open file dialog (blocking, main thread), then spawn one thread per image to decode in parallel.
    fn start_open(&mut self, ctx: &egui::Context) {
        let paths: Vec<PathBuf> = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
            .pick_files()
            .map(|v| v.into_iter().take(8).collect())
            .unwrap_or_default();

        if paths.is_empty() {
            return;
        }

        self.loading_total = paths.len();
        self.loading_received = 0;
        self.loading_buf.clear();
        let (tx, rx) = mpsc::channel();

        for p in paths {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let decoded = (|| {
                    let bytes = std::fs::read(&p).ok()?;
                    let mut img = core::image::decode_image_bytes(&bytes)?;
                    img.path = p;
                    Some(img)
                })();
                tx.send(decoded).ok();
            });
        }
        drop(tx); // so rx won't block after all senders are gone

        self.load_rx = Some(rx);
        ctx.request_repaint();
    }

    /// Poll for completed decodes and upload textures when all are done.
    fn poll_loading(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.load_rx else {
            return;
        };

        // Drain all available messages
        while let Ok(decoded) = rx.try_recv() {
            if let Some(img) = decoded {
                self.loading_buf.push(img);
            }
            self.loading_received += 1;
        }

        if self.loading_received >= self.loading_total {
            let decoded = std::mem::take(&mut self.loading_buf);
            let images: Vec<ImageInfo> = decoded
                .into_iter()
                .map(|d| {
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(d.size, &d.rgba);
                    let texture = ctx.load_texture(
                        d.path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("image"),
                        color_image,
                        egui::TextureOptions::default(),
                    );
                    ImageInfo {
                        texture,
                        size: d.size,
                        path: d.path,
                    }
                })
                .collect();

            self.state.set_images(images);
            self.load_rx = None;
            self.loading_total = 0;
            self.loading_received = 0;
            ctx.request_repaint();
        }
    }
}

impl eframe::App for MmCompare {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.pending_open {
            self.pending_open = false;
            let ctx = ui.ctx().clone();
            self.start_open(&ctx);
        }

        self.poll_loading(ui.ctx());

        ui::menu::menu_bar(ui, &mut self.pending_open);

        egui::CentralPanel::default().show(ui, |ui| {
            ui::viewer::image_grid(
                ui,
                &self.state.images,
                self.loading_total - self.loading_received,
            );
        });
    }
}
