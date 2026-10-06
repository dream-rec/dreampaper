from __future__ import annotations

import json
import os
from pathlib import Path

from .models import AppConfig, ModelProfile, PublicAppConfig, PublicModelProfile

DUCKDUCKGO_PROTOCOL = "duckduckgo"


def app_home() -> Path:
    return Path(os.getenv("DREAMPAPER_HOME", "~/.dreampaper")).expanduser()


DEFAULT_PROXY_URL = "http://127.0.0.1:7890"


def normalize_proxy_url(proxy_url: str | None) -> str | None:
    value = (proxy_url or "").strip()
    if not value:
        return None
    if "://" not in value:
        if value.isdigit():
            return f"http://127.0.0.1:{value}"
        return f"http://{value}"
    return value


def default_search_profile() -> ModelProfile:
    return ModelProfile(
        id="search-default",
        role="search",
        name="Search model",
        protocol=DUCKDUCKGO_PROTOCOL,
        base_url="",
        model="",
        timeout_seconds=120,
        max_retries=1,
        output_defaults={"max_results": "3"},
    )


def search_needs_key(protocol: str) -> bool:
    """该协议要不要密钥：只有免密钥的 duckduckgo 不要，tavily 要。"""
    return protocol != DUCKDUCKGO_PROTOCOL


def search_uses_model(protocol: str) -> bool:
    """该协议要不要填模型：只有对话式联网搜索用得到，其余协议填了也不会读。"""
    return protocol not in {DUCKDUCKGO_PROTOCOL, "tavily"}


def search_url_is_own(protocol: str, base_url: str) -> bool:
    """地址是不是该协议自己的服务；空表示用协议默认地址，也算自己的。"""
    url = (base_url or "").strip().lower()
    if not url:
        return True
    if protocol == DUCKDUCKGO_PROTOCOL:
        return "duckduckgo" in url
    if protocol == "tavily":
        return "tavily" in url
    return True


def default_config() -> AppConfig:
    return AppConfig(
        proxy_url=DEFAULT_PROXY_URL,
        active_search_profile="search-default",
        model_profiles=[
            ModelProfile(
                id="design-default",
                role="design",
                name="Design model",
                protocol="openai_responses",
                base_url="https://api.openai.com",
                model="gpt-5.4",
                timeout_seconds=120,
                max_retries=2,
            ),
            ModelProfile(
                id="implement-default",
                role="implement",
                name="Implement model",
                protocol="image2",
                base_url="https://api.openai.com",
                model="gpt-image-2",
                timeout_seconds=600,
                max_retries=3,
                output_defaults={
                    "size": "1200x675",
                    "quality": "auto",
                    "output_format": "png",
                    "response_format": "url",
                    "aspect_ratio": "16:9",
                    "image_size": "4K",
                    "thinking_level": "high",
                    "mime_type": "image/png",
                },
            ),
            default_search_profile(),
        ]
    )


class ConfigStore:
    def __init__(self, path: Path | None = None) -> None:
        self.path = path or app_home() / "config.json"

    def load(self) -> AppConfig:
        if not self.path.exists():
            return default_config()
        data = json.loads(self.path.read_text(encoding="utf-8"))
        config = AppConfig.model_validate(data)
        return self._normalize_duckduckgo_proxy(self._ensure_search_profile(config))

    def save(self, incoming: AppConfig) -> AppConfig:
        existing = self.load()
        keys_by_id = {profile.id: profile.api_key for profile in existing.model_profiles}
        profiles: list[ModelProfile] = []
        for profile in incoming.model_profiles:
            normalized_protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
            next_profile = profile.model_copy(update={"protocol": normalized_protocol})
            if not next_profile.api_key:
                next_profile = next_profile.model_copy(update={"api_key": keys_by_id.get(profile.id)})
            profiles.append(next_profile)
        saved = incoming.model_copy(
            update={
                "model_profiles": profiles,
                "proxy_url": normalize_proxy_url(incoming.proxy_url),
                "active_search_profile": incoming.active_search_profile or "search-default",
            }
        )
        saved = self._normalize_duckduckgo_proxy(self._ensure_search_profile(saved))
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(saved.model_dump_json(indent=2), encoding="utf-8")
        try:
            self.path.chmod(0o600)
        except OSError:
            pass
        return saved

    def public(self) -> PublicAppConfig:
        config = self.load()
        return PublicAppConfig(
            version=config.version,
            active_design_profile=config.active_design_profile,
            active_implement_profile=config.active_implement_profile,
            active_search_profile=config.active_search_profile,
            proxy_url=normalize_proxy_url(config.proxy_url),
            ppt_page_plan_concurrency=config.ppt_page_plan_concurrency,
            ppt_image_concurrency=config.ppt_image_concurrency,
            model_profiles=[self._public_profile(profile) for profile in config.model_profiles],
        )

    def proxy_url(self) -> str | None:
        return normalize_proxy_url(self.load().proxy_url)

    def active_profile(self, role: str) -> ModelProfile:
        config = self.load()
        if role == "design":
            active_id = config.active_design_profile
        elif role == "implement":
            active_id = config.active_implement_profile
        elif role == "search":
            active_id = config.active_search_profile
        else:
            raise ValueError(f"Unknown model role: {role}")
        for profile in config.model_profiles:
            if profile.id == active_id and profile.role == role:
                return profile
        for profile in config.model_profiles:
            if profile.role == role:
                return profile
        if role == "search":
            return default_search_profile()
        raise ValueError(f"Missing active {role} model profile")

    @staticmethod
    def _ensure_search_profile(config: AppConfig) -> AppConfig:
        # 搜索角色只剩一个联网协议：旧的 openai_chat 搜索配置并入 grok_search
        # （grok_search 就是 chat completions + 要求上游模型调用联网工具）。
        # 读取与保存都经过这里，Web 端的配置不会停留在已下线的选项上。
        profiles = [
            profile.model_copy(
                update={
                    "protocol": "grok_search"
                    if profile.protocol == "openai_chat"
                    else DUCKDUCKGO_PROTOCOL
                    if profile.protocol == "duckduckgo_html"
                    else profile.protocol
                }
            )
            if profile.role == "search"
            and profile.protocol in {"openai_chat", "duckduckgo_html"}
            else profile
            for profile in config.model_profiles
        ]
        if profiles != list(config.model_profiles):
            config = config.model_copy(update={"model_profiles": profiles})
        config = ConfigStore._repair_search_profiles(config)
        profiles = list(config.model_profiles)
        if not any(profile.role == "search" for profile in profiles):
            profiles.append(default_search_profile())
            return config.model_copy(
                update={
                    "model_profiles": profiles,
                    "active_search_profile": config.active_search_profile or "search-default",
                }
            )
        if not config.active_search_profile:
            search_id = next(profile.id for profile in profiles if profile.role == "search")
            return config.model_copy(update={"active_search_profile": search_id})
        return config

    @staticmethod
    def _repair_search_profiles(config: AppConfig) -> AppConfig:
        """把串味的搜索档案还给真正对应的协议。

        搜索角色以前只有一个档案，切换协议会把上一个协议的连接信息留在原地，
        于是免模型的协议里带着别人的地址与模型。读起来像对话式联网搜索的档案
        整份归到 grok_search，并给原协议补一个干净的默认档案；只是多填了模型
        的则把模型清掉，因为那个字段对该协议没有意义。
        """
        profiles = list(config.model_profiles)
        missing: list[str] = []
        for index, profile in enumerate(profiles):
            if (
                profile.role != "search"
                or search_uses_model(profile.protocol)
                or not (profile.model or "").strip()
            ):
                continue
            if search_url_is_own(profile.protocol, profile.base_url):
                profiles[index] = profile.model_copy(update={"model": ""})
                continue
            missing.append(profile.protocol)
            profiles[index] = profile.model_copy(update={"protocol": "grok_search"})
        # 端点由代码固定的协议不给用户留一个看不见的地址字段：
        # 界面不显示它，留着就变成改不到的隐藏配置。
        for index, profile in enumerate(profiles):
            if profile.role == "search" and profile.protocol == DUCKDUCKGO_PROTOCOL:
                profiles[index] = profile.model_copy(update={"base_url": ""})
        for protocol in missing:
            if any(item.role == "search" and item.protocol == protocol for item in profiles):
                continue
            used = {item.id for item in profiles}
            profile_id = f"search-{protocol}"
            suffix = 2
            while profile_id in used:
                profile_id = f"search-{protocol}-{suffix}"
                suffix += 1
            profiles.append(
                default_search_profile().model_copy(
                    update={
                        "id": profile_id,
                        "name": f"Search model ({protocol})",
                        "protocol": protocol,
                    }
                )
            )
        return config.model_copy(update={"model_profiles": profiles})

    @staticmethod
    def _normalize_duckduckgo_proxy(config: AppConfig) -> AppConfig:
        profiles: list[ModelProfile] = []
        changed = False
        for profile in config.model_profiles:
            proxy = (
                normalize_proxy_url(profile.proxy_url)
                if profile.role == "search" and profile.protocol == DUCKDUCKGO_PROTOCOL
                else None
            )
            if proxy != profile.proxy_url:
                changed = True
                profile = profile.model_copy(update={"proxy_url": proxy})
            profiles.append(profile)
        if not changed:
            return config
        return config.model_copy(update={"model_profiles": profiles})

    @staticmethod
    def _public_profile(profile: ModelProfile) -> PublicModelProfile:
        hint = None
        if profile.api_key:
            hint = f"••••{profile.api_key[-4:]}" if len(profile.api_key) >= 4 else "••••"
        return PublicModelProfile(
            id=profile.id,
            role=profile.role,
            name=profile.name,
            protocol=profile.protocol,
            base_url=profile.base_url,
            model=profile.model,
            api_version=profile.api_version,
            headers=profile.headers,
            timeout_seconds=profile.timeout_seconds,
            max_retries=profile.max_retries,
            output_defaults=profile.output_defaults,
            has_api_key=bool(profile.api_key),
            api_key_hint=hint,
            proxy_url=profile.proxy_url,
        )

