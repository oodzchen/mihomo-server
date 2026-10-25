#!/usr/bin/env python3
"""Update the pinned server release in flake.nix."""

import argparse
import base64
import hashlib
import re
from pathlib import Path

# The release block written below; each published Nix system gets one hash.
PATTERN = re.compile(
    r'(?P<indent>[ \t]*)serverRelease\s*=\s*\{\s*version\s*=\s*"[^"]+";\s*hashes\s*=\s*\{[^{}]*\};\s*\};'
)


def compute_sri_hash(filepath: Path) -> str:
    with open(filepath, "rb") as f:
        digest = hashlib.sha256(f.read()).digest()
    return "sha256-" + base64.b64encode(digest).decode()


def update_flake(flake_path: Path, version: str, hashes: dict[str, str]) -> None:
    content = flake_path.read_text(encoding="utf-8")

    def block(match: re.Match) -> str:
        indent = match["indent"]
        lines = [f'{indent}serverRelease = {{', f'{indent}  version = "{version}";', f"{indent}  hashes = {{"]
        lines += [f'{indent}    {system} = "{sri}";' for system, sri in sorted(hashes.items())]
        lines += [f"{indent}  }};", f"{indent}}};"]
        return "\n".join(lines)

    new_content, count = PATTERN.subn(block, content)
    if count != 1:
        raise RuntimeError(
            f"Expected exactly 1 replacement in {flake_path}, found {count}"
        )
    flake_path.write_text(new_content, encoding="utf-8")


def system_tarball(value: str) -> tuple[str, Path]:
    system, sep, path = value.partition("=")
    if not sep or not re.fullmatch(r"[a-z0-9_]+-linux", system) or not path:
        raise argparse.ArgumentTypeError("expected <nix system>=<tarball>, e.g. x86_64-linux=dist/x.tar.gz")
    return system, Path(path)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Update flake.nix with the CI release version and tarball hashes"
    )
    parser.add_argument(
        "--tarball", required=True, action="append", type=system_tarball,
        help="<nix system>=<server bundle tar.gz>; repeat once per published system",
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
    args = parser.parse_args()
    version = args.tag.removeprefix("v")
    hashes = {system: compute_sri_hash(path) for system, path in args.tarball}
    if len(hashes) != len(args.tarball):
        parser.error("each Nix system may be given only once")
    update_flake(args.flake, version, hashes)
    print(f"Updated {args.flake} serverRelease: version={version}, hashes={hashes}")


if __name__ == "__main__":
    main()
