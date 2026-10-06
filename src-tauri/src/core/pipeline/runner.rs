use std::future::Future;

use serde_json::Value;

use crate::core::config::ModelProfile;
use crate::core::model::design::{DeltaSink, DesignClient, DesignDelta, ImageInput};
use crate::core::net::parse_json_response;
use crate::core::pipeline::validate::ValidationError;
use crate::error::{AppError, AppResult};
use crate::event::{DesignLog, DesignSink};

const JSON_CONTEXT_RETRY_ATTEMPTS: usize = 1;
const JSON_RETRY_EXCERPT_LIMIT: usize = 4000;
const VALIDATION_REPAIR_ROUNDS: usize = 2;

fn excerpt(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub trait DesignGenerator {
    fn user_prompt(&self) -> &str;
    fn generate(&self, prompt: String) -> impl Future<Output = AppResult<String>> + Send;
}

/// Which step card a design call reports into.
///
/// Retries all belong to the card the step opened: a dropped connection and a
/// JSON repair round are further attempts at one step, not new steps, so each
/// clears the card instead of stacking another one beside it.
#[derive(Clone, Copy)]
pub struct DesignStep<'a> {
    pub sink: DesignSink<'a>,
    pub step: &'a str,
    pub label: &'a str,
}

use crate::core::pipeline::cache::StepCache;

pub struct DesignCall<'a> {
    pub profile: &'a ModelProfile,
    pub system_prompt: &'a str,
    pub user_prompt: &'a str,
    pub images: &'a [ImageInput],
    pub proxy_url: Option<&'a str>,
    pub response_sink: Option<&'a (dyn Fn(&str) + Send + Sync)>,
    pub log: Option<DesignStep<'a>>,
    /// 有缓存时，同一次调用的答案只买一次：重跑任务时命中就回放。
    pub cache: Option<StepCache<'a>>,
}

impl DesignCall<'_> {
    /// The step's first attempt, on the prompt the call was built with.
    pub async fn first(&self) -> AppResult<String> {
        if let Some(step) = self.log {
            (step.sink)(DesignLog::Begin {
                step: step.step,
                label: step.label,
            });
        }
        self.attempt(self.user_prompt).await
    }

    async fn attempt(&self, prompt: &str) -> AppResult<String> {
        // 命中缓存时不发请求，但照样发 End：进度、日志与卡片内容跟第一次一模一样，
        // 只有网络调用被省掉。
        let cached = self.cache.map(|step| {
            let key = step.design_key(self.profile, self.system_prompt, prompt, self.images);
            (step, key)
        });
        if let Some((step, key)) = &cached {
            if let Some(text) = step.text(key) {
                self.log_end(&text, true);
                return Ok(text);
            }
        }

        let relay = self.log.map(|step| {
            move |delta: DesignDelta<'_>| match delta {
                DesignDelta::Restart => (step.sink)(DesignLog::Reset { step: step.step }),
                DesignDelta::Text(text) => (step.sink)(DesignLog::Delta {
                    step: step.step,
                    text,
                }),
            }
        });
        let sink: Option<DeltaSink<'_>> = relay
            .as_ref()
            .map(|relay| relay as &(dyn Fn(DesignDelta<'_>) + Send + Sync));
        let result = DesignClient::generate(
            self.profile,
            self.system_prompt,
            prompt,
            self.images,
            self.proxy_url,
            sink,
        )
        .await;
        // 只有成功的答案才值得回放，失败的调用下次重跑必须真的重试。
        if let (Some((step, key)), Ok(text)) = (&cached, &result) {
            step.put_text(key, text);
        }
        let (text, ok) = match &result {
            Ok(text) => (text.as_str(), true),
            Err(error) => (error.message.as_str(), false),
        };
        self.log_end(text, ok);
        result
    }

    fn log_end(&self, text: &str, ok: bool) {
        if let Some(step) = self.log {
            (step.sink)(DesignLog::End {
                step: step.step,
                label: step.label,
                text,
                ok,
            });
        }
    }
}

impl DesignGenerator for DesignCall<'_> {
    fn user_prompt(&self) -> &str {
        self.user_prompt
    }

    async fn generate(&self, prompt: String) -> AppResult<String> {
        if let Some(step) = self.log {
            (step.sink)(DesignLog::Reset { step: step.step });
        }
        let text = self.attempt(&prompt).await?;
        if let Some(sink) = self.response_sink {
            sink(&text);
        }
        Ok(text)
    }
}

pub async fn parse_or_repair<G: DesignGenerator>(call: &G, text: &str) -> AppResult<Value> {
    let first_error = match parse_json_response(text) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };

    let mut last_error = first_error.message;
    let mut retry_text = String::new();

    for _ in 0..JSON_CONTEXT_RETRY_ATTEMPTS {
        let retry_prompt = format!(
            "{}\n\n\
             The previous response for this exact task was not parseable JSON. \
             Regenerate the answer using the same task context and return strict JSON only. \
             Do not wrap in Markdown. Do not explain. Do not omit required fields. \
             Fix JSON syntax issues such as missing commas, dangling quotes, trailing prose, and unescaped newlines.\n\n\
             Parse error: {}\n\
             Previous invalid output excerpt:\n{}",
            call.user_prompt(),
            last_error,
            excerpt(text, JSON_RETRY_EXCERPT_LIMIT)
        );
        retry_text = call.generate(retry_prompt).await?;
        match parse_json_response(&retry_text) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error.message,
        }
    }

    let source = if retry_text.is_empty() {
        text
    } else {
        &retry_text
    };
    let repair_prompt = format!(
        "The previous model output was not valid JSON. Convert it into strict JSON only, preserving all useful content.\n\
         Return JSON only. Do not wrap in Markdown. Do not explain.\n\n\
         Original task:\n{}\n\nInvalid output:\n{}",
        call.user_prompt(),
        excerpt(source, JSON_RETRY_EXCERPT_LIMIT)
    );
    let repaired = call.generate(repair_prompt).await?;
    parse_json_response(&repaired)
}

#[derive(Debug)]
pub struct Validated<T> {
    pub parsed: Value,
    pub value: T,
    pub retried: bool,
}

pub async fn parse_validate_or_fill<G, T, F>(
    call: &G,
    text: &str,
    validator: F,
) -> AppResult<Validated<T>>
where
    G: DesignGenerator,
    F: Fn(&Value) -> Result<T, ValidationError>,
{
    let mut parsed = parse_or_repair(call, text).await?;
    let mut retried = false;
    for _ in 0..VALIDATION_REPAIR_ROUNDS {
        let error = match validator(&parsed) {
            Ok(value) => {
                return Ok(Validated {
                    parsed,
                    value,
                    retried,
                })
            }
            Err(error) => error,
        };
        let instruction = if error.is_schema() {
            "The previous model output was valid JSON but failed the required structured output contract.\n\
             Return strict JSON only. Preserve all valid content and do not redesign the figure, slide master, or page plan.\n\
             Only fill, normalize, or add the missing required fields and constraints named by the validation error."
        } else {
            "The previous model output was valid JSON but failed semantic validation.\n\
             Return strict JSON only. Do not redesign the figure, slide master, or page plan, and keep every field that already satisfies the contract unchanged.\n\
             Fix only the entries named by the validation error. When shorter strings are required, replace them with concise labels that keep the original language and meaning."
        };
        let repair_prompt = format!(
            "{instruction}\n\n\
             Validation error:\n{}\n\n\
             Original task:\n{}\n\n\
             Current JSON:\n{}",
            error.message(),
            call.user_prompt(),
            serde_json::to_string_pretty(&parsed).unwrap_or_default()
        );
        retried = true;
        let repaired_text = call.generate(repair_prompt).await?;
        parsed = parse_json_response(&repaired_text)?;
    }
    let value = validator(&parsed).map_err(AppError::from)?;
    Ok(Validated {
        parsed,
        value,
        retried,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pipeline::validate::validate_paper_design;
    use std::sync::Mutex;

    struct StubGenerator {
        user_prompt: String,
        responses: Mutex<Vec<String>>,
        prompts: Mutex<Vec<String>>,
    }

    impl StubGenerator {
        fn new(user_prompt: &str, responses: &[&str]) -> Self {
            Self {
                user_prompt: user_prompt.to_string(),
                responses: Mutex::new(
                    responses
                        .iter()
                        .rev()
                        .map(|item| item.to_string())
                        .collect(),
                ),
                prompts: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.prompts.lock().unwrap().len()
        }

        fn prompt(&self, index: usize) -> String {
            self.prompts.lock().unwrap()[index].clone()
        }
    }

    impl DesignGenerator for StubGenerator {
        fn user_prompt(&self) -> &str {
            &self.user_prompt
        }

        async fn generate(&self, prompt: String) -> AppResult<String> {
            self.prompts.lock().unwrap().push(prompt);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| AppError::new("stub_exhausted", "no stub response left"))
        }
    }

    fn valid_diagram_design() -> Value {
        crate::core::pipeline::validate::tests::valid_diagram_design()
    }

    #[tokio::test]
    async fn parse_failure_retries_same_context_before_repair() {
        let stub = StubGenerator::new(
            "original task with complete context",
            [r#"{"ok": true}"#].as_slice(),
        );
        let parsed = parse_or_repair(&stub, r#"{"ok": true "broken": false}"#)
            .await
            .unwrap();

        assert_eq!(parsed, serde_json::json!({"ok": true}));
        assert_eq!(stub.calls(), 1);
        let retry_prompt = stub.prompt(0);
        assert!(retry_prompt.contains("original task with complete context"));
        assert!(retry_prompt.contains("Regenerate the answer using the same task context"));
        assert!(retry_prompt.contains("Previous invalid output excerpt"));
    }

    #[tokio::test]
    async fn valid_json_does_not_call_the_model() {
        let stub = StubGenerator::new("task", &[]);
        let parsed = parse_or_repair(&stub, r#"{"ok": true}"#).await.unwrap();
        assert_eq!(parsed, serde_json::json!({"ok": true}));
        assert_eq!(stub.calls(), 0);
    }

    #[tokio::test]
    async fn structured_fill_retry_only_fills_schema_gaps() {
        let filled = valid_diagram_design();
        let stub = StubGenerator::new("original task", &[&filled.to_string()]);

        let mut initial = valid_diagram_design();
        initial["figure"]
            .as_object_mut()
            .unwrap()
            .remove("diagram_spec");

        let result = parse_validate_or_fill(&stub, &initial.to_string(), |value| {
            validate_paper_design(value)
        })
        .await
        .unwrap();

        assert_eq!(result.parsed, filled);
        assert!(result.retried);
        assert_eq!(stub.calls(), 1);
        let fill_prompt = stub.prompt(0);
        assert!(fill_prompt.contains("Only fill"));
        assert!(fill_prompt.contains("do not redesign"));
        assert!(fill_prompt.contains("Diagram figure missing diagram_spec"));
    }

    #[tokio::test]
    async fn semantic_errors_retry_then_fail_after_exhaustion() {
        let stub = StubGenerator::new(
            "original task",
            &[r#"{"still": "bad"}"#, r#"{"still": "bad"}"#],
        );
        let error = parse_validate_or_fill(&stub, r#"{"ok": true}"#, |_| {
            Err::<(), _>(ValidationError::Value("page 3 out of order".to_string()))
        })
        .await
        .unwrap_err();

        assert_eq!(stub.calls(), 2);
        assert_eq!(error.message, "page 3 out of order");
        for round in 0..stub.calls() {
            assert!(stub.prompt(round).contains("failed semantic validation"));
            assert!(stub.prompt(round).contains("page 3 out of order"));
        }
    }

    #[tokio::test]
    async fn semantic_error_repaired_without_redesign() {
        let mut broken = valid_diagram_design();
        let long_label = "x".repeat(81);
        broken["figure"]["visible_text"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::Value::String(long_label));
        let fixed = valid_diagram_design();

        let stub = StubGenerator::new("original task", &[&fixed.to_string()]);

        let result = parse_validate_or_fill(&stub, &broken.to_string(), |value| {
            validate_paper_design(value)
        })
        .await
        .unwrap();

        assert_eq!(result.parsed, fixed);
        assert!(result.retried);
        assert_eq!(stub.calls(), 1);
        let repair_prompt = stub.prompt(0);
        assert!(repair_prompt.contains("failed semantic validation"));
        assert!(repair_prompt.contains("visible_text must be short label strings"));
        assert!(repair_prompt.contains("concise labels"));
    }

    /// 本地假上游：记录每次收到的请求体，用来断言「第二次没有真的发请求」。
    async fn counting_upstream(
        status: u16,
        body: &'static str,
    ) -> (String, std::sync::Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = std::sync::Arc::new(Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let recorded = std::sync::Arc::clone(&recorded);
                tokio::spawn(async move {
                    let mut input = Vec::new();
                    let mut buffer = [0; 4096];
                    loop {
                        let Ok(count) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if count == 0 {
                            break;
                        }
                        input.extend_from_slice(&buffer[..count]);
                        if let Some(end) = input.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&input[..end]).to_string();
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            if input.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    recorded
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&input).to_string());
                    let response = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (base, requests)
    }

    fn local_design_profile(base: String) -> crate::core::config::ModelProfile {
        serde_json::from_value(serde_json::json!({
            "id": "design", "role": "design", "name": "Design", "protocol": "openai_chat",
            "base_url": base, "model": "Build/local", "api_key": "local-test-only",
            "headers": {}, "max_retries": 0, "output_defaults": {}
        }))
        .unwrap()
    }

    fn cache_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-runner-cache-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// 重跑同一个任务时，已经买过的设计答案必须从缓存回放：不再发第二次请求。
    #[tokio::test]
    async fn a_successful_design_answer_is_replayed_without_a_second_request() {
        use crate::core::pipeline::cache::{AnswerCache, StepCache};

        let upstream = serde_json::json!({
            "choices": [{ "message": { "content": "第一份答案" } }]
        })
        .to_string();
        let (base, requests) = counting_upstream(200, Box::leak(upstream.into_boxed_str())).await;
        let profile = local_design_profile(base);
        let cache = AnswerCache::new(&cache_dir("replay"), "job");
        let call = DesignCall {
            profile: &profile,
            system_prompt: "system",
            user_prompt: "prompt",
            images: &[],
            proxy_url: None,
            response_sink: None,
            log: None,
            cache: Some(StepCache {
                cache: &cache,
                scope: "paper_design",
            }),
        };

        assert_eq!(call.first().await.expect("第一次真实调用"), "第一份答案");
        assert_eq!(requests.lock().unwrap().len(), 1);
        // 重跑：同样的调用直接回放，请求数不变。
        assert_eq!(call.first().await.expect("第二次回放"), "第一份答案");
        assert_eq!(requests.lock().unwrap().len(), 1, "命中缓存后不该再发请求");
        // 换一份提示词就是另一次调用。
        let other = DesignCall {
            user_prompt: "换了的 prompt",
            ..call
        };
        assert_eq!(
            other.first().await.expect("换了输入要重新调用"),
            "第一份答案"
        );
        assert_eq!(requests.lock().unwrap().len(), 2);
    }

    /// 失败的调用绝不能进缓存：重跑必须真的重试，而不是回放一个错误。
    #[tokio::test]
    async fn a_failed_design_call_is_never_cached() {
        use crate::core::pipeline::cache::{AnswerCache, StepCache};

        let (base, requests) = counting_upstream(500, "{\"error\":\"boom\"}").await;
        let profile = local_design_profile(base);
        let cache = AnswerCache::new(&cache_dir("failed"), "job");
        let call = DesignCall {
            profile: &profile,
            system_prompt: "system",
            user_prompt: "prompt",
            images: &[],
            proxy_url: None,
            response_sink: None,
            log: None,
            cache: Some(StepCache {
                cache: &cache,
                scope: "paper_design",
            }),
        };

        assert!(call.first().await.is_err(), "上游 500 应当是失败");
        let after_first = requests.lock().unwrap().len();
        assert!(call.first().await.is_err());
        assert_eq!(
            requests.lock().unwrap().len(),
            after_first + 1,
            "失败不会被缓存，第二次必须真的再发一次"
        );
    }
}
