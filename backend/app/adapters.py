from __future__ import annotations

import asyncio
import base64
import html
import json
import re
from typing import Any
from urllib.parse import urlparse

import httpx

from .models import ModelProfile


class ModelAdapterError(RuntimeError):
    def __init__(
        self,
        message: str,
        *,
        profile: ModelProfile | None = None,
        endpoint: str | None = None,
        http_status: int | None = None,
        suggestion: str | None = None,
        code: str = "model_request_failed",
    ) -> None:
        super().__init__(message)
        self.profile = profile
        self.endpoint = endpoint
        self.http_status = http_status
        self.suggestion = suggestion
        self.code = code

    def diagnostic(self, stage: str | None = None) -> dict[str, Any]:
        profile = self.profile
        return {
            "summary": str(self),
            "code": self.code,
            "stage": stage,
            "role": profile.role if profile else None,
            "profile_id": profile.id if profile else None,
            "profile_name": profile.name if profile else None,
            "protocol": ("banana2" if profile.protocol == "banna2" else profile.protocol) if profile else None,
            "model": profile.model if profile else None,
            "base_url": profile.base_url if profile else None,
            "endpoint": self.endpoint,
            "http_status": self.http_status,
            "suggestion": self.suggestion,
        }


SENSITIVE_HEADER_NAMES = {"authorization", "x-api-key", "x-goog-api-key"}
RETRY_STATUS_CODES = {429, 500, 502, 503, 504}
# 制图网关（如 WisArt）同步出图时 nginx 常在 60–300s 返回 502，需更长读超时与退避重试
DEFAULT_CONNECT_TIMEOUT_SECONDS = 30.0
MIN_IMAGE_TIMEOUT_SECONDS = 120
MAX_BACKOFF_SECONDS = 30


def _redact_header_value(name: str, value: str) -> str:
    if name.lower() not in SENSITIVE_HEADER_NAMES:
        return value
    return "<redacted>"


def redact_headers(headers: dict[str, str]) -> dict[str, str]:
    return {key: _redact_header_value(key, value) for key, value in headers.items()}


def public_profile_snapshot(profile: ModelProfile) -> dict[str, Any]:
    return {
        "id": profile.id,
        "role": profile.role,
        "name": profile.name,
        "protocol": "banana2" if profile.protocol == "banna2" else profile.protocol,
        "base_url": profile.base_url,
        "model": profile.model,
        "api_version": profile.api_version,
        "headers": {key: _redact_header_value(key, value) for key, value in profile.headers.items()},
        "timeout_seconds": profile.timeout_seconds,
        "max_retries": profile.max_retries,
        "output_defaults": profile.output_defaults,
        "has_api_key": bool(profile.api_key),
    }


def normalize_base_url(base_url: str, protocol: str) -> str:
    base = base_url.rstrip("/")
    if protocol in {"banana2", "banna2"}:
        return base
    parsed = urlparse(base)
    if re.search(r"/v\d+(?:beta)?(?:/|$)", parsed.path):
        return base
    return f"{base}/v1"


def require_api_key(profile: ModelProfile) -> str:
    if not profile.api_key:
        raise ModelAdapterError(
            f"{profile.role.capitalize()} 配置“{profile.name}”缺少 API key。",
            profile=profile,
            suggestion="请在设置中填写该配置的 API key 并保存。",
            code="missing_api_key",
        )
    return profile.api_key


def safe_error_message(error: Exception) -> str:
    message = str(error).strip()
    if not message:
        message = error.__class__.__name__
    else:
        message = f"{error.__class__.__name__}: {message}"
    message = re.sub(r"Bearer\s+[A-Za-z0-9._~+/=-]+", "Bearer <redacted>", message)
    message = re.sub(r"(?i)(api[_-]?key|x-api-key|x-goog-api-key)(['\"\s:=]+)([^'\"\s,&}]+)", r"\1\2<redacted>", message)
    return message[:800]


def data_url(mime_type: str, b64: str) -> str:
    return f"data:{mime_type};base64,{b64}"


def image_to_b64(path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def build_timeout(timeout_seconds: int | float | None, *, minimum: int | None = None) -> httpx.Timeout:
    """分离 connect / read 超时：连接宜短，读超时覆盖同步出图等待。"""
    read = float(timeout_seconds or 120)
    if minimum is not None:
        read = max(read, float(minimum))
    return httpx.Timeout(
        connect=DEFAULT_CONNECT_TIMEOUT_SECONDS,
        read=read,
        write=min(60.0, read),
        pool=DEFAULT_CONNECT_TIMEOUT_SECONDS,
    )


def create_async_client(timeout: httpx.Timeout, proxy_url: str | None = None) -> httpx.AsyncClient:
    # 代理只认设置里填的值：trust_env=False 让 shell 里的 HTTP_PROXY / HTTPS_PROXY /
    # ALL_PROXY 不再被自动采用（残留代理会让请求瞬间连到不存在的本地端口）。
    return httpx.AsyncClient(timeout=timeout, proxy=proxy_url or None, trust_env=False)


def format_transport_error(error: httpx.TransportError, proxy_url: str | None = None) -> str:
    error_type = error.__class__.__name__
    if proxy_url:
        return (
            "模型请求网络错误：无法通过已配置代理建立连接。"
            "请检查设置中的代理地址和代理服务，或清空代理后直连。"
            f"（{error_type}）"
        )
    return (
        "模型请求网络错误：无法连接模型服务。"
        "请检查模型 Base URL、DNS 和网络连接。"
        f"（{error_type}）"
    )


def retry_backoff_seconds(attempt: int, status_code: int | None = None) -> float:
    base = min(2**attempt, MAX_BACKOFF_SECONDS)
    # 网关 502/503 往往表示上游仍在出图或短暂过载，多等一会再重试
    if status_code in {502, 503, 504}:
        return min(10 * (attempt + 1), MAX_BACKOFF_SECONDS)
    if status_code == 429:
        return min(15 * (attempt + 1), MAX_BACKOFF_SECONDS)
    return float(base)


def response_error_summary(body: str) -> str:
    raw = (body or "").strip()
    if not raw:
        return ""
    try:
        data = json.loads(raw)
        if isinstance(data, dict):
            error = data.get("error")
            if isinstance(error, dict):
                candidate = error.get("message") or error.get("detail") or error.get("type")
            else:
                candidate = error or data.get("message") or data.get("detail")
            if candidate:
                return re.sub(r"\s+", " ", str(candidate)).strip()[:240]
    except (TypeError, ValueError):
        pass
    title = re.search(r"<title[^>]*>(.*?)</title>", raw, flags=re.I | re.S)
    if title:
        return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", title.group(1)))).strip()[:240]
    text = html.unescape(re.sub(r"<[^>]+>", " ", raw))
    return re.sub(r"\s+", " ", text).strip()[:240]


def http_error_suggestion(role: str, status_code: int) -> str:
    if status_code in {502, 503, 504}:
        if role == "implement":
            return "上游制图网关异常。可稍后重试，并检查 Implement 地址、超时和重试次数。"
        return f"上游模型网关异常。请稍后重试；若持续出现，请检查 {role.capitalize()} 地址或更换中转服务。"
    if status_code == 401:
        return f"请检查 {role.capitalize()} 配置的 API key。"
    if status_code == 403:
        return f"请检查 {role.capitalize()} 配置的 API key、模型权限和服务商访问策略。"
    if status_code == 404:
        return f"请检查 {role.capitalize()} 的 Base URL、协议和模型名。"
    if status_code == 429:
        return "请求频率或额度受限。请稍后重试，并检查账户额度。"
    return f"请检查 {role.capitalize()} 的 Base URL、协议、模型名和服务商状态。"


def format_http_error(kind: str, status_code: int, body: str) -> str:
    detail = response_error_summary(body)
    suffix = f" 服务返回：{detail}" if detail else ""
    if status_code == 502:
        return f"{kind} 请求失败：HTTP 502，上游网关无法完成请求。{suffix}"
    if status_code in {503, 504}:
        return f"{kind} 请求失败：HTTP {status_code}，上游网关维护、超时或无法连接模型服务。{suffix}"
    return f"{kind} 请求失败：HTTP {status_code}。{suffix}"


async def post_json_with_retries(
    profile: ModelProfile,
    url: str,
    payload: dict[str, Any],
    headers: dict[str, str],
    timeout_seconds: int | None = None,
    proxy_url: str | None = None,
    *,
    minimum_timeout: int | None = None,
) -> httpx.Response:
    merged_headers = {"Content-Type": "application/json", **profile.headers, **headers}
    attempts = max(1, profile.max_retries + 1)
    effective_timeout = timeout_seconds or profile.timeout_seconds
    timeout = build_timeout(effective_timeout, minimum=minimum_timeout)
    last_error: httpx.TimeoutException | httpx.TransportError | None = None
    last_response: httpx.Response | None = None

    for attempt in range(attempts):
        try:
            async with create_async_client(timeout, proxy_url) as client:
                response = await client.post(url, json=payload, headers=merged_headers)
            last_response = response
            if response.status_code not in RETRY_STATUS_CODES or attempt == attempts - 1:
                return response
            await asyncio.sleep(retry_backoff_seconds(attempt, response.status_code))
            continue
        except httpx.TimeoutException as exc:
            last_error = exc
            if attempt == attempts - 1:
                break
        except httpx.TransportError as exc:
            last_error = exc
            if attempt == attempts - 1:
                break
        await asyncio.sleep(retry_backoff_seconds(attempt))

    if isinstance(last_error, httpx.TimeoutException):
        raise ModelAdapterError(
            f"模型请求超时：读超时 {int(timeout.read)} 秒内未收到完整响应。"
            "同步接口可能需要更长时间。",
            profile=profile,
            endpoint=url,
            suggestion=f"请在设置中提高 {profile.role.capitalize()} 超时，或检查服务商状态。",
            code="model_timeout",
        ) from last_error
    if last_error is not None:
        raise ModelAdapterError(
            format_transport_error(last_error, proxy_url),
            profile=profile,
            endpoint=url,
            suggestion=(
                "请检查代理地址和代理服务，或清空代理后直连。"
                if proxy_url
                else f"请检查 {profile.role.capitalize()} 的 Base URL、DNS 和网络连接。"
            ),
            code="model_network_error",
        ) from last_error
    if last_response is not None:
        return last_response
    raise ModelAdapterError("模型请求失败：未收到有效响应")


class DesignClient:
    async def generate(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> str:
        protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
        if protocol == "openai_chat":
            return await self._openai_chat(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url)
        if protocol == "openai_responses":
            return await self._openai_responses(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url)
        if protocol == "anthropic_messages":
            return await self._anthropic_messages(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url)
        raise ModelAdapterError(f"Unsupported design protocol: {profile.protocol}")

    async def _openai_chat(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None,
        proxy_url: str | None,
    ) -> str:
        url = f"{normalize_base_url(profile.base_url, profile.protocol)}/chat/completions"
        content: list[dict[str, Any]] = [{"type": "text", "text": user_prompt}]
        content.extend({"type": "image_url", "image_url": {"url": data_url(img["mime_type"], img["b64"])}} for img in images)
        payload = {
            "model": profile.model,
            "messages": [{"role": "system", "content": system_prompt}, {"role": "user", "content": content}],
            "temperature": 0.2,
        }
        data = await self._post_json(profile, url, payload, {"Authorization": f"Bearer {require_api_key(profile)}"}, timeout_seconds, proxy_url)
        return data["choices"][0]["message"]["content"]

    async def _openai_responses(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None,
        proxy_url: str | None,
    ) -> str:
        url = f"{normalize_base_url(profile.base_url, profile.protocol)}/responses"
        content: list[dict[str, Any]] = [{"type": "input_text", "text": user_prompt}]
        content.extend({"type": "input_image", "image_url": data_url(img["mime_type"], img["b64"])} for img in images)
        payload = {
            "model": profile.model,
            "instructions": system_prompt,
            "input": [{"role": "user", "content": content}],
            "text": {"format": {"type": "text"}},
        }
        data = await self._post_json(profile, url, payload, {"Authorization": f"Bearer {require_api_key(profile)}"}, timeout_seconds, proxy_url)
        if data.get("output_text"):
            return data["output_text"]
        for item in data.get("output", []):
            for part in item.get("content", []):
                if part.get("type") == "output_text":
                    return part.get("text", "")
        raise ModelAdapterError("OpenAI Responses result did not contain output text")

    async def _anthropic_messages(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None,
        proxy_url: str | None,
    ) -> str:
        url = f"{normalize_base_url(profile.base_url, profile.protocol)}/messages"
        content: list[dict[str, Any]] = [{"type": "text", "text": user_prompt}]
        content.extend(
            {
                "type": "image",
                "source": {"type": "base64", "media_type": img["mime_type"], "data": img["b64"]},
            }
            for img in images
        )
        payload = {"model": profile.model, "max_tokens": 4096, "system": system_prompt, "messages": [{"role": "user", "content": content}]}
        data = await self._post_json(
            profile,
            url,
            payload,
            {"x-api-key": require_api_key(profile), "anthropic-version": profile.api_version or "2023-06-01"},
            timeout_seconds,
            proxy_url,
        )
        return "\n".join(part.get("text", "") for part in data.get("content", []) if part.get("type") == "text")

    async def _post_json(
        self,
        profile: ModelProfile,
        url: str,
        payload: dict[str, Any],
        headers: dict[str, str],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> dict[str, Any]:
        response = await post_json_with_retries(profile, url, payload, headers, timeout_seconds, proxy_url)
        if response.status_code >= 400:
            raise ModelAdapterError(
                format_http_error("Design", response.status_code, response.text),
                profile=profile,
                endpoint=url,
                http_status=response.status_code,
                suggestion=http_error_suggestion(profile.role, response.status_code),
                code="model_http_error",
            )
        try:
            return response.json()
        except ValueError as exc:
            detail = response_error_summary(response.text)
            raise ModelAdapterError(
                f"Design 响应不是 JSON。{f' 服务返回：{detail}' if detail else ''}",
                profile=profile,
                endpoint=url,
                suggestion="请检查 Design 协议是否与服务商接口兼容。",
                code="invalid_model_response",
            ) from exc


class ImplementClient:
    async def generate(
        self,
        profile: ModelProfile,
        prompt: str,
        reference_images: list[dict[str, str]] | None = None,
        output_overrides: dict[str, Any] | None = None,
        proxy_url: str | None = None,
    ) -> str:
        protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
        if protocol == "image2":
            return await self._image2(profile, prompt, reference_images or [], output_overrides or {}, proxy_url)
        if protocol == "banana2":
            return await self._banana2(profile, prompt, reference_images or [], output_overrides or {}, proxy_url)
        raise ModelAdapterError(f"Unsupported implement protocol: {profile.protocol}")

    async def _image2(
        self,
        profile: ModelProfile,
        prompt: str,
        reference_images: list[dict[str, str]],
        output_overrides: dict[str, Any],
        proxy_url: str | None,
    ) -> str:
        base = normalize_base_url(profile.base_url, "image2")
        defaults = {key: value for key, value in {**profile.output_defaults, **output_overrides}.items() if value not in (None, "")}
        headers = {"Authorization": f"Bearer {require_api_key(profile)}"}
        image_fields = self._image2_fields(defaults)
        # 同步出图可能远超 design 超时；image2 强制至少 2 分钟读超时
        timeout = build_timeout(profile.timeout_seconds, minimum=MIN_IMAGE_TIMEOUT_SECONDS)
        if reference_images:
            url = f"{base}/images/edits"
            files = [("image", (img["filename"], base64.b64decode(img["b64"]), img["mime_type"])) for img in reference_images]
            data = {"model": profile.model, "prompt": prompt, **{key: str(value) for key, value in image_fields.items()}}
            response = await self._post_multipart_with_retries(profile, url, data, files, headers, timeout, proxy_url)
        else:
            url = f"{base}/images/generations"
            payload = {"model": profile.model, "prompt": prompt, **image_fields}
            response = await post_json_with_retries(
                profile,
                url,
                payload,
                headers,
                proxy_url=proxy_url,
                minimum_timeout=MIN_IMAGE_TIMEOUT_SECONDS,
            )
        if response.status_code >= 400:
            raise ModelAdapterError(
                format_http_error("Implement", response.status_code, response.text),
                profile=profile,
                endpoint=url,
                http_status=response.status_code,
                suggestion=http_error_suggestion(profile.role, response.status_code),
                code="model_http_error",
            )
        try:
            data = response.json()
        except Exception as exc:
            raise ModelAdapterError(
                f"Implement 响应不是 JSON。 服务返回：{response_error_summary(response.text)}",
                profile=profile,
                endpoint=url,
                suggestion="请检查 Implement 协议是否与服务商接口兼容。",
                code="invalid_model_response",
            ) from exc
        image = data.get("data", [{}])[0] if isinstance(data.get("data"), list) and data.get("data") else {}
        if not isinstance(image, dict):
            image = {}
        if image.get("b64_json"):
            return image["b64_json"]
        if image.get("url"):
            async with create_async_client(timeout, proxy_url) as client:
                image_response = await client.get(image["url"])
            if image_response.status_code >= 400:
                raise ModelAdapterError(
                    format_http_error("图片下载", image_response.status_code, image_response.text),
                    profile=profile,
                    endpoint=image["url"],
                    http_status=image_response.status_code,
                    suggestion="图片已生成但下载失败，请检查返回 URL 是否可访问。",
                    code="image_download_error",
                )
            return base64.b64encode(image_response.content).decode("ascii")
        raise ModelAdapterError("Image response did not contain b64_json or url")

    async def _post_multipart_with_retries(
        self,
        profile: ModelProfile,
        url: str,
        data: dict[str, str],
        files: list[Any],
        headers: dict[str, str],
        timeout: httpx.Timeout,
        proxy_url: str | None,
    ) -> httpx.Response:
        attempts = max(1, profile.max_retries + 1)
        last_error: httpx.TimeoutException | httpx.TransportError | None = None
        last_response: httpx.Response | None = None
        for attempt in range(attempts):
            try:
                async with create_async_client(timeout, proxy_url) as client:
                    response = await client.post(url, data=data, files=files, headers={**profile.headers, **headers})
                last_response = response
                if response.status_code not in RETRY_STATUS_CODES or attempt == attempts - 1:
                    return response
                await asyncio.sleep(retry_backoff_seconds(attempt, response.status_code))
                continue
            except httpx.TimeoutException as exc:
                last_error = exc
                if attempt == attempts - 1:
                    break
            except httpx.TransportError as exc:
                last_error = exc
                if attempt == attempts - 1:
                    break
            await asyncio.sleep(retry_backoff_seconds(attempt))
        if isinstance(last_error, httpx.TimeoutException):
            raise ModelAdapterError(
                f"模型请求超时：读超时 {int(timeout.read)} 秒内未收到完整响应。",
                profile=profile,
                endpoint=url,
                suggestion="请在设置中提高 Implement 超时，或检查服务商状态。",
                code="model_timeout",
            ) from last_error
        if last_error is not None:
            raise ModelAdapterError(
                format_transport_error(last_error, proxy_url),
                profile=profile,
                endpoint=url,
                suggestion=(
                    "请检查代理地址和代理服务，或清空代理后直连。"
                    if proxy_url
                    else "请检查 Implement 的 Base URL、DNS 和网络连接。"
                ),
                code="model_network_error",
            ) from last_error
        if last_response is not None:
            return last_response
        raise ModelAdapterError("模型请求失败：未收到有效响应")

    @staticmethod
    def _image2_fields(defaults: dict[str, Any]) -> dict[str, Any]:
        # 默认 url：避免同步接口回传大体积 b64 时被 nginx 502 截断（WisArt 文档支持 url / b64_json）
        fields: dict[str, Any] = {
            "size": defaults.get("size") or "1200x675",
            "quality": defaults.get("quality") or "auto",
            "n": int(defaults.get("n") or 1),
            "response_format": defaults.get("response_format") or "url",
        }
        for key in ("background", "moderation", "output_format", "output_compression", "user"):
            if defaults.get(key) not in (None, ""):
                fields[key] = defaults[key]
        return fields

    async def _banana2(
        self,
        profile: ModelProfile,
        prompt: str,
        reference_images: list[dict[str, str]],
        output_overrides: dict[str, Any],
        proxy_url: str | None,
    ) -> str:
        defaults = {**profile.output_defaults, **output_overrides}
        version = profile.api_version or "v1beta"
        url = f"{profile.base_url.rstrip('/')}/{version}/interactions"
        input_blocks: list[dict[str, Any]] = []
        input_blocks.extend({"type": "image", "mime_type": img["mime_type"], "data": img["b64"]} for img in reference_images)
        input_blocks.append({"type": "text", "text": prompt})
        payload = {
            "model": profile.model,
            "input": input_blocks,
            "response_format": {
                "type": "image",
                "aspect_ratio": defaults.get("aspect_ratio", "16:9"),
                "image_size": defaults.get("image_size", "4K"),
            },
            "generation_config": {"thinking_level": defaults.get("thinking_level", "high")},
        }
        headers = {"x-goog-api-key": require_api_key(profile)}
        response = await post_json_with_retries(
            profile,
            url,
            payload,
            headers,
            proxy_url=proxy_url,
            minimum_timeout=MIN_IMAGE_TIMEOUT_SECONDS,
        )
        if response.status_code >= 400:
            raise ModelAdapterError(
                format_http_error("Implement", response.status_code, response.text),
                profile=profile,
                endpoint=url,
                http_status=response.status_code,
                suggestion=http_error_suggestion(profile.role, response.status_code),
                code="model_http_error",
            )
        data = response.json()
        image = self._extract_gemini_image(data)
        if not image:
            raise ModelAdapterError("Gemini response did not contain image data")
        return image

    @staticmethod
    def _extract_gemini_image(data: dict[str, Any]) -> str | None:
        for step in data.get("steps", []):
            if step.get("type") != "model_output":
                continue
            for block in step.get("content", []):
                if block.get("type") == "image" and block.get("data"):
                    return block["data"]
        for candidate in data.get("candidates", []):
            for part in candidate.get("content", {}).get("parts", []):
                inline_data = part.get("inlineData") or part.get("inline_data")
                if inline_data and inline_data.get("data"):
                    return inline_data["data"]
        return None


def parse_json_response(text: str) -> dict[str, Any]:
    cleaned = text.strip()
    cleaned = re.sub(r"^```(?:json)?", "", cleaned).strip()
    cleaned = re.sub(r"```$", "", cleaned).strip()
    try:
        return json.loads(cleaned)
    except json.JSONDecodeError:
        start = cleaned.find("{")
        end = cleaned.rfind("}")
        if start >= 0 and end > start:
            return json.loads(cleaned[start : end + 1])
        raise
