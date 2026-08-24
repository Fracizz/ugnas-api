from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from ugnas.client import UgnasClient
from ugnas.config import (
    connect,
    credential_path,
    default_config_path,
    default_env_path,
    default_session_path,
    load_config,
    load_dotenv,
    load_saved_password,
    profile_dir,
    profile_name,
    save_config,
    upsert_dotenv,
)


class ConfigTests(unittest.TestCase):
    def test_profile_paths_are_isolated(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, {"UGNAS_HOME": tmp}, clear=False):
            self.assertEqual(profile_dir("work"), Path(tmp, "profiles", "work"))
            self.assertEqual(default_config_path("work"), Path(tmp, "profiles", "work", "config.json"))
            self.assertEqual(default_session_path("work"), Path(tmp, "profiles", "work", "session.json"))
            self.assertEqual(default_env_path("work"), Path(tmp, "profiles", "work", ".env"))
        with self.assertRaises(ValueError):
            profile_name("bad/name")

    def test_dotenv_load_and_upsert(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, ".env")
            path.write_text("# comment\nUGNAS_USERNAME=first\nEMPTY=\n", encoding="utf-8")
            with patch.dict(os.environ, {"UGNAS_USERNAME": "existing"}, clear=False):
                self.assertEqual(load_dotenv(path), path)
                self.assertEqual(os.environ["UGNAS_USERNAME"], "existing")
                load_dotenv(path, override=True)
                self.assertEqual(os.environ["UGNAS_USERNAME"], "first")
            upsert_dotenv({"UGNAS_USERNAME": "second", "NEW": "value"}, path)
            updated = path.read_text(encoding="utf-8")
            self.assertIn("UGNAS_USERNAME=second", updated)
            self.assertIn("NEW=value", updated)

    def test_config_round_trip_and_environment_override(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, "config.json")
            save_config({"base_url": "http://one", "username": "saved", "password": None}, path)
            with patch.dict(
                os.environ,
                {"UGNAS_BASE_URL": "http://env", "UGNAS_USERNAME": "env-user", "UGNAS_INSECURE": "1"},
                clear=False,
            ):
                loaded = load_config(path)
            self.assertEqual(loaded["base_url"], "http://env")
            self.assertEqual(loaded["username"], "env-user")
            self.assertTrue(loaded["insecure"])

    def test_connect_wires_config_and_login(self) -> None:
        fake = unittest.mock.Mock()
        with tempfile.TemporaryDirectory() as tmp, patch("ugnas.client.UgnasClient", return_value=fake) as constructor:
            config_path = Path(tmp, "config.json")
            config_path.write_text(
                json.dumps({"base_url": "http://nas.test", "username": "user", "save_dir": "/volume1/share"}),
                encoding="utf-8",
            )
            client = connect(config_path=config_path, password="pw", session_path=Path(tmp, "session"), insecure=True)
        self.assertIs(client, fake)
        constructor.assert_called_once()
        fake.login.assert_called_once_with("user", "pw", reuse=True)
        self.assertFalse(fake.session.verify)
        self.assertEqual(fake.save_dir, "/volume1/share")

    @unittest.skipUnless(os.name == "nt", "DPAPI is Windows-only")
    def test_saved_password_uses_dpapi_without_plaintext(self) -> None:
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, {"UGNAS_HOME": tmp}, clear=False):
            config_path = Path(tmp, "config-under-test.json")
            save_config(
                {"base_url": "http://nas.test", "username": "user", "password": "top-secret"},
                config_path,
                profile="secure",
            )
            raw = credential_path("secure").read_text(encoding="utf-8")
            self.assertNotIn("top-secret", raw)
            self.assertTrue(json.loads(raw)["password"].startswith("dpapi:"))
            self.assertEqual(load_saved_password("secure"), "top-secret")

    @unittest.skipUnless(os.name == "nt", "DPAPI is Windows-only")
    def test_session_token_is_encrypted_and_round_trips(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            session_path = Path(tmp, "session.json")
            client = UgnasClient("http://nas.test", session_path=session_path)
            client.token = "session-secret"
            client.username = "user"
            client.save_session()
            raw = session_path.read_text(encoding="utf-8")
            self.assertNotIn("session-secret", raw)
            loaded = client.load_session()
            self.assertEqual(loaded["token"], "session-secret")


if __name__ == "__main__":
    unittest.main()
