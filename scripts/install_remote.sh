#!/usr/bin/env bash
# One-shot installer for mihomo-server releases (pure shell; no Python needed).
#
# Downloads a release tarball from GitHub Releases, verifies its SHA-256,
# extracts it and installs it as a systemd *user* service.
#
# Usage:
#   install.sh [OPTIONS]
#
# Options:
#   --version TAG       Release tag to install (default: latest published release)
#   --bundle DIR        Install from an already extracted local bundle (no download)
#   --listen ADDR       Management HTTP listener (e.g. 0.0.0.0:9090)
#   --extra-args "ARGS" Extra arguments for the launcher (e.g. "--public-origin URL")
#   --install-dir DIR   Install destination (default: ~/.local/opt/mihomo-server)
#   --data-dir DIR      Data directory, absolute (default: ~/.local/share/mihomo-server)
#   --no-start          Install the unit but do not enable/start it
#   --uninstall         Stop and remove the service and installed files
#   --purge-data        With --uninstall: also delete the data directory (CAUTION)
#   --repo SLUG         GitHub repository slug (default: baked-in release slug)
#   --base-url URL      Override the download base (default: https://github.com).
#                       Releases are fetched from <base>/<repo>/releases/download/.
#   -h, --help          Show this help
#
# Required tools: tar, sha256sum (or shasum), systemctl, and curl or wget.
#
# One-shot usage:
#   curl -fsSL https://github.com/OWNER/REPO/releases/latest/download/install.sh | bash
#
# __REPO_SLUG__ is replaced by CI with the publishing repository slug.
set -euo pipefail

REPO="__REPO_SLUG__"
TAG=""
BASE_URL="https://github.com"
BUNDLE_SRC=""
LISTEN=""
EXTRA_ARGS=""
INSTALL_DIR="$HOME/.local/opt/mihomo-server"
DATA_DIR="$HOME/.local/share/mihomo-server"
UNIT_DIR="$HOME/.config/systemd/user"
UNIT_NAME="mihomo-server.service"
START=1
UNINSTALL=0
PURGE_DATA=0

usage() {
    sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
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

fetch_progress() {
    # fetch_progress URL DEST — like fetch, but shows a live progress bar on stderr.
    if command -v curl >/dev/null 2>&1; then
        if [ -t 2 ]; then
            curl -fL --retry 3 --progress-bar -o "$2" "$1"
        else
            curl -fsSL --retry 3 -o "$2" "$1"
        fi
    elif command -v wget >/dev/null 2>&1; then
        if [ -t 2 ]; then
            wget -q --show-progress -O "$2" "$1"
        else
            wget -q -O "$2" "$1"
        fi
    else
        echo "error: need curl or wget to download files" >&2
        exit 1
    fi
}

die() {
    echo "error: $*" >&2
    exit 1
}

need_arg() { [ "$2" -ge 2 ] || die "$1 requires a value"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) need_arg "$1" $#; TAG="$2"; shift 2 ;;
        --bundle) need_arg "$1" $#; BUNDLE_SRC="$2"; shift 2 ;;
        --listen) need_arg "$1" $#; LISTEN="$2"; shift 2 ;;
        --extra-args) need_arg "$1" $#; EXTRA_ARGS="$2"; shift 2 ;;
        --install-dir) need_arg "$1" $#; INSTALL_DIR="$2"; shift 2 ;;
        --data-dir) need_arg "$1" $#; DATA_DIR="$2"; shift 2 ;;
        --unit-dir) need_arg "$1" $#; UNIT_DIR="$2"; shift 2 ;;
        --unit-name) need_arg "$1" $#; UNIT_NAME="$2"; shift 2 ;;
        --repo) need_arg "$1" $#; REPO="$2"; shift 2 ;;
        --base-url) need_arg "$1" $#; BASE_URL="$2"; shift 2 ;;
        --no-start) START=0; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        --purge-data) PURGE_DATA=1; shift ;;
        --enable | --start | --) shift ;; # accepted for compatibility; now the default
        -h | --help) usage 0 ;;
        *)
            echo "error: unknown option: $1" >&2
            usage 1
            ;;
    esac
done

# Installer targets $HOME and a systemd user session; refuse root.
if [ "$(id -u)" -eq 0 ]; then
    die "do not run as root; this installs a systemd user service into \$HOME"
fi

case "$INSTALL_DIR" in /*) ;; *) die "--install-dir must be absolute" ;; esac
case "$DATA_DIR" in /*) ;; *) die "--data-dir must be absolute" ;; esac

command -v systemctl >/dev/null 2>&1 || die "missing required tool: systemctl"

# SYSTEMCTL_USER_PREFIX can override the command (e.g. "systemctl --user -M user@.host").
if [ -n "${SYSTEMCTL_USER_PREFIX:-}" ]; then
    read -r -a SYSTEMCTL <<<"$SYSTEMCTL_USER_PREFIX"
else
    SYSTEMCTL=(systemctl --user)
fi

if [ "$UNINSTALL" -eq 1 ]; then
    "${SYSTEMCTL[@]}" disable --now "$UNIT_NAME" >/dev/null 2>&1 || true
    rm -f "$UNIT_DIR/$UNIT_NAME"
    "${SYSTEMCTL[@]}" daemon-reload >/dev/null 2>&1 || true
    rm -rf "$INSTALL_DIR"
    if [ "$PURGE_DATA" -eq 1 ]; then
        rm -rf "$DATA_DIR"
        echo "Uninstalled $UNIT_NAME (data removed)"
    else
        echo "Uninstalled $UNIT_NAME (data kept in $DATA_DIR)"
    fi
    exit 0
fi

if [ -n "$BUNDLE_SRC" ]; then
    BUNDLE="$(cd "$BUNDLE_SRC" && pwd)" || die "bundle directory not found: $BUNDLE_SRC"
else
    [ "$REPO" != "__REPO_SLUG__" ] || die "repository slug is not baked in; pass --repo OWNER/REPO"
    ARCH="$(uname -m)"
    [ "$ARCH" = "x86_64" ] || die "unsupported architecture '$ARCH' (first release supports x86_64-linux-gnu only)"

    command -v tar >/dev/null 2>&1 || die "missing required tool: tar"
    if command -v sha256sum >/dev/null 2>&1; then
        SHA_CHECK=(sha256sum -c)
    elif command -v shasum >/dev/null 2>&1; then
        SHA_CHECK=(shasum -a 256 -c)
    else
        die "missing required tool: sha256sum (or shasum)"
    fi

    # Resolve the release tag when not given.
    if [ -z "$TAG" ]; then
        api_url="https://api.github.com/repos/$REPO/releases/latest"
        [ "$BASE_URL" = "https://github.com" ] || api_url="$BASE_URL/$REPO/releases/latest"
        release_json="$(fetch "$api_url" /dev/stdout)" || die "cannot query latest release for $REPO"
        TAG="$(printf '%s\n' "$release_json" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
        [ -n "$TAG" ] || die "no published release found for $REPO"
    fi

    NAME="mihomo-server-$TAG-x86_64-unknown-linux-gnu"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    echo "==> downloading $NAME"
    fetch_progress "$BASE_URL/$REPO/releases/download/$TAG/$NAME.tar.gz" "$work/$NAME.tar.gz" \
        || die "download failed: $NAME.tar.gz"
    fetch "$BASE_URL/$REPO/releases/download/$TAG/$NAME.tar.gz.sha256" "$work/$NAME.tar.gz.sha256" \
        || die "download failed: checksum file"

    ( cd "$work" && "${SHA_CHECK[@]}" "$NAME.tar.gz.sha256" >/dev/null ) || die "checksum mismatch; aborting"

    echo "==> extracting bundle"
    tar -xzf "$work/$NAME.tar.gz" -C "$work"
    BUNDLE="$work/$NAME"
fi

for f in launch bin/mihomo-server mihomo-server.service resources/manifest.json; do
    [ -f "$BUNDLE/$f" ] || die "unexpected bundle layout (missing $f)"
done

CORE="$(sed -n '/"core"/,/}/s/.*"version":[[:space:]]*"\([^"]*\)".*/\1/p' \
    "$BUNDLE/resources/manifest.json" | head -n 1)"
echo "==> installing (bundled core: ${CORE:-unknown})"

mkdir -p "$INSTALL_DIR" "$DATA_DIR" "$UNIT_DIR"
chmod 700 "$DATA_DIR"
# Copy the bundle (skip when installing in place).
if [ "$(cd "$BUNDLE" && pwd -P)" != "$(cd "$INSTALL_DIR" && pwd -P)" ]; then
    # Stop a running instance first so replaced binaries are not busy.
    "${SYSTEMCTL[@]}" stop "$UNIT_NAME" >/dev/null 2>&1 || true
    rm -rf "${INSTALL_DIR:?}/bin" "${INSTALL_DIR:?}/resources" "${INSTALL_DIR:?}/docs"
    cp -a "$BUNDLE"/. "$INSTALL_DIR"/
fi
chmod 755 "$INSTALL_DIR" "$INSTALL_DIR/launch" "$INSTALL_DIR/bin/mihomo-server"
chmod 755 "$INSTALL_DIR"/resources/core/* 2>/dev/null || true

# Render the unit from the bundled template; %h keeps paths home-relative.
home_rel() {
    case "$1" in
        "$HOME"/*) printf '%%h%s' "${1#"$HOME"}" ;;
        *) printf '%s' "$1" ;;
    esac
}
exec_start="$(home_rel "$INSTALL_DIR")/launch"
[ -z "$LISTEN" ] || exec_start="$exec_start --listen $LISTEN"
[ -z "$EXTRA_ARGS" ] || exec_start="$exec_start $EXTRA_ARGS"
env_line="Environment=MIHOMO_SERVER_DATA_DIR=$(home_rel "$DATA_DIR")"
awk -v exec_line="ExecStart=$exec_start" -v env_line="$env_line" '
    /^ExecStart=/ { print exec_line; next }
    /^Environment=MIHOMO_SERVER_DATA_DIR=/ { print env_line; next }
    { print }
' "$BUNDLE/mihomo-server.service" > "$UNIT_DIR/$UNIT_NAME"
chmod 644 "$UNIT_DIR/$UNIT_NAME"

"${SYSTEMCTL[@]}" daemon-reload
if [ "$START" -eq 1 ]; then
    "${SYSTEMCTL[@]}" enable "$UNIT_NAME" >/dev/null 2>&1
    "${SYSTEMCTL[@]}" restart "$UNIT_NAME"
fi

svc="${UNIT_NAME%.service}"
cat <<EOF

Install complete.
  bundle:    $INSTALL_DIR
  data:      $DATA_DIR
  status:    systemctl --user status $svc
  logs:      journalctl --user -u $svc -f
  token:     cat $DATA_DIR/management-token
  uninstall: install.sh --uninstall   (add --purge-data to delete data)
EOF
if command -v loginctl >/dev/null 2>&1 \
    && [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)" = "no" ]; then
    echo "  note:      to keep running after logout: loginctl enable-linger \"\$USER\""
fi
