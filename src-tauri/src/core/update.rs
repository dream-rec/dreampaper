use std::time::Duration;

use reqwest::header::USER_AGENT;
use semver::Version;
use serde::Serialize;

use crate::error::{AppError, AppResult};

const LATEST_RELEASE_URL: &str = "https://github.com/dream-rec/dreampaper/releases/latest";
const RELEASE_TAG_URL: &str = "https://github.com/dream-rec/dreampaper/releases/tag/";

#[derive(Clone, Debug, Serialize)]
pub struct ReleaseInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub update_available: bool,
}

pub async fn check(current_version: &str, proxy_url: Option<&str>) -> AppResult<ReleaseInfo> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(15));
    if let Some(proxy) = crate::core::net::configured_proxy(proxy_url) {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy)
                .map_err(|error| AppError::new("invalid_proxy", error.to_string()))?,
        );
    }
    let client = builder
        .build()
        .map_err(|error| AppError::new("update_client_failed", error.to_string()))?;
    let response = client
        .get(LATEST_RELEASE_URL)
        .header(USER_AGENT, format!("DreamPaper/{current_version}"))
        .send()
        .await
        .map_err(|error| AppError::new("update_request_failed", error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let message = match status.as_u16() {
            404 => "暂未找到可用的 Release",
            _ => "检查更新失败，请稍后重试",
        };
        return Err(AppError::new("update_http_error", message));
    }
    release_info_from_url(current_version, response.url().as_str())
}

fn release_info_from_url(current_version: &str, release_url: &str) -> AppResult<ReleaseInfo> {
    let tag_name = release_url
        .strip_prefix(RELEASE_TAG_URL)
        .filter(|value| {
            !value.is_empty()
                && !value
                    .chars()
                    .any(|character| matches!(character, '/' | '?' | '#'))
        })
        .ok_or_else(|| {
            AppError::new(
                "update_url_invalid",
                "最新版本链接不是当前项目的 Release 标签",
            )
        })?;
    let current = parse_version(current_version)?;
    let latest = parse_version(tag_name)?;
    Ok(ReleaseInfo {
        current_version: current.to_string(),
        latest_version: latest.to_string(),
        release_url: release_url.to_string(),
        update_available: latest > current,
    })
}

fn parse_version(value: &str) -> AppResult<Version> {
    let normalized = value
        .trim()
        .strip_prefix('v')
        .or_else(|| value.trim().strip_prefix('V'))
        .unwrap_or(value.trim());
    Version::parse(normalized).map_err(|error| {
        AppError::new(
            "update_version_invalid",
            format!("无法识别版本号 {value}: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_numerically() {
        let info = release_info_from_url(
            "0.1.3",
            "https://github.com/dream-rec/dreampaper/releases/tag/v0.1.10",
        )
        .unwrap();
        assert!(info.update_available);
        assert_eq!(info.latest_version, "0.1.10");
    }

    #[test]
    fn accepts_current_or_older_release() {
        let info = release_info_from_url(
            "0.1.3",
            "https://github.com/dream-rec/dreampaper/releases/tag/v0.1.2",
        )
        .unwrap();
        assert!(!info.update_available);
    }

    #[test]
    fn rejects_invalid_latest_release_redirect() {
        for url in [
            "https://github.com/dream-rec/dreampaper/releases/latest",
            "https://github.com/dream-rec/dreampaper/releases/tag/v0.1.4/notes",
            "https://github.com/dream-rec/dreampaper/releases/tag/v0.1.4?download=1",
            "https://github.com/dream-rec/dreampaper/releases-attacker/tag/v0.1.4",
            "https://example.com/releases/tag/v0.1.4",
        ] {
            let result = release_info_from_url("0.1.3", url);
            assert_eq!(result.unwrap_err().code, "update_url_invalid");
        }
    }
}
