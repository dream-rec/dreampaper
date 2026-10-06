use std::future::Future;

use futures::stream::{StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::core::config::{AppConfig, ModelProfile};
use crate::core::memory::{Fingerprint, MemoryCase};
use crate::core::model::design::ImageInput;
use crate::core::model::implement::ImplementClient;
use crate::core::pipeline::advisor::{AdvisorHook, AdvisorRun};
use crate::core::pipeline::cache::{cached_image, profile_identity, AnswerCache, StepCache};
use crate::core::pipeline::contract;
use crate::core::pipeline::figure::StageSink;
use crate::core::pipeline::hook::{injected_summary, ContextHooks, Section, StaticContext};
use crate::core::pipeline::runner::{parse_validate_or_fill, DesignCall, DesignStep};
use crate::core::pipeline::slide_validate::{
    apply_master_prompt_prefix, validate_ppt_outline, validate_ppt_pages, validate_ppt_single_page,
    validate_template_analysis,
};
use crate::core::pipeline::visual::{
    build_visual_asset_context, pick_visual_subjects, visual_asset_context_text,
    visual_asset_log_text,
};
use crate::core::prompt::{PromptAsset, PromptStore};
use crate::error::{AppError, AppResult};
use crate::event::{DesignLog, DesignSink};

/// Stage keys the context hooks are registered against. Page planning shares
/// one key across pages: the hooks contribute the same deck-level context to
/// every page, and the page-specific sections stay with the call.
pub const STAGE_ANALYZE: &str = "ppt_analyze";
pub const STAGE_OUTLINE: &str = "ppt_outline";
pub const STAGE_PAGE: &str = "ppt_page_plan";

const PPT_PLAN_TEMPLATE_LIMIT: usize = 2600;
const PPT_PLAN_MATERIAL_LIMIT: usize = 3200;

pub const MATERIAL_TEXT_LIMIT: usize = 6000;

const MAX_PAGE_COUNT: usize = 20;

const IMAGE2_FALLBACK_OUTPUT: [(&str, &str); 4] = [
    ("size", "1200x675"),
    ("quality", "auto"),
    ("output_format", "png"),
    ("response_format", "b64_json"),
];

#[derive(Clone, Debug, Deserialize)]
pub struct PptSlidePayload {
    #[serde(default)]
    pub template_asset_id: String,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub material_text: String,
    #[serde(default)]
    pub material_asset_ids: Vec<String>,
    #[serde(default = "default_page_count")]
    pub page_count: usize,
    #[serde(default)]
    pub custom_prompt: Option<String>,
}

impl PptSlidePayload {
    pub fn template_ref(&self) -> Option<&str> {
        self.template_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn asset_ref(&self) -> Option<&str> {
        Some(self.template_asset_id.trim()).filter(|value| !value.is_empty())
    }
}

fn default_page_count() -> usize {
    1
}

impl PptSlidePayload {
    pub fn custom_prompt_text(&self) -> String {
        match self.custom_prompt.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => value.to_string(),
            _ => "None".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MaterialAsset {
    pub filename: String,
    pub mime_type: String,
    pub bytes: u64,
    pub parser: String,
    pub text_excerpt: String,
    pub text_length: usize,
    pub truncated: bool,
}

impl MaterialAsset {
    pub fn new(
        filename: String,
        mime_type: String,
        bytes: u64,
        parser: String,
        text: &str,
    ) -> Self {
        let text_length = text.chars().count();
        let text_excerpt: String = text.chars().take(MATERIAL_TEXT_LIMIT).collect();
        Self {
            filename,
            mime_type,
            bytes,
            parser,
            truncated: text_length > text_excerpt.chars().count(),
            text_excerpt,
            text_length,
        }
    }
}

pub fn validate_payload(payload: &PptSlidePayload) -> AppResult<()> {
    if payload.template_ref().is_none() && payload.asset_ref().is_none() {
        return Err(AppError::new(
            "invalid_payload",
            "PPT slide requires a template asset",
        ));
    }
    if payload.page_count < 1 || payload.page_count > MAX_PAGE_COUNT {
        return Err(AppError::new(
            "invalid_payload",
            format!("PPT page_count must be between 1 and {MAX_PAGE_COUNT}"),
        ));
    }
    if payload.material_asset_ids.len() > 10 {
        return Err(AppError::new(
            "invalid_payload",
            "PPT slide accepts at most 10 material files",
        ));
    }
    Ok(())
}

pub fn compose_material_context(material_text: &str, assets: &[MaterialAsset]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !material_text.trim().is_empty() {
        sections.push(format!("User text material:\n{}", material_text.trim()));
    }
    for asset in assets {
        let mut header = format!(
            "File material: {} ({}, {} bytes, parser={})",
            asset.filename, asset.mime_type, asset.bytes, asset.parser
        );
        if asset.truncated {
            header.push_str(&format!(
                ", excerpt={}/{} chars",
                asset.text_excerpt.chars().count(),
                asset.text_length
            ));
        }
        let excerpt = if asset.text_excerpt.is_empty() {
            "No extractable text available; use filename and file type as context only."
        } else {
            asset.text_excerpt.as_str()
        };
        sections.push(format!("{header}\n{excerpt}"));
    }
    sections.join("\n\n---\n\n")
}

pub fn resolve_ppt_concurrency(configured: Option<i64>, page_count: usize) -> usize {
    let default_concurrency = page_count.max(1);
    let requested = match configured {
        Some(value) if value > 0 => value as usize,
        Some(_) => 1,
        None => default_concurrency,
    };
    requested.clamp(1, default_concurrency)
}

pub fn truncate_text(text: &str, limit: usize) -> String {
    let length = text.chars().count();
    if length <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}\n\n[Truncated from {length} chars to {limit} chars for page planning latency.]")
}

fn pick(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

pub fn compact_template_analysis(analysis: &Value) -> String {
    let wrapper = if analysis
        .get("template_analysis")
        .is_some_and(Value::is_object)
    {
        &analysis["template_analysis"]
    } else {
        analysis
    };
    if !wrapper.is_object() {
        let raw = serde_json::to_string(analysis).unwrap_or_default();
        return truncate_text(&raw, PPT_PLAN_TEMPLATE_LIMIT);
    }
    let empty = Value::Object(Map::new());
    let master = wrapper
        .get("master_style_spec")
        .filter(|value| value.is_object())
        .unwrap_or(&empty);
    let compact = json!({
        "master_style_summary": pick(wrapper, "master_style_summary"),
        "master_style_spec": {
            "canvas": pick(master, "canvas"),
            "title_region": pick(master, "title_region"),
            "safe_margins": pick(master, "safe_margins"),
            "header_footer": pick(master, "header_footer"),
            "divider_lines": pick(master, "divider_lines"),
            "palette": pick(master, "palette"),
            "typography": pick(master, "typography"),
            "module_style": pick(master, "module_style"),
            "decorative_elements": pick(master, "decorative_elements"),
            "immutable_elements": pick(master, "immutable_elements"),
            "forbidden_deviations": pick(master, "forbidden_deviations"),
        },
        "global_constraints": pick(wrapper, "global_constraints"),
        "immutable_elements": pick(wrapper, "immutable_elements"),
        "page_layout_rules": pick(wrapper, "page_layout_rules"),
    });
    truncate_text(
        &serde_json::to_string(&compact).unwrap_or_default(),
        PPT_PLAN_TEMPLATE_LIMIT,
    )
}

pub fn adjacent_page_context(deck_outline: &Value, page_number: i64) -> Value {
    let briefs = deck_outline["page_briefs"].as_array();
    let find = |target: i64| -> Value {
        briefs
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.is_object() && item["page"].as_i64() == Some(target))
            })
            .cloned()
            .unwrap_or(Value::Null)
    };
    json!({"previous": find(page_number - 1), "next": find(page_number + 1)})
}

pub fn ppt_output_defaults(profile: &ModelProfile) -> Map<String, Value> {
    let protocol = if profile.protocol == "banna2" {
        "banana2"
    } else {
        profile.protocol.as_str()
    };
    let defaults: Map<String, Value> = profile
        .output_defaults
        .iter()
        .filter(|(_, value)| !value.is_null() && value.as_str() != Some(""))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    match protocol {
        "image2" => {
            let mut merged: Map<String, Value> = IMAGE2_FALLBACK_OUTPUT
                .iter()
                .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
                .collect();
            merged.extend(defaults);
            merged
        }
        "banana2" => {
            let take = |key: &str, fallback: &str| -> Value {
                defaults
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| Value::String(fallback.to_string()))
            };
            let mut merged = Map::new();
            merged.insert("aspect_ratio".to_string(), take("aspect_ratio", "16:9"));
            merged.insert("image_size".to_string(), take("image_size", "4K"));
            merged.insert("thinking_level".to_string(), take("thinking_level", "high"));
            merged.insert("mime_type".to_string(), take("mime_type", "image/png"));
            merged
        }
        _ => defaults,
    }
}

async fn run_ordered<T, F>(tasks: Vec<F>, concurrency: usize) -> AppResult<Vec<T>>
where
    F: Future<Output = AppResult<T>>,
{
    futures::stream::iter(tasks)
        .buffered(concurrency.max(1))
        .try_collect()
        .await
}

struct PagePlanContext<'a> {
    profile: &'a ModelProfile,
    system_prompt: &'a str,
    page_assets: &'a [PromptAsset],
    template_analysis: &'a Value,
    hooks: &'a ContextHooks,
    deck_outline: &'a Value,
    proxy_url: Option<&'a str>,
    design_log: DesignSink<'a>,
    cache: Option<&'a AnswerCache>,
}

impl PagePlanContext<'_> {
    async fn plan(&self, page_brief: &Value, stage: StageSink<'_>) -> AppResult<Value> {
        let page_number = page_brief["page"].as_i64().unwrap_or(0);
        stage(
            &format!("ppt_page_prompt_{page_number}"),
            &format!("准备第 {page_number} 页规划"),
        );
        let page_prompt = self
            .hooks
            .compose(
                STAGE_PAGE,
                self.page_assets,
                vec![
                    Section::new(
                        "Task Mode",
                        "Single-page worker mode. Return exactly one page object only. \
                         Do not plan or output other pages.",
                    )
                    .at(-30),
                    Section::new("Deck Outline", serde_json::to_string(self.deck_outline)?).at(-15),
                    Section::new(
                        "Current Page Brief",
                        serde_json::to_string_pretty(page_brief)?,
                    )
                    .at(-12),
                    Section::new(
                        "Adjacent Page Context",
                        serde_json::to_string_pretty(&adjacent_page_context(
                            self.deck_outline,
                            page_number,
                        ))?,
                    )
                    .at(-11),
                    Section::new(
                        "Output Contract",
                        contract::ppt_single_page(page_number.max(0) as usize),
                    )
                    .at(crate::core::pipeline::hook::CONTRACT_ORDER),
                ],
            )
            .prompt;

        stage(
            &format!("ppt_page_plan_{page_number}"),
            &format!("规划第 {page_number} 页内容"),
        );
        let no_images: Vec<ImageInput> = Vec::new();
        let step_name = format!("ppt_page_plan_{page_number}");
        let step_label = format!("规划第 {page_number} 页内容");
        let call = DesignCall {
            profile: self.profile,
            system_prompt: self.system_prompt,
            user_prompt: &page_prompt,
            images: &no_images,
            proxy_url: self.proxy_url,
            response_sink: None,
            log: Some(DesignStep {
                sink: self.design_log,
                step: &step_name,
                label: &step_label,
            }),
            cache: self.cache.map(|cache| StepCache {
                cache,
                scope: &step_name,
            }),
        };
        let page_text = call.first().await?;

        let expected = page_number.max(0) as usize;
        let validated = parse_validate_or_fill(&call, &page_text, |value| {
            validate_ppt_single_page(value, expected, Some(self.template_analysis))
        })
        .await?;
        Ok(validated.value)
    }
}

pub struct SlideRun<'a> {
    pub prompts: &'a PromptStore,
    pub config: &'a AppConfig,
    pub design_profile: &'a ModelProfile,
    pub implement_profile: Option<&'a ModelProfile>,
    pub search_profile: &'a ModelProfile,
    pub template_image: ImageInput,
    pub material_context: String,
    pub design_log: DesignSink<'a>,
    /// Recalled cases for the advisor step; empty means the step is skipped.
    pub similar_cases: Vec<MemoryCase>,
    pub fingerprint: Option<Fingerprint>,
    /// 失败或停止后重跑这个任务时的答案缓存；没有就每次都真的调用。
    pub cache: Option<&'a AnswerCache>,
}

/// What a slide run hands back: one image per page and the deck-level design
/// product (master analysis plus outline) the memory keeps as the case.
pub struct SlideOutput {
    pub pages: Vec<String>,
    pub final_prompts: Vec<String>,
    pub design: Value,
}

impl SlideRun<'_> {
    pub async fn run(
        &self,
        payload: &PptSlidePayload,
        stage: StageSink<'_>,
    ) -> AppResult<SlideOutput> {
        let proxy = self.config.proxy_url.as_deref();
        let page_count = payload.page_count;
        let custom_prompt = payload.custom_prompt_text();
        let page_plan_concurrency =
            resolve_ppt_concurrency(self.config.ppt_page_plan_concurrency, page_count);
        let image_concurrency =
            resolve_ppt_concurrency(self.config.ppt_image_concurrency, page_count);

        if self.material_context.trim().is_empty() {
            return Err(AppError::new(
                "invalid_payload",
                "PPT slide requires material text or material files",
            ));
        }

        stage("ppt_visual_assets", "联网查询产品与工具的视觉素材");
        // 检索只吃资料与附件：约束（字体、底色一类）不是可检索的视觉主体。
        let subjects = pick_visual_subjects(
            &self.material_context,
            self.design_profile,
            self.prompts,
            proxy,
            self.cache,
        )
        .await;
        let visual_asset_context = build_visual_asset_context(
            &subjects,
            self.search_profile,
            proxy,
            self.design_log,
            self.cache,
        )
        .await?;
        let visual_asset_prompt = visual_asset_context_text(&visual_asset_context);
        (self.design_log)(DesignLog::End {
            step: "ppt_visual_assets",
            label: "联网查询 · 视觉证据",
            text: &visual_asset_log_text(&visual_asset_context),
            ok: true,
        });
        let ppt_output = self
            .implement_profile
            .map(ppt_output_defaults)
            .unwrap_or_default();

        let mut hooks = ContextHooks::default();
        hooks.register(
            StaticContext::new(
                "analysis-contract",
                &[STAGE_ANALYZE],
                "Output Contract",
                contract::template_analysis(),
            )
            .contract(),
        );
        hooks.register(StaticContext::new(
            "content-inventory",
            &[STAGE_OUTLINE, STAGE_PAGE],
            "Material",
            truncate_text(&self.material_context, PPT_PLAN_MATERIAL_LIMIT),
        ));
        hooks.register(
            StaticContext::new(
                "search-evidence",
                &[STAGE_OUTLINE, STAGE_PAGE],
                "Visual Asset Search Context",
                visual_asset_prompt,
            )
            .at(5),
        );
        hooks.register(
            StaticContext::new(
                "custom-prompt",
                &[STAGE_OUTLINE, STAGE_PAGE],
                "Custom Prompt",
                custom_prompt.clone(),
            )
            .at(20),
        );
        hooks.register(
            StaticContext::new(
                "outline-contract",
                &[STAGE_OUTLINE],
                "Output Contract",
                contract::ppt_outline(page_count),
            )
            .contract(),
        );

        let analyzer_assets = self.prompts.load_all(&[
            "global/system.md",
            "modes/ppt_slide/analyzer.md",
            "modes/ppt_slide/master_rules.md",
        ])?;
        let analyzer_prompt = hooks
            .compose(STAGE_ANALYZE, &analyzer_assets, Vec::new())
            .prompt;
        let template_images = vec![self.template_image.clone()];
        let analyzer_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &analyzer_assets[0].content,
            user_prompt: &analyzer_prompt,
            images: &template_images,
            proxy_url: proxy,
            response_sink: None,
            log: Some(DesignStep {
                sink: self.design_log,
                step: "ppt_analyze",
                label: "分析母版",
            }),
            cache: self.cache.map(|cache| StepCache {
                cache,
                scope: STAGE_ANALYZE,
            }),
        };

        stage("ppt_analyze", "分析母版版式");
        let analysis_text = analyzer_call.first().await?;

        stage("ppt_parse_template", "校验母版分析结果");
        let template_analysis =
            parse_validate_or_fill(&analyzer_call, &analysis_text, validate_template_analysis)
                .await?
                .parsed;

        let page_assets = self.prompts.load_all(&[
            "global/system.md",
            "modes/ppt_slide/design.md",
            "global/expression.md",
            "styles/academic_ppt.md",
        ])?;
        // The master contract distils the template image into text; from here
        // on no stage sees the image again, only this section.
        hooks.register(
            StaticContext::new(
                "master-contract",
                &[STAGE_OUTLINE, STAGE_PAGE],
                "Template Analysis",
                compact_template_analysis(&template_analysis),
            )
            .at(-20),
        );

        if let Some(fingerprint) = self
            .fingerprint
            .as_ref()
            .filter(|_| !self.similar_cases.is_empty())
        {
            stage(
                "ppt_advisor",
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
                Some(advice) => {
                    hooks.register(AdvisorHook::new(&[STAGE_OUTLINE, STAGE_PAGE], advice))
                }
                None => stage("ppt_advisor", "没有可用的历史建议，按新任务继续"),
            }
        }

        let outline_composed = hooks.compose(
            STAGE_OUTLINE,
            &page_assets,
            vec![
                Section::new(
                    "Task Mode",
                    "Deck outline mode. Plan only the deck narrative and lightweight page briefs. \
                     Do not write page-level implement_prompt.",
                )
                .at(-30),
                Section::new("Page Count", page_count.to_string()).at(10),
            ],
        );
        stage(
            "ppt_outline_prompt",
            &format!(
                "准备整套大纲规划（注入 {}）",
                injected_summary(&outline_composed)
            ),
        );
        let outline_prompt = outline_composed.prompt;

        let no_images: Vec<ImageInput> = Vec::new();
        let outline_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &page_assets[0].content,
            user_prompt: &outline_prompt,
            images: &no_images,
            proxy_url: proxy,
            response_sink: None,
            log: Some(DesignStep {
                sink: self.design_log,
                step: "ppt_outline",
                label: "规划整套大纲",
            }),
            cache: self.cache.map(|cache| StepCache {
                cache,
                scope: STAGE_OUTLINE,
            }),
        };

        stage("ppt_outline", "规划整套大纲");
        let outline_text = outline_call.first().await?;

        stage("ppt_parse_outline", "校验整套大纲");
        let deck_outline = parse_validate_or_fill(&outline_call, &outline_text, |value| {
            validate_ppt_outline(value, page_count)
        })
        .await?
        .value;
        let page_briefs = deck_outline["page_briefs"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        stage("ppt_page_plan_queue", &format!("排队规划 {page_count} 页"));
        let plan_context = PagePlanContext {
            profile: self.design_profile,
            system_prompt: &page_assets[0].content,
            page_assets: &page_assets,
            template_analysis: &template_analysis,
            hooks: &hooks,
            deck_outline: &deck_outline,
            proxy_url: proxy,
            design_log: self.design_log,
            cache: self.cache,
        };
        let planned = run_ordered(
            page_briefs
                .iter()
                .map(|brief| plan_context.plan(brief, stage))
                .collect(),
            page_plan_concurrency,
        )
        .await?;

        stage("ppt_merge_pages", "汇总页面规划");
        let pages_json = json!({ "pages": planned });
        let pages = validate_ppt_pages(&pages_json, page_count, Some(&template_analysis))?;
        let pages = apply_master_prompt_prefix(&pages, &template_analysis)?;
        let final_prompts = pages
            .iter()
            .map(|page| {
                page["implement_prompt"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();

        let rendered = if self.implement_profile.is_some() {
            stage("ppt_implement_queue", &format!("排队生成图片"));
            run_ordered(
                pages
                    .iter()
                    .map(|page| self.implement_page(page, &ppt_output, proxy, stage))
                    .collect(),
                image_concurrency,
            )
            .await?
        } else {
            Vec::new()
        };
        let page_plans: Vec<Value> = pages
            .iter()
            .map(|page| {
                json!({
                    "page": page["page"],
                    "title": page["title"],
                    "selected_template": page["selected_template"],
                    "body_layout_plan": page["body_layout_plan"],
                })
            })
            .collect();
        Ok(SlideOutput {
            pages: rendered,
            final_prompts,
            design: json!({
                "template_analysis": template_analysis,
                "deck_outline": deck_outline,
                "pages": page_plans,
            }),
        })
    }

    async fn implement_page(
        &self,
        page: &Value,
        ppt_output: &Map<String, Value>,
        proxy: Option<&str>,
        stage: StageSink<'_>,
    ) -> AppResult<String> {
        let page_number = page["page"].as_i64().unwrap_or(0);
        let prompt = page["implement_prompt"].as_str().unwrap_or_default();
        let step = format!("ppt_implement_{page_number}");
        stage(&step, &format!("生成第 {page_number} 页图片"));
        let profile = self
            .implement_profile
            .ok_or_else(|| AppError::new("missing_profile", "Missing implement profile"))?;
        // 每页各自一份缓存：上次跑到第 6 页失败，重跑时前 5 页的图不再重新生成。
        let identity = profile_identity(profile);
        let overrides_json = serde_json::to_string(ppt_output).unwrap_or_default();
        cached_image(
            self.cache.map(|cache| StepCache {
                cache,
                scope: &step,
            }),
            &[
                identity.as_bytes(),
                prompt.as_bytes(),
                overrides_json.as_bytes(),
            ],
            || ImplementClient::generate(profile, prompt, &[], ppt_output, proxy),
        )
        .await
    }
}

/// Custom constraints may name the only concrete subject; prioritize them before bounded material.
#[cfg(test)]
mod tests {
    use super::*;

    fn profile(protocol: &str, defaults: &[(&str, &str)]) -> ModelProfile {
        let mut output_defaults = Map::new();
        for (key, value) in defaults {
            output_defaults.insert((*key).to_string(), Value::String((*value).to_string()));
        }
        ModelProfile {
            id: "test".to_string(),
            role: "implement".to_string(),
            name: "test".to_string(),
            protocol: protocol.to_string(),
            base_url: "https://example.com".to_string(),
            model: "test-model".to_string(),
            api_key: None,
            api_version: None,
            headers: Map::new(),
            timeout_seconds: 600,
            max_retries: 1,
            output_defaults,
            has_api_key: None,
            api_key_hint: None,
        }
    }

    #[test]
    fn concurrency_is_clamped_to_page_count() {
        assert_eq!(resolve_ppt_concurrency(None, 5), 5);
        assert_eq!(resolve_ppt_concurrency(Some(3), 5), 3);
        assert_eq!(resolve_ppt_concurrency(Some(9), 5), 5);
        assert_eq!(resolve_ppt_concurrency(Some(0), 5), 1);
        assert_eq!(resolve_ppt_concurrency(Some(-2), 5), 1);
        assert_eq!(resolve_ppt_concurrency(None, 0), 1);
    }

    #[test]
    fn truncate_counts_characters() {
        let text = "科研图".repeat(10);
        let truncated = truncate_text(&text, 12);
        assert!(truncated.starts_with(&"科研图".repeat(4)));
        assert!(truncated.contains("[Truncated from 30 chars to 12 chars"));
        assert_eq!(truncate_text("short", 12), "short");
    }

    #[test]
    fn image2_output_merges_over_fallback() {
        let defaults = ppt_output_defaults(&profile("image2", &[("response_format", "url")]));
        assert_eq!(defaults["response_format"], json!("url"));
        assert_eq!(defaults["size"], json!("1200x675"));
        assert_eq!(defaults["output_format"], json!("png"));
    }

    #[test]
    fn banana2_output_is_restricted_to_known_fields() {
        let defaults = ppt_output_defaults(&profile(
            "banna2",
            &[("image_size", "2K"), ("size", "1200x675")],
        ));
        assert_eq!(defaults["image_size"], json!("2K"));
        assert_eq!(defaults["aspect_ratio"], json!("16:9"));
        assert!(!defaults.contains_key("size"));
        assert!(!defaults.contains_key("quality"));
    }

    #[test]
    fn empty_output_values_are_dropped() {
        let defaults = ppt_output_defaults(&profile(
            "openai_chat",
            &[("size", ""), ("quality", "auto")],
        ));
        assert!(!defaults.contains_key("size"));
        assert_eq!(defaults["quality"], json!("auto"));
    }

    #[test]
    fn material_context_marks_truncated_files() {
        let long_text = "数".repeat(MATERIAL_TEXT_LIMIT + 40);
        let asset = MaterialAsset::new(
            "notes.md".to_string(),
            "text/plain".to_string(),
            2048,
            "markdown".to_string(),
            &long_text,
        );
        assert!(asset.truncated);
        assert_eq!(asset.text_length, MATERIAL_TEXT_LIMIT + 40);

        let context = compose_material_context("  用户输入  ", &[asset]);
        assert!(context.starts_with("User text material:\n用户输入"));
        assert!(context.contains("\n\n---\n\n"));
        assert!(context.contains("parser=markdown"));
        assert!(context.contains(&format!(
            "excerpt={MATERIAL_TEXT_LIMIT}/{} chars",
            MATERIAL_TEXT_LIMIT + 40
        )));
    }

    #[test]
    fn material_context_falls_back_for_empty_files() {
        let asset = MaterialAsset::new(
            "scan.pdf".to_string(),
            "application/pdf".to_string(),
            10,
            "pdf".to_string(),
            "",
        );
        let context = compose_material_context("", &[asset]);
        assert!(context.contains("No extractable text available"));
        assert!(!context.contains("User text material"));
    }

    #[test]
    fn adjacent_context_covers_deck_edges() {
        let outline = json!({"page_briefs": [
            {"page": 1, "title": "封面"},
            {"page": 2, "title": "方法"},
            {"page": 3, "title": "结论"}
        ]});
        let first = adjacent_page_context(&outline, 1);
        assert!(first["previous"].is_null());
        assert_eq!(first["next"]["title"], json!("方法"));

        let middle = adjacent_page_context(&outline, 2);
        assert_eq!(middle["previous"]["title"], json!("封面"));
        assert_eq!(middle["next"]["title"], json!("结论"));

        let last = adjacent_page_context(&outline, 3);
        assert_eq!(last["previous"]["title"], json!("方法"));
        assert!(last["next"].is_null());
    }

    #[test]
    fn compact_analysis_keeps_master_style_fields() {
        let analysis = json!({"template_analysis": {
            "master_style_summary": "红灰学术母版",
            "master_style_spec": {
                "canvas": {"aspect_ratio": "16:9"},
                "palette": {"primary": "#B40000"},
                "typography": {"title": "32px"},
                "page_layout_rules": "worker 用不上的长文"
            },
            "global_constraints": ["keep title band"],
            "immutable_elements": ["title region"],
            "page_layout_rules": "Body varies inside safe margins."
        }});
        let compact = compact_template_analysis(&analysis);
        for token in [
            "canvas",
            "palette",
            "typography",
            "forbidden_deviations",
            "红灰学术母版",
        ] {
            assert!(compact.contains(token), "compact analysis missing {token}");
        }
        assert!(!compact.contains("worker 用不上的长文"));
        assert!(compact.contains("Body varies inside safe margins."));

        let full = serde_json::to_string_pretty(&analysis).unwrap();
        assert!(compact.chars().count() < full.chars().count());
        assert!(compact.chars().count() <= PPT_PLAN_TEMPLATE_LIMIT + 120);
    }

    #[test]
    fn payload_validation_rejects_out_of_range_pages() {
        let base = PptSlidePayload {
            template_asset_id: "asset-1".to_string(),
            template_id: None,
            material_text: "内容".to_string(),
            material_asset_ids: Vec::new(),
            page_count: 3,
            custom_prompt: None,
        };
        assert!(validate_payload(&base).is_ok());
        assert_eq!(base.custom_prompt_text(), "None");

        let mut too_many = base.clone();
        too_many.page_count = MAX_PAGE_COUNT + 1;
        assert!(validate_payload(&too_many).is_err());

        let mut zero = base.clone();
        zero.page_count = 0;
        assert!(validate_payload(&zero).is_err());

        let mut no_template = base.clone();
        no_template.template_asset_id = "   ".to_string();
        assert!(validate_payload(&no_template).is_err());
    }

    #[test]
    fn payload_validation_accepts_master_from_template_library() {
        let from_library = PptSlidePayload {
            template_asset_id: String::new(),
            template_id: Some("tpl-1".to_string()),
            material_text: "内容".to_string(),
            material_asset_ids: Vec::new(),
            page_count: 2,
            custom_prompt: None,
        };
        assert!(validate_payload(&from_library).is_ok());
        assert_eq!(from_library.template_ref(), Some("tpl-1"));
        assert_eq!(from_library.asset_ref(), None);

        let mut blank = from_library.clone();
        blank.template_id = Some("   ".to_string());
        assert!(validate_payload(&blank).is_err());
    }

    #[tokio::test]
    async fn ordered_run_preserves_order_and_caps_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..6)
            .map(|index| {
                let running = Arc::clone(&running);
                let peak = Arc::clone(&peak);
                async move {
                    let current = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20 - index * 3)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok(index)
                }
            })
            .collect();

        let results = run_ordered(tasks, 2).await.unwrap();
        assert_eq!(results, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(peak.load(Ordering::SeqCst), 2, "并发上限未被正确执行");
    }

    #[tokio::test]
    async fn ordered_run_propagates_first_error() {
        let tasks: Vec<_> = (0..4)
            .map(|index| async move {
                if index == 2 {
                    return Err(AppError::new("boom", "page 3 failed"));
                }
                Ok(index)
            })
            .collect();
        let error = run_ordered(tasks, 2).await.unwrap_err();
        assert_eq!(error.message, "page 3 failed");
    }
}
