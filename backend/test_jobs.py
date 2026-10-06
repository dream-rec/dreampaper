"""Simple Mode 端到端回归：走真实流水线，只替换模型客户端与存储依赖。

覆盖 plan 的验收点：提示词与普通模式一致、无 implement 配置也能完成、制图零调用、
普通模式仍然调用、最终提示词写入失败时任务不得成功、多页 PPT 全部保留且按页排列。
"""

from __future__ import annotations

import base64
import json
import logging
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from pydantic import ValidationError

from backend.app.jobs import (
    FIGURE_COMMON_FIELDS,
    MASTER_STYLE_FIELDS,
    PAGE_EMPHASIS_KEYWORD_FIELDS,
    PAGE_MASTER_BINDING_FIELDS,
    PAGE_VISUAL_ELEMENT_FIELDS,
    JobManager,
    VISUAL_CONTEXT_LIMIT,
    VISUAL_SUBJECTS_KEY,
)
from backend.app.models import JobCreateRequest, ModelProfile, PaperFigurePayload, PptSlidePayload
from backend.app.search import SearchCall, SearchClientError

# 替身不打真实网络：失败路径的 warning 不影响断言，压到 ERROR 保持输出干净。
logging.getLogger("backend.app.jobs").setLevel(logging.ERROR)

OPS = ["OCR 版面分析", "双塔编码", "BM25 检索", "交叉重排", "Top-K 证据包", "负样本挖掘", "证据融合", "答案生成"]
TITLE = "面向科研问答的检索增强生成方法"
SECTION = "本文方法包含下列步骤：" + "；".join(f"{index}. {op} 由独立模块完成并保留中间产物" for index, op in enumerate(OPS, 1)) + "。"

# 1x1 PNG：模板图只需要是一个真实存在的文件。
PNG = bytes.fromhex("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da63fcffff3f0300050001a5f645400000000049454e44ae426082")


def profile(role: str, protocol: str) -> ModelProfile:
    return ModelProfile(
        id=f"{role}-default",
        role=role,
        name=f"{role} model",
        protocol=protocol,
        base_url="https://example.test/v1",
        model={"design": "design-model", "implement": "image-model", "search": "duckduckgo-html"}[role],
        api_key=None if role == "implement" else "test-key",
        output_defaults={} if role != "search" else {"max_results": "3"},
    )


class FakeTemplates:
    def __init__(self, image: Path) -> None:
        self.image = image

    def get(self, template_id: str) -> dict:
        return {
            "template_id": template_id,
            "kind": "diagram",
            "category": "architecture",
            "visual_intent": "pipeline",
            "content": "reference layout",
            "path_to_gt_image": str(self.image),
            "additional_info": {"rounded_ratio": "16:9"},
        }

    def _image_path(self, item: dict) -> Path:
        return self.image


class FakeAssets:
    def __init__(self, path: Path) -> None:
        self.path = path

    def get(self, asset_id: str) -> tuple[Path, str]:
        return self.path, "image/png"

    def metadata(self, asset_id: str) -> dict:
        return {"filename": self.path.name}


class FakePrompts:
    def load(self, key: str) -> dict:
        # 提示词内容一律用键名占位：替身只关心“哪份提示词被加载了”。
        return {"key": key, "version": 1, "hash": "test", "content": f"[{key}]"}

    def load_all(self, keys: list[str]) -> list[dict]:
        return [self.load(key) for key in keys]


class FakeConfig:
    """只有真正被要求的角色才会存在；缺 implement 时按真实 ConfigStore 抛错。"""

    ppt_page_plan_concurrency = None
    ppt_image_concurrency = None

    def __init__(self, *profiles: ModelProfile) -> None:
        self.profiles = list(profiles)
        self.requested: list[str] = []

    def load(self) -> "FakeConfig":
        return self

    def proxy_url(self) -> None:
        return None

    def active_profile(self, role: str) -> ModelProfile:
        self.requested.append(role)
        for item in self.profiles:
            if item.role == role:
                return item
        raise ValueError(f"Missing active {role} model profile")


class FakeDesign:
    """设计模型替身：接受顺序回复列表，或按 prompt 判断返回哪份 fixture。

    识别视觉主体是内部前置步骤（系统提示词就是提示词资源的键名），替身在这里回一句
    JSON 数组，因此顺序回复列表只需要覆盖真正的设计步骤：`subjects` 默认空，
    即“模型认为这份资料里没有值得检索外观的主体”。
    """

    def __init__(self, replies, subjects: list[str] | None = None) -> None:
        self.replies = list(replies) if isinstance(replies, list) else replies
        self.subjects = subjects
        self.materials: list[str] = []

    async def generate(self, profile, system_prompt, user_prompt, images, timeout_seconds=None, proxy_url=None) -> str:
        if system_prompt == f"[{VISUAL_SUBJECTS_KEY}]":
            self.materials.append(user_prompt)
            return json.dumps(self.subjects or [], ensure_ascii=False)
        if callable(self.replies):
            return json.dumps(self.replies(user_prompt), ensure_ascii=False)
        if not self.replies:
            raise AssertionError("design model called more often than the fixture allows")
        return json.dumps(self.replies.pop(0), ensure_ascii=False)


class FakeImplement:
    def __init__(self) -> None:
        self.prompts: list[str] = []

    async def generate(self, profile, prompt, reference_images=None, output_overrides=None, proxy_url=None) -> str:
        self.prompts.append(prompt)
        return base64.b64encode(PNG).decode()


class FakeSearch:
    """默认可用的空搜索（带一份报文，供报文日志的用例断言）；给 error 就每次请求都失败。"""

    def __init__(self, error: Exception | None = None) -> None:
        self.error = error
        self.queries: list[str] = []

    async def search(self, profile, query, max_results=3, proxy_url=None):
        self.queries.append(query)
        if self.error is not None:
            raise self.error
        return SearchCall(
            results=[],
            request='{\n  "model": "fake",\n  "tools": [{"type": "x_search"}, {"type": "web_search"}]\n}',
            response='{"choices": [{"message": {"content": "{\\"results\\": []}"}}]}',
        )


def structure_plan() -> dict:
    return {
        "structure_plan": {
            "visual_family": "pipeline",
            "canvas_ratio_hint": "16:9",
            "primary_flow": "left-to-right",
            "lanes_or_stages": [
                {"name": "检索层", "role": "召回候选", "region": "left", "slot_count": 4},
                {"name": "生成层", "role": "融合生成", "region": "right", "slot_count": 4},
            ],
            "module_slots": [
                {"id": f"m{index}", "role": f"抽象槽位 {index}", "stage": "检索层", "region": "left-middle"}
                for index in range(1, 7)
            ],
            "connection_slots": [
                {"from": f"m{index}", "to": f"m{index + 1}", "style": "solid", "meaning_role": "data"}
                for index in range(1, 6)
            ],
            "grouping": "两阶段分层分组",
            "information_density": "high",
            "palette_and_rhythm": "浅色卡片、等距栅格",
            "layout_skeleton": "双层泳道布局，左层为检索流水线，右层为生成流水线，模块以等宽卡片排布并保留正交折线箭头。" * 2,
        }
    }


def figure_design() -> dict:
    modules = [f"{op}" for op in OPS]
    implement_prompt = "\n".join(
        [
            "Publication-quality academic pipeline figure, faithful to the user section with no hallucination.",
            "Keep every grounded operation as a concise short label for readability: legible typography, strong contrast, grouped hierarchy.",
            "Use the template as reference only, never copy the template image.",
            "模块细节 MODULE DETAIL（每个阶段列出叶子步骤）：",
            *[f"- {op}: 作为独立叶子模块绘制，保留其输入输出" for op in OPS],
            "整体为多阶段分层流程：检索层与生成层各自分组，模块之间以正交折线箭头连接，支路与主干明确分离。",
        ]
    )
    figure = {field: f"{field} placeholder" for field in FIGURE_COMMON_FIELDS}
    figure.update(
        {
            "title": TITLE,
            "visual_type": "diagram",
            "aspect_ratio": "inherit",
            "template_usage": "balanced",
            "content_inventory": list(OPS),
            "visible_text": [op.split()[0] for op in OPS],
            "implement_prompt": implement_prompt,
            "information_units": [{"unit": op, "carrier": "模块框"} for op in OPS],
            "redundancy_check": {"statement": "无图文、文文、图图重复表达"},
            "hierarchy_plan": {"levels": ["标题", "阶段", "模块"], "alignment": "栅格左对齐", "focus_region": "中部主干"},
            "diagram_spec": {
                "modules": modules,
                "entities": ["文档块", "索引", "证据包"],
                "connections": [
                    {"source": modules[index], "target": modules[index + 1], "meaning": "数据流"}
                    for index in range(len(modules) - 1)
                ],
                "flow_direction": "left-to-right",
                "grouping_hierarchy": "两阶段分层：检索层与生成层各自分组",
                "arrow_routing": "正交折线",
                "label_strategy": "短标签，覆盖全部叶子操作",
            },
        }
    )
    return {"figure": figure}


def template_analysis() -> dict:
    master = {field: f"master {field}" for field in MASTER_STYLE_FIELDS}
    master.update(
        {
            "canvas": "16:9 白色画布",
            "immutable_elements": ["标题区", "页码", "分隔线"],
            "page_layout_rules": "标题居中，正文两栏",
            "forbidden_deviations": "不得新增角标或渐变",
        }
    )
    return {
        "template_analysis": {
            "master_style_spec": master,
            "global_constraints": "保持母版字体层级与留白",
            "immutable_elements": ["标题区", "页码", "分隔线"],
            "page_layout_rules": "标题居中，正文两栏",
        }
    }


def deck_outline(page_count: int) -> dict:
    return {
        "deck_outline": {
            "deck_title": TITLE,
            "deck_goal": "讲清方法流程",
            "narrative_arc": "问题—方法—结果",
            "shared_prompt": "整册保持母版风格",
            "page_briefs": [
                {
                    "page": page,
                    "title": f"第 {page} 页标题",
                    "role": "方法说明",
                    "main_message": f"第 {page} 页核心信息",
                    "content_points": [f"要点 {page}A", f"要点 {page}B"],
                    "suggested_template": "tabbed",
                    "visual_direction": "一个可辨识的示意图",
                    "transition_from_previous": "承接上一页",
                    "transition_to_next": "引向下一页",
                }
                for page in range(1, page_count + 1)
            ],
        }
    }


def ppt_page(page: int) -> dict:
    element = {field: f"element {field}" for field in PAGE_VISUAL_ELEMENT_FIELDS}
    element["size_ratio"] = "0.3"
    keyword = {field: f"keyword {field}" for field in PAGE_EMPHASIS_KEYWORD_FIELDS}
    keyword["text"] = "关键短语"
    return {
        "page": page,
        "selected_template": "tabbed",
        "title": f"第 {page} 页标题",
        "slide_type": "content",
        "body_layout_plan": "顶部标题，左文右图，底部留白",
        "master_style_binding": {field: f"binding {field}" for field in PAGE_MASTER_BINDING_FIELDS},
        "visual_element_plan": {
            "usage_decision": "使用一个可辨识的示意图",
            "elements": [element],
            "text_visual_balance": "文字与图各占一半",
        },
        "emphasis_plan": {"keywords": [keyword], "style_rules": "仅对关键短语加粗"},
        "information_units": [{"unit": f"第 {page} 页信息单元", "carrier": "正文段落"}],
        "redundancy_check": {"statement": "同一信息只用一种载体表达"},
        "hierarchy_plan": {"levels": ["标题", "小标题", "正文"], "alignment": "左对齐栅格", "focus_region": "左侧正文区"},
        "visible_text": [f"第 {page} 页标题", "要点"],
        "implement_prompt": (
            f"按母版规范绘制第 {page} 页幻灯片：顶部标题居中并保持母版字体层级，正文左对齐、行距均匀，"
            "右侧绘制一个可辨识的示意图，配色、卡片样式与分隔线都与母版一致，"
            "只保留本页给定的标题与页码位置，不新增页码、角标、渐变或额外装饰元素，"
            "关键短语按强调计划加粗，其余文字保持常规字重。"
        ),
    }


def ppt_page_reply(page: int) -> dict:
    return {"page": ppt_page(page)}


def ppt_design(pages: int):
    """按 prompt 分辨模板分析 / 整套大纲 / 单页规划：单页是并发调用的，顺序不可依赖。"""

    def reply(user_prompt: str) -> dict:
        if "Deck outline mode" in user_prompt:
            return deck_outline(pages)
        if "Single-page worker mode" in user_prompt:
            marker = "## Current Page Brief\n"
            start = user_prompt.index(marker) + len(marker)
            brief = json.loads(user_prompt[start : user_prompt.index("\n\n## ", start)])
            return ppt_page_reply(int(brief["page"]))
        return template_analysis()

    return reply


class JobManagerTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.template_image = self.tmp / "tpl.png"
        self.template_image.write_bytes(PNG)
        self.material = self.tmp / "material.txt"
        self.material.write_text("资料正文", encoding="utf-8")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def manager(
        self, config: FakeConfig, replies, prompts: FakePrompts | None = None, subjects: list[str] | None = None
    ) -> JobManager:
        manager = JobManager(FakeTemplates(self.template_image), FakeAssets(self.material), prompts or FakePrompts(), config)
        manager.root = self.tmp / "jobs"
        manager.design = FakeDesign(replies, subjects)
        manager.implement = FakeImplement()
        manager.search = FakeSearch()
        return manager

    def figure_request(self, simple_mode: bool) -> JobCreateRequest:
        return JobCreateRequest(
            mode="paper_figure",
            simple_mode=simple_mode,
            payload=PaperFigurePayload(figure_title=TITLE, section_description=SECTION, template_ids=["t1"]),
        )

    def slide_request(self, simple_mode: bool, pages: int, material: str = "资料正文") -> JobCreateRequest:
        return JobCreateRequest(
            mode="ppt_slide",
            simple_mode=simple_mode,
            payload=PptSlidePayload(template_asset_id="a1", material_text=material, page_count=pages),
        )

    def run_job(self, manager: JobManager, request: JobCreateRequest) -> str:
        record = manager.create(request)
        return record.id


class SimpleModeFigureTests(JobManagerTestCase):
    def test_simple_mode_keeps_the_prompt_and_skips_the_drawing_model(self) -> None:
        import asyncio

        normal = self.manager(FakeConfig(profile("design", "openai_chat"), profile("implement", "image2")), [structure_plan(), figure_design()])
        normal_id = self.run_job(normal, self.figure_request(simple_mode=False))
        asyncio.run(normal.run(normal_id, self.figure_request(simple_mode=False)))
        normal_record = normal.get(normal_id)
        self.assertEqual(normal_record.status, "succeeded")
        self.assertEqual(len(normal_record.images), 1)
        self.assertEqual(len(normal.implement.prompts), 1)

        config = FakeConfig(profile("design", "openai_chat"), profile("implement", "image2"))
        simple = self.manager(config, [structure_plan(), figure_design()])
        request = self.figure_request(simple_mode=True)
        simple_id = self.run_job(simple, request)
        asyncio.run(simple.run(simple_id, request))
        record = simple.get(simple_id)

        self.assertEqual(record.status, "succeeded")
        self.assertEqual(record.images, [])
        self.assertEqual(simple.implement.prompts, [], "Simple Mode 不得调用制图模型")
        self.assertEqual(config.requested, ["design", "search"], "Simple Mode 不得读取 implement 配置")
        prompts = [log.content for log in record.design_logs if log.step.startswith("final_prompt_")]
        self.assertEqual(prompts, normal.implement.prompts, "两种模式交给制图适配器的提示词必须一致")
        self.assertEqual([log.step for log in record.design_logs], ["final_prompt_1"])

    def test_simple_mode_needs_no_implement_profile_at_all(self) -> None:
        import asyncio

        config = FakeConfig(profile("design", "openai_chat"))
        manager = self.manager(config, [structure_plan(), figure_design()])
        request = self.figure_request(simple_mode=True)
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        self.assertEqual(manager.get(job_id).status, "succeeded")
        self.assertNotIn("implement", config.requested)

    def test_normal_mode_still_requires_the_implement_profile(self) -> None:
        import asyncio

        config = FakeConfig(profile("design", "openai_chat"))
        manager = self.manager(config, [structure_plan()])
        request = self.figure_request(simple_mode=False)
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        self.assertEqual(manager.get(job_id).status, "failed")
        self.assertIn("implement", config.requested)

    def test_final_prompt_that_cannot_be_stored_never_completes(self) -> None:
        import asyncio

        manager = self.manager(FakeConfig(profile("design", "openai_chat")), [structure_plan(), figure_design()])
        request = self.figure_request(simple_mode=True)
        job_id = self.run_job(manager, request)
        real_persist = JobManager._persist
        failed_writes: list[list[str]] = []

        def failing_persist(self, record):
            # 只在写最终提示词的那一次落盘失败：其他写入照常，确保失败确实来自这一步。
            final_steps = [log.step for log in record.design_logs if log.step.startswith("final_prompt_")]
            if final_steps:
                failed_writes.append(final_steps)
                raise OSError("disk full")
            return real_persist(self, record)

        with patch.object(JobManager, "_persist", failing_persist):
            asyncio.run(manager.run(job_id, request))
        record = manager.get(job_id)
        self.assertEqual(failed_writes, [["final_prompt_1"]], "故障必须由写最终提示词的那次落盘触发")
        self.assertEqual(record.status, "failed")
        self.assertIn("disk full", record.error.summary)
        self.assertFalse([log for log in record.design_logs if log.step.startswith("final_prompt_")])


class SimpleModePptTests(JobManagerTestCase):
    def replies(self, pages: int):
        return ppt_design(pages)

    def test_simple_mode_keeps_every_page_in_order(self) -> None:
        import asyncio

        config = FakeConfig(profile("design", "openai_chat"), profile("implement", "image2"), profile("search", "duckduckgo"))
        manager = self.manager(config, self.replies(3))
        request = self.slide_request(simple_mode=True, pages=3)
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        record = manager.get(job_id)

        self.assertEqual(record.status, "succeeded")
        self.assertEqual(record.images, [])
        self.assertEqual(manager.implement.prompts, [])
        steps = [log.step for log in record.design_logs]
        self.assertEqual(steps, ["ppt_visual_assets", "final_prompt_1", "final_prompt_2", "final_prompt_3"])
        for index, log in enumerate((entry for entry in record.design_logs if entry.step.startswith("final_prompt_")), 1):
            self.assertIn(f"第 {index} 页标题", log.content)
        self.assertEqual(manager.get(job_id).stage, "completed")

        # 同一组页面 fixture 走普通模式：并发设为 1 使调用顺序就是页序，
        # 再逐页比较交给制图适配器的提示词。
        normal_config = FakeConfig(profile("design", "openai_chat"), profile("implement", "image2"), profile("search", "duckduckgo"))
        normal_config.ppt_image_concurrency = 1
        normal = self.manager(normal_config, self.replies(3))
        normal_request = self.slide_request(simple_mode=False, pages=3)
        normal_id = self.run_job(normal, normal_request)
        asyncio.run(normal.run(normal_id, normal_request))
        self.assertEqual(normal.get(normal_id).status, "succeeded")
        self.assertEqual(
            [log.content for log in record.design_logs if log.step.startswith("final_prompt_")],
            normal.implement.prompts,
                         "两种模式交给制图适配器的提示词必须逐页一致")

    def test_normal_mode_still_renders_every_page(self) -> None:
        import asyncio

        config = FakeConfig(profile("design", "openai_chat"), profile("implement", "image2"), profile("search", "duckduckgo"))
        manager = self.manager(config, self.replies(2))
        request = self.slide_request(simple_mode=False, pages=2)
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        record = manager.get(job_id)
        self.assertEqual(record.status, "succeeded")
        self.assertEqual(len(record.images), 2)
        self.assertEqual(len(manager.implement.prompts), 2)
        self.assertEqual([log.step for log in record.design_logs], ["ppt_visual_assets"])


class VisualAssetSearchTests(JobManagerTestCase):
    """搜索失败的处理：grok_search 必须让任务失败，其他协议照旧降级。"""

    SUBJECTS = ["Docker"]
    MATERIAL = "本方案使用 Docker 构建部署流程，并在论文中画出部署拓扑。"

    def search_manager(self, protocol: str, error: Exception | None) -> JobManager:
        design = profile("design", "openai_chat")
        search = profile("search", protocol)
        if protocol == "grok_search":
            search = search.model_copy(update={"model": "Build/deployed-model"})
        manager = self.manager(FakeConfig(design, search), ppt_design(1), subjects=self.SUBJECTS)
        manager.search = FakeSearch(error)
        return manager

    def run_slide(self, manager: JobManager) -> str:
        import asyncio

        request = self.slide_request(simple_mode=True, pages=1, material=self.MATERIAL)
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        return job_id

    def test_grok_search_failure_fails_the_job(self) -> None:
        manager = self.search_manager("grok_search", SearchClientError("Search model failed: HTTP 401 invalid Build route"))
        job_id = self.run_slide(manager)
        record = manager.get(job_id)
        self.assertTrue(manager.search.queries, "识别出主体后必须真的发起检索")
        self.assertEqual(record.status, "failed")
        self.assertIn("401", record.error.summary)
        self.assertFalse([log for log in record.design_logs if log.step.startswith("final_prompt_")])

    def test_every_search_protocol_fails_the_job_instead_of_degrading(self) -> None:
        """被限流/被反爬/路由错了都要让任务失败，不能伪装成“没有来源”。"""
        for protocol in ("grok_search", "duckduckgo", "tavily"):
            with self.subTest(protocol=protocol):
                manager = self.search_manager(protocol, SearchClientError("Search model failed: HTTP 500"))
                job_id = self.run_slide(manager)
                self.assertTrue(manager.search.queries, "识别出主体后必须真的发起检索")
                record = manager.get(job_id)
                self.assertEqual(record.status, "failed")
                self.assertIn("500", record.error.summary)
                self.assertFalse([log for log in record.design_logs if log.step.startswith("final_prompt_")])

    def test_search_traces_are_logged_for_the_panel(self) -> None:
        manager = self.search_manager("grok_search", None)
        job_id = self.run_slide(manager)
        record = manager.get(job_id)
        traces = {log.step: log.content for log in record.design_logs if log.step.startswith("search_")}
        # 每次检索都留一对：发出去的请求体与拿回来的原文。
        self.assertEqual(sorted(traces), ["search_request_1", "search_response_1"])
        self.assertIn("x_search", traces["search_request_1"])
        self.assertIn("web_search", traces["search_request_1"])
        self.assertNotIn("api_key", traces["search_request_1"])
        self.assertIn("choices", traces["search_response_1"])
        # 检索阶段只用文本：传进去的是资料文本，没有任何图片/内联数据。
        self.assertTrue(all(isinstance(query, str) for query in manager.search.queries))
        self.assertTrue(all("data:image" not in query for query in manager.search.queries))


class FigureVisualAssetSearchTests(JobManagerTestCase):
    """科研图与幻灯片同源：配了检索模型就联网查询，没配置只是跳过。"""

    SUBJECTS = ["Docker"]
    MATERIAL = "本方案使用 Docker 构建部署流程，并在论文中画出部署拓扑。"

    def figure_with_material(self) -> JobCreateRequest:
        return JobCreateRequest(
            mode="paper_figure",
            simple_mode=True,
            payload=PaperFigurePayload(
                figure_title="Docker 部署流程",
                section_description=self.MATERIAL,
                template_ids=["t1"],
            ),
        )

    def test_configured_search_runs_and_is_logged(self) -> None:
        import asyncio

        manager = self.manager(
            FakeConfig(profile("design", "openai_chat"), profile("search", "duckduckgo")),
            [structure_plan(), figure_design()],
            subjects=self.SUBJECTS,
        )
        manager.search = FakeSearch(None)
        request = self.figure_with_material()
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        record = manager.get(job_id)

        self.assertEqual(record.status, "succeeded")
        self.assertTrue(manager.search.queries, "识别出主体后科研图也要发起检索")
        steps = {log.step for log in record.design_logs}
        self.assertIn("search_request_1", steps)
        self.assertIn("search_response_1", steps)
        self.assertTrue(
            all(
                log.label.startswith("联网查询")
                for log in record.design_logs
                if log.step.startswith("search_")
            )
        )

    def test_without_a_search_profile_the_figure_still_renders(self) -> None:
        import asyncio

        manager = self.manager(
            FakeConfig(profile("design", "openai_chat")),
            [structure_plan(), figure_design()],
        )
        request = self.figure_with_material()
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        record = manager.get(job_id)

        self.assertEqual(record.status, "succeeded")
        self.assertFalse([log for log in record.design_logs if log.step.startswith("search_")])


class GroundedSubjectTests(JobManagerTestCase):
    """识别视觉主体：只认资料原文里的字符串，模型报什么都不越界。"""

    def test_subjects_keep_the_spelling_of_the_material(self):
        material = "本方案用 muse 与 dots 两个模型，部署在 Kubernetes 上。"
        reply = json.dumps(["muse", "dots", "Kubernetes"], ensure_ascii=False)
        self.assertEqual(JobManager._visual_subjects_from_reply(reply, material), ["muse", "dots", "Kubernetes"])
        # 模型把名字规范化了也算命中：比对忽略大小写、空格与标点。
        self.assertEqual(
            JobManager._visual_subjects_from_reply(
                '```json\n["Grok Bot", "dots.llm1"]\n```', "介绍 grokbot 和 dots.llm1"
            ),
            ["Grok Bot", "dots.llm1"],
        )

    def test_subjects_absent_from_the_material_are_dropped(self):
        # 识别只看资料原文：约束里的字体与色值不在其中，模型就算报也进不了检索。
        material = "图中展示 Grok Bot 的界面。"
        self.assertEqual(
            JobManager._visual_subjects_from_reply(
                json.dumps(["Microsoft", "FFFFFF", "微软雅黑", "Grok Bot"], ensure_ascii=False), material
            ),
            ["Grok Bot"],
        )
        for reply in ("", "没有需要检索的主体", '["unclosed', "[]", json.dumps([{"term": "Grok Bot"}])):
            self.assertEqual(JobManager._visual_subjects_from_reply(reply, material), [])

    def test_subjects_are_deduped_and_capped(self):
        material = " ".join(f"subject{index}" for index in range(1, 12))
        reply = json.dumps([f"subject{index}" for index in range(1, 12)])
        self.assertEqual(
            JobManager._visual_subjects_from_reply(reply, material),
            [f"subject{index}" for index in range(1, 9)],
        )
        self.assertEqual(JobManager._visual_subjects_from_reply(json.dumps(["muse", "Muse"]), "muse"), ["muse"])

    def test_raw_subject_reaches_the_search_payload_unchanged(self):
        import asyncio
        import httpx
        from unittest.mock import AsyncMock
        from backend.app.search import SearchClient

        # 缺凭据时请求在发出去之前就会失败，测试不依赖真实网络。
        for raw in ("grokbot", "grok bot", "gRoKbOt", "gRoK   bot", "myGrokbot_v2", "cuda_model", "sdxl-based"):
            with self.subTest(raw=raw):
                manager = self.manager(FakeConfig(profile("search", "grok_search")), [])
                manager.search = SearchClient()
                response = httpx.Response(200, json={"choices": [{"message": {"content": '{"results":[]}'}}]})
                with patch("backend.app.search.post_json_with_retries", AsyncMock(return_value=response)) as post:
                    context = asyncio.run(manager._build_visual_asset_context([raw]))
                self.assertEqual(context["terms"], [raw])
                self.assertEqual(post.call_args.args[2]["messages"][1]["content"], f"Search query: {raw}")
                self.assertEqual(context["items"][0]["query"], raw)
                self.assertEqual(context["items"][0]["term"], raw)
                self.assertNotIn("xAI", manager._visual_asset_context_text(context))
                self.assertTrue(context["degraded"])
                self.assertIn("unavailable", manager._visual_asset_context_text(context))

    def test_fair_context_budget_preserves_every_description_and_reference(self):
        for long_names in (False, True):
            terms = [f"subject{index}" + ("名" * 10000 if long_names else "") for index in range(8)]
            context = {"terms": terms, "items": [{"term": term, "results": [
                {"title": "Official source", "url": f"https://example.test/{index}/{source}/" + "x" * 10000,
                 "snippet": f"description-{index}-{source} silver arms " + "feature " * 100}
                for source in range(3)]} for index, term in enumerate(terms)]}
            text = JobManager._visual_asset_context_text(context)
            self.assertLessEqual(len(text), VISUAL_CONTEXT_LIMIT)
            full = JobManager._visual_asset_log_text(context)
            for index in range(8):
                self.assertIn(f"subject{index}", text)
                for source in range(3):
                    self.assertIn(f"description-{index}-{source}", text)
                    self.assertIn(f"[{index + 1}.{source + 1}]", text)
                    self.assertIn(context["items"][index]["results"][source]["url"], full)
            context["items"][-1]["results"] = []
            self.assertIn("unavailable", JobManager._visual_asset_context_text(context))

    def test_picker_reads_material_but_never_constraints(self):
        import asyncio
        from backend.app.search import SearchCall

        for mode in ("paper_figure", "ppt_slide"):
            with self.subTest(mode=mode):
                captured = []
                figure_replies = iter([structure_plan(), figure_design()])
                def reply(prompt):
                    captured.append(prompt)
                    return next(figure_replies) if mode == "paper_figure" else ppt_design(1)(prompt)
                manager = self.manager(
                    FakeConfig(profile("design", "openai_chat"), profile("search", "grok_search")),
                    reply,
                    subjects=["gRoK bot"],
                )
                class GroundingSearch(FakeSearch):
                    async def search(self, profile, query, **kwargs):
                        self.queries.append(query)
                        return SearchCall(results=[{"title": "Source", "url": "https://example.test/robot", "snippet": "Silver arms and a round base"}], request="raw request", response="raw response")
                manager.search = GroundingSearch()
                request = self.figure_request(True) if mode == "paper_figure" else self.slide_request(True, 1, "在图中展示gRoK bot")
                update = {"custom_prompt": "标题用微软雅黑，底色 #FFFFFF"}
                if mode == "paper_figure":
                    update["section_description"] = "在图中展示gRoK bot"
                request.payload = request.payload.model_copy(update=update)
                job_id = self.run_job(manager, request)
                asyncio.run(manager.run(job_id, request))
                record = manager.get(job_id)
                self.assertEqual(record.status, "succeeded", record.error)
                # 识别主体的输入是资料/方法，不是约束：字体与色值根本没进这一步。
                material = manager.design.materials[-1]
                self.assertIn("gRoK bot", material)
                self.assertNotIn("微软雅黑", material)
                self.assertNotIn("FFFFFF", material)
                self.assertEqual(manager.search.queries, ["gRoK bot"])
                planners = [prompt for prompt in captured if "Visual Asset Search Context" in prompt]
                self.assertTrue(planners)
                for prompt in planners:
                    self.assertIn("Silver arms and a round base", prompt)
                    self.assertIn("https://example.test/robot", prompt)
                    self.assertIn("Subject: gRoK bot", prompt)
                    self.assertNotIn("xAI", prompt)
                evidence = [log for log in record.design_logs if log.step.endswith("_visual_assets")]
                self.assertEqual(len(evidence), 1)
                self.assertIn("Silver arms and a round base", evidence[0].content)
                self.assertIn("https://example.test/robot", evidence[0].content)

    def test_failed_transport_traces_are_retained_and_marked_failed(self):
        import asyncio
        manager = self.manager(
            FakeConfig(profile("design", "openai_chat"), profile("search", "grok_search")),
            [],
            subjects=["grokbot"],
        )
        class FailedSearch(FakeSearch):
            async def search(self, *args, **kwargs):
                return SearchCall(request="raw failed request", response="raw denied response", error=SearchClientError("HTTP 401"))
        manager.search = FailedSearch()
        request = self.slide_request(True, 1, "请画grokbot")
        job_id = self.run_job(manager, request)
        asyncio.run(manager.run(job_id, request))
        record = manager.get(job_id)
        self.assertEqual(record.status, "failed")
        traces = [log for log in record.design_logs if log.step.startswith("search_")]
        self.assertEqual([log.content for log in traces], ["raw failed request", "raw denied response"])
        self.assertTrue(all(log.status == "failed" for log in traces))


class RequestEnvelopeTests(unittest.TestCase):
    def test_missing_flag_is_ordinary_mode_and_non_boolean_is_rejected(self) -> None:
        payload = PaperFigurePayload(figure_title=TITLE, section_description=SECTION, template_ids=["t1"])
        request = JobCreateRequest(mode="paper_figure", payload=payload)
        self.assertIs(request.simple_mode, False)
        with self.assertRaises(ValidationError):
            JobCreateRequest(mode="paper_figure", simple_mode="true", payload=payload)
        with self.assertRaises(ValidationError):
            JobCreateRequest(mode="paper_figure", simple_mode=1, payload=payload)


if __name__ == "__main__":
    unittest.main()
