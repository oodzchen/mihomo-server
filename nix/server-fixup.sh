# shellcheck shell=bash
# shellcheck disable=SC2154
# autoPatchelfHook/fixup may change ELF bytes. Pin the installed core, not its
# original download hash, because Resources verifies the bytes before copying.
python3 - "$out" "$version" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
manifest = root / "resources/manifest.json"
data = json.loads(manifest.read_text())
with (root / "resources/core/verge-mihomo").open("rb") as stream:
    data["core"]["sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
manifest.write_text(json.dumps(data, indent=2) + "\n")
(root / "nix-installation.json").write_text(json.dumps({"kind": "nix", "version": sys.argv[2]}) + "\n")
PY
