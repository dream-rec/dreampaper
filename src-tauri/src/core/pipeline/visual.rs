//! 视觉素材检索：先让模型从资料里挑出值得检索真实外观的主体，再逐条联网查询，
//! 把客观描述与来源链接作为纯文本线索交给制图阶段。

use crate::core::config::ModelProfile;
use crate::core::pipeline::cache::{AnswerCache, StepCache};
use crate::core::pipeline::runner::DesignCall;
use crate::core::prompt::PromptStore;
use crate::core::search::{
    filter_results, SearchClient, SearchResult, SearchTrace, GROK_SEARCH_PROTOCOL,
};
use crate::error::AppResult;
use crate::event::{DesignLog, DesignSink};

pub const VISUAL_ASSET_SEARCH_TERM_LIMIT: usize = 8;
pub const VISUAL_ASSET_SEARCH_RESULT_LIMIT: usize = 3;
pub const VISUAL_CONTEXT_LIMIT: usize = 8192;
const MATERIAL_TEXT_LIMIT: usize = 6000;

/// 识别主体的提示词资源；哪些词值得检索由模型读资料自己判断，不再维护主体词库。
pub const VISUAL_SUBJECTS_KEY: &str = "global/visual_subjects.md";

/// 缓存作用域：识别主体与逐条检索各一个，不跟设计步骤混在一起。
const SUBJECTS_SCOPE: &str = "visual_subjects";
const SEARCH_SCOPE: &str = "visual_search";

/// 一次检索的缓存内容：源文本加发出去的报文，重跑时日志也能一模一样地重建。
#[derive(serde::Deserialize, serde::Serialize)]
struct CachedSearch {
    request: Option<String>,
    response: Option<String>,
    results: Vec<SearchResult>,
}

/// 让模型挑出这份资料里值得检索真实外观的主体。
///
/// 内部固定流程：不写设计日志、不单独占一个阶段。识别失败、提示词缺失或没有可用主体时
/// 返回空列表，检索阶段随之降级（图照画，只是没有客观外观描述可依）。
pub async fn pick_visual_subjects(
    material: &str,
    design_profile: &ModelProfile,
    prompts: &PromptStore,
    proxy_url: Option<&str>,
    cache: Option<&AnswerCache>,
) -> Vec<String> {
    let text: String = material.chars().take(MATERIAL_TEXT_LIMIT).collect();
    if text.trim().is_empty() {
        return Vec::new();
    }
    let Ok(prompt) = prompts.load(VISUAL_SUBJECTS_KEY) else {
        return Vec::new();
    };
    let reply = DesignCall {
        profile: design_profile,
        system_prompt: &prompt.content,
        user_prompt: &text,
        images: &[],
        proxy_url,
        response_sink: None,
        log: None,
        cache: cache.map(|cache| StepCache {
            cache,
            scope: SUBJECTS_SCOPE,
        }),
    }
    .first()
    .await;
    match reply {
        Ok(reply) => visual_subjects_from_reply(&reply, &text),
        Err(_) => Vec::new(),
    }
}

/// 模型可能把数组包在代码块或一句说明里，取第一个 JSON 数组。
///
/// 候选必须真的出现在资料原文里（大小写、空格、标点不计），这样模型无法凭记忆塞进来
/// 一个原文没提过的品牌——顺带保证字体、色值这类约束描述不会被当成视觉主体。
fn visual_subjects_from_reply(reply: &str, material: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (reply.find('['), reply.rfind(']')) else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&reply[start..=end]) else {
        return Vec::new();
    };

    let haystack = compact(material);
    let mut seen: Vec<String> = Vec::new();
    let mut subjects: Vec<String> = Vec::new();
    for item in items {
        let Some(candidate) = item.as_str() else {
            continue;
        };
        let subject = candidate.trim();
        let key = compact(subject);
        if key.is_empty() || !haystack.contains(&key) || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        subjects.push(subject.to_string());
        if subjects.len() >= VISUAL_ASSET_SEARCH_TERM_LIMIT {
            break;
        }
    }
    subjects
}

/// 只留字母数字并折叠大小写：`Grok Bot` 与原文 `grokbot` 视为同一个主体。
fn compact(text: &str) -> String {
    text.chars()
        .flat_map(|ch| ch.to_lowercase())
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct VisualAssetItem {
    pub term: String,
    pub results: Vec<SearchResult>,
    pub provider: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct VisualAssetContext {
    pub enabled: bool,
    pub degraded: bool,
    pub terms: Vec<String>,
    pub items: Vec<VisualAssetItem>,
    pub message: String,
}

/// 一次检索的结果：来自缓存（连同当时的报文）或刚刚发出的请求。
enum SearchOutcome {
    Cached(CachedSearch),
    Live {
        trace: SearchTrace,
        results: AppResult<Vec<SearchResult>>,
    },
}

/// 逐主体检索，每个主体一次查询，报文与原始返回先写进设计日志再决定冒泡还是降级。
pub async fn build_visual_asset_context(
    subjects: &[String],
    search_profile: &ModelProfile,
    proxy_url: Option<&str>,
    design_log: DesignSink<'_>,
    cache: Option<&AnswerCache>,
) -> AppResult<VisualAssetContext> {
    if subjects.is_empty() {
        return Ok(VisualAssetContext {
            enabled: true,
            degraded: true,
            terms: Vec::new(),
            items: Vec::new(),
            message: "No explicit product/tool/equipment terms were detected.".to_string(),
        });
    }

    let max_results = search_profile
        .output_defaults
        .get("max_results")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(VISUAL_ASSET_SEARCH_RESULT_LIMIT as u64)
        .clamp(1, 8) as usize;

    let futures = subjects.iter().map(|subject| async move {
        // 逐条检索也缓存：上一次跑到一半失败时搜到的东西不该再买一遍。
        let step = cache.map(|cache| StepCache {
            cache,
            scope: SEARCH_SCOPE,
        });
        let key = step.map(|step| {
            let identity = format!(
                "{}|{}|{}|{max_results}",
                search_profile.protocol, search_profile.base_url, search_profile.model
            );
            step.key(&[identity.as_bytes(), subject.as_bytes()])
        });
        if let (Some(step), Some(key)) = (step, key.as_deref()) {
            if let Some(cached) = step
                .text(key)
                .and_then(|text| serde_json::from_str::<CachedSearch>(&text).ok())
            {
                return (subject, SearchOutcome::Cached(cached));
            }
        }
        let outcome = SearchClient::search(search_profile, subject, max_results, proxy_url).await;
        if let (Some(step), Some(key), Ok(results)) = (step, key.as_deref(), &outcome.results) {
            let entry = CachedSearch {
                request: outcome.trace.request.clone(),
                response: outcome.trace.response.clone(),
                results: results.clone(),
            };
            if let Ok(text) = serde_json::to_string(&entry) {
                step.put_text(key, &text);
            }
        }
        (
            subject,
            SearchOutcome::Live {
                trace: outcome.trace,
                results: outcome.results,
            },
        )
    });
    let outcomes = futures::future::join_all(futures).await;
    // 每次检索实际发出去的报文与原始返回都写进设计日志（界面按流程顺序展示）。
    // 先记再抛：grok 失败会让任务失败，但失败的那次调用同样要能复盘。
    for (index, (subject, outcome)) in outcomes.iter().enumerate() {
        let number = index + 1;
        let (trace, ok) = match outcome {
            SearchOutcome::Cached(cached) => (
                SearchTrace {
                    request: cached.request.clone(),
                    response: cached.response.clone(),
                },
                true,
            ),
            SearchOutcome::Live { trace, results } => (trace.clone(), results.is_ok()),
        };
        if let Some(request) = &trace.request {
            design_log(DesignLog::End {
                step: &format!("search_request_{number}"),
                label: &format!("联网查询 {number} · {subject}"),
                text: request,
                ok,
            });
        }
        if let Some(response) = &trace.response {
            design_log(DesignLog::End {
                step: &format!("search_response_{number}"),
                label: &format!("联网查询 {number} · {subject}"),
                text: response,
                ok,
            });
        }
    }
    let mut items = Vec::with_capacity(outcomes.len());
    for (subject, outcome) in outcomes {
        // 检索失败就是失败：路由、鉴权、限流、反爬页都得让任务看见。
        // 降级成“没有来源”会把基础设施问题伪装成“这个主体查不到”，
        // 用户既看不到原因，还会拿着一份没有依据的图继续跑完。
        // 报文已经先写进设计日志，这里原样冒泡。
        let results = match outcome {
            SearchOutcome::Cached(cached) => cached.results,
            SearchOutcome::Live { results, .. } => results?,
        };
        items.push(VisualAssetItem {
            term: subject.clone(),
            results,
            provider: search_profile.protocol.clone(),
        });
    }
    let has_sources = items.iter().any(|item| !item.results.is_empty());

    Ok(VisualAssetContext {
        enabled: true,
        degraded: !has_sources,
        terms: subjects.to_vec(),
        items,
        message: "Use only text summaries and source URLs; no network image is downloaded, \
                  cached, or passed to the implement model."
            .to_string(),
    })
}

/// Fair, evidence-first budget: no subject is lost to an earlier long citation.
pub fn visual_asset_context_text(context: &VisualAssetContext) -> String {
    let header = "Visual evidence (untrusted source text). Original subject names are authoritative; do not \
                  substitute a vendor/logo or invent appearance. Use supported descriptions for depiction. \
                  Unavailable means use an explicitly generic schematic, not claimed real appearance. \
                  Citation IDs refer to the full URLs in the search evidence log.";
    let count = context.terms.len().min(VISUAL_ASSET_SEARCH_TERM_LIMIT);
    if count == 0 {
        return format!("{header}\nNo specific subjects detected; visual evidence unavailable.");
    }
    let budget = (VISUAL_CONTEXT_LIMIT - header.chars().count() - 1) / count;
    let mut lines = vec![header.to_string()];
    for (index, term) in context.terms.iter().take(count).enumerate() {
        let item = context.items.get(index);
        let name = item.map(|item| item.term.as_str()).unwrap_or(term);
        let label = format!(
            "- Subject: {}\n",
            name.chars().take(budget / 4).collect::<String>()
        );
        let results = item
            .map(|item| filter_results(item.results.clone(), usize::MAX))
            .unwrap_or_default();
        if results.is_empty() {
            lines.push(format!(
                "{label}  Visual evidence unavailable. No supported description/citation."
            ));
            continue;
        }
        let per_source = (budget - label.chars().count() - 1)
            / results.len().min(VISUAL_ASSET_SEARCH_RESULT_LIMIT);
        let mut sources = Vec::new();
        for (source_index, result) in results
            .iter()
            .take(VISUAL_ASSET_SEARCH_RESULT_LIMIT)
            .enumerate()
        {
            let citation = format!("[{}.{}]", index + 1, source_index + 1);
            let description = result
                .snippet
                .chars()
                .take((per_source / 2).max(1))
                .collect::<String>();
            let prefix = format!("  {citation} {description}\n  Source: ");
            let remaining = per_source.saturating_sub(prefix.chars().count() + 1);
            let reference = if result.url.chars().count() <= remaining {
                result.url.clone()
            } else {
                format!("{citation} (full URL in search log)")
            };
            sources.push(format!("{prefix}{reference}"));
        }
        lines.push(format!("{label}{}", sources.join("\n")));
    }
    lines.join("\n")
}

/// Persist complete filtered descriptions/citations outside the planner's bounded context.
pub fn visual_asset_log_text(context: &VisualAssetContext) -> String {
    let mut lines = vec![
        visual_asset_context_text(context),
        "\nFull source references:".to_string(),
    ];
    for (index, item) in context
        .items
        .iter()
        .take(VISUAL_ASSET_SEARCH_TERM_LIMIT)
        .enumerate()
    {
        for (source_index, result) in filter_results(item.results.clone(), usize::MAX)
            .iter()
            .enumerate()
        {
            lines.push(format!(
                "[{}.{}] {} | {} | {}",
                index + 1,
                source_index + 1,
                result.snippet,
                result.title,
                result.url
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subjects(reply: &str, material: &str) -> Vec<String> {
        visual_subjects_from_reply(reply, material)
    }

    fn search_profile(protocol: &str) -> ModelProfile {
        // 不带 api_key：检索请求在发出去之前就会失败，测试不依赖真实网络。
        serde_json::from_value(serde_json::json!({
            "id": "search", "role": "search", "name": "Search", "protocol": protocol,
            "base_url": "https://example.test/v1", "model": "Build/deployed-model",
            "headers": {}, "max_retries": 0, "output_defaults": {}
        }))
        .unwrap()
    }

    fn context(subjects: &[&str], results: Vec<SearchResult>) -> VisualAssetContext {
        VisualAssetContext {
            enabled: true,
            degraded: results.is_empty(),
            terms: subjects.iter().map(|term| term.to_string()).collect(),
            items: subjects
                .iter()
                .map(|term| VisualAssetItem {
                    term: term.to_string(),
                    results: results.clone(),
                    provider: "grok_search".to_string(),
                })
                .collect(),
            message: String::new(),
        }
    }

    #[test]
    fn subjects_keep_the_spelling_of_the_material() {
        let material = "本方案用 muse 与 dots 两个模型，部署在 Kubernetes 上。";
        assert_eq!(
            subjects("[\"muse\", \"dots\", \"Kubernetes\"]", material),
            vec!["muse", "dots", "Kubernetes"]
        );
        // 模型把名字规范化了也算命中：比对时忽略大小写、空格与标点。
        assert_eq!(
            subjects(
                "```json\n[\"Grok Bot\", \"dots.llm1\"]\n```",
                "介绍 grokbot 和 dots.llm1"
            ),
            vec!["Grok Bot", "dots.llm1"]
        );
    }

    #[test]
    fn subjects_absent_from_the_material_are_dropped() {
        // 识别只看资料原文：约束里的字体与色值不在其中，因此模型就算报也入不了检索。
        let material = "图中展示 Grok Bot 的界面。";
        assert_eq!(
            subjects(
                "[\"Microsoft\", \"FFFFFF\", \"微软雅黑\", \"Grok Bot\"]",
                material
            ),
            vec!["Grok Bot"]
        );
        assert!(subjects("[\"OpenAI\", \"PyTorch\"]", material).is_empty());
    }

    #[test]
    fn subjects_are_deduped_capped_and_only_strings_count() {
        let material = "muse 是模型，dots 也是模型，都部署在 Kubernetes 上。";
        // 大小写重复只留一个；非字符串元素直接跳过。
        assert_eq!(
            subjects("[\"muse\", \"Muse\", 7, {\"term\": \"dots\"}]", material),
            vec!["muse"]
        );
        assert_eq!(
            subjects("[\"muse\", \"dots\", \"Kubernetes\"]", material),
            vec!["muse", "dots", "Kubernetes"]
        );

        let many = (1..=12)
            .map(|index| format!("\"subject{index}\""))
            .collect::<Vec<_>>()
            .join(",");
        let material = (1..=12)
            .map(|index| format!("subject{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            subjects(&format!("[{many}]"), &material).len(),
            VISUAL_ASSET_SEARCH_TERM_LIMIT
        );
    }

    #[test]
    fn unusable_replies_yield_no_subjects() {
        for reply in ["", "没有需要检索的主体", "[\"unclosed", "{not: json}", "[]"] {
            assert!(subjects(reply, "muse dots").is_empty(), "{reply}");
        }
    }

    #[test]
    fn context_text_pushes_real_depiction() {
        let text = visual_asset_context_text(&context(
            &["离心机"],
            vec![SearchResult {
                title: "离心机产品页".to_string(),
                url: "https://example.com".to_string(),
                snippet: "台式高速离心机".to_string(),
            }],
        ));
        assert!(text.contains("Original subject names are authoritative"));
        assert!(text.contains("台式高速离心机"));
        assert!(text.contains("https://example.com"));
        assert!(text.contains("- Subject: 离心机"));
        assert!(!text.to_lowercase().contains("base64"));
    }

    #[test]
    fn no_subjects_means_visual_evidence_is_unavailable() {
        let empty = VisualAssetContext {
            enabled: true,
            degraded: true,
            terms: Vec::new(),
            items: Vec::new(),
            message: String::new(),
        };
        assert!(visual_asset_context_text(&empty).contains("visual evidence unavailable"));
    }

    #[test]
    fn every_subject_keeps_descriptions_with_long_names_and_references() {
        for long_names in [false, true] {
            let terms: Vec<String> = (0..8)
                .map(|index| {
                    format!(
                        "subject{index}{}",
                        if long_names {
                            "名".repeat(10000)
                        } else {
                            String::new()
                        }
                    )
                })
                .collect();
            let mut context = VisualAssetContext {
                enabled: true,
                degraded: false,
                terms: terms.clone(),
                message: String::new(),
                items: terms
                    .iter()
                    .enumerate()
                    .map(|(index, term)| VisualAssetItem {
                        term: term.clone(),
                        provider: "grok_search".into(),
                        results: (0..3)
                            .map(|source| SearchResult {
                                title: "Official source".into(),
                                url: format!(
                                    "https://example.test/{index}/{source}/{}",
                                    "x".repeat(10000)
                                ),
                                snippet: format!(
                                    "description-{index}-{source} silver arms {}",
                                    "feature ".repeat(100)
                                ),
                            })
                            .collect(),
                    })
                    .collect(),
            };
            let text = visual_asset_context_text(&context);
            assert!(
                text.chars().count() <= VISUAL_CONTEXT_LIMIT,
                "{}",
                text.chars().count()
            );
            let full = visual_asset_log_text(&context);
            for index in 0..8 {
                assert!(text.contains(&format!("subject{index}")));
                for source in 0..3 {
                    assert!(text.contains(&format!("description-{index}-{source}")));
                    assert!(text.contains(&format!("[{}.{}]", index + 1, source + 1)));
                    assert!(full.contains(&context.items[index].results[source].url));
                }
            }
            context.items[7].results.clear();
            assert!(visual_asset_context_text(&context).contains("Visual evidence unavailable"));
        }
    }

    #[tokio::test]
    async fn search_request_failures_fail_the_job_instead_of_degrading() {
        let subjects = vec!["Docker".to_string()];

        // 每个协议一视同仁：路由、鉴权、限流、反爬页都必须让任务失败，
        // 否则基础设施问题会被伪装成“这个主体查不到”。
        for protocol in [GROK_SEARCH_PROTOCOL, "duckduckgo", "tavily", "openai_chat"] {
            let profile = ModelProfile {
                // 本地必然拒绝连接：测试不碰真网络，也不靠超时来失败。
                base_url: "http://127.0.0.1:1".to_string(),
                ..search_profile(protocol)
            };
            build_visual_asset_context(&subjects, &profile, None, &|_: DesignLog<'_>| {}, None)
                .await
                .expect_err("请求失败必须让任务失败，而不是降级成“没有来源”");
        }

        // 缺凭据同样是失败，不是降级。
        let error = build_visual_asset_context(
            &subjects,
            &search_profile(GROK_SEARCH_PROTOCOL),
            None,
            &|_: DesignLog<'_>| {},
            None,
        )
        .await
        .expect_err("缺密钥必须让任务失败");
        assert_eq!(error.code, "missing_api_key");
    }

    #[tokio::test]
    async fn a_challenge_page_fails_the_job_with_its_reason() {
        // DuckDuckGo 被拦住时返回 202 反爬页：不能安静地变成“没有来源”。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buffer = [0; 4096];
            let _ = stream.read(&mut buffer).await.unwrap();
            let body = "<div class=\"anomaly-modal\">Unfortunately, bots use DuckDuckGo too.</div>";
            let response = format!(
                "HTTP/1.1 202 Test\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let profile = ModelProfile {
            base_url: base,
            ..search_profile("duckduckgo")
        };

        let error = build_visual_asset_context(
            &["Docker".to_string()],
            &profile,
            None,
            &|_: DesignLog<'_>| {},
            None,
        )
        .await
        .expect_err("反爬页必须让任务失败");
        server.await.unwrap();
        assert!(
            error.message.contains("anti-bot challenge"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn grok_search_traces_the_request_of_a_failed_call() {
        let logs = std::sync::Mutex::new(Vec::new());
        let sink = |log: DesignLog<'_>| {
            if let DesignLog::End { step, text, .. } = log {
                logs.lock()
                    .unwrap()
                    .push((step.to_string(), text.to_string()));
            }
        };

        let error = build_visual_asset_context(
            &["Docker".to_string()],
            &search_profile(GROK_SEARCH_PROTOCOL),
            None,
            &sink,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "missing_api_key");

        // 每个词一次检索：发出去的请求体必须留下来，且两种搜索工具都已声明。
        let logs = logs.lock().unwrap();
        assert!(!logs.is_empty());
        for (step, text) in logs.iter() {
            assert!(step.starts_with("search_request_"), "{step}");
            let payload: serde_json::Value = serde_json::from_str(text).expect("请求体是 JSON");
            let kinds: Vec<&str> = payload["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .filter_map(|tool| tool["type"].as_str())
                .collect();
            assert!(kinds.contains(&"x_search"), "{kinds:?}");
            assert!(kinds.contains(&"web_search"), "{kinds:?}");
            assert!(!text.contains("api_key"), "请求体里不该出现密钥");
            // 检索阶段只用文本：母版截图绝不进入检索请求。
            let lowered = text.to_lowercase();
            for marker in ["image_url", "input_image", "base64", "data:image"] {
                assert!(!lowered.contains(marker), "{marker} 不该出现在检索请求里");
            }
            let messages = payload["messages"].as_array().expect("messages");
            assert!(
                messages
                    .iter()
                    .all(|message| message["content"].is_string()),
                "检索消息只能是文本"
            );
        }
    }

    #[tokio::test]
    async fn raw_subject_reaches_the_search_payload_unchanged() {
        for raw in [
            "grokbot",
            "grok bot",
            "gRoKbOt",
            "gRoK   bot",
            "myGrokbot_v2",
            "cuda_model",
            "sdxl-based",
        ] {
            let logs = std::sync::Mutex::new(Vec::new());
            let sink = |log: DesignLog<'_>| {
                if let DesignLog::End { text, ok, .. } = log {
                    logs.lock().unwrap().push((text.to_string(), ok));
                }
            };
            // 缺凭据就能走到真实请求构造：调用失败，但报文必须先留下。
            let profile = ModelProfile {
                base_url: "http://127.0.0.1:1".to_string(),
                ..search_profile("openai_chat")
            };
            build_visual_asset_context(&[raw.to_string()], &profile, None, &sink, None)
                .await
                .expect_err("请求失败必须让任务失败");
            let logs = logs.lock().unwrap();
            assert_eq!(logs.len(), 1);
            assert!(!logs[0].1, "failed calls must not be marked succeeded");
            let payload: serde_json::Value = serde_json::from_str(&logs[0].0).unwrap();
            assert_eq!(
                payload["messages"][1]["content"],
                format!("Search query: {raw}")
            );
        }
    }
}
