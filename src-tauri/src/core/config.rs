use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

use super::store::Store;

const CONFIG_KEY: &str = "app_config";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppConfig {
    pub version: i64,
    pub active_design_profile: String,
    pub active_implement_profile: String,
    #[serde(default = "default_search_profile_id")]
    pub active_search_profile: String,
    pub proxy_url: Option<String>,
    pub ppt_page_plan_concurrency: Option<i64>,
    pub ppt_image_concurrency: Option<i64>,
    pub model_profiles: Vec<ModelProfile>,
}

fn default_search_profile_id() -> String {
    "search-default".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelProfile {
    pub id: String,
    pub role: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_version: Option<String>,
    pub headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub timeout_seconds: i64,
    pub max_retries: i64,
    pub output_defaults: serde_json::Map<String, serde_json::Value>,
    pub has_api_key: Option<bool>,
    pub api_key_hint: Option<String>,
}

impl ModelProfile {
    pub fn request_timeout(&self) -> u64 {
        if self.timeout_seconds > 0 {
            self.timeout_seconds as u64
        } else {
            default_timeout_seconds(&self.role) as u64
        }
    }
}

fn default_timeout_seconds(role: &str) -> i64 {
    match role {
        "implement" => 600,
        // search 与 design 同级：grok_search 让上游模型自己联网检索，
        // 实测一次请求就要几十秒，旧的 15 秒默认值必然超时。
        _ => 120,
    }
}

pub struct ConfigService<'a> {
    store: &'a Store,
}

impl<'a> ConfigService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn get_config(&self) -> AppResult<AppConfig> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT value FROM config WHERE key = ?1")?;
        let result: Result<String, rusqlite::Error> =
            stmt.query_row(params![CONFIG_KEY], |row| row.get(0));
        match result {
            Ok(value) => {
                let mut config: AppConfig = serde_json::from_str(&value)?;
                ensure_search_profile(&mut config);
                Ok(public_config(config))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let config = default_config();
                self.persist_config(&config)?;
                Ok(public_config(config))
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn save_config(&self, config: AppConfig) -> AppResult<AppConfig> {
        let existing = self.load_private_config()?;
        let normalized = normalize_config(config, Some(existing));
        self.persist_config(&normalized)?;
        Ok(public_config(normalized))
    }

    pub fn runtime_config(&self) -> AppResult<AppConfig> {
        let mut config = self.load_private_config()?;
        ensure_search_profile(&mut config);
        config.proxy_url = normalize_proxy_url(config.proxy_url);
        for profile in &mut config.model_profiles {
            profile.timeout_seconds = profile.request_timeout() as i64;
        }
        Ok(config)
    }

    pub fn active_profile<'c>(config: &'c AppConfig, role: &str) -> AppResult<&'c ModelProfile> {
        let active_id = match role {
            "design" => &config.active_design_profile,
            "implement" => &config.active_implement_profile,
            "search" => &config.active_search_profile,
            other => {
                return Err(AppError::new(
                    "invalid_role",
                    format!("Unknown model role: {other}"),
                ))
            }
        };
        config
            .model_profiles
            .iter()
            .find(|profile| &profile.id == active_id && profile.role == role)
            .or_else(|| {
                config
                    .model_profiles
                    .iter()
                    .find(|profile| profile.role == role)
            })
            .ok_or_else(|| {
                AppError::new(
                    "missing_profile",
                    format!("Missing active {role} model profile"),
                )
            })
    }

    fn load_private_config(&self) -> AppResult<AppConfig> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT value FROM config WHERE key = ?1")?;
        let result: Result<String, rusqlite::Error> =
            stmt.query_row(params![CONFIG_KEY], |row| row.get(0));
        match result {
            Ok(value) => Ok(serde_json::from_str(&value)?),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(default_config()),
            Err(error) => Err(error.into()),
        }
    }

    fn persist_config(&self, config: &AppConfig) -> AppResult<()> {
        let conn = self.store.connection()?;
        let value = serde_json::to_string_pretty(config)?;
        conn.execute(
            "INSERT INTO config(key, value, updated_at) VALUES (?1, ?2, ?3)\
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![CONFIG_KEY, value, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}

fn normalize_config(mut incoming: AppConfig, existing: Option<AppConfig>) -> AppConfig {
    incoming.proxy_url = normalize_proxy_url(incoming.proxy_url);
    incoming.ppt_page_plan_concurrency = normalize_concurrency(incoming.ppt_page_plan_concurrency);
    incoming.ppt_image_concurrency = normalize_concurrency(incoming.ppt_image_concurrency);
    for profile in &mut incoming.model_profiles {
        profile.timeout_seconds = profile.request_timeout() as i64;
        if profile.protocol == "banna2" {
            profile.protocol = "banana2".to_string();
        }
        if profile
            .api_key
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            profile.api_key = existing
                .as_ref()
                .and_then(|config| {
                    config
                        .model_profiles
                        .iter()
                        .find(|item| item.id == profile.id)
                })
                .and_then(|profile| profile.api_key.clone());
        }
        profile.has_api_key = None;
        profile.api_key_hint = None;
    }
    ensure_search_profile(&mut incoming);
    incoming
}

fn ensure_search_profile(config: &mut AppConfig) {
    // 搜索角色只剩一个联网协议：旧的 openai_chat 搜索配置并入 grok_search
    // （grok_search 就是 chat completions + 要求上游模型调用联网工具）。
    // 读取、运行时与保存三条路径都经过这里，配置不会停留在已下线的选项上。
    for profile in &mut config.model_profiles {
        if profile.role != "search" {
            continue;
        }
        if profile.protocol == "openai_chat" {
            profile.protocol = SEARCH_MODEL_PROTOCOL.to_string();
        }
        // 旧名字换成新名字：duckduckgo_html -> duckduckgo。
        if profile.protocol == LEGACY_DUCKDUCKGO_PROTOCOL {
            profile.protocol = DUCKDUCKGO_PROTOCOL.to_string();
        }
    }
    repair_search_profiles(config);
    if !config
        .model_profiles
        .iter()
        .any(|item| item.role == "search")
    {
        config.model_profiles.push(default_search());
    }
    if config.active_search_profile.trim().is_empty() {
        config.active_search_profile = config
            .model_profiles
            .iter()
            .find(|item| item.role == "search")
            .map(|item| item.id.clone())
            .unwrap_or_else(default_search_profile_id);
    }
}

/// 带模型的那个搜索协议：需要 URL、模型与密钥三样齐全。
const SEARCH_MODEL_PROTOCOL: &str = "grok_search";

/// 免密钥、也免模型的那个协议：端点固定，URL 字段不显示。
const DUCKDUCKGO_PROTOCOL: &str = "duckduckgo";

/// 旧配置里的名字。
const LEGACY_DUCKDUCKGO_PROTOCOL: &str = "duckduckgo_html";

/// 该协议要不要填模型：只有对话式联网搜索用得到，其余协议填了也不会读。
fn search_uses_model(protocol: &str) -> bool {
    !matches!(protocol, DUCKDUCKGO_PROTOCOL | "tavily")
}

/// 地址是不是该协议自己的服务。空表示用协议默认地址，也算自己的。
fn search_url_is_own(protocol: &str, base_url: &str) -> bool {
    let url = base_url.trim().to_ascii_lowercase();
    if url.is_empty() {
        return true;
    }
    match protocol {
        DUCKDUCKGO_PROTOCOL => url.contains("duckduckgo"),
        "tavily" => url.contains("tavily"),
        _ => true,
    }
}

/// 把串味的搜索档案还给真正对应的协议。
///
/// 搜索角色以前只有一个档案，切换协议会把上一个协议的连接信息留在原地，于是
/// 免模型的协议里带着别人的地址与模型：`duckduckgo` 会拿着这个地址去抓别人
/// 的站点，`tavily` 会把别的厂商的密钥发给自己的接口。读起来像对话式联网搜索的
/// 档案整份归到 grok_search（地址/模型/密钥都跟着走），并给原协议补一个干净的
/// 默认档案；只是多填了模型的则把模型清掉，因为那个字段对该协议没有意义。
fn repair_search_profiles(config: &mut AppConfig) {
    let mut missing: Vec<String> = Vec::new();
    for profile in &mut config.model_profiles {
        if profile.role != "search"
            || search_uses_model(&profile.protocol)
            || profile.model.trim().is_empty()
        {
            continue;
        }
        if search_url_is_own(&profile.protocol, &profile.base_url) {
            profile.model.clear();
            continue;
        }
        missing.push(profile.protocol.clone());
        profile.protocol = SEARCH_MODEL_PROTOCOL.to_string();
    }
    // 端点由代码固定的协议不给用户留一个看不见的地址字段：
    // 界面不显示它，留着就变成改不到的隐藏配置。
    for profile in &mut config.model_profiles {
        if profile.role == "search" && profile.protocol == DUCKDUCKGO_PROTOCOL {
            profile.base_url.clear();
        }
    }
    for protocol in missing {
        if config
            .model_profiles
            .iter()
            .any(|item| item.role == "search" && item.protocol == protocol)
        {
            continue;
        }
        let mut profile = default_search();
        profile.id = free_search_profile_id(&config.model_profiles, &protocol);
        profile.name = format!("Search model ({protocol})");
        profile.protocol = protocol;
        config.model_profiles.push(profile);
    }
}

fn free_search_profile_id(profiles: &[ModelProfile], protocol: &str) -> String {
    let base = format!("search-{protocol}");
    if !profiles.iter().any(|item| item.id == base) {
        return base;
    }
    (2..)
        .map(|index| format!("{base}-{index}"))
        .find(|id| !profiles.iter().any(|item| &item.id == id))
        .expect("an unused search profile id")
}

fn normalize_proxy_url(proxy_url: Option<String>) -> Option<String> {
    let value = proxy_url.unwrap_or_default().trim().to_string();
    if value.is_empty() {
        return None;
    }
    if value.contains("://") {
        Some(value)
    } else if value.chars().all(|ch| ch.is_ascii_digit()) {
        Some(format!("http://127.0.0.1:{value}"))
    } else {
        Some(format!("http://{value}"))
    }
}

fn normalize_concurrency(value: Option<i64>) -> Option<i64> {
    value.map(|number| number.clamp(1, 20))
}

fn public_config(mut config: AppConfig) -> AppConfig {
    for profile in &mut config.model_profiles {
        profile.timeout_seconds = profile.request_timeout() as i64;
        profile.api_key_hint = profile.api_key.as_deref().map(mask_key);
        profile.has_api_key = Some(
            profile
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
        );
        profile.api_key = None;
    }
    config
}

fn mask_key(value: &str) -> String {
    if value.len() >= 4 {
        format!("••••{}", &value[value.len() - 4..])
    } else {
        "••••".to_string()
    }
}

fn default_config() -> AppConfig {
    AppConfig {
        version: 1,
        active_design_profile: "design-default".to_string(),
        active_implement_profile: "implement-default".to_string(),
        active_search_profile: default_search_profile_id(),
        proxy_url: None,
        ppt_page_plan_concurrency: None,
        ppt_image_concurrency: None,
        model_profiles: vec![default_design(), default_implement(), default_search()],
    }
}

fn default_search() -> ModelProfile {
    let mut output_defaults = serde_json::Map::new();
    output_defaults.insert(
        "max_results".to_string(),
        serde_json::Value::String("3".to_string()),
    );
    ModelProfile {
        id: "search-default".to_string(),
        role: "search".to_string(),
        name: "Search model".to_string(),
        protocol: DUCKDUCKGO_PROTOCOL.to_string(),
        base_url: String::new(),
        model: String::new(),
        api_key: None,
        api_version: None,
        headers: serde_json::Map::new(),
        timeout_seconds: default_timeout_seconds("search"),
        max_retries: 1,
        output_defaults,
        has_api_key: Some(false),
        api_key_hint: None,
    }
}

fn default_design() -> ModelProfile {
    ModelProfile {
        id: "design-default".to_string(),
        role: "design".to_string(),
        name: "Design model".to_string(),
        protocol: "openai_responses".to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: "gpt-5.4".to_string(),
        api_key: None,
        api_version: None,
        headers: serde_json::Map::new(),
        timeout_seconds: default_timeout_seconds("design"),
        max_retries: 2,
        output_defaults: serde_json::Map::new(),
        has_api_key: Some(false),
        api_key_hint: None,
    }
}

fn default_implement() -> ModelProfile {
    let mut output_defaults = serde_json::Map::new();
    for (key, value) in [
        ("size", "1200x675"),
        ("quality", "auto"),
        ("output_format", "png"),
        ("response_format", "url"),
        ("aspect_ratio", "16:9"),
        ("image_size", "4K"),
        ("thinking_level", "high"),
        ("mime_type", "image/png"),
    ] {
        output_defaults.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    ModelProfile {
        id: "implement-default".to_string(),
        role: "implement".to_string(),
        name: "Implement model".to_string(),
        protocol: "image2".to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: "gpt-image-2".to_string(),
        api_key: None,
        api_version: None,
        headers: serde_json::Map::new(),
        timeout_seconds: default_timeout_seconds("implement"),
        max_retries: 3,
        output_defaults,
        has_api_key: Some(false),
        api_key_hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_profiles_migrate_the_old_openai_chat_protocol() {
        let mut config = default_config();
        config.model_profiles.push(ModelProfile {
            role: "search".to_string(),
            protocol: "openai_chat".to_string(),
            ..default_search()
        });
        config.model_profiles.push(ModelProfile {
            role: "design".to_string(),
            protocol: "openai_chat".to_string(),
            ..default_design()
        });

        let normalized = normalize_config(config, None);

        assert!(normalized
            .model_profiles
            .iter()
            .any(|profile| { profile.role == "search" && profile.protocol == "grok_search" }));
        // 设计/制图角色的 openai_chat 不受影响。
        assert!(normalized
            .model_profiles
            .iter()
            .any(|profile| { profile.role == "design" && profile.protocol == "openai_chat" }));
    }

    /// 一个串味的搜索档案：免模型的协议里带着对话式联网搜索的地址、模型与密钥。
    fn polluted_search_profile() -> ModelProfile {
        ModelProfile {
            protocol: "duckduckgo".to_string(),
            base_url: "https://grok.draem.me".to_string(),
            model: "grok-build-0.1".to_string(),
            api_key: Some("xai-secret".to_string()),
            ..default_search()
        }
    }

    #[test]
    fn polluted_search_profiles_move_to_the_protocol_they_belong_to() {
        let mut config = default_config();
        config.model_profiles = vec![polluted_search_profile()];

        let normalized = normalize_config(config, None);

        let search: Vec<&ModelProfile> = normalized
            .model_profiles
            .iter()
            .filter(|item| item.role == "search")
            .collect();
        let moved = search
            .iter()
            .find(|item| item.protocol == "grok_search")
            .expect("the grok configuration moves over");
        assert_eq!(moved.base_url, "https://grok.draem.me");
        assert_eq!(moved.model, "grok-build-0.1");
        assert_eq!(moved.api_key.as_deref(), Some("xai-secret"));
        // 原协议补一个干净的默认档案，原来的档案 id 保持不变。
        let fresh = search
            .iter()
            .find(|item| item.protocol == "duckduckgo")
            .expect("the emptied protocol gets a fresh profile");
        assert!(fresh.base_url.is_empty());
        assert!(fresh.model.is_empty());
        assert!(fresh.api_key.is_none());
        assert_ne!(fresh.id, moved.id);
        // 重跑一次不再产生新档案。
        let again = normalize_config(normalized, None);
        assert_eq!(
            again
                .model_profiles
                .iter()
                .filter(|item| item.role == "search")
                .count(),
            2
        );
    }

    #[test]
    fn search_profiles_keep_their_own_endpoint_and_drop_unused_models() {
        let mut config = default_config();
        config.model_profiles = vec![
            // 自己的地址：只把模型清掉，地址与密钥原样留着。
            ModelProfile {
                protocol: "tavily".to_string(),
                base_url: "https://api.tavily.com".to_string(),
                model: "leftover".to_string(),
                api_key: Some("tvly-key".to_string()),
                ..default_search()
            },
            // 空地址表示用协议默认地址，同样只清模型。
            ModelProfile {
                id: "search-empty".to_string(),
                protocol: "duckduckgo".to_string(),
                base_url: String::new(),
                model: "leftover".to_string(),
                ..default_search()
            },
        ];

        let normalized = normalize_config(config, None);

        let tavily = normalized
            .model_profiles
            .iter()
            .find(|item| item.protocol == "tavily")
            .unwrap();
        assert_eq!(tavily.base_url, "https://api.tavily.com");
        assert_eq!(tavily.api_key.as_deref(), Some("tvly-key"));
        assert!(tavily.model.is_empty());
        let duck = normalized
            .model_profiles
            .iter()
            .find(|item| item.protocol == "duckduckgo")
            .unwrap();
        assert!(duck.base_url.is_empty());
        assert!(duck.model.is_empty());
        assert_eq!(normalized.model_profiles.len(), 2);
    }

    #[test]
    fn duckduckgo_profiles_drop_the_hidden_endpoint_and_the_old_name() {
        let mut config = default_config();
        config.model_profiles = vec![ModelProfile {
            protocol: "duckduckgo_html".to_string(),
            base_url: "https://duckduckgo.com".to_string(),
            ..default_search()
        }];

        let normalized = normalize_config(config, None);

        let duck = &normalized.model_profiles[0];
        assert_eq!(duck.protocol, "duckduckgo");
        // 端点由代码固定，界面不显示 URL，存量值也要清掉，免得藏着改不到。
        assert!(duck.base_url.is_empty());
    }
}
