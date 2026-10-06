import json
import unittest
from unittest.mock import AsyncMock, patch

import httpx

from backend.app.models import ModelProfile
from backend.app.search import SearchClient, SearchClientError, grok_search_options


def profile(protocol="grok_search", model="grok-build-0.1", **defaults):
    return ModelProfile(id="search", role="search", name="Search", protocol=protocol,
                        base_url="https://example.test/v1", model=model,
                        api_key="test", output_defaults=defaults)


def conless_profile(protocol: str) -> ModelProfile:
    """地址留空：用协议自带的默认端点。"""
    return ModelProfile(id="search", role="search", name="Search", protocol=protocol,
                        base_url="", model="", api_key=None, output_defaults={})


class GrokSearchTests(unittest.IsolatedAsyncioTestCase):
    def test_tools(self):
        cases = [
            ([], [{"type": "x_search"}, {"type": "web_search"}]),
            ([{"type": "x_search"}], [{"type": "x_search"}, {"type": "web_search"}]),
            ([{"type": "web_search", "options": {"country": "US"}}],
             [{"type": "web_search", "options": {"country": "US"}}, {"type": "x_search"}]),
            ([{"type": "web_search"}, {"type": "x_search"}], None),
            ([{"type": "function", "function": {"name": "web_search"}}],
             [{"type": "function", "function": {"name": "web_search"}},
              {"type": "x_search"}, {"type": "web_search"}]),
        ]
        for tools, expected in cases:
            with self.subTest(tools=tools):
                original = list(tools)
                result = grok_search_options(profile(tools=tools))
                self.assertEqual(result["tools"], tools if expected is None else expected)
                self.assertEqual(tools, original)
                self.assertIs(result["stream"], False)
                self.assertEqual(result["tool_choice"], "required")
        self.assertEqual(len(grok_search_options(profile())["tools"]), 2)

    def test_invalid_models_and_tools(self):
        for model in ("", " ", "Build/", "Build/ ", " grok-build-0.1", "grok-build-0.1 "):
            with self.subTest(model=model), self.assertRaises(SearchClientError):
                grok_search_options(profile(model=model))
        # 部署直接暴露的模型 ID（例如 grok-build-0.1）必须能用，不能硬要求 Build/ 前缀。
        for model in ("grok-build-0.1", "grok-4.5", "Build/deployed-model"):
            with self.subTest(model=model):
                self.assertEqual(grok_search_options(profile(model=model))["model"], model)
        for tools in (None, {}, "web_search", ["web_search"]):
            with self.subTest(tools=tools), self.assertRaises(SearchClientError):
                grok_search_options(profile(tools=tools))

    async def test_request_and_generic_search_isolation(self):
        for protocol in ("grok_search", "openai_chat"):
            response = httpx.Response(200, json={"choices": [{"message": {"content": '{"results":[]}'}}]})
            with patch("backend.app.search.post_json_with_retries", AsyncMock(return_value=response)) as post:
                call = await SearchClient().search(profile(protocol), "query")
                self.assertIsNone(call.error)
                self.assertEqual(call.results, [])
                args = post.call_args.args
                self.assertEqual(args[1], "https://example.test/v1/chat/completions")
                # 检索阶段只用文本：母版截图与内联数据绝不进入检索请求。
                body = json.dumps(args[2], ensure_ascii=False).lower()
                for marker in ("image_url", "input_image", "base64", "data:image"):
                    self.assertNotIn(marker, body, "检索请求里不该出现图片数据")
                self.assertTrue(all(isinstance(item["content"], str) for item in args[2]["messages"]))
                if protocol == "grok_search":
                    self.assertEqual(args[2]["tool_choice"], "required")
                    self.assertIs(args[2]["stream"], False)
                else:
                    self.assertNotIn("tools", args[2])
                    self.assertNotIn("tool_choice", args[2])


class SearchEvidenceTests(unittest.IsolatedAsyncioTestCase):
    @staticmethod
    def evidence():
        return [
            {"url": "https://example.test/empty", "snippet": " "},
            {"url": "https://example.test/links", "snippet": "1. https://a.test\n2. https://b.test"},
            {"url": "https://example.test/cn", "snippet": "参考链接：https://a.test"},
            {"url": "https://example.test/en", "snippet": "Sources: https://a.test"},
            {"url": "https://example.test/md", "snippet": "[Official page](https://a.test)"},
            {"url": "javascript:alert(1)", "snippet": "silver articulated arms"},
            {"url": "https://", "snippet": "silver articulated arms"},
            {"url": "https://bad host/test", "snippet": "silver articulated arms"},
            {"url": "https://example.test/first", "snippet": "silver articulated arms"},
            {"url": "https://EXAMPLE.test/first#duplicate", "snippet": "duplicate description"},
            {"url": "https://example.test/second", "snippet": "圆形底座与蓝色外壳"},
        ]

    async def test_every_provider_filters_before_limit_and_keeps_trace(self):
        evidence = self.evidence()
        for protocol in ("openai_chat", "openai_responses", "grok_search", "tavily", "duckduckgo"):
            with self.subTest(protocol=protocol):
                if protocol in ("openai_chat", "openai_responses", "grok_search"):
                    response = httpx.Response(200, json={"choices": [{"message": {"content": json.dumps({"results": evidence})}}]})
                    with patch("backend.app.search.post_json_with_retries", AsyncMock(return_value=response)):
                        call = await SearchClient().search(profile(protocol), "gRoK bot", max_results=2)
                    self.assertIsNotNone(call.request)
                    self.assertEqual(call.response, response.text)
                    request = json.loads(call.request)
                    self.assertEqual(request["messages"][1]["content"], "Search query: gRoK bot")
                    system = request["messages"][0]["content"]
                    for phrase in ("source-supported", "Each snippet must describe", "never replace", "Do not invent appearance"):
                        self.assertIn(phrase, system)
                else:
                    client = AsyncMock()
                    if protocol == "tavily":
                        client.post.return_value = httpx.Response(200, json={"results": evidence})
                    else:
                        html = "".join(f'<a class="result__a" href="{item["url"]}">Title</a><a class="result__snippet">{item["snippet"]}</a>' for item in evidence)
                        client.get.return_value = httpx.Response(200, text=html)
                    client.__aenter__.return_value = client
                    with patch("backend.app.search.create_async_client", return_value=client):
                        call = await SearchClient().search(profile(protocol), "gRoK bot", max_results=2)
                    if protocol == "duckduckgo":
                        # 带地址时用它自己的；留空时用官方无 JS 端点
                        #（曾经把尾斜杠的常量又拼一次 /html/）。
                        self.assertEqual(client.get.call_args[0][0], "https://example.test/v1/html/?q=gRoK+bot")
                        for header in ("Sec-Fetch-Dest", "Sec-Fetch-Mode", "Sec-Fetch-Site", "Sec-Fetch-User", "Upgrade-Insecure-Requests"):
                            self.assertIn(header, client.get.call_args[1]["headers"])
                self.assertIsNone(call.error)
                self.assertEqual([item["url"] for item in call.results], ["https://example.test/first", "https://example.test/second"])
                self.assertEqual(call.results[1]["snippet"], "圆形底座与蓝色外壳")

    async def test_default_endpoint_is_the_official_no_js_page(self):
        client = AsyncMock()
        client.get.return_value = httpx.Response(200, text="")
        client.__aenter__.return_value = client
        with patch("backend.app.search.create_async_client", return_value=client):
            await SearchClient().search(conless_profile("duckduckgo"), "grok bot")
        self.assertEqual(client.get.call_args[0][0], "https://html.duckduckgo.com/html/?q=grok+bot")

    async def test_anti_bot_page_is_an_error_not_an_empty_result(self):
        """被挡住不等于没有结果：任务要失败，且错误里点名替代方案。"""
        challenge = '<html><body><div class="anomaly-modal">Unfortunately, bots use DuckDuckGo too.</div></body></html>'
        client = AsyncMock()
        client.get.return_value = httpx.Response(202, text=challenge)
        client.__aenter__.return_value = client
        with patch("backend.app.search.create_async_client", return_value=client):
            call = await SearchClient().search(profile("duckduckgo"), "grok bot")
        self.assertEqual(call.results, [])
        message = str(call.error)
        self.assertIn("anti-bot challenge", message)
        self.assertIn("tavily or grok_search", message)

    def test_missing_ddg_snippet_does_not_borrow_next_description(self):
        html = '<a class="result__a" href="https://example.test/empty">Empty</a><a class="result__a" href="https://example.test/real">Real</a><a class="result__snippet">Two silver arms</a>'
        results = SearchClient._parse_duckduckgo_html(html, 1)
        self.assertEqual(results, [{"title": "Real", "url": "https://example.test/real", "snippet": "Two silver arms"}])

    def test_link_only_content_has_no_evidence(self):
        for snippet in ("", "https://a.test", "1. https://a.test\n2. https://b.test", "参考链接：https://a.test", "Sources: https://a.test", '<a href="https://a.test">Official page</a>'):
            with self.subTest(snippet=snippet):
                self.assertEqual(SearchClient.filter_results([{"url": "https://a.test", "snippet": snippet}]), [])

    def test_truncation_cannot_turn_accepted_evidence_into_bare_links(self):
        snippet = "https://example.test/" + "x" * 600 + " silver arms"
        self.assertEqual(SearchClient.filter_results([{"url": "https://example.test", "snippet": snippet}]), [])

    async def test_failed_call_keeps_request_and_response(self):
        response = httpx.Response(401, text="denied")
        with patch("backend.app.search.post_json_with_retries", AsyncMock(return_value=response)):
            call = await SearchClient().search(profile(), "grokbot")
        self.assertIsNotNone(call.error)
        self.assertEqual(call.response, "denied")
        self.assertIn("grokbot", call.request)
        self.assertEqual(call.results, [])


if __name__ == "__main__":
    unittest.main()
