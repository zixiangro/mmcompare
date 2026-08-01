use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{AppState, CellKind, ImageInfo, MAX_IMAGES};
use crate::ui;

/// 解码线程的单张图片载荷：解码结果 + EXIF 摘要 + Y 直方图。
/// 三者都在子线程生成，主线程只负责纹理上传。
type LoadResult = Result<(core::image::DecodedImage, String, [u32; 256]), PathBuf>;

pub struct MmCompare {
    state: AppState,
    load_rx: Option<mpsc::Receiver<(usize, LoadResult)>>,
    loading_total: usize,
    loading_received: usize,
    /// 按加载顺序索引；每张到达即上传纹理，收齐后按序写入 state。
    loading_buf: Vec<Option<ImageInfo>>,
    /// 加载期间拖拽进来的路径，等本次加载完成后处理，避免丢失。
    pending_drops: Vec<PathBuf>,
}

impl Default for MmCompare {
    fn default() -> Self {
        Self {
            state: AppState::new(),
            load_rx: None,
            loading_total: 0,
            loading_received: 0,
            loading_buf: Vec::new(),
            pending_drops: Vec::new(),
        }
    }
}

fn is_image_ext(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp"
        )
    })
}

fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        na.to_lowercase().cmp(&nb.to_lowercase())
    });
}

impl MmCompare {
    fn is_loading(&self) -> bool {
        self.load_rx.is_some()
    }

    /// 过滤非图片 / 已加载路径，按文件名排序，截断到剩余名额。
    /// 入选路径立即标记为已加载，避免同一批拖拽内重复入队。
    fn filter_paths(&mut self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let remaining = MAX_IMAGES
            .saturating_sub(self.state.cell_order.len())
            .saturating_sub(self.loading_total);
        let mut paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| is_image_ext(p) && !self.state.loaded_paths.contains(p))
            .take(remaining)
            .collect();
        sort_paths(&mut paths);
        for p in &paths {
            self.state.loaded_paths.insert(p.clone());
        }
        paths
    }

    pub fn load_startup_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let paths = self.filter_paths(paths);
        if paths.is_empty() {
            return;
        }
        self.spawn_loaders(paths, ctx);
    }

    fn poll_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let paths = dropped.into_iter().filter_map(|f| f.path).collect();
        let paths = self.filter_paths(paths);
        if paths.is_empty() {
            return;
        }
        if self.is_loading() {
            self.pending_drops.extend(paths);
        } else {
            self.spawn_loaders(paths, ctx);
        }
    }

    fn drain_pending_drops(&mut self, ctx: &egui::Context) {
        if self.is_loading() || self.pending_drops.is_empty() {
            return;
        }
        let remaining = MAX_IMAGES
            .saturating_sub(self.state.cell_order.len())
            .saturating_sub(self.loading_total);
        let mut paths = std::mem::take(&mut self.pending_drops);
        paths.truncate(remaining);
        if !paths.is_empty() {
            self.spawn_loaders(paths, ctx);
        }
    }

    fn spawn_loaders(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        self.loading_total = paths.len();
        self.loading_received = 0;
        self.loading_buf = (0..paths.len()).map(|_| None).collect();
        self.state.load_errors.clear();
        let (tx, rx) = mpsc::channel();

        for (i, p) in paths.into_iter().enumerate() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                // 读文件 / 解码 / EXIF / 直方图全部是纯 CPU 计算，放在子线程。
                let result: LoadResult = (|| {
                    let bytes = std::fs::read(&p).map_err(|e| {
                        log::warn!("read failed {}: {}", p.display(), e);
                        p.clone()
                    })?;
                    let mut img = core::image::decode_image_bytes(&bytes).ok_or_else(|| {
                        log::warn!("decode failed {}", p.display());
                        p.clone()
                    })?;
                    img.path = p.clone();
                    let exif = core::image::extract_exif(&bytes);
                    let histogram = core::image::compute_y_histogram(&img.rgba);
                    Ok((img, exif, histogram))
                })();
                tx.send((i, result)).ok();
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

        while let Ok((i, result)) = rx.try_recv() {
            match result {
                Ok((img, exif, histogram)) => {
                    // 逐张上传纹理，避免收齐后一帧内集中上传造成卡顿。
                    let name = img
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("image");
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(img.size, &img.rgba);
                    let texture =
                        ctx.load_texture(name, color_image, egui::TextureOptions::default());
                    self.loading_buf[i] = Some(ImageInfo {
                        texture,
                        size: img.size,
                        rgba: img.rgba,
                        path: img.path,
                        exif,
                        histogram,
                    });
                }
                Err(path) => {
                    // 从已加载集合移除，允许用户重新拖入重试。
                    self.state.loaded_paths.remove(&path);
                    self.state.load_errors.push(path);
                }
            }
            self.loading_received += 1;
        }

        if self.loading_received >= self.loading_total {
            let buf = std::mem::take(&mut self.loading_buf);
            let infos: Vec<ImageInfo> = buf.into_iter().flatten().collect();
            self.state.append_standalone_images(infos);

            self.load_rx = None;
            self.loading_total = 0;
            self.loading_received = 0;
            ctx.request_repaint();
        }
    }

    fn rotate_image_cell(&mut self, img_idx: usize, ctx: &egui::Context) {
        let cell = &mut self.state.image_cells[img_idx];
        let (new_rgba, new_size) =
            core::image::rotate_rgba_90_cw(&cell.info.rgba, cell.info.size[0], cell.info.size[1]);
        cell.info.rgba = new_rgba;
        cell.info.size = new_size;
        cell.info.histogram = core::image::compute_y_histogram(&cell.info.rgba);

        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [cell.info.size[0], cell.info.size[1]],
            &cell.info.rgba,
        );
        cell.info.texture = ctx.load_texture(
            cell.info
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image"),
            color_image,
            egui::TextureOptions::default(),
        );

        if self.state.local_mode {
            for img in &mut self.state.image_cells {
                img.selection = None;
                img.avg_stats = None;
            }
        } else {
            cell.avg_stats = None;
            cell.selection = None;
        }
    }
}

impl eframe::App for MmCompare {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut changed = false;
        let all_images = self.state.is_all_images();

        if all_images {
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
            // 一次 input 读完全部快捷键。
            let (p, e, h, nums) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::P),
                    i.key_pressed(egui::Key::E),
                    i.key_pressed(egui::Key::H),
                    num_keys
                        .iter()
                        .enumerate()
                        .filter(|(_, k)| i.key_pressed(**k))
                        .map(|(idx, _)| idx)
                        .collect::<Vec<usize>>(),
                )
            });

            if p {
                self.state.local_mode = !self.state.local_mode;
                changed = true;
                if !self.state.local_mode {
                    for img in &mut self.state.image_cells {
                        img.selection = None;
                        img.avg_stats = None;
                    }
                }
            }
            if e {
                self.state.show_exif = !self.state.show_exif;
                changed = true;
            }
            if h {
                self.state.show_histogram = !self.state.show_histogram;
                changed = true;
            }
            for idx in nums {
                if idx < self.state.cell_order.len() {
                    let CellKind::Image(img_idx) = self.state.cell_order[idx];
                    self.rotate_image_cell(img_idx, ui.ctx());
                }
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

        // 拖拽随时接收；加载期间先缓存，完成后统一处理。
        self.poll_drops(ui.ctx());
        self.poll_loading(ui.ctx());
        self.drain_pending_drops(ui.ctx());

        egui::CentralPanel::default().show(ui, |ui| {
            ui::viewer::image_grid(
                ui,
                &mut self.state,
                self.loading_total - self.loading_received,
            );
        });
    }
}
