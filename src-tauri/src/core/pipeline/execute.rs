use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter};

use crate::core::asset::AssetService;
use crate::core::config::ConfigService;
use crate::core::doc::extract_material_text;
use crate::core::job::{JobImage, JobService};
use crate::core::memory::{Fingerprint, MemoryService, RECALL_LIMIT};
use crate::core::model::design::ImageInput;
use crate::core::net::{decode_b64, encode_b64};
use crate::core::pipeline::cache::AnswerCache;
use crate::core::pipeline::figure::{self, FigureRun, PaperFigurePayload};
use crate::core::pipeline::slide::{
    compose_material_context, validate_payload, MaterialAsset, PptSlidePayload, SlideRun,
};
use crate::core::tpl::TemplateService;
use crate::core::Core;
use crate::error::{AppError, AppResult};
use crate::event::{DesignLog, DesignLogPayload, DesignSink, JobEventPayload};

const MAX_FIGURE_TEMPLATES: usize = 3;

/// How long streamed text is pooled before it is pushed to the webview.
///
/// Deltas arrive one SSE chunk at a time, and a long design answer is thousands
/// of them. One IPC message and one React render each is enough to make the
/// result panel stutter, so they are batched into ~8 updates a second — fast
/// enough to read as text being written, cheap enough not to compete with the
/// job for the UI thread.
const DELTA_FLUSH_MS: u128 = 120;

/// Text waiting to be pushed, per step.
///
/// `Reset` and `End` both discard whatever is pooled rather than flushing it:
/// a reset voids the attempt it belonged to, and `End` carries the whole answer
/// and replaces the card's text outright, so a trailing partial adds nothing.
#[derive(Default)]
struct DeltaPool {
    pending: std::sync::Mutex<std::collections::HashMap<String, (String, std::time::Instant)>>,
}

impl DeltaPool {
    /// The text to push now, or `None` while it is still worth pooling.
    fn push(&self, step: &str, text: &str) -> Option<String> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let slot = pending
            .entry(step.to_string())
            .or_insert_with(|| (String::new(), std::time::Instant::now()));
        slot.0.push_str(text);
        if slot.1.elapsed().as_millis() < DELTA_FLUSH_MS {
            return None;
        }
        let flushed = std::mem::take(&mut slot.0);
        slot.1 = std::time::Instant::now();
        Some(flushed)
    }

    fn discard(&self, step: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(step);
    }
}

pub fn spawn(app: AppHandle, core: Arc<Core>, job_id: String) {
    let cancelled = Arc::new(AtomicBool::new(false));
    let handle = tauri::async_runtime::spawn({
        let core = Arc::clone(&core);
        let cancelled = Arc::clone(&cancelled);
        let job_id = job_id.clone();
        async move {
            let emit = |stage: &str, message: &str, status: &str| {
                let _ = app.emit(
                    "job://stage",
                    JobEventPayload {
                        job_id: job_id.clone(),
                        status: status.to_string(),
                        stage: stage.to_string(),
                        message: message.to_string(),
                        timestamp: Utc::now(),
                    },
                );
            };
            let sink = |stage: &str, message: &str| {
                let _ = JobService::new(&core.store).mark_stage(&job_id, stage, message, "running");
                emit(stage, message, "running");
            };
            // Deltas are pushed rather than polled: the result panel refreshes
            // the job every 1.8s, which is fine for a stage name but would turn
            // a streamed answer into a slideshow.
            let pool = DeltaPool::default();
            let design = |entry: DesignLog<'_>| {
                let (kind, step, label, text, status) = match entry {
                    DesignLog::Begin { step, label } => {
                        ("begin", step, label, String::new(), "running")
                    }
                    DesignLog::Delta { step, text } => match pool.push(step, text) {
                        Some(batched) => ("delta", step, "", batched, "running"),
                        None => return,
                    },
                    DesignLog::Reset { step } => {
                        pool.discard(step);
                        ("reset", step, "", String::new(), "running")
                    }
                    DesignLog::End {
                        step,
                        label,
                        text,
                        ok,
                    } => {
                        pool.discard(step);
                        (
                            "end",
                            step,
                            label,
                            text.to_string(),
                            if ok { "succeeded" } else { "failed" },
                        )
                    }
                };
                if kind == "end" {
                    let _ = JobService::new(&core.store)
                        .record_design_log(&job_id, step, label, status, &text);
                }
                let _ = app.emit(
                    "job://design",
                    DesignLogPayload {
                        job_id: job_id.clone(),
                        kind: kind.to_string(),
                        step: step.to_string(),
                        label: label.to_string(),
                        text,
                        status: status.to_string(),
                        timestamp: Utc::now(),
                    },
                );
            };

            let outcome = run(&core, &job_id, &sink, &design).await;
            if cancelled.load(Ordering::SeqCst) {
                return;
            }
            match outcome {
                Ok(RunOutput { images, case }) => {
                    // The case is sedimented before the job is marked done, so
                    // a rating can never race ahead of the row it lands on. A
                    // memory failure is not a job failure: the image exists.
                    if let Some((fingerprint, design)) = case {
                        sink("memory_record", "沉淀 design 产物到案例库");
                        let _ =
                            MemoryService::new(&core.store).record(&job_id, &fingerprint, &design);
                    }
                    let _ = JobService::new(&core.store).finish(&job_id, &images);
                    emit("completed", "任务完成", "succeeded");
                }
                Err(error) => {
                    let failed_stage = JobService::new(&core.store)
                        .get_job(job_id.clone())
                        .ok()
                        .and_then(|job| job.stage)
                        .unwrap_or_else(|| "failed".to_string());
                    let _ = JobService::new(&core.store).fail(
                        &job_id,
                        &error.message,
                        error.detail.as_ref(),
                    );
                    emit(&failed_stage, &error.message, "failed");
                }
            }
            core.cancels.finish(&job_id);
        }
    });
    core.cancels
        .register(&job_id, cancelled, move || handle.abort());
}

struct RunOutput {
    images: Vec<JobImage>,
    /// The task fingerprint and design product to sediment as a case.
    case: Option<(Fingerprint, serde_json::Value)>,
}

/// Recall similar finished cases for this job, or nothing when the brief is
/// empty or the memory query fails: recall is best effort, the job is not.
fn recall_cases(
    core: &Core,
    job_id: &str,
    fingerprint: Option<&Fingerprint>,
    stage: &(dyn Fn(&str, &str) + Send + Sync),
    stage_key: &str,
) -> Vec<crate::core::memory::MemoryCase> {
    let Some(fingerprint) = fingerprint else {
        return Vec::new();
    };
    let cases = MemoryService::new(&core.store)
        .recall(fingerprint, Some(job_id), RECALL_LIMIT)
        .unwrap_or_default();
    if cases.is_empty() {
        stage(stage_key, "案例库里没有相似历史任务，跳过参考");
    } else {
        stage(stage_key, &format!("参考 {} 个相似历史案例", cases.len()));
    }
    cases
}

async fn run(
    core: &Core,
    job_id: &str,
    stage: &(dyn Fn(&str, &str) + Send + Sync),
    design_log: DesignSink<'_>,
) -> AppResult<RunOutput> {
    let envelope = JobService::new(&core.store).payload(job_id)?;
    let mode = envelope
        .get("mode")
        .and_then(|value| value.as_str())
        .unwrap_or("paper_figure")
        .to_string();
    let payload = envelope
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let fingerprint = Fingerprint::from_payload(&envelope);

    let config = ConfigService::new(&core.store).runtime_config()?;
    let design_profile = ConfigService::active_profile(&config, "design")?.clone();
    let simple_mode = crate::core::job::simple_mode(&envelope)?;
    let implement_profile = if simple_mode {
        None
    } else {
        Some(ConfigService::active_profile(&config, "implement")?.clone())
    };

    // 同一个任务的缓存：重跑（失败后重试 / 停止后继续）时，已经买过的模型答案
    // 与图片从这里回放，只有没跑完的部分才真的花钱。
    let cache = AnswerCache::new(&core.app_data, job_id);

    let output = match mode.as_str() {
        "paper_figure" => {
            let payload: PaperFigurePayload = serde_json::from_value(payload)?;
            stage("paper_validate", "校验输入");
            figure::validate_payload(&payload)?;

            stage("paper_templates", "读取参考模板");
            let templates = TemplateService::new(&core.store, &core.app_data);
            let mut images = Vec::new();
            let mut metadata = Vec::new();
            for id in payload.template_ids.iter().take(MAX_FIGURE_TEMPLATES) {
                let detail = templates.template_detail(id)?;
                metadata.push(detail.metadata());
                images.push(read_image_input(&detail.image_path, &detail.mime_type)?);
            }

            let similar_cases =
                recall_cases(core, job_id, fingerprint.as_ref(), stage, "paper_memory");
            // 检索是加分项而不是前提：没有配检索模型时图照样能画，只是少了
            // 联网得到的客观外观描述。
            let search_profile = ConfigService::active_profile(&config, "search").ok();
            let run = FigureRun {
                prompts: &core.prompts,
                config: &config,
                design_profile: &design_profile,
                implement_profile: implement_profile.as_ref(),
                search_profile,
                template_images: images,
                template_metadata: serde_json::Value::Array(metadata),
                app_data: &core.app_data,
                job_id,
                design_log,
                similar_cases,
                fingerprint: fingerprint.clone(),
                cache: Some(&cache),
            };
            let output = run.run(&payload, stage).await?;
            if simple_mode {
                persist_final_prompts(&core.store, job_id, &output.final_prompts, design_log)?;
            }
            Ok(RunOutput {
                images: output
                    .image_b64
                    .as_deref()
                    .map(|image| save_image(core, job_id, "figure.png", image))
                    .transpose()?
                    .into_iter()
                    .collect(),
                case: fingerprint.map(|fingerprint| (fingerprint, output.design)),
            })
        }
        "ppt_slide" => {
            let payload: PptSlidePayload = serde_json::from_value(payload)?;
            stage("ppt_validate", "校验输入");
            validate_payload(&payload)?;
            let search_profile = ConfigService::active_profile(&config, "search")?.clone();

            let assets = AssetService::new(&core.store, &core.app_data);
            stage("ppt_template", "读取母版图片");
            let template_image = if let Some(template_id) = payload.template_ref() {
                let templates = TemplateService::new(&core.store, &core.app_data);
                let detail = templates.template_detail(template_id)?;
                read_image_input(&detail.image_path, &detail.mime_type)?
            } else {
                let template = assets.asset_file(&payload.template_asset_id)?;
                read_image_input(&template.path, &template.mime_type)?
            };

            stage("ppt_material", "整理资料输入");
            let mut material_assets = Vec::new();
            for id in &payload.material_asset_ids {
                material_assets.push(material_summary(&assets, id)?);
            }
            let material_context =
                compose_material_context(&payload.material_text, &material_assets);
            // File material has no payload field, so a deck built only from
            // uploads still gets a fingerprint from the composed context.
            let fingerprint = fingerprint.or_else(|| {
                let brief = material_context.trim();
                (!brief.is_empty()).then(|| Fingerprint {
                    mode: "ppt_slide".to_string(),
                    title: brief.lines().next().unwrap_or_default().trim().to_string(),
                    brief: brief.chars().take(4000).collect(),
                })
            });
            let similar_cases =
                recall_cases(core, job_id, fingerprint.as_ref(), stage, "ppt_memory");

            let run = SlideRun {
                prompts: &core.prompts,
                config: &config,
                design_profile: &design_profile,
                implement_profile: implement_profile.as_ref(),
                search_profile: &search_profile,
                template_image,
                material_context,
                design_log,
                similar_cases,
                fingerprint: fingerprint.clone(),
                cache: Some(&cache),
            };
            let output = run.run(&payload, stage).await?;
            if simple_mode {
                persist_final_prompts(&core.store, job_id, &output.final_prompts, design_log)?;
            } else {
                stage("ppt_save", "保存图片");
            }
            let images = output
                .pages
                .iter()
                .enumerate()
                .map(|(index, image_b64)| {
                    save_image(core, job_id, &format!("slide_{}.png", index + 1), image_b64)
                })
                .collect::<AppResult<Vec<_>>>()?;
            Ok(RunOutput {
                images,
                case: fingerprint.map(|fingerprint| (fingerprint, output.design)),
            })
        }
        other => Err(AppError::new(
            "invalid_mode",
            format!("Unsupported job mode: {other}"),
        )),
    }?;
    // 任务已经成功跑完，缓存没有被重试复用的机会，直接清掉省磁盘。
    cache.clear();
    Ok(output)
}

fn persist_final_prompts(
    store: &crate::core::store::Store,
    job_id: &str,
    prompts: &[String],
    design_log: DesignSink<'_>,
) -> AppResult<()> {
    if prompts.is_empty() || prompts.iter().any(|prompt| prompt.trim().is_empty()) {
        return Err(AppError::new(
            "missing_final_prompt",
            "Final drawing prompts are empty",
        ));
    }
    for (index, prompt) in prompts.iter().enumerate() {
        let step = format!("final_prompt_{}", index + 1);
        let label = format!("最终制图提示词 {}", index + 1);
        // The prompt is the whole product here, so the write is checked rather
        // than left to the event sink: the job must not be marked done with
        // nothing stored to show for it.
        JobService::new(store).record_design_log(job_id, &step, &label, "succeeded", prompt)?;
        design_log(DesignLog::End {
            step: &step,
            label: &label,
            text: prompt,
            ok: true,
        });
    }
    Ok(())
}

fn read_image_input(path: &std::path::Path, mime_type: &str) -> AppResult<ImageInput> {
    let bytes = std::fs::read(path).map_err(|error| {
        AppError::new(
            "template_image_read_failed",
            format!("Failed to read template image {}: {error}", path.display()),
        )
    })?;
    Ok(ImageInput {
        filename: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("template.png")
            .to_string(),
        mime_type: mime_type.to_string(),
        b64: encode_b64(&bytes),
    })
}

fn material_summary(assets: &AssetService<'_>, asset_id: &str) -> AppResult<MaterialAsset> {
    let asset = assets.asset_file(asset_id)?;
    let (text, parser) = extract_material_text(&asset.path);
    let bytes = std::fs::metadata(&asset.path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    Ok(MaterialAsset::new(
        asset.filename,
        asset.mime_type,
        bytes,
        parser,
        &text,
    ))
}

fn save_image(core: &Core, job_id: &str, name: &str, image_b64: &str) -> AppResult<JobImage> {
    let bytes = decode_b64(image_b64)?;
    let saved =
        AssetService::new(&core.store, &core.app_data).save_job_image(job_id, name, &bytes)?;
    Ok(JobImage {
        name: name.to_string(),
        url: saved.url,
        asset_id: Some(saved.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pooled_deltas_are_batched_and_lose_nothing_in_order() {
        let pool = DeltaPool::default();
        // Chunks arriving inside one window are held back rather than sent one
        // IPC message at a time.
        assert_eq!(pool.push("paper_design", "{\"fig"), None);
        assert_eq!(pool.push("paper_design", "ure\":"), None);

        std::thread::sleep(std::time::Duration::from_millis(DELTA_FLUSH_MS as u64 + 20));
        assert_eq!(
            pool.push("paper_design", "1}").as_deref(),
            Some("{\"figure\":1}"),
            "一个窗口内的分片必须按序合并后一次发出"
        );

        // Steps are pooled independently: slide pages plan concurrently, and one
        // page's chunks must not land in another page's card.
        assert_eq!(pool.push("ppt_page_plan_1", "第一页"), None);
        assert_eq!(pool.push("ppt_page_plan_2", "第二页"), None);
        std::thread::sleep(std::time::Duration::from_millis(DELTA_FLUSH_MS as u64 + 20));
        assert_eq!(
            pool.push("ppt_page_plan_1", "内容").as_deref(),
            Some("第一页内容")
        );
        assert_eq!(
            pool.push("ppt_page_plan_2", "内容").as_deref(),
            Some("第二页内容")
        );

        // A retry voids what it already streamed, so the pooled tail must go
        // with it instead of being prepended to the new attempt. (This first
        // chunk flushes straight through: the step's window expired while the
        // two pages above were being pooled, and text that has been waiting
        // should not wait another window.)
        pool.push("paper_design", "作废的开头");
        pool.push("paper_design", "作废的结尾");
        pool.discard("paper_design");
        std::thread::sleep(std::time::Duration::from_millis(DELTA_FLUSH_MS as u64 + 20));
        assert_eq!(
            pool.push("paper_design", "新的一段"),
            None,
            "丢弃后应重新开一个窗口"
        );
        std::thread::sleep(std::time::Duration::from_millis(DELTA_FLUSH_MS as u64 + 20));
        assert_eq!(
            pool.push("paper_design", "的后半").as_deref(),
            Some("新的一段的后半"),
            "重试后发出的内容不能带上被丢弃的那一次"
        );
    }

    fn store(tag: &str) -> (std::path::PathBuf, crate::core::store::Store) {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-execute-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let store = crate::core::store::Store::initialize(&dir).expect("初始化 store");
        (dir, store)
    }

    #[test]
    fn final_prompts_are_persisted_before_the_job_may_complete() {
        let (_dir, store) = store("final-prompt");
        let job = JobService::new(&store)
            .create_job(serde_json::json!({"mode": "paper_figure"}))
            .expect("建任务");

        // The sink type is `Fn`, so what it collects lives behind a lock.
        let streamed = std::sync::Mutex::new(Vec::new());
        let sink = |entry: DesignLog<'_>| {
            if let DesignLog::End { step, text, .. } = entry {
                streamed
                    .lock()
                    .expect("锁")
                    .push((step.to_string(), text.to_string()));
            }
        };

        persist_final_prompts(&store, &job.id, &["第一页提示词".to_string()], &sink)
            .expect("保存最终提示词");
        let loaded = JobService::new(&store)
            .get_job(job.id.clone())
            .expect("读任务");
        assert_eq!(loaded.design_logs.len(), 1);
        assert_eq!(loaded.design_logs[0].step, "final_prompt_1");
        assert_eq!(loaded.design_logs[0].content, "第一页提示词");
        assert_eq!(loaded.design_logs[0].status, "succeeded");
        assert_eq!(
            *streamed.lock().expect("锁"),
            vec![("final_prompt_1".to_string(), "第一页提示词".to_string())],
            "事件只在写入成功之后才发出"
        );

        // A prompt with nothing in it is a pipeline bug, not a product.
        for empty in [Vec::new(), vec![String::new()], vec!["  ".to_string()]] {
            assert!(persist_final_prompts(&store, &job.id, &empty, &sink).is_err());
        }

        // The write is checked: a store that cannot take the prompt has to fail
        // the job instead of completing it with nothing saved.
        store
            .connection()
            .expect("连接")
            .execute_batch("DROP TABLE job_design_logs")
            .expect("模拟存储故障");
        assert!(
            persist_final_prompts(&store, &job.id, &["第二页提示词".to_string()], &sink).is_err()
        );
        assert_eq!(
            streamed.lock().expect("锁").len(),
            1,
            "写入失败时不得再发成功事件，否则前端会看到并未落盘的提示词"
        );
    }
}
