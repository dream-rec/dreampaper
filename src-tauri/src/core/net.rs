use std::time::Duration;

use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::error::{AppError, AppResult};

pub const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 30;
pub const RETRY_INTERVAL_SECONDS: u64 = 180;
const RETRY_STATUS_CODES: [u16; 5] = [429, 500, 502, 503, 504];

pub fn is_retryable(status: u16) -> bool {
    RETRY_STATUS_CODES.contains(&status)
}

pub fn data_url(mime_type: &str, b64: &str) -> String {
    format!("data:{mime_type};base64,{b64}")
}

pub fn encode_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_b64(value: &str) -> AppResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|error| AppError::new("b64_error", error.to_string()))
}

pub fn normalize_base_url(base_url: &str, protocol: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if matches!(protocol, "banana2" | "banna2") {
        return base.to_string();
    }
    let version_pattern = Regex::new(r"/v\d+(?:beta)?(?:/|$)").expect("valid regex");
    let path = base.split_once("://").map(|(_, rest)| rest).unwrap_or(base);
    let path = path
        .split_once('/')
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap_or_default();
    if version_pattern.is_match(&path) {
        return base.to_string();
    }
    format!("{base}/v1")
}

pub fn require_api_key(profile: &ModelProfile) -> AppResult<String> {
    match profile.api_key.as_deref().map(str::trim) {
        Some(key) if !key.is_empty() => Ok(key.to_string()),
        _ => Err(AppError::new(
            "missing_api_key",
            format!("{} 缺少 API key", profile.name),
        )),
    }
}

pub fn safe_error_message(message: &str) -> String {
    let bearer = Regex::new(r"Bearer\s+[A-Za-z0-9._~+/=-]+").expect("valid regex");
    let keyed = Regex::new(r#"(?i)(api[_-]?key|x-api-key|x-goog-api-key)(['"\s:=]+)([^'"\s,&}]+)"#)
        .expect("valid regex");
    let cleaned = bearer.replace_all(message, "Bearer <redacted>");
    let cleaned = keyed.replace_all(&cleaned, "$1$2<redacted>");
    cleaned.chars().take(800).collect()
}

pub fn model_error(message: impl AsRef<str>) -> AppError {
    AppError::new("model_error", safe_error_message(message.as_ref()))
}

pub fn model_profile_error(
    profile: &ModelProfile,
    endpoint: &str,
    code: &str,
    message: impl AsRef<str>,
    http_status: Option<u16>,
    suggestion: impl AsRef<str>,
) -> AppError {
    let summary = safe_error_message(message.as_ref());
    AppError::with_detail(
        code,
        &summary,
        json!({
            "summary": summary.clone(),
            "code": code,
            "role": &profile.role,
            "profile_id": &profile.id,
            "profile_name": &profile.name,
            "protocol": &profile.protocol,
            "model": &profile.model,
            "base_url": &profile.base_url,
            "endpoint": endpoint,
            "http_status": http_status,
            "suggestion": suggestion.as_ref(),
        }),
    )
}

pub async fn wait_before_retry() {
    tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECONDS)).await;
}

pub fn describe_transport_error(error: &reqwest::Error) -> String {
    let mut chain = vec![error.to_string()];
    let mut cursor: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(cause) = cursor {
        let text = cause.to_string();
        if !text.trim().is_empty() && !chain.iter().any(|item| item == &text) {
            chain.push(text);
        }
        cursor = cause.source();
    }
    let mut kinds = Vec::new();
    for (flag, label) in [
        (error.is_timeout(), "timeout"),
        (error.is_connect(), "connect"),
        (error.is_request(), "request"),
        (error.is_body(), "body"),
        (error.is_decode(), "decode"),
    ] {
        if flag {
            kinds.push(label);
        }
    }
    let detail = chain.join(" <- ");
    if kinds.is_empty() {
        detail
    } else {
        format!("{detail} [{}]", kinds.join(","))
    }
}

pub fn format_http_error(kind: &str, status: u16, body: &str) -> String {
    let detail = response_error_summary(body);
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(" 服务返回：{detail}")
    };
    match status {
        502 => format!("{kind} 请求失败：HTTP 502，上游网关无法完成请求。{suffix}"),
        503 | 504 => format!(
            "{kind} 请求失败：HTTP {status}，上游网关维护、超时或无法连接模型服务。{suffix}"
        ),
        _ => format!("{kind} 请求失败：HTTP {status}。{suffix}"),
    }
}

pub fn response_error_summary(body: &str) -> String {
    let raw = body.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        let candidate = value["error"]["message"]
            .as_str()
            .or_else(|| value["error"]["detail"].as_str())
            .or_else(|| value["message"].as_str())
            .or_else(|| value["detail"].as_str());
        if let Some(text) = candidate {
            return text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(240)
                .collect();
        }
    }
    let title = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid title regex");
    let tags = Regex::new(r"(?is)<[^>]+>").expect("valid tag regex");
    let source = title
        .captures(raw)
        .and_then(|capture| capture.get(1).map(|item| item.as_str()))
        .unwrap_or(raw);
    tags.replace_all(source, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

pub fn http_error_suggestion(role: &str, status: u16) -> String {
    match status {
        502 | 503 | 504 => format!(
            "{} 上游网关异常，请稍后重试；若持续出现，请检查该服务地址或更换中转服务。",
            capitalize_role(role)
        ),
        401 => format!("请检查 {} 配置的 API key。", capitalize_role(role)),
        403 => format!(
            "请检查 {} 配置的 API key、模型权限和服务商访问策略。",
            capitalize_role(role)
        ),
        404 => format!(
            "请检查 {} 的 Base URL、协议和模型名。",
            capitalize_role(role)
        ),
        429 => "请求频率或额度受限。请稍后重试，并检查账户额度。".to_string(),
        _ => format!(
            "请检查 {} 的 Base URL、协议、模型名和服务商状态。",
            capitalize_role(role)
        ),
    }
}

fn capitalize_role(role: &str) -> String {
    let mut chars = role.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn model_http_error(
    profile: &ModelProfile,
    endpoint: &str,
    kind: &str,
    status: u16,
    body: &str,
) -> AppError {
    model_profile_error(
        profile,
        endpoint,
        "model_http_error",
        format_http_error(kind, status, body),
        Some(status),
        http_error_suggestion(&profile.role, status),
    )
}

/// reqwest sends no `User-Agent` by default; ModelScope's LFS CDN answers
/// such requests with 403 (reproduced through a proxy), and model gateways
/// generally prefer an identifiable client too.
pub const USER_AGENT: &str = concat!(
    "DreamPaper/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/dream-rec/dreampaper)"
);

fn client_builder(proxy_url: Option<&str>) -> AppResult<reqwest::ClientBuilder> {
    // 代理只认设置里填的值：环境里的 HTTP_PROXY / HTTPS_PROXY / ALL_PROXY 一律不采用。
    // 否则用户 shell 里的残留代理会让请求瞬间连到不存在的本地端口，报“隧道错误”。
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .user_agent(USER_AGENT)
        .tcp_keepalive(Duration::from_secs(30))
        .pool_max_idle_per_host(0);
    if let Some(proxy) = configured_proxy(proxy_url) {
        let proxy = reqwest::Proxy::all(proxy)
            .map_err(|error| model_error(format!("代理配置无效: {error}")))?;
        builder = builder.proxy(proxy);
    }
    Ok(builder)
}

/// 只有显式配置且非空白的值才算代理；其余一律直连。
pub(crate) fn configured_proxy(proxy_url: Option<&str>) -> Option<&str> {
    proxy_url.map(str::trim).filter(|value| !value.is_empty())
}

/// DuckDuckGo 自己的代理优先于应用级代理。两者都空则直连，不读环境变量。
pub(crate) fn duckduckgo_proxy(
    profile_proxy: Option<&str>,
    settings_proxy: Option<&str>,
) -> Option<String> {
    configured_proxy(profile_proxy)
        .or_else(|| configured_proxy(settings_proxy))
        .map(str::to_string)
}

pub fn build_client(
    read_timeout_seconds: u64,
    proxy_url: Option<&str>,
) -> AppResult<reqwest::Client> {
    client_builder(proxy_url)?
        .http1_only()
        .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS))
        .timeout(Duration::from_secs(read_timeout_seconds.max(1)))
        .build()
        .map_err(|error| model_error(format!("HTTP client 构建失败: {error}")))
}

pub fn build_model_client(
    timeout_seconds: u64,
    proxy_url: Option<&str>,
) -> AppResult<reqwest::Client> {
    let timeout = Duration::from_secs(timeout_seconds);
    client_builder(proxy_url)?
        // Some gateways reject HTTP/2 fingerprints on large image requests.
        .http1_only()
        .connect_timeout(timeout)
        .timeout(timeout)
        .retry(reqwest::retry::never())
        .build()
        .map_err(|error| model_error(format!("HTTP client 构建失败: {error}")))
}

pub fn merged_headers(
    profile: &ModelProfile,
    extra: Vec<(String, String)>,
) -> Vec<(String, String)> {
    // Content-Type is intentionally not set here: request builders (.json() /
    // .multipart()) already set it, and duplicating it makes some gateways
    // answer `200 "Header Content-Type Error"` instead of the model output.
    let mut headers: Vec<(String, String)> = Vec::new();
    for (key, value) in &profile.headers {
        if let Some(text) = value.as_str() {
            headers.push((key.clone(), text.to_string()));
        }
    }
    headers.extend(extra);
    headers
}

pub struct HttpOutcome {
    pub status: u16,
    pub body: String,
}

pub async fn read_http_response(
    response: reqwest::Response,
) -> Result<HttpOutcome, reqwest::Error> {
    let status = response.status().as_u16();
    let body = match response.text().await {
        Ok(body) => body,
        Err(_) if status >= 400 && !is_retryable(status) => String::new(),
        Err(error) => return Err(error),
    };
    Ok(HttpOutcome { status, body })
}

pub async fn post_json_with_retries(
    profile: &ModelProfile,
    url: &str,
    payload: &Value,
    headers: Vec<(String, String)>,
    proxy_url: Option<&str>,
) -> AppResult<HttpOutcome> {
    let read_timeout = profile.request_timeout();
    let client = build_model_client(read_timeout, proxy_url)?;
    let all_headers = merged_headers(profile, headers);
    let attempts = (profile.max_retries.max(0) as u32) + 1;
    let mut last_transport_error: Option<String> = None;
    let mut timed_out = false;

    for attempt in 0..attempts {
        let started = std::time::Instant::now();
        let mut request = client.post(url).json(payload);
        for (key, value) in &all_headers {
            request = request.header(key.as_str(), value.as_str());
        }
        let outcome = async { read_http_response(request.send().await?).await }.await;
        match outcome {
            Ok(HttpOutcome { status, body }) => {
                if !is_retryable(status) || attempt == attempts - 1 {
                    return Ok(HttpOutcome { status, body });
                }
                wait_before_retry().await;
            }
            Err(error) => {
                let elapsed = started.elapsed().as_secs();
                timed_out = error.is_timeout();
                last_transport_error = Some(if error.is_timeout() {
                    format!(
                        "模型请求超时：{read_timeout} 秒内未收到完整响应（本次等待 {elapsed} 秒）。\
                         请在 Model 配置中检查对应模型的超时与服务状态。"
                    )
                } else {
                    format!(
                        "模型请求网络错误（发起后 {elapsed} 秒断开）：{}",
                        describe_transport_error(&error)
                    )
                });
                if attempt == attempts - 1 {
                    break;
                }
                wait_before_retry().await;
            }
        }
    }

    let message =
        last_transport_error.unwrap_or_else(|| "模型请求失败：未收到有效响应".to_string());
    let suggestion = transport_suggestion(
        &profile.role,
        &profile.protocol,
        timed_out,
        proxy_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some(),
    );
    Err(model_profile_error(
        profile,
        url,
        "model_network_error",
        message,
        None,
        suggestion,
    ))
}

/// 超时的处理建议：grok_search 的往返天生要几十秒（上游模型要真跑 x_search /
/// web_search），把它指到代理或 DNS 只会贴错方向。
fn transport_suggestion(role: &str, protocol: &str, timed_out: bool, has_proxy: bool) -> String {
    let role = capitalize_role(role);
    if timed_out {
        if protocol.trim().eq_ignore_ascii_case("grok_search") {
            return format!(
                "{role} 的 grok_search 由上游模型自行联网检索，实测一次往返约 45 秒：\
                 请把超时提到 120 秒以上（设置 → Search → 超时）。"
            );
        }
        return format!("请提高 {role} 的超时，或检查服务商状态。");
    }
    if has_proxy {
        return "请检查代理地址和代理服务，或清空代理后直连。".to_string();
    }
    format!("请检查 {role} 的 Base URL、DNS 和网络连接。")
}

pub struct SseOutcome {
    pub status: u16,
    pub body: String,
    pub events: Vec<Value>,
}

/// What a caller watching an SSE request sees as it happens.
///
/// The outcome above is only handed back once the stream closes, which is too
/// late to show a model's answer arriving. An observer gets each event as it
/// lands instead.
pub enum SseSignal<'a> {
    /// A retry is about to start. Everything forwarded so far belongs to an
    /// attempt that produced nothing usable and must be discarded.
    Restart,
    Event(&'a Value),
}

pub type SseObserver<'a> = &'a (dyn Fn(SseSignal<'_>) + Send + Sync);

pub fn drain_sse_events(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut events = Vec::new();
    while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buffer.drain(..=index).collect();
        let decoded = String::from_utf8_lossy(&line);
        let Some(rest) = decoded.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = rest.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        events.push(payload.to_string());
    }
    events
}

fn build_stream_client(
    idle_timeout_seconds: u64,
    proxy_url: Option<&str>,
) -> AppResult<reqwest::Client> {
    let timeout = Duration::from_secs(idle_timeout_seconds);
    client_builder(proxy_url)?
        .connect_timeout(timeout)
        .read_timeout(timeout)
        .retry(reqwest::retry::never())
        .build()
        .map_err(|error| model_error(format!("HTTP client 构建失败: {error}")))
}

pub async fn post_sse_with_retries(
    profile: &ModelProfile,
    url: &str,
    payload: &Value,
    headers: Vec<(String, String)>,
    proxy_url: Option<&str>,
    observer: Option<SseObserver<'_>>,
) -> AppResult<SseOutcome> {
    let idle_timeout = profile.request_timeout();
    let client = build_stream_client(idle_timeout, proxy_url)?;
    let mut all_headers = merged_headers(profile, headers);
    all_headers.push(("Accept".to_string(), "text/event-stream".to_string()));
    let attempts = (profile.max_retries.max(0) as u32) + 1;
    let mut last_error: Option<String> = None;

    for attempt in 0..attempts {
        if attempt > 0 {
            if let Some(observe) = observer {
                observe(SseSignal::Restart);
            }
        }
        let started = std::time::Instant::now();
        let mut request = client.post(url).json(payload);
        for (key, value) in &all_headers {
            request = request.header(key.as_str(), value.as_str());
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if status >= 400 {
                    let body = match read_http_response(response).await {
                        Ok(outcome) => outcome.body,
                        Err(error) => {
                            last_error = Some(describe_transport_error(&error));
                            if attempt == attempts - 1 {
                                break;
                            }
                            wait_before_retry().await;
                            continue;
                        }
                    };
                    if !is_retryable(status) || attempt == attempts - 1 {
                        return Ok(SseOutcome {
                            status,
                            body,
                            events: Vec::new(),
                        });
                    }
                    wait_before_retry().await;
                    continue;
                }
                match read_sse_events(response, observer).await {
                    Ok(events) => {
                        return Ok(SseOutcome {
                            status,
                            body: String::new(),
                            events,
                        })
                    }
                    Err(error) => {
                        last_error = Some(error.message);
                        if attempt == attempts - 1 {
                            break;
                        }
                        wait_before_retry().await;
                    }
                }
            }
            Err(error) => {
                let elapsed = started.elapsed().as_secs();
                last_error = Some(if error.is_timeout() {
                    format!(
                        "模型流式请求超时：{idle_timeout} 秒内没有收到新的输出（本次等待 {elapsed} 秒）。\
                         请在 Model 配置中提高 design 超时，或确认上游是否支持 stream。"
                    )
                } else {
                    format!(
                        "模型请求网络错误（发起后 {elapsed} 秒断开）：{}",
                        describe_transport_error(&error)
                    )
                });
                if attempt == attempts - 1 {
                    break;
                }
                wait_before_retry().await;
            }
        }
    }

    let message = last_error.unwrap_or_else(|| "模型流式请求失败：未收到有效响应".to_string());
    let suggestion = if proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        "请检查代理地址和代理服务，或清空代理后直连。".to_string()
    } else {
        format!(
            "请检查 {} 的 Base URL、DNS 和网络连接。",
            capitalize_role(&profile.role)
        )
    };
    Err(model_profile_error(
        profile,
        url,
        "model_network_error",
        message,
        None,
        suggestion,
    ))
}

async fn read_sse_events(
    response: reqwest::Response,
    observer: Option<SseObserver<'_>>,
) -> AppResult<Vec<Value>> {
    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut events: Vec<Value> = Vec::new();
    let push = |payload: &str, events: &mut Vec<Value>| {
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            if let Some(observe) = observer {
                observe(SseSignal::Event(&value));
            }
            events.push(value);
        }
    };

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            model_error(format!(
                "流式响应中断：{}",
                describe_transport_error(&error)
            ))
        })?;
        buffer.extend_from_slice(&chunk);
        for payload in drain_sse_events(&mut buffer) {
            push(&payload, &mut events);
        }
    }
    if !buffer.is_empty() {
        buffer.push(b'\n');
        for payload in drain_sse_events(&mut buffer) {
            push(&payload, &mut events);
        }
    }
    Ok(events)
}

pub fn parse_json_response(text: &str) -> AppResult<Value> {
    let fence_start = Regex::new(r"^```(?:json)?").expect("valid regex");
    let fence_end = Regex::new(r"```$").expect("valid regex");
    let cleaned = text.trim();
    let cleaned = fence_start.replace(cleaned, "");
    let cleaned = cleaned.trim();
    let cleaned = fence_end.replace(cleaned, "");
    let cleaned = cleaned.trim();

    if let Ok(value) = serde_json::from_str::<Value>(cleaned) {
        return Ok(value);
    }
    let mut last_error = None;
    for candidate in balanced_json_values(&cleaned) {
        match serde_json::from_str::<Value>(candidate) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(model_error(format!("模型返回的不是合法 JSON: {error}"))),
        None => Err(model_error("模型返回的不是合法 JSON")),
    }
}

/// Yields top-level balanced `{...}` / `[...]` substrings in arrival order,
/// skipping brackets that appear inside JSON string literals.
fn balanced_json_values(text: &str) -> Vec<&str> {
    let mut candidates = Vec::new();
    let mut start: Option<usize> = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' | '[' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' | ']' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(start) = start {
                            candidates.push(&text[start..=index]);
                        }
                        start = None;
                    }
                }
            }
            _ => {}
        }
    }
    candidates
}

#[cfg(test)]
mod tests;
