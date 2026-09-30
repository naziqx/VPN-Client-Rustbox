//! RustBox desktop GUI (egui/eframe).
// A GUI app on Windows must not open a console window next to it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod dialogs;
mod tasks;
mod theme;
mod view;

#[cfg(test)]
mod screenshots;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RustBox")
            .with_app_id("rustbox")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([700.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RustBox",
        options,
        Box::new(|cc| Ok(Box::new(app::RustBoxApp::new(cc)?))),
    )
}
