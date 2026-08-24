from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from ugnas.api import Ugnas
from ugnas.cli import _public_preview


class _Client:
    save_dir = None

    def ensure_dir(self, path):
        self.dest_dir = path

    def upload_file(self, local_path, dest_dir, filename=None, **kwargs):
        self.upload = {"dest_dir": dest_dir, "filename": filename}
        return {"result": f"{dest_dir}/{filename}"}


class ApiCliTests(unittest.TestCase):
    def test_dotted_destination_is_a_directory_by_default(self) -> None:
        client = _Client()
        nas = Ugnas(client)
        with tempfile.TemporaryDirectory() as tmp:
            local = Path(tmp, "local.bin")
            local.write_bytes(b"x")
            result = nas.upload(str(local), "/volume1/releases/v1.2")
        self.assertEqual(client.upload, {"dest_dir": "/volume1/releases/v1.2", "filename": "local.bin"})
        self.assertEqual(result["path"], "/volume1/releases/v1.2/local.bin")

    def test_explicit_full_destination_path(self) -> None:
        client = _Client()
        nas = Ugnas(client)
        with tempfile.TemporaryDirectory() as tmp:
            local = Path(tmp, "local.bin")
            local.write_bytes(b"x")
            nas.upload_to_path(str(local), "/volume1/releases/renamed.data")
        self.assertEqual(client.upload, {"dest_dir": "/volume1/releases", "filename": "renamed.data"})

    def test_url_query_secrets_are_redacted(self) -> None:
        value = {"url": "https://example/file?password=secret&token=abc&safe=yes"}
        preview = _public_preview(value)
        self.assertNotIn("secret", preview["url"])
        self.assertNotIn("abc", preview["url"])
        self.assertIn("safe=yes", preview["url"])


if __name__ == "__main__":
    unittest.main()
