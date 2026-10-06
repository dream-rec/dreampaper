use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as WallClock};

use serde_json::{json, Map};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{advance, Instant};

use super::*;
use crate::core::model::design::ImageInput;
use crate::core::model::implement::ImplementClient;

#[derive(Clone)]
struct Reply {
    status: u16,
    chunks: Vec<(Duration, String)>,
    extra_length: usize,
    hold: bool,
    headers: bool,
}

impl Reply {
    fn body(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            chunks: vec![(Duration::ZERO, body.into())],
            extra_length: 0,
            hold: false,
            headers: true,
        }
    }

    fn stalled(headers: bool) -> Self {
        Self {
            status: 200,
            chunks: Vec::new(),
            extra_length: 100,
            hold: true,
            headers,
        }
    }
}

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Instant>>>,
    task: JoinHandle<()>,
}

impl Server {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            let mut replies = replies.into_iter();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let reply = replies
                    .next()
                    .unwrap_or_else(|| Reply::body(500, "exhausted"));
                let observed = observed.clone();
                connections.spawn(async move {
                    serve(stream, reply, observed).await;
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(mut stream: TcpStream, reply: Reply, requests: Arc<Mutex<Vec<Instant>>>) {
    let mut input = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        if count == 0 {
            return;
        }
        input.extend_from_slice(&buffer[..count]);
        if let Some(end) = input.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&input[..end]);
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
    requests.lock().unwrap().push(Instant::now());
    if reply.headers {
        let length = reply
            .chunks
            .iter()
            .map(|(_, body)| body.len())
            .sum::<usize>()
            + reply.extra_length;
        let headers = format!(
            "HTTP/1.1 {} Test\r\nContent-Length: {length}\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            reply.status,
        );
        if stream.write_all(headers.as_bytes()).await.is_err() {
            return;
        }
        for (delay, body) in reply.chunks {
            tokio::time::sleep(delay).await;
            if stream.write_all(body.as_bytes()).await.is_err() {
                return;
            }
        }
    }
    if reply.hold {
        std::future::pending::<()>().await;
    }
}

// Keep the paused clock from advancing while the local sockets are being polled.
async fn until(mut ready: impl FnMut() -> bool) {
    let deadline = WallClock::now() + Duration::from_secs(5);
    while !ready() {
        assert!(WallClock::now() < deadline, "本地请求未在期限内推进");
        tokio::task::yield_now().await;
    }
}

async fn settle() {
    let deadline = WallClock::now() + Duration::from_millis(20);
    while WallClock::now() < deadline {
        tokio::task::yield_now().await;
    }
}

#[derive(Clone, Copy, Debug)]
enum Api {
    Json,
    Sse,
    Image,
    Edit,
}

impl Api {
    const ALL: [Self; 4] = [Self::Json, Self::Sse, Self::Image, Self::Edit];

    fn success(self) -> Reply {
        let body = match self {
            Self::Json => json!({"ok": true}).to_string(),
            Self::Sse => "data: {\"ok\":true}\n\n".to_string(),
            Self::Image | Self::Edit => {
                json!({"data": [{"b64_json": "a".repeat(256)}]}).to_string()
            }
        };
        Reply::body(200, body)
    }

    fn run(self, server: &Server, timeout: i64, retries: i64) -> JoinHandle<AppResult<()>> {
        let url = server.url.clone();
        tokio::spawn(async move {
            let profile = profile(&url, timeout, retries);
            match self {
                Self::Json => {
                    let outcome =
                        post_json_with_retries(&profile, &url, &json!({}), vec![], Some(&url))
                            .await?;
                    if outcome.status >= 400 {
                        return Err(model_error(format!("HTTP {}", outcome.status)));
                    }
                }
                Self::Sse => {
                    let outcome =
                        post_sse_with_retries(&profile, &url, &json!({}), vec![], Some(&url), None)
                            .await?;
                    if outcome.status >= 400 {
                        return Err(model_error(format!("HTTP {}", outcome.status)));
                    }
                }
                Self::Image | Self::Edit => {
                    let images = if matches!(self, Self::Edit) {
                        vec![ImageInput {
                            filename: "reference.png".to_string(),
                            mime_type: "image/png".to_string(),
                            b64: encode_b64(b"reference"),
                        }]
                    } else {
                        Vec::new()
                    };
                    ImplementClient::generate(&profile, "test", &images, &Map::new(), Some(&url))
                        .await?;
                }
            }
            Ok(())
        })
    }
}

fn profile(url: &str, timeout: i64, retries: i64) -> ModelProfile {
    ModelProfile {
        id: "test".to_string(),
        role: "implement".to_string(),
        name: "test".to_string(),
        protocol: "image2".to_string(),
        base_url: url.to_string(),
        model: "test".to_string(),
        api_key: Some("test".to_string()),
        api_version: None,
        headers: Map::new(),
        timeout_seconds: timeout,
        max_retries: retries,
        output_defaults: Map::new(),
        has_api_key: None,
        api_key_hint: None,
        proxy_url: None,
    }
}

#[tokio::test(start_paused = true)]
async fn every_retryable_status_waits_exactly_180_seconds() {
    for api in Api::ALL {
        for status in [429, 500, 502, 503, 504] {
            let server = Server::start(vec![Reply::body(status, "retry"), api.success()]).await;
            let task = api.run(&server, 7, 1);
            until(|| server.count() == 1).await;
            settle().await;
            advance(Duration::from_secs(179)).await;
            settle().await;
            assert_eq!(server.count(), 1, "{api:?} HTTP {status}");
            assert!(!task.is_finished());
            advance(Duration::from_secs(1)).await;
            until(|| task.is_finished()).await;
            task.await.unwrap().unwrap();
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[1] - requests[0], Duration::from_secs(180));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn zero_retries_and_non_retryable_errors_do_not_wait() {
    for api in Api::ALL {
        for (status, retries) in [(503, 0), (400, 2), (401, 2), (403, 2), (404, 2)] {
            let server = Server::start(vec![Reply::body(status, "error")]).await;
            let task = api.run(&server, 5, retries);
            until(|| task.is_finished()).await;
            assert!(task.await.unwrap().is_err());
            assert_eq!(server.count(), 1);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn retry_budget_counts_the_initial_request_and_does_not_sleep_after_exhaustion() {
    for api in Api::ALL {
        let server = Server::start(vec![Reply::body(503, "retry"); 3]).await;
        let task = api.run(&server, 600, 2);
        for expected in 1..=3 {
            until(|| server.count() == expected).await;
            settle().await;
            if expected < 3 {
                assert!(!task.is_finished());
                advance(Duration::from_secs(180)).await;
            }
        }
        until(|| task.is_finished()).await;
        assert!(task.await.unwrap().is_err());
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2] - requests[0], Duration::from_secs(360));
    }
}

#[tokio::test(start_paused = true)]
async fn configured_timeout_applies_to_headers_and_body_before_fixed_retry() {
    for api in Api::ALL {
        for headers in [false, true] {
            let server = Server::start(vec![Reply::stalled(headers), api.success()]).await;
            let task = api.run(&server, 1, 1);
            until(|| server.count() == 1).await;
            settle().await;
            advance(Duration::from_secs(1)).await;
            settle().await;
            advance(Duration::from_secs(179)).await;
            settle().await;
            assert_eq!(server.count(), 1);
            assert!(!task.is_finished());
            advance(Duration::from_secs(1)).await;
            until(|| task.is_finished()).await;
            task.await.unwrap().unwrap();
            let requests = server.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[1] - requests[0], Duration::from_secs(181));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn truncated_success_bodies_retry_but_auth_errors_do_not() {
    for api in Api::ALL {
        let mut broken = api.success();
        broken.extra_length = 10;
        let server = Server::start(vec![broken, api.success()]).await;
        let task = api.run(&server, 120, 1);
        until(|| server.count() == 1).await;
        settle().await;
        assert!(!task.is_finished());
        advance(Duration::from_secs(180)).await;
        until(|| task.is_finished()).await;
        task.await.unwrap().unwrap();
        assert_eq!(server.count(), 2);

        let mut unauthorized = Reply::body(401, "unauthorized");
        unauthorized.extra_length = 10;
        let server = Server::start(vec![unauthorized]).await;
        let task = api.run(&server, 120, 2);
        until(|| task.is_finished()).await;
        assert!(task.await.unwrap().is_err());
        assert_eq!(server.count(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn streaming_timeout_resets_when_data_arrives() {
    let mut reply = Reply::body(200, "");
    reply.chunks = vec![
        (Duration::ZERO, "data: {\"part\":1}\n\n".to_string()),
        (
            Duration::from_millis(750),
            "data: {\"part\":2}\n\n".to_string(),
        ),
        (
            Duration::from_millis(750),
            "data: {\"part\":3}\n\n".to_string(),
        ),
    ];
    let server = Server::start(vec![reply]).await;
    let task = Api::Sse.run(&server, 1, 0);
    until(|| server.count() == 1).await;
    settle().await;
    advance(Duration::from_millis(750)).await;
    settle().await;
    assert!(!task.is_finished());
    advance(Duration::from_millis(750)).await;
    until(|| task.is_finished()).await;
    task.await.unwrap().unwrap();
    assert_eq!(server.count(), 1);
}

#[tokio::test(start_paused = true)]
async fn model_connection_timeout_is_not_capped_at_30_seconds() {
    for streaming in [false, true] {
        let server = Server::start(vec![Reply::stalled(false)]).await;
        let client = if streaming {
            build_stream_client(60, Some(&server.url)).unwrap()
        } else {
            build_model_client(60, Some(&server.url)).unwrap()
        };
        let task =
            tokio::spawn(async move { client.get("https://model.invalid/test").send().await });
        until(|| server.count() == 1).await;
        settle().await;
        advance(Duration::from_secs(31)).await;
        settle().await;
        assert!(!task.is_finished());
        advance(Duration::from_secs(29)).await;
        until(|| task.is_finished()).await;
        assert!(task.await.unwrap().unwrap_err().is_timeout());
    }
}

#[test]
fn only_an_explicit_non_blank_proxy_counts() {
    assert_eq!(configured_proxy(None), None);
    assert_eq!(configured_proxy(Some("")), None);
    assert_eq!(configured_proxy(Some("   ")), None);
    assert_eq!(
        configured_proxy(Some(" http://127.0.0.1:7890 ")),
        Some("http://127.0.0.1:7890")
    );
}

#[tokio::test]
async fn environment_proxies_are_ignored_without_an_explicit_setting() {
    // 用户 shell 里常见的 *_PROXY 不能悄悄生效：只认设置里填的地址。
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("代理监听");
    let proxy = format!("http://{}", listener.local_addr().expect("地址"));
    let closed = TcpListener::bind("127.0.0.1:0").await.expect("目标监听");
    let target = closed.local_addr().expect("地址");
    drop(closed); // 目标端口已释放：直连立刻被拒，只有走代理才会连到上面那个监听端口。
    let proxy_keys = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    for key in proxy_keys {
        std::env::set_var(key, &proxy);
    }
    for key in ["NO_PROXY", "no_proxy"] {
        std::env::remove_var(key);
    }

    let client = build_model_client(3, None).expect("客户端");
    let _ = client.get(format!("http://{target}/")).send().await;

    for key in proxy_keys {
        std::env::remove_var(key);
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err(),
        "环境里的代理不该被采用"
    );
}

#[test]
fn duckduckgo_proxy_prefers_its_own_setting_over_the_app_proxy() {
    assert_eq!(
        duckduckgo_proxy(Some("http://127.0.0.1:7890"), Some("http://app:1")).as_deref(),
        Some("http://127.0.0.1:7890")
    );
    assert_eq!(
        duckduckgo_proxy(None, Some(" http://app:1 ")).as_deref(),
        Some("http://app:1")
    );
    assert_eq!(duckduckgo_proxy(Some("  "), Some("")), None);
}

#[test]
fn transport_suggestions_point_at_the_real_cause() {
    // 超时不该引导用户去查代理或 DNS：grok_search 的往返天生要几十秒。
    let grok = transport_suggestion("search", "grok_search", true, false);
    assert!(grok.contains("120 秒"), "{grok}");
    assert!(!grok.contains("DNS"), "{grok}");

    let general = transport_suggestion("design", "openai_responses", true, false);
    assert!(general.contains("超时"), "{general}");

    assert!(transport_suggestion("search", "grok_search", false, true).contains("代理"));
    assert!(transport_suggestion("search", "grok_search", false, false).contains("DNS"));
}
