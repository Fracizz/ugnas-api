from __future__ import annotations

import unittest
from unittest.mock import patch

from ugnas.client import UgnasClient, parse_share_ref


class ClientMethodTests(unittest.TestCase):
    def test_parse_share_references(self) -> None:
        self.assertEqual(parse_share_ref("abc"), ("abc", None))
        self.assertEqual(parse_share_ref("https://x.ug.link/file?id=abc"), ("abc", "https://x.ug.link"))
        with self.assertRaises(RuntimeError):
            parse_share_ref("")

    def test_simple_file_and_capacity_endpoints(self) -> None:
        client = UgnasClient("http://nas.test")

        def request(method, path, **kwargs):
            if path.endswith("getVolumes"):
                return {"data": {"result": [{"path": "/volume1"}]}}
            if path.endswith("getPathFreeSize"):
                return {"data": {"result": 123}}
            if path.endswith("quota"):
                return {"data": {"limit": 10}}
            if path.endswith("pathExist"):
                return {"data": {"result": True}}
            return {"data": {"result": []}}

        with patch.object(client, "request", side_effect=request) as mocked:
            self.assertEqual(client.get_volumes()[0]["path"], "/volume1")
            self.assertEqual(client.get_path_free_size("/volume1"), 123)
            self.assertEqual(client.get_user_quota()["limit"], 10)
            self.assertTrue(client.path_exists("/volume1"))
            client.list_dir("/volume1")
            client.list_dir_v2("/volume1")
            client.delete_paths(["/volume1/a"])
            client.rename("/volume1/a", "b")
            self.assertGreaterEqual(mocked.call_count, 8)

    def test_share_direct_flow_and_url_encoding(self) -> None:
        client = UgnasClient("http://nas.test")
        client._public_base = "https://x.ug.link"
        with (
            patch.object(client, "query_share_info", return_value={"name": "a"}),
            patch.object(client, "verify_share_password", return_value={"file_info": [{"path": "/v/a", "name": "a"}]}),
            patch.object(client, "add_paths_by_share_id", return_value="task"),
        ):
            result = client.get_share_download_url("share", password="p w")
        self.assertEqual(result["task_id"], "task")
        self.assertIn("password=p+w", result["url"])

    def test_share_create_get_list_and_delete_resolution(self) -> None:
        client = UgnasClient("http://nas.test")
        responses = {
            "/ugreen/v1/filemgr/addShare": {"data": {"link": "https://share"}},
            "/ugreen/v1/filemgr/getShareMsg": {"data": {"link": "https://share"}},
            "/ugreen/v1/filemgr/shareList": {
                "data": {"result": [{"id": 9, "share_id": "public", "link": "https://share?id=public"}]}
            },
            "/ugreen/v1/filemgr/delShare": {"data": {}},
        }

        def request(method, path, **kwargs):
            return responses[path]

        with patch.object(client, "request", side_effect=request):
            self.assertEqual(client.add_share(["/v/a"])["link"], "https://share")
            self.assertEqual(client.share_url(share_id=9), "https://share")
            self.assertEqual(client.resolve_share_ids("public"), ["9"])
            client.delete_share("public")

    def test_ensure_dir_and_download_center(self) -> None:
        client = UgnasClient("http://nas.test")
        with patch.object(client, "path_exists", return_value=False), patch.object(
            client, "request", return_value={"data": {"task_name": "job"}}
        ) as request:
            client.ensure_dir("/volume1/new")
            self.assertEqual(client.add_http_download("https://example/a", "/volume1/new")["task_name"], "job")
            client.list_downloads()
            client.list_completed_downloads()
            self.assertGreaterEqual(request.call_count, 4)


if __name__ == "__main__":
    unittest.main()
