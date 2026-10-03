#!/usr/bin/env bash
# One-shot installer for mihomo-server releases (pure shell; no Python needed).
#
# Downloads a release tarball from GitHub Releases, verifies its SHA-256,
# extracts it and installs it as a systemd *user* service.
#
# Two modes:
#   per-user (default, run as a normal user): bundle in ~/.local/opt, one service.
#   --system (run as root): one shared bundle in /opt/mihomo-server for all local
#     users. Each user runs `mihomo-server-user init` to enable their own instance
#     with private data, ports and TUN routing.
#
# Usage:
#   install.sh [OPTIONS]
#
# Options:
#   --version TAG       Release tag to install (default: latest published release)
#   --bundle DIR        Install from an already extracted local bundle (no download)
#   --listen ADDR       Management HTTP listener (e.g. 0.0.0.0:9090; per-user mode)
#   --extra-args "ARGS" Extra arguments for the launcher (e.g. "--public-origin URL")
#   --install-dir DIR   Install destination (default: ~/.local/opt/mihomo-server,
#                       or /opt/mihomo-server with --system)
#   --data-dir DIR      Data directory, absolute (default: ~/.local/share/mihomo-server)
#   --no-start          Install the unit but do not enable/start it
#   --uninstall         Stop and remove the service and installed files
#   --purge-data        With --uninstall: also delete the data directory (CAUTION);
#                       with --system: the slot registry (users' homes are kept)
#   --system            Shared installation for all local users (requires root)
#   --tun-user USER     With --system: allow USER to use TUN (adds USER to group
#                       mihomo-tun); may be repeated
#   --repo SLUG         GitHub repository slug (default: baked-in release slug)
#   --base-url URL      Override the download base (default: https://github.com).
#                       Releases are fetched from <base>/<repo>/releases/download/.
#   -h, --help          Show this help
#
# Required tools: tar, sha256sum (or shasum), systemctl, and curl or wget;
# --system also uses groupadd, usermod and setcap (libcap) for TUN.
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
SYSTEM=0
TUN_USERS=()
INSTALL_DIR_SET=0
UNIT_DIR_SET=0
TUN_GROUP="mihomo-tun"
SLOT_DIR="/var/lib/mihomo-server/slots"
BIN_DIR="/usr/local/bin"

usage() {
    sed -n '2,42p' "$0" | sed 's/^# \{0,1\}//'
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

# Run a systemctl command in every running user manager (logged-in or lingering users).
each_user_manager() {
    command -v loginctl >/dev/null 2>&1 || return 0
    local user
    while read -r _ user _; do
        [ -n "$user" ] || continue
        systemctl --user -M "$user@" "$@" >/dev/null 2>&1 || true
    done < <(loginctl list-users --no-legend 2>/dev/null)
}

system_install() {
    [ -f "$BUNDLE/mihomo-server-user" ] || die "bundle lacks mihomo-server-user; it predates --system support"
    local releases="$INSTALL_DIR/releases" name release stage previous=""
    name="${TAG:-$(basename "$BUNDLE")}"
    case "$name" in "" | . | .. | */* | .*) die "cannot derive a release name from $BUNDLE" ;; esac
    release="$releases/$name"
    [ ! -e "$release" ] || release="$release-$(date +%Y%m%d%H%M%S)"
    if [ -L "$INSTALL_DIR/current" ]; then
        previous="$(basename "$(readlink "$INSTALL_DIR/current")")"
    fi

    mkdir -p "$releases"
    chmod 755 "$INSTALL_DIR" "$releases"
    stage="$(mktemp -d "$releases/.staging-XXXXXX")"
    cp -a "$BUNDLE"/. "$stage"/
    chown -R root:root "$stage"
    chmod -R u+rwX,go+rX,go-w "$stage"
    chmod 755 "$stage"

    # TUN: a capability-carrying copy of the core that only the group can run.
    getent group "$TUN_GROUP" >/dev/null 2>&1 || groupadd --system "$TUN_GROUP"
    local tun_core="$stage/resources/core/verge-mihomo-tun" tun_note=""
    if command -v setcap >/dev/null 2>&1; then
        cp "$stage/resources/core/verge-mihomo" "$tun_core"
        chown "root:$TUN_GROUP" "$tun_core"
        chmod 0750 "$tun_core"
        setcap 'cap_net_admin,cap_net_bind_service,cap_net_raw+ep' "$tun_core"
    else
        tun_note="setcap not found (install libcap); TUN is unavailable to all users"
    fi
    mv -T "$stage" "$release"

    local user
    for user in "${TUN_USERS[@]}"; do
        id "$user" >/dev/null 2>&1 || die "--tun-user: no such user '$user'"
        usermod -aG "$TUN_GROUP" "$user"
    done

    # Slot registry: like /tmp, anyone may claim a slot, only its owner may release it.
    mkdir -p "$SLOT_DIR"
    chown root:root "$SLOT_DIR"
    chmod 1777 "$SLOT_DIR"

    # Atomic switch: running instances keep their canonical (previous) release.
    ln -sfn "releases/$(basename "$release")" "$INSTALL_DIR/.current.new"
    mv -T "$INSTALL_DIR/.current.new" "$INSTALL_DIR/current"

    mkdir -p "$UNIT_DIR" "$BIN_DIR"
    local exec_line="ExecStart=$INSTALL_DIR/current/launch --multi-user --slot-registry $SLOT_DIR"
    [ -z "$EXTRA_ARGS" ] || exec_line="$exec_line $EXTRA_ARGS"
    # Each user's ~/.config/mihomo-server/env may set MIHOMO_SERVER_ARGS (e.g. --listen).
    awk -v exec_line="$exec_line \$MIHOMO_SERVER_ARGS" '
        /^ExecStart=/ { print exec_line; next }
        /^Environment=MIHOMO_SERVER_DATA_DIR=/ {
            print "Environment=MIHOMO_SERVER_DATA_DIR=%h/.local/share/mihomo-server"
            print "EnvironmentFile=-%h/.config/mihomo-server/env"
            next
        }
        { print }
    ' "$release/mihomo-server.service" > "$UNIT_DIR/$UNIT_NAME"
    chmod 644 "$UNIT_DIR/$UNIT_NAME"
    ln -sfn "$INSTALL_DIR/current/mihomo-server-user" "$BIN_DIR/mihomo-server-user"
    # cp -a keeps the source's SELinux labels (often default_t); apply the policy's.
    # File capabilities live in another xattr and are not affected.
    if command -v restorecon >/dev/null 2>&1; then
        restorecon -R "$INSTALL_DIR" "$SLOT_DIR" "$UNIT_DIR/$UNIT_NAME" "$BIN_DIR/mihomo-server-user" 2>/dev/null || true
    fi

    # Running instances move to the new bundle; stopped ones stay stopped.
    each_user_manager daemon-reload
    [ -z "$previous" ] || each_user_manager try-restart "$UNIT_NAME"

    # Keep the current and the previous release for rollback.
    local entry
    for entry in "$releases"/*; do
        [ -d "$entry" ] || continue
        case "$(basename "$entry")" in
            "$(basename "$release")" | "${previous:-/}") ;;
            *) rm -rf "${entry:?}" ;;
        esac
    done

    cat <<EOF

System install complete.
  bundle:    $release
  current:   $INSTALL_DIR/current
  unit:      $UNIT_DIR/$UNIT_NAME (one instance per user)
  slots:     $SLOT_DIR
  TUN group: $TUN_GROUP${TUN_USERS[*]:+ (added: ${TUN_USERS[*]})}
Each user enables their own instance with:
  mihomo-server-user init
EOF
    [ -z "$tun_note" ] || echo "  warning:   $tun_note"
    if [ "${#TUN_USERS[@]}" -gt 0 ]; then
        echo "  note:      group changes apply to new logins; a lingering user's"
        echo "             manager needs: systemctl restart user@<uid>.service"
    fi
}

system_uninstall() {
    each_user_manager stop "$UNIT_NAME"
    rm -f "${UNIT_DIR:?}/${UNIT_NAME:?}"
    if [ -L "${BIN_DIR:?}/mihomo-server-user" ]; then
        rm -f "${BIN_DIR:?}/mihomo-server-user"
    fi
    each_user_manager daemon-reload
    rm -rf "${INSTALL_DIR:?}"
    if [ "$PURGE_DATA" -eq 1 ]; then
        rm -rf "${SLOT_DIR:?}"
        echo "Uninstalled the system installation (slot registry removed)"
    else
        echo "Uninstalled the system installation (slot registry kept in $SLOT_DIR)"
    fi
    echo "Users' data in ~/.local/share/mihomo-server is untouched; group $TUN_GROUP is kept."
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) need_arg "$1" $#; TAG="$2"; shift 2 ;;
        --bundle) need_arg "$1" $#; BUNDLE_SRC="$2"; shift 2 ;;
        --listen) need_arg "$1" $#; LISTEN="$2"; shift 2 ;;
        --extra-args) need_arg "$1" $#; EXTRA_ARGS="$2"; shift 2 ;;
        --install-dir) need_arg "$1" $#; INSTALL_DIR="$2"; INSTALL_DIR_SET=1; shift 2 ;;
        --data-dir) need_arg "$1" $#; DATA_DIR="$2"; shift 2 ;;
        --unit-dir) need_arg "$1" $#; UNIT_DIR="$2"; UNIT_DIR_SET=1; shift 2 ;;
        --unit-name) need_arg "$1" $#; UNIT_NAME="$2"; shift 2 ;;
        --repo) need_arg "$1" $#; REPO="$2"; shift 2 ;;
        --base-url) need_arg "$1" $#; BASE_URL="$2"; shift 2 ;;
        --no-start) START=0; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        --purge-data) PURGE_DATA=1; shift ;;
        --system) SYSTEM=1; shift ;;
        --tun-user) need_arg "$1" $#; TUN_USERS+=("$2"); shift 2 ;;
        # Test overrides for a scratch root.
        --slot-dir) need_arg "$1" $#; SLOT_DIR="$2"; shift 2 ;;
        --bin-dir) need_arg "$1" $#; BIN_DIR="$2"; shift 2 ;;
        --enable | --start | --) shift ;; # accepted for compatibility; now the default
        -h | --help) usage 0 ;;
        *)
            echo "error: unknown option: $1" >&2
            usage 1
            ;;
    esac
done

if [ "$SYSTEM" -eq 1 ]; then
    [ "$(id -u)" -eq 0 ] || die "--system installs for all users and must run as root"
    [ -z "$LISTEN" ] || die "--listen is per user with --system; users pass it to 'mihomo-server-user init'"
    [ "$INSTALL_DIR_SET" -eq 1 ] || INSTALL_DIR="/opt/mihomo-server"
    [ "$UNIT_DIR_SET" -eq 1 ] || UNIT_DIR="/etc/systemd/user"
elif [ "$(id -u)" -eq 0 ]; then
    # Per-user mode targets $HOME and a systemd user session; refuse root.
    die "do not run as root; this installs a systemd user service into \$HOME (use --system for all users)"
elif [ "${#TUN_USERS[@]}" -gt 0 ]; then
    die "--tun-user requires --system"
fi

case "$INSTALL_DIR" in /?*) ;; *) die "--install-dir must be absolute" ;; esac
case "$DATA_DIR" in /*) ;; *) die "--data-dir must be absolute" ;; esac
case "$SLOT_DIR" in /?*) ;; *) die "--slot-dir must be absolute" ;; esac

command -v systemctl >/dev/null 2>&1 || die "missing required tool: systemctl"

# SYSTEMCTL_USER_PREFIX can override the command (e.g. "systemctl --user -M user@.host").
if [ -n "${SYSTEMCTL_USER_PREFIX:-}" ]; then
    read -r -a SYSTEMCTL <<<"$SYSTEMCTL_USER_PREFIX"
else
    SYSTEMCTL=(systemctl --user)
fi

if [ "$SYSTEM" -eq 1 ] && [ "$UNINSTALL" -eq 1 ]; then
    system_uninstall
    exit 0
fi

if [ "$UNINSTALL" -eq 1 ]; then
    "${SYSTEMCTL[@]}" disable --now "$UNIT_NAME" >/dev/null 2>&1 || true
    rm -f "${UNIT_DIR:?}/${UNIT_NAME:?}"
    "${SYSTEMCTL[@]}" daemon-reload >/dev/null 2>&1 || true
    rm -rf "${INSTALL_DIR:?}"
    if [ "$PURGE_DATA" -eq 1 ]; then
        rm -rf "${DATA_DIR:?}"
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

if [ "$SYSTEM" -eq 1 ]; then
    system_install
    exit 0
fi

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
