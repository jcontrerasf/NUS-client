// Sin consola en Windows para builds release.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod ble;
mod config;
mod log;
mod messages;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NUS Client")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "NUS Client",
        options,
        Box::new(|cc| {
            let (cmd_tx, evt_rx) = ble::spawn(cc.egui_ctx.clone());
            Ok(Box::new(app::NusApp::new(cmd_tx, evt_rx)))
        }),
    )
}
