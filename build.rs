// Embed vibezen.ico into the exe. The window itself only gets a small (title bar) icon
// from winit, so the taskbar, pinned shortcuts, Alt+Tab and Explorer use this one.
fn main() {
    println!("cargo:rerun-if-changed=vibezen.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("vibezen.ico");
        res.compile().expect("embedding vibezen.ico");
    }
}
