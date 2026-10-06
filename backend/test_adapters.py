import os
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from unittest.mock import patch

import httpx

from backend.app.adapters import create_async_client

PROXY_KEYS = ("HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy")


class _Ok(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *args: object) -> None:
        pass


class EnvironmentProxyTests(unittest.IsolatedAsyncioTestCase):
    """shell 里的 *_PROXY 不能让模型请求悄悄走代理：代理只认设置里填的地址。"""

    async def test_client_ignores_environment_proxy(self) -> None:
        server = HTTPServer(("127.0.0.1", 0), _Ok)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        # clear=True 连 NO_PROXY 一起清掉，否则本机地址会被环境自身旁路掉，测不出问题。
        env = {key: "http://127.0.0.1:9" for key in PROXY_KEYS}
        try:
            with patch.dict(os.environ, env, clear=True):
                client = create_async_client(httpx.Timeout(5))
                async with client:
                    response = await client.get(f"http://127.0.0.1:{server.server_port}/")
            self.assertEqual(response.status_code, 200)
        finally:
            server.shutdown()
            server.server_close()
