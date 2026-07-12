use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{AppState, ImageInfo};
use crate::ui;

pub struct MmCompare {
    state: AppState,
    load_rx: Option<mpsc::Receiver<(usize, Option<core::image::DecodedImage>)>>,
    loading_total: usize,
    loading_received: usize,
    loading_buf: Vec<(usize, core::image::DecodedImage)>,
}

impl Default for MmCompare {
    fn default() -> Self {
        Self {
            state: AppState::new(),
            load_rx: None,
            loading_total: 0,
            loading_received: 0,
            loading_buf: Vec::new(),
        }
    }
}

impl MmCompare {
    fn is_loading(&self) -> bool {
        self.load_rx.is_some()
    }

    pub fn load_startup_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let remaining = 8usize.saturating_sub(self.state.images.len());
        let mut paths: Vec<_> = paths
            .into_iter()
            .filter(|p| {
                !self.state.loaded_paths.contains(p)
                    && p.extension()
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

        if paths.is_empty() {
            return;
        }

        paths.sort_by(|a, b| {
            let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            na.to_lowercase().cmp(&nb.to_lowercase())
        });

        for p in &paths {
            self.state.loaded_paths.insert(p.clone());
        }

        self.spawn_loaders(paths, ctx);
    }

    fn poll_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let remaining = 8usize
            .saturating_sub(self.state.images.len())
            .saturating_sub(self.loading_total);

        let mut paths: Vec<PathBuf> = dropped
            .into_iter()
            .filter_map(|f| f.path)
            .filter(|p| {
                !self.state.loaded_paths.contains(p)
                    && p.extension()
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

        if paths.is_empty() {
            return;
        }

        paths.sort_by(|a, b| {
            let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            na.to_lowercase().cmp(&nb.to_lowercase())
        });

        for p in &paths {
            self.state.loaded_paths.insert(p.clone());
        }

        self.spawn_loaders(paths, ctx);
    }

    fn spawn_loaders(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        self.loading_total = paths.len();
        self.loading_received = 0;
        self.loading_buf.clear();
        let (tx, rx) = mpsc::channel();

        for (i, p) in paths.into_iter().enumerate() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let decoded = (|| {
                    let bytes = std::fs::read(&p).ok()?;
                    let mut img = core::image::decode_image_bytes(&bytes)?;
                    img.path = p;
                    img.raw_bytes = bytes;
                    Some(img)
                })();
                tx.send((i, decoded)).ok();
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

        while let Ok((i, decoded)) = rx.try_recv() {
            if let Some(img) = decoded {
                self.loading_buf.push((i, img));
            }
            self.loading_received += 1;
        }

        if self.loading_received >= self.loading_total {
            let mut decoded = std::mem::take(&mut self.loading_buf);
            decoded.sort_by_key(|(i, _)| *i);
            let decoded: Vec<_> = decoded.into_iter().map(|(_, d)| d).collect();

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

            self.state.append_images(images);
            self.state.exif.extend(exif);
            self.state.histogram.extend(histogram);

            self.load_rx = None;
            self.loading_total = 0;
            self.loading_received = 0;
            ctx.request_repaint();
        }
    }

    fn rotate_image(&mut self, idx: usize, ctx: &egui::Context) {
        let img = &mut self.state.images[idx];
        let (new_rgba, new_size) =
            core::image::rotate_rgba_90_cw(&img.rgba, img.size[0], img.size[1]);
        img.rgba = new_rgba;
        img.size = new_size;

        let color_image =
            egui::ColorImage::from_rgba_unmultiplied([img.size[0], img.size[1]], &img.rgba);
        img.texture = ctx.load_texture(
            img.path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image"),
            color_image,
            egui::TextureOptions::default(),
        );

        if idx < self.state.histogram.len() {
            self.state.histogram[idx] = core::image::compute_y_histogram(&img.rgba);
        }

        if self.state.local_mode {
            self.state.selection.fill(None);
            self.state.avg_stats.fill(None);
        } else {
            self.state.avg_stats[idx] = None;
            self.state.selection[idx] = None;
        }
    }
}

impl eframe::App for MmCompare {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let toggle = |key| ui.input(|i| i.key_pressed(key));
        let mut changed = false;

        if toggle(egui::Key::P) {
            self.state.local_mode = !self.state.local_mode;
            changed = true;
            if !self.state.local_mode {
                self.state.selection.fill(None);
                self.state.avg_stats.fill(None);
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
        for (idx, &key) in num_keys.iter().enumerate() {
            if ui.input(|i| i.key_pressed(key)) && idx < self.state.images.len() {
                self.rotate_image(idx, ui.ctx());
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
