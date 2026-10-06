use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::core::net::{
    build_model_client, model_error, normalize_base_url, post_json_with_retries, require_api_key,
};
use crate::error::AppResult;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// 一次检索实际发往上游的请求体与原始返回（HTML 抓取类协议没有请求体）。
#[derive(Clone, Debug, Default)]
pub struct SearchTrace {
    pub request: Option<String>,
    pub response: Option<String>,
}

/// 检索结果连同它的报文：失败的调用也要能复盘发出去的是什么。
pub struct SearchCall {
    pub trace: SearchTrace,
    pub results: AppResult<Vec<SearchResult>>,
}

// 命中 grok2api 的 Build 路由：需要它自己的请求体与更严格的模型名。
pub const GROK_SEARCH_PROTOCOL: &str = "grok_search";

/// 官方无 JS 检索端点。默认直接用它，省掉 `duckduckgo.com/html/` 的那次 302。
const DUCKDUCKGO_ENDPOINT: &str = "https://html.duckduckgo.com/html";

/// 浏览器整页跳转时会带的头。
///
/// 只带 User-Agent 是不够的：DuckDuckGo 会直接回 HTTP 202 反爬页
/// （见 `duckduckgo_challenge`），页面里一个结果都没有，调用方会误以为“搜不到”。
const DUCKDUCKGO_HEADERS: [(&str, &str); 8] = [
    (
        "User-Agent",
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    ),
    (
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    ),
    ("Accept-Language", "en-US,en;q=0.9"),
    ("Upgrade-Insecure-Requests", "1"),
    ("Sec-Fetch-Dest", "document"),
    ("Sec-Fetch-Mode", "navigate"),
    ("Sec-Fetch-Site", "none"),
    ("Sec-Fetch-User", "?1"),
];

/// DuckDuckGo 的检索地址：地址留空就用官方无 JS 端点。
fn duckduckgo_url(base_url: &str, query: &str) -> String {
    let base = if base_url.trim().is_empty() {
        DUCKDUCKGO_ENDPOINT.to_string()
    } else {
        base_url.trim_end_matches('/').to_string()
    };
    let encoded = urlencode(query);
    if base.ends_with("/html") {
        format!("{base}/?q={encoded}")
    } else {
        format!("{base}/html/?q={encoded}")
    }
}

/// DuckDuckGo 的反爬页：HTTP 202，正文里带 anomaly-modal。
///
/// 这是“被挡住”，不是“没有结果”，两者必须分开：前者要让任务看见并失败，
/// 后者才能安静地变成“没有客观外观描述”。
fn duckduckgo_challenge(body: &str) -> bool {
    body.contains("anomaly-modal")
        || body
            .to_ascii_lowercase()
            .contains("bots use duckduckgo too")
}

pub struct SearchClient;

impl SearchClient {
    pub async fn search(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> SearchCall {
        let protocol = profile.protocol.trim().to_lowercase();
        let results = match protocol.as_str() {
            "duckduckgo" => Self::duckduckgo(profile, query, max_results, proxy_url).await,
            "tavily" => Self::tavily(profile, query, max_results, proxy_url).await,
            "openai_chat" | "openai_responses" | GROK_SEARCH_PROTOCOL => {
                return Self::model_search(profile, query, max_results, proxy_url).await
            }
            other => Err(model_error(format!("Unsupported search protocol: {other}"))),
        };
        SearchCall {
            trace: SearchTrace::default(),
            results,
        }
    }

    async fn duckduckgo(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> AppResult<Vec<SearchResult>> {
        let url = duckduckgo_url(&profile.base_url, query);
        let timeout = profile.request_timeout();
        let client = build_model_client(timeout, proxy_url)?;
        let mut request = client.get(&url);
        for (key, value) in DUCKDUCKGO_HEADERS {
            request = request.header(key, value);
        }
        for (key, value) in &profile.headers {
            if let Some(text) = value.as_str() {
                request = request.header(key.as_str(), text);
            }
        }
        let response = request
            .send()
            .await
            .map_err(|error| model_error(format!("DuckDuckGo search failed: {error}")))?;
        let status = response.status().as_u16();
        if status >= 400 {
            return Err(model_error(format!(
                "DuckDuckGo search failed: HTTP {status}"
            )));
        }
        let body = response
            .text()
            .await
            .map_err(|error| model_error(format!("DuckDuckGo search failed: {error}")))?;
        if duckduckgo_challenge(&body) {
            return Err(model_error(format!(
                "DuckDuckGo answered with its anti-bot challenge instead of results (HTTP {status})\\n\
                 It refuses this request; search with tavily or grok_search instead, \
                 or send DuckDuckGo through another proxy."
            )));
        }
        Ok(Self::parse_duckduckgo_html(&body, max_results))
    }

    async fn tavily(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> AppResult<Vec<SearchResult>> {
        let base = if profile.base_url.trim().is_empty() {
            "https://api.tavily.com".to_string()
        } else {
            profile.base_url.trim_end_matches('/').to_string()
        };
        let depth = profile
            .output_defaults
            .get("search_depth")
            .and_then(|value| value.as_str())
            .unwrap_or("basic");
        let payload = json!({
            "api_key": require_api_key(profile)?,
            "query": query,
            "max_results": max_results,
            "include_answer": false,
            "search_depth": depth
        });
        let outcome = post_json_with_retries(
            profile,
            &format!("{base}/search"),
            &payload,
            vec![],
            proxy_url,
        )
        .await?;
        if outcome.status >= 400 {
            return Err(model_error(format!(
                "Tavily search failed: HTTP {} {}",
                outcome.status,
                outcome.body.chars().take(200).collect::<String>()
            )));
        }
        let data: Value = serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("Tavily 响应不是 JSON: {error}")))?;
        let mut results = Vec::new();
        if let Some(items) = data["results"].as_array() {
            for item in items.iter().filter(|item| item.is_object()) {
                results.push(SearchResult {
                    title: item["title"]
                        .as_str()
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    url: item["url"].as_str().unwrap_or_default().trim().to_string(),
                    snippet: item["content"]
                        .as_str()
                        .or_else(|| item["snippet"].as_str())
                        .unwrap_or_default()
                        .chars()
                        .take(500)
                        .collect(),
                });
            }
        }
        Ok(filter_results(results, max_results))
    }

    async fn model_search(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> SearchCall {
        let mut trace = SearchTrace::default();
        let results =
            Self::model_search_inner(profile, query, max_results, proxy_url, &mut trace).await;
        SearchCall { trace, results }
    }

    async fn model_search_inner(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
        trace: &mut SearchTrace,
    ) -> AppResult<Vec<SearchResult>> {
        let base = normalize_base_url(
            if profile.base_url.trim().is_empty() {
                "https://api.openai.com"
            } else {
                &profile.base_url
            },
            "openai_chat",
        );
        let system = format!(
            "You are a web search assistant for academic slide visual grounding. \
             Return strict JSON only: {{\"results\":[{{\"title\":\"\",\"url\":\"\",\"snippet\":\"\"}}]}}. \
             Return at most {max_results} source-supported results for the exact requested entity. \
             Each snippet must describe visible shape, structure, colors or distinguishing features supported by \
             that source, not a link list. Keep the original entity and qualifiers; never replace it with its \
             vendor, brand or logo. Do not invent appearance. Prefer official docs and product pages. \
             If no source supports a visual description, return {{\"results\":[]}}. No markdown fences."
        );
        let model = if profile.model.trim().is_empty() {
            "gpt-4o-mini"
        } else {
            &profile.model
        };
        let mut payload = json!({
            "model": model,
            "temperature": 0.1,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": format!("Search query: {query}")}
            ]
        });
        if profile
            .protocol
            .trim()
            .eq_ignore_ascii_case(GROK_SEARCH_PROTOCOL)
        {
            let options = grok_search_options(profile)?;
            payload
                .as_object_mut()
                .expect("search payload is an object")
                .extend(
                    options
                        .as_object()
                        .expect("Grok options are an object")
                        .clone(),
                );
        }
        // 记下真实发出去的报文（API key 在 header 里，不在请求体里）。
        trace.request =
            Some(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()));
        let outcome = post_json_with_retries(
            profile,
            &format!("{base}/chat/completions"),
            &payload,
            vec![(
                "Authorization".to_string(),
                format!("Bearer {}", require_api_key(profile)?),
            )],
            proxy_url,
        )
        .await?;
        trace.response = Some(outcome.body.clone());
        if outcome.status >= 400 {
            return Err(model_error(format!(
                "Search model failed: HTTP {} {}",
                outcome.status,
                outcome.body.chars().take(200).collect::<String>()
            )));
        }
        let data: Value = serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("Search model 响应不是 JSON: {error}")))?;
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        let mut results = Self::parse_json_results(content)?;
        results.truncate(max_results);
        Ok(results)
    }

    fn parse_json_results(content: &str) -> AppResult<Vec<SearchResult>> {
        let value = crate::core::net::parse_json_response(content)?;
        let items = if value.is_array() {
            value.as_array().cloned().unwrap_or_default()
        } else {
            value["results"]
                .as_array()
                .cloned()
                .ok_or_else(|| model_error("Search model JSON missing results list"))?
        };
        Ok(filter_results(
            items
                .iter()
                .filter(|item| item.is_object())
                .map(|item| SearchResult {
                    title: item["title"]
                        .as_str()
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    url: item["url"].as_str().unwrap_or_default().trim().to_string(),
                    snippet: item["snippet"]
                        .as_str()
                        .or_else(|| item["content"].as_str())
                        .unwrap_or_default()
                        .chars()
                        .take(500)
                        .collect(),
                })
                .collect(),
            usize::MAX,
        ))
    }

    pub fn parse_duckduckgo_html(html_text: &str, max_results: usize) -> Vec<SearchResult> {
        let link_pattern = Regex::new(
            r#"(?is)<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#,
        )
        .expect("valid regex");
        let snippet_pattern = Regex::new(
            r#"(?is)<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>|<td[^>]+class="[^"]*result-snippet[^"]*"[^>]*>(.*?)</td>"#,
        )
        .expect("valid regex");

        let links: Vec<_> = link_pattern.captures_iter(html_text).collect();
        let mut results = Vec::new();
        for (index, caps) in links.iter().enumerate() {
            let end = links
                .get(index + 1)
                .map(|next| next.get(0).unwrap().start())
                .unwrap_or(html_text.len());
            let region = &html_text[caps.get(0).unwrap().end()..end];
            let snippet = snippet_pattern
                .captures(region)
                .and_then(|caps| {
                    caps.get(1)
                        .or_else(|| caps.get(2))
                        .map(|m| clean_html(m.as_str()))
                })
                .unwrap_or_default();
            let href = unescape_html(caps.get(1).map(|m| m.as_str()).unwrap_or_default());
            results.push(SearchResult {
                title: clean_html(caps.get(2).map(|m| m.as_str()).unwrap_or_default()),
                url: normalize_result_url(&href),
                snippet,
            });
        }
        filter_results(results, max_results)
    }
}

/// Common evidence validation for every provider; filtering/deduplication precedes limiting.
pub fn filter_results(results: Vec<SearchResult>, max_results: usize) -> Vec<SearchResult> {
    let links =
        Regex::new(r"!?\[[^\]]*\]\([^)]*\)|(?is:<a\b[^>]*>.*?</a>)|(?i:(?:https?://|www\.)\S+)")
            .expect("valid evidence regex");
    let labels = Regex::new(r"(?im)^\s*(?:[-*•]|\d+[.)、])\s*|(?i:\b(?:sources?|references?|links?|citations?|official\s+links?)\s*[:：]?)|(?:参考(?:资料|链接|来源|文献)?|来源|链接|引用|官网)\s*[:：]?")
        .expect("valid reference label regex");
    let mut seen = std::collections::HashSet::new();
    results
        .into_iter()
        .filter_map(|mut result| {
            result.url = result.url.trim().to_string();
            result.snippet = result.snippet.trim().chars().take(500).collect();
            if !(result.url.to_ascii_lowercase().starts_with("https://")
                || result.url.to_ascii_lowercase().starts_with("http://"))
            {
                return None;
            }
            let mut url = reqwest::Url::parse(&result.url).ok()?;
            let prose = links.replace_all(&result.snippet, "");
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || result.url.contains('\\')
                || result.url.chars().any(char::is_whitespace)
                || !labels
                    .replace_all(&prose, "")
                    .chars()
                    .any(char::is_alphabetic)
            {
                return None;
            }
            url.set_fragment(None);
            if !seen.insert(url.as_str().trim_end_matches('/').to_string()) {
                return None;
            }
            result.title = result.title.trim().to_string();
            Some(result)
        })
        .take(max_results)
        .collect()
}
fn grok_search_options(profile: &ModelProfile) -> AppResult<Value> {
    // 只挡明显为空的写法（两侧留空格、空的 `Build/` 前缀）：模型到底支不支持联网
    // 工具由服务端回答，客户端不猜；失败会让任务失败，不静默降级。
    let model = &profile.model;
    let id = model.strip_prefix("Build/").unwrap_or(model);
    if id.trim().is_empty() || model != model.trim() {
        return Err(model_error(
            "Grok search requires a non-empty model ID, for example grok-build-0.1",
        ));
    }
    let mut tools = match profile.output_defaults.get("tools") {
        None => Vec::new(),
        Some(Value::Array(tools)) if tools.iter().all(Value::is_object) => tools.clone(),
        _ => return Err(model_error("Grok search tools must be an array of objects")),
    };
    // 上游要求请求体里同时声明两种搜索工具：缺哪个补哪个，用户另配的工具原样保留。
    for kind in ["x_search", "web_search"] {
        if !tools.iter().any(|tool| tool["type"] == kind) {
            tools.push(json!({"type": kind}));
        }
    }
    Ok(json!({"model": model, "tools": tools, "stream": false, "tool_choice": "required"}))
}

fn normalize_result_url(url: &str) -> String {
    let Some((_, query)) = url.split_once('?') else {
        return url.to_string();
    };
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("uddg=") {
            return urldecode(value);
        }
    }
    url.to_string()
}

fn clean_html(value: &str) -> String {
    let tags = Regex::new(r"<[^>]+>").expect("valid regex");
    let spaces = Regex::new(r"\s+").expect("valid regex");
    let text = tags.replace_all(value, " ");
    let text = unescape_html(&text);
    spaces.replace_all(&text, " ").trim().to_string()
}

fn unescape_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn urlencode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push('+'),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckduckgo_url_defaults_to_the_official_no_js_endpoint() {
        // 曾经把带尾斜杠的常量再拼一次 /html/，拼出 /html//html/。
        assert_eq!(
            duckduckgo_url("", "grok bot"),
            "https://html.duckduckgo.com/html/?q=grok+bot"
        );
        assert_eq!(
            duckduckgo_url("https://duckduckgo.com", "a b"),
            "https://duckduckgo.com/html/?q=a+b"
        );
        assert_eq!(
            duckduckgo_url("https://html.duckduckgo.com/html", "q"),
            "https://html.duckduckgo.com/html/?q=q"
        );
    }

    #[test]
    fn parses_duckduckgo_results_and_unwraps_redirect() {
        let html = r#"
        <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.docker.com%2Fcompany%2Fnewsroom%2Fmedia-resources%2F">Docker Media Resources</a>
        <a class="result__snippet">Official Docker logos and brand resources.</a>
        "#;
        let results = SearchClient::parse_duckduckgo_html(html, 3);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].url,
            "https://www.docker.com/company/newsroom/media-resources/"
        );
        assert_eq!(results[0].title, "Docker Media Resources");
        assert!(results[0].snippet.contains("Official Docker logos"));
    }

    #[test]
    fn parses_model_search_json() {
        let content = "```json\n{\"results\":[{\"title\":\"T\",\"url\":\"https://a.b\",\"snippet\":\"S\"}]}\n```";
        let results = SearchClient::parse_json_results(content).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://a.b");
    }

    #[test]
    fn grok_tools_preserve_options_and_validate_model_id() {
        let mut profile: ModelProfile = serde_json::from_value(json!({
            "id":"search", "role":"search", "name":"Search", "protocol":"grok_search",
            "base_url":"https://example.test/v1", "model":"grok-build-0.1",
            "headers":{}, "max_retries":0, "output_defaults":{}
        }))
        .unwrap();
        for (tools, expected) in [
            (
                json!([]),
                json!([{"type":"x_search"},{"type":"web_search"}]),
            ),
            (
                json!([{"type":"x_search"}]),
                json!([{"type":"x_search"},{"type":"web_search"}]),
            ),
            (
                json!([{"type":"web_search","options":{"country":"US"}}]),
                json!([{"type":"web_search","options":{"country":"US"}},{"type":"x_search"}]),
            ),
            (
                json!([{"type":"web_search"},{"type":"x_search"}]),
                json!([{"type":"web_search"},{"type":"x_search"}]),
            ),
            (
                json!([{"type":"function","function":{"name":"web_search"}}]),
                json!([{"type":"function","function":{"name":"web_search"}},{"type":"x_search"},{"type":"web_search"}]),
            ),
        ] {
            profile
                .output_defaults
                .insert("tools".into(), tools.clone());
            let options = grok_search_options(&profile).unwrap();
            assert_eq!(options["tools"], expected);
            assert_eq!(options["stream"], false);
            assert_eq!(options["tool_choice"], "required");
            assert_eq!(profile.output_defaults["tools"], tools);
        }
        for tools in [Value::Null, json!({}), json!(["web_search"])] {
            profile.output_defaults.insert("tools".into(), tools);
            assert!(grok_search_options(&profile).is_err());
        }
        profile.output_defaults.remove("tools");
        // 部署直接暴露的模型 ID（例如 grok-build-0.1）必须能用，不能硬要求 Build/ 前缀。
        for model in ["grok-build-0.1", "grok-4.5", "Build/deployed-model"] {
            profile.model = model.into();
            let options = grok_search_options(&profile).unwrap();
            assert_eq!(options["model"], model);
        }
        for model in ["", " ", "Build/", "Build/ ", " Build/model", "Build/model "] {
            profile.model = model.into();
            assert!(grok_search_options(&profile).is_err(), "{model:?}");
        }
    }

    #[test]
    fn encodes_chinese_query() {
        assert_eq!(
            urlencode("离心机 实物"),
            "%E7%A6%BB%E5%BF%83%E6%9C%BA+%E5%AE%9E%E7%89%A9"
        );
        assert_eq!(urldecode("%E7%A6%BB%E5%BF%83%E6%9C%BA"), "离心机");
    }
    fn evidence_fixture() -> Value {
        json!([
            {"url":"https://example.test/empty", "snippet":" "},
            {"url":"https://example.test/links", "snippet":"1. https://a.test\n2. https://b.test"},
            {"url":"https://example.test/cn", "snippet":"参考链接：https://a.test"},
            {"url":"https://example.test/en", "snippet":"Sources: https://a.test"},
            {"url":"https://example.test/md", "snippet":"[Official page](https://a.test)"},
            {"url":"javascript:alert(1)", "snippet":"silver articulated arms"},
            {"url":"https://", "snippet":"silver articulated arms"},
            {"url":"https://bad host/test", "snippet":"silver articulated arms"},
            {"url":"https://example.test/first", "snippet":"silver articulated arms"},
            {"url":"https://EXAMPLE.test/first#duplicate", "snippet":"duplicate description"},
            {"url":"https://example.test/second", "snippet":"圆形底座与蓝色外壳"}
        ])
    }

    #[test]
    fn missing_ddg_snippet_cannot_borrow_the_next_description() {
        let html = r#"<a class="result__a" href="https://example.test/empty">Empty</a><a class="result__a" href="https://example.test/real">Real</a><a class="result__snippet">Two silver arms</a>"#;
        let results = SearchClient::parse_duckduckgo_html(html, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.test/real");
        assert_eq!(results[0].snippet, "Two silver arms");
    }

    #[test]
    fn bare_links_and_reference_labels_are_not_visual_evidence() {
        for snippet in [
            "",
            "https://a.test",
            "1. https://a.test\n2. https://b.test",
            "参考链接：https://a.test",
            "Sources: https://a.test",
            r#"<a href="https://a.test">Official page</a>"#,
        ] {
            let results = filter_results(
                vec![SearchResult {
                    title: "Page".into(),
                    url: "https://a.test".into(),
                    snippet: snippet.into(),
                }],
                3,
            );
            assert!(results.is_empty(), "{snippet}");
        }
    }

    #[test]
    fn truncation_cannot_turn_accepted_evidence_into_bare_links() {
        let results = filter_results(
            vec![SearchResult {
                title: String::new(),
                url: "https://example.test".into(),
                snippet: format!("https://example.test/{} silver arms", "x".repeat(600)),
            }],
            3,
        );
        assert!(results.is_empty());
    }

    async fn local_reply(status: u16, body: String) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut input = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0, "request ended before its body");
                    input.extend_from_slice(&buffer[..count]);
                    if let Some(end) = input.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&input[..end]);
                        let length = headers.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                        }).unwrap_or(0);
                        if input.len() >= end + 4 + length { break; }
                    }
                }
                let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
                String::from_utf8(input).unwrap()
            }).await.expect("local search request timed out")
        });
        (base, task)
    }

    fn local_profile(protocol: &str, base: String) -> ModelProfile {
        serde_json::from_value(json!({
            "id":"search", "role":"search", "name":"Search", "protocol":protocol,
            "base_url":base, "model":"grok-build-0.1", "api_key":"local-test-only",
            "headers":{}, "max_retries":0, "timeout_seconds":5, "output_defaults":{}
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn all_providers_filter_before_limit_and_preserve_original_query() {
        let evidence = evidence_fixture();
        for protocol in [
            "openai_chat",
            "openai_responses",
            "grok_search",
            "tavily",
            "duckduckgo",
        ] {
            let model = !matches!(protocol, "tavily" | "duckduckgo");
            let body = if model {
                json!({"choices":[{"message":{"content":json!({"results":evidence}).to_string()}}]})
                    .to_string()
            } else if protocol == "tavily" {
                json!({"results":evidence}).to_string()
            } else {
                evidence.as_array().unwrap().iter().map(|item| format!(
                    "<a class=\"result__a\" href=\"{}\">Title</a><a class=\"result__snippet\">{}</a>",
                    item["url"].as_str().unwrap(), item["snippet"].as_str().unwrap()
                )).collect::<String>()
            };
            let (base, task) = local_reply(200, body.clone()).await;
            let call =
                SearchClient::search(&local_profile(protocol, base), "gRoK bot", 2, None).await;
            let request = task.await.unwrap();
            let results = call
                .results
                .unwrap_or_else(|error| panic!("{protocol}: {error:?}"));
            assert_eq!(results.len(), 2, "{protocol}");
            assert_eq!(results[0].url, "https://example.test/first");
            assert_eq!(results[1].url, "https://example.test/second");
            assert_eq!(results[1].snippet, "圆形底座与蓝色外壳");
            if model {
                assert_eq!(call.trace.response.as_deref(), Some(body.as_str()));
                let payload: Value = serde_json::from_str(&call.trace.request.unwrap()).unwrap();
                let sent: Value =
                    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                assert_eq!(sent, payload);
                assert_eq!(payload["messages"][1]["content"], "Search query: gRoK bot");
                let system = payload["messages"][0]["content"].as_str().unwrap();
                for phrase in [
                    "source-supported",
                    "Each snippet must describe",
                    "never replace",
                    "Do not invent appearance",
                ] {
                    assert!(system.contains(phrase));
                }
                if protocol == "grok_search" {
                    assert_eq!(payload["tool_choice"], "required");
                    assert_eq!(payload["stream"], false);
                } else {
                    assert!(payload.get("tools").is_none());
                }
            } else if protocol == "tavily" {
                let payload: Value =
                    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                assert_eq!(payload["query"], "gRoK bot");
            } else {
                assert!(request.starts_with("GET /html/?q=gRoK+bot "));
                // 反爬页就是少这些头回来的：每个都要真的发出去。
                for header in [
                    "sec-fetch-dest: document",
                    "sec-fetch-mode: navigate",
                    "sec-fetch-site: none",
                    "sec-fetch-user: ?1",
                    "upgrade-insecure-requests: 1",
                    "accept-language: en-us,en;q=0.9",
                ] {
                    assert!(
                        request.to_ascii_lowercase().contains(header),
                        "missing {header} in {request}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn duckduckgo_anti_bot_page_is_an_error_not_an_empty_result() {
        // 真实反爬页的骨架：HTTP 202 + anomaly-modal。
        let challenge = "<html><body><div class=\"anomaly-modal\"><p>Unfortunately, bots use DuckDuckGo too.</p></div></body></html>".to_string();
        let (base, task) = local_reply(202, challenge).await;
        let call =
            SearchClient::search(&local_profile("duckduckgo", base), "grok bot", 3, None).await;
        task.await.unwrap();
        let error = call
            .results
            .expect_err("a challenge must not look like no results");
        let message = error.message;
        assert!(message.contains("anti-bot challenge"), "{message}");
        assert!(message.contains("tavily or grok_search"), "{message}");
    }

    #[tokio::test]
    async fn failed_model_call_keeps_transport_trace() {
        let (base, task) = local_reply(401, "denied".into()).await;
        let call =
            SearchClient::search(&local_profile("grok_search", base), "grokbot", 3, None).await;
        task.await.unwrap();
        assert!(call.results.is_err());
        assert_eq!(call.trace.response.as_deref(), Some("denied"));
        assert!(call
            .trace
            .request
            .unwrap()
            .contains("Search query: grokbot"));
    }
}
