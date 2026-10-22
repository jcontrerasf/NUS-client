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
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        "NUS Client",
        options,
        Box::new(|cc| {
            add_symbol_fallback(&cc.egui_ctx);
            let (cmd_tx, evt_rx) = ble::spawn(cc.egui_ctx.clone());
            Ok(Box::new(app::NusApp::new(cmd_tx, evt_rx)))
        }),
    )
}

/// En Windows, wgpu prueba Vulkan primero y algunos drivers/capas Vulkan
/// provocan un access violation al crear la ventana. DX12 (+ GL de respaldo)
/// está siempre disponible en Windows 10/11. `WGPU_BACKEND` sigue mandando.
fn wgpu_options() -> eframe::WgpuConfiguration {
    let mut opts = eframe::WgpuConfiguration::default();
    if cfg!(windows)
        && std::env::var_os("WGPU_BACKEND").is_none()
        && let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut opts.wgpu_setup
    {
        setup.instance_descriptor.backends =
            eframe::wgpu::Backends::DX12 | eframe::wgpu::Backends::GL;
    }
    opts
}

/// Las fuentes de egui no traen algunos símbolos de la UI (●, ➤, ⏺…), que
/// salen como cuadros. En Windows se usa Segoe UI Symbol como respaldo.
fn add_symbol_fallback(ctx: &egui::Context) {
    if !cfg!(windows) {
        return;
    }
    let dir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let path = std::path::Path::new(&dir)
        .join("Fonts")
        .join("seguisym.ttf");
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
        .into_iter()
        .map(|family| egui::epaint::text::InsertFontFamily {
            family,
            priority: egui::epaint::text::FontPriority::Lowest,
        })
        .collect();
    ctx.add_font(egui::epaint::text::FontInsert::new(
        "seguisym",
        egui::FontData::from_owned(bytes),
        families,
    ));
}
