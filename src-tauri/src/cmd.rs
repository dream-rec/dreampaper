use chrono::Utc;
use tauri::{AppHandle, Emitter, State};

use crate::core::asset::AssetUpload;
use crate::core::config::AppConfig;
use crate::core::doc::{DocumentChunkHit, DocumentSummary};
use crate::core::job::JobRecord;
use crate::core::tpl::{TemplatePackSummary, TemplateSummary};
use crate::core::update::ReleaseInfo;
use crate::core::workbench::asset::CleanupResult;
use crate::core::workbench::color::RegionAnalysis;
use crate::core::workbench::doc::{ProjectDoc, Shape};
use crate::core::workbench::geom::PixelRect;
use crate::core::workbench::ocr::OcrPackageStatus;
use crate::core::workbench::project::{DeleteResult, ExportRecord, ProjectDetail, ProjectSummary};
use crate::core::workbench::render::ExportPreview;
use crate::core::workbench::sidecar::{OcrEngineStatus, RecognizeResult};
use crate::core::workbench::text::{FontInfo, TextLayout, TextSpec};
use crate::core::workbench::{OpenResult, ProjectSource, StorageStats};
use crate::error::{AppError, AppResult};
use crate::event::JobEventPayload;
use crate::state::AppState;

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppResult<AppConfig> {
    state.core().get_config()
}

#[tauri::command]
pub fn save_config(state: State<'_, AppState>, config: AppConfig) -> AppResult<AppConfig> {
    state.core().save_config(config)
}

#[tauri::command]
pub async fn check_update(app: AppHandle, state: State<'_, AppState>) -> AppResult<ReleaseInfo> {
    let current_version = app.package_info().version.to_string();
    let config = state.core().get_config()?;
    crate::core::update::check(&current_version, config.proxy_url.as_deref()).await
}

#[tauri::command]
pub fn list_templates(
    state: State<'_, AppState>,
    kind: String,
    query: String,
) -> AppResult<Vec<TemplateSummary>> {
    state.core().list_templates(kind, query)
}

#[tauri::command]
pub fn import_asset(
    state: State<'_, AppState>,
    filename: String,
    mime_type: String,
    bytes: Vec<u8>,
) -> AppResult<AssetUpload> {
    state.core().import_asset(filename, mime_type, bytes)
}

#[tauri::command]
pub fn import_template_image(
    state: State<'_, AppState>,
    filename: String,
    mime_type: String,
    bytes: Vec<u8>,
    kind: String,
    category: Option<String>,
    visual_intent: Option<String>,
    content_summary: Option<String>,
) -> AppResult<TemplateSummary> {
    state.core().import_template_image(
        filename,
        mime_type,
        bytes,
        kind,
        category,
        visual_intent,
        content_summary,
    )
}

#[tauri::command]
pub fn import_template_pack(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<TemplatePackSummary> {
    state.core().import_template_pack(path)
}

#[tauri::command]
pub fn import_document(state: State<'_, AppState>, path: String) -> AppResult<DocumentSummary> {
    state.core().import_document(path)
}

#[tauri::command]
pub fn import_document_asset(
    state: State<'_, AppState>,
    asset_id: String,
) -> AppResult<DocumentSummary> {
    state.core().import_document_asset(asset_id)
}

#[tauri::command]
pub fn search_documents(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> AppResult<Vec<DocumentChunkHit>> {
    state.core().search_documents(query, limit.unwrap_or(8))
}

#[tauri::command]
pub fn create_job(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: serde_json::Value,
) -> AppResult<JobRecord> {
    let record = state.core().create_job(payload)?;
    let _ = app.emit(
        "job://queued",
        JobEventPayload {
            job_id: record.id.clone(),
            status: record.status.clone(),
            stage: record.stage.clone().unwrap_or_else(|| "queued".to_string()),
            message: record
                .message
                .clone()
                .unwrap_or_else(|| "任务已排队".to_string()),
            timestamp: Utc::now(),
        },
    );
    crate::core::pipeline::execute::spawn(app, state.core_arc(), record.id.clone());
    Ok(record)
}

#[tauri::command]
pub fn get_job(state: State<'_, AppState>, id: String) -> AppResult<JobRecord> {
    state.core().get_job(id)
}

#[tauri::command]
pub fn list_jobs(
    state: State<'_, AppState>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> AppResult<Vec<JobRecord>> {
    state
        .core()
        .list_jobs(limit.unwrap_or(50), offset.unwrap_or(0))
}

/// Open a generated image with the OS default viewer.
///
/// The webview cannot do this on its own: inside Tauri a plain
/// `<a target="_blank">` is a silent no-op (no window.open handler, so no
/// navigation and no error), so the result preview has to round-trip through
/// the backend. `open_path` is called from Rust, which bypasses the plugin ACL
/// scope — that only gates calls arriving from the frontend, and the path here
/// comes from our own asset table rather than from the webview.
#[tauri::command]
pub fn open_artifact(state: State<'_, AppState>, artifact_id: String) -> AppResult<()> {
    let file = state.core().asset_file(&artifact_id)?;
    if !file.path.exists() {
        return Err(AppError::new("asset_file_missing", "资源文件已不在磁盘上"));
    }
    tauri_plugin_opener::open_path(&file.path, None::<&str>)
        .map_err(|error| AppError::new("open_artifact_failed", error.to_string()))
}

#[tauri::command]
pub fn cancel_job(app: AppHandle, state: State<'_, AppState>, id: String) -> AppResult<JobRecord> {
    let record = state.core().cancel_job(id)?;
    let _ = app.emit(
        "job://stage",
        JobEventPayload {
            job_id: record.id.clone(),
            status: record.status.clone(),
            stage: record
                .stage
                .clone()
                .unwrap_or_else(|| "cancelled".to_string()),
            message: record
                .message
                .clone()
                .unwrap_or_else(|| "任务已停止".to_string()),
            timestamp: Utc::now(),
        },
    );
    Ok(record)
}

#[tauri::command]
pub fn resume_job(app: AppHandle, state: State<'_, AppState>, id: String) -> AppResult<JobRecord> {
    let record = state.core().resume_job(id)?;
    let _ = app.emit(
        "job://stage",
        JobEventPayload {
            job_id: record.id.clone(),
            status: record.status.clone(),
            stage: record
                .stage
                .clone()
                .unwrap_or_else(|| "resuming".to_string()),
            message: record
                .message
                .clone()
                .unwrap_or_else(|| "继续上次进度".to_string()),
            timestamp: Utc::now(),
        },
    );
    crate::core::pipeline::execute::spawn(app, state.core_arc(), record.id.clone());
    Ok(record)
}

#[tauri::command]
pub fn delete_templates(state: State<'_, AppState>, ids: Vec<String>) -> AppResult<usize> {
    state.core().delete_templates(ids)
}

#[tauri::command]
pub fn delete_job(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.core().delete_job(id)
}

/// 优/良/差 on a finished job; `rating: null` clears it.
#[tauri::command]
pub fn rate_job(
    state: State<'_, AppState>,
    id: String,
    rating: Option<String>,
) -> AppResult<JobRecord> {
    state.core().rate_job(id, rating)
}

#[tauri::command]
pub fn save_asset(state: State<'_, AppState>, asset_id: String, path: String) -> AppResult<()> {
    state
        .core()
        .export_asset(&asset_id, std::path::Path::new(&path))
}

// ---- Workbench -------------------------------------------------------------
//
// Commands stay thin: deserialize, pick the service, convert errors. Anything
// that decodes or composites an image runs on a blocking thread so a
// 5504×3072 export never freezes the window.

async fn blocking<T: Send + 'static>(
    state: &State<'_, AppState>,
    work: impl FnOnce(std::sync::Arc<crate::core::Core>) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let core = state.core_arc();
    tauri::async_runtime::spawn_blocking(move || work(core))
        .await
        .map_err(|error| AppError::new("task_join_failed", error.to_string()))?
}

#[tauri::command]
pub fn list_workbench_projects(state: State<'_, AppState>) -> AppResult<Vec<ProjectSummary>> {
    state.core().workbench().list_projects()
}

#[tauri::command]
pub async fn open_workbench_project(
    state: State<'_, AppState>,
    source: ProjectSource,
    force_new: Option<bool>,
) -> AppResult<OpenResult> {
    blocking(&state, move |core| {
        core.workbench().open(source, force_new.unwrap_or(false))
    })
    .await
}

#[tauri::command]
pub fn copy_workbench_project(
    state: State<'_, AppState>,
    project_id: String,
) -> AppResult<ProjectDetail> {
    state.core().workbench().copy_project(&project_id)
}

#[tauri::command]
pub fn get_workbench_project(
    state: State<'_, AppState>,
    project_id: String,
) -> AppResult<ProjectDetail> {
    state.core().workbench().get_project(&project_id)
}

#[tauri::command]
pub fn save_workbench_project(
    state: State<'_, AppState>,
    project_id: String,
    base_revision: u64,
    document: ProjectDoc,
) -> AppResult<ProjectDetail> {
    state
        .core()
        .workbench()
        .save_project(&project_id, base_revision, document)
}

#[tauri::command]
pub fn rename_workbench_project(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> AppResult<ProjectSummary> {
    state.core().workbench().rename_project(&project_id, &name)
}

#[tauri::command]
pub fn delete_workbench_project(
    state: State<'_, AppState>,
    project_id: String,
) -> AppResult<DeleteResult> {
    state.core().workbench().delete_project(&project_id)
}

#[tauri::command]
pub async fn analyze_workbench_region(
    state: State<'_, AppState>,
    project_id: String,
    rect: PixelRect,
    shape: Shape,
) -> AppResult<RegionAnalysis> {
    blocking(&state, move |core| {
        core.workbench().analyze_region(&project_id, rect, shape)
    })
    .await
}

#[tauri::command]
pub async fn measure_workbench_text(
    state: State<'_, AppState>,
    spec: TextSpec,
) -> AppResult<TextLayout> {
    blocking(&state, move |core| core.workbench().measure_text(spec)).await
}

#[tauri::command]
pub async fn list_workbench_fonts(
    state: State<'_, AppState>,
    sample: Option<String>,
) -> AppResult<Vec<FontInfo>> {
    blocking(&state, move |core| {
        Ok(core.workbench().list_fonts(sample.as_deref()))
    })
    .await
}

#[tauri::command]
pub async fn preview_workbench_export(
    state: State<'_, AppState>,
    project_id: String,
    document: ProjectDoc,
) -> AppResult<ExportPreview> {
    blocking(&state, move |core| {
        core.workbench().export_preview(&project_id, &document)
    })
    .await
}

/// Progress arrives on `workbench://export-progress` tagged with the project.
#[tauri::command]
pub async fn export_workbench_project(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    document: ProjectDoc,
    path: String,
) -> AppResult<ExportRecord> {
    blocking(&state, move |core| {
        let emitter = app.clone();
        let tag = project_id.clone();
        let mut progress = |update: crate::core::workbench::render::ExportProgress| {
            let _ = emitter.emit(
                "workbench://export-progress",
                serde_json::json!({ "project_id": tag, "stage": update.stage, "percent": update.percent }),
            );
        };
        core.workbench().export_with_progress(
            &project_id,
            &document,
            std::path::Path::new(&path),
            &mut progress,
        )
    })
    .await
}

#[tauri::command]
pub fn workbench_storage(state: State<'_, AppState>) -> AppResult<StorageStats> {
    state.core().workbench().storage()
}

#[tauri::command]
pub fn cleanup_workbench_assets(state: State<'_, AppState>) -> AppResult<CleanupResult> {
    state.core().workbench().cleanup()
}

/// Package (downloadable models) and engine (bundled sidecar + runtime)
/// status in one payload for the settings page and the workbench.
#[derive(serde::Serialize)]
pub struct OcrStatus {
    #[serde(flatten)]
    pub package: OcrPackageStatus,
    pub engine: OcrEngineStatus,
}

#[tauri::command]
pub fn get_ocr_package_status(state: State<'_, AppState>) -> OcrStatus {
    let core = state.core();
    OcrStatus {
        package: core.ocr.status(&core.app_data),
        engine: core.sidecar.status(),
    }
}

/// D1: OCR on a project region. The webview sends coordinates only; the
/// crop, padding and sidecar round trip happen in Rust off the UI thread.
#[tauri::command]
pub async fn recognize_workbench_region(
    state: State<'_, AppState>,
    project_id: String,
    rect: PixelRect,
    background: Option<String>,
    request_id: u64,
) -> AppResult<RecognizeResult> {
    blocking(&state, move |core| {
        core.recognize_region(&project_id, rect, background.as_deref(), request_id)
    })
    .await
}

#[tauri::command]
pub fn cancel_workbench_ocr(state: State<'_, AppState>, request_id: u64) -> AppResult<bool> {
    state.core().sidecar.cancel(request_id)
}

/// Start the model download; progress arrives on `ocr://progress`.
#[tauri::command]
pub fn install_ocr_package(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    let core = state.core();
    let proxy = core.get_config().ok().and_then(|config| config.proxy_url);
    core.ocr
        .start_install(core.app_data.clone(), proxy, move |progress| {
            let _ = app.emit("ocr://progress", progress);
        })
}

#[tauri::command]
pub fn cancel_ocr_package_install(state: State<'_, AppState>) -> AppResult<()> {
    state.core().ocr.cancel_install()
}

/// Removing models while the sidecar holds them open would fail on Windows
/// and leave a half-deleted package, so the engine is stopped first.
#[tauri::command]
pub async fn remove_ocr_package(state: State<'_, AppState>) -> AppResult<()> {
    blocking(&state, move |core| {
        core.sidecar.shutdown();
        core.ocr.remove(&core.app_data)
    })
    .await
}
