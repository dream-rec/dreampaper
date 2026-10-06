mod cmd;
mod core;
mod error;
mod event;
mod probe;
mod protocol;
mod state;
mod window;

use tauri::Manager;

pub fn run() {
    if let Some(code) = probe::run_if_requested() {
        std::process::exit(code);
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("dp-asset", protocol::asset_response)
        .register_asynchronous_uri_scheme_protocol("dp-template", protocol::template_response)
        .register_asynchronous_uri_scheme_protocol("dp-workbench", protocol::workbench_response)
        .setup(|app| {
            let state = state::AppState::new(app.handle())
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
            app.manage(state);
            // Windows 11 needs the rounded-corner preference set explicitly;
            // no-op elsewhere. See window.rs.
            if let Some(main) = app.get_webview_window("main") {
                window::round_corners(&main);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd::get_config,
            cmd::save_config,
            cmd::check_update,
            cmd::list_templates,
            cmd::import_asset,
            cmd::import_template_image,
            cmd::import_template_pack,
            cmd::import_document,
            cmd::import_document_asset,
            cmd::search_documents,
            cmd::create_job,
            cmd::get_job,
            cmd::list_jobs,
            cmd::cancel_job,
            cmd::resume_job,
            cmd::delete_job,
            cmd::rate_job,
            cmd::delete_templates,
            cmd::save_asset,
            cmd::open_artifact,
            cmd::list_workbench_projects,
            cmd::open_workbench_project,
            cmd::copy_workbench_project,
            cmd::get_workbench_project,
            cmd::save_workbench_project,
            cmd::rename_workbench_project,
            cmd::delete_workbench_project,
            cmd::analyze_workbench_region,
            cmd::recognize_workbench_region,
            cmd::cancel_workbench_ocr,
            cmd::measure_workbench_text,
            cmd::list_workbench_fonts,
            cmd::preview_workbench_export,
            cmd::export_workbench_project,
            cmd::workbench_storage,
            cmd::cleanup_workbench_assets,
            cmd::get_ocr_package_status,
            cmd::install_ocr_package,
            cmd::cancel_ocr_package_install,
            cmd::remove_ocr_package
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // The OCR sidecar must never outlive the app: stop it before the process
    // exits, whatever closed the last window.
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<state::AppState>() {
                state.core().sidecar.shutdown();
            }
        }
    });
}
