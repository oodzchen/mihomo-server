import tempfile
import unittest
from pathlib import Path

from scripts.update_flake_desktop import compute_sri_hash, update_flake


class TestUpdateFlakeDesktop(unittest.TestCase):
    def test_server_pin_changes_without_touching_desktop(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "flake.nix"
            path.write_text('serverRelease = { version = "old"; hash = "old-server"; };\n'
                            'desktopRelease = { version = "old"; hash = "old-desktop"; };\n')
            update_flake(path, "0.2.13", "sha256-server", "serverRelease")
            self.assertIn('serverRelease = { version = "0.2.13"; hash = "sha256-server"; };', path.read_text())
            self.assertIn('desktopRelease = { version = "old"; hash = "old-desktop"; };', path.read_text())

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
    desktopRelease = {
      version = "0.1.0";
      hash = "sha256-oldhasholdhasholdhasholdhasholdhasholdha=";
    };
  };
}
"""
        with tempfile.TemporaryDirectory() as tmpdir:
            flake_file = Path(tmpdir) / "flake.nix"
            flake_file.write_text(sample_flake, encoding="utf-8")

            update_flake(flake_file, "0.2.10", "sha256-newhashnewhashnewhashnewhashnewhashnewha=")
            updated = flake_file.read_text(encoding="utf-8")

            self.assertIn('version = "0.2.10";', updated)
            self.assertIn('hash = "sha256-newhashnewhashnewhashnewhashnewhashnewha=";', updated)


if __name__ == "__main__":
    unittest.main()
