"""Verify launcher boundaries without executing a real service or changing system state."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class Launcher(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ms-launch-")
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle with 'quotes'"
        (self.bundle / "bin").mkdir(parents=True)
        shutil.copy2(ROOT / "deploy/launch.sh", self.bundle / "launch")
        self.capture = self.root / "arguments.json"
        binary = self.bundle / "bin/mihomo-server"
        binary.write_text("#!/usr/bin/env python3\nimport json, os, sys\njson.dump(sys.argv[1:], open(os.environ['CAPTURE'], 'w'))\n")
        binary.chmod(0o755)
        self.env = {k:v for k,v in os.environ.items() if not k.startswith(("MIHOMO_SERVER_", "XDG_"))}
        self.env.update(HOME=str(self.root / "home"), CAPTURE=str(self.capture))

    def tearDown(self):
        self.temp.cleanup()

    def run_launcher(self, args=(), **env):
        return subprocess.run(["sh", str(self.bundle / "launch"), *args], env=self.env | env,
                              capture_output=True, text=True)

    def arguments(self):
        return json.loads(self.capture.read_text())

    def test_defaults_do_not_depend_on_working_directory(self):
        result = self.run_launcher()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.arguments(), ["--resource-dir", str(self.bundle / "resources"),
                                           "--data-dir", self.env["HOME"] + "/.local/share/mihomo-server"])

    def test_absolute_xdg_and_explicit_data_override(self):
        for variables, expected in [({"XDG_DATA_HOME":str(self.root / "data space")}, str(self.root / "data space/mihomo-server")),
                                    ({"MIHOMO_SERVER_DATA_DIR":str(self.root / "private")}, str(self.root / "private"))]:
            result = self.run_launcher(**variables)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.arguments()[3], expected)

    def test_invalid_xdg_falls_back_and_explicit_relative_data_fails(self):
        for invalid in ["", "relative"]:
            self.assertEqual(self.run_launcher(XDG_DATA_HOME=invalid).returncode, 0)
            self.assertEqual(self.arguments()[3], self.env["HOME"] + "/.local/share/mihomo-server")
        self.assertNotEqual(self.run_launcher(MIHOMO_SERVER_DATA_DIR="relative").returncode, 0)
        self.assertNotEqual(self.run_launcher(MIHOMO_SERVER_DATA_DIR="/").returncode, 0)

    def test_named_values_replace_legacy_options_and_keep_argument_boundaries(self):
        args = ["--listen", "127.0.0.1:9999", "--public-origin=http://old:9999", "--profile-name", "a 'quoted' name"]
        result = self.run_launcher(args, MIHOMO_SERVER_LISTEN="0.0.0.0:9090",
                                   MIHOMO_SERVER_PUBLIC_ORIGIN="https://example.com")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.arguments()[4:], ["--profile-name", "a 'quoted' name", "--listen", "0.0.0.0:9090",
                                              "--public-origin", "https://example.com"])

    def test_stale_group_authorization_quotes_arguments_for_sg(self):
        tools = self.root / "tools"
        tools.mkdir()
        scripts = {
            "id": "case \"$1\" in -u) echo 1000;; -un) echo alice;; -G) if [ $# = 1 ]; then echo 1000; else echo '1000 1234'; fi;; esac\n",
            "getent": "echo 'mihomo-tun:x:1234:alice'\n",
            "sg": "exec sh -c \"$3\"\n",
        }
        for name, script in scripts.items():
            file = tools / name
            file.write_text("#!/bin/sh\n" + script)
            file.chmod(0o755)
        # The reexecuted launcher sees its updated group, avoiding recursion.
        (tools / "sg").write_text('#!/bin/sh\nexport PATH="$REAL_PATH"\nexec sh -c "$3"\n')
        self.env["REAL_PATH"] = os.environ["PATH"]
        # A real host without mihomo-tun membership will skip group reentry.
        self.env["PATH"] = str(tools) + ":" + os.environ["PATH"]
        args = ["--multi-user", "--profile-name", "spaces 'quotes' $(touch /tmp/ms-unwanted-launch)"]
        result = self.run_launcher(args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.arguments()[4:], args)


if __name__ == "__main__":
    unittest.main()
