// Without this, a release build pops a console window next to the main window:
// Rust links against the console subsystem by default, so Windows allocates one.
// That console also hosts the process, so closing it kills the app. Kept in debug
// builds because `tauri dev` still needs println/panic output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK 2.50 asks AT-SPI synchronously on every focus change. Moving
    // between the title and the method field, or Tab, waits on that bus, which
    // is the lag. This app does not expose an assistive-technology tree.
    #[cfg(target_os = "linux")]
    std::env::set_var("GTK_A11Y", "none");
    dreampaper_lib::run();
}
