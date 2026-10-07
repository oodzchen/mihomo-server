#!/usr/bin/env python3
"""Update the pinned desktop and optional server CI releases in flake.nix."""

import argparse
import base64
import hashlib
import re
from pathlib import Path


def compute_sri_hash(filepath: Path) -> str:
    with open(filepath, "rb") as f:
        digest = hashlib.sha256(f.read()).digest()
    return "sha256-" + base64.b64encode(digest).decode()


def update_flake(flake_path: Path, version: str, sri_hash: str, release: str = "desktopRelease") -> None:
    content = flake_path.read_text(encoding="utf-8")
    if release not in {"desktopRelease", "serverRelease"}:
        raise ValueError("Unknown release block")
    pattern = r'(' + release + r'\s*=\s*\{\s*version\s*=\s*)"[^"]+"(;\s*hash\s*=\s*)"[^"]+"(;\s*\};)'
    replacement = f'\\g<1>"{version}"\\g<2>"{sri_hash}"\\g<3>'

    new_content, count = re.subn(pattern, replacement, content)
    if count != 1:
        raise RuntimeError(
            f"Expected exactly 1 replacement in {flake_path}, found {count}"
        )
    flake_path.write_text(new_content, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Update flake.nix with CI release versions and tarball hashes"
    )
    parser.add_argument(
        "--tarball", required=True, type=Path, help="Path to desktop tar.gz archive"
    )
    parser.add_argument(
        "--tag", required=True, help="Git release tag (e.g. v0.2.10)"
    )
    parser.add_argument(
        "--flake",
        type=Path,
        default=Path("flake.nix"),
        help="Path to flake.nix (default: flake.nix)",
    )

    parser.add_argument("--server-tarball", type=Path, help="Also update the precompiled server release")
    args = parser.parse_args()
    version = args.tag.removeprefix("v")
    sri_hash = compute_sri_hash(args.tarball)
    update_flake(args.flake, version, sri_hash)
    if args.server_tarball:
        update_flake(args.flake, version, compute_sri_hash(args.server_tarball), "serverRelease")
    print(f"Updated {args.flake} desktopRelease: version={version}, hash={sri_hash}")


if __name__ == "__main__":
    main()
