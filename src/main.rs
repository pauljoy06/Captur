#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod annotations;
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
    let start_hidden = std::env::args_os().any(|argument| argument == "--background");
    let app_icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/captur.png"))
        .expect("embedded Captur icon must be valid PNG data");
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(platform::windows::WINDOW_TITLE)
        .with_icon(app_icon)
        .with_inner_size([1040.0, 720.0])
        .with_min_inner_size([720.0, 520.0])
        .with_visible(true)
        .with_resizable(true);
    if start_hidden {
        viewport = viewport.with_position([-32_000.0, -32_000.0]);
    }
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Captur",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::CapturApp::new(cc, !start_hidden)))),
    )
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Captur is a Windows-only application. Build x86_64-pc-windows-msvc.");
}
