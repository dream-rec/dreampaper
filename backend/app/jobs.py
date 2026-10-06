from __future__ import annotations

import asyncio
import base64
import json
import logging
import mimetypes
import re
import uuid
import zipfile
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from xml.etree import ElementTree

from .adapters import (
    DesignClient,
    ImplementClient,
    ModelAdapterError,
    image_to_b64,
    normalize_base_url,
    parse_json_response,
    public_profile_snapshot,
    response_error_summary,
    safe_error_message,
)
from .assets import AssetStore
from .config import ConfigStore, app_home
from .models import (
    JobCreateRequest,
    JobError,
    JobEvent,
    JobImage,
    JobRecord,
    ModelProfile,
    PaperFigurePayload,
    PptSlidePayload,
)
from .prompts import PromptStore, compose_prompt
from .search import SearchClient
from .templates import TemplateStore


IMAGE2_FALLBACK_OUTPUT = {
    "size": "1200x675",
    "quality": "auto",
    "output_format": "png",
    "response_format": "b64_json",
}
PPT_DESIGN_TIMEOUT_SECONDS = 300
MATERIAL_TEXT_LIMIT = 6000
PPT_PLAN_MATERIAL_LIMIT = 3200
PPT_PLAN_TEMPLATE_LIMIT = 2600
VISUAL_CONTEXT_LIMIT = 8192
JSON_CONTEXT_RETRY_ATTEMPTS = 1
JSON_RETRY_EXCERPT_LIMIT = 4000


FORBIDDEN_TEMPLATE_COPY_PHRASES = (
    "copy the template",
    "duplicate the template",
    "trace the template",
    "use the template as a base image",
    "edit the template image",
    "background edit",
)

FIGURE_ASPECT_RATIOS = {"inherit", "16:9", "4:3", "1:1", "3:2", "2:3", "9:16"}
FIGURE_COMMON_FIELDS = (
    "visual_type",
    "layout_constraints",
    "semantic_constraints",
    "visual_constraints",
    "forbidden_errors",
    "quality_rubric",
    "visible_text",
    "implement_prompt",
)
DIAGRAM_SPEC_FIELDS = ("modules", "entities", "connections", "flow_direction", "grouping_hierarchy", "arrow_routing", "label_strategy")
PLOT_SPEC_FIELDS = (
    "chart_type",
    "data_fields",
    "axes",
    "units",
    "series_or_categories",
    "legend",
    "statistical_annotations",
    "data_integrity_rules",
)
MASTER_STYLE_FIELDS = (
    "canvas",
    "title_region",
    "safe_margins",
    "header_footer",
    "divider_lines",
    "palette",
    "typography",
    "module_style",
    "decorative_elements",
    "immutable_elements",
    "page_layout_rules",
    "forbidden_deviations",
)
PAGE_MASTER_BINDING_FIELDS = (
    "title_region",
    "safe_margins",
    "header_footer",
    "divider_lines",
    "palette",
    "typography",
    "module_style",
    "background",
)
PAGE_VISUAL_PLAN_FIELDS = ("usage_decision", "elements", "text_visual_balance")
PAGE_VISUAL_ELEMENT_FIELDS = ("type", "subject", "appearance", "source_reference", "placement", "style", "size_ratio")
PAGE_EMPHASIS_PLAN_FIELDS = ("keywords", "style_rules")
PAGE_EMPHASIS_KEYWORD_FIELDS = ("text", "style", "reason")
# 表达分析字段（图与 PPT 单页共用）：只校验形状，语义去重由 design model 负责。
EXPRESSION_INFORMATION_UNIT_FIELDS = ("unit", "carrier")
EXPRESSION_HIERARCHY_PLAN_FIELDS = ("levels", "alignment", "focus_region")
VISUAL_ASSET_SEARCH_TERM_LIMIT = 8
VISUAL_ASSET_SEARCH_RESULT_LIMIT = 3
# 识别主体的提示词资源；哪些词值得检索由模型读资料自己判断，不再维护主体词库。
VISUAL_SUBJECTS_KEY = "global/visual_subjects.md"
API_OUTPUT_PROMPT_PATTERN = re.compile(
    r"\b(?:size|quality|output_format|response_format|aspect_ratio|image_size|thinking_level|mime_type)\s*=\s*[^,.;\n]+[,.;]?\s*",
    re.IGNORECASE,
)
OUTPUT_SETTINGS_SENTENCE_PATTERN = re.compile(r"output settings preserved exactly:\s*[^.\n]*(?:\.|\n)?", re.IGNORECASE)

class DesignSchemaError(ValueError):
    """设计 JSON 已可解析但缺少必需结构化字段时抛出。"""


def now_iso() -> str:
    return datetime.now(UTC).isoformat()


class JobManager:
    def __init__(self, templates: TemplateStore, assets: AssetStore, prompts: PromptStore, config: ConfigStore) -> None:
        self.templates = templates
        self.assets = assets
        self.prompts = prompts
        self.config = config
        self.jobs: dict[str, JobRecord] = {}
        self.root = app_home() / "jobs"
        self.design = DesignClient()
        self.implement = ImplementClient()
        self.search = SearchClient()

    def create(self, request: JobCreateRequest) -> JobRecord:
        job_id = uuid.uuid4().hex
        timestamp = now_iso()
        record = JobRecord(
            id=job_id,
            mode=request.mode,
            payload=request.model_dump(),
            status="queued",
            stage="queued",
            message="任务已排队",
            created_at=timestamp,
            updated_at=timestamp,
            events=[{"stage": "queued", "message": "任务已排队", "status": "running", "timestamp": timestamp}],
        )
        self.jobs[job_id] = record
        self._persist(record)
        return record

    def get(self, job_id: str) -> JobRecord:
        if job_id in self.jobs:
            record = self._normalize_record(self.jobs[job_id])
            self.jobs[job_id] = record
            return record
        path = self.root / job_id / "job.json"
        if path.exists():
            record = JobRecord.model_validate_json(path.read_text(encoding="utf-8"))
            record = self._normalize_record(record)
            self.jobs[job_id] = record
            self._persist(record)
            return record
        raise KeyError(job_id)

    async def run(self, job_id: str, request: JobCreateRequest) -> None:
        self._mark_stage(job_id, "started", "任务开始运行")
        try:
            if request.mode == "paper_figure":
                await self._run_paper(job_id, PaperFigurePayload.model_validate(request.payload), request.simple_mode)
            else:
                await self._run_ppt(job_id, PptSlidePayload.model_validate(request.payload), request.simple_mode)
        except Exception as exc:
            record = self.get(job_id)
            failed_stage = record.stage if record.stage not in {"queued", "started", "failed"} else None
            if isinstance(exc, ModelAdapterError):
                diagnostic = exc.diagnostic(failed_stage)
                error_message = diagnostic["summary"]
            else:
                error_message = safe_error_message(exc)
                diagnostic = {
                    "summary": error_message,
                    "code": "job_failed",
                    "stage": failed_stage,
                }
            diagnostic = self._enrich_error_diagnostic(record, diagnostic)
            artifacts = dict(record.internal_artifacts)
            artifacts["error"] = {**diagnostic, "message": error_message, "failed_at": now_iso()}
            self._mark_stage(
                job_id,
                failed_stage or "failed",
                error_message,
                status="failed",
                event_status="failed",
                error=JobError.model_validate(diagnostic),
                internal_artifacts=artifacts,
            )

    async def _run_paper(self, job_id: str, payload: PaperFigurePayload, simple_mode: bool = False) -> None:
        """科研图两阶段 design：①仅 template 结构规划 ②结构+用户内容 → implement_prompt。"""
        self._mark_stage(job_id, "paper_validate", "校验输入")
        if not payload.template_ids:
            raise ValueError("Paper figure requires at least one template")
        design_profile = self.config.active_profile("design")
        implement_profile = None if simple_mode else self.config.active_profile("implement")
        proxy_url = self.config.proxy_url()
        self._mark_stage(job_id, "paper_templates", "读取参考模板")
        selected = [self.templates.get(template_id) for template_id in payload.template_ids[:3]]
        selected_template_metadata = [self._template_metadata(item) for item in selected]
        template_images = [self._template_image_payload(item) for item in selected]
        template_summary = json.dumps(selected_template_metadata, ensure_ascii=False, indent=2)
        system_prompt = self.prompts.load("global/system.md")["content"]
        user_context = {
            "Figure title": payload.figure_title.strip(),
            "Section description": payload.section_description.strip(),
            "Aspect ratio": payload.aspect_ratio,
            "Layout fidelity": payload.layout_fidelity,
            "Style strength": payload.style_strength,
            "Candidate count": payload.candidate_count,
            "Custom prompt": payload.custom_prompt or "None",
        }
        network_context = {"proxy_url": proxy_url}
        # 视觉素材检索与幻灯片同源：先联网拿到产品/工具的客观外观描述，再作为
        # 纯文本线索注入制图方案；图片只进结构分析那一个调用。
        # 检索只吃标题与方法：约束（字体、底色一类）不是可检索的视觉主体。
        search_material = "\n".join(
            part
            for part in (
                payload.figure_title.strip(),
                payload.section_description.strip(),
            )
            if part
        )
        visual_asset_prompt = ""
        if self._optional_profile("search") is None:
            self._mark_stage(job_id, "paper_visual_assets", "未配置联网查询模型，跳过视觉素材检索")
        else:
            self._mark_stage(job_id, "paper_visual_assets", "联网查询产品与工具的视觉素材")
            subjects = await self._pick_visual_subjects(search_material, design_profile, proxy_url)
            visual_asset_context = await self._build_visual_asset_context(subjects, proxy_url, job_id=job_id)
            visual_asset_prompt = self._visual_asset_context_text(visual_asset_context)
            self._append_design_log(job_id, "paper_visual_assets", "联网查询 · 视觉证据", self._visual_asset_log_text(visual_asset_context))
            self._merge_artifacts(job_id, {"visual_asset_context": visual_asset_context})

        # —— Stage 1: 只看 template 图，抽取结构规划（不混入方法长文）——
        structure_assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("global/figure_style.md"),
            self.prompts.load("modes/paper_figure/structure.md"),
        ]
        self._mark_stage(job_id, "paper_structure_prompt", "准备母版结构分析")
        structure_prompt, structure_prompt_assets = compose_prompt(
            structure_assets,
            {
                "Selected Template Metadata": template_summary,
                "Task": (
                    "Analyze the attached template figure image(s) only. "
                    "Return a reusable structure_plan JSON. Do not invent the user's research content."
                ),
                "Output Contract": self._structure_plan_contract(),
            },
        )
        structure_request = self._design_request_summary(
            design_profile, structure_prompt_assets, structure_prompt, template_images
        )
        self._merge_artifacts(
            job_id,
            {
                "normalized_input": user_context,
                "network": network_context,
                "selected_templates": selected_template_metadata,
                "pipeline": "paper_two_stage_structure_then_content",
                "structure_model_request": structure_request,
            },
        )
        self._mark_stage(job_id, "paper_structure", "分析母版结构")
        structure_text = await self.design.generate(
            design_profile, system_prompt, structure_prompt, template_images, proxy_url=proxy_url
        )
        self._mark_stage(job_id, "paper_structure_parse", "校验结构规划")
        structure_json, structure_plan, structure_schema_retry = await self._parse_validate_or_fill_missing(
            design_profile,
            system_prompt,
            structure_prompt,
            structure_text,
            template_images,
            self._validate_structure_plan,
            timeout_seconds=None,
            proxy_url=proxy_url,
        )
        self._merge_artifacts(
            job_id,
            {
                "structure_model_response": {"raw_text": structure_text, "parsed_json": structure_json},
                "structure_plan": structure_plan,
                "retries": {"structure_schema_fill": structure_schema_retry},
            },
        )

        # —— Stage 2: 结构规划 + 用户内容 → 完整 design JSON（不再塞 template 图，避免内容被图面“带跑”）——
        design_assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("global/figure_style.md"),
            self.prompts.load("global/expression.md"),
            self.prompts.load("modes/paper_figure/design.md"),
            self.prompts.load("modes/paper_figure/diagram_rules.md"),
            self.prompts.load("modes/paper_figure/plot_rules.md"),
            self.prompts.load("modes/paper_figure/validator.md"),
        ]
        self._mark_stage(job_id, "paper_prompt", "准备内容填充")
        design_sections = {
            "Structure Plan From Templates": json.dumps(structure_plan, ensure_ascii=False, indent=2),
            "User Input": json.dumps(user_context, ensure_ascii=False, indent=2),
            "Selected Template Metadata": template_summary,
        }
        if visual_asset_prompt:
            design_sections["Visual Asset Search Context"] = visual_asset_prompt
        design_sections["Output Contract"] = self._paper_contract()
        design_user_prompt, design_prompt_assets = compose_prompt(design_assets, design_sections)
        design_request = self._design_request_summary(
            design_profile, design_prompt_assets, design_user_prompt, []
        )
        self._merge_artifacts(
            job_id,
            {
                "prompt_assets": design_prompt_assets,
                "design_model_request": design_request,
            },
        )
        self._mark_stage(job_id, "paper_design", "生成制图方案")
        design_text = await self.design.generate(
            design_profile, system_prompt, design_user_prompt, [], proxy_url=proxy_url
        )
        self._mark_stage(job_id, "paper_parse", "校验制图方案")

        def _validate_design_with_content(data: dict[str, Any]) -> None:
            self._validate_paper_design(data)
            self._validate_design_content_coverage(
                data,
                payload.figure_title.strip(),
                payload.section_description.strip(),
            )

        design_json, _, paper_schema_retry = await self._parse_validate_or_fill_missing(
            design_profile,
            system_prompt,
            design_user_prompt,
            design_text,
            [],
            _validate_design_with_content,
            timeout_seconds=None,
            proxy_url=proxy_url,
        )
        implement_prompt = self._paper_implement_prompt(design_json)
        self._merge_artifacts(
            job_id,
            {
                "design_model_response": {"raw_text": design_text, "parsed_json": design_json},
                "design_response": design_json,
                "implement_model_request": None if simple_mode else self._implement_request_summary(implement_profile, implement_prompt, {}, 0),
                "implement_prompts": [implement_prompt],
            },
        )
        if simple_mode:
            self._finish_prompts(job_id, [implement_prompt])
            return
        self._mark_stage(job_id, "paper_implement", "生成图片")
        image_b64 = await self.implement.generate(implement_profile, implement_prompt, proxy_url=proxy_url)
        self._mark_stage(job_id, "paper_save", "保存生成图片")
        image = self._save_image(job_id, "paper_figure.png", image_b64)
        self._mark_stage(job_id, "completed", "任务完成", status="succeeded", event_status="succeeded", images=[image])
        prev_retries = dict(self.get(job_id).internal_artifacts.get("retries") or {})
        prev_retries["paper_schema_fill"] = paper_schema_retry
        self._update(
            job_id,
            internal_artifacts={
                **self.get(job_id).internal_artifacts,
                "implement_model_response": self._image_response_summary(image, image_b64),
                "retries": prev_retries,
            },
        )

    async def _run_ppt(self, job_id: str, payload: PptSlidePayload, simple_mode: bool = False) -> None:
        self._mark_stage(job_id, "ppt_validate", "校验输入")
        app_config = self.config.load()
        design_profile = self.config.active_profile("design")
        implement_profile = None if simple_mode else self.config.active_profile("implement")
        page_plan_concurrency = self._resolve_ppt_concurrency(app_config.ppt_page_plan_concurrency, payload.page_count)
        image_concurrency = self._resolve_ppt_concurrency(app_config.ppt_image_concurrency, payload.page_count)
        proxy_url = self.config.proxy_url()
        ppt_design_timeout = max(design_profile.timeout_seconds, PPT_DESIGN_TIMEOUT_SECONDS)
        self._mark_stage(job_id, "ppt_template", "读取母版图片")
        template_path, template_mime = self.assets.get(payload.template_asset_id)
        template_image = {"filename": template_path.name, "mime_type": template_mime, "b64": image_to_b64(template_path)}
        self._mark_stage(job_id, "ppt_material", "整理资料输入")
        material_assets = [self._material_asset_summary(asset_id) for asset_id in payload.material_asset_ids]
        material_context = self._compose_material_context(payload.material_text, material_assets)
        if not material_context.strip():
            raise ValueError("PPT slide requires material text or material files")
        self._mark_stage(job_id, "ppt_visual_assets", "联网查询产品与工具的视觉素材")
        # 检索只吃资料与附件：约束（字体、底色一类）不是可检索的视觉主体。
        subjects = await self._pick_visual_subjects(material_context, design_profile, proxy_url)
        visual_asset_context = await self._build_visual_asset_context(subjects, proxy_url, job_id=job_id)
        visual_asset_prompt = self._visual_asset_context_text(visual_asset_context)
        self._append_design_log(job_id, "ppt_visual_assets", "联网查询 · 视觉证据", self._visual_asset_log_text(visual_asset_context))
        ppt_output = {} if simple_mode else self._ppt_output_defaults(implement_profile)
        normalized_input = {
            "template_asset_id": payload.template_asset_id,
            "material_text": payload.material_text.strip(),
            "material_asset_ids": payload.material_asset_ids,
            "material_assets": material_assets,
            "page_count": payload.page_count,
            "custom_prompt": payload.custom_prompt or "None",
            "output": ppt_output,
            "design_timeout_seconds": ppt_design_timeout,
            "visual_asset_search": {
                "enabled": True,
                "terms": visual_asset_context.get("terms", []),
                "degraded": visual_asset_context.get("degraded", False),
            },
        }
        network_context = {
            "proxy_url": proxy_url,
            "visual_asset_search": {
                "enabled": True,
                "term_limit": VISUAL_ASSET_SEARCH_TERM_LIMIT,
                "result_limit": VISUAL_ASSET_SEARCH_RESULT_LIMIT,
            },
        }
        analyzer_assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("modes/ppt_slide/analyzer.md"),
            self.prompts.load("modes/ppt_slide/master_rules.md"),
        ]
        analyzer_prompt, analyzer_prompt_assets = compose_prompt(
            analyzer_assets,
            {"Output Contract": self._template_analysis_contract()},
        )
        self._merge_artifacts(
            job_id,
            {
                "normalized_input": normalized_input,
                "network": network_context,
                "prompt_assets": analyzer_prompt_assets,
                "template_asset_id": payload.template_asset_id,
                "design_model_request": self._design_request_summary(
                    design_profile,
                    analyzer_prompt_assets,
                    analyzer_prompt,
                    [template_image],
                    timeout_seconds=ppt_design_timeout,
                ),
            },
        )
        self._mark_stage(job_id, "ppt_analyze", "分析母版版式")
        analysis_text = await self.design.generate(
            design_profile,
            analyzer_assets[0]["content"],
            analyzer_prompt,
            [template_image],
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        self._mark_stage(job_id, "ppt_parse_template", "校验母版分析结果")
        template_analysis, _, template_schema_retry = await self._parse_validate_or_fill_missing(
            design_profile,
            analyzer_assets[0]["content"],
            analyzer_prompt,
            analysis_text,
            [template_image],
            self._validate_template_analysis,
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        page_assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("modes/ppt_slide/design.md"),
            self.prompts.load("global/expression.md"),
            self.prompts.load("styles/academic_ppt.md"),
        ]
        compact_template_analysis = self._compact_template_analysis(template_analysis)
        compact_material_context = self._truncate_text(material_context, PPT_PLAN_MATERIAL_LIMIT)
        compact_visual_asset_prompt = visual_asset_prompt
        self._mark_stage(job_id, "ppt_outline_prompt", "准备整套大纲规划")
        outline_prompt, outline_prompt_assets = compose_prompt(
            page_assets,
            {
                "Task Mode": "Deck outline mode. Plan only the deck narrative and lightweight page briefs. Do not write page-level implement_prompt.",
                "Template Analysis": compact_template_analysis,
                "Material": compact_material_context,
                "Visual Asset Search Context": compact_visual_asset_prompt,
                "Page Count": payload.page_count,
                "Custom Prompt": payload.custom_prompt or "None",
                "Output Contract": self._ppt_outline_contract(payload.page_count),
            },
        )
        all_prompt_assets = analyzer_prompt_assets + outline_prompt_assets
        self._merge_artifacts(
            job_id,
            {
                "template_analysis": template_analysis,
                "visual_asset_context": visual_asset_context,
                "page_planner_compaction": {
                    "template_chars": len(compact_template_analysis),
                    "material_chars": len(compact_material_context),
                    "visual_asset_chars": len(compact_visual_asset_prompt),
                    "outline_prompt_chars": len(outline_prompt),
                    "page_plan_concurrency": page_plan_concurrency,
                    "image_concurrency": image_concurrency,
                    "removed_redundant_master_rules_asset": True,
                },
                "prompt_assets": all_prompt_assets,
                "design_model_response": {"template_analysis_raw_text": analysis_text, "template_analysis_json": template_analysis},
                "outline_design_model_request": self._design_request_summary(
                    design_profile,
                    outline_prompt_assets,
                    outline_prompt,
                    [],
                    timeout_seconds=ppt_design_timeout,
                ),
            },
        )
        self._mark_stage(job_id, "ppt_outline", "规划整套大纲")
        outline_text = await self.design.generate(
            design_profile,
            page_assets[0]["content"],
            outline_prompt,
            [],
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        self._mark_stage(job_id, "ppt_parse_outline", "校验整套大纲")
        outline_json, deck_outline, outline_schema_retry = await self._parse_validate_or_fill_missing(
            design_profile,
            page_assets[0]["content"],
            outline_prompt,
            outline_text,
            [],
            lambda parsed: self._validate_ppt_outline(parsed, payload.page_count),
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        page_briefs = deck_outline["page_briefs"]
        self._mark_stage(job_id, "ppt_page_plan_queue", f"排队规划 {payload.page_count} 页")
        page_results = await self._run_in_ordered_batches(
            page_briefs,
            page_plan_concurrency,
            lambda brief: self._plan_single_ppt_page(
                job_id=job_id,
                profile=design_profile,
                system_prompt=page_assets[0]["content"],
                page_assets=page_assets,
                template_analysis=template_analysis,
                compact_template_analysis=compact_template_analysis,
                compact_material_context=compact_material_context,
                compact_visual_asset_prompt=compact_visual_asset_prompt,
                deck_outline=deck_outline,
                page_brief=brief,
                custom_prompt=payload.custom_prompt or "None",
                timeout_seconds=ppt_design_timeout,
                proxy_url=proxy_url,
            ),
        )
        pages_json = {"pages": [result["page"] for result in page_results]}
        self._mark_stage(job_id, "ppt_merge_pages", "汇总页面规划")
        pages = self._validate_ppt_pages(pages_json, payload.page_count, template_analysis)
        pages = self._apply_ppt_master_prompt_prefix(pages, template_analysis)
        if simple_mode:
            self._merge_artifacts(job_id, {
                "page_plan": pages,
                "design_response": {"template_analysis": template_analysis, "deck_outline": deck_outline, "pages": pages},
                "implement_prompts": [page["implement_prompt"] for page in pages],
            })
            self._finish_prompts(job_id, [page["implement_prompt"] for page in pages])
            return
        self._mark_stage(job_id, "ppt_implement_queue", "排队生成图片")
        implement_results = await self._run_in_ordered_batches(
            pages,
            image_concurrency,
            lambda page: self._implement_single_ppt_page(job_id, implement_profile, page, ppt_output, proxy_url),
        )
        images = [result["image"] for result in implement_results]
        implement_prompts = [result["prompt"] for result in implement_results]
        implement_requests = [result["request"] for result in implement_results]
        implement_responses = [result["response"] for result in implement_results]
        self._mark_stage(job_id, "completed", "任务完成", status="succeeded", event_status="succeeded", images=images)
        self._update(
            job_id,
            internal_artifacts={
                **self.get(job_id).internal_artifacts,
                "outline_design_model_response": {"raw_text": outline_text, "parsed_json": outline_json, "deck_outline": deck_outline},
                "page_design_model_response": {
                    "parsed_json": pages_json,
                    "workers": [
                        {
                            "page": result["page"]["page"],
                            "raw_text": result["raw_text"],
                            "parsed_json": result["parsed_json"],
                            "request": result["request"],
                        }
                        for result in page_results
                    ],
                },
                "page_plan": pages,
                "design_response": {"template_analysis": template_analysis, "deck_outline": deck_outline, "pages": pages},
                "implement_model_request": implement_requests,
                "implement_model_response": implement_responses,
                "implement_prompts": implement_prompts,
                "retries": {
                    "design_json_repair": "attempted only on parse failure",
                    "template_schema_fill": template_schema_retry,
                    "outline_schema_fill": outline_schema_retry,
                    "page_schema_fill": {str(result["page"]["page"]): result["retry"] for result in page_results},
                },
            },
        )

    @staticmethod
    def _resolve_ppt_concurrency(configured: int | None, page_count: int) -> int:
        default_concurrency = max(1, page_count)
        requested = configured if configured is not None else default_concurrency
        return min(max(1, requested), default_concurrency)

    async def _run_in_ordered_batches(self, items: list[Any], concurrency: int, worker: Callable[[Any], Any]) -> list[Any]:
        results: list[Any] = []
        limit = max(1, concurrency)
        for start in range(0, len(items), limit):
            batch = items[start : start + limit]
            results.extend(await asyncio.gather(*(worker(item) for item in batch)))
        return results

    async def _plan_single_ppt_page(
        self,
        *,
        job_id: str,
        profile,
        system_prompt: str,
        page_assets: list[dict[str, str]],
        template_analysis: dict[str, Any],
        compact_template_analysis: str,
        compact_material_context: str,
        compact_visual_asset_prompt: str,
        deck_outline: dict[str, Any],
        page_brief: dict[str, Any],
        custom_prompt: str,
        timeout_seconds: int,
        proxy_url: str | None,
    ) -> dict[str, Any]:
        page_number = int(page_brief.get("page") or 0)
        self._mark_stage(job_id, f"ppt_page_prompt_{page_number}", f"准备第 {page_number} 页规划")
        page_prompt, page_prompt_assets = compose_prompt(
            page_assets,
            {
                "Task Mode": "Single-page worker mode. Return exactly one page object only. Do not plan or output other pages.",
                "Template Analysis": compact_template_analysis,
                "Deck Outline": json.dumps(deck_outline, ensure_ascii=False, separators=(",", ":")),
                "Current Page Brief": json.dumps(page_brief, ensure_ascii=False, indent=2),
                "Adjacent Page Context": json.dumps(self._adjacent_page_context(deck_outline, page_number), ensure_ascii=False, indent=2),
                "Material": compact_material_context,
                "Visual Asset Search Context": compact_visual_asset_prompt,
                "Custom Prompt": custom_prompt,
                "Output Contract": self._ppt_single_page_contract(page_number),
            },
        )
        request = self._design_request_summary(profile, page_prompt_assets, page_prompt, [], timeout_seconds=timeout_seconds)
        self._mark_stage(job_id, f"ppt_page_plan_{page_number}", f"规划第 {page_number} 页内容")
        page_text = await self.design.generate(
            profile,
            system_prompt,
            page_prompt,
            [],
            timeout_seconds=timeout_seconds,
            proxy_url=proxy_url,
        )
        page_json, page, retry = await self._parse_validate_or_fill_missing(
            profile,
            system_prompt,
            page_prompt,
            page_text,
            [],
            lambda parsed: self._validate_ppt_single_page(parsed, page_number, template_analysis),
            timeout_seconds=timeout_seconds,
            proxy_url=proxy_url,
        )
        return {"page": page, "raw_text": page_text, "parsed_json": page_json, "retry": retry, "request": request}

    async def _implement_single_ppt_page(
        self,
        job_id: str,
        profile,
        page: dict[str, Any],
        ppt_output: dict[str, Any],
        proxy_url: str | None,
    ) -> dict[str, Any]:
        page_number = int(page["page"])
        prompt = page["implement_prompt"]
        self._mark_stage(job_id, f"ppt_implement_{page_number}", f"生成第 {page_number} 页图片")
        image_b64 = await self.implement.generate(profile, prompt, output_overrides=ppt_output, proxy_url=proxy_url)
        image = self._save_image(job_id, f"slide_{page_number}.png", image_b64)
        return {
            "image": image,
            "prompt": prompt,
            "request": self._implement_request_summary(profile, prompt, ppt_output, 0, page=page_number),
            "response": self._image_response_summary(image, image_b64, page=page_number),
        }

    async def _parse_or_repair(
        self,
        profile,
        system_prompt: str,
        original_prompt: str,
        text: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> dict[str, Any]:
        try:
            return parse_json_response(text)
        except Exception as first_error:
            last_error = first_error
            retry_text = ""
            for attempt in range(1, JSON_CONTEXT_RETRY_ATTEMPTS + 1):
                retry_prompt = (
                    f"{original_prompt}\n\n"
                    "The previous response for this exact task was not parseable JSON. "
                    "Regenerate the answer using the same task context and return strict JSON only. "
                    "Do not wrap in Markdown. Do not explain. Do not omit required fields. "
                    "Fix JSON syntax issues such as missing commas, dangling quotes, trailing prose, and unescaped newlines.\n\n"
                    f"Parse error: {safe_error_message(last_error)}\n"
                    f"Previous invalid output excerpt:\n{text[:JSON_RETRY_EXCERPT_LIMIT]}"
                )
                retry_text = await self.design.generate(
                    profile,
                    system_prompt,
                    retry_prompt,
                    images,
                    timeout_seconds=timeout_seconds,
                    proxy_url=proxy_url,
                )
                try:
                    return parse_json_response(retry_text)
                except Exception as retry_error:
                    last_error = retry_error
            repair_prompt = (
                "The previous model output was not valid JSON. Convert it into strict JSON only, preserving all useful content.\n"
                "Return JSON only. Do not wrap in Markdown. Do not explain.\n\n"
                f"Original task:\n{original_prompt}\n\nInvalid output:\n{(retry_text or text)[:JSON_RETRY_EXCERPT_LIMIT]}"
            )
            repaired = await self.design.generate(profile, system_prompt, repair_prompt, images, timeout_seconds=timeout_seconds, proxy_url=proxy_url)
            return parse_json_response(repaired)

    async def _parse_validate_or_fill_missing(
        self,
        profile,
        system_prompt: str,
        original_prompt: str,
        text: str,
        images: list[dict[str, str]],
        validator: Callable[[dict[str, Any]], Any],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> tuple[dict[str, Any], Any, dict[str, Any]]:
        parsed = await self._parse_or_repair(profile, system_prompt, original_prompt, text, images, timeout_seconds=timeout_seconds, proxy_url=proxy_url)
        try:
            return parsed, validator(parsed), {"attempted": False}
        except DesignSchemaError as exc:
            fill_prompt = (
                "The previous model output was valid JSON but failed the required structured output contract.\n"
                "Return strict JSON only. Preserve all valid content and do not redesign the figure, slide master, or page plan.\n"
                "Only fill, normalize, or add the missing required fields and constraints named by the validation error.\n\n"
                f"Validation error:\n{exc}\n\nOriginal task:\n{original_prompt}\n\nCurrent JSON:\n"
                f"{json.dumps(parsed, ensure_ascii=False, indent=2)}"
            )
            filled_text = await self.design.generate(profile, system_prompt, fill_prompt, images, timeout_seconds=timeout_seconds, proxy_url=proxy_url)
            filled = parse_json_response(filled_text)
            return filled, validator(filled), {"attempted": True, "reason": str(exc), "raw_text": filled_text[:2000]}

    def _template_image_payload(self, item: dict[str, Any]) -> dict[str, str]:
        path = self.templates._image_path(item)
        mime_type = mimetypes.guess_type(path.name)[0] or "image/jpeg"
        return {"filename": path.name, "mime_type": mime_type, "b64": image_to_b64(path)}

    def _template_metadata(self, item: dict[str, Any]) -> dict[str, Any]:
        additional = item.get("additional_info") or {}
        return {
            "id": item["template_id"],
            "kind": item["kind"],
            "category": item.get("category") or item.get("original_category"),
            "visual_intent": item.get("visual_intent"),
            "content": item.get("content"),
            "rounded_ratio": additional.get("rounded_ratio"),
            "path_to_gt_image": item.get("path_to_gt_image"),
        }

    def _material_asset_summary(self, asset_id: str) -> dict[str, Any]:
        metadata = self.assets.metadata(asset_id)
        path, mime_type = self.assets.get(asset_id)
        text, parser = self._extract_material_text(path, mime_type)
        excerpt = text[:MATERIAL_TEXT_LIMIT]
        return {
            "id": asset_id,
            "filename": metadata.get("filename") or path.name,
            "mime_type": mime_type,
            "bytes": path.stat().st_size if path.exists() else 0,
            "parser": parser,
            "text_excerpt": excerpt,
            "text_length": len(text),
            "truncated": len(text) > len(excerpt),
        }

    @staticmethod
    def _compose_material_context(material_text: str, material_assets: list[dict[str, Any]]) -> str:
        sections: list[str] = []
        if material_text.strip():
            sections.append(f"User text material:\n{material_text.strip()}")
        for asset in material_assets:
            header = f"File material: {asset['filename']} ({asset['mime_type']}, {asset['bytes']} bytes, parser={asset['parser']})"
            if asset.get("truncated"):
                header += f", excerpt={len(asset.get('text_excerpt') or '')}/{asset.get('text_length')} chars"
            excerpt = asset.get("text_excerpt") or "No extractable text available; use filename and file type as context only."
            sections.append(f"{header}\n{excerpt}")
        return "\n\n---\n\n".join(sections)

    async def _pick_visual_subjects(
        self, material_context: str, design_profile, proxy_url: str | None = None
    ) -> list[str]:
        """让模型挑出资料里值得检索真实外观的主体。

        内部固定流程：不写设计日志、不单独占一个阶段。识别失败、提示词缺失或没有可用主体
        时返回空列表，检索阶段随之降级（图照画，只是没有客观外观描述可依）。
        """
        text = (material_context or "")[:MATERIAL_TEXT_LIMIT].strip()
        if not text:
            return []
        try:
            system_prompt = self.prompts.load(VISUAL_SUBJECTS_KEY)["content"]
        except Exception:  # noqa: BLE001 提示词缺失不该让任务失败，退化成“没有主体”
            return []
        try:
            reply = await self.design.generate(design_profile, system_prompt, text, [], proxy_url=proxy_url)
        except Exception:  # noqa: BLE001 同 Rust：识别失败＝没有主体
            return []
        return self._visual_subjects_from_reply(reply, text)

    @staticmethod
    def _visual_subjects_from_reply(reply: str, material: str) -> list[str]:
        """取回复里第一个 JSON 数组，并只留下真的出现在资料原文里的候选。

        大小写、空格与标点不计差异（`Grok Bot` 对得上原文 `grokbot`），所以模型无法凭记忆塞进
        一个原文没提过的品牌；顺带保证字体、色值这类约束描述不会被当成视觉主体。
        """
        if not isinstance(reply, str):
            return []
        start, end = reply.find("["), reply.rfind("]")
        if start < 0 or end < start:
            return []
        try:
            items = json.loads(reply[start : end + 1])
        except (ValueError, TypeError):
            return []
        if not isinstance(items, list):
            return []
        haystack = JobManager._compact_for_match(material)
        seen: set[str] = set()
        subjects: list[str] = []
        for item in items:
            if not isinstance(item, str):
                continue
            subject = item.strip()
            key = JobManager._compact_for_match(subject)
            if not key or key in seen or key not in haystack:
                continue
            seen.add(key)
            subjects.append(subject)
            if len(subjects) >= VISUAL_ASSET_SEARCH_TERM_LIMIT:
                break
        return subjects

    @staticmethod
    def _compact_for_match(text: str) -> str:
        """只留字母数字并折叠大小写，用于判断候选是否真的出自原文。"""
        return "".join(ch for ch in text.lower() if ch.isalnum())

    async def _build_visual_asset_context(
        self, subjects: list[str], proxy_url: str | None = None, *, job_id: str | None = None
    ) -> dict[str, Any]:
        """逐主体检索，每个主体一次查询，报文与原始返回先写进设计日志再决定冒泡还是降级。"""
        search_profile = self.config.active_profile("search")
        search_meta = {
            "profile_id": search_profile.id,
            "protocol": search_profile.protocol,
            "model": search_profile.model,
            "base_url": search_profile.base_url,
            "has_api_key": bool(search_profile.api_key),
        }
        if not subjects:
            return {
                "enabled": True,
                "degraded": True,
                "terms": [],
                "items": [],
                "search_profile": search_meta,
                "message": "No explicit product/tool/platform terms were detected; use generic semantic icons or object illustrations only when content benefits.",
            }
        max_results = int((search_profile.output_defaults or {}).get("max_results") or VISUAL_ASSET_SEARCH_RESULT_LIMIT)
        max_results = max(1, min(8, max_results))
        tasks = [
            self._search_visual_asset_term(subject, search_profile, proxy_url, max_results=max_results)
            for subject in subjects
        ]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        # 每次检索实际发出去的报文与原始返回都写进设计日志（界面在进度下方展示）。
        # 先记再抛：grok 失败会让任务失败，但失败的那次调用同样要能复盘。
        self._append_search_traces(job_id, subjects, results)
        items: list[dict[str, Any]] = []
        for subject, result in zip(subjects, results, strict=False):
            if isinstance(result, Exception):
                # 检索任务自身炸了（不是某次请求失败）：也要能按协议决定冒泡还是降级。
                item = {"term": subject, "query": subject, "results": [], "provider": search_profile.protocol}
                failure: Exception | None = result
            else:
                item, failure = result
            if failure is not None:
                item["error"] = safe_error_message(failure)
                # 检索失败就是失败：路由、鉴权、限流、反爬页都得让任务看见。
                # 降级成“没有来源”会把基础设施问题伪装成“这个主体查不到”，
                # 用户既看不到原因，还会拿着一份没有依据的图继续跑完。
                # 报文已经先写进设计日志，这里原样冒泡。
                raise failure
            items.append(item)
        has_sources = any(item.get("results") for item in items)
        return {
            "enabled": True,
            "degraded": not has_sources,
            "terms": list(subjects),
            "items": items,
            "search_profile": search_meta,
            "message": "Use only text summaries and source URLs; no network image is downloaded, cached, or passed to the implement model.",
        }

    async def _search_visual_asset_term(
        self,
        subject: str,
        search_profile,
        proxy_url: str | None = None,
        *,
        max_results: int = VISUAL_ASSET_SEARCH_RESULT_LIMIT,
    ) -> tuple[dict[str, Any], Exception | None]:
        call = await self.search.search(
            search_profile,
            subject,
            max_results=max_results,
            proxy_url=proxy_url,
        )
        item: dict[str, Any] = {
            "term": subject,
            "query": subject,
            "results": call.results,
            "provider": search_profile.protocol,
            "request": call.request,
            "response": call.response,
        }
        if call.error is not None:
            item["error"] = safe_error_message(call.error)
        # 错误不在这里抛：调用方要先记录报文，再决定是冒泡（grok）还是降级。
        return item, call.error

    def _append_design_log(self, job_id: str | None, step: str, label: str, content: str) -> None:
        """写一行设计日志（同 step 覆盖旧的，不重复累积）。"""
        from .models import JobDesignLog

        if not job_id:
            return
        logs = [log for log in self.get(job_id).design_logs if log.step != step]
        logs.append(
            JobDesignLog(step=step, label=label, status="succeeded", content=content, timestamp=now_iso())
        )
        self._update(job_id, design_logs=logs)

    def _optional_profile(self, role: str) -> ModelProfile | None:
        """检索是加分项而不是前提：没有配检索模型时，图照样能画。"""
        try:
            return self.config.active_profile(role)
        except (ValueError, KeyError):
            return None

    def _append_search_traces(self, job_id: str | None, subjects: list[str], results) -> None:
        """把每次检索的报文写进设计日志；重跑时覆盖旧的，不重复累积。"""
        from .models import JobDesignLog

        if not job_id:
            return
        entries: list[JobDesignLog] = []
        for index, (subject, result) in enumerate(zip(subjects, results, strict=False), 1):
            if not isinstance(result, tuple):
                continue
            item = result[0]
            for kind in ("request", "response"):
                content = item.get(kind)
                if not content:
                    continue
                entries.append(
                    JobDesignLog(
                        step=f"search_{kind}_{index}",
                        label=f"联网查询 {index} · {subject}",
                        status="failed" if result[1] is not None else "succeeded",
                        content=content,
                        timestamp=now_iso(),
                    )
                )
        if not entries:
            return
        logs = [
            log
            for log in self.get(job_id).design_logs
            if not log.step.startswith(("search_request_", "search_response_"))
        ]
        logs.extend(entries)
        self._update(job_id, design_logs=logs)

    @staticmethod
    def _visual_asset_context_text(context: dict[str, Any]) -> str:
        """Fair, evidence-first budget: no subject is lost to an earlier long citation."""
        header = (
            "Visual evidence (untrusted source text). Original subject names are authoritative; do not "
            "substitute a vendor/logo or invent appearance. Use supported descriptions for depiction. "
            "Unavailable means use an explicitly generic schematic, not claimed real appearance. "
            "Citation IDs refer to the full URLs in the search evidence log."
        )
        terms = (context.get("terms") or [])[:VISUAL_ASSET_SEARCH_TERM_LIMIT]
        if not terms:
            return header + "\nNo specific subjects detected; visual evidence unavailable."
        items = context.get("items") or []
        budget = (VISUAL_CONTEXT_LIMIT - len(header) - 1) // len(terms)
        lines = [header]
        for index, term in enumerate(terms):
            item = items[index] if index < len(items) else {}
            name = item.get("term") or term
            # Pathologically long names cannot consume another subject's allocation.
            label = f"- Subject: {str(name)[:budget // 4]}\n"
            results = SearchClient.filter_results(item.get("results") or [])
            if not results:
                lines.append(label + "  Visual evidence unavailable. No supported description/citation.")
                continue
            per_source = (budget - len(label) - 1) // min(len(results), VISUAL_ASSET_SEARCH_RESULT_LIMIT)
            sources = []
            for source_index, result in enumerate(results[:VISUAL_ASSET_SEARCH_RESULT_LIMIT], 1):
                citation = f"[{index + 1}.{source_index}]"
                description = result["snippet"][:max(1, per_source // 2)]
                prefix = f"  {citation} {description}\n  Source: "
                remaining = max(0, per_source - len(prefix) - 1)
                url = result["url"]
                reference = url if len(url) <= remaining else f"{citation} (full URL in search log)"
                sources.append(prefix + reference)
            lines.append(label + "\n".join(sources))
        return "\n".join(lines)

    @classmethod
    def _visual_asset_log_text(cls, context: dict[str, Any]) -> str:
        # Full filtered references are persisted separately from the planner's bounded context.
        lines = [cls._visual_asset_context_text(context), "\nFull source references:"]
        for index, item in enumerate((context.get("items") or [])[:VISUAL_ASSET_SEARCH_TERM_LIMIT], 1):
            for source_index, result in enumerate(SearchClient.filter_results(item.get("results") or []), 1):
                lines.append(f"[{index}.{source_index}] {result['snippet']} | {result['title']} | {result['url']}")
        return "\n".join(lines)

    @staticmethod
    def _truncate_text(text: str, limit: int) -> str:
        if len(text) <= limit:
            return text
        return f"{text[:limit]}\n\n[Truncated from {len(text)} chars to {limit} chars for page planning latency.]"

    @classmethod
    def _compact_template_analysis(cls, template_analysis: dict[str, Any]) -> str:
        wrapper = template_analysis.get("template_analysis") if isinstance(template_analysis.get("template_analysis"), dict) else template_analysis
        if not isinstance(wrapper, dict):
            return cls._truncate_text(json.dumps(template_analysis, ensure_ascii=False), PPT_PLAN_TEMPLATE_LIMIT)
        master = wrapper.get("master_style_spec") if isinstance(wrapper.get("master_style_spec"), dict) else {}
        compact = {
            "master_style_summary": wrapper.get("master_style_summary"),
            "master_style_spec": {
                "canvas": master.get("canvas"),
                "title_region": master.get("title_region"),
                "safe_margins": master.get("safe_margins"),
                "header_footer": master.get("header_footer"),
                "divider_lines": master.get("divider_lines"),
                "palette": master.get("palette"),
                "typography": master.get("typography"),
                "module_style": master.get("module_style"),
                "decorative_elements": master.get("decorative_elements"),
                "immutable_elements": master.get("immutable_elements"),
                "forbidden_deviations": master.get("forbidden_deviations"),
            },
            "global_constraints": wrapper.get("global_constraints"),
            "immutable_elements": wrapper.get("immutable_elements"),
            "page_layout_rules": wrapper.get("page_layout_rules"),
        }
        return cls._truncate_text(json.dumps(compact, ensure_ascii=False, separators=(",", ":")), PPT_PLAN_TEMPLATE_LIMIT)

    @staticmethod
    def _extract_material_text(path: Path, mime_type: str) -> tuple[str, str]:
        suffix = path.suffix.lower()
        if suffix in {".txt", ".md", ".markdown", ".csv", ".tsv", ".json"} or mime_type.startswith("text/"):
            return path.read_text(encoding="utf-8", errors="ignore"), "text"
        if suffix == ".docx":
            try:
                with zipfile.ZipFile(path) as docx:
                    xml = docx.read("word/document.xml")
                root = ElementTree.fromstring(xml)
                text = "\n".join(node.text or "" for node in root.iter() if node.tag.endswith("}t"))
                return text, "docx-xml"
            except Exception as exc:
                return f"DOCX text extraction failed: {safe_error_message(exc)}", "docx-error"
        if suffix == ".pdf" or mime_type == "application/pdf":
            try:
                from pypdf import PdfReader  # type: ignore

                reader = PdfReader(str(path))
                text = "\n".join(page.extract_text() or "" for page in reader.pages[:20])
                return text, "pypdf"
            except Exception as exc:
                return f"PDF text extraction unavailable: {safe_error_message(exc)}", "pdf-unavailable"
        return "", "metadata-only"

    def _save_image(self, job_id: str, name: str, image_b64: str) -> JobImage:
        job_dir = self.root / job_id
        job_dir.mkdir(parents=True, exist_ok=True)
        path = job_dir / name
        path.write_bytes(base64.b64decode(image_b64))
        return JobImage(name=name, url=f"/api/jobs/{job_id}/images/{name}")

    def image_path(self, job_id: str, name: str) -> Path:
        path = self.root / job_id / name
        if not path.exists():
            raise FileNotFoundError(name)
        return path

    def _merge_artifacts(self, job_id: str, changes: dict[str, Any]) -> None:
        record = self.get(job_id)
        self._update(job_id, internal_artifacts={**record.internal_artifacts, **changes})

    def _mark_stage(
        self,
        job_id: str,
        stage: str,
        message: str,
        status: str = "running",
        event_status: str = "running",
        **changes: Any,
    ) -> None:
        record = self.get(job_id)
        event = JobEvent(stage=stage, message=message, status=event_status, timestamp=now_iso())
        self._update(job_id, status=status, stage=stage, message=message, events=[*record.events, event], **changes)

    @staticmethod
    def _normalize_record(record: JobRecord) -> JobRecord:
        message = (record.message or "").strip()
        artifacts = record.internal_artifacts or {}
        error = artifacts.get("error") if isinstance(artifacts.get("error"), dict) else {}
        error_message = str(error.get("message") or "").strip()
        timestamp = str(error.get("failed_at") or record.updated_at)

        if record.status == "failed":
            fallback = "任务失败，旧记录没有保存具体错误；请重新提交以获取阶段日志。"
            normalized_message = message or error_message or fallback
            events = record.events or [JobEvent(stage="failed", message=normalized_message, status="failed", timestamp=timestamp)]
            failed_stage = record.stage if record.stage not in {"queued", "started", "failed"} else next(
                (event.stage for event in reversed(events) if event.stage not in {"queued", "started", "failed"}),
                None,
            )
            diagnostic = record.error.model_dump() if record.error else {
                "summary": normalized_message,
                "code": "job_failed",
                "stage": failed_stage,
            }
            diagnostic = JobManager._enrich_error_diagnostic(record, diagnostic)
            return record.model_copy(
                update={
                    "message": diagnostic["summary"],
                    "stage": failed_stage or "failed",
                    "events": events,
                    "error": JobError.model_validate(diagnostic),
                }
            )

        if record.status == "succeeded" and not record.events:
            event = JobEvent(stage="completed", message=message or "任务完成", status="succeeded", timestamp=record.updated_at)
            return record.model_copy(update={"message": message or "任务完成", "stage": "completed", "events": [event]})

        if not record.events:
            normalized_message = message or ("任务已排队" if record.status == "queued" else "任务运行中")
            event = JobEvent(stage=record.stage or record.status, message=normalized_message, status="running", timestamp=record.updated_at)
            return record.model_copy(update={"message": normalized_message, "events": [event]})

        if not message:
            return record.model_copy(update={"message": record.events[-1].message})
        return record

    @staticmethod
    def _enrich_error_diagnostic(record: JobRecord, diagnostic: dict[str, Any]) -> dict[str, Any]:
        result = {key: value for key, value in diagnostic.items() if value is not None}
        stage = str(result.get("stage") or "")
        artifacts = record.internal_artifacts or {}
        role = result.get("role")
        if not role:
            design_markers = ("design", "structure", "analyze", "outline", "page_plan", "parse")
            role = "implement" if "implement" in stage else "design" if any(marker in stage for marker in design_markers) else None
            result["role"] = role

        if role == "implement":
            request_keys = ["implement_model_request"]
        elif role != "design":
            request_keys = []
        elif "outline" in stage:
            request_keys = ["outline_design_model_request", "design_model_request"]
        elif "structure" in stage:
            request_keys = ["structure_model_request", "design_model_request"]
        else:
            request_keys = ["design_model_request", "outline_design_model_request", "structure_model_request"]
        profile: dict[str, Any] = {}
        for key in request_keys:
            request = artifacts.get(key)
            if isinstance(request, list):
                request = request[-1] if request else None
            if isinstance(request, dict) and isinstance(request.get("profile"), dict):
                profile = request["profile"]
                break
        for key in ("id", "name", "protocol", "model", "base_url"):
            target = {"id": "profile_id", "name": "profile_name"}.get(key, key)
            if profile.get(key) is not None and not result.get(target):
                result[target] = profile[key]

        status_match = re.search(r"HTTP\s+(\d{3})", str(result.get("summary") or ""), flags=re.I)
        if status_match and not result.get("http_status"):
            result["http_status"] = int(status_match.group(1))
        status = result.get("http_status")
        summary = str(result.get("summary") or "任务失败")
        if "<html" in summary.lower() or "<!doctype" in summary.lower():
            detail = response_error_summary(summary)
            prefix = f"{str(role).capitalize()} 请求失败"
            summary = f"{prefix}：HTTP {status or '错误'}。{f' 服务返回：{detail}' if detail else ''}"
        summary = re.sub(r"^ModelAdapterError:\s*", "", summary).strip()
        result["summary"] = summary[:500]

        if not result.get("endpoint") and profile.get("base_url") and profile.get("protocol"):
            base = normalize_base_url(str(profile["base_url"]), str(profile["protocol"]))
            if role == "design":
                suffix = {
                    "openai_chat": "/chat/completions",
                    "openai_responses": "/responses",
                    "anthropic_messages": "/messages",
                }.get(str(profile["protocol"]), "")
            elif profile.get("protocol") == "image2":
                suffix = "/images/generations"
            else:
                version = profile.get("api_version") or "v1beta"
                base = str(profile["base_url"]).rstrip("/")
                suffix = f"/{version}/interactions"
            result["endpoint"] = f"{base}{suffix}"
        if not result.get("suggestion"):
            if status in {502, 503, 504}:
                result["suggestion"] = f"{str(role).capitalize()} 上游网关异常，请稍后重试；若持续出现，请检查该服务地址或更换中转服务。"
            elif not role:
                result["suggestion"] = "请根据失败阶段检查输入文件、运行环境或对应服务配置。"
            else:
                result["suggestion"] = f"请检查 {str(role).capitalize()} 配置的地址、协议、模型名和密钥。"
        return result

    def _finish_prompts(self, job_id: str, prompts: list[str]) -> None:
        from .models import JobDesignLog

        if not prompts or any(not prompt.strip() for prompt in prompts):
            raise ValueError("Final drawing prompts are empty")
        logs = [log for log in self.get(job_id).design_logs if not log.step.startswith("final_prompt_")]
        logs.extend(JobDesignLog(step=f"final_prompt_{index}", label=f"最终制图提示词 {index}",
                                 status="succeeded", content=prompt, timestamp=now_iso())
                    for index, prompt in enumerate(prompts, 1))
        self._update(job_id, design_logs=logs)
        self._mark_stage(job_id, "completed", "最终制图提示词已生成", status="succeeded", event_status="succeeded", images=[])

    def _update(self, job_id: str, **changes) -> None:
        record = self.get(job_id)
        next_record = record.model_copy(update={"updated_at": now_iso(), **changes})
        self._persist(next_record)
        self.jobs[job_id] = next_record

    def _persist(self, record: JobRecord) -> None:
        job_dir = self.root / record.id
        job_dir.mkdir(parents=True, exist_ok=True)
        (job_dir / "job.json").write_text(record.model_dump_json(indent=2), encoding="utf-8")

    @staticmethod
    def _paper_implement_prompt(design_json: dict[str, Any]) -> str:
        figure = design_json.get("figure") if isinstance(design_json.get("figure"), dict) else design_json
        if not isinstance(figure, dict):
            raise ValueError("Design model response must be a JSON object")
        prompt = figure.get("implement_prompt")
        if not isinstance(prompt, str):
            raise DesignSchemaError("Design model response missing implement_prompt")
        return prompt

    @staticmethod
    def _is_filled(value: Any) -> bool:
        if value is None:
            return False
        if isinstance(value, str):
            return bool(value.strip())
        if isinstance(value, (list, dict, tuple, set)):
            return bool(value)
        return True

    @classmethod
    def _require_fields(cls, data: dict[str, Any], fields: tuple[str, ...], label: str) -> None:
        missing = [field for field in fields if not cls._is_filled(data.get(field))]
        if missing:
            raise DesignSchemaError(f"{label} missing required fields: {', '.join(missing)}")

    @classmethod
    def _validate_expression_plan(cls, data: dict[str, Any], label: str) -> None:
        units = data.get("information_units")
        if not isinstance(units, list):
            raise DesignSchemaError(f"{label} missing information_units array")
        if not units:
            raise DesignSchemaError(f"{label} information_units must list at least one information unit with its carrier")
        for index, unit in enumerate(units, start=1):
            if not isinstance(unit, dict):
                raise DesignSchemaError(f"{label} information unit {index} must be an object")
            for field in EXPRESSION_INFORMATION_UNIT_FIELDS:
                value = unit.get(field)
                if not isinstance(value, str) or not value.strip():
                    raise DesignSchemaError(f"{label} information unit {index} missing string {field}")
        redundancy = data.get("redundancy_check")
        if not isinstance(redundancy, dict):
            raise DesignSchemaError(f"{label} missing redundancy_check object")
        statement = redundancy.get("statement")
        if not isinstance(statement, str) or not statement.strip():
            raise DesignSchemaError(
                f"{label} redundancy_check.statement must confirm no graphic-graphic, text-text, or graphic-text duplication"
            )
        plan = data.get("hierarchy_plan")
        if not isinstance(plan, dict):
            raise DesignSchemaError(f"{label} missing hierarchy_plan object")
        cls._require_fields(plan, EXPRESSION_HIERARCHY_PLAN_FIELDS, f"{label} hierarchy_plan")
        if not isinstance(plan.get("levels"), list):
            raise DesignSchemaError(f"{label} hierarchy_plan.levels must be a non-empty list of typography levels")
        for field in ("alignment", "focus_region"):
            if not isinstance(plan.get(field), str):
                raise DesignSchemaError(f"{label} hierarchy_plan.{field} must be a string")

    @staticmethod
    def _keyword_groups_present(text: str, groups: dict[str, tuple[str, ...]]) -> set[str]:
        lowered = text.lower()
        return {name for name, keywords in groups.items() if any(keyword.lower() in lowered for keyword in keywords)}

    @classmethod
    def _validate_no_copy_request(cls, prompt: str) -> None:
        """拒绝明确要求复制/编辑模板底图的 implement_prompt。

        design model 常会在同一句里写「禁止 copy the template」；旧实现只看
        短语前 24 字符，且把 ValueError 排除在 schema 修复重试之外，导致误杀。
        这里按句子窗口判断否定/禁止语气，失败时抛 DesignSchemaError 以触发补全重试。
        """
        lowered_prompt = prompt.lower()
        negations = (
            "do not ",
            "don't ",
            "dont ",
            "never ",
            " no ",
            "not ",
            "avoid ",
            "must not ",
            "cannot ",
            "can't ",
            "without ",
            "rather than ",
            "instead of ",
            "forbid",
            "forbidden",
            "prohibit",
            "refrain",
            "禁止",
            "不要",
            "不得",
            "不能",
            "不可",
            "严禁",
            "避免",
            "勿",
        )
        allow_context = (
            "reference only",
            "as reference",
            "style only",
            "layout only",
            "not as base",
            "not a base",
            "not base image",
            "inspiration only",
            "few-shot",
            "few shot",
        )
        sentences = re.split(r"(?<=[\.\!\?\n；;。！？])\s*|\n+", lowered_prompt)
        for sentence in sentences:
            if not sentence.strip():
                continue
            for phrase in FORBIDDEN_TEMPLATE_COPY_PHRASES:
                start = 0
                while True:
                    idx = sentence.find(phrase, start)
                    if idx == -1:
                        break
                    before = sentence[:idx]
                    # 句首无空格时也要识别 leading "no "
                    before_padded = f" {before}"
                    negated = any(neg in before_padded for neg in negations) or any(
                        sentence.lstrip().startswith(neg.strip()) for neg in negations if neg.strip()
                    )
                    contextual_allow = any(token in sentence for token in allow_context) and any(
                        neg in sentence for neg in negations
                    )
                    if not negated and not contextual_allow:
                        raise DesignSchemaError(
                            "Implement prompt requests direct template copying or editing "
                            f"(matched {phrase!r}). Rewrite implement_prompt so templates are "
                            "style/layout reference only; never copy, trace, edit, or use as base image."
                        )
                    start = idx + len(phrase)

    @classmethod
    def _normalize_inventory_item(cls, value: Any) -> str:
        # 只压缩连续空白，不能删除：删空格会把 "External retriever" 变成
        # "Externalretriever"，使英文条目在原文中永远匹配不到。
        # 中文分词空格的容错改由比对阶段的去空白版本承担。
        return re.sub(r"\s+", " ", str(value or "")).strip()

    @staticmethod
    def _strip_ws(text: str) -> str:
        return re.sub(r"\s+", "", text)

    @classmethod
    def _item_grounded_in_source(cls, item: str, source: str) -> bool:
        """inventory 短标签是否可在原文中找到依据（允许子串，避免中文分词）。"""
        if not item:
            return False
        if item in source or item.lower() in source.lower():
            return True
        # 去空白后再比一次：中文可能被模型插入空格，英文原文可能跨行断开
        if cls._strip_ws(item).lower() in cls._strip_ws(source).lower():
            return True
        # 标签过长时：任意连续 4 字中文子串命中也算 grounded
        cn_parts = re.findall(r"[\u4e00-\u9fff]{4,}", item)
        if any(part in source for part in cn_parts):
            return True
        en_parts = re.findall(r"[A-Za-z][A-Za-z0-9\-+_/]{1,}", item)
        if any(part.lower() in source.lower() for part in en_parts if len(part) >= 2):
            return True
        return False

    @classmethod
    def _item_present_in_output(cls, item: str, haystack: str) -> bool:
        if not item:
            return False
        if item in haystack or item.lower() in haystack.lower():
            return True
        if cls._strip_ws(item).lower() in cls._strip_ws(haystack).lower():
            return True
        # 模块可能被缩短：检查 inventory 中较长的中文/英文片段
        for part in re.findall(r"[\u4e00-\u9fff]{3,8}", item):
            if part in haystack:
                return True
        for part in re.findall(r"[A-Za-z][A-Za-z0-9\-+_/]{2,}", item):
            if part.lower() in haystack.lower():
                return True
        return False

    @classmethod
    def _validate_design_content_coverage(cls, design_json: dict[str, Any], title: str, section: str) -> None:
        """以 content_inventory 为中心做保真，避免中文无空格全文被切成乱片段导致假阴性。"""
        figure = design_json.get("figure") if isinstance(design_json.get("figure"), dict) else design_json
        if not isinstance(figure, dict):
            return
        implement_prompt = cls._paper_implement_prompt(design_json)
        source = f"{title}\n{section}".strip()
        inventory_raw = figure.get("content_inventory") if isinstance(figure.get("content_inventory"), list) else []
        inventory = [cls._normalize_inventory_item(item) for item in inventory_raw if cls._is_filled(item)]
        # 去重保序
        deduped: list[str] = []
        seen: set[str] = set()
        for item in inventory:
            key = item.lower()
            if key in seen:
                continue
            seen.add(key)
            deduped.append(item)
        inventory = deduped

        diagram_spec = figure.get("diagram_spec") if isinstance(figure.get("diagram_spec"), dict) else {}
        modules = diagram_spec.get("modules") if isinstance(diagram_spec.get("modules"), list) else []
        modules_text = " ".join(str(item) for item in modules if cls._is_filled(item))
        haystack = f"{implement_prompt}\n{modules_text}\n{' '.join(inventory)}\n{title}"

        # 短方法文：只要求 prompt 足够长
        if len(source) < 180:
            if len(implement_prompt.strip()) < 360:
                raise DesignSchemaError("implement_prompt too short; expand grounded operational detail from the user section")
            return

        if len(inventory) < 8:
            raise DesignSchemaError(
                "content_inventory too small for a rich method section; extract at least 8 short grounded "
                "operation/component labels from the user text (e.g. OCR, BM25, 双塔编码, 交叉重排)"
            )

        # inventory 项本身必须能在用户原文中找到依据
        ungrounded = [item for item in inventory if not cls._item_grounded_in_source(item, source)]
        if len(ungrounded) > max(2, len(inventory) // 3):
            raise DesignSchemaError(
                "content_inventory contains too many items not grounded in the user section. "
                f"Fix or remove: {', '.join(ungrounded[:10])}"
            )

        # implement_prompt / modules 应覆盖大部分 inventory（短标签），而不是原文长句碎片
        missing = [item for item in inventory if not cls._item_present_in_output(item, haystack)]
        covered = len(inventory) - len(missing)
        coverage = covered / max(1, len(inventory))
        if coverage < 0.55:
            raise DesignSchemaError(
                "implement_prompt/modules miss too many content_inventory items "
                f"(coverage {coverage:.0%}). Re-include short labels such as: {', '.join(missing[:12])}"
            )

        detail_ok = any(
            marker in implement_prompt.lower() or marker in implement_prompt
            for marker in ("module detail", "模块细节", "per stage", "各阶段", "子模块", "leaf module")
        )
        if not detail_ok and ("阶段" not in implement_prompt or len(implement_prompt) < 450):
            raise DesignSchemaError(
                "implement_prompt must include a MODULE DETAIL / 模块细节 section listing leaf steps per stage"
            )

    @classmethod
    def _validate_paper_design(cls, design_json: dict[str, Any]) -> None:
        figure = design_json.get("figure") if isinstance(design_json.get("figure"), dict) else design_json
        if not isinstance(figure, dict):
            raise DesignSchemaError("Design model response must contain a figure object")
        cls._require_fields(figure, FIGURE_COMMON_FIELDS, "Paper figure")
        cls._validate_expression_plan(figure, "Paper figure")
        implement_prompt = cls._paper_implement_prompt(design_json)
        if len(implement_prompt.strip()) < 360:
            raise DesignSchemaError(
                "Paper figure implement_prompt is too short; keep leaf-level operations from the user section"
            )
        aspect_ratio = figure.get("aspect_ratio")
        if aspect_ratio and aspect_ratio not in FIGURE_ASPECT_RATIOS:
            raise ValueError(f"Invalid paper figure aspect_ratio: {aspect_ratio}")
        visible_text = figure.get("visible_text", [])
        if not isinstance(visible_text, list) or any(not isinstance(item, str) or len(item.strip()) > 80 for item in visible_text):
            raise ValueError("Paper figure visible_text must be short label strings")
        cls._validate_no_copy_request(implement_prompt)
        prompt_groups = cls._keyword_groups_present(
            implement_prompt,
            {
                "publication": ("publication", "academic", "paper", "论文", "出版"),
                "faithfulness": ("faithful", "faithfulness", "grounded", "no hallucination", "忠实", "不虚构"),
                "conciseness": ("concise", "abstraction", "short label", "简洁", "抽象", "短标签"),
                "readability": ("readable", "legible", "contrast", "可读", "对比"),
                "template_boundary": ("template", "reference", "not copy", "参考", "模板"),
            },
        )
        if len(prompt_groups) < 4:
            raise DesignSchemaError("Paper figure implement_prompt must cover publication quality, faithfulness, conciseness, readability, and template boundary")
        visual_type = str(figure.get("visual_type") or "").strip().lower()
        if visual_type in {"diagram", "workflow", "comparison", "mechanism"}:
            diagram_spec = figure.get("diagram_spec")
            if not isinstance(diagram_spec, dict):
                raise DesignSchemaError("Diagram figure missing diagram_spec")
            cls._require_fields(diagram_spec, DIAGRAM_SPEC_FIELDS, "Diagram spec")
            modules = diagram_spec.get("modules")
            if not isinstance(modules, list) or len([m for m in modules if cls._is_filled(m)]) < 8:
                raise DesignSchemaError(
                    "Diagram modules too few; expand grounded leaf steps from the user section "
                    "(need at least 8 named modules for multi-step methods)"
                )
            connections = diagram_spec.get("connections")
            if not isinstance(connections, list) or len(connections) < 7:
                raise DesignSchemaError(
                    "Diagram connections too few; need at least 7 grounded edges for multi-stage flow"
                )
            for index, connection in enumerate(connections, start=1):
                if not isinstance(connection, dict):
                    raise DesignSchemaError(f"Diagram connection {index} must be an object")
                cls._require_fields(connection, ("source", "target", "meaning"), f"Diagram connection {index}")
            grouping = str(diagram_spec.get("grouping_hierarchy") or "").strip()
            if len(grouping) < 12:
                raise DesignSchemaError("Diagram grouping_hierarchy must describe multi-stage/lane structure")
            lower_prompt = implement_prompt.lower()
            complexity_markers = (
                "stage",
                "pipeline",
                "multi",
                "branch",
                "group",
                "layer",
                "阶段",
                "支路",
                "分层",
                "模块",
                "流程",
                "module detail",
                "模块细节",
            )
            if not any(marker in lower_prompt or marker in implement_prompt for marker in complexity_markers):
                raise DesignSchemaError(
                    "implement_prompt must describe multi-stage/hierarchical flowchart layout "
                    "with leaf module details from the user section"
                )
            return
        if visual_type in {"plot", "chart"}:
            plot_spec = figure.get("plot_spec")
            if not isinstance(plot_spec, dict):
                raise DesignSchemaError("Plot/chart figure missing plot_spec")
            cls._require_fields(plot_spec, PLOT_SPEC_FIELDS, "Plot spec")
            axes = plot_spec.get("axes")
            if not isinstance(axes, dict) or not cls._is_filled(axes.get("x")) or not cls._is_filled(axes.get("y")):
                raise DesignSchemaError("Plot spec axes must include x and y definitions")
            integrity = str(plot_spec.get("data_integrity_rules") or "")
            if len(integrity) < 40:
                raise DesignSchemaError("Plot spec data_integrity_rules must explicitly describe anti-distortion constraints")
            return
        raise DesignSchemaError("Paper figure visual_type must be diagram, workflow, comparison, mechanism, plot, or chart")

    @classmethod
    def _master_style_spec(cls, template_analysis: dict[str, Any]) -> dict[str, Any]:
        wrapper = template_analysis.get("template_analysis") if isinstance(template_analysis.get("template_analysis"), dict) else template_analysis
        if not isinstance(wrapper, dict):
            raise DesignSchemaError("Template analysis must be a JSON object")
        master = wrapper.get("master_style_spec")
        if not isinstance(master, dict):
            raise DesignSchemaError("Template analysis missing master_style_spec")
        return master

    @classmethod
    def _validate_template_analysis(cls, template_analysis: dict[str, Any]) -> None:
        wrapper = template_analysis.get("template_analysis") if isinstance(template_analysis.get("template_analysis"), dict) else template_analysis
        if not isinstance(wrapper, dict):
            raise DesignSchemaError("Template analysis must contain template_analysis object")
        master = cls._master_style_spec(template_analysis)
        cls._require_fields(master, MASTER_STYLE_FIELDS, "PPT master_style_spec")
        cls._require_fields(wrapper, ("global_constraints", "immutable_elements", "page_layout_rules"), "PPT template_analysis")
        immutable = wrapper.get("immutable_elements")
        if not isinstance(immutable, list) or len(immutable) < 3:
            raise DesignSchemaError("PPT immutable_elements must list at least title/page marker/divider or equivalent master elements")

    @classmethod
    def _validate_ppt_outline(cls, outline_json: dict[str, Any], page_count: int) -> dict[str, Any]:
        outline = outline_json.get("deck_outline") if isinstance(outline_json.get("deck_outline"), dict) else outline_json
        if not isinstance(outline, dict):
            raise DesignSchemaError("PPT outline response must be a JSON object")
        cls._require_fields(
            outline,
            ("deck_title", "deck_goal", "narrative_arc", "shared_prompt", "page_briefs"),
            "PPT deck_outline",
        )
        page_briefs = outline.get("page_briefs")
        if not isinstance(page_briefs, list):
            raise DesignSchemaError("PPT deck_outline.page_briefs must be a list")
        if len(page_briefs) != page_count:
            raise ValueError(f"Deck outline returned {len(page_briefs)} page briefs, expected {page_count}")
        sorted_briefs = sorted(page_briefs, key=lambda item: item.get("page", 0) if isinstance(item, dict) else 0)
        actual_pages = [item.get("page") for item in sorted_briefs if isinstance(item, dict)]
        expected_pages = list(range(1, page_count + 1))
        if actual_pages != expected_pages:
            raise ValueError(f"PPT outline page briefs must be ordered 1..{page_count}; got {actual_pages}")
        for brief in sorted_briefs:
            if not isinstance(brief, dict):
                raise DesignSchemaError("Each PPT page brief must be an object")
            cls._require_fields(
                brief,
                (
                    "page",
                    "title",
                    "role",
                    "main_message",
                    "content_points",
                    "suggested_template",
                    "visual_direction",
                    "transition_from_previous",
                    "transition_to_next",
                ),
                f"Page brief {brief.get('page')}",
            )
            points = brief.get("content_points")
            if not isinstance(points, list) or not points:
                raise DesignSchemaError(f"Page brief {brief.get('page')} content_points must be a non-empty list")
        return {**outline, "page_briefs": sorted_briefs}

    @staticmethod
    def _adjacent_page_context(deck_outline: dict[str, Any], page_number: int) -> dict[str, Any]:
        briefs = deck_outline.get("page_briefs") if isinstance(deck_outline.get("page_briefs"), list) else []
        previous_brief = next((brief for brief in briefs if isinstance(brief, dict) and brief.get("page") == page_number - 1), None)
        next_brief = next((brief for brief in briefs if isinstance(brief, dict) and brief.get("page") == page_number + 1), None)
        return {"previous": previous_brief, "next": next_brief}

    @classmethod
    def _validate_ppt_single_page(
        cls,
        page_json: dict[str, Any],
        expected_page: int,
        template_analysis: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        if template_analysis is not None:
            cls._validate_template_analysis(template_analysis)
        page = page_json.get("page") if isinstance(page_json.get("page"), dict) else None
        if page is None:
            pages = page_json.get("pages")
            if isinstance(pages, list) and len(pages) == 1 and isinstance(pages[0], dict):
                page = pages[0]
        if not isinstance(page, dict):
            raise DesignSchemaError("Single-page worker response must contain one page object")
        if page.get("page") != expected_page:
            raise ValueError(f"Single-page worker returned page {page.get('page')}, expected {expected_page}")
        cls._validate_ppt_page_fields(page)
        return page

    @classmethod
    def _validate_ppt_page_fields(cls, page: dict[str, Any]) -> None:
        if not isinstance(page, dict):
            raise DesignSchemaError("Each PPT page must be an object")
        cls._require_fields(
            page,
            (
                "page",
                "selected_template",
                "title",
                "slide_type",
                "body_layout_plan",
                "master_style_binding",
                "visual_element_plan",
                "emphasis_plan",
                "information_units",
                "redundancy_check",
                "hierarchy_plan",
                "visible_text",
                "implement_prompt",
            ),
            f"Page {page.get('page')}",
        )
        binding = page.get("master_style_binding")
        if not isinstance(binding, dict):
            raise DesignSchemaError(f"Page {page.get('page')} missing master_style_binding object")
        cls._require_fields(binding, PAGE_MASTER_BINDING_FIELDS, f"Page {page.get('page')} master_style_binding")
        visual_plan = page.get("visual_element_plan")
        if not isinstance(visual_plan, dict):
            raise DesignSchemaError(f"Page {page.get('page')} missing visual_element_plan object")
        cls._require_fields(visual_plan, PAGE_VISUAL_PLAN_FIELDS, f"Page {page.get('page')} visual_element_plan")
        visual_elements = visual_plan.get("elements")
        if not isinstance(visual_elements, list):
            raise DesignSchemaError(f"Page {page.get('page')} visual_element_plan.elements must be a list")
        if not visual_elements and "none" not in str(visual_plan.get("usage_decision") or "").lower() and "不用" not in str(visual_plan.get("usage_decision") or ""):
            raise DesignSchemaError(f"Page {page.get('page')} visual_element_plan must list elements or explicitly justify using none")
        for index, element in enumerate(visual_elements, start=1):
            if not isinstance(element, dict):
                raise DesignSchemaError(f"Page {page.get('page')} visual element {index} must be an object")
            cls._require_fields(element, PAGE_VISUAL_ELEMENT_FIELDS, f"Page {page.get('page')} visual element {index}")
        emphasis_plan = page.get("emphasis_plan")
        if not isinstance(emphasis_plan, dict):
            raise DesignSchemaError(f"Page {page.get('page')} missing emphasis_plan object")
        cls._require_fields(emphasis_plan, PAGE_EMPHASIS_PLAN_FIELDS, f"Page {page.get('page')} emphasis_plan")
        keywords = emphasis_plan.get("keywords")
        if not isinstance(keywords, list):
            raise DesignSchemaError(f"Page {page.get('page')} emphasis_plan.keywords must be a list")
        if len(keywords) > 5:
            raise DesignSchemaError(f"Page {page.get('page')} emphasis_plan may highlight at most 5 key phrases")
        for index, keyword in enumerate(keywords, start=1):
            if not isinstance(keyword, dict):
                raise DesignSchemaError(f"Page {page.get('page')} emphasis keyword {index} must be an object")
            cls._require_fields(keyword, PAGE_EMPHASIS_KEYWORD_FIELDS, f"Page {page.get('page')} emphasis keyword {index}")
            text = str(keyword.get("text") or "").strip()
            if len(text) > 24:
                raise DesignSchemaError(f"Page {page.get('page')} emphasis keyword {index} must be a short phrase")
        cls._validate_expression_plan(page, f"Page {page.get('page')}")
        visible_text = page.get("visible_text")
        if not isinstance(visible_text, list) or any(not isinstance(item, str) or len(item.strip()) > 120 for item in visible_text):
            raise DesignSchemaError(f"Page {page.get('page')} visible_text must be short strings")
        prompt = page.get("implement_prompt")
        if not isinstance(prompt, str) or len(prompt.strip()) < 80:
            raise DesignSchemaError(f"Page {page.get('page')} missing usable page-specific implement_prompt")
        if API_OUTPUT_PROMPT_PATTERN.search(prompt) or OUTPUT_SETTINGS_SENTENCE_PATTERN.search(prompt):
            raise DesignSchemaError(f"Page {page.get('page')} implement_prompt must not include API output settings")
        cls._validate_no_copy_request(prompt)

    @classmethod
    def _validate_ppt_pages(
        cls,
        pages_json: dict[str, Any],
        page_count: int,
        template_analysis: dict[str, Any] | None = None,
    ) -> list[dict[str, Any]]:
        pages = pages_json.get("pages") or []
        if not isinstance(pages, list):
            raise DesignSchemaError("PPT design response pages must be a list")
        if len(pages) != page_count:
            raise ValueError(f"Design model returned {len(pages)} pages, expected {page_count}")
        sorted_pages = sorted(pages, key=lambda item: item.get("page", 0) if isinstance(item, dict) else 0)
        expected_pages = list(range(1, page_count + 1))
        actual_pages = [item.get("page") for item in sorted_pages if isinstance(item, dict)]
        if actual_pages != expected_pages:
            raise ValueError(f"PPT pages must be ordered 1..{page_count}; got {actual_pages}")
        if template_analysis is not None:
            cls._validate_template_analysis(template_analysis)
        for page in sorted_pages:
            cls._validate_ppt_page_fields(page)
        return sorted_pages

    @classmethod
    def _sanitize_ppt_page_prompt(cls, prompt: str) -> str:
        cleaned = OUTPUT_SETTINGS_SENTENCE_PATTERN.sub("", prompt)
        cleaned = API_OUTPUT_PROMPT_PATTERN.sub("", cleaned)
        cleaned = re.sub(r"\s{2,}", " ", cleaned)
        cleaned = re.sub(r"\s+([,.;:])", r"\1", cleaned)
        return cleaned.strip()

    @staticmethod
    def _stringify_master_value(value: Any) -> str:
        if isinstance(value, str):
            return value.strip()
        return json.dumps(value, ensure_ascii=False, sort_keys=True)

    @classmethod
    def _ppt_master_prompt_prefix(cls, template_analysis: dict[str, Any], page: dict[str, Any]) -> str:
        master = cls._master_style_spec(template_analysis)
        binding = page.get("master_style_binding") if isinstance(page.get("master_style_binding"), dict) else {}
        parts = [
            "Create one 16:9 academic PowerPoint-style slide.",
            "All visible slide text must be Simplified Chinese only.",
            "Use the extracted template master specification below as immutable; the implement model does not receive the template image, so these text constraints are the source of truth.",
            "Use only the page-specific title and page number supplied in the page-specific section; keep them in the extracted title and page-number regions.",
        ]
        parts.extend(
            [
                f"Background/canvas: {cls._stringify_master_value(master.get('canvas'))}",
                f"Title region: {cls._stringify_master_value(binding.get('title_region') or master.get('title_region'))}",
                f"Safe margins/body area: {cls._stringify_master_value(binding.get('safe_margins') or master.get('safe_margins'))}",
                f"Header/footer/page number/logo/corner marks: {cls._stringify_master_value(binding.get('header_footer') or master.get('header_footer'))}",
                f"Divider lines: {cls._stringify_master_value(binding.get('divider_lines') or master.get('divider_lines'))}",
                f"Palette/background colors: {cls._stringify_master_value(binding.get('palette') or master.get('palette'))}",
                f"Typography/font hierarchy: {cls._stringify_master_value(binding.get('typography') or master.get('typography'))}",
                f"Module/card/border style: {cls._stringify_master_value(binding.get('module_style') or master.get('module_style'))}",
                f"Decorative/immutable elements: {cls._stringify_master_value(master.get('decorative_elements'))}; {cls._stringify_master_value(master.get('immutable_elements'))}",
                f"Forbidden deviations: {cls._stringify_master_value(master.get('forbidden_deviations'))}",
                "Same-level text uses one uniform font size and weight across the whole slide; adjacent levels differ visibly.",
                "Module and card edges align to a shared grid with uniform gutters and equal heights per row; nothing crosses the safe margins.",
                "Never express one information unit with two carriers (for example a table plus a chart of the same data, or a diagram plus a text list of the same steps); keep exactly one.",
                "Do not add API output settings to the prompt text. Do not add extra page numbers, random logos, new corner marks, unrelated footer citations, gradients, editing grids, or decorative noise.",
            ]
        )
        return "\n".join(part for part in parts if part and part != "None")

    @classmethod
    def _apply_ppt_master_prompt_prefix(cls, pages: list[dict[str, Any]], template_analysis: dict[str, Any]) -> list[dict[str, Any]]:
        updated: list[dict[str, Any]] = []
        for page in pages:
            page_prompt = cls._sanitize_ppt_page_prompt(str(page.get("implement_prompt") or ""))
            prefix = cls._ppt_master_prompt_prefix(template_analysis, page)
            page_number = page.get("page")
            title = str(page.get("title") or "").strip()
            page_context = [
                f"Slide title: {title}." if title else "",
                f"Use page number {page_number} only in the extracted page-number position." if page_number else "",
                f"Selected body skeleton: {page.get('selected_template')}. Slide type: {page.get('slide_type')}.",
                f"Body layout plan: {page.get('body_layout_plan')}.",
                f"Visual element plan: {cls._stringify_master_value(page.get('visual_element_plan'))}.",
                f"Keyword emphasis plan: {cls._stringify_master_value(page.get('emphasis_plan'))}.",
                "Render the planned visual elements as actual depictions of their subject — draw the device, product, "
                "specimen, or scene itself with recognizable shape, structure and proportion. Do not substitute a "
                "labeled rectangle, a bare text card, or a generic placeholder box for a subject that can be drawn.",
                "Recolor every visual into the template palette and match the template line weight and card/border "
                "style, so depicted objects read as part of the deck rather than pasted stock art.",
                "Use visual elements only inside the body safe area. Keep them proportional to text, aligned to the template palette, and avoid inventing real brand logos when source context is missing.",
                "Highlight only the planned key phrases using bold weight or the template primary/accent red; do not over-highlight full sentences.",
                page_prompt,
            ]
            merged_prompt = f"{prefix}\n\nPage-specific body layout and content:\n" + "\n".join(item for item in page_context if item)
            updated.append({**page, "implement_prompt": merged_prompt.strip()})
        return updated

    @staticmethod
    def _design_request_summary(
        profile,
        prompt_assets: list[dict[str, str]],
        prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None = None,
    ) -> dict[str, Any]:
        summary = {
            "profile": public_profile_snapshot(profile),
            "prompt_assets": prompt_assets,
            "prompt": prompt,
            "reference_images": [
                {
                    "filename": image["filename"],
                    "mime_type": image["mime_type"],
                    "base64_chars": len(image.get("b64") or ""),
                    "bytes": len(image.get("b64") or "") // 4 * 3,
                }
                for image in images
            ],
        }
        if timeout_seconds is not None:
            summary["timeout_seconds"] = timeout_seconds
        return summary

    @staticmethod
    def _implement_request_summary(
        profile,
        prompt: str,
        output_overrides: dict[str, Any],
        reference_count: int,
        page: int | None = None,
    ) -> dict[str, Any]:
        summary = {
            "profile": public_profile_snapshot(profile),
            "prompt": prompt,
            "output_overrides": output_overrides,
            "reference_image_count": reference_count,
        }
        if page is not None:
            summary["page"] = page
        return summary

    @staticmethod
    def _image_response_summary(image: JobImage, image_b64: str, page: int | None = None) -> dict[str, Any]:
        summary: dict[str, Any] = {
            "name": image.name,
            "url": image.url,
            "bytes": len(base64.b64decode(image_b64)),
        }
        if page is not None:
            summary["page"] = page
        return summary

    @staticmethod
    def _ppt_output_defaults(profile) -> dict[str, Any]:
        protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
        defaults = {key: value for key, value in profile.output_defaults.items() if value not in (None, "")}
        if protocol == "image2":
            return {**IMAGE2_FALLBACK_OUTPUT, **defaults}
        if protocol == "banana2":
            return {
                "aspect_ratio": defaults.get("aspect_ratio", "16:9"),
                "image_size": defaults.get("image_size", "4K"),
                "thinking_level": defaults.get("thinking_level", "high"),
                "mime_type": defaults.get("mime_type", "image/png"),
            }
        return defaults

    @classmethod
    def _validate_structure_plan(cls, data: dict[str, Any]) -> dict[str, Any]:
        plan = data.get("structure_plan") if isinstance(data.get("structure_plan"), dict) else data
        if not isinstance(plan, dict):
            raise DesignSchemaError("Structure plan must be a JSON object under structure_plan")
        required = (
            "visual_family",
            "primary_flow",
            "lanes_or_stages",
            "module_slots",
            "connection_slots",
            "grouping",
            "information_density",
            "layout_skeleton",
        )
        cls._require_fields(plan, required, "Structure plan")
        stages = plan.get("lanes_or_stages")
        if not isinstance(stages, list) or len(stages) < 2:
            raise DesignSchemaError("Structure plan lanes_or_stages needs at least 2 stages/lanes")
        slots = plan.get("module_slots")
        if not isinstance(slots, list) or len(slots) < 6:
            raise DesignSchemaError("Structure plan module_slots needs at least 6 abstract slots for template-like density")
        edges = plan.get("connection_slots")
        if not isinstance(edges, list) or len(edges) < 5:
            raise DesignSchemaError("Structure plan connection_slots needs at least 5 abstract edges")
        skeleton = str(plan.get("layout_skeleton") or "").strip()
        if len(skeleton) < 80:
            raise DesignSchemaError("Structure plan layout_skeleton is too short")
        return plan

    @staticmethod
    def _structure_plan_contract() -> str:
        return """Return strict JSON only:
{
  "structure_plan": {
    "visual_family": "pipeline|architecture|mechanism|comparison|multi-panel|other",
    "canvas_ratio_hint": "16:9|4:3|1:1|inherit",
    "primary_flow": "left-to-right|top-to-bottom|...",
    "lanes_or_stages": [
      {"name": "stage/lane abstract name", "role": "what this band does", "region": "top|middle|bottom|left|right", "slot_count": 3}
    ],
    "module_slots": [
      {"id": "m1", "role": "abstract role e.g. encoder-like", "stage": "stage name", "region": "left-middle"}
    ],
    "connection_slots": [
      {"from": "m1", "to": "m2", "style": "solid|dashed", "meaning_role": "data|control|feedback"}
    ],
    "grouping": "how panels/cards/nested boxes are organized",
    "information_density": "high|medium",
    "palette_and_rhythm": "short note on box style, color mood, spacing rhythm",
    "layout_skeleton": "80+ chars free-text blueprint for later content filling (no user research claims)"
  }
}
Use at least 2 lanes_or_stages, 6 module_slots, and 5 connection_slots. Roles must be abstract, not copied caption text."""

    @staticmethod
    def _paper_contract() -> str:
        return """Return strict JSON only. Choose diagram/workflow/comparison/mechanism OR plot/chart.
CRITICAL: Do not over-summarize the user section. Keep leaf operations (OCR, BM25, dual-tower, cross-encoder, Top-K, etc.) as modules or explicit sub-labels.
Fill structure_plan layout with user content. Prefer the user brief language for on-figure labels (Chinese brief → Chinese labels).
Contract:
{
  "figure": {
    "title": "...",
    "visual_type": "diagram|workflow|comparison|mechanism|plot|chart",
    "aspect_ratio": "inherit|16:9|4:3|1:1|3:2|2:3|9:16",
    "template_usage": "strict|balanced|loose",
    "layout_constraints": ["canvas, composition, hierarchy, spacing, structure_plan alignment"],
    "semantic_constraints": ["faithfulness rules grounded in the user section; no dropped subprocesses"],
    "visual_constraints": ["publication quality, palette, typography, contrast, readable dense layout"],
    "forbidden_errors": ["hallucination", "reversed flow", "scope violation", "text overload", "over-simplification", "direct template copying"],
    "quality_rubric": {
      "faithfulness": "retain user-listed operations",
      "conciseness": "short labels, not fewer steps",
      "readability": "...",
      "aesthetics": "..."
    },
    "content_inventory": ["12-25 grounded operations/components extracted from user section"],
    "visible_text": ["short labels covering inventory"],
    "diagram_spec": {
      "modules": ["≥8 grounded short names covering inventory leaf steps"],
      "entities": ["artifacts e.g. 文档块/证据包/索引"],
      "connections": [{"source": "...", "target": "...", "meaning": "..."}],
      "flow_direction": "left-to-right primary flow (or top-to-bottom)",
      "grouping_hierarchy": "outer stages with nested leaf modules",
      "arrow_routing": "solid data + dashed control/feedback",
      "label_strategy": "keyword labels; keep leaf count"
    },
    "plot_spec": {
      "chart_type": "...",
      "data_fields": ["..."],
      "axes": {"x": "label and unit", "y": "label and unit"},
      "units": "... or none",
      "series_or_categories": ["..."],
      "legend": "...",
      "statistical_annotations": "none or supported annotations only",
      "data_integrity_rules": "No value distortion, misleading scales, label fabrication, wrong chart type, or unsupported statistics."
    },
    "information_units": [{"unit": "one information unit from the material", "carrier": "table|chart|diagram|text|icon|object", "reason": "why this single carrier"}],
    "redundancy_check": {"removed": ["duplicate expressions merged or dropped"], "statement": "confirm no graphic-graphic, text-text, or graphic-text semantic duplication"},
    "hierarchy_plan": {"levels": [{"level": "title|section|body|caption", "font_size": "...", "weight": "...", "color": "..."}], "alignment": "edge/baseline/grid alignment rules", "focus_region": "where the core content sits in the main body area"},
    "implement_prompt": "Long bilingual-capable drawing brief WITH required sections: (1) canvas/layout (2) stage list (3) MODULE DETAIL / 模块细节 per stage listing every leaf module and edges (4) arrows (5) style (6) faithfulness/forbidden/template boundary. Must restate key user terms (OCR/BM25/双塔/重排/Top-K/… when present). Must restate the expression constraints: one carrier per information unit, uniform font size for same-level text, aligned edges/baselines, core content in the main region."
  },
  "quality_checklist": ["detail-preserving", "multi-stage", "faithful", "readable"]
}
Omit `diagram_spec` only for plot/chart. Omit `plot_spec` only for diagram/workflow/comparison/mechanism."""

    @staticmethod
    def _template_analysis_contract() -> str:
        return """Return strict JSON only with professional PPT master analysis:
{
  "template_analysis": {
    "master_style_summary": "...",
    "master_style_spec": {
      "canvas": {"aspect_ratio": "16:9", "orientation": "landscape", "background": "..."},
      "title_region": {"position": "normalized coordinates", "alignment": "...", "hierarchy": "...", "reserved_whitespace": "..."},
      "safe_margins": {"top": "...", "right": "...", "bottom": "...", "left": "...", "body_area": "..."},
      "header_footer": {"page_number": "...", "logo": "...", "corner_marks": "...", "reserved_regions": ["..."]},
      "divider_lines": [{"position": "...", "stroke": "...", "color": "..."}],
      "palette": {"background": "#...", "primary": "#...", "secondary": "#...", "accent": "#...", "neutral": "#...", "forbidden_drift": "..."},
      "typography": {"title": "...", "subtitle": "...", "body": "...", "caption": "...", "alignment": "..."},
      "module_style": {"border": "...", "radius": "...", "fill": "...", "shadow": "...", "spacing": "..."},
      "decorative_elements": ["..."],
      "immutable_elements": ["title region", "page number/logo/corner marks", "divider lines", "background", "palette", "typography", "module/card style"],
      "page_layout_rules": "How A/B/C body skeletons may vary inside safe body area only.",
      "forbidden_deviations": ["moving master elements", "palette drift", "new logo/page number", "changing title region", "overflowing safe margins"]
    },
    "global_constraints": ["..."],
    "immutable_elements": ["..."],
    "page_layout_rules": "..."
  }
}"""

    @classmethod
    def _ppt_outline_contract(cls, page_count: int) -> str:
        return f"""Return strict JSON only. Deck outline mode: return exactly {page_count} lightweight page briefs and a shared prompt. Do not write page-level implement_prompt here.
{{
  "deck_outline": {{
    "deck_title": "Simplified Chinese deck title",
    "deck_goal": "What the deck must communicate to the audience.",
    "narrative_arc": "How page 1..{page_count} progress logically without repetition.",
    "shared_prompt": {{
      "audience": "target audience",
      "tone": "academic presentation tone",
      "global_style_constraints": "Use the extracted template master as immutable; body layouts vary only inside safe area.",
      "terminology": ["consistent key terms"],
      "visual_language": "Use proportionate icons, logo-like/product/object visuals when useful and source-grounded.",
      "emphasis_language": "Use bold or template accent red for 2-5 short phrases per page only."
    }},
    "page_briefs": [
      {{
        "page": 1,
        "title": "Simplified Chinese slide title",
        "role": "cover|problem|method|experiment|result|summary|transition|technical body",
        "main_message": "One-sentence page claim.",
        "content_points": ["3-5 concise content points grounded in material"],
        "suggested_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
        "visual_direction": "Suggested visual/icon/object/chart direction for this page.",
        "transition_from_previous": "How this page connects from previous page, or none for page 1.",
        "transition_to_next": "How this page leads to next page, or closure for last page."
      }}
    ]
  }}
}}"""

    @classmethod
    def _ppt_single_page_contract(cls, page_number: int) -> str:
        return f"""Return strict JSON only. Single-page worker mode: return exactly one page object for page {page_number}. Do not output other pages. Do not put API output parameters such as size, quality, output_format, response_format, aspect_ratio, image_size, thinking_level, or mime_type into prompt text.
{{
  "page": {{
    "page": {page_number},
    "selected_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
    "title": "Simplified Chinese slide title from the current page brief",
    "slide_type": "...",
    "body_layout_plan": "Describe only this page's variable body skeleton inside the safe body area, including text/visual balance.",
    "master_style_binding": {{
      "title_region": "same extracted title region and title hierarchy",
      "safe_margins": "same body safe area and no-overflow margins",
      "header_footer": "same page number/logo/corner marks/header/footer behavior",
      "divider_lines": "same divider/separator geometry and stroke",
      "palette": "same background/primary/accent/neutral colors",
      "typography": "same font hierarchy, weights, sizes, and alignment",
      "module_style": "same card/frame/border/radius/fill/shadow/spacing rhythm",
      "background": "same background treatment"
    }},
    "visual_element_plan": {{
      "usage_decision": "Default to depicting real subjects. Name what will actually be drawn, or explicitly justify why this page is too abstract for any depiction.",
      "elements": [
        {{
          "type": "object/device render|specimen or material illustration|schematic cutaway|scene illustration|product/tool mark|logo-like symbol|semantic icon|none",
          "subject": "what the visual represents",
          "appearance": "Concrete look: overall shape and proportion, dominant materials and colors, defining structural features, typical orientation. The implement model has no other source for this.",
          "source_reference": "source URL/title from Visual Asset Search Context, or 'domain knowledge, generic form' when no source is available",
          "placement": "where it sits inside the safe body area",
          "style": "must be recolored into the template palette and match template line weight, card/border style, and academic restraint",
          "size_ratio": "small|medium|large with approximate body-area percentage"
        }}
      ],
      "text_visual_balance": "Explain how visual elements and text remain proportionate and readable."
    }},
    "emphasis_plan": {{
      "keywords": [{{"text": "short Chinese key phrase", "style": "bold|template-primary-red|template-accent-red", "reason": "why this phrase is emphasized"}}],
      "style_rules": "Highlight only 2-5 short key phrases per page; never mark whole sentences or drift from template palette."
    }},
    "visible_text": ["Simplified Chinese visible text only, short strings"],
    "information_units": [{{"unit": "one information unit from the material", "carrier": "table|chart|diagram|text|icon|object", "reason": "why this single carrier"}}],
    "redundancy_check": {{"removed": ["duplicate expressions merged or dropped"], "statement": "confirm no graphic-graphic, text-text, or graphic-text semantic duplication"}},
    "hierarchy_plan": {{"levels": [{{"level": "title|section|body|caption", "font_size": "...", "weight": "...", "color": "..."}}], "alignment": "edge/baseline/grid alignment rules", "focus_region": "where the core content sits in the main body area"}},
    "implement_prompt": "Page-specific body instructions only. Start with: Create one 16:9 academic PowerPoint-style slide. Describe this page's variable body content, layout skeleton, visual/icon/object/product elements, diagrams/charts, keyword emphasis, and visible Chinese text. Restate the expression constraints: one carrier per information unit, uniform font size for same-level text, aligned edges/baselines, core content in the main region. Do not repeat API output settings. Do not rely on the uploaded image or network images being available to the implement model."
  }}
}}"""
