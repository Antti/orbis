#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod operations;
mod theme;
mod worker;

fn main() -> eframe::Result {
    let path = std::env::args_os()
        .skip(1)
        .find(|arg| !arg.to_string_lossy().starts_with("-psn_"));
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Orbis")
            .with_inner_size([640.0, 560.0])
            .with_min_inner_size([520.0, 440.0])
            .with_icon(eframe::egui::IconData::default())
            .with_app_id("io.github.Antti.Orbis.Desktop"),
        centered: true,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Orbis",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::OrbisApp::new(
                &cc.egui_ctx,
                path.map(Into::into),
            )))
        }),
    );
    if let Err(error) = &result {
        rfd::MessageDialog::new()
            .set_title("Orbis could not start")
            .set_description(format!("Could not open the application window.\n\n{error}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
    result
}
