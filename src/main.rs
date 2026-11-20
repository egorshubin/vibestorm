#![windows_subsystem = "windows"]
#![allow(deprecated)]
//! VibeZen — a lightweight native IDE (Rust + egui/OpenGL) with git and
//! parallel Claude Code agents, one per open project.

mod app;
mod attach;
mod claude;
mod claude_ui;
mod editor;
mod git;
mod highlight;
mod plan;
mod search;
mod settings;
mod theme;
mod tree;
mod util;

use eframe::egui;

fn main() -> eframe::Result {
    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("--write-icon") {
        if let Some(p) = std::env::args().nth(2) {
            let _ = util::write_icon(std::path::Path::new(&p));
        }
        return Ok(());
    }
    if arg.as_deref() == Some("--test-toast") {
        util::notify("VibeZen", "Notifications are working ✔", true, None);
        std::thread::sleep(std::time::Duration::from_secs(4));
        return Ok(());
    }
    let viewport = egui::ViewportBuilder::default()
        .with_title("VibeZen")
        .with_app_id("VibeZen")
        .with_inner_size([1400.0, 860.0])
        .with_maximized(true)
        .with_min_inner_size([820.0, 500.0])
        .with_icon(std::sync::Arc::new(util::app_icon()))
        .with_drag_and_drop(true);
    // OpenGL (glow) measured lighter than wgpu/DX12 here: ~150 MB vs ~420 MB, 10 vs 29 threads.
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "VibeZen",
        options,
        Box::new(move |cc| Ok(Box::new(app::IdeApp::new(cc, arg)))),
    )
}
