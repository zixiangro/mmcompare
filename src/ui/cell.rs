use eframe::egui;

use crate::state::ImageInfo;

/// Draw a single image scaled to fit and centered within the given rect.
/// This is the "cell" — it only cares about displaying its own image.
pub fn draw_image(ui: &mut egui::Ui, img: &ImageInfo, rect: egui::Rect) {
    let img_w = img.size[0] as f32;
    let img_h = img.size[1] as f32;
    let scale = (rect.width() / img_w).min(rect.height() / img_h);
    let display = egui::vec2(img_w * scale, img_h * scale);

    let offset = (rect.size() - display) / 2.0;
    let img_rect = egui::Rect::from_min_size(rect.min + offset, display);

    ui.put(
        img_rect,
        egui::Image::from_texture(egui::load::SizedTexture::new(img.texture.id(), display)),
    );
}
