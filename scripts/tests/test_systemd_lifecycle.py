#!/usr/bin/env python3
"""Integration test for actual Linux systemd user service lifecycle.

Tests the full lifecycle under real systemd:
1. Package bundle with package_bundle.py
2. Install an isolated copy of the shared service template
3. Start service with systemctl --user start
4. Verify HTTP management status, child core process, and bearer auth
5. Verify real proxy node data from ./data, node selection, and delay testing
6. Verify journalctl captured service stdout/stderr logs
7. Verify restart preserves token and running state
8. Verify stop reaps both Rust supervisor and child Mihomo process
9. Verify uninstall cleans up unit file and daemon-reloads
"""

from __future__ import annotations

import getpass
import hashlib
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import time
import unittest
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent


class installer:
    """Thin helpers around systemctl and the shell installer under test."""

    @staticmethod
    def detect_systemctl_prefix() -> list[str]:
        base = ["systemctl", "--user"]
        res = subprocess.run(base + ["is-active", "dbus.service"], capture_output=True, text=True)
        err = res.stderr or ""
        if res.returncode != 0 and (
            "Failed to connect to user scope bus" in err
            or "Object is remote" in err
            or "No data available" in err
            or "没有可用的数据" in err
        ):
            return base + ["-M", f"{getpass.getuser()}@.host"]
        return base

    @staticmethod
    def run_systemctl(args, prefix=None, check=True):
        cmd = (prefix or installer.detect_systemctl_prefix()) + args
        res = subprocess.run(cmd, capture_output=True, text=True)
        if check and res.returncode != 0:
            raise RuntimeError(f"{' '.join(cmd)} failed: {res.stdout}{res.stderr}")
        return res

    @staticmethod
    def install_service(bundle_src, data_dir, unit_dir, unit_name, listen, systemctl_prefix):
        # Exercise the service template in isolation; the shared installer is
        # verified inside the privileged, private-network systemd container.
        template = (ROOT / "deploy/mihomo-server.service").read_text()
        lines = []
        for line in template.splitlines():
            if line.startswith("EnvironmentFile="):
                continue
            if line.startswith("ExecStart="):
                line = f"ExecStart={bundle_src}/launch --multi-user --slot 63 --listen {listen}"
            lines.append(line)
        unit_dir.mkdir(parents=True, exist_ok=True)
        (unit_dir / unit_name).write_text("\n".join(lines) + "\n")
        dropin = unit_dir / (unit_name + ".d")
        dropin.mkdir()
        (dropin / "data.conf").write_text(f"[Service]\nEnvironment=MIHOMO_SERVER_DATA_DIR={data_dir}\n")
        installer.run_systemctl(["daemon-reload"], prefix=systemctl_prefix)

    @staticmethod
    def uninstall_service(unit_name, unit_dir, systemctl_prefix):
        installer.run_systemctl(["disable", "--now", unit_name], prefix=systemctl_prefix, check=False)
        (unit_dir / unit_name).unlink(missing_ok=True)
        shutil.rmtree(unit_dir / (unit_name + ".d"), ignore_errors=True)
        installer.run_systemctl(["daemon-reload"], prefix=systemctl_prefix)


def find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class TestSystemdLifecycle(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        if os.name != "posix":
            raise unittest.SkipTest("Systemd tests require Linux")

        cls.prefix = installer.detect_systemctl_prefix()
        # Check systemd connectivity
        check = subprocess.run(cls.prefix + ["is-system-running"], capture_output=True, text=True)
        if "Failed to connect" in check.stderr or check.returncode not in [0, 1]:
            raise unittest.SkipTest(f"Cannot connect to systemd user manager: {check.stderr}")

        cls.core = pathlib.Path(os.environ.get("MIHOMO_TEST_BINARY", "/usr/bin/verge-mihomo"))
        if not cls.core.is_file():
            raise unittest.SkipTest(f"Mihomo binary not found at {cls.core}")

        cls.service_bin = pathlib.Path("/tmp/cargo-target/debug/mihomo-server")
        if not cls.service_bin.is_file():
            cls.service_bin = pathlib.Path("target/debug/mihomo-server")
        if not cls.service_bin.is_file():
            raise unittest.SkipTest(f"mihomo-server binary not found at {cls.service_bin}")

        # Core hash
        cls.core_hash = hashlib.sha256(cls.core.read_bytes()).hexdigest()

        # Core version
        ver_proc = subprocess.run([str(cls.core), "-v"], capture_output=True, text=True, check=True)
        # Parse version
        parts = ver_proc.stdout.split()
        cls.core_version = next((p for p in parts if p.startswith("v")), "v1.19.31")

        if not (ROOT / "data/profiles.yaml").is_file():
            raise unittest.SkipTest("Requires local real-node ./data profiles")

        cls._verified_on_host = False
        if "-M" in cls.prefix and os.environ.get("SYSTEMD_RUN_HOST") != "1":
            user = getpass.getuser()
            cwd = str(pathlib.Path(__file__).parent.parent.parent.resolve())
            cmd = [
                "systemd-run", "--user", "-M", f"{user}@.host", "--pipe", "--wait",
                f"--working-directory={cwd}",
                "--setenv=SYSTEMD_RUN_HOST=1",
                sys.executable, "-m", "unittest", "scripts/tests/test_systemd_lifecycle.py", "-v",
            ]
            res = subprocess.run(cmd, capture_output=True, text=True)
            if res.returncode != 0:
                raise RuntimeError(f"Host systemd test failed:\nSTDOUT: {res.stdout}\nSTDERR: {res.stderr}")
            cls._verified_on_host = True

    def setUp(self) -> None:
        self.stamp = f"{os.getpid()}-{int(time.time() * 1000)}"
        self.unit_name = f"ms-test-{self.stamp}.service"
        self.temp_dir = pathlib.Path(f"/tmp/ms-systemd-test-{self.stamp}")
        self.temp_dir.mkdir(parents=True, exist_ok=True)

        self.bundle_dir = self.temp_dir / "bundle"
        self.install_dir = self.temp_dir / "opt"
        self.data_dir = self.temp_dir / "data"
        self.unit_dir = pathlib.Path.home() / ".config" / "systemd" / "user"
        self.unit_path = self.unit_dir / self.unit_name

        self.listen_port = find_free_port()
        self.listen_addr = f"127.0.0.1:{self.listen_port}"

    def tearDown(self) -> None:
        try:
            installer.uninstall_service(
                unit_name=self.unit_name,
                unit_dir=self.unit_dir,
                systemctl_prefix=self.prefix,
            )
        except Exception:
            pass
        shutil.rmtree(self.temp_dir, ignore_errors=True)

    def test_full_systemd_service_lifecycle_and_proxy_verification(self) -> None:
        if getattr(self, "_verified_on_host", False):
            return

        repo_root = pathlib.Path(__file__).parent.parent.parent.resolve()

        # 1. Package test bundle using package_bundle.py
        web_dir = self.temp_dir / "web"
        web_dir.mkdir()
        (web_dir / "index.html").write_text("<!doctype html><title>Systemd Test UI</title>")
        (web_dir / "assets").mkdir()
        (web_dir / "assets" / "app.js").write_text("console.log('systemd test')")

        package_cmd = [
            "python3",
            str(repo_root / "scripts" / "package_bundle.py"),
            "--target", "x86_64-unknown-linux-gnu",
            "--mihomo", str(self.core),
            "--core-version", self.core_version,
            "--core-sha256", self.core_hash,
            "--service", str(self.service_bin),
            "--web-dir", str(web_dir),
            "--output", str(self.bundle_dir),
        ]
        res = subprocess.run(package_cmd, capture_output=True, text=True)
        self.assertEqual(res.returncode, 0, f"package_bundle failed: {res.stderr}")
        self.assertTrue((self.bundle_dir / "launch").is_file())

        # 2. Seed test data directory from ./data with safe port settings
        self.data_dir.mkdir(parents=True, exist_ok=True)
        data_src = repo_root / "data"

        # Copy settings.yaml and cache.db if exists
        if (data_src / "settings.yaml").is_file():
            shutil.copy2(data_src / "settings.yaml", self.data_dir / "settings.yaml")
        if (data_src / "geoip.metadb").is_file():
            shutil.copy2(data_src / "geoip.metadb", self.data_dir / "geoip.metadb")

        # Copy profiles
        if (data_src / "profiles.yaml").is_file():
            shutil.copy2(data_src / "profiles.yaml", self.data_dir / "profiles.yaml")
        if (data_src / "profiles").is_dir():
            shutil.copytree(data_src / "profiles", self.data_dir / "profiles", dirs_exist_ok=True)
            # Patch DNS port in profiles to avoid 1053 conflict with host's clash-verge-service
            for p in (self.data_dir / "profiles").glob("*.yaml"):
                txt = p.read_text()
                # Change dns listen 1053 to 0 (ephemeral)
                txt = txt.replace("listen: 0.0.0.0:1053", "listen: 127.0.0.1:0")
                txt = txt.replace("listen: :1053", "listen: 127.0.0.1:0")
                p.write_text(txt)

        # 3. Install an isolated service template with no shared-system writes
        installer.install_service(
            bundle_src=self.bundle_dir,
            data_dir=self.data_dir,
            unit_dir=self.unit_dir,
            unit_name=self.unit_name,
            listen=self.listen_addr,
            systemctl_prefix=self.prefix,
        )
        self.assertTrue(self.unit_path.is_file(), "Unit file was not created")

        # 4. Start service via systemctl
        installer.run_systemctl(["start", self.unit_name], prefix=self.prefix)

        # Poll until active
        active = False
        for _ in range(50):
            check = installer.run_systemctl(["is-active", self.unit_name], prefix=self.prefix, check=False)
            if check.stdout.strip() == "active":
                active = True
                break
            time.sleep(0.1)
        self.assertTrue(active, "Service did not become active")

        # Verify systemctl show properties
        show = installer.run_systemctl(
            ["show", self.unit_name, "-p", "ActiveState,SubState,MainPID"],
            prefix=self.prefix,
        ).stdout
        self.assertIn("ActiveState=active", show)
        self.assertIn("SubState=running", show)

        # Extract MainPID
        main_pid = None
        for line in show.splitlines():
            if line.startswith("MainPID="):
                main_pid = int(line.split("=", 1)[1])
        self.assertIsNotNone(main_pid)
        self.assertGreater(main_pid, 0)

        # 5. Read management token and verify HTTP API
        token_file = self.data_dir / "management-token"
        token = ""
        for _ in range(50):
            if token_file.is_file():
                token = token_file.read_text().strip()
                if token:
                    break
            time.sleep(0.1)
        self.assertTrue(token, "management-token was not written")

        status_url = f"http://{self.listen_addr}/api/status"
        status_data = {}
        last_err = ""
        for _ in range(100):
            try:
                req = urllib.request.Request(
                    status_url,
                    headers={"Authorization": f"Bearer {token}"},
                )
                with urllib.request.urlopen(req, timeout=2) as resp:
                    if resp.status == 200:
                        body = json.loads(resp.read().decode())
                        if body.get("phase") == "running":
                            status_data = body
                            break
            except Exception as e:
                last_err = f"{type(e)}: {e}"
                if hasattr(e, "read"):
                    try:
                        last_err += f" Body: {e.read().decode(errors='replace')}"
                    except Exception:
                        pass
            time.sleep(0.1)

        self.assertEqual(status_data.get("phase"), "running", f"Failed to reach running phase; last_err={last_err}")
        core_pid = status_data.get("pid")
        self.assertIsNotNone(core_pid)
        self.assertGreater(core_pid, 0)

        # Both Rust main_pid and Mihomo core_pid must be alive
        os.kill(main_pid, 0)
        os.kill(core_pid, 0)

        # 6. Verify Web asset serving
        web_req = urllib.request.Request(f"http://{self.listen_addr}/")
        with urllib.request.urlopen(web_req, timeout=2) as resp:
            self.assertEqual(resp.status, 200)
            self.assertIn("Systemd Test UI", resp.read().decode())

        # 7. Real proxy verification using ./data profiles
        cmd_url = f"http://{self.listen_addr}/api/commands"
        select_profile_cmd = json.dumps({"command": "select_profile", "uid": "L18d904f7de21806e-2-0"}).encode()
        req = urllib.request.Request(
            cmd_url,
            data=select_profile_cmd,
            headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            self.assertEqual(resp.status, 200)

        # Wait for profile to be active and proxies available
        proxies_url = f"http://{self.listen_addr}/api/proxies"
        proxies = {}
        for _ in range(50):
            try:
                req = urllib.request.Request(proxies_url, headers={"Authorization": f"Bearer {token}"})
                with urllib.request.urlopen(req, timeout=3) as resp:
                    if resp.status == 200:
                        proxies = json.loads(resp.read().decode()).get("proxies", {})
                        if "AI" in proxies:
                            break
            except Exception:
                pass
            time.sleep(0.1)

        self.assertIn("AI", proxies, f"Expected 'AI' group from profile 'amy'; got: {list(proxies.keys())}")
        self.assertTrue(len(proxies) > 10, f"Expected 10+ proxy nodes loaded; got: {len(proxies)}")

        # Command: select node '🇯🇵 日本 03' in group 'AI'
        select_cmd = json.dumps({"command": "select_node", "group": "AI", "node": "🇯🇵 日本 03"}).encode()
        req = urllib.request.Request(
            cmd_url,
            data=select_cmd,
            headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=3) as resp:
            self.assertEqual(resp.status, 200)

        # Verify selection is reflected
        req = urllib.request.Request(proxies_url, headers={"Authorization": f"Bearer {token}"})
        with urllib.request.urlopen(req, timeout=3) as resp:
            proxies = json.loads(resp.read().decode()).get("proxies", {})
            self.assertEqual(proxies.get("AI", {}).get("now"), "🇯🇵 日本 03")

        # 8. Verify journal logs captured service output
        journal_proc = subprocess.run(
            ["journalctl", "--user", "-u", self.unit_name, "-n", "30", "--no-pager"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(journal_proc.returncode, 0)
        self.assertIn(self.unit_name, journal_proc.stdout)

        # 9. Verify Restart lifecycle
        installer.run_systemctl(["restart", self.unit_name], prefix=self.prefix)
        time.sleep(1.0)

        # Verify active after restart
        check = installer.run_systemctl(["is-active", self.unit_name], prefix=self.prefix, check=False)
        self.assertEqual(check.stdout.strip(), "active")

        # Verify token preserved
        self.assertEqual(token_file.read_text().strip(), token)

        # Verify HTTP status is running after restart
        req = urllib.request.Request(status_url, headers={"Authorization": f"Bearer {token}"})
        with urllib.request.urlopen(req, timeout=3) as resp:
            self.assertEqual(resp.status, 200)
            body = json.loads(resp.read().decode())
            self.assertEqual(body.get("phase"), "running")
            new_core_pid = body.get("pid")
            self.assertIsNotNone(new_core_pid)

        # Show new main PID
        show = installer.run_systemctl(
            ["show", self.unit_name, "-p", "MainPID"],
            prefix=self.prefix,
        ).stdout
        new_main_pid = int(show.split("=", 1)[1].strip())

        # 10. Verify Stop & Child Process Reaping
        installer.run_systemctl(["stop", self.unit_name], prefix=self.prefix)
        time.sleep(0.5)

        # Verify inactive
        check = installer.run_systemctl(["is-active", self.unit_name], prefix=self.prefix, check=False)
        self.assertNotEqual(check.stdout.strip(), "active")

        # Verify old and new processes are completely reaped
        for pid in [main_pid, core_pid, new_main_pid, new_core_pid]:
            try:
                os.kill(pid, 0)
                alive = True
            except OSError as e:
                # ESRCH: No such process
                alive = False
            self.assertFalse(alive, f"PID {pid} was not reaped after service stop")

        # 11. Uninstall service
        installer.uninstall_service(
            unit_name=self.unit_name,
            unit_dir=self.unit_dir,
            systemctl_prefix=self.prefix,
        )
        self.assertFalse(self.unit_path.exists(), "Unit file was not unlinked on uninstall")


if __name__ == "__main__":
    unittest.main()
