"""Pinned Geo downloads are prepared before building the release bundle."""
import hashlib
import importlib.util
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "prepare_geo.py"
sys.path.insert(0, str(SCRIPT.parent))
spec = importlib.util.spec_from_file_location("prepare_geo", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PrepareGeo(unittest.TestCase):
    def test_download_is_pinned_and_published_with_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            data = b"Geo fixture"
            digest = hashlib.sha256(data).hexdigest()
            with patch.object(module, "urlopen", return_value=io.BytesIO(data)):
                manifest = module.prepare("geoip.metadb", "https://example.org/geo", digest, directory)
            self.assertEqual((Path(directory) / "geoip.metadb").read_bytes(), data)
            self.assertIn(digest, manifest.read_text())
            with patch.object(module, "urlopen", return_value=io.BytesIO(b"corrupt")):
                with self.assertRaisesRegex(ValueError, "SHA-256"):
                    module.prepare("geoip.metadb", "https://example.org/geo", digest, directory)
            self.assertEqual((Path(directory) / "geoip.metadb").read_bytes(), data)

    def test_rejects_unpinned_or_insecure_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            for name, url, digest in [
                ("../outside", "https://example.org/geo", "0" * 64),
                ("geoip.metadb", "http://example.org/geo", "0" * 64),
                ("geoip.metadb", "https://example.org/geo", "invalid"),
            ]:
                with self.assertRaises(ValueError):
                    module.prepare(name, url, digest, directory)


if __name__ == "__main__":
    unittest.main()
