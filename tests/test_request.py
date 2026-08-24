from __future__ import annotations

import io
import unittest
from unittest.mock import Mock, patch

import requests

from ugnas.client import UgnasClient
from ugnas.errors import NetworkError, RequestNotReplayable


class _Response:
    def __init__(self, status: int, payload: dict):
        self.status_code = status
        self.payload = payload
        self.headers = {}

    def json(self):
        return self.payload


class _ReplaySession:
    def __init__(self):
        self.bodies = []

    def request(self, method, url, **kwargs):
        body = kwargs.get("data")
        self.bodies.append(body.read() if hasattr(body, "read") else body)
        if len(self.bodies) == 1:
            return _Response(401, {"code": 1024, "msg": "expired"})
        return _Response(200, {"code": 200, "data": {}})


class _NonSeekable:
    def read(self):
        return b"body"


class RequestTests(unittest.TestCase):
    def test_seekable_body_is_rewound_after_reauth(self) -> None:
        client = UgnasClient("http://nas.test")
        client.token = "old"
        client.session = _ReplaySession()
        with patch.object(client, "_recover_auth", return_value=True):
            client.request("POST", "/upload", data=io.BytesIO(b"payload"))
        self.assertEqual(client.session.bodies, [b"payload", b"payload"])

    def test_non_seekable_body_is_not_silently_replayed(self) -> None:
        client = UgnasClient("http://nas.test")
        client.token = "old"
        client.session = _ReplaySession()
        with patch.object(client, "_recover_auth", return_value=True), self.assertRaises(RequestNotReplayable):
            client.request("POST", "/upload", data=_NonSeekable())

    def test_network_error_is_structured(self) -> None:
        client = UgnasClient("http://nas.test")
        client.token = "token"
        session = Mock()
        session.request.side_effect = requests.ConnectionError("offline")
        client.session = session
        with self.assertRaises(NetworkError):
            client.request("GET", "/status")

    def test_download_url_does_not_mask_network_error(self) -> None:
        client = UgnasClient("http://nas.test")
        client.token = "token"
        with patch.object(client, "get_download_token", side_effect=NetworkError("offline")), self.assertRaises(
            NetworkError
        ):
            client.download_url(["/volume1/a.txt"])


if __name__ == "__main__":
    unittest.main()
