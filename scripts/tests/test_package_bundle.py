"""Boundary checks for the real packager; native service/core workflow is in Rust."""
import hashlib
import json
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "package_bundle.py"
spec = importlib.util.spec_from_file_location("packager", SCRIPT)
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


class PackageBoundaries(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="ms-packager-")
        self.root = Path(self.temporary.name)
        self.core = self.root / "core"
        self.core.write_bytes(b"\x7fELF\x02\x01" + bytes(12) + (62).to_bytes(2, "little"))
        self.core.chmod(0o700)
        self.args = SimpleNamespace(target=packager.TARGET, mihomo=str(self.core),
                                    core_sha256=hashlib.sha256(self.core.read_bytes()).hexdigest(),
                                    core_version="v1.19.31", output=str(self.root / "bundle"),
                                    build=False, service=None, web_dir=None)

    def tearDown(self):
        self.temporary.cleanup()

    def test_wrong_hash_is_rejected_before_executing_input(self):
        self.args.core_sha256 = "0" * 64
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            packager.package(self.args)
        self.assertFalse((self.root / "bundle").exists())

    def test_existing_release_is_preserved_without_executing_or_building(self):
        output = self.root / "bundle"
        output.mkdir()
        (output / "retained").write_text("existing release")
        with self.assertRaisesRegex(ValueError, "already exists"):
            packager.package(self.args)
        self.assertEqual((output / "retained").read_text(), "existing release")

    def test_unsupported_target_is_not_silently_approximated(self):
        self.args.target = "aarch64-unknown-linux-gnu"
        with self.assertRaisesRegex(ValueError, "supports only"):
            packager.package(self.args)

    def test_wrong_binary_architecture_is_rejected(self):
        self.core.write_bytes(b"not an ELF executable")
        with self.assertRaisesRegex(ValueError, "x86_64 ELF"):
            packager.executable(self.core)

    def test_web_symlinks_are_rejected(self):
        web = self.root / "web"
        web.mkdir()
        (web / "escape").symlink_to(self.core)
        with self.assertRaisesRegex(ValueError, "Unsafe resource"):
            packager.regular_tree(web)

    def test_version_pin_cannot_contain_arguments_or_path_segments(self):
        self.args.core_version = "latest / bad"
        with self.assertRaisesRegex(ValueError, "pinned core version"):
            packager.package(self.args)

    def geo_args(self, pins):
        directory = self.root / "geo"
        directory.mkdir(exist_ok=True)
        manifest = self.root / "geo.json"
        manifest.write_text(json.dumps(pins))
        self.args.geo_dir, self.args.geo_manifest = str(directory), str(manifest)
        return directory

    def test_geo_pins_accept_fixed_regular_files_and_reject_content_changes(self):
        contents = b"pinned geo fixture"
        directory = self.geo_args({"geoip.metadb": {"bytes":len(contents), "sha256":hashlib.sha256(contents).hexdigest()}})
        (directory / "geoip.metadb").write_bytes(contents)
        _, pins = packager.geo_seeds(self.args)
        self.assertEqual(pins["geoip.metadb"]["bytes"], len(contents))
        (directory / "geoip.metadb").write_bytes(b"x" * len(contents))
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            packager.geo_seeds(self.args)

    def test_geo_inputs_require_both_arguments_and_deny_links(self):
        self.args.geo_dir = str(self.root)
        with self.assertRaisesRegex(ValueError, "supplied together"):
            packager.geo_seeds(self.args)
        directory = self.geo_args({"geoip.metadb":{"bytes":self.core.stat().st_size,"sha256":self.args.core_sha256}})
        (directory / "geoip.metadb").symlink_to(self.core)
        with self.assertRaisesRegex(ValueError, "size/type"):
            packager.geo_seeds(self.args)

    def test_geo_paths_and_bounds_reject_before_running_any_core(self):
        for pins in [{"../escape":{"bytes":1,"sha256":"0"*64}}, {"geoip.metadb":{"bytes":0,"sha256":"0"*64}}, {"geoip.metadb":{"bytes":1,"sha256":"wrong"}}, {"geoip.metadb":{"bytes":True,"sha256":"0"*64}}]:
            self.geo_args(pins)
            with self.assertRaises(ValueError):
                packager.package(self.args)
            self.assertFalse((self.root / "bundle").exists())


if __name__ == "__main__":
    unittest.main()
