#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod runtime;
mod ui;

fn main() -> eframe::Result {
    let wav = std::env::args().nth(1);
    eframe::run_native(
        "wsjt-rusty — Native FT8 receiver",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 860.0])
                .with_min_inner_size([740.0, 560.0]),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, wav)))),
    )
}
