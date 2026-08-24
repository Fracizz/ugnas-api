from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from ugnas.api import Ugnas
from ugnas.errors import ApiError


class FacadeTests(unittest.TestCase):
    def test_file_and_space_facade_methods(self) -> None:
        client = Mock()
        client.save_dir = "/volume1/share"
        client.list_dir_v2.return_value = {"data": {"result": []}}
        client.iter_dir.return_value = iter([{"name": "a"}])
        client.path_exists.return_value = True
        client.get_volumes.return_value = [
            {"name": "v1", "path": "/volume1", "free": 10, "used": 20, "all": 30, "fs_type": "btrfs"}
        ]
        client.get_user_quota.return_value = {"free": 5}
        client.get_path_free_size.return_value = 9
        client.get_size_and_count.return_value = {"size": 3, "takes_space": 4, "status": 1}
        nas = Ugnas(client)

        self.assertEqual(nas.list("/volume1")["data"]["result"], [])
        self.assertEqual(nas.list_all("/volume1"), [{"name": "a"}])
        self.assertTrue(nas.exists("/volume1/a"))
        self.assertEqual(nas.space()["volumes"][0]["total"], 30)
        self.assertEqual(nas.free()["free"], 9)
        self.assertEqual(nas.size("/volume1/a")["takes_space"], 4)
        nas.mkdir("/volume1/new")
        nas.delete(["/volume1/a"])
        nas.rename("/volume1/a", "b")
        nas.move("/volume1/a", "/volume1/new")
        nas.copy("/volume1/a", "/volume1/new")
        client.ensure_dir.assert_called()
        client.delete_paths.assert_called_once()
        client.rename.assert_called_once()
        self.assertEqual(client.copy_or_move.call_count, 2)

    def test_free_falls_back_only_for_api_error(self) -> None:
        client = Mock()
        client.save_dir = "/volume1/share"
        client.get_path_free_size.side_effect = ApiError("unsupported")
        client.get_volumes.return_value = [{"path": "/volume1", "free": 42, "all": 100}]
        client.get_user_quota.return_value = {}
        result = Ugnas(client).free()
        self.assertEqual(result, {"path": "/volume1/share", "free": 42, "volume": "/volume1"})

    def test_pull_share_and_download_share(self) -> None:
        client = Mock()
        client.save_dir = "/volume1/share"
        client.add_http_download.return_value = {"task_name": "job"}
        client.add_share.return_value = {"id": 7, "share_id": "public", "link": "https://ug.link/?id=public"}
        client.get_share.return_value = {"id": 7, "link": "https://ug.link/?id=public"}
        client.get_share_download_url.return_value = {
            "url": "https://ug.link/download?task_id=one",
            "share_id": "public",
            "paths": ["/volume1/a"],
            "name": "a",
        }
        client.download_url_to_file.return_value = {"dest": "a", "bytes": 1}
        nas = Ugnas(client)

        with patch.object(nas, "_wait_download", return_value="/volume1/share/a"):
            self.assertEqual(nas.pull("https://example/a", wait=True)["nas_path"], "/volume1/share/a")
        shared = nas.share_file("/volume1/a", password="pw")
        self.assertEqual(shared["share_id"], "public")
        downloaded = nas.download_share("public", "local")
        self.assertEqual(downloaded["share_id"], "public")

    def test_upload_and_download_dry_run(self) -> None:
        client = Mock()
        client.save_dir = "/volume1/share"
        client.upload_file.return_value = {"result": "/volume1/share/a.bin"}
        client.download_url.return_value = "http://nas/download"
        client.download_url_to_file.return_value = {"dest": "local", "dry_run": True}
        nas = Ugnas(client)
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp, "a.bin")
            source.write_bytes(b"a")
            cancelled = Mock(return_value=False)
            self.assertEqual(
                nas.upload_to_dir(str(source), "/volume1/share", cancelled=cancelled)["path"],
                "/volume1/share/a.bin",
            )
            self.assertIs(client.upload_file.call_args.kwargs["cancelled"], cancelled)
        self.assertTrue(nas.download("/volume1/a", "local", dry_run=True)["dry_run"])

    def test_wait_download_matches_completed_task(self) -> None:
        client = Mock()
        client.list_completed_downloads.return_value = {
            "data": {"result": [{"task_name": "job.zip", "save_dir": "/volume1/share"}]}
        }
        nas = Ugnas(client)
        self.assertEqual(nas._wait_download({"task_name": "job.zip"}, "/fallback", 1), "/volume1/share/job.zip")


if __name__ == "__main__":
    unittest.main()
