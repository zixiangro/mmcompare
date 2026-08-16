//! 文件夹 cell 的专属模块：扫描、缩略图、打开/导航、渲染。
//!
//! 文件夹的一切（管理 + 绘制）都收在这里，imlayout 只做编排调用：
//! - 目录扫描走子线程批次（`scan_folders` / `poll_scan`），主线程只登记结果；
//! - 缩略图分批生成（每次 ≤ `THUMB_BATCH` 张），队列按 (文件夹, 偏移) 推进，
//!   峰值内存 ≈ 并发数 × (文件字节 + 解码缓冲)，不随目录大小增长；
//! - 打开/导航复用同一加载管线（`LoadTarget::OpenEntry` / `OpenEntries` /
//!   `Navigate` / `NavigateMany`），每个文件夹同时最多打开 1 张图片；
//! - 渲染（列表/缩略图双视图、选择、滚动、右键菜单）在本模块，
//!   只读改 `FolderCell` 自身的视图状态，不碰全局业务状态。
//!
//! 线程原语与 imlayout.rs 一样受 ADR-0001 约束：只出现在本模块的
//! 加载/扫描方法组内。完整流程见 docs/folder.md。

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;

use crate::core;
use crate::state::{
    AppState, CellKind, FolderAction, FolderCell, FolderView, ImageInfo, MAX_IMAGES,
};

/// 缩略图单批并发上限：同时解码的原图数 × (文件字节 + 解码缓冲) 即峰值内存。
const THUMB_BATCH: usize = 8;
/// 单个文件夹的缩略图数量上限：超过的条目不生成缩略图（渲染时显示占位），
/// 避免海量目录长期占用加载管线（缩略图与打开/导航共用 `load_rx`）。
const THUMB_LIMIT: usize = 200;

const ROW_H: f32 = 52.0;
const THUMB_SIZE: f32 = 44.0;
const THUMB_PAD: f32 = 6.0;
const GRID_CELL: f32 = 110.0;
const GRID_PAD: f32 = 8.0;

type LoadResult = Result<(core::image::DecodedImage, String, [u32; 256]), PathBuf>;
type ScanResult = Option<(PathBuf, Vec<PathBuf>)>;
/// 缩略图载荷：只需要缩小后的像素（EXIF/直方图跳过）。
type ThumbResult = Result<core::image::DecodedImage, PathBuf>;

enum LoadTarget {
    /// 打开单个条目：目标文件夹带 `dir` 做漂移校验（加载期间文件夹被删/索引
    /// 移动时，结果丢弃而非写到错误的文件夹）。
    OpenEntry {
        folder_idx: usize,
        entry_idx: usize,
        dir: PathBuf,
    },
    OpenEntries {
        folder_idx: usize,
        entry_idxs: Vec<usize>,
        dir: PathBuf,
    },
    /// 空格打开：多个文件夹的选中条目拍平为一个批次
    /// (文件夹下标, 条目下标, 目标 dir)。
    OpenMany(Vec<(usize, usize, PathBuf)>),
    /// (图片下标, 文件夹下标, 新条目下标, 目标文件夹 dir)
    NavigateMany(Vec<(usize, usize, usize, PathBuf)>),
}

/// 文件夹 cell 的扫描/加载/缩略图状态机。
///
/// 三条独立管线（各自 mpsc 批次，互不阻塞）：
/// - `load_rx`：打开/导航（全图解码）；
/// - `thumb_rx`：缩略图（64px 解码）——缩略图加载中打开/导航仍可响应；
/// - `scan_rx`：目录扫描。
#[derive(Default)]
pub struct FolderManager {
    load_rx: Option<mpsc::Receiver<(usize, LoadResult)>>,
    loading_total: usize,
    loading_received: usize,
    loading_buf: Vec<Option<ImageInfo>>,
    load_target: Option<LoadTarget>,
    thumb_rx: Option<mpsc::Receiver<(usize, ThumbResult)>>,
    thumb_total: usize,
    thumb_received: usize,
    thumb_buf: Vec<Option<egui::TextureHandle>>,
    /// 当前缩略图批次的目标 (文件夹下标, 起始条目, 目标 dir)——
    /// dir 用于漂移校验（加载期间文件夹被删时丢弃结果）。
    thumb_buf_meta: Option<(usize, usize, PathBuf)>,
    scan_rx: Option<mpsc::Receiver<(usize, ScanResult)>>,
    scan_total: usize,
    scan_received: usize,
    scan_buf: Vec<ScanResult>,
    /// 排队等待扫描的目录批次（当前扫描批次完成后按序启动）。
    pending_scan: VecDeque<Vec<PathBuf>>,
    /// 待生成缩略图的 (文件夹下标, 下一个条目偏移, 目标 dir)——
    /// dir 用于漂移校验（文件夹索引移动时丢弃过期的请求）。
    /// 调度：新目录 `push_front` 插队（后拖入的优先，方便及时对比），
    /// `drain_thumbnails` 每批处理后未完成的回队尾（轮转，多目录均衡）。
    pending_thumbnails: VecDeque<(usize, usize, PathBuf)>,
}

pub(crate) fn is_image_ext(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp"
        )
    })
}

pub(crate) fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        let na = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let nb = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        na.to_lowercase().cmp(&nb.to_lowercase())
    });
}

impl FolderManager {
    /// 是否有未完成/未开始的目录扫描（文件夹模式判定用：
    /// 目录从拖入到登记入格之间的窗口期也算文件夹模式）。
    pub fn has_pending(&self) -> bool {
        self.scan_rx.is_some() || !self.pending_scan.is_empty()
    }

    /// 排队扫描：当前无扫描批次时立即启动；扫描中则入队，
    /// 当前批次完成后自动接续——拖入新目录不受旧目录加载进度影响。
    pub fn queue_scan(&mut self, dirs: Vec<PathBuf>) {
        if dirs.is_empty() {
            return;
        }
        if self.scan_rx.is_none() {
            self.scan_folders(dirs);
        } else {
            self.pending_scan.push_back(dirs);
        }
    }

    /// 启动一批目录扫描（子线程 read_dir + 过滤 + 排序），
    /// 结果由 `poll_scan` 收齐后登记为文件夹 cell。
    fn scan_folders(&mut self, dirs: Vec<PathBuf>) {
        self.scan_total = dirs.len();
        self.scan_received = 0;
        self.scan_buf = (0..dirs.len()).map(|_| None).collect();
        let (tx, rx) = mpsc::channel();
        for (i, dir) in dirs.into_iter().enumerate() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let entries: ScanResult = std::fs::read_dir(&dir).ok().map(|rd| {
                    let mut v: Vec<PathBuf> = rd
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| is_image_ext(p))
                        .collect();
                    sort_paths(&mut v);
                    (dir.clone(), v)
                });
                tx.send((i, entries)).ok();
            });
        }
        drop(tx);
        self.scan_rx = Some(rx);
    }

    /// 每帧收齐扫描结果，逐个登记文件夹 cell；缩略图加载排队。
    pub fn poll_scan(&mut self, state: &mut AppState) {
        let Some(rx) = &self.scan_rx else {
            return;
        };
        while let Ok((i, entries)) = rx.try_recv() {
            self.scan_buf[i] = entries;
            self.scan_received += 1;
        }
        if self.scan_received < self.scan_total {
            return;
        }
        let dirs = std::mem::take(&mut self.scan_buf);
        for (dir, entries) in dirs.into_iter().flatten() {
            self.register_folder_cell(state, dir, entries);
        }
        self.scan_rx = None;
        self.scan_total = 0;
        self.scan_received = 0;
        if let Some(next) = self.pending_scan.pop_front() {
            self.scan_folders(next);
        }
    }

    /// 登记一个已扫描完成的文件夹 cell（网格满时跳过）。
    /// `thumbnails` 预填 `None` 与 `entries` 等长，分批写入时按槽位对齐。
    fn register_folder_cell(&mut self, state: &mut AppState, dir: PathBuf, entries: Vec<PathBuf>) {
        if state.cell_order.len() >= MAX_IMAGES {
            return;
        }
        let idx = state.folder_cells.len();
        state.folder_cells.push(FolderCell {
            dir_path: dir,
            entries,
            selected: HashSet::new(),
            view_mode: FolderView::List,
            scroll_offset: 0.0,
            thumbnails: Vec::new(),
            open_entry: None,
            scroll_to: None,
        });
        state.cell_order.push(CellKind::Folder(idx));
        state.pan_offset.push([0.0, 0.0]);
        let n = state.folder_cells[idx].entries.len();
        // 只预填前 THUMB_LIMIT 个槽位：超限条目渲染时 get(i) 越界返回 None（占位）
        state.folder_cells[idx].thumbnails = (0..n.min(THUMB_LIMIT)).map(|_| None).collect();
        // 新目录插队到队首：后拖入的优先加载，方便及时对比
        let dir = state.folder_cells[idx].dir_path.clone();
        self.pending_thumbnails.push_front((idx, 0, dir));
    }

    /// 缩略图调度：取队首文件夹的一批（≤`THUMB_BATCH` 张），
    /// 未完成的回队尾（轮转）——多目录交替加载，单目录自转不受影响。
    /// 走独立管线（`thumb_rx`），不阻塞打开/导航。
    pub fn drain_thumbnails(&mut self, state: &mut AppState, ctx: &egui::Context) {
        if self.thumb_rx.is_some() || self.pending_thumbnails.is_empty() {
            return;
        }
        let (fi, offset, dir) = self.pending_thumbnails[0].clone();
        let entries = match state.folder_cells.get(fi) {
            Some(f) => {
                if f.dir_path != dir {
                    // 文件夹索引已移动（加载期间删过其他文件夹），丢弃过期请求
                    self.pending_thumbnails.pop_front();
                    return;
                }
                f.entries.clone()
            }
            None => {
                self.pending_thumbnails.pop_front();
                return;
            }
        };
        if offset >= entries.len().min(THUMB_LIMIT) {
            self.pending_thumbnails.pop_front();
            return;
        }
        let end = (offset + THUMB_BATCH).min(entries.len()).min(THUMB_LIMIT);
        let batch = entries[offset..end].to_vec();
        self.spawn_thumbnails(batch, ctx, fi, offset, dir.clone());
        let done = end >= entries.len().min(THUMB_LIMIT);
        self.pending_thumbnails.pop_front();
        if !done {
            self.pending_thumbnails.push_back((fi, end, dir));
        }
    }

    /// 启动一批缩略图解码（独立 channel）：线程内读文件 + 64px 解码，
    /// 结果由 `poll_thumbnails` 收齐后按槽位写入。
    fn spawn_thumbnails(
        &mut self,
        paths: Vec<PathBuf>,
        ctx: &egui::Context,
        folder_idx: usize,
        start: usize,
        dir: PathBuf,
    ) {
        self.thumb_total = paths.len();
        self.thumb_received = 0;
        self.thumb_buf = (0..paths.len()).map(|_| None).collect();
        let (tx, rx) = mpsc::channel();

        for (i, p) in paths.into_iter().enumerate() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let result: ThumbResult = (|| {
                    let bytes = std::fs::read(&p).map_err(|e| {
                        log::warn!("read failed {}: {}", p.display(), e);
                        p.clone()
                    })?;
                    let mut img =
                        core::image::decode_thumbnail_bytes(&bytes, 64).ok_or_else(|| {
                            log::warn!("thumb decode failed {}", p.display());
                            p.clone()
                        })?;
                    img.path = p.clone();
                    Ok(img)
                })();
                tx.send((i, result)).ok();
            });
        }
        drop(tx);

        self.thumb_rx = Some(rx);
        self.thumb_buf_meta = Some((folder_idx, start, dir));
        ctx.request_repaint();
    }

    /// 每帧把已完成的缩略图搬进 state（独立于打开/导航管线）。
    pub fn poll_thumbnails(&mut self, state: &mut AppState, ctx: &egui::Context) {
        let Some(rx) = &self.thumb_rx else {
            return;
        };
        while let Ok((i, result)) = rx.try_recv() {
            if let Ok(img) = result {
                let name = img
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("thumb");
                let tex = crate::ui::imcell::upload_texture(ctx, &img.rgba, img.size, name);
                self.thumb_buf[i] = Some(tex);
            }
            self.thumb_received += 1;
        }
        if self.thumb_received < self.thumb_total {
            return;
        }
        let buf = std::mem::take(&mut self.thumb_buf);
        if let Some((folder_idx, start, dir)) = self.thumb_buf_meta.take()
            && let Some(folder) = state.folder_cells.get_mut(folder_idx)
            && folder.dir_path == dir
        // 漂移校验：文件夹被删/索引移动时丢弃结果
        {
            for (i, tex) in buf.into_iter().enumerate() {
                if start + i < folder.thumbnails.len() {
                    folder.thumbnails[start + i] = tex;
                }
            }
        }
        self.thumb_rx = None;
        self.thumb_total = 0;
        self.thumb_received = 0;
        ctx.request_repaint();
    }

    /// 打开文件夹条目（每个文件夹同时最多 1 张，重复打开自动替换）。
    pub fn open_entry(
        &mut self,
        state: &mut AppState,
        ctx: &egui::Context,
        folder_idx: usize,
        entry_idx: usize,
    ) {
        if self.load_rx.is_some() || folder_idx >= state.folder_cells.len() {
            return;
        }
        if entry_idx >= state.folder_cells[folder_idx].entries.len() {
            return;
        }
        let path = state.folder_cells[folder_idx].entries[entry_idx].clone();
        let dir = state.folder_cells[folder_idx].dir_path.clone();
        self.spawn_loaders(
            vec![path],
            ctx,
            LoadTarget::OpenEntry {
                folder_idx,
                entry_idx,
                dir,
            },
        );
    }

    /// 打开选中条目，按剩余网格名额截断（打开后文件夹隐藏腾 1 格）。
    pub fn open_selected(&mut self, state: &mut AppState, ctx: &egui::Context, folder_idx: usize) {
        if self.load_rx.is_some() || folder_idx >= state.folder_cells.len() {
            return;
        }
        let sel: Vec<usize> = state.folder_cells[folder_idx]
            .selected
            .iter()
            .copied()
            .collect();
        if sel.is_empty() {
            return;
        }
        let room = MAX_IMAGES
            .saturating_sub(state.cell_order.len())
            .saturating_add(1);
        let sel: Vec<usize> = sel.into_iter().take(room).collect();
        if sel.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = sel
            .iter()
            .map(|&i| state.folder_cells[folder_idx].entries[i].clone())
            .collect();
        let dir = state.folder_cells[folder_idx].dir_path.clone();
        self.spawn_loaders(
            paths,
            ctx,
            LoadTarget::OpenEntries {
                folder_idx,
                entry_idxs: sel,
                dir,
            },
        );
    }

    /// 空格打开：收集**所有**文件夹的选中条目为一个批次（双栏联动选中后
    /// 一次打开两张），按网格名额截断（可见文件夹各腾 1 格）。
    pub fn open_selected_all(&mut self, state: &mut AppState, ctx: &egui::Context) {
        if self.load_rx.is_some() {
            return;
        }
        let mut flat: Vec<(usize, usize, PathBuf)> = Vec::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        for (fi, f) in state.folder_cells.iter().enumerate() {
            for &ei in &f.selected {
                if ei < f.entries.len() {
                    flat.push((fi, ei, f.dir_path.clone()));
                    paths.push(f.entries[ei].clone());
                }
            }
        }
        if flat.is_empty() {
            return;
        }
        // 每个可见文件夹打开后隐藏腾 1 格，净增 = 打开数 - 可见文件夹数
        let visible_folders = state
            .cell_order
            .iter()
            .filter(|c| matches!(c, CellKind::Folder(_)))
            .count();
        let room = MAX_IMAGES
            .saturating_sub(state.cell_order.len())
            .saturating_add(visible_folders);
        flat.truncate(room);
        paths.truncate(room);
        if flat.is_empty() {
            return;
        }
        self.spawn_loaders(paths, ctx, LoadTarget::OpenMany(flat));
    }

    /// 同步导航：所有打开的文件夹图片各自前进/后退一张（双文件夹对比索引）。
    /// 每个目标带 dir 做漂移校验。
    pub fn navigate(
        &mut self,
        state: &mut AppState,
        ctx: &egui::Context,
        targets: Vec<(usize, usize, usize, PathBuf)>,
    ) {
        if self.load_rx.is_some() || targets.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = targets.iter().map(|t| t.3.clone()).collect();
        let index: Vec<(usize, usize, usize, PathBuf)> = targets
            .into_iter()
            .map(|(img, fi, ei, _path)| {
                let dir = state
                    .folder_cells
                    .get(fi)
                    .map(|f| f.dir_path.clone())
                    .unwrap_or_default();
                (img, fi, ei, dir)
            })
            .collect();
        self.spawn_loaders(paths, ctx, LoadTarget::NavigateMany(index));
    }

    fn spawn_loaders(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context, target: LoadTarget) {
        self.loading_total = paths.len();
        self.loading_received = 0;
        self.loading_buf = (0..paths.len()).map(|_| None).collect();
        self.load_target = Some(target);
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

    /// 每帧把已完成的解码结果搬进 state，收齐后按 `load_target` 分发。
    pub fn poll_loading(&mut self, state: &mut AppState, ctx: &egui::Context) {
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
                    let texture = crate::ui::imcell::upload_texture(ctx, &img.rgba, img.size, name);
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
                    state.loaded_paths.remove(&path);
                    state.load_errors.push(path);
                }
            }
            self.loading_received += 1;
        }

        if self.loading_received < self.loading_total {
            return;
        }
        let buf = std::mem::take(&mut self.loading_buf);
        let dir_matches = |state: &AppState, folder_idx: usize, dir: &PathBuf| {
            state.folder_cells.get(folder_idx).map(|f| &f.dir_path) == Some(dir)
        };
        match self.load_target.take() {
            Some(LoadTarget::OpenEntry {
                folder_idx,
                entry_idx,
                dir,
            }) => {
                if let Some(Some(info)) = buf.into_iter().next()
                    && dir_matches(state, folder_idx, &dir)
                // 漂移校验
                {
                    state.open_folder_entry(folder_idx, entry_idx, info);
                }
            }
            Some(LoadTarget::OpenEntries {
                folder_idx,
                entry_idxs,
                dir,
            }) => {
                if dir_matches(state, folder_idx, &dir) {
                    for (i, slot) in buf.into_iter().enumerate() {
                        if let (Some(info), Some(&ei)) = (slot, entry_idxs.get(i)) {
                            state.open_folder_entry(folder_idx, ei, info);
                        }
                    }
                }
            }
            Some(LoadTarget::OpenMany(entries)) => {
                for (i, slot) in buf.into_iter().enumerate() {
                    if let (Some(info), Some(&(folder_idx, entry_idx, ref dir))) =
                        (slot, entries.get(i))
                        && dir_matches(state, folder_idx, dir)
                    // 漂移校验
                    {
                        state.open_folder_entry(folder_idx, entry_idx, info);
                    }
                }
            }
            Some(LoadTarget::NavigateMany(targets)) => {
                for (i, slot) in buf.into_iter().enumerate() {
                    if let (Some(info), Some(&(img_idx, folder_idx, entry_idx, ref dir))) =
                        (slot, targets.get(i))
                        && img_idx < state.image_cells.len()
                        && dir_matches(state, folder_idx, dir)
                    // 漂移校验
                    {
                        state.apply_navigated_image(img_idx, folder_idx, entry_idx, info);
                    }
                }
            }
            _ => {}
        }
        self.load_rx = None;
        self.loading_total = 0;
        self.loading_received = 0;
        ctx.request_repaint();
    }

    /// 文件夹渲染层上报的用户意图（双击打开、右键菜单等）。
    pub fn handle_action(
        &mut self,
        state: &mut AppState,
        action: FolderAction,
        ctx: &egui::Context,
    ) {
        match action {
            FolderAction::OpenImage(idx) => {
                for pos in 0..state.cell_order.len() {
                    if let CellKind::Folder(fi) = state.cell_order[pos]
                        && idx < state.folder_cells[fi].entries.len()
                    {
                        self.open_entry(state, ctx, fi, idx);
                        break;
                    }
                }
            }
            FolderAction::OpenFolder(path) => {
                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("explorer")
                        .arg("/select,")
                        .arg(&path)
                        .spawn();
                }
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open")
                        .arg("-R")
                        .arg(&path)
                        .spawn();
                }
                #[cfg(target_os = "linux")]
                {
                    if let Some(p) = path.parent() {
                        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
                    }
                }
            }
            FolderAction::Remove(idx) => {
                for pos in 0..state.cell_order.len() {
                    if let CellKind::Folder(fi) = state.cell_order[pos] {
                        let f = &mut state.folder_cells[fi];
                        if idx < f.entries.len() {
                            f.entries.remove(idx);
                            f.selected.remove(&idx);
                            let sel: Vec<usize> = f.selected.iter().copied().collect();
                            f.selected.clear();
                            for s in sel {
                                f.selected.insert(if s > idx { s - 1 } else { s });
                            }
                            if idx < f.thumbnails.len() {
                                f.thumbnails.remove(idx);
                            }
                        }
                        break;
                    }
                }
            }
            FolderAction::OpenSelected => {
                for pos in 0..state.cell_order.len() {
                    if let CellKind::Folder(fi) = state.cell_order[pos] {
                        self.open_selected(state, ctx, fi);
                        break;
                    }
                }
            }
            FolderAction::ToggleView => {
                for pos in 0..state.cell_order.len() {
                    if let CellKind::Folder(fi) = state.cell_order[pos] {
                        let f = &mut state.folder_cells[fi];
                        f.view_mode = match f.view_mode {
                            FolderView::List => FolderView::Thumbnail,
                            FolderView::Thumbnail => FolderView::List,
                        };
                        break;
                    }
                }
            }
            // 无修饰单击：所有文件夹 cell 联动选中同索引（双栏对比的基准操作）
            FolderAction::SelectSynced(idx) => {
                for f in &mut state.folder_cells {
                    f.selected.clear();
                    f.selected.insert(idx);
                }
            }
            FolderAction::None => {}
        }
    }
}

pub fn render_folder_cell(
    ui: &mut egui::Ui,
    folder: &mut FolderCell,
    cell_rect: egui::Rect,
    // 文件夹下标：行/格 widget id 的盐，避免多栏同索引条目 id 冲突
    // （egui 的 persistent id 同帧重复会互相覆盖交互状态）。
    id_salt: usize,
) -> FolderAction {
    ui.painter()
        .rect_filled(cell_rect, 0.0, egui::Color32::from_gray(248));
    ui.allocate_rect(cell_rect, egui::Sense::hover());

    if folder.entries.is_empty() {
        ui.painter().text(
            cell_rect.center(),
            egui::Align2::CENTER_CENTER,
            "No images in folder",
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(150),
        );
        return FolderAction::None;
    }

    let hover_pos = ui.input(|i| i.pointer.hover_pos());
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    let shift = ui.input(|i| i.modifiers.shift);
    let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);

    if matches!(folder.view_mode, FolderView::List) {
        render_list(
            ui,
            folder,
            cell_rect,
            hover_pos,
            ctrl,
            shift,
            scroll_delta,
            id_salt,
        )
    } else {
        render_grid(
            ui,
            folder,
            cell_rect,
            hover_pos,
            ctrl,
            shift,
            scroll_delta,
            id_salt,
        )
    }
}

#[allow(clippy::too_many_arguments)] // 纯绘制+交互函数，扁平参数
fn render_list(
    ui: &mut egui::Ui,
    folder: &mut FolderCell,
    cell_rect: egui::Rect,
    hover_pos: Option<egui::Pos2>,
    ctrl: bool,
    shift: bool,
    scroll_delta: f32,
    id_salt: usize,
) -> FolderAction {
    let total_h = folder.entries.len() as f32 * ROW_H;
    let max_scroll = (total_h - cell_rect.height()).max(0.0);
    // scroll_to：恢复视图时滚动一次，使目标条目成为可见区域**第二条**
    // （顶部留一行上下文）。take 后即清除——不做 selected 回退，
    // 否则每帧强制重置会锁死用户自己的滚动。
    if let Some(target) = folder.scroll_to.take() {
        let want = target.saturating_sub(1) as f32 * ROW_H;
        folder.scroll_offset = want.clamp(0.0, max_scroll);
    }
    folder.scroll_offset = (folder.scroll_offset - scroll_delta).clamp(0.0, max_scroll);
    let base_y = cell_rect.top() - folder.scroll_offset;
    let menu_action = std::rc::Rc::new(std::cell::RefCell::new(None));
    let mut result = FolderAction::None;

    for i in 0..folder.entries.len() {
        let y = base_y + i as f32 * ROW_H;
        if y + ROW_H < cell_rect.top() || y > cell_rect.bottom() {
            continue;
        }
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(cell_rect.left(), y),
            egui::vec2(cell_rect.width(), ROW_H),
        );
        let is_hovered = hover_pos.is_some_and(|hp| row_rect.contains(hp));
        let is_selected = folder.selected.contains(&i);

        let bg = if is_selected {
            egui::Color32::from_rgb(200, 220, 255)
        } else if is_hovered {
            egui::Color32::from_gray(225)
        } else {
            egui::Color32::from_gray(248)
        };
        ui.painter().rect_filled(row_rect, 0.0, bg);

        let thumb_rect = egui::Rect::from_min_size(
            row_rect.left_center() + egui::vec2(THUMB_PAD, -THUMB_SIZE / 2.0),
            egui::vec2(THUMB_SIZE, THUMB_SIZE),
        );
        if let Some(tex) = folder.thumbnails.get(i).and_then(|t| t.as_ref()) {
            let fit = fit_rect(thumb_rect, tex.size_vec2());
            ui.painter().image(
                tex.id(),
                fit,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let name = folder.entries[i]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("???");
        let tc = if is_selected {
            egui::Color32::from_gray(20)
        } else {
            egui::Color32::from_gray(60)
        };
        ui.painter().text(
            row_rect.left_center() + egui::vec2(THUMB_SIZE + THUMB_PAD * 2.0, -6.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(12.0),
            tc,
        );

        let row_id = ui.make_persistent_id(format!("frow_{}_{}", id_salt, i));
        let row_resp = ui.interact(row_rect, row_id, egui::Sense::click());
        if row_resp.clicked() {
            if ctrl {
                // Ctrl+单击：重新选当前栏的另一张（替换本栏选择，不联动）
                folder.selected.clear();
                folder.selected.insert(i);
            } else if shift {
                let last = folder.selected.iter().max().copied().unwrap_or(i);
                let (lo, hi) = if i < last { (i, last) } else { (last, i) };
                for j in lo..=hi {
                    folder.selected.insert(j);
                }
            } else {
                // 无修饰单击：联动选中所有栏的同索引（由 imlayout 统一处理）
                result = FolderAction::SelectSynced(i);
            }
        }
        if row_resp.double_clicked() {
            result = FolderAction::OpenImage(i);
        }

        let entry_c = folder.entries[i].clone();
        let sc = folder.selected.len();
        let ac = menu_action.clone();
        if !ctrl {
            row_resp.context_menu(move |ui| {
                context_menu(ui, i, entry_c, sc, ac);
            });
        }
    }

    if matches!(result, FolderAction::None)
        && let Some(a) = menu_action.borrow_mut().take()
    {
        result = a;
    }
    result
}

#[allow(clippy::too_many_arguments)] // 纯绘制+交互函数，扁平参数
fn render_grid(
    ui: &mut egui::Ui,
    folder: &mut FolderCell,
    cell_rect: egui::Rect,
    hover_pos: Option<egui::Pos2>,
    ctrl: bool,
    shift: bool,
    scroll_delta: f32,
    id_salt: usize,
) -> FolderAction {
    let avail_w = cell_rect.width() - GRID_PAD;
    let cols = (avail_w / (GRID_CELL + GRID_PAD)).max(1.0) as usize;
    let rows = folder.entries.len().div_ceil(cols);
    let grid_h = rows as f32 * (GRID_CELL + GRID_PAD) + GRID_PAD;
    let max_scroll = (grid_h - cell_rect.height()).max(0.0);
    // scroll_to：恢复视图时滚动一次，使目标条目成为可见区域**第二条**
    // （顶部留一行上下文）。take 后即清除——不做 selected 回退，
    // 否则每帧强制重置会锁死用户自己的滚动。
    if let Some(target) = folder.scroll_to.take() {
        let row = target / cols;
        let want = if row > 0 {
            GRID_PAD + (row - 1) as f32 * (GRID_CELL + GRID_PAD)
        } else {
            0.0
        };
        folder.scroll_offset = want.clamp(0.0, max_scroll);
    }
    folder.scroll_offset = (folder.scroll_offset - scroll_delta).clamp(0.0, max_scroll);
    let base_y = cell_rect.top() - folder.scroll_offset + GRID_PAD;
    let start_x = cell_rect.left()
        + (avail_w - (cols as f32 * GRID_CELL + (cols - 1) as f32 * GRID_PAD)) / 2.0
        + GRID_PAD;
    let menu_action = std::rc::Rc::new(std::cell::RefCell::new(None));
    let mut result = FolderAction::None;

    for i in 0..folder.entries.len() {
        let col = i % cols;
        let row = i / cols;
        let x = start_x + col as f32 * (GRID_CELL + GRID_PAD);
        let y = base_y + row as f32 * (GRID_CELL + GRID_PAD);
        if y + GRID_CELL < cell_rect.top() || y > cell_rect.bottom() {
            continue;
        }
        let gc = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(GRID_CELL, GRID_CELL));
        let is_hovered = hover_pos.is_some_and(|hp| gc.contains(hp));
        let is_selected = folder.selected.contains(&i);

        let bg = if is_selected {
            egui::Color32::from_rgb(200, 220, 255)
        } else if is_hovered {
            egui::Color32::from_gray(225)
        } else {
            egui::Color32::from_gray(248)
        };
        ui.painter().rect_filled(gc, 0.0, bg);

        let thumb = gc.shrink(4.0);
        if let Some(tex) = folder.thumbnails.get(i).and_then(|t| t.as_ref()) {
            let fit = fit_rect(thumb, tex.size_vec2());
            ui.painter().image(
                tex.id(),
                fit,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let name = folder.entries[i]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("???");
        let dname = if name.len() > 14 {
            format!("{}…", &name[..13])
        } else {
            name.to_string()
        };
        let tc = if is_selected {
            egui::Color32::from_gray(20)
        } else {
            egui::Color32::from_gray(60)
        };
        ui.painter().text(
            gc.center_bottom() + egui::vec2(0.0, 2.0),
            egui::Align2::CENTER_TOP,
            &dname,
            egui::FontId::proportional(10.0),
            tc,
        );

        let row_id = ui.make_persistent_id(format!("fgrid_{}_{}", id_salt, i));
        let resp = ui.interact(gc, row_id, egui::Sense::click());
        if resp.clicked() {
            if ctrl {
                // Ctrl+单击：重新选当前栏的另一张（替换本栏选择，不联动）
                folder.selected.clear();
                folder.selected.insert(i);
            } else if shift {
                let last = folder.selected.iter().max().copied().unwrap_or(i);
                let (lo, hi) = if i < last { (i, last) } else { (last, i) };
                for j in lo..=hi {
                    folder.selected.insert(j);
                }
            } else {
                // 无修饰单击：联动选中所有栏的同索引（由 imlayout 统一处理）
                result = FolderAction::SelectSynced(i);
            }
        }
        if resp.double_clicked() {
            result = FolderAction::OpenImage(i);
        }

        let ec = folder.entries[i].clone();
        let sc = folder.selected.len();
        let ac = menu_action.clone();
        if !ctrl {
            resp.context_menu(move |ui| {
                context_menu(ui, i, ec, sc, ac);
            });
        }
    }

    if matches!(result, FolderAction::None)
        && let Some(a) = menu_action.borrow_mut().take()
    {
        result = a;
    }
    result
}

fn context_menu(
    ui: &mut egui::Ui,
    i: usize,
    entry: std::path::PathBuf,
    selected_count: usize,
    action: std::rc::Rc<std::cell::RefCell<Option<FolderAction>>>,
) {
    if ui.button("Open image").clicked() {
        *action.borrow_mut() = Some(FolderAction::OpenImage(i));
        ui.close();
    }
    if ui.button("Open file location").clicked() {
        *action.borrow_mut() = Some(FolderAction::OpenFolder(entry));
        ui.close();
    }
    if ui.button("Remove from list").clicked() {
        *action.borrow_mut() = Some(FolderAction::Remove(i));
        ui.close();
    }
    if selected_count > 0 {
        ui.separator();
        if ui
            .button(format!("Open selected ({})", selected_count))
            .clicked()
        {
            *action.borrow_mut() = Some(FolderAction::OpenSelected);
            ui.close();
        }
    }
    ui.separator();
    if ui.button("Toggle thumbnail/list view").clicked() {
        *action.borrow_mut() = Some(FolderAction::ToggleView);
        ui.close();
    }
}

fn fit_rect(outer: egui::Rect, img_size: egui::Vec2) -> egui::Rect {
    let scale = (outer.width() / img_size.x).min(outer.height() / img_size.y);
    let w = img_size.x * scale;
    let h = img_size.y * scale;
    egui::Rect::from_min_size(
        egui::pos2(outer.center().x - w / 2.0, outer.center().y - h / 2.0),
        egui::vec2(w, h),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("img{i}.png")))
            .collect()
    }

    #[test]
    fn new_folder_jumps_thumb_queue_front() {
        let mut m = FolderManager::default();
        let mut s = AppState::new();
        m.register_folder_cell(&mut s, PathBuf::from("a"), entries(20));
        m.register_folder_cell(&mut s, PathBuf::from("b"), entries(20));
        assert_eq!(m.pending_thumbnails.len(), 2);
        assert_eq!(m.pending_thumbnails[0].0, 1, "后拖入的 B 插队到队首");
        assert_eq!(m.pending_thumbnails[1].0, 0);
    }

    #[test]
    fn thumb_round_robin_rotates_unfinished_to_back() {
        let mut m = FolderManager::default();
        let mut s = AppState::new();
        m.register_folder_cell(&mut s, PathBuf::from("a"), entries(20));
        m.register_folder_cell(&mut s, PathBuf::from("b"), entries(20));
        // 模拟 A 已完成一批（offset=8），手动推进队列：队首 B 完成一批回队尾
        m.pending_thumbnails.pop_front(); // 取 B
        m.pending_thumbnails.push_back((1, 8, PathBuf::from("b"))); // B 未完成回队尾
        assert_eq!(m.pending_thumbnails[0].0, 0, "A 回到队首，下一批加载 A");
        assert_eq!(
            m.pending_thumbnails[1],
            (1, 8, PathBuf::from("b")),
            "B 保留进度在队尾"
        );
    }
    #[test]
    fn thumb_limit_caps_prealloc_and_queue() {
        let mut m = FolderManager::default();
        let mut s = AppState::new();
        m.register_folder_cell(&mut s, PathBuf::from("big"), entries(300));
        assert_eq!(
            s.folder_cells[0].thumbnails.len(),
            THUMB_LIMIT,
            "只预填上限个槽位"
        );
        assert_eq!(m.pending_thumbnails[0], (0, 0, PathBuf::from("big")));
        // 模拟推进到上限边界：offset >= THUMB_LIMIT 视为完成
        m.pending_thumbnails[0].1 = THUMB_LIMIT;
        // 直接验证 drain 的完成判定（不 spawn：entries 上限截断后 offset 已到顶）
        let ctx = egui::Context::default();
        m.drain_thumbnails(&mut s, &ctx);
        assert!(m.thumb_rx.is_none(), "超限后不再启动缩略图批次");
        assert!(m.pending_thumbnails.is_empty(), "队列清空");
    }
    #[test]
    fn thumb_queue_drops_stale_after_folder_index_shift() {
        // 加载期间文件夹被删导致索引漂移：队列里的 dir 与当前位置不匹配 → 丢弃
        let mut m = FolderManager::default();
        let mut s = AppState::new();
        m.register_folder_cell(&mut s, PathBuf::from("A"), entries(5));
        // 模拟 remove_cell(Folder)：folder 0 被删，目录 B 顶到下标 0
        s.folder_cells.remove(0);
        s.folder_cells.push(FolderCell {
            dir_path: PathBuf::from("B"),
            entries: entries(5),
            selected: HashSet::new(),
            view_mode: FolderView::List,
            scroll_offset: 0.0,
            thumbnails: vec![None; 5],
            open_entry: None,
            scroll_to: None,
        });
        s.cell_order.clear();
        s.cell_order.push(CellKind::Folder(0));
        // 队列里还是 (0, 0, dir="A") → 与 B 不匹配 → drain 应丢弃且不 spawn
        let ctx = egui::Context::default();
        m.drain_thumbnails(&mut s, &ctx);
        assert!(m.pending_thumbnails.is_empty(), "漂移的缩略图请求被丢弃");
        assert!(m.thumb_rx.is_none(), "未启动新批次");
    }
    #[test]
    fn select_synced_updates_all_folders() {
        let ctx = egui::Context::default();
        let mut m = FolderManager::default();
        let mut s = AppState::new();
        m.register_folder_cell(&mut s, PathBuf::from("A"), entries(5));
        m.register_folder_cell(&mut s, PathBuf::from("B"), entries(5));
        s.folder_cells[0].selected.insert(0);
        m.handle_action(&mut s, FolderAction::SelectSynced(2), &ctx);
        assert_eq!(s.folder_cells[0].selected, HashSet::from([2]), "左栏联动");
        assert_eq!(s.folder_cells[1].selected, HashSet::from([2]), "右栏联动");
    }

    #[test]
    fn open_selected_all_flattens_all_folders() {
        let ctx = egui::Context::default();
        let mut m = FolderManager::default();
        let mut s = AppState::new();
        m.register_folder_cell(&mut s, PathBuf::from("A"), entries(5));
        m.register_folder_cell(&mut s, PathBuf::from("B"), entries(5));
        s.folder_cells[0].selected.insert(2);
        s.folder_cells[1].selected.insert(3);
        m.open_selected_all(&mut s, &ctx);
        assert!(m.load_rx.is_some(), "批次已启动");
        match m.load_target.take() {
            Some(LoadTarget::OpenMany(v)) => {
                assert_eq!(v.len(), 2, "两栏选中条目拍平为一个批次");
                assert_eq!(v[0], (0, 2, PathBuf::from("A")));
                assert_eq!(v[1], (1, 3, PathBuf::from("B")));
            }
            _ => panic!("expected OpenMany"),
        }
    }
}
