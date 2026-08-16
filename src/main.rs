//! 程序入口。
//!
//! 职责边界（见 docs/architecture.md 分层）：
//! - 本文件只做环境初始化与窗口创建，不包含任何业务逻辑；
//! - 真正的应用（状态、加载管线、键盘事件）都在 [`ui::imlayout::MmCompare`] 里。
//!   imlayout 统筹所有 imcell，imcell 只负责单格渲染。
//!
//! 初始化顺序是唯一的：`env_logger` → 收集命令行参数 → 构造 `NativeOptions`
//! → `run_native` 回调里安装图片加载器、设置主题、注入启动路径。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod state;
mod ui;

use eframe::egui;

use std::path::PathBuf;

fn main() -> eframe::Result {
    env_logger::init();

    let startup_paths: Vec<PathBuf> = std::env::args()
        .skip(1)
        .filter_map(|a| {
            let p = PathBuf::from(&a);
            p.exists().then_some(p)
        })
        .collect();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "MMCompare",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            let mut app = ui::imlayout::MmCompare::default();
            if !startup_paths.is_empty() {
                app.load_startup_paths(startup_paths, &cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}
