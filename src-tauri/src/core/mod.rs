pub mod asset;
pub mod cancel;
pub mod config;
pub mod doc;
pub mod job;
pub mod memory;
pub mod model;
pub mod net;
pub mod pipeline;
pub mod prompt;
pub mod search;
pub mod store;
pub mod thumb;
pub mod tpl;
pub mod update;
pub mod workbench;

use std::path::PathBuf;

use crate::error::AppResult;

use self::asset::{AssetFile, AssetService};
use self::cancel::CancelRegistry;
use self::config::{AppConfig, ConfigService};
use self::doc::{DocumentChunkHit, DocumentService, DocumentSummary};
use self::job::{JobRecord, JobService};
use self::prompt::PromptStore;
use self::store::Store;
use self::tpl::{TemplateFile, TemplatePackSummary, TemplateService, TemplateSummary};
use self::workbench::WorkbenchService;

pub struct Core {
    pub app_data: PathBuf,
    pub store: Store,
    pub prompts: PromptStore,
    pub cancels: CancelRegistry,
    /// Directory holding the bundled fallback font (a Tauri resource).
    pub font_root: PathBuf,
    /// Decoded workbench sources, fonts and OCR download state outlive a
    /// single command, so they live here rather than on the service.
    pub sources: workbench::source::SourceCache,
    pub fonts: workbench::text::FontHandle,
    pub ocr: workbench::ocr::OcrPackages,
    /// The native OCR sidecar (process supervisor + engine location).
    pub sidecar: workbench::sidecar::OcrSidecar,
}

impl Core {
    pub fn new(
        app_data: PathBuf,
        prompt_root: PathBuf,
        font_root: PathBuf,
        sidecar: workbench::sidecar::SidecarLocation,
    ) -> AppResult<Self> {
        let store = Store::initialize(&app_data)?;
        let core = Self {
            app_data,
            store,
            prompts: PromptStore::new(prompt_root),
            cancels: CancelRegistry::default(),
            font_root,
            sources: workbench::source::SourceCache::default(),
            fonts: workbench::text::FontHandle::default(),
            ocr: workbench::ocr::OcrPackages::default(),
            sidecar: workbench::sidecar::OcrSidecar::new(sidecar),
        };
        core.workbench().bootstrap()?;
        Ok(core)
    }

    /// OCR on a project region (D1): resolve the snapshot, cut and pad the
    /// crop here, hand it to the sidecar and map the answer back. Blocks.
    pub fn recognize_region(
        &self,
        project_id: &str,
        rect: workbench::geom::PixelRect,
        background: Option<&str>,
        request_id: u64,
    ) -> AppResult<workbench::sidecar::RecognizeResult> {
        let models = self.ocr.model_paths(&self.app_data)?;
        let (source, rect) = self.workbench().region_source(project_id, rect)?;
        let background = match background {
            Some(value) => workbench::sidecar::parse_hex_color(value).ok_or_else(|| {
                crate::error::AppError::new("workbench_color_invalid", "背景色格式无效")
            })?,
            None => {
                let analysis =
                    workbench::color::analyze(&source, &rect, workbench::doc::Shape::Rect);
                workbench::sidecar::parse_hex_color(&analysis.color).unwrap_or([255, 255, 255])
            }
        };
        self.sidecar
            .recognize(&models, &source, rect, background, request_id)
    }

    pub fn workbench(&self) -> WorkbenchService<'_> {
        WorkbenchService::new(
            &self.store,
            &self.app_data,
            &self.font_root,
            &self.sources,
            &self.fonts,
        )
    }

    pub fn get_config(&self) -> AppResult<AppConfig> {
        ConfigService::new(&self.store).get_config()
    }

    pub fn save_config(&self, config: AppConfig) -> AppResult<AppConfig> {
        ConfigService::new(&self.store).save_config(config)
    }

    pub fn import_asset(
        &self,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
    ) -> AppResult<asset::AssetUpload> {
        AssetService::new(&self.store, &self.app_data).import_asset(filename, mime_type, bytes)
    }

    pub fn asset_file(&self, id: &str) -> AppResult<AssetFile> {
        AssetService::new(&self.store, &self.app_data).asset_file(id)
    }

    pub fn export_asset(&self, id: &str, target: &std::path::Path) -> AppResult<()> {
        AssetService::new(&self.store, &self.app_data).export_asset(id, target)
    }

    pub fn list_templates(&self, kind: String, query: String) -> AppResult<Vec<TemplateSummary>> {
        TemplateService::new(&self.store, &self.app_data).list_templates(kind, query)
    }

    pub fn import_template_image(
        &self,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
        kind: String,
        category: Option<String>,
        visual_intent: Option<String>,
        content_summary: Option<String>,
    ) -> AppResult<TemplateSummary> {
        TemplateService::new(&self.store, &self.app_data).import_template_image(
            filename,
            mime_type,
            bytes,
            kind,
            category,
            visual_intent,
            content_summary,
        )
    }

    pub fn import_template_pack(&self, path: String) -> AppResult<TemplatePackSummary> {
        TemplateService::new(&self.store, &self.app_data).import_template_pack(path)
    }

    pub fn template_file(&self, id: &str) -> AppResult<TemplateFile> {
        TemplateService::new(&self.store, &self.app_data).template_file(id)
    }

    pub fn import_document(&self, path: String) -> AppResult<DocumentSummary> {
        DocumentService::new(&self.store, &self.app_data).import_document(path)
    }

    pub fn import_document_asset(&self, asset_id: String) -> AppResult<DocumentSummary> {
        let asset = self.asset_file(&asset_id)?;
        DocumentService::new(&self.store, &self.app_data)
            .import_document(asset.path.to_string_lossy().to_string())
    }

    pub fn search_documents(
        &self,
        query: String,
        limit: usize,
    ) -> AppResult<Vec<DocumentChunkHit>> {
        DocumentService::new(&self.store, &self.app_data).search_documents(query, limit)
    }

    pub fn create_job(&self, payload: serde_json::Value) -> AppResult<JobRecord> {
        JobService::new(&self.store).create_job(payload)
    }

    pub fn get_job(&self, id: String) -> AppResult<JobRecord> {
        JobService::new(&self.store).get_job(id)
    }

    pub fn list_jobs(&self, limit: usize, offset: usize) -> AppResult<Vec<JobRecord>> {
        JobService::new(&self.store).list_jobs(limit, offset)
    }

    pub fn cancel_job(&self, id: String) -> AppResult<JobRecord> {
        let jobs = JobService::new(&self.store);
        let record = jobs.get_job(id.clone())?;
        if matches!(record.status.as_str(), "succeeded" | "failed" | "cancelled") {
            return Ok(record);
        }
        self.cancels.cancel(&id);
        jobs.cancel(&id, "任务已停止")?;
        jobs.get_job(id)
    }

    /// 继续一个失败或已停止的任务：同一条记录重新跑，已完成的部分从缓存回放。
    pub fn resume_job(&self, id: String) -> AppResult<JobRecord> {
        JobService::new(&self.store).resume(&id)
    }

    pub fn delete_job(&self, id: String) -> AppResult<()> {
        JobService::new(&self.store).delete(&self.app_data, &id)
    }

    /// Tag a finished job 优/良/差 (`good`/`fair`/`poor`), or clear the tag
    /// with `None`. Only jobs that reached implement have a case to tag.
    pub fn rate_job(&self, id: String, rating: Option<String>) -> AppResult<JobRecord> {
        let parsed = match rating
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(value) => Some(memory::Rating::parse(value).ok_or_else(|| {
                crate::error::AppError::new("invalid_rating", "评分只能是 good / fair / poor")
            })?),
            None => None,
        };
        let jobs = JobService::new(&self.store);
        let record = jobs.get_job(id.clone())?;
        if record.status != "succeeded" {
            return Err(crate::error::AppError::new(
                "job_not_finished",
                "只能评价已完成的任务",
            ));
        }
        memory::MemoryService::new(&self.store).rate(&id, parsed)?;
        jobs.get_job(id)
    }

    pub fn delete_templates(&self, ids: Vec<String>) -> AppResult<usize> {
        TemplateService::new(&self.store, &self.app_data).delete_templates(&ids)
    }
}
