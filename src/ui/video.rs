//! 视频 cell 渲染单元：帧居中绘制（letterbox）、控制条（播放/暂停、进度 seek、时间）。
//! 不碰 state：交互意图通过 [`VideoAction`] 返回，由 imlayout 统一写回
//! （seek 需要启动解码线程，属于统筹层职责）。

use eframe::egui;

use crate::state::VideoCell;

/// 视频 cell 交互意图。
pub enum VideoAction {
    None,
    TogglePlay,
    Seek(f64),
}

pub fn draw_video_cell(ui: &mut egui::Ui, cell: &VideoCell, cell_rect: egui::Rect) -> VideoAction {
    ui.painter()
        .rect_filled(cell_rect, 0.0, egui::Color32::BLACK);

    if let Some(tex) = &cell.texture {
        let size = tex.size_vec2();
        let scale = (cell_rect.width() / size.x).min(cell_rect.height() / size.y);
        let img_rect = egui::Rect::from_center_size(cell_rect.center(), size * scale);
        ui.painter().with_clip_rect(cell_rect).image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        let name = cell
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("video");
        let msg = if cell.failed {
            format!("{name}\ndecode error")
        } else {
            format!("{name}\nloading...")
        };
        ui.painter().text(
            cell_rect.center(),
            egui::Align2::CENTER_CENTER,
            msg,
            egui::FontId::monospace(13.0),
            egui::Color32::from_gray(180),
        );
    }

    draw_control_bar(ui, cell, cell_rect)
}

fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn draw_control_bar(ui: &mut egui::Ui, cell: &VideoCell, cell_rect: egui::Rect) -> VideoAction {
    let bar_h = 26.0;
    let bar = egui::Rect::from_min_size(
        egui::pos2(cell_rect.left(), cell_rect.bottom() - bar_h),
        egui::vec2(cell_rect.width(), bar_h),
    );
    ui.painter()
        .rect_filled(bar, 0.0, egui::Color32::from_black_alpha(170));

    let btn_w = 34.0;
    let btn = egui::Rect::from_min_size(bar.min, egui::vec2(btn_w, bar_h));
    ui.painter().text(
        btn.center(),
        egui::Align2::CENTER_CENTER,
        if cell.playing { "⏸" } else { "▶" },
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );

    let time_w = 104.0;
    let time_rect = egui::Rect::from_min_size(
        egui::pos2(bar.right() - time_w, bar.top()),
        egui::vec2(time_w, bar_h),
    );
    ui.painter().text(
        time_rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        format!(
            "{} / {}",
            fmt_time(cell.position_secs),
            fmt_time(cell.info.duration_secs)
        ),
        egui::FontId::monospace(11.0),
        egui::Color32::WHITE,
    );

    let prog = egui::Rect::from_min_max(
        egui::pos2(btn.right() + 6.0, bar.top() + 7.0),
        egui::pos2(time_rect.left() - 6.0, bar.bottom() - 7.0),
    );
    ui.painter()
        .rect_filled(prog, 2.0, egui::Color32::from_gray(80));
    let frac = if cell.info.duration_secs > 0.0 {
        (cell.position_secs / cell.info.duration_secs).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    if frac > 0.0 {
        let filled =
            egui::Rect::from_min_size(prog.min, egui::vec2(prog.width() * frac, prog.height()));
        ui.painter()
            .rect_filled(filled, 2.0, egui::Color32::from_rgb(0, 150, 255));
    }

    let mut action = VideoAction::None;
    let btn_resp = ui.allocate_rect(btn, egui::Sense::click());
    if btn_resp.clicked() {
        action = VideoAction::TogglePlay;
    }
    let prog_resp = ui.allocate_rect(prog, egui::Sense::click_and_drag());
    if (prog_resp.dragged() || prog_resp.clicked())
        && let Some(p) = prog_resp.interact_pointer_pos()
    {
        let frac = ((p.x - prog.left()) / prog.width()) as f64;
        action = VideoAction::Seek(frac * cell.info.duration_secs);
    }
    action
}
