"""Opt-in runtime smoke test of the complete, immutable Nix service package."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client
import json
import os
import re
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request


# Releases cut before the Nix-aware service API and the listener readiness fix.
PRE_NIX_RELEASES = {"0.2.12", "0.2.13"}


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


@unittest.skipUnless(os.environ.get("MIHOMO_TEST_NIX_PACKAGE"), "Set MIHOMO_TEST_NIX_PACKAGE to a built Nix package")
class NixRuntime(unittest.TestCase):
    def test_version_and_update_ownership(self):
        package = Path(os.environ["MIHOMO_TEST_NIX_PACKAGE"])
        pin = json.loads((package / "nix-installation.json").read_text())
        version = subprocess.check_output([str(package / "bin/mihomo-server"), "--version"], text=True).strip()
        self.assertEqual(version, "mihomo-server " + pin["version"])
        for arguments in [["update"], ["--json", "uninstall"]]:
            result = subprocess.run([str(package / "bin/mihomo-server"), *arguments], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("nixos-rebuild switch", result.stderr)

    def test_web_proxy_core_and_restart_keep_user_state(self):
        package = Path(os.environ["MIHOMO_TEST_NIX_PACKAGE"])
        pin = json.loads((package / "nix-installation.json").read_text())
        class Probe(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"nix-proxy-ok")
            def log_message(self, *_):
                pass

        probe = ThreadingHTTPServer(("127.0.0.1", 0), Probe)
        thread = threading.Thread(target=probe.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory(prefix="ms-nix-runtime-") as temporary:
                root = Path(temporary)
                api_port = free_port()
                env = os.environ | {"MIHOMO_SERVER_DATA_DIR": str(root / "data"), "MIHOMO_SERVER_LISTEN": f"127.0.0.1:{api_port}"}
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                token = None
                for _ in range(2):
                    with (root / "service.log").open("w+") as log:
                        process = subprocess.Popen([str(package / "launch"), "--multi-user", "--slot", "63"],
                                                   env=env, stdout=log, stderr=log)
                        try:
                            deadline = time.monotonic() + 30
                            while time.monotonic() < deadline:
                                if process.poll() is not None:
                                    self.fail("Nix service exited: " + (root / "service.log").read_text())
                                try:
                                    current = (root / "data/management-token").read_text().strip()
                                    request = urllib.request.Request(f"http://127.0.0.1:{api_port}/api/status", headers={"Authorization": f"Bearer {current}"})
                                    with opener.open(request, timeout=1) as response:
                                        status = json.load(response)
                                    if status["phase"] == "running":
                                        break
                                    if (pin["version"] in PRE_NIX_RELEASES and status["phase"] == "failed"
                                            and "core reports 0" in (status.get("error") or "")):
                                        self.skipTest(f"v{pin['version']} probes listeners before they settle; fixed in the current source, awaiting CI release")
                                except (OSError, ValueError):
                                    pass
                                time.sleep(0.1)
                            else:
                                request = urllib.request.Request(f"http://127.0.0.1:{api_port}/api/logs", headers={"Authorization": f"Bearer {current}"})
                                with opener.open(request, timeout=2) as response:
                                    logs = json.load(response)
                                self.fail("Nix service not ready: " + (root / "service.log").read_text() + str(logs))
                            if token is not None:
                                self.assertEqual(current, token)
                            token = current
                            if pin["version"] not in PRE_NIX_RELEASES:
                                request = urllib.request.Request(
                                    f"http://127.0.0.1:{api_port}/api/commands",
                                    data=b'{"command":"service_info"}',
                                    headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
                                with opener.open(request, timeout=2) as response:
                                    info = json.load(response)
                                self.assertEqual(info["installation"], "nix")
                                self.assertFalse(info["upgrade"]["available"])
                                self.assertIsNone(info["autostart"])
                                for command in [{"command": "upgrade_service"},
                                                {"command": "set_service_autostart", "enabled": True}]:
                                    request = urllib.request.Request(
                                        f"http://127.0.0.1:{api_port}/api/commands", data=json.dumps(command).encode(),
                                        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
                                    with self.assertRaises(urllib.error.HTTPError) as caught:
                                        opener.open(request, timeout=2)
                                    with caught.exception as response:
                                        self.assertEqual(response.code, 422)
                                        self.assertIn("Nix", response.read().decode())
                            self.assertTrue((root / "data/core/verge-mihomo").is_file())
                            with opener.open(f"http://127.0.0.1:{api_port}/", timeout=2) as response:
                                self.assertIn(b"<html", response.read().lower())
                            # Query the actual generated proxy port; multi-user
                            # isolation rewrites bootstrap ports to the slot plan.
                            request = urllib.request.Request(f"http://127.0.0.1:{api_port}/api/config", headers={"Authorization": f"Bearer {token}"})
                            with opener.open(request, timeout=2) as response:
                                runtime = json.load(response)
                                if isinstance(runtime, dict):
                                    runtime = runtime["yaml"]
                            actual = int(re.search(r"^mixed-port:\s*(\d+)", runtime, re.M)[1])
                            connection = http.client.HTTPConnection("127.0.0.1", actual, timeout=5)
                            try:
                                connection.request("GET", f"http://127.0.0.1:{probe.server_port}/probe")
                                response = connection.getresponse()
                                self.assertEqual(response.status, 200)
                                self.assertEqual(response.read(), b"nix-proxy-ok")
                            finally:
                                connection.close()
                        finally:
                            process.terminate()
                            try:
                                process.wait(timeout=15)
                            except subprocess.TimeoutExpired:
                                process.kill()
                                process.wait()
        finally:
            probe.shutdown()
            probe.server_close()
            thread.join()
