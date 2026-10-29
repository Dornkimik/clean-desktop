#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod lnk;
mod preview;
mod scan;
mod theme;
mod util;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Clean Desktop")
            .with_inner_size([1240.0, 760.0])
            .with_min_inner_size([1000.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Clean Desktop",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
