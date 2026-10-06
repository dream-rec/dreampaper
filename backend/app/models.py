from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field, StrictBool


DesignProtocol = Literal["openai_chat", "openai_responses", "anthropic_messages"]
ImplementProtocol = Literal["image2", "banana2", "banna2"]
SearchProtocol = Literal["duckduckgo", "duckduckgo_html", "tavily", "openai_chat", "grok_search"]
# duckduckgo_html 是旧名字，留在字面量里只为让老配置还能被读进来；
# 载入时会改写成 duckduckgo（与 openai_chat -> grok_search 同一套处理）。
ModelRole = Literal["design", "implement", "search"]
JobMode = Literal["paper_figure", "ppt_slide"]


class ModelProfile(BaseModel):
    id: str
    role: ModelRole
    name: str
    protocol: DesignProtocol | ImplementProtocol | SearchProtocol
    base_url: str
    model: str
    api_key: str | None = None
    api_version: str | None = None
    headers: dict[str, str] = Field(default_factory=dict)
    timeout_seconds: int = 120
    max_retries: int = 2
    output_defaults: dict[str, Any] = Field(default_factory=dict)


class PublicModelProfile(BaseModel):
    id: str
    role: ModelRole
    name: str
    protocol: str
    base_url: str
    model: str
    api_version: str | None = None
    headers: dict[str, str] = Field(default_factory=dict)
    timeout_seconds: int
    max_retries: int
    output_defaults: dict[str, Any] = Field(default_factory=dict)
    has_api_key: bool = False
    api_key_hint: str | None = None


class AppConfig(BaseModel):
    version: int = 1
    active_design_profile: str = "design-default"
    active_implement_profile: str = "implement-default"
    active_search_profile: str = "search-default"
    proxy_url: str | None = "http://127.0.0.1:7890"
    ppt_page_plan_concurrency: int | None = Field(default=None, ge=1, le=20)
    ppt_image_concurrency: int | None = Field(default=None, ge=1, le=20)
    model_profiles: list[ModelProfile] = Field(default_factory=list)


class PublicAppConfig(BaseModel):
    version: int = 1
    active_design_profile: str
    active_implement_profile: str
    active_search_profile: str = "search-default"
    proxy_url: str | None = None
    ppt_page_plan_concurrency: int | None = None
    ppt_image_concurrency: int | None = None
    model_profiles: list[PublicModelProfile]


class TestModelRequest(BaseModel):
    profile: ModelProfile


class TemplateSummary(BaseModel):
    id: str
    source_id: str
    kind: Literal["diagram", "plot"]
    category: str | None = None
    rounded_ratio: str | None = None
    visual_intent: str
    content_summary: str
    image_url: str


class AssetUploadResponse(BaseModel):
    id: str
    filename: str
    mime_type: str
    url: str


class PaperFigurePayload(BaseModel):
    figure_title: str
    section_description: str
    template_ids: list[str] = Field(default_factory=list, max_length=3)
    aspect_ratio: str = "inherit"
    layout_fidelity: Literal["strict", "balanced", "loose"] = "balanced"
    style_strength: Literal["high", "medium", "low"] = "high"
    candidate_count: int = 1
    custom_prompt: str | None = None


class PptSlidePayload(BaseModel):
    template_asset_id: str
    material_text: str = ""
    material_asset_ids: list[str] = Field(default_factory=list, max_length=10)
    page_count: int = Field(default=1, ge=1, le=20)
    custom_prompt: str | None = None


class JobCreateRequest(BaseModel):
    mode: JobMode
    simple_mode: StrictBool = False
    payload: PaperFigurePayload | PptSlidePayload


class JobImage(BaseModel):
    name: str
    url: str


class JobEvent(BaseModel):
    stage: str
    message: str
    status: Literal["pending", "running", "succeeded", "failed"] = "running"
    timestamp: str


class JobError(BaseModel):
    summary: str
    code: str = "job_failed"
    stage: str | None = None
    role: ModelRole | None = None
    profile_id: str | None = None
    profile_name: str | None = None
    protocol: str | None = None
    model: str | None = None
    base_url: str | None = None
    endpoint: str | None = None
    http_status: int | None = None
    suggestion: str | None = None


class JobDesignLog(BaseModel):
    step: str
    label: str
    status: str
    content: str
    timestamp: str


class JobRecord(BaseModel):
    id: str
    mode: JobMode
    status: Literal["queued", "running", "succeeded", "failed"]
    message: str | None = None
    stage: str = "queued"
    created_at: str
    updated_at: str
    images: list[JobImage] = Field(default_factory=list)
    events: list[JobEvent] = Field(default_factory=list)
    design_logs: list[JobDesignLog] = Field(default_factory=list)
    payload: dict[str, Any] | None = None
    error: JobError | None = None
    internal_artifacts: dict[str, Any] = Field(default_factory=dict)

