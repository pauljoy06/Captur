#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod capture;
mod evidence;
mod export;

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
mod encoding;
#[cfg(target_os = "windows")]
mod platform;
#[cfg(target_os = "windows")]
mod ui;

#[cfg(target_os = "windows")]
fn main() -> eframe::Result<()> {
    platform::windows::configure_process()?;
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title(platform::windows::WINDOW_TITLE)
            .with_inner_size([440.0, 620.0])
            .with_min_inner_size([360.0, 420.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "ProofSnip",
        native_options,
        Box::new(|cc| Ok(Box::new(app::ProofSnipApp::new(cc)))),
    )
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("ProofSnip is a Windows-only application. Build x86_64-pc-windows-msvc.");
}
