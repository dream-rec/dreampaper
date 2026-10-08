// Without this, a release build pops a console window next to the main window:
// Rust links against the console subsystem by default, so Windows allocates one.
// That console also hosts the process, so closing it kills the app. Kept in debug
// builds because `tauri dev` still needs println/panic output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Linux WebKitGTK only. Must run before GTK/WebKit starts.
    #[cfg(target_os = "linux")]
    {
        // AT-SPI is queried synchronously on every focus change, so moving
        // between text fields waits on that bus. This app has no AT tree.
        std::env::set_var("GTK_A11Y", "none");
        // These two are a common NVIDIA workaround in /etc/environment. They
        // force software compositing for every WebKit process, which is why an
        // installed .deb feels sticky next to Windows and macOS. The installed
        // app inherits the session environment; unset them for this process.
        std::env::remove_var("WEBKIT_DISABLE_COMPOSITING_MODE");
        std::env::remove_var("WEBKIT_DISABLE_DMABUF_RENDERER");
    }
    dreampaper_lib::run();
}
