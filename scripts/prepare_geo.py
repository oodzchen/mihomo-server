#!/usr/bin/env python3
"""Download a pinned Geo seed before packaging; never contact the network on the device."""
import argparse
import hashlib
import json
from pathlib import Path
import tempfile
from urllib.request import urlopen

from package_bundle import GEO_NAMES, MAX_GEO_FILE


def prepare(name, url, expected_hash, directory):
    if name not in GEO_NAMES:
        raise ValueError("unsupported Geo filename")
    if not url.startswith("https://"):
        raise ValueError("Geo source must use HTTPS")
    if len(expected_hash) != 64 or any(c not in "0123456789abcdefABCDEF" for c in expected_hash):
        raise ValueError("expected SHA-256 must be 64 hexadecimal digits")
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    if directory.is_symlink():
        raise ValueError("Geo output directory must be real")
    manifest = directory / "geo-pins.json"
    if manifest.is_symlink():
        raise ValueError("Geo pin manifest must be a regular file")
    pins = json.loads(manifest.read_text()) if manifest.exists() else {}
    if not isinstance(pins, dict):
        raise ValueError("Geo pin manifest must be a JSON object")
    with tempfile.NamedTemporaryFile(dir=directory, prefix=".geo-download-", delete=False) as temporary:
        stage = Path(temporary.name)
    try:
        size = 0
        digest = hashlib.sha256()
        with urlopen(url, timeout=30) as source, stage.open("wb") as output:
            if hasattr(source, "geturl") and not source.geturl().startswith("https://"):
                raise ValueError("Geo redirect must use HTTPS")
            while chunk := source.read(65536):
                size += len(chunk)
                if size > MAX_GEO_FILE:
                    raise ValueError("Geo download exceeds 128 MiB")
                digest.update(chunk)
                output.write(chunk)
        if not size or digest.hexdigest() != expected_hash.lower():
            raise ValueError("Geo download size or SHA-256 mismatch")
        stage.replace(directory / name)
        pins[name] = {"bytes": size, "sha256": expected_hash.lower()}
        with tempfile.NamedTemporaryFile(mode="w", dir=directory, prefix=".geo-pins-", delete=False) as output:
            pin_stage = Path(output.name)
        try:
            pin_stage.write_text(json.dumps(pins, indent=2, sort_keys=True) + "\n")
            pin_stage.replace(manifest)
        finally:
            pin_stage.unlink(missing_ok=True)
    finally:
        stage.unlink(missing_ok=True)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True, choices=sorted(GEO_NAMES))
    parser.add_argument("--url", required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    print(prepare(args.name, args.url, args.sha256, args.output_dir))


if __name__ == "__main__":
    main()
