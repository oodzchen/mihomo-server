"""Installer interface, integrity and sudo transport tests; no system writes."""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/install_remote.sh"


class Installer(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ms-installer-")
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle with spaces"
        self.bundle.mkdir()
        for name in ["launch", "bin/mihomo-server", "mihomo-server.service", "mihomo-server-user", "resources/manifest.json"]:
            p = self.bundle / name
            p.parent.mkdir(exist_ok=True)
            p.write_text("fixture\n")
        self.checksums()
        self.env = {k:v for k,v in os.environ.items() if not k.startswith(("MIHOMO_INSTALL_", "XDG_", "SUDO_"))}
        self.env["MIHOMO_INSTALL_BUNDLE"] = str(self.bundle)

    def checksums(self):
        lines = [hashlib.sha256(p.read_bytes()).hexdigest() + "  " + str(p.relative_to(self.bundle)) + "\n"
                 for p in sorted(self.bundle.rglob("*")) if p.is_file() and p.name != "checksums.sha256"]
        (self.bundle / "checksums.sha256").write_text("".join(lines))

    def tearDown(self):
        self.temp.cleanup()

    def run_shell(self, code, **env):
        return subprocess.run(["bash", "-c", code, "bash", str(SCRIPT)],
                              env=self.env | env, capture_output=True, text=True)

    def test_help_works_from_pipe_without_privileges(self):
        result = subprocess.run(["bash", "-s", "--", "--help"], input=SCRIPT.read_text(),
                                capture_output=True, text=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--uninstall", result.stdout)
        self.assertNotIn("--bundle", result.stdout)

    def test_removed_options_are_rejected_before_action(self):
        for option in ["--system", "--bundle", "--listen", "--purge-data", "--version"]:
            result = subprocess.run(["bash", str(SCRIPT), option], env=self.env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unknown option", result.stderr)

    def test_local_bundle_uses_one_install_action(self):
        result = self.run_shell('source "$1"; root_action() { printf "%s\\n" "$@"; }; main')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[:2], ["install_shared", str(self.bundle)])

    def test_checksum_failure_never_reaches_root_action(self):
        (self.bundle / "launch").write_text("changed")
        result = self.run_shell('source "$1"; root_action() { echo MUTATED; }; main')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("MUTATED", result.stdout)
        self.assertIn("checksum mismatch", result.stderr)

    def test_symlink_bundle_is_rejected(self):
        (self.bundle / "link").symlink_to("launch")
        result = self.run_shell('source "$1"; root_action() { echo MUTATED; }; main')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("MUTATED", result.stdout)
        self.assertIn("unsafe bundle", result.stderr)

    def test_uninstall_needs_no_download_or_bundle(self):
        result = self.run_shell('source "$1"; root_action() { printf "%s\\n" "$@"; }; main --uninstall',
                                MIHOMO_INSTALL_BUNDLE="/missing")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.split(), ["uninstall_shared", "0"])

    def test_purge_requires_uninstall_and_is_forwarded(self):
        result = self.run_shell('source "$1"; root_action() { printf "%s\\n" "$@"; }; main --uninstall --purge')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.split(), ["uninstall_shared", "1"])
        result = self.run_shell('source "$1"; root_action() { echo MUTATED; }; main --purge')
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("MUTATED", result.stdout)
        self.assertIn("only valid with --uninstall", result.stderr)

    def test_sudo_preserves_values_as_arguments_without_shell_evaluation(self):
        tools = self.root / "tools"
        tools.mkdir()
        for name, content in {
            "id": '#!/bin/sh\nif [ "$1" = -u ]; then echo 1000; else echo tester; fi\n',
            "sudo": '#!/bin/sh\n[ "$1" = -- ] && shift\nexec "$@"\n',
        }.items():
            p = tools / name
            p.write_text(content)
            p.chmod(0o755)
        value = str(self.root / "spaces 'quotes' $(touch unexpected)")
        # Replace the system mutator, while exercising the real serialization.
        result = self.run_shell('source "$1"; install_shared() { printf "%s\\n" "$@"; }; '
                                'root_action install_shared "$VALUE"',
                                PATH=str(tools) + ":" + self.env["PATH"], VALUE=value)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), value)

    def test_truncated_pipe_never_calls_main(self):
        source = SCRIPT.read_text().split("# Also usable as an internal library")[0]
        result = subprocess.run(["bash"], input=source, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
