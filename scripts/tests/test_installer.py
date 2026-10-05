"""Installer interface, integrity and sudo transport tests; no system writes."""
import hashlib
from http.server import BaseHTTPRequestHandler, HTTPServer
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/install_remote.sh"
HELPER = ROOT / "deploy/mihomo-server-user"


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

    def test_pkexec_elevation_preserves_values_as_arguments(self):
        tools = self.root / "tools"
        tools.mkdir()
        for name, content in {
            "id": '#!/bin/sh\nif [ "$1" = -u ]; then echo 1000; else echo tester; fi\n',
            "pkexec": '#!/bin/sh\ncase "$1" in /*) exec "$@" ;; esac\necho "relative program" >&2; exit 1\n',
            "sudo": '#!/bin/sh\necho SUDO; exit 1\n',
        }.items():
            p = tools / name
            p.write_text(content)
            p.chmod(0o755)
        value = str(self.root / "spaces 'quotes' $(touch unexpected)")
        result = self.run_shell('source "$1"; install_shared() { printf "%s\\n" "$@"; }; '
                                'require_elevation; root_action install_shared "$VALUE"',
                                PATH=str(tools) + ":" + self.env["PATH"], VALUE=value,
                                MIHOMO_INSTALL_ELEVATE="pkexec")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), value)
        result = self.run_shell('source "$1"; require_elevation',
                                PATH=str(tools) + ":" + self.env["PATH"], MIHOMO_INSTALL_ELEVATE="doas")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported MIHOMO_INSTALL_ELEVATE", result.stderr)

    def test_truncated_pipe_never_calls_main(self):
        source = SCRIPT.read_text().split("# Also usable as an internal library")[0]
        result = subprocess.run(["bash"], input=source, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")

    def test_latest_tag_reads_redirect_without_api(self):
        class Redirect(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(302 if self.path == "/o/r/releases/latest" else 404)
                self.send_header("Location", "/o/r/releases/tag/v1.2.3")
                self.send_header("Content-Length", "0")
                self.end_headers()

            def log_message(self, *args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Redirect)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.shutdown)
        base = f"http://127.0.0.1:{server.server_port}"
        result = self.run_shell('source "$1"; latest_tag "$BASE/o/r/releases/latest"', BASE=base)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "v1.2.3")
        result = self.run_shell('source "$1"; latest_tag "$BASE/missing/releases/latest"', BASE=base)
        self.assertNotEqual(result.returncode, 0)


class UserManagementLinks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ms-management-link-")
        self.data = Path(self.temp.name) / "data with spaces"
        self.data.mkdir()
        self.token = "a1" * 32
        (self.data / "management-token").write_text(self.token + "\n")

    def tearDown(self):
        self.temp.cleanup()

    def info(self, listen="127.0.0.1:20030", origin="", running=True, field="manage", state="active"):
        # Load functions only; exercise real endpoint/info without a user manager.
        source = HELPER.read_text().split("\ncommand=${1:-info}")[0]
        source += '''
systemctl() {
    case "$*" in
        *MainPID*) echo "$TEST_PID" ;;
        *is-active*) echo "$TEST_STATE" ;;
    esac
}
slot() { echo 3; }
current_log() { echo 'multi-user slot 3 (available)'; }
process_argument() {
    case "$2" in
        --listen) echo "$TEST_LISTEN" ;;
        --public-origin) echo "$TEST_ORIGIN" ;;
        --data-dir) echo "$TEST_DATA" ;;
    esac
}
request() {
    case "$1 ${2:-}" in
        '/api/config ') printf '%s' '{"yaml":"allow-lan: false\\nmixed-port: 1089\\nport: 0\\n"}' ;;
        '/api/status ') echo '{"config_revision":"rev-1","phase":"running","version":"v1.19.32"}' ;;
        '/api/commands {"command":"service_version"}') echo '"1.2.3"' ;;
    esac
}
DATA_DIR="$TEST_DATA"
ENV_FILE="$TEST_DATA/env"
info
'''
        result = subprocess.run(["bash"], input=source, capture_output=True, text=True,
                                env=os.environ | {"TEST_DATA": str(self.data), "TEST_LISTEN": listen,
                                                  "TEST_ORIGIN": origin, "TEST_PID": "42" if running else "0",
                                                  "TEST_STATE": state})
        self.assertEqual(result.returncode, 0, result.stderr)
        return next(line.removeprefix(f"{field}:").strip() for line in result.stdout.splitlines()
                    if line.startswith(f"{field}:"))

    def test_management_link_contains_token_for_default_and_wildcard_listeners(self):
        for listen, address in [("127.0.0.1:20030", "127.0.0.1:20030"),
                                ("0.0.0.0:20030", "127.0.0.1:20030"),
                                ("[::]:20030", "[::1]:20030")]:
            with self.subTest(listen=listen):
                self.assertEqual(self.info(listen), f"http://{address}/#token={self.token}")

    def test_public_origin_is_used_for_browser_login(self):
        self.assertEqual(self.info("0.0.0.0:20030", "https://manage.example/"),
                         f"https://manage.example/#token={self.token}")

    def test_stopped_instance_uses_saved_token_and_slot_address(self):
        self.assertEqual(self.info(running=False), f"http://127.0.0.1:20030/#token={self.token}")

    def test_missing_or_invalid_token_keeps_plain_management_link(self):
        (self.data / "management-token").unlink()
        self.assertEqual(self.info(), "http://127.0.0.1:20030")
        (self.data / "management-token").write_text("invalid&extra=parameter\n")
        self.assertEqual(self.info(), "http://127.0.0.1:20030")

    def test_versions_come_from_the_running_service(self):
        self.assertEqual(self.info(field="version"), "1.2.3")
        self.assertEqual(self.info(field="core"), "v1.19.32")
        self.assertEqual(self.info(field="version", state="inactive"), "unknown")
        self.assertEqual(self.info(field="core", state="inactive"), "unknown (core not running)")

    def test_proxy_port_follows_runtime_config_then_saved_settings(self):
        self.assertEqual(self.info(field="proxy"), "HTTP/SOCKS 127.0.0.1:1089")
        self.assertEqual(self.info(field="proxy", state="inactive"), "HTTP/SOCKS 127.0.0.1:20031")
        (self.data / "settings.yaml").write_text("schema_version: 1\nruntime:\n  mixed-port: 7899\n  mode: rule\n")
        self.assertEqual(self.info(field="proxy", state="inactive"), "HTTP/SOCKS 127.0.0.1:7899")


if __name__ == "__main__":
    unittest.main()
