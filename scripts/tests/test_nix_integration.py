"""Nix installation ownership must not change ordinary installer commands."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class NixIntegration(unittest.TestCase):
    def test_release_wrapper_guards_only_program_installation_commands(self):
        guard = (ROOT / "nix/cli-guard.sh").read_text() + "\nprintf 'executed\\n'\n"
        for arguments in [[], ["info"], ["sub", "update"], ["core", "update"],
                          ["--api", "update", "status"], ["--json", "sub", "update"]]:
            result = subprocess.run(["sh", "-c", guard, "wrapper", *arguments], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "executed\n")
        for arguments in [["update"], ["uninstall", "--purge"], ["--json", "update"],
                          ["--api", "http://localhost:9090", "uninstall"]]:
            result = subprocess.run(["sh", "-c", guard, "wrapper", *arguments], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("executed", result.stdout)
            self.assertIn("nixos-rebuild switch", result.stderr)

    def test_nix_helper_does_not_change_declarative_startup_or_delete_data(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / "mihomo-server-user"
            shutil.copy(ROOT / "deploy/mihomo-server-user", helper)
            (root / "nix-installation.json").write_text('{"kind":"nix"}')
            tools = root / "tools"
            tools.mkdir()
            for name, script in {
                "id": "echo 1000",
                "systemctl": "echo MUTATED >&2; exit 0",
                "systemd-run": "exit 1",
            }.items():
                path = tools / name
                path.write_text("#!/bin/sh\n" + script + "\n")
                path.chmod(0o755)
            env = os.environ | {"HOME": str(root / "home"), "PATH": str(tools) + ":" + os.environ["PATH"]}
            for command in [["enable"], ["disable"], ["purge", "--yes"]]:
                result = subprocess.run(["bash", str(helper), *command], env=env, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("services.mihomo-server.users", result.stderr)
                self.assertNotIn("MUTATED", result.stderr)
