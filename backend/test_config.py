import tempfile
import unittest
from pathlib import Path

from backend.app.config import ConfigStore, default_search_profile, search_needs_key, search_uses_model


class SearchProtocolMigrationTests(unittest.TestCase):
    """旧配置兼容：搜索角色的 openai_chat 已并入 grok_search，其他角色不动。"""

    def test_load_migrates_search_role_only(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config.json"
            store = ConfigStore(path=path)
            config = store.load()
            # 直接落一份"旧配置"文件：搜索与设计角色都写着 openai_chat。
            legacy = config.model_copy(
                update={
                    "model_profiles": [
                        profile.model_copy(update={"protocol": "openai_chat"})
                        if profile.role in {"search", "design"}
                        else profile
                        for profile in config.model_profiles
                    ]
                }
            )
            path.write_text(legacy.model_dump_json(indent=2), encoding="utf-8")

            loaded = store.load()

            by_role = {profile.role: profile for profile in loaded.model_profiles}
            self.assertEqual(by_role["search"].protocol, "grok_search")
            self.assertEqual(by_role["design"].protocol, "openai_chat")

    def test_default_search_timeout_survives_a_real_grok_round_trip(self):
        """grok_search 让上游模型自己联网检索，默认超时必须够久（15 秒必超时）。"""
        with tempfile.TemporaryDirectory() as directory:
            store = ConfigStore(path=Path(directory) / "config.json")
            search = next(p for p in store.load().model_profiles if p.role == "search")
            self.assertGreaterEqual(search.timeout_seconds, 120)


class SearchProfileIsolationTests(unittest.TestCase):
    """每个协议各自存自己那套连接信息，串味的档案在读取时归位。"""

    def _store_with(self, directory: str, profiles) -> ConfigStore:
        path = Path(directory) / "config.json"
        store = ConfigStore(path=path)
        config = store.load()
        path.write_text(
            config.model_copy(update={"model_profiles": profiles}).model_dump_json(indent=2),
            encoding="utf-8",
        )
        return store

    def test_which_protocol_reads_a_model_and_which_needs_a_key(self):
        self.assertFalse(search_uses_model("duckduckgo"))
        self.assertFalse(search_uses_model("tavily"))
        self.assertTrue(search_uses_model("grok_search"))
        self.assertFalse(search_needs_key("duckduckgo"))
        self.assertTrue(search_needs_key("tavily"))
        self.assertTrue(search_needs_key("grok_search"))

    def test_polluted_profile_moves_to_the_protocol_it_belongs_to(self):
        base = default_search_profile()
        polluted = base.model_copy(
            update={
                "protocol": "duckduckgo",
                "base_url": "https://grok.draem.me",
                "model": "grok-build-0.1",
                "api_key": "xai-secret",
            }
        )
        with tempfile.TemporaryDirectory() as directory:
            store = self._store_with(directory, [polluted])
            loaded = store.load()
            moved = next(p for p in loaded.model_profiles if p.protocol == "grok_search")
            self.assertEqual(moved.base_url, "https://grok.draem.me")
            self.assertEqual(moved.model, "grok-build-0.1")
            self.assertEqual(moved.api_key, "xai-secret")
            fresh = next(p for p in loaded.model_profiles if p.protocol == "duckduckgo")
            self.assertEqual(fresh.base_url, "")
            self.assertEqual(fresh.model, "")
            self.assertIsNone(fresh.api_key)
            self.assertNotEqual(fresh.id, moved.id)
            # 幂等：再读一次不会多出档案。
            again = store.load()
            self.assertEqual(len([p for p in again.model_profiles if p.role == "search"]), 2)

    def test_legacy_protocol_name_and_hidden_endpoint_are_normalized(self):
        legacy = default_search_profile().model_copy(
            update={
                "protocol": "duckduckgo_html",
                "base_url": "https://duckduckgo.com",
                "model": "leftover",
            }
        )
        with tempfile.TemporaryDirectory() as directory:
            store = self._store_with(directory, [legacy])
            loaded = store.load()
            profile = next(p for p in loaded.model_profiles if p.role == "search")
            self.assertEqual(profile.protocol, "duckduckgo")
            # 端点写死在代码里，界面不显示 URL，存量值也要清掉，免得藏着改不到。
            self.assertEqual(profile.base_url, "")
            self.assertEqual(profile.model, "")

    def test_own_endpoint_keeps_its_key_and_only_drops_the_unused_model(self):
        base = default_search_profile()
        tavily = base.model_copy(
            update={
                "id": "search-tavily",
                "protocol": "tavily",
                "base_url": "https://api.tavily.com",
                "model": "leftover",
                "api_key": "tvly-key",
            }
        )
        with tempfile.TemporaryDirectory() as directory:
            store = self._store_with(directory, [tavily])
            loaded = store.load()
            kept = next(p for p in loaded.model_profiles if p.protocol == "tavily")
            self.assertEqual(kept.base_url, "https://api.tavily.com")
            self.assertEqual(kept.api_key, "tvly-key")
            self.assertEqual(kept.model, "")
            self.assertEqual(len(loaded.model_profiles), 1)
