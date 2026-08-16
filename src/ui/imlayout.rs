//! 统筹层：加载管线、键盘事件、窗口标题、网格布局与交互编排。
//!
//! 本模块是应用主体（`MmCompare`），管理**所有** cell 的编排：布局怎么排、
//! 交互怎么响应、图片怎么加载，都在这里；文件夹 cell 的扫描/缩略图/
//! 打开/导航与渲染收在 `ui/folder`（`FolderManager`），imlayout 只做
//! 编排调用与键盘分发。`imcell` 负责图片 cell 的绘制。
//!
//! 这是全项目**仅有的两个**允许出现线程原语的地方之一（ADR-0001）：
//! 本模块的解码线程（standalone 图片）与 `folder.rs` 的扫描/加载线程，
//! 其余模块永远运行在主线程，不需要考虑线程安全。
//!
//! 加载是一个"批次"状态机：`load_rx` 有值 = 正在加载；
//! `loading_total` / `loading_received` 相等即批次完成；
//! `loading_buf` 按加载顺序占位，批次完成时按序追加到 state。
//! 加载期间收到的拖拽缓存到 `pending_drops`，完成后按序处理。
//!
//! 布局采用完全手动坐标（ADR-0002）。交互状态变更集中在帧末统一应用
//! （`PanFeedback` 模式），避免渲染中途改状态。
//!
//! 完整流程见 docs/loading.md 与 docs/layout.md。

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{
    AppState, CellKind, FolderAction, ImageInfo, ImageSource, MAX_IMAGES, MAX_VIDEOS, VideoCell,
};

use super::{folder, imcell, video};

const SEP: f32 = 1.0;
const MARGIN: f32 = 6.0;
/// 视频解码最长边（M2 降采样上限，控制帧内存与传输；M5 再按需调整）。
const MAX_VIDEO_DIM: u32 = 1280;

/// 视频解码线程 → 主线程的消息（按 path 关联 cell，防下标漂移）。
enum VideoMsg {
    Frame {
        path: PathBuf,
        pts: f64,
        rgb: Vec<u8>,
        width: u32,
        height: u32,
    },
    Done {
        path: PathBuf,
    },
    Error {
        path: PathBuf,
    },
}

/// 一个活跃的视频解码会话（播放流或单帧 seek）。
struct VideoSession {
    path: PathBuf,
    rx: mpsc::Receiver<VideoMsg>,
}

type LoadResult = Result<(core::image::DecodedImage, String, [u32; 256]), PathBuf>;
type VideoLoadResult = Result<(core::video::VideoInfo, Vec<u8>), PathBuf>;

pub struct MmCompare {
    state: AppState,
    load_rx: Option<mpsc::Receiver<(usize, LoadResult)>>,
    loading_total: usize,
    loading_received: usize,
    loading_buf: Vec<Option<ImageInfo>>,
    pending_drops: Vec<PathBuf>,
    video_load_rx: Option<mpsc::Receiver<(usize, PathBuf, VideoLoadResult)>>,
    video_loading_total: usize,
    video_loading_received: usize,
    video_loading_buf: Vec<Option<VideoCell>>,
    video_sessions: Vec<VideoSession>,
    folder: folder::FolderManager,
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
            video_load_rx: None,
            video_loading_total: 0,
            video_loading_received: 0,
            video_loading_buf: Vec::new(),
            video_sessions: Vec::new(),
            folder: folder::FolderManager::default(),
        }
    }
}

impl MmCompare {
    /// 图片文件加载管线是否忙（文件夹的扫描/缩略图是独立管线，不影响文件加载）。
    fn is_busy(&self) -> bool {
        self.load_rx.is_some()
    }

    /// 把输入路径分类为图片文件、文件夹与视频文件（去重、排序、截断名额），
    /// 文件路径立即标记进 `loaded_paths`，避免同批重复入队。
    ///
    /// **模式互斥**：视频批（含视频文件）优先——进入/保持视频模式并清空图片与
    /// 文件夹（M2 简化切换，M3 改为保留状态）；图片/文件夹批则清空视频回图片模式。
    /// 文件夹模式（已有目录/扫描队列）拒绝文件，文件模式拒绝目录；同批混合时目录优先。
    fn classify_paths(
        &mut self,
        paths: Vec<PathBuf>,
    ) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
        let batch_has_video = paths.iter().any(|p| !p.is_dir() && folder::is_video_ext(p));
        let in_video_mode = !self.state.video_cells.is_empty() || self.video_loading_total > 0;

        if batch_has_video {
            if !in_video_mode {
                self.state.clear_images_and_folders();
                self.load_rx = None;
                self.loading_total = 0;
                self.loading_received = 0;
                self.loading_buf.clear();
                self.pending_drops.clear();
                self.folder = folder::FolderManager::default();
            }
            let remaining = MAX_VIDEOS
                .saturating_sub(self.state.video_cells.len())
                .saturating_sub(self.video_loading_total);
            let mut video_paths = Vec::new();
            for p in paths {
                if video_paths.len() >= remaining {
                    break;
                }
                if !p.is_dir() && folder::is_video_ext(&p) && !self.state.loaded_paths.contains(&p)
                {
                    video_paths.push(p);
                }
            }
            folder::sort_paths(&mut video_paths);
            for p in &video_paths {
                self.state.loaded_paths.insert(p.clone());
            }
            return (Vec::new(), Vec::new(), video_paths);
        }

        if in_video_mode {
            self.state.clear_videos();
            self.video_sessions.clear();
            self.video_load_rx = None;
            self.video_loading_total = 0;
            self.video_loading_received = 0;
            self.video_loading_buf.clear();
        }

        let remaining = MAX_IMAGES
            .saturating_sub(self.state.cell_order.len())
            .saturating_sub(self.loading_total);
        let folder_mode = !self.state.folder_cells.is_empty() || self.folder.has_pending();
        let file_mode =
            !folder_mode && (!self.state.image_cells.is_empty() || self.loading_total > 0);
        let batch_has_dir = paths.iter().any(|p| p.is_dir());
        let accept_dir = !file_mode;
        let accept_file = !folder_mode && (!batch_has_dir || file_mode);
        let mut file_paths = Vec::new();
        let mut folder_paths = Vec::new();
        for p in paths.into_iter().take(remaining) {
            if p.is_dir() {
                if accept_dir && !self.state.folder_cells.iter().any(|fc| fc.dir_path == p) {
                    folder_paths.push(p);
                }
            } else if accept_file
                && folder::is_image_ext(&p)
                && !self.state.loaded_paths.contains(&p)
            {
                file_paths.push(p);
            }
        }
        folder::sort_paths(&mut file_paths);
        // 同一批次（一次拖入/一次命令行）的文件夹按名字排序，保证对比时
        // 两栏同索引条目对应；依次拖入（每批单个）不受影响，保持拖入顺序。
        folder::sort_paths(&mut folder_paths);
        for p in &file_paths {
            self.state.loaded_paths.insert(p.clone());
        }
        (file_paths, folder_paths, Vec::new())
    }

    pub fn load_startup_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let (file_paths, folder_paths, video_paths) = self.classify_paths(paths);
        if !video_paths.is_empty() {
            self.spawn_video_loaders(video_paths, ctx);
        }
        if !file_paths.is_empty() {
            self.spawn_loaders(file_paths, ctx);
        }
        if !folder_paths.is_empty() {
            self.folder.queue_scan(folder_paths);
        }
    }

    /// 读取本帧的拖拽事件（egui 的 `dropped_files` 只保留一帧，读走即失）。
    ///
    /// 目录直接交给 folder 的扫描队列（立即接受，不受图片加载进度影响）；
    /// 图片文件在加载中时缓存到 `pending_drops`，空闲后按序启动。
    fn poll_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let paths = dropped.into_iter().filter_map(|f| f.path).collect();
        let (file_paths, folder_paths, video_paths) = self.classify_paths(paths);
        if !video_paths.is_empty() && self.video_loading_total > 0 {
            self.pending_drops.extend(video_paths);
        } else if !video_paths.is_empty() {
            self.spawn_video_loaders(video_paths, ctx);
        }
        if !file_paths.is_empty() && self.is_busy() {
            self.pending_drops.extend(file_paths);
        } else if !file_paths.is_empty() {
            self.spawn_loaders(file_paths, ctx);
        }
        if !folder_paths.is_empty() {
            self.folder.queue_scan(folder_paths);
        }
    }

    /// 加载完成后，把缓存中的文件启动为新一批（重新分类：
    /// 缓存期间格子可能已被删除/加满，名额变了）。
    fn drain_pending_drops(&mut self, ctx: &egui::Context) {
        if self.is_busy() || self.video_loading_total > 0 || self.pending_drops.is_empty() {
            return;
        }
        let paths = std::mem::take(&mut self.pending_drops);
        let (file_paths, _folder_paths, video_paths) = self.classify_paths(paths);
        if !video_paths.is_empty() {
            self.spawn_video_loaders(video_paths, ctx);
        }
        if !file_paths.is_empty() {
            self.spawn_loaders(file_paths, ctx);
        }
    }

    /// 启动一批 standalone 图片加载：每张图一个临时线程，
    /// 线程内读文件 → 解码 → EXIF → 直方图（纯 CPU，无共享状态）。
    fn spawn_loaders(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        self.loading_total = paths.len();
        self.loading_received = 0;
        self.loading_buf = (0..paths.len()).map(|_| None).collect();
        self.state.load_errors.clear();
        let (tx, rx) = mpsc::channel();

        for (i, p) in paths.into_iter().enumerate() {
            let tx = tx.clone();
            std::thread::spawn(move || {
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

    /// 每帧把已完成的解码结果搬进 state，收齐后按序追加。
    fn poll_loading(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.load_rx else {
            return;
        };

        while let Ok((i, result)) = rx.try_recv() {
            match result {
                Ok((img, exif, histogram)) => {
                    let name = img
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("image");
                    let texture = imcell::upload_texture(ctx, &img.rgba, img.size, name);
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

    /// 启动一批视频首帧加载：每视频一个线程，`read_info` + 首帧提取（降采样）。
    fn spawn_video_loaders(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        self.video_loading_total = paths.len();
        self.video_loading_received = 0;
        self.video_loading_buf = (0..paths.len()).map(|_| None).collect();
        self.state.load_errors.clear();
        let (tx, rx) = mpsc::channel();

        for (i, p) in paths.into_iter().enumerate() {
            let tx = tx.clone();
            let wpath = p.clone();
            std::thread::spawn(move || {
                let result: VideoLoadResult = (|| {
                    let (info, rgb) =
                        core::video::first_frame(&wpath, MAX_VIDEO_DIM).map_err(|e| {
                            log::warn!("video first frame failed {}: {}", wpath.display(), e);
                            wpath.clone()
                        })?;
                    Ok((info, rgb))
                })();
                tx.send((i, p, result)).ok();
            });
        }
        drop(tx);

        self.video_load_rx = Some(rx);
        ctx.request_repaint();
    }

    /// 每帧把已完成的首帧解码搬进 state，收齐后按序追加视频 cell。
    fn poll_video_loading(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.video_load_rx else {
            return;
        };

        while let Ok((i, path, result)) = rx.try_recv() {
            match result {
                Ok((info, rgb)) => {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("video");
                    let texture = imcell::upload_rgb_texture(
                        ctx,
                        &rgb,
                        [info.width as usize, info.height as usize],
                        name,
                    );
                    self.video_loading_buf[i] = Some(VideoCell {
                        path,
                        info,
                        texture: Some(texture),
                        playing: false,
                        position_secs: 0.0,
                        frame_pts: 0.0,
                        failed: false,
                    });
                }
                Err(path) => {
                    self.state.loaded_paths.remove(&path);
                    self.state.load_errors.push(path);
                }
            }
            self.video_loading_received += 1;
        }

        if self.video_loading_received >= self.video_loading_total {
            let buf = std::mem::take(&mut self.video_loading_buf);
            let cells: Vec<VideoCell> = buf.into_iter().flatten().collect();
            self.state.append_videos(cells);

            self.video_load_rx = None;
            self.video_loading_total = 0;
            self.video_loading_received = 0;
            ctx.request_repaint();
        }
    }

    /// 启动一个视频解码会话：`continuous=false` 解码单帧（暂停时 seek/步进，
    /// 精确到目标 pts），`continuous=true` 按帧率持续发帧（播放）。
    /// 帧通道 rx 被主线程丢弃时线程退出。
    fn spawn_video_worker(&mut self, cell_idx: usize, from_secs: f64, continuous: bool) {
        let path = self.state.video_cells[cell_idx].path.clone();
        self.video_sessions.retain(|s| s.path != path);
        let (tx, rx) = mpsc::channel();
        let wpath = path.clone();
        std::thread::spawn(move || {
            let mut dec = match core::video::VideoDecoder::open(&wpath, MAX_VIDEO_DIM) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("video open failed {}: {}", wpath.display(), e);
                    let _ = tx.send(VideoMsg::Error { path: wpath });
                    return;
                }
            };
            let send_frame = |tx: &mpsc::Sender<VideoMsg>, frame: core::video::DecodedFrame| {
                tx.send(VideoMsg::Frame {
                    path: wpath.clone(),
                    pts: frame.pts_secs,
                    rgb: frame.rgb,
                    width: frame.width,
                    height: frame.height,
                })
            };
            let result = if continuous {
                let frame_dur = dec.frame_duration();
                dec.play(from_secs, |frame| {
                    if send_frame(&tx, frame).is_err() {
                        return false;
                    }
                    std::thread::sleep(std::time::Duration::from_secs_f64(frame_dur));
                    true
                })
            } else {
                match dec.seek_frame(from_secs) {
                    Ok(Some(frame)) => {
                        let _ = send_frame(&tx, frame);
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                }
            };
            if let Err(e) = result {
                log::warn!("video play failed {}: {}", wpath.display(), e);
                let _ = tx.send(VideoMsg::Error { path: wpath });
            } else {
                let _ = tx.send(VideoMsg::Done { path: wpath });
            }
        });
        self.video_sessions.push(VideoSession { path, rx });
    }

    fn video_drop_session(&mut self, cell_idx: usize) {
        let Some(path) = self.state.video_cells.get(cell_idx).map(|c| c.path.clone()) else {
            return;
        };
        self.video_sessions.retain(|s| s.path != path);
    }

    fn video_seek(&mut self, cell_idx: usize, secs: f64) {
        let pos = {
            let cell = &mut self.state.video_cells[cell_idx];
            cell.position_secs = secs.clamp(0.0, cell.info.duration_secs);
            cell.position_secs
        };
        self.spawn_video_worker(cell_idx, pos, false);
    }

    fn video_toggle_play(&mut self, cell_idx: usize) {
        let (playing, pos) = {
            let cell = &mut self.state.video_cells[cell_idx];
            let dur = cell.info.duration_secs;
            cell.playing = !cell.playing;
            if cell.playing && cell.position_secs >= dur - 0.05 {
                cell.position_secs = 0.0; // 播完再播：从头开始
            }
            (cell.playing, cell.position_secs)
        };
        if playing {
            self.spawn_video_worker(cell_idx, pos, true);
        } else {
            self.video_drop_session(cell_idx);
        }
    }

    /// 每帧搬视频解码帧进 state；会话结束（worker 退出）时移除。
    fn poll_video(&mut self, ctx: &egui::Context) {
        let mut keep: Vec<VideoSession> = Vec::new();
        for session in std::mem::take(&mut self.video_sessions) {
            let mut dead = false;
            loop {
                match session.rx.try_recv() {
                    Ok(VideoMsg::Frame {
                        path,
                        pts,
                        rgb,
                        width,
                        height,
                    }) => {
                        if let Some(idx) =
                            self.state.video_cells.iter().position(|c| c.path == path)
                        {
                            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("video");
                            let cell = &mut self.state.video_cells[idx];
                            cell.texture = Some(imcell::upload_rgb_texture(
                                ctx,
                                &rgb,
                                [width as usize, height as usize],
                                name,
                            ));
                            cell.frame_pts = pts;
                            if cell.playing {
                                cell.position_secs = pts;
                            }
                            cell.failed = false;
                        }
                    }
                    Ok(VideoMsg::Done { path }) => {
                        if let Some(idx) =
                            self.state.video_cells.iter().position(|c| c.path == path)
                        {
                            let cell = &mut self.state.video_cells[idx];
                            if cell.playing {
                                cell.playing = false;
                                cell.position_secs = cell.frame_pts;
                            }
                        }
                    }
                    Ok(VideoMsg::Error { path }) => {
                        if let Some(idx) =
                            self.state.video_cells.iter().position(|c| c.path == path)
                        {
                            let cell = &mut self.state.video_cells[idx];
                            cell.playing = false;
                            cell.failed = true;
                        }
                        self.state.load_errors.push(path);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                }
            }
            // 只有 worker 已退出（通道断开）才移除会话；Empty 时 worker 仍在跑
            if !dead {
                keep.push(session);
            }
        }
        self.video_sessions = keep;
        if !self.video_sessions.is_empty() {
            ctx.request_repaint();
        }
    }

    fn rotate_image_cell(&mut self, img_idx: usize, ctx: &egui::Context) {
        let new_info = {
            let info = &self.state.image_cells[img_idx].info;
            imcell::rotate_image(info, ctx)
        };
        self.state.image_cells[img_idx].info = new_info;
        self.state.invalidate_selection_after_rotation(img_idx);
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
                if idx < self.state.cell_order.len()
                    && let CellKind::Image(img_idx) = self.state.cell_order[idx]
                {
                    self.rotate_image_cell(img_idx, ui.ctx());
                }
            }
        }

        let (space, b_key, esc) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::B),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if space || b_key || esc {
            if esc {
                let mut to_close: Vec<usize> = (0..self.state.cell_order.len())
                    .filter(|pos| {
                        matches!(
                            self.state.cell_order[*pos],
                            CellKind::Image(img_idx)
                                if matches!(
                                    self.state.image_cells[img_idx].source,
                                    ImageSource::FromFolder { .. }
                                )
                        )
                    })
                    .collect();
                to_close.sort_unstable();
                for pos in to_close.into_iter().rev() {
                    self.state.close_folder_at_pos(pos);
                }
                changed = true;
            } else if !self.is_busy() {
                // 导航条件：每个文件夹打开 ≤1 张（单文件夹 1 张 / 多文件夹对比
                // 对都响应，同步推进每个文件夹的索引；单文件夹多图不响应）。
                let delta: i32 = if space { 1 } else { -1 };
                let targets = self.state.folder_nav_targets(delta);
                if !targets.is_empty() && self.state.folder_nav_allowed() {
                    self.folder.navigate(&mut self.state, ui.ctx(), targets);
                } else if space {
                    // 空格：打开所有文件夹的选中条目（双栏联动选中后一次开两张）
                    self.folder.open_selected_all(&mut self.state, ui.ctx());
                }
            }
        }

        if changed {
            let mut flags = String::new();
            if self.state.is_all_videos() {
                flags.push('V');
            }
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

        self.poll_drops(ui.ctx());
        self.poll_loading(ui.ctx());
        self.poll_video_loading(ui.ctx());
        self.poll_video(ui.ctx());
        self.folder.poll_scan(&mut self.state);
        self.folder.poll_loading(&mut self.state, ui.ctx());
        self.folder.poll_thumbnails(&mut self.state, ui.ctx());
        self.drain_pending_drops(ui.ctx());
        self.folder.drain_thumbnails(&mut self.state, ui.ctx());

        egui::CentralPanel::default().show(ui, |ui| {
            let (actions, video_actions) = image_grid(
                ui,
                &mut self.state,
                self.loading_total - self.loading_received,
                self.video_loading_total - self.video_loading_received,
            );
            for action in actions {
                self.folder.handle_action(&mut self.state, action, ui.ctx());
            }
            for (cell_pos, action) in video_actions {
                let Some(&CellKind::Video(video_idx)) = self.state.cell_order.get(cell_pos) else {
                    continue;
                };
                match action {
                    video::VideoAction::TogglePlay => self.video_toggle_play(video_idx),
                    video::VideoAction::Seek(secs) => self.video_seek(video_idx, secs),
                    video::VideoAction::None => {}
                }
            }
        });
    }
}

struct CellSnapshot {
    pos: usize,
    cell_rect: egui::Rect,
    img_size: [usize; 2],
}

struct GridLayout {
    row_layout: Vec<usize>,
    grid: egui::Rect,
    cell_w: f32,
    row_h: f32,
    inter: f32,
}

struct PanFeedback {
    left_dragged: bool,
    drag_delta_acc: [f32; 2],
    snapshots: Vec<CellSnapshot>,
}

pub fn image_grid(
    ui: &mut egui::Ui,
    state: &mut AppState,
    loading_count: usize,
    video_loading_count: usize,
) -> (Vec<FolderAction>, Vec<(usize, video::VideoAction)>) {
    if loading_count > 0 || video_loading_count > 0 {
        ui.ctx().request_repaint();
    }

    if state.cell_order.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            if loading_count > 0 {
                ui.label(format!("Loading {} image(s)...", loading_count));
            } else if video_loading_count > 0 {
                ui.label(format!("Loading {} video(s)...", video_loading_count));
            } else {
                ui.label(egui::RichText::new("MMCompare").size(24.0).strong());
                ui.add_space(12.0);
                ui.label(format!(
                    "Drag images or folders here to view  (max {})",
                    MAX_IMAGES
                ));
                ui.add_space(6.0);
                ui.label("P: Local mode   E: EXIF   H: Histogram");
                ui.label(format!(
                    "1-{}: Rotate   Q: Compare   Ctrl+RMB: Remove   Ctrl+Drag: Reorder",
                    MAX_IMAGES
                ));
                ui.add_space(12.0);
                ui.label(format!(
                    "Videos: drag in (max {}), Space: play/pause, arrows: seek/step, Ctrl+arrows: single",
                    MAX_VIDEOS
                ));
                ui.add_space(12.0);
                ui.hyperlink_to("Project Homepage", "https://github.com/zixiangro/mmcompare");
            }
        });
        return (Vec::new(), Vec::new());
    }

    let n = state.cell_order.len();
    let avail = ui.available_size();
    let sep_color = egui::Color32::from_gray(200);

    let row_layout = match n {
        1..=3 => vec![n],
        4 => vec![2, 2],
        _ => vec![n.div_ceil(2), n / 2],
    };

    let max_cols = *row_layout.iter().max().unwrap_or(&1) as f32;
    let rows = row_layout.len() as f32;
    let inter = MARGIN + SEP + MARGIN;
    let row_h = (avail.y - (rows - 1.0) * SEP) / rows;
    let cell_w = (avail.x - (max_cols - 1.0) * inter) / max_cols;

    let total_h = rows * row_h + (rows - 1.0) * SEP;
    let (_, grid_resp) = ui.allocate_exact_size(egui::vec2(avail.x, total_h), egui::Sense::hover());
    let layout = GridLayout {
        row_layout,
        grid: grid_resp.rect,
        cell_w,
        row_h,
        inter,
    };

    let ctrl = ui.input(|i| i.modifiers.ctrl);
    let all_images = state.is_all_images();
    let mut feedback = PanFeedback {
        left_dragged: false,
        drag_delta_acc: [0.0, 0.0],
        snapshots: Vec::with_capacity(n),
    };
    let mut folder_actions: Vec<FolderAction> = Vec::new();
    let mut video_actions: Vec<(usize, video::VideoAction)> = Vec::new();

    let mut offset = 0;
    for (row_idx, &col_count) in layout.row_layout.iter().enumerate() {
        let row_top = layout.grid.top() + row_idx as f32 * (layout.row_h + SEP);

        if row_idx > 0 {
            let sr = egui::Rect::from_min_size(
                egui::pos2(layout.grid.left(), row_top - SEP),
                egui::vec2(avail.x, SEP),
            );
            ui.painter().rect_filled(sr, 0.0, sep_color);
            ui.allocate_rect(sr, egui::Sense::hover());
        }

        let row_content = col_count as f32 * layout.cell_w + (col_count - 1) as f32 * layout.inter;
        let mut x = layout.grid.left() + (avail.x - row_content) / 2.0;

        for i in 0..col_count {
            let cell_pos = offset + i;

            if i > 0 {
                x = paint_zone(ui, x, row_top, layout.row_h, MARGIN, None);
                x = paint_zone(ui, x, row_top, layout.row_h, SEP, Some(sep_color));
                x = paint_zone(ui, x, row_top, layout.row_h, MARGIN, None);
            }

            let cell_rect = egui::Rect::from_min_size(
                egui::pos2(x, row_top),
                egui::vec2(layout.cell_w, layout.row_h),
            );

            let Some(&cell_kind) = state.cell_order.get(cell_pos) else {
                x += layout.cell_w;
                continue;
            };

            match cell_kind {
                CellKind::Image(img_idx) => {
                    render_image_cell(
                        ui,
                        state,
                        cell_rect,
                        cell_pos,
                        img_idx,
                        ctrl,
                        all_images,
                        &layout,
                        &mut feedback,
                    );
                }
                CellKind::Folder(folder_idx) => {
                    // 对比对（2 文件夹各 1 张）禁删：防误删对比图，用 Esc 退出
                    if ctrl
                        && !state.is_compare_pair()
                        && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary))
                        && ui
                            .input(|i| i.pointer.hover_pos())
                            .is_some_and(|hp| cell_rect.contains(hp))
                    {
                        state.pending_remove.push(cell_pos);
                    }
                    let action = folder::render_folder_cell(
                        ui,
                        &mut state.folder_cells[folder_idx],
                        cell_rect,
                        folder_idx,
                    );
                    if !matches!(action, FolderAction::None) {
                        folder_actions.push(action);
                    }
                }
                CellKind::Video(video_idx) => {
                    // 视频模式禁删？否——D1：直接拖入的视频用 Ctrl+RMB 关闭
                    if ctrl
                        && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary))
                        && ui
                            .input(|i| i.pointer.hover_pos())
                            .is_some_and(|hp| cell_rect.contains(hp))
                    {
                        state.pending_remove.push(cell_pos);
                    }
                    let action =
                        video::draw_video_cell(ui, &state.video_cells[video_idx], cell_rect);
                    if !matches!(action, video::VideoAction::None) {
                        video_actions.push((cell_pos, action));
                    }
                }
            }

            x += layout.cell_w;
        }

        offset += col_count;
    }

    if all_images && feedback.left_dragged {
        state.pan[0] += feedback.drag_delta_acc[0];
        state.pan[1] += feedback.drag_delta_acc[1];
        apply_clamp_feedback(state, &feedback.snapshots);
    }

    if state.is_all_videos() {
        let (space, left, right, up, down, ctrl) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.modifiers.ctrl,
            )
        });
        if space || left || right || up || down {
            // 非 Ctrl 箭头作用于全部视频；Ctrl+箭头只作用于鼠标悬停的视频（D1）
            let hover_pos = if ctrl {
                find_cell_at(ui.input(|i| i.pointer.hover_pos()), &layout)
            } else {
                None
            };
            for (cell_pos, cell_kind) in state.cell_order.iter().copied().enumerate() {
                let CellKind::Video(video_idx) = cell_kind else {
                    continue;
                };
                if ctrl && hover_pos != Some(cell_pos) {
                    continue;
                }
                let cell = &state.video_cells[video_idx];
                if space {
                    video_actions.push((cell_pos, video::VideoAction::TogglePlay));
                }
                let step = if cell.info.frame_rate > 0.0 {
                    1.0 / cell.info.frame_rate
                } else {
                    1.0 / 30.0
                };
                if left {
                    video_actions
                        .push((cell_pos, video::VideoAction::Seek(cell.position_secs - 5.0)));
                }
                if right {
                    video_actions
                        .push((cell_pos, video::VideoAction::Seek(cell.position_secs + 5.0)));
                }
                if up {
                    video_actions.push((
                        cell_pos,
                        video::VideoAction::Seek(cell.position_secs - step),
                    ));
                }
                if down {
                    video_actions.push((
                        cell_pos,
                        video::VideoAction::Seek(cell.position_secs + step),
                    ));
                }
            }
        }
    }

    if !state.pending_remove.is_empty() {
        let mut indices: Vec<usize> = state.pending_remove.drain(..).collect();
        indices.sort_unstable();
        indices.reverse();
        for pos in indices {
            if pos < state.cell_order.len() {
                state.remove_cell(pos);
            }
        }
        if state.cell_order.is_empty() {
            state.local_mode = false;
            state.show_exif = false;
            state.show_histogram = false;
            state.zoom = 1.0;
            state.pan = [0.0, 0.0];
            state.pan_offset.clear();
            for img in &mut state.image_cells {
                img.selection = None;
                img.avg_stats = None;
            }
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title("MMCompare".to_string()));
        }
    }

    draw_status_banner(
        ui,
        layout.grid,
        loading_count,
        video_loading_count,
        &state.load_errors,
    );
    (folder_actions, video_actions)
}

fn draw_status_banner(
    ui: &mut egui::Ui,
    grid: egui::Rect,
    loading_count: usize,
    video_loading_count: usize,
    errors: &[PathBuf],
) {
    let mut lines: Vec<String> = Vec::new();
    if loading_count > 0 {
        lines.push(format!("Loading {} image(s)...", loading_count));
    }
    if video_loading_count > 0 {
        lines.push(format!("Loading {} video(s)...", video_loading_count));
    }
    for p in errors {
        lines.push(format!("Failed: {}", p.display()));
    }
    if lines.is_empty() {
        return;
    }

    let font = egui::FontId::monospace(12.0);
    let (w, h) = ui.fonts_mut(|f| {
        let mut w = 0.0f32;
        let mut h = 0.0f32;
        for line in &lines {
            let sz = f
                .layout_no_wrap(line.clone(), font.clone(), egui::Color32::WHITE)
                .size();
            w = w.max(sz.x);
            h += sz.y + 3.0;
        }
        (w, h)
    });
    let pad = 8.0;
    let rect = egui::Rect::from_center_size(
        egui::pos2(grid.center().x, grid.top() + pad + h / 2.0),
        egui::vec2(w + pad * 2.0, h + pad),
    );
    ui.painter()
        .rect_filled(rect, 4.0, egui::Color32::from_black_alpha(180));
    ui.painter().text(
        rect.min + egui::vec2(pad, pad / 2.0),
        egui::Align2::LEFT_TOP,
        lines.join("\n"),
        font,
        egui::Color32::WHITE,
    );
    ui.allocate_rect(rect, egui::Sense::hover());
}

#[allow(clippy::too_many_arguments)]
fn render_image_cell(
    ui: &mut egui::Ui,
    state: &mut AppState,
    cell_rect: egui::Rect,
    cell_pos: usize,
    img_idx: usize,
    ctrl: bool,
    all_images: bool,
    layout: &GridLayout,
    feedback: &mut PanFeedback,
) {
    let sense = if ctrl || state.local_mode || state.zoom > 1.0 {
        egui::Sense::drag()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.allocate_rect(cell_rect, sense);

    // 对比对（2 文件夹各 1 张）禁删：防误删对比图，用 Esc 退出
    if ctrl
        && !state.is_compare_pair()
        && resp.hovered()
        && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary))
    {
        state.pending_remove.push(cell_pos);
    }

    if !state.local_mode && !ctrl && resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            state.zoom = (state.zoom + scroll * 0.005).max(1.0);
        }
    }

    if !state.local_mode && !ctrl && all_images {
        if resp.dragged_by(egui::PointerButton::Primary) {
            let delta = resp.drag_delta();
            feedback.drag_delta_acc[0] += delta.x;
            feedback.drag_delta_acc[1] += delta.y;
            feedback.left_dragged = true;
        }
        if resp.dragged_by(egui::PointerButton::Secondary) {
            let delta = resp.drag_delta();
            state.pan_offset[cell_pos][0] += delta.x;
            state.pan_offset[cell_pos][1] += delta.y;
        }
        if state.zoom <= 1.0 {
            state.pan = [0.0, 0.0];
            state.pan_offset.fill([0.0, 0.0]);
        }
    }

    if ctrl && all_images {
        if resp.drag_started_by(egui::PointerButton::Primary) {
            state.reorder_src = Some(cell_pos);
        }
        if resp.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(src) = state.reorder_src.take()
            && let Some(dst) = find_cell_at(ui.input(|i| i.pointer.hover_pos()), layout)
            && src != dst
        {
            state.swap_cells(src, dst);
        }
    }

    let compare =
        all_images && state.image_cells.len() == 2 && ui.input(|i| i.key_down(egui::Key::Q));
    let draw_idx = if compare && cell_pos == 0 { 1 } else { img_idx };

    let cell_pan = [
        state.pan[0] + state.pan_offset[cell_pos][0],
        state.pan[1] + state.pan_offset[cell_pos][1],
    ];

    handle_drag(state, &resp, img_idx, cell_rect, state.zoom, cell_pan, ctrl);

    let img_size = state.image_cells[draw_idx].info.size;
    feedback.snapshots.push(CellSnapshot {
        pos: cell_pos,
        cell_rect,
        img_size,
    });

    imcell::draw_image(
        ui,
        &state.image_cells[draw_idx].info,
        cell_rect,
        state.zoom,
        cell_pan,
    );

    let cell = &state.image_cells[draw_idx];
    let label = cell
        .avg_stats
        .as_ref()
        .map(core::image::format_cell_label)
        .unwrap_or_default();
    // EXIF 摘要第一行显示图片名称，其余照旧（数据层不变，仅展示时拼接）
    let exif_display = if state.show_exif {
        let name = cell
            .info
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image");
        if cell.info.exif.is_empty() {
            name.to_string()
        } else {
            format!("{name}\n{}", cell.info.exif)
        }
    } else {
        String::new()
    };
    imcell::draw_overlay(
        ui,
        cell_rect,
        &cell.info,
        cell.selection,
        &label,
        &exif_display,
        if state.show_histogram {
            &cell.info.histogram
        } else {
            &[0; 256]
        },
        state.zoom,
        cell_pan,
        state.is_dragging(),
    );

    if ctrl && state.reorder_src.is_some() {
        let src = state.reorder_src == Some(cell_pos);
        let dst = !src
            && ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|hp| cell_rect.contains(hp));
        if src || dst {
            ui.painter().rect_filled(
                cell_rect,
                0.0,
                if src {
                    egui::Color32::from_rgba_premultiplied(0, 0, 0, 60)
                } else {
                    egui::Color32::from_rgba_premultiplied(0, 0, 0, 40)
                },
            );
        }
    }
}

fn paint_zone(
    ui: &mut egui::Ui,
    x: f32,
    row_top: f32,
    row_h: f32,
    width: f32,
    color: Option<egui::Color32>,
) -> f32 {
    let rect = egui::Rect::from_min_size(egui::pos2(x, row_top), egui::vec2(width, row_h));
    if let Some(c) = color {
        ui.painter().rect_filled(rect, 0.0, c);
    }
    ui.allocate_rect(rect, egui::Sense::hover());
    x + width
}

fn apply_clamp_feedback(state: &mut AppState, snapshots: &[CellSnapshot]) {
    if state.local_mode || state.zoom <= 1.0 {
        return;
    }
    let mut global_adj = [0.0f32, 0.0f32];
    let mut has_global = false;

    for s in snapshots {
        let raw = [
            state.pan[0] + state.pan_offset[s.pos][0],
            state.pan[1] + state.pan_offset[s.pos][1],
        ];
        let eff = imcell::clamp_pan(raw, s.cell_rect, s.img_size, state.zoom);
        let diff = [raw[0] - eff[0], raw[1] - eff[1]];
        if diff[0] == 0.0 && diff[1] == 0.0 {
            continue;
        }

        for axis in 0..2 {
            let d = diff[axis];
            if d == 0.0 {
                continue;
            }
            let off = state.pan_offset[s.pos][axis];
            if off == 0.0 {
                if d.abs() > global_adj[axis].abs() {
                    global_adj[axis] = d;
                }
                has_global = true;
            } else if off.signum() == d.signum() {
                let new_off = off - d;
                if off.signum() == new_off.signum() || new_off == 0.0 {
                    state.pan_offset[s.pos][axis] = new_off;
                } else {
                    state.pan_offset[s.pos][axis] = 0.0;
                    let rem = d - off;
                    if rem.abs() > global_adj[axis].abs() {
                        global_adj[axis] = rem;
                    }
                    has_global = true;
                }
            } else if d.abs() > global_adj[axis].abs() {
                global_adj[axis] = d;
                has_global = true;
            }
        }
    }
    if has_global {
        state.pan[0] -= global_adj[0];
        state.pan[1] -= global_adj[1];
    }
}

fn handle_drag(
    state: &mut AppState,
    resp: &egui::Response,
    img_idx: usize,
    cell_rect: egui::Rect,
    zoom: f32,
    pan: [f32; 2],
    ctrl: bool,
) {
    if !state.local_mode || ctrl {
        return;
    }
    let Some(mouse_pos) = resp.interact_pointer_pos() else {
        return;
    };

    let img_size = state.image_cells[img_idx].info.size;

    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_start_new(img_idx, norm);
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && state.is_dragging()
        && !state.is_moving_selection()
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_update(norm);
    }
    if resp.drag_started_by(egui::PointerButton::Secondary)
        && state.image_cells[img_idx].selection.is_some()
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_start_move(img_idx, norm);
    }
    if resp.dragged_by(egui::PointerButton::Secondary)
        && state.is_moving_selection()
        && let Some(norm) = imcell::mouse_to_norm(mouse_pos, cell_rect, img_size, zoom, pan)
    {
        state.drag_update(norm);
    }

    if resp.drag_stopped_by(egui::PointerButton::Primary)
        && let Some(cell) = state.drag_end()
        && cell < state.image_cells.len() // 防御：拖拽期间该图片可能已被删除
        && let Some(sel) = state.image_cells[cell].selection
    {
        for img in &mut state.image_cells {
            img.avg_stats = Some(core::image::compute_selection_stats(
                &img.info.rgba,
                img.info.size[0],
                img.info.size[1],
                &sel,
            ));
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Secondary)
        && let Some(cell) = state.drag_end()
        && cell < state.image_cells.len() // 防御：拖拽期间该图片可能已被删除
        && let Some(sel) = state.image_cells[cell].selection
    {
        let stats = core::image::compute_selection_stats(
            &state.image_cells[cell].info.rgba,
            state.image_cells[cell].info.size[0],
            state.image_cells[cell].info.size[1],
            &sel,
        );
        state.image_cells[cell].avg_stats = Some(stats);
    }
}

fn find_cell_at(pos: Option<egui::Pos2>, layout: &GridLayout) -> Option<usize> {
    let pos = pos?;
    let mut idx = 0usize;
    for (row_idx, &col_count) in layout.row_layout.iter().enumerate() {
        let row_top = layout.grid.top() + row_idx as f32 * (layout.row_h + SEP);
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(layout.grid.left(), row_top),
            egui::vec2(layout.grid.width(), layout.row_h),
        );
        if !row_rect.contains(pos) {
            idx += col_count;
            continue;
        }
        let row_content = col_count as f32 * layout.cell_w + (col_count - 1) as f32 * layout.inter;
        let mut x = row_rect.left() + (layout.grid.width() - row_content) / 2.0;
        for _ in 0..col_count {
            let cr = egui::Rect::from_min_size(
                egui::pos2(x, row_top),
                egui::vec2(layout.cell_w, layout.row_h),
            );
            if cr.contains(pos) {
                return Some(idx);
            }
            x += layout.cell_w + layout.inter;
            idx += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FolderCell, FolderView};
    use std::collections::HashSet;
    use std::fs;

    fn setup(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mmc_mode_{}_{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.png"), b"png").unwrap();
        dir
    }

    fn make_info(ctx: &egui::Context, name: &str) -> ImageInfo {
        let tex = ctx.load_texture(
            name,
            egui::ColorImage::new([4, 4], vec![egui::Color32::WHITE; 16]),
            egui::TextureOptions::default(),
        );
        ImageInfo {
            texture: tex,
            size: [4, 4],
            rgba: vec![255; 64],
            path: PathBuf::from(name),
            exif: String::new(),
            histogram: [0; 256],
        }
    }

    #[test]
    fn undecided_mode_accepts_files() {
        let tmp = setup("accepts_files");
        let file = tmp.join("a.png");
        let mut app = MmCompare::default();
        let (files, dirs, videos) = app.classify_paths(vec![file.clone()]);
        assert_eq!(files, vec![file], "未定模式接受文件");
        assert!(dirs.is_empty());
        assert!(videos.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_mode_rejects_folder() {
        let tmp = setup("rejects_folder");
        let ctx = egui::Context::default();
        let mut app = MmCompare::default();
        app.state
            .append_standalone_images(vec![make_info(&ctx, "a")]);
        let (files, dirs, _videos) = app.classify_paths(vec![tmp.join("sub")]);
        assert!(files.is_empty() && dirs.is_empty(), "文件模式拒绝目录");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn file_loading_window_rejects_folder() {
        let tmp = setup("loading_window");
        let mut app = MmCompare {
            loading_total: 1, // 模拟文件批次加载中（图片尚未入 state）
            ..Default::default()
        };
        let (files, dirs, _videos) = app.classify_paths(vec![tmp.join("sub")]);
        assert!(
            files.is_empty() && dirs.is_empty(),
            "文件加载窗口期拒绝目录"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn folder_mode_rejects_files() {
        let tmp = setup("folder_mode");
        let mut app = MmCompare::default();
        app.state.folder_cells.push(FolderCell {
            dir_path: tmp.join("sub"),
            entries: Vec::new(),
            selected: HashSet::new(),
            view_mode: FolderView::List,
            scroll_offset: 0.0,
            thumbnails: Vec::new(),
            open_entry: None,
            scroll_to: None,
        });
        app.state.cell_order.push(CellKind::Folder(0));
        let (files, dirs, _videos) = app.classify_paths(vec![tmp.join("a.png")]);
        assert!(files.is_empty() && dirs.is_empty(), "文件夹模式拒绝文件");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn folder_scan_window_rejects_files() {
        // 目录刚拖入、扫描未登记期间（窗口期）也拒绝文件
        let tmp = setup("scan_window");
        let mut app = MmCompare::default();
        app.folder.queue_scan(vec![tmp.clone()]);
        assert!(app.folder.has_pending(), "扫描窗口期");
        let (files, dirs, _videos) = app.classify_paths(vec![tmp.join("a.png")]);
        assert!(files.is_empty() && dirs.is_empty(), "扫描窗口期拒绝文件");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn mixed_batch_prefers_folder() {
        let tmp = setup("mixed_batch");
        let mut app = MmCompare::default();
        let (files, dirs, _videos) = app.classify_paths(vec![tmp.join("a.png"), tmp.join("sub")]);
        assert!(files.is_empty(), "同批混合时目录优先，文件被忽略");
        assert_eq!(dirs, vec![tmp.join("sub")]);
        let _ = fs::remove_dir_all(&tmp);
    }
    #[test]
    fn video_playback_roundtrip_no_crash() {
        // 回归：拖视频崩溃（0xc0000409 栈溢出）。模拟播放会话：播放 → poll → 暂停 → seek。
        video_playback_roundtrip_impl("tests/fixtures/sample.mp4");
    }

    #[test]
    fn video_playback_1080p_no_crash() {
        // 回归：真实分辨率（1080p）播放，覆盖解码线程栈溢出场景
        video_playback_roundtrip_impl("tests/fixtures/sample1080.mp4");
    }

    fn video_playback_roundtrip_impl(sample: &str) {
        let ctx = egui::Context::default();
        let mut app = MmCompare::default();
        let path = PathBuf::from(sample);
        app.state.append_videos(vec![VideoCell {
            path: path.clone(),
            info: crate::core::video::VideoInfo {
                width: 320,
                height: 240,
                duration_secs: 3.0,
                frame_rate: 30.0,
                rotation: 0,
            },
            texture: None,
            playing: false,
            position_secs: 0.0,
            frame_pts: 0.0,
            failed: false,
        }]);
        app.video_toggle_play(0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1200);
        while std::time::Instant::now() < deadline {
            app.poll_video(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        app.video_toggle_play(0); // 暂停
        app.video_seek(0, 1.0); // 单帧 seek
        for _ in 0..20 {
            app.poll_video(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        let cell = &app.state.video_cells[0];
        assert!(
            app.state.load_errors.is_empty(),
            "no load errors: {:?}",
            app.state.load_errors
        );
        assert!(!cell.failed, "cell should not be failed");
        assert!(cell.texture.is_some(), "播放应产生纹理");
        assert!(!cell.playing, "暂停后不再播放");
        assert!(cell.frame_pts > 0.0, "播放推进了 pts");
    }

    #[test]
    fn video_batch_enters_video_mode() {
        // 已有图片 + 拖入视频 → 图片被清空，视频被接受（M2 简化切换）
        let tmp = setup("video_enter");
        fs::write(tmp.join("a.mp4"), b"mp4").unwrap();
        let ctx = egui::Context::default();
        let mut app = MmCompare::default();
        app.state
            .append_standalone_images(vec![make_info(&ctx, "img")]);
        let (files, dirs, videos) = app.classify_paths(vec![tmp.join("a.mp4")]);
        assert!(files.is_empty() && dirs.is_empty());
        assert_eq!(videos, vec![tmp.join("a.mp4")]);
        assert!(app.state.image_cells.is_empty(), "进入视频模式清空图片");
        assert!(!app.state.loaded_paths.contains(&PathBuf::from("img")));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn image_batch_exits_video_mode() {
        let tmp = setup("video_exit");
        let mut app = MmCompare::default();
        let v = VideoCell {
            path: tmp.join("a.mp4"),
            info: crate::core::video::VideoInfo {
                width: 320,
                height: 240,
                duration_secs: 2.0,
                frame_rate: 30.0,
                rotation: 0,
            },
            texture: None,
            playing: false,
            position_secs: 0.0,
            frame_pts: 0.0,
            failed: false,
        };
        app.state.append_videos(vec![v]);
        let (files, dirs, videos) = app.classify_paths(vec![tmp.join("a.png")]);
        assert_eq!(files, vec![tmp.join("a.png")]);
        assert!(dirs.is_empty() && videos.is_empty());
        assert!(app.state.video_cells.is_empty(), "回图片模式清空视频");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn mixed_batch_prefers_video() {
        let tmp = setup("video_mixed");
        fs::write(tmp.join("v.mp4"), b"mp4").unwrap();
        let mut app = MmCompare::default();
        let (files, dirs, videos) = app.classify_paths(vec![tmp.join("v.mp4"), tmp.join("a.png")]);
        assert!(files.is_empty() && dirs.is_empty(), "混合批视频优先");
        assert_eq!(videos, vec![tmp.join("v.mp4")]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn videos_truncated_to_max() {
        let tmp = setup("video_max");
        let mut paths = Vec::new();
        for i in 0..6 {
            let p = tmp.join(format!("v{i}.mp4"));
            fs::write(&p, b"mp4").unwrap();
            paths.push(p);
        }
        let mut app = MmCompare::default();
        let (_, _, videos) = app.classify_paths(paths);
        assert_eq!(videos.len(), MAX_VIDEOS, "视频上限 4");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn same_batch_folders_are_sorted() {
        // 同时拖入（同一批次）的文件夹按名字排序；依次拖入（每批单个）不受影响
        let dir = std::env::temp_dir().join(format!("mmc_sort_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("z_folder")).unwrap();
        fs::create_dir_all(dir.join("a_folder")).unwrap();
        let mut app = MmCompare::default();
        let (files, dirs, _videos) =
            app.classify_paths(vec![dir.join("z_folder"), dir.join("a_folder")]);
        assert!(files.is_empty());
        assert_eq!(
            dirs,
            vec![dir.join("a_folder"), dir.join("z_folder")],
            "同批文件夹按名字排序"
        );
        // 单目录批次：保持原样（排序对单个无影响）
        let (_, dirs2, _) = app.classify_paths(vec![dir.join("z_folder")]);
        assert_eq!(dirs2, vec![dir.join("z_folder")]);
        let _ = fs::remove_dir_all(&dir);
    }
}
