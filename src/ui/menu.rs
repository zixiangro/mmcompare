use eframe::egui;

/// Draw the menu bar. Sets `pending_open` when the user clicks "Open...".
pub fn menu_bar(ui: &mut egui::Ui, pending_open: &mut bool) {
    egui::Panel::top("menu_bar")
        .resizable(false)
        .show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                egui::menu::MenuButton::new("File").ui(ui, |ui| {
                    if ui.button("Open...").clicked() {
                        *pending_open = true;
                        ui.close();
                    }
                });
            });
        });
}
