#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
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
            let mut app = app::MmCompare::default();
            if !startup_paths.is_empty() {
                app.load_startup_paths(startup_paths, &cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}
