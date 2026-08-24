from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from ugnas.client import UgnasClient
from ugnas.errors import TransferError


class _Response:
    def __init__(self, body: bytes, *, status: int = 200, headers=None):
        self.body = body
        self.status_code = status
        self.headers = headers or {"Content-Length": str(len(body))}
        self.url = "http://nas/file"

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def close(self):
        pass

    def iter_content(self, size):
        for start in range(0, len(self.body), size):
            yield self.body[start : start + size]


class _Session:
    def __init__(self, response):
        self.response = response
        self.last_headers = None

    def get(self, url, **kwargs):
        self.last_headers = kwargs.get("headers")
        return self.response


class DownloadTests(unittest.TestCase):
    def test_download_replaces_destination_atomically(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp, "file.bin")
            dest.write_bytes(b"old")
            client = UgnasClient("http://nas.test")
            client.session = _Session(_Response(b"new-content"))
            result = client.download_url_to_file("http://nas/file", dest, chunk_size=3)
            self.assertEqual(dest.read_bytes(), b"new-content")
            self.assertFalse(Path(str(dest) + ".part").exists())
            self.assertEqual(result["bytes"], 11)

    def test_download_resumes_existing_part(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp, "file.bin")
            Path(str(dest) + ".part").write_bytes(b"abc")
            response = _Response(
                b"def",
                status=206,
                headers={"Content-Range": "bytes 3-5/6", "Content-Length": "3"},
            )
            client = UgnasClient("http://nas.test")
            client.session = _Session(response)
            result = client.download_url_to_file("http://nas/file", dest)
            self.assertEqual(dest.read_bytes(), b"abcdef")
            self.assertEqual(client.session.last_headers["Range"], "bytes=3-")
            self.assertEqual(result["resumed_from"], 3)

    def test_incomplete_download_keeps_part_and_old_destination(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp, "file.bin")
            dest.write_bytes(b"old")
            response = _Response(b"short", headers={"Content-Length": "10"})
            client = UgnasClient("http://nas.test")
            client.session = _Session(response)
            with self.assertRaises(TransferError):
                client.download_url_to_file("http://nas/file", dest)
            self.assertEqual(dest.read_bytes(), b"old")
            self.assertEqual(Path(str(dest) + ".part").read_bytes(), b"short")


if __name__ == "__main__":
    unittest.main()
