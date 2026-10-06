use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::asset::protocol_url;
use super::store::Store;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobImage {
    pub name: String,
    pub url: String,
    /// Id in the asset table, so the workbench can snapshot the image without
    /// the frontend parsing the URL. Derived from the URL when reading rows
    /// written before this field existed.
    #[serde(default)]
    pub asset_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobEvent {
    pub stage: String,
    pub message: String,
    pub status: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobError {
    pub summary: String,
    pub code: String,
    pub stage: Option<String>,
    pub role: Option<String>,
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
    pub protocol: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub endpoint: Option<String>,
    pub http_status: Option<u16>,
    pub suggestion: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobDesignLog {
    pub step: String,
    pub label: String,
    pub status: String,
    pub content: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobRecord {
    pub id: String,
    pub mode: String,
    pub status: String,
    pub message: Option<String>,
    pub stage: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub images: Vec<JobImage>,
    pub events: Vec<JobEvent>,
    pub error: Option<JobError>,
    pub title: Option<String>,
    pub thumbnail: Option<String>,
    pub payload: Option<serde_json::Value>,
    /// Design-model output per step. Only `get_job` fills this — a step's answer
    /// runs to tens of kilobytes, so list rows stay light the same way `payload`
    /// does.
    pub design_logs: Vec<JobDesignLog>,
    /// The user's 优/良/差 tag on the finished result, kept on the job's
    /// memory case. `None` until rated, or when the job never reached implement.
    #[serde(default)]
    pub rating: Option<String>,
    /// 简单模式：产物是制图提示词、没有成品图，历史卡片因此显示参考母版。
    #[serde(default)]
    pub simple: bool,
}

pub fn simple_mode(envelope: &serde_json::Value) -> AppResult<bool> {
    match envelope.get("simple_mode") {
        None => Ok(false),
        Some(serde_json::Value::Bool(value)) => Ok(*value),
        _ => Err(AppError::new(
            "invalid_payload",
            "simple_mode must be a boolean",
        )),
    }
}

pub struct JobService<'a> {
    store: &'a Store,
}

impl<'a> JobService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn create_job(&self, payload: serde_json::Value) -> AppResult<JobRecord> {
        simple_mode(&payload)?;
        let mode = payload
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("paper_figure")
            .to_string();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let payload_json = serde_json::to_string(&payload)?;
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO jobs(id, mode, status, payload_json, message, stage, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, mode, "queued", payload_json, "任务已排队", "queued", now, now],
        )?;
        conn.execute(
            "INSERT INTO job_stages(job_id, stage, message, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, "queued", "任务已排队", "queued", now],
        )?;
        self.get_job(id)
    }

    pub fn get_job(&self, id: String) -> AppResult<JobRecord> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, mode, status, message, stage, created_at, updated_at, payload_json, result_json FROM jobs WHERE id = ?1",
        )?;
        let mut record = stmt
            .query_row(params![id], |row| {
                let mode: String = row.get(1)?;
                let payload_json: Option<String> = row.get(7)?;
                let result_json: Option<String> = row.get(8)?;
                Ok(JobRecord {
                    id: row.get(0)?,
                    mode: mode.clone(),
                    status: row.get(2)?,
                    message: row.get(3)?,
                    stage: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    images: Vec::new(),
                    events: Vec::new(),
                    error: None,
                    title: job_title(&mode, payload_json.clone()),
                    thumbnail: preview_url(result_json.as_deref(), payload_json.as_deref(), &mode),
                    simple: simple_flag(payload_json.as_deref()),
                    payload: payload_json.and_then(|raw| serde_json::from_str(&raw).ok()),
                    design_logs: Vec::new(),
                    rating: None,
                })
            })
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::new("job_not_found", "Job not found")
                }
                other => other.into(),
            })?;
        record.events = self.events_for_job(&record.id)?;
        record.images = self.images_for_job(&record.id)?;
        record.error = self.error_for_job(&record.id, &record.events)?;
        record.design_logs = self.design_logs_for_job(&record.id)?;
        record.rating = self.rating_for_job(&record.id)?;
        Ok(record)
    }

    pub fn list_jobs(&self, limit: usize, offset: usize) -> AppResult<Vec<JobRecord>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, mode, status, message, stage, created_at, updated_at, payload_json, result_json FROM jobs ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            let mode: String = row.get(1)?;
            let payload_json: Option<String> = row.get(7)?;
            let result_json: Option<String> = row.get(8)?;
            Ok(JobRecord {
                id: row.get(0)?,
                mode: mode.clone(),
                status: row.get(2)?,
                message: row.get(3)?,
                stage: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                images: Vec::new(),
                events: Vec::new(),
                error: None,
                title: job_title(&mode, payload_json.clone()),
                thumbnail: preview_url(result_json.as_deref(), payload_json.as_deref(), &mode),
                simple: simple_flag(payload_json.as_deref()),
                payload: None,
                design_logs: Vec::new(),
                rating: None,
            })
        })?;
        let mut records = rows.collect::<Result<Vec<_>, _>>()?;
        for record in &mut records {
            record.events = self.events_for_job(&record.id)?;
            record.images = self.images_for_job(&record.id)?;
            record.error = self.error_for_job(&record.id, &record.events)?;
            record.rating = self.rating_for_job(&record.id)?;
        }
        Ok(records)
    }

    pub fn mark_stage(
        &self,
        job_id: &str,
        stage: &str,
        message: &str,
        status: &str,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO job_stages(job_id, stage, message, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![job_id, stage, message, status, now],
        )?;
        let job_status = match status {
            "succeeded" | "failed" | "cancelled" => status,
            _ => "running",
        };
        conn.execute(
            "UPDATE jobs SET status = ?1, stage = ?2, message = ?3, updated_at = ?4 \
             WHERE id = ?5 AND status NOT IN ('succeeded', 'failed', 'cancelled')",
            params![job_status, stage, message, now, job_id],
        )?;
        Ok(())
    }

    /// Records the design model's answer for one step.
    ///
    /// Keyed by (job, step) so the repair rounds that re-ask the same step
    /// overwrite rather than pile up — the card the user reopens should show the
    /// answer the pipeline actually went on to use.
    pub fn record_design_log(
        &self,
        job_id: &str,
        step: &str,
        label: &str,
        status: &str,
        content: &str,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO job_design_logs(job_id, step, label, status, content, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(job_id, step) DO UPDATE SET \
             label = excluded.label, status = excluded.status, content = excluded.content",
            params![job_id, step, label, status, content, now],
        )?;
        Ok(())
    }

    fn rating_for_job(&self, job_id: &str) -> AppResult<Option<String>> {
        Ok(super::memory::MemoryService::new(self.store)
            .rating(job_id)?
            .map(|rating| rating.as_str().to_string()))
    }

    fn design_logs_for_job(&self, job_id: &str) -> AppResult<Vec<JobDesignLog>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT step, label, status, content, created_at FROM job_design_logs \
             WHERE job_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![job_id], |row| {
            Ok(JobDesignLog {
                step: row.get(0)?,
                label: row.get(1)?,
                status: row.get(2)?,
                content: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn payload(&self, job_id: &str) -> AppResult<serde_json::Value> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT payload_json FROM jobs WHERE id = ?1")?;
        let raw: String = stmt
            .query_row(params![job_id], |row| row.get(0))
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::new("job_not_found", "Job not found")
                }
                other => other.into(),
            })?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn finish(&self, job_id: &str, images: &[JobImage]) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let result = serde_json::json!({ "images": images });
        let conn = self.store.connection()?;
        conn.execute(
            "UPDATE jobs SET status = 'succeeded', stage = 'completed', message = '任务完成', \
             result_json = ?1, updated_at = ?2 \
             WHERE id = ?3 AND status NOT IN ('succeeded', 'failed', 'cancelled')",
            params![serde_json::to_string(&result)?, now, job_id],
        )?;
        drop(conn);
        self.mark_stage(job_id, "completed", "任务完成", "succeeded")
    }

    pub fn fail(
        &self,
        job_id: &str,
        message: &str,
        detail: Option<&serde_json::Value>,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let stage = self
            .events_for_job(job_id)?
            .last()
            .map(|event| event.stage.clone());
        let mut error = detail.cloned().unwrap_or_else(|| serde_json::json!({}));
        if let Some(object) = error.as_object_mut() {
            object.insert(
                "summary".to_string(),
                serde_json::Value::String(message.to_string()),
            );
            object.insert(
                "message".to_string(),
                serde_json::Value::String(message.to_string()),
            );
            object.insert(
                "failed_at".to_string(),
                serde_json::Value::String(now.clone()),
            );
            if object
                .get("stage")
                .map(serde_json::Value::is_null)
                .unwrap_or(true)
            {
                object.insert("stage".to_string(), serde_json::to_value(stage.clone())?);
            }
            if !object.contains_key("code") {
                object.insert(
                    "code".to_string(),
                    serde_json::Value::String("job_failed".to_string()),
                );
            }
        }
        let conn = self.store.connection()?;
        conn.execute(
            "UPDATE jobs SET error_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![serde_json::to_string(&error)?, now, job_id],
        )?;
        drop(conn);
        self.mark_stage(
            job_id,
            stage.as_deref().unwrap_or("failed"),
            message,
            "failed",
        )
    }

    pub fn cancel(&self, job_id: &str, message: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let error = serde_json::json!({ "message": message, "cancelled_at": now });
        let conn = self.store.connection()?;
        conn.execute(
            "UPDATE jobs SET error_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![serde_json::to_string(&error)?, now, job_id],
        )?;
        drop(conn);
        self.mark_stage(job_id, "cancelled", message, "cancelled")
    }

    /// 把失败或已停止的任务放回运行状态重跑。
    ///
    /// 输入、设计日志与已经生成的图片都保留，步骤缓存也还在，所以只有没跑完的部分
    /// 会真的重新调用。成功完成的任务不能“继续”，它在结果上已经没有可补的东西。
    pub fn resume(&self, job_id: &str) -> AppResult<JobRecord> {
        let now = Utc::now().to_rfc3339();
        let conn = self.store.connection()?;
        let changed = conn.execute(
            "UPDATE jobs SET status = 'running', stage = 'resuming', message = '继续上次进度', error_json = NULL, updated_at = ?1 \
             WHERE id = ?2 AND status IN ('failed', 'cancelled')",
            params![now, job_id],
        )?;
        drop(conn);
        if changed == 0 {
            // 先分清「没这个任务」与「这个任务不在可继续的状态」，报错才有用。
            self.get_job(job_id.to_string())?;
            return Err(AppError::new(
                "job_not_resumable",
                "只有失败或已停止的任务可以继续",
            ));
        }
        self.mark_stage(job_id, "resuming", "继续上次进度", "running")?;
        self.get_job(job_id.to_string())
    }

    /// Removes a settled job with its stage history, outputs and logs.
    /// Live jobs are rejected - cancel first. Missing directories are tolerated
    /// so partially-written jobs can still be cleaned up.
    pub fn delete(&self, app_data: &std::path::Path, job_id: &str) -> AppResult<()> {
        let conn = self.store.connection()?;
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM jobs WHERE id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::new("job_not_found", "Job not found")
                }
                other => other.into(),
            })?;
        if matches!(status.as_deref(), Some("queued") | Some("running")) {
            return Err(AppError::new(
                "job_active",
                "任务仍在进行中，请先停止再删除",
            ));
        }
        conn.execute("DELETE FROM job_stages WHERE job_id = ?1", params![job_id])?;
        conn.execute(
            "DELETE FROM job_design_logs WHERE job_id = ?1",
            params![job_id],
        )?;
        conn.execute("DELETE FROM jobs WHERE id = ?1", params![job_id])?;
        drop(conn);
        // The case would otherwise keep advising from a job the user threw away.
        super::memory::MemoryService::new(self.store).forget(job_id)?;
        for dir in ["outputs", "logs"] {
            let _ = std::fs::remove_dir_all(app_data.join(dir).join(job_id));
        }
        Ok(())
    }

    fn images_for_job(&self, job_id: &str) -> AppResult<Vec<JobImage>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT result_json FROM jobs WHERE id = ?1")?;
        let raw: Option<String> = stmt
            .query_row(params![job_id], |row| row.get(0))
            .unwrap_or(None);
        let Some(raw) = raw else {
            return Ok(Vec::new());
        };
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        let mut images: Vec<JobImage> =
            serde_json::from_value(value["images"].clone()).unwrap_or_default();
        for image in &mut images {
            image.url = normalize_asset_url(&image.url);
            if image.asset_id.is_none() {
                image.asset_id = asset_id_from_url(&image.url);
            }
        }
        Ok(images)
    }

    fn events_for_job(&self, job_id: &str) -> AppResult<Vec<JobEvent>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT stage, message, status, created_at FROM job_stages WHERE job_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![job_id], |row| {
            Ok(JobEvent {
                stage: row.get(0)?,
                message: row.get(1)?,
                status: row.get(2)?,
                timestamp: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn error_for_job(&self, job_id: &str, events: &[JobEvent]) -> AppResult<Option<JobError>> {
        let conn = self.store.connection()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT error_json FROM jobs WHERE id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .unwrap_or(None);
        let Some(raw) = raw else {
            return Ok(None);
        };
        let mut value: serde_json::Value = serde_json::from_str(&raw)?;
        if let Some(object) = value.as_object_mut() {
            if !object.contains_key("summary") {
                let summary = object
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("任务失败")
                    .to_string();
                object.insert("summary".to_string(), serde_json::Value::String(summary));
            }
            if !object.contains_key("code") {
                object.insert(
                    "code".to_string(),
                    serde_json::Value::String("job_failed".to_string()),
                );
            }
            if !object.contains_key("stage") {
                let stage = events
                    .iter()
                    .rev()
                    .find(|event| event.stage != "failed")
                    .map(|event| event.stage.clone());
                object.insert("stage".to_string(), serde_json::to_value(stage)?);
            }
        }
        Ok(serde_json::from_value(value).ok())
    }
}

/// Rebuild a stored output URL for the current platform.
///
/// `result_json` stores the full URL as built at generation time, and the URL
/// form is platform-bound (see `asset::protocol_url`). Fixing only the
/// generating side is not enough: jobs a Windows user ran before this fix, and
/// app data moved over from macOS, would keep showing broken images under
/// recent jobs. Resources are stored by id, so recovering the id from the URL
/// and rebuilding is enough — no database migration needed.
fn normalize_asset_url(url: &str) -> String {
    if !url.contains("dp-asset") {
        return url.to_string();
    }
    match url.rsplit('/').next() {
        Some(id) if !id.is_empty() => protocol_url("dp-asset", id),
        _ => url.to_string(),
    }
}

/// The asset id behind one of our own protocol URLs; external links have none.
fn asset_id_from_url(url: &str) -> Option<String> {
    if !url.contains("dp-asset") {
        return None;
    }
    url.rsplit('/')
        .next()
        .map(|id| id.split('?').next().unwrap_or(id))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn simple_flag(payload_json: Option<&str>) -> bool {
    // 宽松读法：只影响列表怎么显示；严格的布尔校验在真正执行任务时做。
    payload_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.get("simple_mode").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Derived display title for list rendering. Localization of the fallback
/// ("untitled") stays on the frontend; the backend only extracts.
fn job_title(mode: &str, payload_json: Option<String>) -> Option<String> {
    let raw = payload_json?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let payload = value.get("payload").unwrap_or(&value);
    let (field, limit) = if mode == "ppt_slide" {
        ("material_text", 40)
    } else {
        ("figure_title", 60)
    };
    let mut text = payload.get(field).and_then(|v| v.as_str()).map(str::trim);
    if (text.is_none() || text.is_some_and(str::is_empty)) && mode != "ppt_slide" {
        text = payload
            .get("section_description")
            .and_then(|v| v.as_str())
            .map(str::trim);
    }
    let first_line = text?.lines().next()?.trim();
    if first_line.is_empty() {
        return None;
    }
    Some(first_line.chars().take(limit).collect())
}

/// Width the history grid asks for. Cards are ~400 CSS px wide, so this still
/// has headroom on a HiDPI screen while costing ~1/45th of the source decode.
const THUMBNAIL_WIDTH: u32 = 800;

fn job_thumbnail(result_json: Option<&str>) -> Option<String> {
    let raw = result_json?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let url = value.get("images")?.get(0)?.get("url")?.as_str()?;
    if url.trim().is_empty() {
        return None;
    }
    Some(thumbnail_url(url))
}

/// 历史卡片的预览：有成品图用成品图；简单模式的产物是提示词、成品图永远不存在，
/// 于是退回所选参考母版。普通模式没有成品图只说明没跑完，不能拿参考图冒充结果。
fn preview_url(
    result_json: Option<&str>,
    payload_json: Option<&str>,
    mode: &str,
) -> Option<String> {
    job_thumbnail(result_json).or_else(|| {
        simple_flag(payload_json)
            .then(|| reference_thumbnail(payload_json, mode))
            .flatten()
    })
}

/// 同一件资源在两个协议下都走缩略图缓存，列表才不会整页解码大图。
fn thumbnail_url(url: &str) -> String {
    let normalized = normalize_asset_url(url);
    if normalized.contains("dp-asset") || normalized.contains("dp-template") {
        return format!("{normalized}?w={THUMBNAIL_WIDTH}");
    }
    // 只有我们自己的协议能重缩放；外面的链接原样发出去。
    normalized
}

/// 简单模式没有成品图（产物是制图提示词），卡片上退回它选中的那件参考图：
/// 母版/模板才是这个任务的视觉身份，比一个空图标有信息量。
fn reference_thumbnail(payload_json: Option<&str>, mode: &str) -> Option<String> {
    let raw = payload_json?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let payload = value.get("payload").unwrap_or(&value);
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    // 幻灯片：先看模板，再看上传的母版图；科研图是一组模板，取第一张。
    if mode == "ppt_slide" {
        if let Some(id) = text("template_id") {
            return Some(thumbnail_url(&protocol_url("dp-template", id)));
        }
        if let Some(id) = text("template_asset_id") {
            return Some(thumbnail_url(&protocol_url("dp-asset", id)));
        }
        return None;
    }
    let id = payload
        .get("template_ids")
        .and_then(|v| v.as_array())?
        .iter()
        .find_map(|item| item.as_str().map(str::trim))
        .filter(|id| !id.is_empty())?;
    Some(thumbnail_url(&protocol_url("dp-template", id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The top-level flag is a snapshot of the UI switch: absent means a job
    /// stored before Simple Mode existed, and anything but a real boolean is a
    /// malformed request rather than a truthy value.
    #[test]
    fn simple_mode_defaults_to_off_and_rejects_non_booleans() {
        assert!(
            !simple_mode(&serde_json::json!({"mode": "paper_figure"})).expect("旧任务默认普通模式")
        );
        assert!(simple_mode(&serde_json::json!({"simple_mode": true})).expect("显式开启"));
        assert!(!simple_mode(&serde_json::json!({"simple_mode": false})).expect("显式关闭"));
        for invalid in [
            serde_json::json!({"simple_mode": "true"}),
            serde_json::json!({"simple_mode": 1}),
            serde_json::json!({"simple_mode": null}),
        ] {
            let error = simple_mode(&invalid).expect_err("非布尔必须报错");
            assert_eq!(error.code, "invalid_payload");
        }
    }

    /// Old jobs hold the other platform's URL form; reads have to rebuild it
    /// into one this machine can load, or images under recent jobs stay broken
    /// after an upgrade. Non-dp-asset URLs pass through untouched.
    #[test]
    fn stored_image_urls_are_rebuilt_for_this_platform() {
        let expected = protocol_url("dp-asset", "abc-123");
        assert_eq!(
            normalize_asset_url("dp-asset://localhost/abc-123"),
            expected
        );
        assert_eq!(
            normalize_asset_url("http://dp-asset.localhost/abc-123"),
            expected
        );
        assert_eq!(
            normalize_asset_url("https://example.com/x.png"),
            "https://example.com/x.png",
            "external image links must not be rewritten"
        );
    }

    #[test]
    fn titles_follow_the_per_mode_extraction_rules() {
        let figure = serde_json::json!({
            "mode": "paper_figure",
            "payload": { "figure_title": "TriPath Overview", "section_description": "ignored" }
        })
        .to_string();
        assert_eq!(
            job_title("paper_figure", Some(figure)),
            Some("TriPath Overview".to_string())
        );

        let long: String = std::iter::repeat('x').take(120).collect();
        let fallback = serde_json::json!({
            "mode": "paper_figure",
            "payload": { "figure_title": "", "section_description": format!("{long}\nsecond line") }
        })
        .to_string();
        assert_eq!(
            job_title("paper_figure", Some(fallback)),
            Some(long.chars().take(60).collect())
        );

        let slide = serde_json::json!({
            "mode": "ppt_slide",
            "payload": { "material_text": format!("第一行素材\n{long}") }
        })
        .to_string();
        assert_eq!(
            job_title("ppt_slide", Some(slide)),
            Some("第一行素材".to_string())
        );

        assert_eq!(job_title("paper_figure", None), None);
        let empty = serde_json::json!({ "mode": "paper_figure", "payload": {} }).to_string();
        assert_eq!(job_title("paper_figure", Some(empty)), None);
        let blank = serde_json::json!({
            "mode": "paper_figure",
            "payload": { "figure_title": "   ", "section_description": "  \n  " }
        })
        .to_string();
        assert_eq!(job_title("paper_figure", Some(blank)), None);
    }

    #[test]
    fn thumbnails_come_from_the_first_result_image() {
        let result = serde_json::json!({
            "images": [{ "name": "figure.png", "url": "http://dp-asset.localhost/abc" }]
        })
        .to_string();
        // The grid gets the downscaled variant; the full image stays one query
        // strip away for the lightbox.
        assert_eq!(
            job_thumbnail(Some(result.as_str())),
            Some(format!("{}?w=800", protocol_url("dp-asset", "abc")))
        );
        assert_eq!(job_thumbnail(None), None);
        let no_images = serde_json::json!({}).to_string();
        assert_eq!(job_thumbnail(Some(no_images.as_str())), None);

        // An image hosted elsewhere cannot be rescaled by our protocol, so it
        // must come back untouched rather than with a query we cannot honour.
        let external = serde_json::json!({
            "images": [{ "name": "figure.png", "url": "https://example.com/x.png" }]
        })
        .to_string();
        assert_eq!(
            job_thumbnail(Some(external.as_str())),
            Some("https://example.com/x.png".to_string())
        );
    }

    #[test]
    fn a_simple_mode_card_falls_back_to_the_reference_image() {
        let no_result = || serde_json::json!({ "images": [] }).to_string();
        // 幻灯片：模板优先，其次是上传的母版图。
        let slide = serde_json::json!({
            "mode": "ppt_slide",
            "simple_mode": true,
            "payload": { "template_id": "tpl-1", "template_asset_id": "asset-1" }
        })
        .to_string();
        assert_eq!(
            preview_url(Some(&no_result()), Some(&slide), "ppt_slide"),
            Some(thumbnail_url(&protocol_url("dp-template", "tpl-1")))
        );
        let uploaded = serde_json::json!({
            "mode": "ppt_slide",
            "simple_mode": true,
            "payload": { "template_id": "", "template_asset_id": "asset-1" }
        })
        .to_string();
        assert_eq!(
            preview_url(Some(&no_result()), Some(&uploaded), "ppt_slide"),
            Some(thumbnail_url(&protocol_url("dp-asset", "asset-1")))
        );
        // 科研图是一组模板，取第一张。
        let figure = serde_json::json!({
            "mode": "paper_figure",
            "simple_mode": true,
            "payload": { "template_ids": ["tpl-2", "tpl-3"] }
        })
        .to_string();
        assert_eq!(
            preview_url(Some(&no_result()), Some(&figure), "paper_figure"),
            Some(thumbnail_url(&protocol_url("dp-template", "tpl-2")))
        );
        // 普通模式没跑完（没有成品图）：不拿参考图冒充结果。
        let normal = serde_json::json!({
            "mode": "paper_figure",
            "payload": { "template_ids": ["tpl-2"] }
        })
        .to_string();
        assert_eq!(
            preview_url(Some(&no_result()), Some(&normal), "paper_figure"),
            None
        );
        // 旧任务（无 payload / 无参考图）仍是空图标，不能报错。
        assert_eq!(preview_url(Some(&no_result()), None, "ppt_slide"), None);
        assert_eq!(
            preview_url(Some(&no_result()), Some("{ broken"), "ppt_slide"),
            None
        );
        // 有成品图时预览永远是成品图。
        let result = serde_json::json!({
            "images": [{ "url": "http://dp-asset.localhost/done" }]
        })
        .to_string();
        assert_eq!(
            preview_url(Some(&result), Some(&slide), "ppt_slide"),
            Some(thumbnail_url(&protocol_url("dp-asset", "done")))
        );
    }

    #[test]
    fn a_simple_mode_row_carries_its_flag_and_the_reference_preview() {
        let dir = service_dir("list-simple");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);
        let job = jobs
            .create_job(serde_json::json!({
                "mode": "ppt_slide",
                "simple_mode": true,
                "payload": { "template_id": "tpl-9", "material_text": "介绍一下 grokbot" }
            }))
            .expect("建任务");
        // 简单模式没有成品图。
        jobs.finish(&job.id, &[]).expect("收尾");

        let listed = jobs.list_jobs(20, 0).expect("列表");
        let row = listed.iter().find(|r| r.id == job.id).expect("找到任务");
        assert!(row.simple, "简单模式要在列表行上看得出来");
        assert_eq!(
            row.thumbnail.as_deref(),
            Some(thumbnail_url(&protocol_url("dp-template", "tpl-9")).as_str())
        );

        // 普通模式没有参考图时仍然是空预览，不能被误标。
        let normal = jobs
            .create_job(serde_json::json!({
                "mode": "paper_figure",
                "payload": { "figure_title": "普通任务" }
            }))
            .expect("建任务");
        jobs.finish(&normal.id, &[]).expect("收尾");
        let listed = jobs.list_jobs(20, 0).expect("列表");
        let row = listed.iter().find(|r| r.id == normal.id).expect("找到任务");
        assert!(!row.simple);
        assert_eq!(row.thumbnail, None);
    }

    #[test]
    fn list_jobs_carries_titles_and_thumbnails_without_failing_on_old_rows() {
        let dir = service_dir("list-meta");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);
        let job = jobs
            .create_job(serde_json::json!({
                "mode": "paper_figure",
                "payload": { "figure_title": "带标题的任务" }
            }))
            .expect("建任务");
        jobs.finish(
            &job.id,
            &[JobImage {
                name: "figure.png".to_string(),
                url: "dp-asset://localhost/t1".to_string(),
                asset_id: None,
            }],
        )
        .expect("收尾");

        let listed = jobs.list_jobs(20, 0).expect("列表");
        let with_meta = listed.iter().find(|r| r.id == job.id).expect("找到任务");
        assert_eq!(with_meta.title.as_deref(), Some("带标题的任务"));
        assert_eq!(
            with_meta.thumbnail.as_deref(),
            Some(format!("{}?w=800", protocol_url("dp-asset", "t1")).as_str())
        );

        // create_job without a payload body leaves extraction fields null, not errors.
        let bare = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建裸任务");
        let listed = jobs.list_jobs(20, 0).expect("列表");
        let bare_row = listed.iter().find(|r| r.id == bare.id).expect("找到裸任务");
        assert_eq!(bare_row.title, None);
        assert_eq!(bare_row.thumbnail, None);
    }

    #[test]
    fn get_job_returns_the_original_payload_for_replay() {
        let dir = service_dir("payload");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);
        let body = serde_json::json!({
            "mode": "paper_figure",
            "payload": { "figure_title": "回放", "template_ids": ["t1"] }
        });
        let job = jobs.create_job(body.clone()).expect("建任务");
        let job_id = job.id.clone();
        let loaded = jobs.get_job(job.id).expect("读任务");
        assert_eq!(loaded.payload, Some(body));
        // list rows stay light: payload is None there.
        let listed = jobs.list_jobs(20, 0).expect("列表");
        let row = listed.iter().find(|r| r.id == job_id).unwrap();
        assert_eq!(row.payload, None);
    }

    #[test]
    fn delete_removes_settled_jobs_with_files_and_rejects_live_ones() {
        let dir = service_dir("delete");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);

        let settled = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建任务");
        jobs.cancel(&settled.id, "任务已停止").expect("停止");
        std::fs::create_dir_all(dir.join("outputs").join(&settled.id)).expect("建产物目录");
        std::fs::create_dir_all(dir.join("logs").join(&settled.id)).expect("建日志目录");
        std::fs::write(
            dir.join("outputs").join(&settled.id).join("figure.png"),
            b"png",
        )
        .expect("写产物");

        let other = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建另一个任务");

        jobs.delete(&dir, &settled.id).expect("删除已结束任务");
        assert!(!dir.join("outputs").join(&settled.id).exists());
        assert!(!dir.join("logs").join(&settled.id).exists());
        let listed = jobs.list_jobs(20, 0).expect("列表");
        assert!(listed.iter().all(|r| r.id != settled.id), "行应已删除");
        assert!(listed.iter().any(|r| r.id == other.id), "其他任务不受影响");

        let live = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建进行中任务");
        jobs.mark_stage(&live.id, "paper_design", "设计中", "running")
            .expect("置为运行中");
        let error = jobs
            .delete(&dir, &live.id)
            .expect_err("运行中任务应拒绝删除");
        assert_eq!(error.code, "job_active");
        assert!(jobs
            .list_jobs(20, 0)
            .unwrap()
            .iter()
            .any(|r| r.id == live.id));

        // Missing output/log directories must not block deletion.
        jobs.cancel(&live.id, "任务已停止").expect("停止");
        jobs.delete(&dir, &live.id).expect("无目录也应能删除");
    }

    fn service_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-job-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn design_logs_are_kept_per_step_and_reopened_with_the_job() {
        let dir = service_dir("design-log");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);
        let job = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建任务");

        jobs.record_design_log(
            &job.id,
            "paper_structure",
            "分析母版结构",
            "succeeded",
            "{\"structure_plan\":1}",
        )
        .expect("记录第一步");
        jobs.record_design_log(
            &job.id,
            "paper_design",
            "映射内容并生成制图方案",
            "succeeded",
            "第一次的答案",
        )
        .expect("记录第二步");
        // A repair round re-asks the same step; the card must end up showing the
        // answer the pipeline went on to use, not both attempts.
        jobs.record_design_log(
            &job.id,
            "paper_design",
            "映射内容并生成制图方案",
            "succeeded",
            "修复后的答案",
        )
        .expect("重试覆盖同一步");

        let loaded = jobs.get_job(job.id.clone()).expect("读任务");
        assert_eq!(loaded.design_logs.len(), 2, "同一 step 不该堆两张卡");
        assert_eq!(loaded.design_logs[0].step, "paper_structure");
        assert_eq!(loaded.design_logs[0].label, "分析母版结构");
        assert_eq!(loaded.design_logs[1].content, "修复后的答案");
        assert!(loaded
            .design_logs
            .iter()
            .all(|log| !log.timestamp.is_empty()));

        // List rows stay light: a step's answer runs to tens of kilobytes.
        let listed = jobs.list_jobs(20, 0).expect("列表");
        let row = listed.iter().find(|r| r.id == job.id).expect("找到任务");
        assert!(row.design_logs.is_empty());

        // Deleting the job must take its logs with it, or the next job reusing
        // the id would inherit them.
        jobs.cancel(&job.id, "任务已停止").expect("停止");
        jobs.delete(&dir, &job.id).expect("删除");
        let orphans: i64 = store
            .connection()
            .expect("连接")
            .query_row(
                "SELECT count(*) FROM job_design_logs WHERE job_id = ?1",
                params![job.id],
                |row| row.get(0),
            )
            .expect("统计");
        assert_eq!(orphans, 0);
    }

    #[test]
    fn a_cancelled_job_is_not_resurrected_by_late_reports() {
        let dir = service_dir("cancel");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);
        let job = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建任务");

        jobs.mark_stage(&job.id, "paper_design", "设计中", "running")
            .expect("上报阶段");
        jobs.cancel(&job.id, "任务已停止").expect("停止");

        jobs.mark_stage(&job.id, "paper_implement", "制图中", "running")
            .expect("迟到的上报");
        jobs.finish(
            &job.id,
            &[JobImage {
                name: "figure.png".to_string(),
                url: "dp-asset://x".to_string(),
                asset_id: None,
            }],
        )
        .expect("迟到的收尾");

        let after = jobs.get_job(job.id.clone()).expect("读任务");
        assert_eq!(after.status, "cancelled");
        assert_eq!(after.stage.as_deref(), Some("cancelled"));
        assert!(after.images.is_empty(), "停止之后不该再把产出挂回这条任务");
    }

    /// 失败与已停止的任务可以继续；还在跑和已经成功的不能。
    #[test]
    fn only_settled_unsuccessful_jobs_can_be_resumed() {
        let dir = service_dir("resume");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);
        let job = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建任务");

        // 跑着的任务不能“继续”：它已经在跑了。
        let error = jobs.resume(&job.id).expect_err("运行中的任务不可继续");
        assert_eq!(error.code, "job_not_resumable");

        jobs.mark_stage(&job.id, "paper_design", "生成制图方案", "running")
            .expect("推进阶段");
        jobs.fail(&job.id, "HTTP 500", None).expect("失败");
        let resumed = jobs.resume(&job.id).expect("失败的任务可以继续");
        assert_eq!(resumed.status, "running");
        assert_eq!(resumed.stage.as_deref(), Some("resuming"));
        assert_eq!(resumed.message.as_deref(), Some("继续上次进度"));
        assert!(resumed.error.is_none(), "继续之后不该还挂着上一次的报错");
        // 输入与已经产生的日志都留着，重跑时才能接着用。
        assert!(resumed.payload.is_some());
        assert_eq!(
            resumed.events.last().map(|event| event.status.as_str()),
            Some("running")
        );

        // 停止过的任务同样可以继续。
        jobs.cancel(&job.id, "任务已停止").expect("停止");
        let resumed = jobs.resume(&job.id).expect("已停止的任务可以继续");
        assert_eq!(resumed.status, "running");

        // 成功完成之后没有可补的东西。
        jobs.finish(&job.id, &[]).expect("完成");
        let error = jobs.resume(&job.id).expect_err("成功的任务不可继续");
        assert_eq!(error.code, "job_not_resumable");

        let missing = jobs.resume("没有这个任务").expect_err("不存在的任务");
        assert_eq!(missing.code, "job_not_found");
    }
}
