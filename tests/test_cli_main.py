from __future__ import annotations

import io
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import Mock, patch

from ugnas.cli import main


class _Nas:
    def __init__(self):
        self.client = Mock(reused_session=True, login_data={"token": "secret", "username": "u"})

    def pull(self, *args, **kwargs):
        return {"task": "ok"}

    def download(self, *args, **kwargs):
        return "local"

    def upload(self, *args, **kwargs):
        callback = kwargs.get("progress")
        if callback:
            callback(1, 1)
        return {"path": "/remote"}

    def rename(self, *args, **kwargs):
        return {"name": "b"}

    def move(self, *args, **kwargs):
        return {"moved": True}

    def copy(self, *args, **kwargs):
        return {"copied": True}

    def share_file(self, *args, **kwargs):
        return {"url": "https://x/file?task_id=secret"}

    def share_direct(self, *args, **kwargs):
        return {"url": "https://x/file?token=secret"}

    def download_share(self, *args, **kwargs):
        return {"dest": "local"}

    def unshare(self, *args, **kwargs):
        return {"deleted": True}

    def free(self, *args, **kwargs):
        return {"free": 1}

    def space(self):
        return {"volumes": []}

    def size(self, *args, **kwargs):
        return {"size": 1}


class CliMainTests(unittest.TestCase):
    def run_cli(self, args):
        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            patch("sys.argv", ["ugnas", *args]),
            patch("ugnas.cli.Ugnas.connect", return_value=_Nas()),
            redirect_stdout(stdout),
            redirect_stderr(stderr),
        ):
            code = main()
        self.assertEqual(code, 0)
        return stdout.getvalue(), stderr.getvalue()

    def test_command_dispatch(self) -> None:
        commands = [
            ["login"],
            ["pull", "--url", "https://example/a", "--wait", "--no-force"],
            ["download", "--path", "/v/a", "--dest", "local"],
            ["upload", "--path", "local", "--dest", "/v", "--name", "a", "--progress"],
            ["rename", "--path", "/v/a", "--name", "b"],
            ["move", "--path", "/v/a", "--dest", "/v/b"],
            ["copy", "--path", "/v/a", "--dest", "/v/b"],
            ["share", "--path", "/v/a", "--share-password", "pw"],
            ["share-direct", "--share-id", "id", "--path", "/v/a"],
            ["download-share", "--url", "https://share?id=x", "--dest", "local"],
            ["unshare", "--share-id", "id"],
            ["space"],
            ["space", "--path", "/v"],
            ["size", "--path", "/v"],
        ]
        for args in commands:
            with self.subTest(command=args[0]):
                output, _ = self.run_cli(args)
                self.assertIn("login ok", output)
                self.assertNotIn("secret", output)

    def test_config_commands(self) -> None:
        with (
            patch("sys.argv", ["ugnas", "config-save", "--username", "u", "--prompt-password"]),
            patch("ugnas.cli.getpass.getpass", return_value="pw"),
            patch("ugnas.cli.save_config", return_value="config.json") as save,
            redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(main(), 0)
            self.assertEqual(save.call_args.args[0]["password"], "pw")
        with (
            patch("sys.argv", ["ugnas", "config-show", "--profile", "work"]),
            patch("ugnas.cli.load_config", return_value={"username": "u"}) as load,
            redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(main(), 0)
            load.assert_called_once_with(profile="work")


if __name__ == "__main__":
    unittest.main()
