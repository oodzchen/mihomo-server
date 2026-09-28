#!/usr/bin/env python3
"""Linux systemd user service installer and lifecycle manager for mihomo-server.

Provides commands to install, uninstall, enable, disable, start, stop, restart,
check status, and view journal logs for mihomo-server under systemd.
"""

from __future__ import annotations

import argparse
import getpass
import os
import pathlib
import shutil
import stat
import subprocess
import sys


DEFAULT_INSTALL_DIR = pathlib.Path.home() / ".local" / "opt" / "mihomo-server"
DEFAULT_DATA_DIR = pathlib.Path.home() / ".local" / "share" / "mihomo-server"
DEFAULT_UNIT_DIR = pathlib.Path.home() / ".config" / "systemd" / "user"
DEFAULT_UNIT_NAME = "mihomo-server.service"


def detect_systemctl_prefix() -> list[str]:
    """Detect appropriate systemctl prefix for user services.

    In environments where PID/user namespaces are separated (such as sandboxes
    or containers), direct `systemctl --user` may fail with remote credential errors.
    In such cases, connecting via machined (`-M <user>@.host`) works seamlessly.
    """
    override = os.environ.get("SYSTEMCTL_USER_PREFIX")
    if override:
        return override.strip().split()

    base = ["systemctl", "--user"]
    try:
        res = subprocess.run(
            base + ["is-active", "dbus.service"],
            capture_output=True,
            text=True,
            timeout=2,
        )
        if res.returncode == 0:
            return base
        # Check for remote credential / local transport disconnects
        err = res.stderr or ""
        if (
            "Failed to connect to user scope bus" in err
            or "Object is remote" in err
            or "没有可用的数据" in err
            or "No data available" in err
        ):
            user = getpass.getuser()
            candidate = base + ["-M", f"{user}@.host"]
            res2 = subprocess.run(
                candidate + ["is-active", "dbus.service"],
                capture_output=True,
                text=True,
                timeout=2,
            )
            if res2.returncode == 0 or "Failed to connect" not in (res2.stderr or ""):
                return candidate
    except Exception:
        pass
    return base


def run_systemctl(args: list[str], prefix: list[str] | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    """Execute a systemctl user command using the detected prefix."""
    cmd = (prefix or detect_systemctl_prefix()) + args
    res = subprocess.run(cmd, capture_output=True, text=True)
    if check and res.returncode != 0:
        raise RuntimeError(
            f"Command {' '.join(cmd)} failed with code {res.returncode}:\n"
            f"STDOUT: {res.stdout.strip()}\n"
            f"STDERR: {res.stderr.strip()}"
        )
    return res


def validate_bundle(bundle_dir: pathlib.Path) -> None:
    """Validate that bundle_dir contains required bundle artifacts."""
    if not bundle_dir.is_dir():
        raise ValueError(f"Bundle directory does not exist or is not a directory: {bundle_dir}")
    launch = bundle_dir / "launch"
    if not launch.is_file():
        raise ValueError(f"Bundle missing launch script: {launch}")
    service_bin = bundle_dir / "bin" / "mihomo-server"
    if not service_bin.is_file():
        raise ValueError(f"Bundle missing service binary: {service_bin}")
    manifest = bundle_dir / "resources" / "manifest.json"
    if not manifest.is_file():
        raise ValueError(f"Bundle missing manifest: {manifest}")


def render_unit_content(
    exec_start: str,
    data_dir: pathlib.Path,
    extra_env: dict[str, str] | None = None,
) -> str:
    """Generate systemd user service unit content."""
    home = str(pathlib.Path.home())
    data_str = str(data_dir)
    if data_str.startswith(home):
        env_data = "%h" + data_str[len(home):]
    else:
        env_data = data_str

    env_lines = [f"Environment=MIHOMO_SERVER_DATA_DIR={env_data}"]
    if extra_env:
        for k, v in sorted(extra_env.items()):
            env_lines.append(f"Environment={k}={v}")

    env_block = "\n".join(env_lines)

    return f"""[Unit]
Description=Headless Mihomo management service
After=network.target

[Service]
Type=simple
{env_block}
ExecStart={exec_start}
Restart=on-failure
RestartSec=3
KillSignal=SIGTERM
KillMode=mixed
TimeoutStopSec=30
UMask=0077

[Install]
WantedBy=default.target
"""


def install_bundle_files(src_bundle: pathlib.Path, dest_dir: pathlib.Path) -> None:
    """Copy bundle files to target install directory with strict permissions."""
    validate_bundle(src_bundle)
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest_dir.chmod(0o755)

    for item in src_bundle.iterdir():
        target = dest_dir / item.name
        if item.is_dir():
            if target.exists():
                shutil.rmtree(target)
            shutil.copytree(item, target, symlinks=False)
        else:
            shutil.copy2(item, target)

    # Ensure executable permissions
    launch = dest_dir / "launch"
    if launch.exists():
        launch.chmod(0o755)

    bin_dir = dest_dir / "bin"
    if bin_dir.exists():
        for f in bin_dir.iterdir():
            if f.is_file():
                f.chmod(0o755)

    core_dir = dest_dir / "resources" / "core"
    if core_dir.exists():
        for f in core_dir.iterdir():
            if f.is_file():
                f.chmod(0o755)


def install_service(
    bundle_src: pathlib.Path | None,
    install_dir: pathlib.Path = DEFAULT_INSTALL_DIR,
    data_dir: pathlib.Path = DEFAULT_DATA_DIR,
    unit_dir: pathlib.Path = DEFAULT_UNIT_DIR,
    unit_name: str = DEFAULT_UNIT_NAME,
    listen: str | None = None,
    extra_args: list[str] | None = None,
    enable: bool = False,
    start: bool = False,
    systemctl_prefix: list[str] | None = None,
    dry_run: bool = False,
) -> pathlib.Path:
    """Install bundle and systemd user unit."""
    if not data_dir.is_absolute():
        raise ValueError(f"Data directory must be an absolute path: {data_dir}")

    # 1. Install or verify bundle
    if bundle_src:
        bundle_src = bundle_src.resolve()
        install_dir = install_dir.resolve()
        if not dry_run and bundle_src != install_dir:
            install_bundle_files(bundle_src, install_dir)
    else:
        install_dir = install_dir.resolve()
        if not dry_run:
            validate_bundle(install_dir)

    # 2. Ensure data directory exists with private permissions (0700)
    if not dry_run:
        data_dir.mkdir(parents=True, exist_ok=True)
        data_dir.chmod(0o700)

    # 3. Construct ExecStart command
    home = str(pathlib.Path.home())
    install_str = str(install_dir)
    if install_str.startswith(home):
        launch_path = "%h" + install_str[len(home):] + "/launch"
    else:
        launch_path = str(install_dir / "launch")

    exec_parts = [launch_path]
    if listen:
        exec_parts.extend(["--listen", listen])
    if extra_args:
        if isinstance(extra_args, str):
            exec_parts.extend(extra_args.strip().split())
        else:
            exec_parts.extend(extra_args)
    exec_start = " ".join(exec_parts)

    unit_content = render_unit_content(exec_start, data_dir)

    # 4. Write unit file
    unit_path = unit_dir / unit_name
    if dry_run:
        print(f"[DRY-RUN] Target unit file: {unit_path}")
        print(f"[DRY-RUN] Unit content:\n{unit_content}")
        return unit_path

    unit_dir.mkdir(parents=True, exist_ok=True)
    unit_path.write_text(unit_content)
    unit_path.chmod(0o644)

    # 5. Reload daemon
    run_systemctl(["daemon-reload"], prefix=systemctl_prefix)

    # 6. Enable / Start if requested
    if enable:
        run_systemctl(["enable", unit_name], prefix=systemctl_prefix)
    if start:
        run_systemctl(["start", unit_name], prefix=systemctl_prefix)

    return unit_path


def uninstall_service(
    unit_name: str = DEFAULT_UNIT_NAME,
    unit_dir: pathlib.Path = DEFAULT_UNIT_DIR,
    install_dir: pathlib.Path = DEFAULT_INSTALL_DIR,
    data_dir: pathlib.Path = DEFAULT_DATA_DIR,
    remove_bundle: bool = False,
    purge_data: bool = False,
    systemctl_prefix: list[str] | None = None,
) -> None:
    """Stop, disable, and remove systemd user unit."""
    # Check if active, stop if so
    active_check = run_systemctl(["is-active", unit_name], prefix=systemctl_prefix, check=False)
    if active_check.returncode == 0:
        run_systemctl(["stop", unit_name], prefix=systemctl_prefix, check=False)

    # Disable unit
    run_systemctl(["disable", unit_name], prefix=systemctl_prefix, check=False)

    # Unlink unit file
    unit_path = unit_dir / unit_name
    unit_path.unlink(missing_ok=True)

    # Reload daemon
    run_systemctl(["daemon-reload"], prefix=systemctl_prefix, check=False)

    # Clean bundle if requested
    if remove_bundle and install_dir.exists():
        shutil.rmtree(install_dir)

    # Purge data if requested
    if purge_data and data_dir.exists():
        shutil.rmtree(data_dir)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Install and manage mihomo-server systemd user service."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # install
    p_inst = subparsers.add_parser("install", help="Install bundle and systemd user unit")
    p_inst.add_argument("--bundle", type=pathlib.Path, help="Source bundle directory to install from")
    p_inst.add_argument("--install-dir", type=pathlib.Path, default=DEFAULT_INSTALL_DIR, help=f"Install destination (default: {DEFAULT_INSTALL_DIR})")
    p_inst.add_argument("--data-dir", type=pathlib.Path, default=DEFAULT_DATA_DIR, help=f"Data directory (default: {DEFAULT_DATA_DIR})")
    p_inst.add_argument("--unit-dir", type=pathlib.Path, default=DEFAULT_UNIT_DIR, help=f"Systemd user unit directory (default: {DEFAULT_UNIT_DIR})")
    p_inst.add_argument("--unit-name", default=DEFAULT_UNIT_NAME, help=f"Unit name (default: {DEFAULT_UNIT_NAME})")
    p_inst.add_argument("--listen", help="Management HTTP listener (e.g. 127.0.0.1:9090)")
    p_inst.add_argument("--extra-args", type=str, help="Extra arguments passed to launcher (e.g. '--no-start')")
    p_inst.add_argument("--enable", action="store_true", help="Enable service at boot/login")
    p_inst.add_argument("--start", action="store_true", help="Start service immediately after install")
    p_inst.add_argument("--dry-run", action="store_true", help="Print configuration without making changes")

    # uninstall
    p_uninst = subparsers.add_parser("uninstall", help="Stop, disable and remove systemd service unit")
    p_uninst.add_argument("--unit-name", default=DEFAULT_UNIT_NAME, help=f"Unit name (default: {DEFAULT_UNIT_NAME})")
    p_uninst.add_argument("--unit-dir", type=pathlib.Path, default=DEFAULT_UNIT_DIR, help=f"Systemd user unit directory (default: {DEFAULT_UNIT_DIR})")
    p_uninst.add_argument("--install-dir", type=pathlib.Path, default=DEFAULT_INSTALL_DIR, help=f"Install destination to remove if requested")
    p_uninst.add_argument("--data-dir", type=pathlib.Path, default=DEFAULT_DATA_DIR, help=f"Data directory to purge if requested")
    p_uninst.add_argument("--remove-bundle", action="store_true", help="Delete installed bundle files")
    p_uninst.add_argument("--purge-data", action="store_true", help="Delete persistent data directory (CAUTION)")

    # Lifecycle commands
    for action in ["start", "stop", "restart", "enable", "disable", "status", "is-active"]:
        p_act = subparsers.add_parser(action, help=f"{action.capitalize()} the service unit")
        p_act.add_argument("--unit-name", default=DEFAULT_UNIT_NAME, help=f"Unit name (default: {DEFAULT_UNIT_NAME})")

    # logs
    p_logs = subparsers.add_parser("logs", help="View journalctl logs for the service unit")
    p_logs.add_argument("--unit-name", default=DEFAULT_UNIT_NAME, help=f"Unit name (default: {DEFAULT_UNIT_NAME})")
    p_logs.add_argument("-n", "--lines", type=int, default=50, help="Number of journal lines to show")
    p_logs.add_argument("-f", "--follow", action="store_true", help="Follow journal output")

    # unit (render only)
    p_unit = subparsers.add_parser("unit", help="Print rendered unit file content to stdout")
    p_unit.add_argument("--install-dir", type=pathlib.Path, default=DEFAULT_INSTALL_DIR)
    p_unit.add_argument("--data-dir", type=pathlib.Path, default=DEFAULT_DATA_DIR)
    p_unit.add_argument("--listen", help="Management HTTP listener")
    p_unit.add_argument("--extra-args", type=str, help="Extra arguments passed to launcher")

    args = parser.parse_args()

    if args.command == "install":
        unit = install_service(
            bundle_src=args.bundle,
            install_dir=args.install_dir,
            data_dir=args.data_dir,
            unit_dir=args.unit_dir,
            unit_name=args.unit_name,
            listen=args.listen,
            extra_args=args.extra_args,
            enable=args.enable,
            start=args.start,
            dry_run=args.dry_run,
        )
        if not args.dry_run:
            print(f"Installed {args.unit_name} -> {unit}")

    elif args.command == "uninstall":
        uninstall_service(
            unit_name=args.unit_name,
            unit_dir=args.unit_dir,
            install_dir=args.install_dir,
            data_dir=args.data_dir,
            remove_bundle=args.remove_bundle,
            purge_data=args.purge_data,
        )
        print(f"Uninstalled {args.unit_name}")

    elif args.command in ["start", "stop", "restart", "enable", "disable"]:
        run_systemctl([args.command, args.unit_name], check=True)
        print(f"{args.command.capitalize()}ed {args.unit_name}")

    elif args.command == "status":
        res = run_systemctl(["status", args.unit_name], check=False)
        print(res.stdout if res.stdout else res.stderr)
        sys.exit(res.returncode)

    elif args.command == "is-active":
        res = run_systemctl(["is-active", args.unit_name], check=False)
        print(res.stdout.strip())
        sys.exit(res.returncode)

    elif args.command == "logs":
        cmd = ["journalctl", "--user", "-u", args.unit_name, "-n", str(args.lines), "--no-pager"]
        if args.follow:
            cmd.append("-f")
        res = subprocess.run(cmd)
        sys.exit(res.returncode)

    elif args.command == "unit":
        home = str(pathlib.Path.home())
        install_str = str(args.install_dir.resolve())
        if install_str.startswith(home):
            launch_path = "%h" + install_str[len(home):] + "/launch"
        else:
            launch_path = str(args.install_dir / "launch")
        exec_parts = [launch_path]
        if args.listen:
            exec_parts.extend(["--listen", args.listen])
        if args.extra_args:
            if isinstance(args.extra_args, str):
                exec_parts.extend(args.extra_args.strip().split())
            else:
                exec_parts.extend(args.extra_args)
        content = render_unit_content(" ".join(exec_parts), args.data_dir.resolve())
        print(content)


if __name__ == "__main__":
    main()
