use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::core::net::{
    data_url, model_error, model_http_error, normalize_base_url, post_json_with_retries,
    post_sse_with_retries, require_api_key, SseObserver, SseSignal,
};
use crate::error::AppResult;

pub fn stream_enabled(profile: &ModelProfile) -> bool {
    stream_enabled_for(&profile.protocol, profile)
}

/// Explicit `output_defaults.stream` wins; otherwise anthropic_messages defaults
/// to streaming because many gateways time out non-streaming requests that take
/// longer than ~60s to generate.
pub fn stream_enabled_for(protocol: &str, profile: &ModelProfile) -> bool {
    match profile.output_defaults.get("stream") {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        ),
        _ => protocol == "anthropic_messages",
    }
}

fn design_max_tokens(profile: &ModelProfile) -> u64 {
    profile
        .output_defaults
        .get("max_tokens")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .unwrap_or(16384)
        .clamp(256, 32768)
}

const EMPTY_TEXT_RETRY_ATTEMPTS: usize = 3;

/// Gateways sometimes wrap an upstream failure in an HTTP 200 `{"error": ...}` body.
fn upstream_error_message(data: &Value) -> Option<String> {
    let error = data.get("error")?;
    if error.is_null() {
        return None;
    }
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string(error).unwrap_or_default());
    Some(message)
}

fn response_excerpt(data: &Value) -> String {
    serde_json::to_string(data)
        .unwrap_or_default()
        .chars()
        .take(400)
        .collect()
}

/// The text a single stream event contributes, or `None` when the event carries
/// no visible output (role announcements, tool frames, thinking deltas).
///
/// Shared by the collector below and by the live sink, so what the user watches
/// arrive is assembled by the same rule as what the pipeline ends up parsing.
fn delta_text<'a>(protocol: &str, event: &'a Value) -> Option<&'a str> {
    match protocol {
        "openai_chat" => event["choices"][0]["delta"]["content"].as_str(),
        "openai_responses" => match event["type"].as_str() {
            Some("response.output_text.delta") => event["delta"].as_str(),
            _ => None,
        },
        "anthropic_messages" => {
            if event["type"].as_str() == Some("content_block_delta")
                && event["delta"]["type"].as_str() == Some("text_delta")
            {
                event["delta"]["text"].as_str()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn collect_stream_text(protocol: &str, events: &[Value]) -> AppResult<String> {
    if let Some(message) = events.iter().find_map(stream_error_message) {
        return Err(model_error(format!("模型流式响应报错：{message}")));
    }
    let mut text = String::new();
    for event in events {
        match protocol {
            "openai_chat" | "anthropic_messages" => {
                if let Some(delta) = delta_text(protocol, event) {
                    text.push_str(delta);
                }
            }
            "openai_responses" => {
                if let Some(delta) = delta_text(protocol, event) {
                    text.push_str(delta);
                } else if event["type"].as_str() == Some("response.completed") && text.is_empty() {
                    if let Some(full) = event["response"]["output_text"].as_str() {
                        text.push_str(full);
                    }
                }
            }
            other => return Err(model_error(format!("Unsupported design protocol: {other}"))),
        }
    }
    if text.trim().is_empty() {
        return Err(model_error(
            "模型流式响应没有返回文本。若上游不支持 stream，请在 Model 配置里关掉流式。",
        ));
    }
    Ok(text)
}

fn stream_error_message(event: &Value) -> Option<String> {
    let candidate = match event.get("error") {
        Some(value) if !value.is_null() => value,
        _ => match event["type"].as_str() {
            Some("error") => event,
            Some("response.failed") => &event["response"]["error"],
            _ => return None,
        },
    };
    if candidate.is_null() {
        return None;
    }
    candidate["message"]
        .as_str()
        .map(str::to_string)
        .or_else(|| Some(candidate.to_string()))
}

#[derive(Clone, Debug)]
pub struct ImageInput {
    pub filename: String,
    pub mime_type: String,
    pub b64: String,
}

/// Text from a design call as it arrives, for a caller that wants to show the
/// answer being written rather than wait for the whole thing.
pub enum DesignDelta<'a> {
    /// Discard everything handed over so far: a retry restarted the call and
    /// the previous attempt contributed nothing to the final answer.
    Restart,
    Text(&'a str),
}

pub type DeltaSink<'a> = &'a (dyn Fn(DesignDelta<'_>) + Send + Sync);

pub struct DesignClient;

impl DesignClient {
    /// Runs one design-model call, forwarding the partial answer to `sink`.
    ///
    /// The sink only fires when the profile has streaming on: a non-streaming
    /// profile has nothing to report until the response is whole, so the caller
    /// shows it in one piece instead.
    pub async fn generate(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        proxy_url: Option<&str>,
        sink: Option<DeltaSink<'_>>,
    ) -> AppResult<String> {
        let protocol = if profile.protocol == "banna2" {
            "banana2"
        } else {
            profile.protocol.as_str()
        };
        match protocol {
            "openai_chat" => {
                Self::openai_chat(profile, system_prompt, user_prompt, images, proxy_url, sink)
                    .await
            }
            "openai_responses" => {
                Self::openai_responses(profile, system_prompt, user_prompt, images, proxy_url, sink)
                    .await
            }
            "anthropic_messages" => {
                Self::anthropic_messages(
                    profile,
                    system_prompt,
                    user_prompt,
                    images,
                    proxy_url,
                    sink,
                )
                .await
            }
            other => Err(model_error(format!("Unsupported design protocol: {other}"))),
        }
    }

    async fn openai_chat(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        proxy_url: Option<&str>,
        sink: Option<DeltaSink<'_>>,
    ) -> AppResult<String> {
        let url = format!(
            "{}/chat/completions",
            normalize_base_url(&profile.base_url, &profile.protocol)
        );
        let mut content = vec![json!({"type": "text", "text": user_prompt})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "image_url",
                "image_url": {"url": data_url(&image.mime_type, &image.b64)}
            })
        }));
        let mut payload = json!({
            "model": profile.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": content}
            ],
            "temperature": 0.2
        });
        if stream_enabled(profile) {
            payload["stream"] = Value::Bool(true);
            let events = Self::stream(
                profile,
                "openai_chat",
                &url,
                &payload,
                Self::bearer(profile)?,
                proxy_url,
                sink,
            )
            .await?;
            return collect_stream_text("openai_chat", &events);
        }
        let data = Self::post(profile, &url, &payload, proxy_url).await?;
        if let Some(error) = upstream_error_message(&data) {
            return Err(model_error(format!(
                "模型在 HTTP 200 中返回错误对象: {error}"
            )));
        }
        data["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| {
                model_error(format!(
                    "OpenAI Chat 响应缺少 message.content，响应摘录: {}",
                    response_excerpt(&data)
                ))
            })
    }

    async fn openai_responses(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        proxy_url: Option<&str>,
        sink: Option<DeltaSink<'_>>,
    ) -> AppResult<String> {
        let url = format!(
            "{}/responses",
            normalize_base_url(&profile.base_url, &profile.protocol)
        );
        let mut content = vec![json!({"type": "input_text", "text": user_prompt})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "input_image",
                "image_url": data_url(&image.mime_type, &image.b64)
            })
        }));
        let mut payload = json!({
            "model": profile.model,
            "instructions": system_prompt,
            "input": [{"role": "user", "content": content}],
            "text": {"format": {"type": "text"}}
        });
        if stream_enabled(profile) {
            payload["stream"] = Value::Bool(true);
            let events = Self::stream(
                profile,
                "openai_responses",
                &url,
                &payload,
                Self::bearer(profile)?,
                proxy_url,
                sink,
            )
            .await?;
            return collect_stream_text("openai_responses", &events);
        }
        let data = Self::post(profile, &url, &payload, proxy_url).await?;
        if let Some(error) = upstream_error_message(&data) {
            return Err(model_error(format!(
                "模型在 HTTP 200 中返回错误对象: {error}"
            )));
        }
        if let Some(text) = data["output_text"].as_str() {
            if !text.is_empty() {
                return Ok(text.to_string());
            }
        }
        if let Some(items) = data["output"].as_array() {
            for item in items {
                if let Some(parts) = item["content"].as_array() {
                    for part in parts {
                        if part["type"].as_str() == Some("output_text") {
                            if let Some(text) = part["text"].as_str() {
                                return Ok(text.to_string());
                            }
                        }
                    }
                }
            }
        }
        Err(model_error(format!(
            "OpenAI Responses result did not contain output text，响应摘录: {}",
            response_excerpt(&data)
        )))
    }

    async fn anthropic_messages(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        proxy_url: Option<&str>,
        sink: Option<DeltaSink<'_>>,
    ) -> AppResult<String> {
        let url = format!(
            "{}/messages",
            normalize_base_url(&profile.base_url, &profile.protocol)
        );
        let mut content = vec![json!({"type": "text", "text": user_prompt})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": image.mime_type,
                    "data": image.b64
                }
            })
        }));
        let mut payload = json!({
            "model": profile.model,
            "max_tokens": design_max_tokens(profile),
            "system": system_prompt,
            "messages": [{"role": "user", "content": content}]
        });
        let api_key = require_api_key(profile)?;
        let version = profile
            .api_version
            .clone()
            .unwrap_or_else(|| "2023-06-01".to_string());
        let headers = vec![
            ("x-api-key".to_string(), api_key),
            ("anthropic-version".to_string(), version),
        ];
        if stream_enabled(profile) {
            payload["stream"] = Value::Bool(true);
            let events = Self::stream(
                profile,
                "anthropic_messages",
                &url,
                &payload,
                headers,
                proxy_url,
                sink,
            )
            .await?;
            return collect_stream_text("anthropic_messages", &events);
        }
        let mut last_empty_detail = String::new();
        for _ in 0..EMPTY_TEXT_RETRY_ATTEMPTS {
            let outcome =
                post_json_with_retries(profile, &url, &payload, headers.clone(), proxy_url).await?;
            if outcome.status >= 400 {
                return Err(model_http_error(
                    profile,
                    &url,
                    "Design",
                    outcome.status,
                    &outcome.body,
                ));
            }
            let data: Value = serde_json::from_str(&outcome.body)
                .map_err(|error| model_error(format!("Anthropic 响应不是 JSON: {error}")))?;
            if let Some(error) = upstream_error_message(&data) {
                return Err(model_error(format!(
                    "模型在 HTTP 200 中返回错误对象: {error}"
                )));
            }
            let text = data["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|part| part["type"].as_str() == Some("text"))
                        .filter_map(|part| part["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if !text.trim().is_empty() {
                return Ok(text);
            }
            let stop = data["stop_reason"].as_str().unwrap_or("unknown");
            last_empty_detail = format!(
                "stop_reason: {stop}，原始响应摘录: {}",
                outcome.body.chars().take(600).collect::<String>()
            );
        }
        Err(model_error(format!(
            "Anthropic 响应缺少文本内容(已重试 {EMPTY_TEXT_RETRY_ATTEMPTS} 次)，{last_empty_detail}"
        )))
    }

    async fn post(
        profile: &ModelProfile,
        url: &str,
        payload: &Value,
        proxy_url: Option<&str>,
    ) -> AppResult<Value> {
        let outcome =
            post_json_with_retries(profile, url, payload, Self::bearer(profile)?, proxy_url)
                .await?;
        if outcome.status >= 400 {
            return Err(model_http_error(
                profile,
                url,
                "Design",
                outcome.status,
                &outcome.body,
            ));
        }
        serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("模型响应不是 JSON: {error}")))
    }

    fn bearer(profile: &ModelProfile) -> AppResult<Vec<(String, String)>> {
        Ok(vec![(
            "Authorization".to_string(),
            format!("Bearer {}", require_api_key(profile)?),
        )])
    }

    #[allow(clippy::too_many_arguments)]
    async fn stream(
        profile: &ModelProfile,
        protocol: &str,
        url: &str,
        payload: &Value,
        headers: Vec<(String, String)>,
        proxy_url: Option<&str>,
        sink: Option<DeltaSink<'_>>,
    ) -> AppResult<Vec<Value>> {
        let forward = sink.map(|sink| {
            move |signal: SseSignal<'_>| match signal {
                SseSignal::Restart => sink(DesignDelta::Restart),
                SseSignal::Event(event) => {
                    if let Some(text) = delta_text(protocol, event) {
                        sink(DesignDelta::Text(text));
                    }
                }
            }
        });
        let observer: Option<SseObserver<'_>> = forward
            .as_ref()
            .map(|forward| forward as &(dyn Fn(SseSignal<'_>) + Send + Sync));
        let outcome =
            post_sse_with_retries(profile, url, payload, headers, proxy_url, observer).await?;
        if outcome.status >= 400 {
            return Err(model_http_error(
                profile,
                url,
                "Design",
                outcome.status,
                &outcome.body,
            ));
        }
        Ok(outcome.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_error_message_extracts_gateway_error_bodies() {
        let payload: Value = serde_json::from_str(
            r#"{"error":{"message":"max_tokens must be less than or equal to 2147483647","type":"invalid_request_error"}}"#,
        )
        .unwrap();
        assert_eq!(
            upstream_error_message(&payload).as_deref(),
            Some("max_tokens must be less than or equal to 2147483647")
        );

        let opaque: Value = serde_json::from_str(r#"{"error":"quota exhausted"}"#).unwrap();
        assert_eq!(
            upstream_error_message(&opaque).as_deref(),
            Some("\"quota exhausted\"")
        );

        let normal: Value = serde_json::from_str(r#"{"choices":[]}"#).unwrap();
        assert_eq!(upstream_error_message(&normal), None);

        let null_error: Value = serde_json::from_str(r#"{"error":null,"content":[]}"#).unwrap();
        assert_eq!(upstream_error_message(&null_error), None);
    }

    fn events(raw: &[&str]) -> Vec<Value> {
        raw.iter()
            .map(|line| serde_json::from_str(line).expect("测试数据是合法 JSON"))
            .collect()
    }

    fn profile_with(stream: Option<Value>) -> ModelProfile {
        let mut output_defaults = serde_json::Map::new();
        if let Some(value) = stream {
            output_defaults.insert("stream".to_string(), value);
        }
        ModelProfile {
            id: "design".to_string(),
            role: "design".to_string(),
            name: "Design model".to_string(),
            protocol: "openai_responses".to_string(),
            base_url: "https://example.com".to_string(),
            model: "m".to_string(),
            api_key: Some("k".to_string()),
            api_version: None,
            headers: serde_json::Map::new(),
            timeout_seconds: 120,
            max_retries: 0,
            output_defaults,
            has_api_key: Some(true),
            api_key_hint: None,
            proxy_url: None,
        }
    }

    #[test]
    fn stream_flag_accepts_both_literals_and_defaults_off() {
        assert!(!stream_enabled(&profile_with(None)));
        assert!(stream_enabled(&profile_with(Some(Value::Bool(true)))));
        assert!(stream_enabled(&profile_with(Some(Value::String(
            "true".into()
        )))));
        assert!(stream_enabled(&profile_with(Some(Value::String(
            " ON ".into()
        )))));
        assert!(!stream_enabled(&profile_with(Some(Value::String(
            "false".into()
        )))));
    }

    #[test]
    fn design_max_tokens_defaults_high_and_accepts_an_override() {
        let mut profile = profile_with(None);
        assert_eq!(design_max_tokens(&profile), 16384);
        profile
            .output_defaults
            .insert("max_tokens".to_string(), Value::String("12000".to_string()));
        assert_eq!(design_max_tokens(&profile), 12000);
    }

    #[test]
    fn anthropic_defaults_to_streaming_unless_overridden() {
        let mut profile = profile_with(None);
        assert!(!stream_enabled_for("openai_chat", &profile));
        assert!(stream_enabled_for("anthropic_messages", &profile));
        profile
            .output_defaults
            .insert("stream".to_string(), Value::Bool(false));
        assert!(!stream_enabled_for("anthropic_messages", &profile));
        profile
            .output_defaults
            .insert("stream".to_string(), Value::String("true".into()));
        assert!(stream_enabled_for("openai_chat", &profile));
    }

    #[test]
    fn collects_text_from_each_protocol() {
        let chat = events(&[
            r#"{"choices":[{"delta":{"role":"assistant"}}]}"#,
            r#"{"choices":[{"delta":{"content":"{\"layout\""}}]}"#,
            r#"{"choices":[{"delta":{"content":":1}"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(
            collect_stream_text("openai_chat", &chat).unwrap(),
            "{\"layout\":1}"
        );

        let responses = events(&[
            r#"{"type":"response.created"}"#,
            r#"{"type":"response.output_text.delta","delta":"前半"}"#,
            r#"{"type":"response.output_text.delta","delta":"后半"}"#,
            r#"{"type":"response.completed"}"#,
        ]);
        assert_eq!(
            collect_stream_text("openai_responses", &responses).unwrap(),
            "前半后半"
        );

        let anthropic = events(&[
            r#"{"type":"message_start"}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"图"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"忽略"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"例"}}"#,
        ]);
        assert_eq!(
            collect_stream_text("anthropic_messages", &anthropic).unwrap(),
            "图例"
        );
    }

    #[test]
    fn the_live_deltas_add_up_to_the_collected_answer() {
        // What the user watches arrive and what the pipeline goes on to parse
        // must be the same string, or a card would contradict the figure built
        // from it.
        let cases: [(&str, &[&str]); 3] = [
            (
                "openai_chat",
                &[
                    r#"{"choices":[{"delta":{"role":"assistant"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"{\"layout\""}}]}"#,
                    r#"{"choices":[{"delta":{"content":":1}"}}]}"#,
                ],
            ),
            (
                "openai_responses",
                &[
                    r#"{"type":"response.created"}"#,
                    r#"{"type":"response.output_text.delta","delta":"前半"}"#,
                    r#"{"type":"response.output_text.delta","delta":"后半"}"#,
                ],
            ),
            (
                "anthropic_messages",
                &[
                    r#"{"type":"message_start"}"#,
                    r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"图"}}"#,
                    r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"忽略"}}"#,
                    r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"例"}}"#,
                ],
            ),
        ];
        for (protocol, raw) in cases {
            let parsed = events(raw);
            let streamed: String = parsed
                .iter()
                .filter_map(|event| delta_text(protocol, event))
                .collect();
            assert_eq!(
                streamed,
                collect_stream_text(protocol, &parsed).unwrap(),
                "协议 {protocol} 的流式增量与最终文本不一致"
            );
        }
    }

    #[test]
    fn a_terminal_only_response_streams_nothing_to_show() {
        // openai_responses may skip deltas entirely and hand the answer over in
        // response.completed. Nothing streams, so the card stays empty until the
        // final text replaces it — which is why End carries the whole answer
        // rather than trusting the deltas.
        let only_completed = events(&[
            r#"{"type":"response.created"}"#,
            r#"{"type":"response.completed","response":{"output_text":"全文"}}"#,
        ]);
        assert!(only_completed
            .iter()
            .all(|event| delta_text("openai_responses", event).is_none()));
        assert_eq!(
            collect_stream_text("openai_responses", &only_completed).unwrap(),
            "全文"
        );
    }

    #[test]
    fn falls_back_to_the_completed_event_when_no_deltas_arrive() {
        let only_completed = events(&[
            r#"{"type":"response.created"}"#,
            r#"{"type":"response.completed","response":{"output_text":"全文"}}"#,
        ]);
        assert_eq!(
            collect_stream_text("openai_responses", &only_completed).unwrap(),
            "全文"
        );
    }

    #[test]
    fn surfaces_an_error_event_instead_of_an_empty_result() {
        let failed = events(&[
            r#"{"choices":[{"delta":{"content":"半截"}}]}"#,
            r#"{"error":{"message":"insufficient quota"}}"#,
        ]);
        let error = collect_stream_text("openai_chat", &failed).expect_err("错误事件应当报错");
        assert!(
            error.message.contains("insufficient quota"),
            "实际：{}",
            error.message
        );

        let refused = events(&[
            r#"{"type":"response.failed","response":{"error":{"message":"model not found"}}}"#,
        ]);
        let error =
            collect_stream_text("openai_responses", &refused).expect_err("失败事件应当报错");
        assert!(
            error.message.contains("model not found"),
            "实际：{}",
            error.message
        );
    }

    #[test]
    fn empty_stream_points_at_the_toggle() {
        let error = collect_stream_text("openai_chat", &[]).expect_err("空流应当报错");
        assert!(error.message.contains("stream"), "实际：{}", error.message);
    }
}
