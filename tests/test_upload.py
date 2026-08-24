from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from ugnas.client import UgnasClient
from ugnas.errors import NetworkError


class UploadTests(unittest.TestCase):
    def test_upload_prepares_queries_cache_and_sends_chunks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp, "a b.txt")
            source.write_bytes(b"hello upload")
            client = UgnasClient("http://nas.test", client_id="client")
            calls = []

            def fake_request(method, path, **kwargs):
                calls.append((method, path, kwargs))
                if path.endswith("fileUpload"):
                    return {"code": 200, "data": {"result": False}}
                if path.endswith("getUpdateTmpInfo"):
                    return {"code": 200, "data": {"result": 0}}
                return {"code": 200, "data": {"result": "/volume1/share/a b.txt"}}

            progress = []
            with patch.object(client, "request", side_effect=fake_request):
                result = client.upload_file(
                    str(source),
                    "/volume1/share",
                    chunk_size=5,
                    progress=lambda done, total: progress.append((done, total)),
                )

            self.assertEqual(result["result"], "/volume1/share/a b.txt")
            prepare = calls[0][2]
            self.assertEqual(prepare["data"]["first_request"], "true")
            chunks = [call for call in calls if call[1].endswith("fileUploadV2")]
            self.assertEqual([call[2]["data"] for call in chunks], [b"hello", b" uplo", b"ad"])
            params = [json.loads(call[2]["extra_headers"]["ug-param"]) for call in chunks]
            self.assertEqual([item["begin_size"] for item in params], [0, 5, 10])
            self.assertEqual(params[0]["dir"], "%2Fvolume1%2Fshare")
            self.assertEqual(progress, [(5, 12), (10, 12), (12, 12)])

    def test_resume_starts_at_server_confirmed_offset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp, "resume.bin")
            source.write_bytes(b"0123456789")
            client = UgnasClient("http://nas.test", client_id="stable")
            bodies = []

            def fake_request(method, path, **kwargs):
                if path.endswith("fileUpload"):
                    return {"code": 200, "data": {"result": False}}
                if path.endswith("getUpdateTmpInfo"):
                    return {"code": 200, "data": {"result": 4}}
                bodies.append(kwargs["data"])
                return {"code": 200, "data": {}}

            with patch.object(client, "request", side_effect=fake_request):
                client.upload_file(str(source), "/volume1/share", chunk_size=3)
            self.assertEqual(bodies, [b"456", b"789"])

    def test_network_retry_uses_confirmed_cache_offset(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp, "retry.bin")
            source.write_bytes(b"abcdefgh")
            client = UgnasClient("http://nas.test")
            uploads = 0
            cache_queries = 0

            def fake_request(method, path, **kwargs):
                nonlocal uploads, cache_queries
                if path.endswith("fileUpload"):
                    return {"code": 200, "data": {"result": False}}
                if path.endswith("getUpdateTmpInfo"):
                    cache_queries += 1
                    return {"code": 200, "data": {"result": 4 if cache_queries > 1 else 0}}
                uploads += 1
                if uploads == 1:
                    raise NetworkError("response lost")
                return {"code": 200, "data": {}}

            with patch.object(client, "request", side_effect=fake_request), patch("ugnas.client.time.sleep"):
                result = client.upload_file(str(source), "/volume1/share", chunk_size=4)
            self.assertEqual(result["result"], "/volume1/share/retry.bin")
            self.assertEqual(uploads, 2)

    def test_server_skip_does_not_send_file_body(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp, "same.bin")
            source.write_bytes(b"same")
            client = UgnasClient("http://nas.test")
            with patch.object(client, "request", return_value={"code": 200, "data": {"result": True}}) as request:
                result = client.upload_file(str(source), "/volume1/share", force=False)
            self.assertTrue(result["result"])
            self.assertEqual(request.call_count, 1)

    def test_rejects_non_file_source(self) -> None:
        client = UgnasClient("http://nas.test")
        with self.assertRaises(FileNotFoundError):
            client.upload_file("does-not-exist", "/volume1/share")


if __name__ == "__main__":
    unittest.main()
