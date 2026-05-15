#![cfg_attr(
    all(windows, feature = "gui", not(debug_assertions)),
    windows_subsystem = "windows"
)]

fn main() -> anyhow::Result<()> {
    launch_app()
}

#[cfg(feature = "gui")]
fn launch_app() -> anyhow::Result<()> {
    biscotti::gui::run();
    Ok(())
}

#[cfg(not(feature = "gui"))]
fn launch_app() -> anyhow::Result<()> {
    println!("Biscotti workspace initialized.");
    println!("Build with --features gui to launch the GPUI shell.");
    Ok(())
}
