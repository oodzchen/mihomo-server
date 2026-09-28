#!/usr/bin/env python3
"""Unit tests for Linux systemd installer and lifecycle manager."""

from __future__ import annotations

import os
import pathlib
import stat
import tempfile
import unittest
from unittest.mock import MagicMock, patch

import scripts.install_service as installer


class TestInstallService(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp_dir.name)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def create_mock_bundle(self) -> pathlib.Path:
        bundle = self.root / "bundle"
        bundle.mkdir()
        (bundle / "launch").write_text("#!/bin/sh\nexit 0\n")
        (bundle / "launch").chmod(0o755)

        bin_dir = bundle / "bin"
        bin_dir.mkdir()
        (bin_dir / "mihomo-server").write_text("binary")
        (bin_dir / "mihomo-server").chmod(0o755)

        res_dir = bundle / "resources"
        res_dir.mkdir()
        (res_dir / "manifest.json").write_text('{"schema_version": 1}\n')

        core_dir = res_dir / "core"
        core_dir.mkdir()
        (core_dir / "verge-mihomo").write_text("core")
        (core_dir / "verge-mihomo").chmod(0o755)

        return bundle

    def test_render_unit_content_default(self) -> None:
        home = pathlib.Path.home()
        exec_start = "%h/.local/opt/mihomo-server/launch"
        data_dir = home / ".local" / "share" / "mihomo-server"
        content = installer.render_unit_content(exec_start, data_dir)

        self.assertIn("Environment=MIHOMO_SERVER_DATA_DIR=%h/.local/share/mihomo-server", content)
        self.assertIn("ExecStart=%h/.local/opt/mihomo-server/launch", content)
        self.assertIn("KillMode=mixed", content)
        self.assertIn("KillSignal=SIGTERM", content)
        self.assertIn("TimeoutStopSec=30", content)
        self.assertIn("UMask=0077", content)
        self.assertIn("WantedBy=default.target", content)

    def test_render_unit_content_custom_paths(self) -> None:
        exec_start = "/opt/custom-mihomo/launch --listen 127.0.0.1:9095 --no-start"
        data_dir = pathlib.Path("/var/lib/mihomo-data")
        content = installer.render_unit_content(exec_start, data_dir)

        self.assertIn("Environment=MIHOMO_SERVER_DATA_DIR=/var/lib/mihomo-data", content)
        self.assertIn("ExecStart=/opt/custom-mihomo/launch --listen 127.0.0.1:9095 --no-start", content)

    def test_validate_bundle(self) -> None:
        bundle = self.create_mock_bundle()
        # Valid bundle passes
        installer.validate_bundle(bundle)

        # Non-directory raises
        with self.assertRaises(ValueError):
            installer.validate_bundle(bundle / "launch")

        # Missing launch raises
        (bundle / "launch").unlink()
        with self.assertRaises(ValueError):
            installer.validate_bundle(bundle)

        # Restore launch, missing manifest raises
        (bundle / "launch").write_text("#!/bin/sh\n")
        (bundle / "resources" / "manifest.json").unlink()
        with self.assertRaises(ValueError):
            installer.validate_bundle(bundle)

    def test_install_bundle_files_and_permissions(self) -> None:
        bundle = self.create_mock_bundle()
        dest = self.root / "installed-opt"

        installer.install_bundle_files(bundle, dest)

        self.assertTrue((dest / "launch").is_file())
        self.assertTrue((dest / "bin" / "mihomo-server").is_file())
        self.assertTrue((dest / "resources" / "core" / "verge-mihomo").is_file())

        # Check permissions: launch should have execute bit set
        launch_mode = (dest / "launch").stat().st_mode
        self.assertTrue(bool(launch_mode & stat.S_IXUSR))

        bin_mode = (dest / "bin" / "mihomo-server").stat().st_mode
        self.assertTrue(bool(bin_mode & stat.S_IXUSR))

        core_mode = (dest / "resources" / "core" / "verge-mihomo").stat().st_mode
        self.assertTrue(bool(core_mode & stat.S_IXUSR))

    def test_install_service_requires_absolute_data_dir(self) -> None:
        with self.assertRaises(ValueError):
            installer.install_service(
                bundle_src=None,
                data_dir=pathlib.Path("relative/data"),
            )

    def test_install_service_dry_run(self) -> None:
        bundle = self.create_mock_bundle()
        install_dir = self.root / "dry-opt"
        data_dir = self.root / "dry-data"
        unit_dir = self.root / "dry-units"

        unit_path = installer.install_service(
            bundle_src=bundle,
            install_dir=install_dir,
            data_dir=data_dir,
            unit_dir=unit_dir,
            dry_run=True,
        )

        self.assertFalse(install_dir.exists())
        self.assertFalse(data_dir.exists())
        self.assertFalse(unit_path.exists())

    @patch("scripts.install_service.run_systemctl")
    def test_install_service_lifecycle_mocked(self, mock_systemctl: MagicMock) -> None:
        bundle = self.create_mock_bundle()
        install_dir = self.root / "test-opt"
        data_dir = self.root / "test-data"
        unit_dir = self.root / "test-units"
        unit_name = "test-mihomo.service"

        unit_path = installer.install_service(
            bundle_src=bundle,
            install_dir=install_dir,
            data_dir=data_dir,
            unit_dir=unit_dir,
            unit_name=unit_name,
            listen="127.0.0.1:9095",
            extra_args="--no-start",
            enable=True,
            start=True,
        )

        self.assertTrue(unit_path.is_file())
        self.assertEqual(unit_path.stat().st_mode & 0o777, 0o644)

        # Data dir must be private (0700)
        self.assertEqual(data_dir.stat().st_mode & 0o777, 0o700)

        # Verify unit content
        unit_content = unit_path.read_text()
        self.assertIn("ExecStart=", unit_content)
        self.assertIn("--listen 127.0.0.1:9095", unit_content)
        self.assertIn("--no-start", unit_content)

        # Verify systemctl calls
        mock_systemctl.assert_any_call(["daemon-reload"], prefix=None)
        mock_systemctl.assert_any_call(["enable", unit_name], prefix=None)
        mock_systemctl.assert_any_call(["start", unit_name], prefix=None)

    @patch("scripts.install_service.run_systemctl")
    def test_uninstall_service_mocked(self, mock_systemctl: MagicMock) -> None:
        install_dir = self.root / "uninst-opt"
        install_dir.mkdir()
        data_dir = self.root / "uninst-data"
        data_dir.mkdir()
        unit_dir = self.root / "uninst-units"
        unit_dir.mkdir()
        unit_name = "uninst-mihomo.service"
        unit_file = unit_dir / unit_name
        unit_file.write_text("unit")

        # Mock is-active returning 0 (active)
        active_proc = MagicMock()
        active_proc.returncode = 0
        mock_systemctl.return_value = active_proc

        installer.uninstall_service(
            unit_name=unit_name,
            unit_dir=unit_dir,
            install_dir=install_dir,
            data_dir=data_dir,
            remove_bundle=True,
            purge_data=False,
        )

        self.assertFalse(unit_file.exists())
        self.assertFalse(install_dir.exists())
        self.assertTrue(data_dir.exists())  # Data preserved!

        mock_systemctl.assert_any_call(["stop", unit_name], prefix=None, check=False)
        mock_systemctl.assert_any_call(["disable", unit_name], prefix=None, check=False)
        mock_systemctl.assert_any_call(["daemon-reload"], prefix=None, check=False)


if __name__ == "__main__":
    unittest.main()
