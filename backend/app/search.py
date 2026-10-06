from __future__ import annotations

import html
import json
import re
from dataclasses import dataclass, field
from typing import Any
from urllib.parse import parse_qs, unquote, urlparse

import httpx

from .adapters import create_async_client, post_json_with_retries, require_api_key, safe_error_message

DUCKDUCKGO_PROTOCOL = "duckduckgo"

# 浏览器整页跳转时会带的头。只带 User-Agent 是不够的：DuckDuckGo 会直接回
# HTTP 202 反爬页（见 duckduckgo_challenge），页面里一个结果都没有，
# 调用方会误以为“搜不到”。
DUCKDUCKGO_HEADERS = {
    "User-Agent": (
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/131.0.0.0 Safari/537.36"
    ),
    "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    "Accept-Language": "en-US,en;q=0.9",
    "Upgrade-Insecure-Requests": "1",
    "Sec-Fetch-Dest": "document",
    "Sec-Fetch-Mode": "navigate",
    "Sec-Fetch-Site": "none",
    "Sec-Fetch-User": "?1",
}


def duckduckgo_challenge(body: str) -> bool:
    """DuckDuckGo 的反爬页：HTTP 202，正文里带 anomaly-modal。

    这是“被挡住”，不是“没有结果”，两者必须分开：前者要让任务看见并失败，
    后者才能安静地变成“没有客观外观描述”。
    """
    lowered = (body or "").lower()
    return "anomaly-modal" in lowered or "bots use duckduckgo too" in lowered
from .models import ModelProfile

SEARCH_RESULT_LINK_PATTERN = re.compile(
    r'<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>(.*?)</a>',
    re.I | re.S,
)
SEARCH_RESULT_SNIPPET_PATTERN = re.compile(
    r'<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>|<td[^>]+class="[^"]*result-snippet[^"]*"[^>]*>(.*?)</td>',
    re.I | re.S,
)


class SearchClientError(RuntimeError):
    pass


@dataclass
class SearchCall:
    """一次检索的结果连同它的报文：失败的调用也要能复盘发出去的是什么。"""

    results: list[dict[str, str]] = field(default_factory=list)
    request: str | None = None
    response: str | None = None
    error: Exception | None = None


class SearchClient:
    """可配置的检索客户端：duckduckgo / tavily / grok_search（OpenAI 兼容 chat
    completions，要求上游模型自己调用 x_search / web_search）。"""

    async def search(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int = 3,
        proxy_url: str | None = None,
    ) -> SearchCall:
        protocol = (profile.protocol or DUCKDUCKGO_PROTOCOL).strip().lower()
        if protocol == DUCKDUCKGO_PROTOCOL:
            return await self._guarded(
                self._duckduckgo(profile, query, max_results=max_results, proxy_url=proxy_url)
            )
        if protocol == "tavily":
            return await self._guarded(
                self._tavily(profile, query, max_results=max_results, proxy_url=proxy_url)
            )
        if protocol in {"openai_chat", "openai_responses", "grok_search"}:
            # openai_responses 走 chat 兼容路径即可
            return await self._openai_chat_search(profile, query, max_results=max_results, proxy_url=proxy_url)
        return SearchCall(error=SearchClientError(f"Unsupported search protocol: {profile.protocol}"))

    @staticmethod
    async def _guarded(awaitable) -> SearchCall:
        """HTML 抓取类协议没有报文可看，失败也交给调用方决定是否降级。"""
        try:
            return SearchCall(results=await awaitable)
        except Exception as exc:
            return SearchCall(error=exc)

    async def _duckduckgo(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int,
        proxy_url: str | None,
    ) -> list[dict[str, str]]:
        from urllib.parse import quote_plus

        base = (profile.base_url or "https://html.duckduckgo.com/html").rstrip("/")
        if base.endswith("/html"):
            url = f"{base}/?q={quote_plus(query)}"
        else:
            url = f"{base}/html/?q={quote_plus(query)}"
        timeout = httpx.Timeout(float(profile.timeout_seconds or 15))
        headers = {**DUCKDUCKGO_HEADERS, **(profile.headers or {})}
        async with create_async_client(timeout, proxy_url) as client:
            response = await client.get(url, headers=headers)
        if response.status_code >= 400:
            raise SearchClientError(f"DuckDuckGo search failed: HTTP {response.status_code}")
        if duckduckgo_challenge(response.text):
            raise SearchClientError(
                "DuckDuckGo answered with its anti-bot challenge instead of results "
                f"(HTTP {response.status_code}); search with tavily or grok_search instead, "
                "or send DuckDuckGo through another proxy."
            )
        return self._parse_duckduckgo_html(response.text, max_results=max_results)

    async def _tavily(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int,
        proxy_url: str | None,
    ) -> list[dict[str, str]]:
        api_key = require_api_key(profile)
        base = (profile.base_url or "https://api.tavily.com").rstrip("/")
        url = f"{base}/search"
        payload = {
            "api_key": api_key,
            "query": query,
            "max_results": max_results,
            "include_answer": False,
            "search_depth": (profile.output_defaults or {}).get("search_depth") or "basic",
        }
        headers = {"Content-Type": "application/json", **(profile.headers or {})}
        timeout = httpx.Timeout(float(profile.timeout_seconds or 30))
        async with create_async_client(timeout, proxy_url) as client:
            response = await client.post(url, json=payload, headers=headers)
        if response.status_code >= 400:
            raise SearchClientError(f"Tavily search failed: HTTP {response.status_code} {response.text[:200]}")
        data = response.json()
        results: list[dict[str, str]] = []
        for item in data.get("results") or []:
            if not isinstance(item, dict):
                continue
            results.append(
                {
                    "title": str(item.get("title") or "").strip(),
                    "url": str(item.get("url") or "").strip(),
                    "snippet": str(item.get("content") or item.get("snippet") or "").strip()[:500],
                }
            )
        return self.filter_results(results, max_results)

    async def _openai_chat_search(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int,
        proxy_url: str | None,
    ) -> SearchCall:
        """
        Search model：OpenAI 兼容 chat 接口。
        适用于带联网能力的中转/Grok 等；模型需返回严格 JSON 结果列表。
        注意：这不是 MCP 进程内调用，而是 HTTP API（可填 xAI / 任意 OpenAI 兼容 base_url）。
        """
        from .adapters import normalize_base_url

        base = normalize_base_url(profile.base_url or "https://api.openai.com", "openai_chat")
        url = f"{base}/chat/completions"
        system = (
            "You are a web search assistant for academic slide visual grounding. "
            "Return strict JSON only: {\"results\":[{\"title\":\"\",\"url\":\"\",\"snippet\":\"\"}]}. "
            f"Return at most {max_results} source-supported results for the exact requested entity. "
            "Each snippet must describe visible shape, structure, colors or distinguishing features supported by "
            "that source, not a link list. Keep the original entity and qualifiers; never replace it with its "
            "vendor, brand or logo. Do not invent appearance. Prefer official docs and product pages. "
            "If no source supports a visual description, return {\"results\":[]}. No markdown fences."
        )
        payload = {
            "model": profile.model or "gpt-4o-mini",
            "temperature": 0.1,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": f"Search query: {query}"},
            ],
        }
        if profile.protocol.strip().lower() == "grok_search":
            try:
                payload.update(grok_search_options(profile))
            except Exception as exc:
                return SearchCall(error=exc)
        # 记下真实发出去的报文（API key 只在 header 里，不在请求体里）。
        request_text = json.dumps(payload, ensure_ascii=False, indent=2)
        try:
            headers = {"Authorization": f"Bearer {require_api_key(profile)}"}
            response = await post_json_with_retries(
                profile,
                url,
                payload,
                headers,
                timeout_seconds=profile.timeout_seconds,
                proxy_url=proxy_url,
            )
        except Exception as exc:
            return SearchCall(request=request_text, error=exc)
        body = response.text
        if response.status_code >= 400:
            return SearchCall(
                request=request_text,
                response=body,
                error=SearchClientError(f"Search model failed: HTTP {response.status_code} {body[:200]}"),
            )
        try:
            data = response.json()
            content = data.get("choices", [{}])[0].get("message", {}).get("content") or ""
            parsed = self._parse_json_results(content)
        except Exception as exc:
            return SearchCall(request=request_text, response=body, error=exc)
        return SearchCall(results=parsed[:max_results], request=request_text, response=body)

    @classmethod
    def _parse_json_results(cls, content: str) -> list[dict[str, str]]:
        text = content.strip()
        text = re.sub(r"^```(?:json)?", "", text).strip()
        text = re.sub(r"```$", "", text).strip()
        try:
            data = json.loads(text)
        except json.JSONDecodeError:
            start = text.find("{")
            end = text.rfind("}")
            if start >= 0 and end > start:
                data = json.loads(text[start : end + 1])
            else:
                raise SearchClientError("Search model did not return valid JSON results") from None
        raw_items = data.get("results") if isinstance(data, dict) else data
        if not isinstance(raw_items, list):
            raise SearchClientError("Search model JSON missing results list")
        results: list[dict[str, str]] = []
        for item in raw_items:
            if not isinstance(item, dict):
                continue
            results.append(
                {
                    "title": str(item.get("title") or "").strip(),
                    "url": str(item.get("url") or "").strip(),
                    "snippet": str(item.get("snippet") or item.get("content") or "").strip()[:500],
                }
            )
        return cls.filter_results(results)

    @classmethod
    def _parse_duckduckgo_html(cls, html_text: str, max_results: int) -> list[dict[str, str]]:
        links = list(SEARCH_RESULT_LINK_PATTERN.finditer(html_text))
        results: list[dict[str, str]] = []
        for index, link in enumerate(links):
            end = links[index + 1].start() if index + 1 < len(links) else len(html_text)
            # Bind the snippet to its own result; a missing snippet must not borrow the next one.
            snippet = SEARCH_RESULT_SNIPPET_PATTERN.search(html_text, link.end(), end)
            results.append({
                "title": cls._clean_search_html(link.group(2)),
                "url": cls._normalize_search_result_url(html.unescape(link.group(1))),
                "snippet": cls._clean_search_html((snippet.group(1) or snippet.group(2)) if snippet else ""),
            })
        return cls.filter_results(results, max_results)

    @staticmethod
    def filter_results(results: list[dict[str, str]], max_results: int | None = None) -> list[dict[str, str]]:
        """Keep usable text evidence and distinct HTTP(S) citations before applying the limit."""
        filtered: list[dict[str, str]] = []
        seen: set[str] = set()
        for item in results:
            url = item.get("url", "").strip()
            snippet = item.get("snippet", "").strip()[:500]
            try:
                parsed = urlparse(url)
                if (parsed.scheme.lower() not in {"http", "https"} or not parsed.hostname
                        or parsed.username is not None or "\\" in url or any(ch.isspace() for ch in url)):
                    continue
                parsed.port  # Reject malformed ports too.
            except ValueError:
                continue
            # A list of bare/Markdown/HTML links is not a visual description.
            prose = re.sub(r"!?\[[^\]]*\]\([^)]*\)", "", snippet)
            prose = re.sub(r"<a\b[^>]*>.*?</a>", "", prose, flags=re.I | re.S)
            prose = re.sub(r"(?:https?://|www\.)\S+", "", prose, flags=re.I)
            prose = re.sub(r"(?im)^\s*(?:[-*•]|\d+[.)、])\s*", "", prose)
            prose = re.sub(r"(?i)\b(?:sources?|references?|links?|citations?|official\s+links?)\s*[:：]?", "", prose)
            prose = re.sub(r"(?:参考(?:资料|链接|来源|文献)?|来源|链接|引用|官网)\s*[:：]?", "", prose)
            if not any(ch.isalpha() for ch in prose):
                continue
            key = parsed._replace(scheme=parsed.scheme.lower(), netloc=parsed.netloc.lower(), fragment="").geturl().rstrip("/")
            if key in seen:
                continue
            seen.add(key)
            filtered.append({"title": item.get("title", "").strip(), "url": url, "snippet": snippet})
        return filtered if max_results is None else filtered[:max(0, max_results)]

    @staticmethod
    def _normalize_search_result_url(url: str) -> str:
        parsed = urlparse(url)
        if "uddg" in parse_qs(parsed.query):
            return unquote(parse_qs(parsed.query)["uddg"][0])
        return url

    @staticmethod
    def _clean_search_html(value: str) -> str:
        text = re.sub(r"<[^>]+>", " ", value or "")
        text = html.unescape(text)
        return re.sub(r"\s+", " ", text).strip()


def grok_search_options(profile: ModelProfile) -> dict[str, Any]:
    model = profile.model or ""
    # grok2api v3 用 `Build/<模型 ID>` 固定 Build 来源，但部署也可能直接暴露自带模型 ID
    # （实测 `grok-build-0.1` 会真的执行 x_search/web_search），这里只挡明显为空的写法；
    # 模型到底支不支持联网工具，由服务端回答（失败会让任务失败，不静默降级）。
    model_id = model[6:] if model.startswith("Build/") else model
    if not model_id.strip() or model != model.strip():
        raise SearchClientError(
            "Grok search requires the deployment's own model ID, for example grok-build-0.1"
        )
    tools = (profile.output_defaults or {}).get("tools", [])
    if not isinstance(tools, list) or any(not isinstance(tool, dict) for tool in tools):
        raise SearchClientError("Grok search tools must be an array of objects")
    tools = list(tools)
    # 上游要求请求体里同时声明两种搜索工具：缺哪个补哪个，用户另配的工具原样保留。
    for kind in ("x_search", "web_search"):
        if not any(tool.get("type") == kind for tool in tools):
            tools.append({"type": kind})
    return {"model": model, "tools": tools, "stream": False, "tool_choice": "required"}


def public_search_error(error: Exception) -> str:
    return safe_error_message(error)
