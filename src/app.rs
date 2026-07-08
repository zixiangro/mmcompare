use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{AppState, ImageInfo};
use crate::ui;

pub struct MmCompare {
    state: AppState,
    load_rx: Option<mpsc::Receiver<Option<core::image::DecodedImage>>>,
    loading_total: usize,
    loading_received: usize,
    loading_buf: Vec<core::image::DecodedImage>,
    loading_append: bool,
}

impl Default for MmCompare {
    fn default() -> Self {
        Self {
            state: AppState::new(),
            load_rx: None,
            loading_total: 0,
            loading_received: 0,
            loading_buf: Vec::new(),
            loading_append: false,
        }
    }
}

impl MmCompare {
    fn is_loading(&self) -> bool {
        self.load_rx.is_some()
    }

    fn poll_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let remaining = 8usize
            .saturating_sub(self.state.images.len())
            .saturating_sub(self.loading_total);

        let paths: Vec<PathBuf> = dropped
            .into_iter()
            .filter_map(|f| f.path)
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| {
                        matches!(
                            e.to_ascii_lowercase().as_str(),
                            "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp"
                        )
                    })
                    .unwrap_or(false)
            })
            .take(remaining)
            .collect();

        if !paths.is_empty() {
            self.spawn_loaders(paths, ctx, true);
        }
    }

    fn spawn_loaders(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context, append: bool) {
        self.loading_total = paths.len();
        self.loading_received = 0;
        self.loading_buf.clear();
        self.loading_append = append;
        let (tx, rx) = mpsc::channel();

        for p in paths {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let decoded = (|| {
                    let bytes = std::fs::read(&p).ok()?;
                    let mut img = core::image::decode_image_bytes(&bytes)?;
                    img.path = p;
                    img.raw_bytes = bytes;
                    Some(img)
                })();
                tx.send(decoded).ok();
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

        while let Ok(decoded) = rx.try_recv() {
            if let Some(img) = decoded {
                self.loading_buf.push(img);
            }
            self.loading_received += 1;
        }

        if self.loading_received >= self.loading_total {
            let decoded = std::mem::take(&mut self.loading_buf);

            // Compute EXIF and histogram from decoded data
            let mut exif = Vec::with_capacity(decoded.len());
            let mut histogram = Vec::with_capacity(decoded.len());

            let images: Vec<ImageInfo> = decoded
                .into_iter()
                .map(|d| {
                    exif.push(core::image::extract_exif(&d.raw_bytes));
                    histogram.push(core::image::compute_y_histogram(&d.rgba));
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
                        rgba: d.rgba,
                        path: d.path,
                    }
                })
                .collect();

            if self.loading_append {
                self.state.append_images(images);
                self.state.exif.extend(exif);
                self.state.histogram.extend(histogram);
            } else {
                self.state.set_images(images);
                self.state.exif = exif;
                self.state.histogram = histogram;
            }

            self.load_rx = None;
            self.loading_total = 0;
            self.loading_received = 0;
            ctx.request_repaint();
        }
    }
}

impl eframe::App for MmCompare {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Toggle modes: P=local, E=exif, H=histogram
        let toggle = |key| ui.input(|i| i.key_pressed(key));
        let mut changed = false;

        if toggle(egui::Key::P) {
            self.state.local_mode = !self.state.local_mode;
            changed = true;
            if !self.state.local_mode {
                self.state.selection = None;
                self.state.avg_y.fill(None);
            }
        }
        if toggle(egui::Key::E) {
            self.state.show_exif = !self.state.show_exif;
            changed = true;
        }
        if toggle(egui::Key::H) {
            self.state.show_histogram = !self.state.show_histogram;
            changed = true;
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

        if !self.is_loading() {
            self.poll_drops(ui.ctx());
        }

        self.poll_loading(ui.ctx());

        egui::CentralPanel::default().show(ui, |ui| {
            ui::viewer::image_grid(
                ui,
                &mut self.state,
                self.loading_total - self.loading_received,
            );
        });
    }
}
