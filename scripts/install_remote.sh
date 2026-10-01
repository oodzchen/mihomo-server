#!/usr/bin/env bash
# Remote one-shot installer for mihomo-server releases.
#
# Downloads a release tarball from GitHub Releases, verifies its SHA-256,
# extracts it and installs it as a systemd *user* service via the pinned
# scripts/install_service.py from the same release tag.
#
# Usage:
#   install.sh [OPTIONS] [-- INSTALLER_ARGS...]
#
# Options:
#   --version TAG   Release tag to install (default: latest published release)
#   --repo SLUG     GitHub repository slug (default: baked-in release slug)
#   --base-url URL  Override the download base (default: https://github.com).
#                   Releases are fetched from <base>/<repo>/releases/download/,
#                   the installer from <base>/raw (or raw.githubusercontent.com
#                   for the default GitHub base).
#   -h, --help      Show this help
#
# INSTALLER_ARGS are passed through to install_service.py install
# (e.g. --listen 127.0.0.1:9090 --data-dir ~/.local/share/mihomo-server).
# When omitted, the installer is invoked with --enable --start.
#
# One-shot usage:
#   curl -fsSL https://github.com/OWNER/REPO/releases/latest/download/install.sh \
#     | bash -s -- --enable --start
#
# __REPO_SLUG__ is replaced by CI with the publishing repository slug.
set -euo pipefail

REPO="__REPO_SLUG__"
TAG=""
BASE_URL="https://github.com"

usage() {
    sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

fetch() {
    # fetch URL DEST — curl or wget, whichever exists.
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        echo "error: need curl or wget to download files" >&2
        exit 1
    fi
}

die() {
    echo "error: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version requires a tag"
            TAG="$2"
            shift 2
            ;;
        --repo)
            [ $# -ge 2 ] || die "--repo requires a slug"
            REPO="$2"
            shift 2
            ;;
        --base-url)
            [ $# -ge 2 ] || die "--base-url requires a URL"
            BASE_URL="$2"
            shift 2
            ;;
        -h | --help)
            usage 0
            ;;
        --)
            shift
            break
            ;;
        *)
            echo "error: unknown option: $1 (use -- to pass arguments to the installer)" >&2
            usage 1
            ;;
    esac
done

[ "$REPO" != "__REPO_SLUG__" ] || die "repository slug is not baked in; pass --repo OWNER/REPO"

# Installer targets $HOME and a systemd user session; refuse root.
if [ "$(id -u)" -eq 0 ]; then
    die "do not run as root; this installs a systemd user service into \$HOME"
fi

ARCH="$(uname -m)"
[ "$ARCH" = "x86_64" ] || die "unsupported architecture '$ARCH' (first release supports x86_64-linux-gnu only)"

for tool in tar gunzip python3 sha256sum systemctl; do
    command -v "$tool" >/dev/null 2>&1 || die "missing required tool: $tool"
done

# Resolve the release tag when not given.
if [ -z "$TAG" ]; then
    api_url="https://api.github.com/repos/$REPO/releases/latest"
    [ "$BASE_URL" = "https://github.com" ] || api_url="$BASE_URL/$REPO/releases/latest"
    release_json="$(fetch "$api_url" /dev/stdout)" || die "cannot query latest release for $REPO"
    TAG="$(printf '%s\n' "$release_json" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
    [ -n "$TAG" ] || die "no published release found for $REPO"
fi

if [ "$BASE_URL" = "https://github.com" ]; then
    RAW_URL="https://raw.githubusercontent.com"
else
    RAW_URL="$BASE_URL/raw"
fi

NAME="mihomo-server-$TAG-x86_64-linux-gnu"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
echo "==> downloading $NAME"
fetch "$BASE_URL/$REPO/releases/download/$TAG/$NAME.tar.gz" "$work/$NAME.tar.gz" \
    || die "download failed: $NAME.tar.gz"
fetch "$BASE_URL/$REPO/releases/download/$TAG/$NAME.tar.gz.sha256" "$work/$NAME.tar.gz.sha256" \
    || die "download failed: checksum file"

( cd "$work" && sha256sum -c "$NAME.tar.gz.sha256" ) || die "checksum mismatch; aborting"

echo "==> extracting bundle"
tar -xzf "$work/$NAME.tar.gz" -C "$work"
BUNDLE="$work/$NAME"
if [ ! -f "$BUNDLE/launch" ] || [ ! -f "$BUNDLE/mihomo-server.service" ]; then
    die "unexpected tarball layout (missing launch/unit)"
fi

echo "==> fetching installer from tag $TAG"
fetch "$RAW_URL/$REPO/$TAG/scripts/install_service.py" "$work/install_service.py" \
    || die "download failed: install_service.py"

# Read the tarball's own manifest so we can show what is being installed.
CORE="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["core"]["version"])' \
    "$BUNDLE/resources/manifest.json" 2>/dev/null || echo unknown)"
echo "==> installing (bundled core: $CORE)"

INSTALLER_ARGS=(--enable --start)
[ $# -gt 0 ] && INSTALLER_ARGS=("$@")
python3 "$work/install_service.py" install --bundle "$BUNDLE" "${INSTALLER_ARGS[@]}"

cat <<EOF

Install complete.
  bundle:   $HOME/.local/opt/mihomo-server
  unit:     systemctl --user status mihomo-server
  logs:     mihomo-server logs -f
EOF