from __future__ import annotations

import os
import tempfile
import unittest
import uuid
from pathlib import Path

from ugnas import Ugnas


@unittest.skipUnless(os.environ.get("UGNAS_INTEGRATION") == "1", "set UGNAS_INTEGRATION=1 for real NAS tests")
class NasIntegrationTests(unittest.TestCase):
    def test_upload_download_exists_and_cleanup(self) -> None:
        nas = Ugnas.connect(profile=os.environ.get("UGNAS_PROFILE"))
        target_dir = nas.default_save_dir
        self.assertTrue(target_dir, "configure save_dir before integration tests")
        name = f"ugnas-integration-{uuid.uuid4().hex[:10]}.txt"
        remote = f"{target_dir.rstrip('/')}/{name}"
        local = Path(tempfile.gettempdir(), name)
        downloaded = Path(tempfile.gettempdir(), "downloaded-" + name)
        local.write_bytes(b"ugnas integration\n")
        try:
            result = nas.upload_to_dir(str(local), target_dir)
            self.assertEqual(result["path"], remote)
            self.assertTrue(nas.exists(remote))
            self.assertEqual(nas.download(remote, str(downloaded)), str(downloaded))
            self.assertEqual(downloaded.read_bytes(), local.read_bytes())
        finally:
            if nas.exists(remote):
                nas.delete([remote])
            local.unlink(missing_ok=True)
            downloaded.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
