use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use crate::core::config::{AppConfig, ModelProfile};
use crate::core::memory::{Fingerprint, MemoryCase};
use crate::core::model::design::ImageInput;
use crate::core::model::implement::ImplementClient;
use crate::core::pipeline::advisor::{AdvisorHook, AdvisorRun};
use crate::core::pipeline::cache::{cached_image, profile_identity, AnswerCache, StepCache};
use crate::core::pipeline::hook::{injected_summary, ContextHooks, Section, StaticContext};
use crate::core::pipeline::runner::{parse_validate_or_fill, DesignCall, DesignStep};
use crate::core::pipeline::visual::{
    build_visual_asset_context, pick_visual_subjects, visual_asset_context_text,
    visual_asset_log_text,
};
use crate::core::pipeline::{contract, validate};
use crate::core::prompt::PromptStore;
use crate::error::{AppError, AppResult};
use crate::event::{DesignLog, DesignSink};

/// Stage keys the context hooks are registered against.
pub const STAGE_STRUCTURE: &str = "paper_structure";
pub const STAGE_DESIGN: &str = "paper_design";

#[derive(Clone, Debug, Deserialize)]
pub struct PaperFigurePayload {
    #[serde(default)]
    pub figure_title: String,
    #[serde(default)]
    pub section_description: String,
    #[serde(default)]
    pub template_ids: Vec<String>,
    #[serde(default = "default_ratio")]
    pub aspect_ratio: String,
    #[serde(default = "default_fidelity")]
    pub layout_fidelity: String,
    #[serde(default = "default_strength")]
    pub style_strength: String,
    #[serde(default)]
    pub custom_prompt: Option<String>,
}

fn default_ratio() -> String {
    "inherit".to_string()
}
fn default_fidelity() -> String {
    "balanced".to_string()
}
fn default_strength() -> String {
    "high".to_string()
}

pub type StageSink<'a> = &'a (dyn Fn(&str, &str) + Send + Sync);

/// 检索只吃标题与方法描述：约束（字体、底色一类）不是可检索的视觉主体。
fn figure_search_material(payload: &PaperFigurePayload) -> String {
    [
        payload.figure_title.trim(),
        payload.section_description.trim(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

pub fn validate_payload(payload: &PaperFigurePayload) -> AppResult<()> {
    if payload.template_ids.is_empty() {
        return Err(AppError::new(
            "invalid_payload",
            "Paper figure requires at least one template",
        ));
    }
    Ok(())
}

pub struct FigureRun<'a> {
    pub prompts: &'a PromptStore,
    pub config: &'a AppConfig,
    pub design_profile: &'a ModelProfile,
    pub implement_profile: Option<&'a ModelProfile>,
    /// Search runs only when a search profile is configured: a figure job that
    /// never had one must keep working instead of failing on a missing profile.
    pub search_profile: Option<&'a ModelProfile>,
    pub template_images: Vec<ImageInput>,
    pub template_metadata: Value,
    pub app_data: &'a Path,
    pub job_id: &'a str,
    pub design_log: DesignSink<'a>,
    /// Recalled cases for the advisor step; empty means the step is skipped.
    pub similar_cases: Vec<MemoryCase>,
    pub fingerprint: Option<Fingerprint>,
    /// 失败或停止后重跑这个任务时的答案缓存；没有就每次都真的调用。
    pub cache: Option<&'a AnswerCache>,
}

/// What a figure run hands back: the image and the design product that the
/// memory keeps as the job's case.
pub struct FigureOutput {
    pub image_b64: Option<String>,
    pub final_prompts: Vec<String>,
    pub design: Value,
}

fn save_design_diagnostic(
    app_data: &Path,
    job_id: &str,
    name: &str,
    text: &str,
) -> AppResult<PathBuf> {
    let dir = app_data.join("logs").join(job_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, text)?;
    Ok(path)
}

fn visible_text_diagnostic(text: &str) -> Value {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let value = parsed
        .as_ref()
        .and_then(|root| root.get("figure").or(Some(root)))
        .and_then(|figure| figure.get("visible_text"));
    match value {
        Some(Value::Array(items)) => {
            let invalid = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| match item.as_str() {
                    Some(text) if text.trim().chars().count() <= 80 => None,
                    Some(text) => Some(serde_json::json!({
                        "index": index,
                        "type": "string",
                        "length": text.trim().chars().count(),
                        "excerpt": text.chars().take(120).collect::<String>(),
                    })),
                    None => Some(serde_json::json!({
                        "index": index,
                        "type": match item {
                            Value::Object(_) => "object",
                            Value::Array(_) => "array",
                            Value::Number(_) => "number",
                            Value::Bool(_) => "boolean",
                            Value::Null => "null",
                            Value::String(_) => "string",
                        },
                    })),
                })
                .collect::<Vec<_>>();
            serde_json::json!({"type": "array", "count": items.len(), "invalid_items": invalid})
        }
        Some(value) => serde_json::json!({
            "type": match value {
                Value::Object(_) => "object",
                Value::Array(_) => "array",
                Value::String(_) => "string",
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                Value::Null => "null",
            }
        }),
        None => serde_json::json!({"type": "missing_or_unparseable"}),
    }
}

impl FigureRun<'_> {
    pub async fn run(
        &self,
        payload: &PaperFigurePayload,
        stage: StageSink<'_>,
    ) -> AppResult<FigureOutput> {
        let proxy = self.config.proxy_url.as_deref();

        let system = self.prompts.load("global/system.md")?;
        let template_summary = serde_json::to_string_pretty(&self.template_metadata)?;

        // Everything the stages share is a hook; the stage code only adds
        // what is intrinsic to its own call (the task line, the plan it just
        // produced). Contracts are registered per stage and always close the
        // prompt.
        let mut hooks = ContextHooks::default();
        hooks.register(StaticContext::new(
            "template-metadata",
            &[STAGE_STRUCTURE, STAGE_DESIGN],
            "Selected Template Metadata",
            template_summary,
        ));
        hooks.register(
            StaticContext::new(
                "structure-contract",
                &[STAGE_STRUCTURE],
                "Output Contract",
                contract::structure_plan(),
            )
            .contract(),
        );
        let user_context = serde_json::json!({
            "Figure title": payload.figure_title.trim(),
            "Section description": payload.section_description.trim(),
            "Aspect ratio": payload.aspect_ratio,
            "Layout fidelity": payload.layout_fidelity,
            "Style strength": payload.style_strength,
            "Custom prompt": payload.custom_prompt.clone().unwrap_or_else(|| "None".to_string()),
        });
        hooks.register(
            StaticContext::new(
                "content-inventory",
                &[STAGE_DESIGN],
                "User Input",
                serde_json::to_string_pretty(&user_context)?,
            )
            .at(-5),
        );
        hooks.register(
            StaticContext::new(
                "paper-contract",
                &[STAGE_DESIGN],
                "Output Contract",
                contract::paper(),
            )
            .contract(),
        );

        // 视觉素材检索与幻灯片同源：先联网拿到产品/工具的客观外观描述，再作为
        // 纯文本线索注入制图方案；图片只进结构分析那一个调用。
        match self.search_profile {
            Some(search_profile) => {
                stage("paper_visual_assets", "联网查询产品与工具的视觉素材");
                let subjects = pick_visual_subjects(
                    &figure_search_material(payload),
                    self.design_profile,
                    self.prompts,
                    proxy,
                    self.cache,
                )
                .await;
                let visual_context = build_visual_asset_context(
                    &subjects,
                    search_profile,
                    proxy,
                    self.design_log,
                    self.cache,
                )
                .await?;
                (self.design_log)(DesignLog::End {
                    step: "paper_visual_assets",
                    label: "联网查询 · 视觉证据",
                    text: &visual_asset_log_text(&visual_context),
                    ok: true,
                });
                hooks.register(
                    StaticContext::new(
                        "search-evidence",
                        &[STAGE_DESIGN],
                        "Visual Asset Search Context",
                        visual_asset_context_text(&visual_context),
                    )
                    .at(5),
                );
            }
            None => stage(
                "paper_visual_assets",
                "未配置联网查询模型，跳过视觉素材检索",
            ),
        }

        stage("paper_structure_prompt", "准备母版结构分析");
        let structure_assets = self.prompts.load_all(&[
            "global/system.md",
            "global/figure_style.md",
            "modes/paper_figure/structure.md",
        ])?;
        let structure_composed = hooks.compose(
            STAGE_STRUCTURE,
            &structure_assets,
            vec![Section::new(
                "Task",
                "Analyze the attached template figure image(s) only. \
                 Return a reusable structure_plan JSON. Do not invent the user's research content.",
            )
            .at(10)],
        );
        let structure_prompt = structure_composed.prompt;

        let structure_retry_count = std::sync::Mutex::new(0usize);
        let structure_app_data = self.app_data;
        let structure_job_id = self.job_id;
        let structure_retry_sink = move |text: &str| {
            let mut count = structure_retry_count.lock().unwrap();
            *count += 1;
            let name = format!("paper_structure_raw_retry{count}.txt");
            if save_design_diagnostic(structure_app_data, structure_job_id, &name, text).is_ok() {
                stage(
                    "paper_structure_parse",
                    &format!("结果校验未通过，自动重新生成（第 {count} 次）"),
                );
            }
        };
        let structure_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &system.content,
            user_prompt: &structure_prompt,
            images: &self.template_images,
            proxy_url: proxy,
            response_sink: Some(&structure_retry_sink),
            log: Some(DesignStep {
                sink: self.design_log,
                step: "paper_structure",
                label: "分析母版结构",
            }),
            cache: self.cache.map(|cache| StepCache {
                cache,
                scope: STAGE_STRUCTURE,
            }),
        };

        stage("paper_structure", "分析母版结构");
        let structure_text = structure_call.first().await?;
        save_design_diagnostic(
            self.app_data,
            self.job_id,
            "paper_structure_raw.txt",
            &structure_text,
        )?;

        stage("paper_structure_parse", "校验结构规划");
        let structure = parse_validate_or_fill(&structure_call, &structure_text, |value| {
            validate::validate_structure_plan(value)
        })
        .await?;

        // The plan is stage-1 output, so it can only join the chain now.
        hooks.register(
            StaticContext::new(
                "structure-plan",
                &[STAGE_DESIGN],
                "Structure Plan From Templates",
                serde_json::to_string_pretty(&structure.value)?,
            )
            .at(-10),
        );

        if let Some(fingerprint) = self
            .fingerprint
            .as_ref()
            .filter(|_| !self.similar_cases.is_empty())
        {
            stage(
                "paper_advisor",
                &format!("对比 {} 个相似历史案例", self.similar_cases.len()),
            );
            let advisor = AdvisorRun {
                prompts: self.prompts,
                profile: self.design_profile,
                proxy_url: proxy,
                design_log: self.design_log,
                cache: self.cache,
            };
            match advisor.advise(fingerprint, &self.similar_cases).await {
                Some(advice) => hooks.register(AdvisorHook::new(&[STAGE_DESIGN], advice)),
                None => stage("paper_advisor", "没有可用的历史建议，按新任务继续"),
            }
        }

        let design_assets = self.prompts.load_all(&[
            "global/system.md",
            "global/figure_style.md",
            "global/expression.md",
            "modes/paper_figure/design.md",
            "modes/paper_figure/diagram_rules.md",
            "modes/paper_figure/plot_rules.md",
            "modes/paper_figure/validator.md",
        ])?;
        let design_composed = hooks.compose(STAGE_DESIGN, &design_assets, Vec::new());
        stage(
            "paper_prompt",
            &format!(
                "准备内容填充（注入 {}）",
                injected_summary(&design_composed)
            ),
        );
        let design_prompt = design_composed.prompt;

        let no_images: Vec<ImageInput> = Vec::new();
        let retry_count = std::sync::Mutex::new(0usize);
        let app_data = self.app_data;
        let job_id = self.job_id;
        let retry_sink = move |text: &str| {
            let mut count = retry_count.lock().unwrap();
            *count += 1;
            let name = format!("paper_design_raw_retry{count}.txt");
            if save_design_diagnostic(app_data, job_id, &name, text).is_ok() {
                stage(
                    "paper_parse",
                    &format!("结果校验未通过，自动重新生成（第 {count} 次）"),
                );
            }
        };
        let design_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &system.content,
            user_prompt: &design_prompt,
            images: &no_images,
            proxy_url: proxy,
            response_sink: Some(&retry_sink),
            log: Some(DesignStep {
                sink: self.design_log,
                step: "paper_design",
                label: "映射内容并生成制图方案",
            }),
            cache: self.cache.map(|cache| StepCache {
                cache,
                scope: STAGE_DESIGN,
            }),
        };

        stage("paper_design", "生成制图方案");
        let design_text = design_call.first().await?;
        let diagnostic_path = save_design_diagnostic(
            self.app_data,
            self.job_id,
            "paper_design_raw.txt",
            &design_text,
        )?;

        stage("paper_parse", "校验制图方案");
        let title = payload.figure_title.trim().to_string();
        let section = payload.section_description.trim().to_string();
        let design_result = parse_validate_or_fill(&design_call, &design_text, move |value| {
            validate::validate_paper_design(value)?;
            validate::validate_design_content_coverage(value, &title, &section)?;
            Ok(())
        })
        .await;
        let design = design_result.map_err(|error| {
            let mut detail = error.detail.unwrap_or_else(|| serde_json::json!({}));
            if !detail.is_object() {
                detail = serde_json::json!({"upstream_detail": detail});
            }
            if let Some(object) = detail.as_object_mut() {
                object.insert(
                    "design_raw_path".to_string(),
                    Value::String(diagnostic_path.display().to_string()),
                );
                object.insert(
                    "visible_text".to_string(),
                    visible_text_diagnostic(&design_text),
                );
                object.insert("summary".to_string(), Value::String(error.message.clone()));
                object.insert("code".to_string(), Value::String(error.code.clone()));
            }
            AppError::with_detail(error.code, error.message, detail)
        })?;

        let implement_prompt =
            validate::implement_prompt(&design.parsed).map_err(AppError::from)?;

        let image_b64 = if let Some(profile) = self.implement_profile {
            stage("paper_implement", "生成图片");
            let overrides = Map::new();
            let identity = profile_identity(profile);
            let overrides_json = serde_json::to_string(&overrides).unwrap_or_default();
            let image = cached_image(
                self.cache.map(|cache| StepCache {
                    cache,
                    scope: "paper_implement",
                }),
                &[
                    identity.as_bytes(),
                    implement_prompt.as_bytes(),
                    overrides_json.as_bytes(),
                ],
                || ImplementClient::generate(profile, &implement_prompt, &[], &overrides, proxy),
            )
            .await?;
            stage("paper_save", "保存图片");
            Some(image)
        } else {
            None
        };
        Ok(FigureOutput {
            image_b64,
            final_prompts: vec![implement_prompt],
            design: serde_json::json!({
                "structure_plan": structure.value,
                "design": design.parsed,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(title: &str, section: &str, custom: Option<&str>) -> PaperFigurePayload {
        PaperFigurePayload {
            figure_title: title.to_string(),
            section_description: section.to_string(),
            template_ids: vec!["t1".to_string()],
            aspect_ratio: default_ratio(),
            layout_fidelity: default_fidelity(),
            style_strength: default_strength(),
            custom_prompt: custom.map(str::to_string),
        }
    }

    /// 约束只影响版式与配色，不进检索输入。
    #[test]
    fn search_material_excludes_constraints() {
        let figure = payload(
            "  部署拓扑  ",
            "  用 Docker 构建  ",
            Some("  标题用微软雅黑，底色 #FFFFFF  "),
        );
        assert_eq!(figure_search_material(&figure), "部署拓扑\n用 Docker 构建");
        assert_eq!(figure_search_material(&payload("标题", "", None)), "标题");
        assert_eq!(
            figure_search_material(&payload("", "", Some("白色背景"))),
            ""
        );
    }
}
