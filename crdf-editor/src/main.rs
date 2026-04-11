use crdf_editor::app;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("crdf Editor"),
        ..Default::default()
    };
    eframe::run_native(
        "crdf Editor",
        options,
        Box::new(|_cc| Ok(Box::new(app::CrdfEditorApp::new()))),
    )
}
