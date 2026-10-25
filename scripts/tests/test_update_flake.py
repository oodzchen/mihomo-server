import argparse
import tempfile
import unittest
from pathlib import Path

from scripts.update_flake import compute_sri_hash, system_tarball, update_flake


class TestUpdateFlake(unittest.TestCase):
    def test_compute_sri_hash(self):
        with tempfile.NamedTemporaryFile() as f:
            f.write(b"hello world")
            f.flush()
            # sha256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
            # base64(digest) = uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek=
            sri = compute_sri_hash(Path(f.name))
            self.assertEqual(sri, "sha256-uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek=")

    def test_update_flake(self):
        sample_flake = """
{
  outputs = { self }: {
    packages.default = ...;
    serverRelease = {
      version = "0.1.0";
      hashes = {
        x86_64-linux = "sha256-oldhasholdhasholdhasholdhasholdhasholdha=";
      };
    };
    targets = { x86_64-linux = "x"; };
  };
}
"""
        with tempfile.TemporaryDirectory() as tmpdir:
            flake_file = Path(tmpdir) / "flake.nix"
            flake_file.write_text(sample_flake, encoding="utf-8")

            update_flake(flake_file, "0.2.10", {"x86_64-linux": "sha256-new-x86", "aarch64-linux": "sha256-new-arm"})
            updated = flake_file.read_text(encoding="utf-8")

            self.assertIn(
                """    serverRelease = {
      version = "0.2.10";
      hashes = {
        aarch64-linux = "sha256-new-arm";
        x86_64-linux = "sha256-new-x86";
      };
    };
    targets = { x86_64-linux = "x"; };
""",
                updated,
            )
            self.assertNotIn("oldhash", updated)

    def test_cli_requires_system_for_each_tarball(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            tarball = Path(tmpdir) / "bundle.tar.gz"
            tarball.write_bytes(b"bundle")
            self.assertEqual(system_tarball(f"aarch64-linux={tarball}"), ("aarch64-linux", tarball))
            with self.assertRaises(argparse.ArgumentTypeError):
                system_tarball(str(tarball))

    def test_update_flake_requires_the_pin(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            flake_file = Path(tmpdir) / "flake.nix"
            flake_file.write_text("{ }\n", encoding="utf-8")
            with self.assertRaises(RuntimeError):
                update_flake(flake_file, "0.2.10", {"x86_64-linux": "sha256-new"})


if __name__ == "__main__":
    unittest.main()
