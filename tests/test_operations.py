from __future__ import annotations

import unittest
from unittest.mock import patch

from ugnas.client import UgnasClient
from ugnas.errors import NetworkError, OperationTimeout


class OperationTests(unittest.TestCase):
    def test_directory_iterator_reads_all_pages(self) -> None:
        client = UgnasClient("http://nas.test")
        pages = [
            {"data": {"result": [{"name": "a"}, {"name": "b"}]}},
            {"data": {"result": [{"name": "c"}]}},
        ]
        with patch.object(client, "list_dir_v2", side_effect=pages) as request:
            items = list(client.iter_dir("/volume1/share", page_size=2))
        self.assertEqual([item["name"] for item in items], ["a", "b", "c"])
        self.assertEqual(request.call_count, 2)

    def test_path_exists_propagates_network_failure(self) -> None:
        client = UgnasClient("http://nas.test")
        with patch.object(client, "request", side_effect=NetworkError("offline")), self.assertRaises(NetworkError):
            client.path_exists("/volume1/share")

    def test_copy_move_timeout_is_not_reported_as_success(self) -> None:
        client = UgnasClient("http://nas.test")
        with (
            patch.object(client, "request", return_value={"code": 200, "data": {"task_id": "1"}}),
            patch.object(client, "path_exists", return_value=False),
            patch("ugnas.client.time.sleep"),
            self.assertRaises(OperationTimeout),
        ):
            client.copy_or_move("/volume1/a", "/volume1/b", wait_timeout=0)


if __name__ == "__main__":
    unittest.main()
