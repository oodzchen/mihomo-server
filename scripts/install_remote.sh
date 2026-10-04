#!/usr/bin/env bash
# Zero-argument shared Linux installer. No Python is required.
set -euo pipefail

usage() {
    cat <<'HELP'
Usage: install.sh [--help | --uninstall [--purge]]

With no arguments, install or upgrade the latest release system-wide.
The installing user is automatically started, authorized for TUN and kept
running after logout/reboot. sudo may ask for your password.

--uninstall          Stop every instance and remove the shared installation,
                     TUN group, slot registry and installer-enabled lingering.
                     Users' subscriptions and settings are kept.
--uninstall --purge  Also delete every user's instance data and configuration.
--help               Show this help.

Other users start their own instance with: mihomo-server enable
HELP
}
die() { echo "error: $*" >&2; exit 1; }
fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        die 'need curl or wget'
    fi
}
# Latest tag from the releases/latest redirect; the REST API is limited to 60
# anonymous requests per hour per IP, which shared proxy exits exhaust.
latest_tag() {
    local location
    if command -v curl >/dev/null 2>&1; then
        location=$(curl -fsS --retry 3 -o /dev/null -w '%{redirect_url}' "$1") || return 1
    elif command -v wget >/dev/null 2>&1; then
        # wget and wget2 report the unfollowed redirect differently and exit non-zero.
        location=$(wget -S --max-redirect=0 -O /dev/null "$1" 2>&1 || true)
    else
        die 'need curl or wget'
    fi
    sed -n "s#.*/releases/tag/\([^/[:space:]'\"]*\).*#\1#p" <<< "$location" | head -n 1
}
fetch_progress() {
    if command -v curl >/dev/null 2>&1 && [ -t 2 ]; then
        curl -fL --retry 3 --progress-bar -o "$2" "$1"
    else
        fetch "$@"
    fi
}
verify_bundle() {
    local bundle=$1 file
    for file in launch bin/mihomo-server mihomo-server.service mihomo-server-user resources/manifest.json checksums.sha256; do
        [ -f "$bundle/$file" ] || die "unexpected bundle layout: missing $file"
    done
    # Release bundles contain only regular files/directories, never links/devices.
    [ -z "$(find "$bundle" ! -type f ! -type d -print -quit)" ] || die 'unsafe bundle file type'
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$bundle" && sha256sum -c checksums.sha256 >/dev/null) || die 'bundle checksum mismatch'
    else
        (cd "$bundle" && shasum -a 256 -c checksums.sha256 >/dev/null) || die 'bundle checksum mismatch'
    fi
}
each_user_manager() {
    local uid user failed=0
    while read -r uid user _; do
        if [ -z "$user" ] || [ ! -S "/run/user/$uid/bus" ]; then continue; fi
        if [ "${1:-}" = disable ] && [ "$(systemctl --user -M "$user@" show mihomo-server -p LoadState --value)" = not-found ]; then continue; fi
        if ! systemctl --user -M "$user@" "$@"; then
            echo "error: $* failed for $user" >&2
            failed=1
        fi
    done < <(loginctl list-users --no-legend)
    return "$failed"
}

# Runs as the instance owner, never root. Keep old files for rollback.
activate_user() {
    local home=$1 config_home=$2 helper=$3 unit exec_line data_line args old_root dropins
    local backup='' was_active='' was_enabled='' env_file="$config_home/mihomo-server/env"
    local env_backup='' old_dropin='' dropin_backup=''
    unit=$(systemctl --user show mihomo-server -p FragmentPath --value)
    if [ -n "$unit" ] && [[ "$unit" != /etc/systemd/user/* ]]; then
        if [ ! -f "$unit" ] || [ -L "$unit" ]; then die 'legacy unit is not a regular file'; fi
        # Recognize only the installer template. Preserve custom units for review.
        if awk '
            /^[[:space:]]*($|#|;)/ {next}
            /^\[(Unit|Service|Install)\]$/ {next}
            /^Description=/ {next}
            /^(After=network.target|Type=simple|Restart=on-failure|RestartSec=3|KillSignal=SIGTERM|KillMode=mixed|TimeoutStopSec=30|UMask=0077|WantedBy=default.target)$/ {next}
            /^Environment=MIHOMO_SERVER_DATA_DIR=/ {next}
            /^ExecStart=/ {next}
            {exit 1}' "$unit"; then :; else die "custom unit requires manual migration: $unit"; fi
        dropins=$(systemctl --user show mihomo-server -p DropInPaths --value)
        # Distro-wide vendor drop-ins also apply to the new shared unit. Only
        # user/admin-specific overrides need review before replacing a unit.
        if [ -n "$dropins" ] && ! [[ "$dropins" =~ ^(/(usr/)?lib/systemd/user/[^[:space:]]+[[:space:]]*)+$ ]]; then
            die 'custom legacy drop-ins require manual migration'
        fi
        [ "$(grep -c '^ExecStart=' "$unit")" = 1 ] || die 'ambiguous legacy ExecStart'
        [ "$(grep -c '^Environment=MIHOMO_SERVER_DATA_DIR=' "$unit")" = 1 ] || die 'ambiguous legacy data directory'
        exec_line=$(sed -n 's/^ExecStart=//p' "$unit")
        exec_line=${exec_line//%h/$home}
        old_root=${exec_line%%/launch*}
        [[ "$exec_line" = "$old_root/launch"* && "$old_root" = "$home/"* && "$exec_line" != *%* ]] \
            || die "unrecognized legacy launcher: $unit"
        if [ ! -f "$old_root/resources/manifest.json" ] || [ ! -f "$old_root/bin/mihomo-server" ]; then
            die "unrecognized legacy bundle: $old_root"
        fi
        data_line=$(sed -n 's/^Environment=MIHOMO_SERVER_DATA_DIR=//p' "$unit")
        data_line=${data_line//%h/$home}
        [[ "$data_line" = /?* && "$data_line" != *%* && "$data_line" != *\"* ]] || die 'unsupported legacy data path'
        args=${exec_line#"$old_root/launch"}; args=${args# }
        if [ -f "$env_file" ]; then
            if grep -q '^MIHOMO_SERVER_DATA_DIR=' "$env_file" || { [ -n "$args" ] && grep -q '^MIHOMO_SERVER_ARGS=' "$env_file"; }; then
                die "legacy settings conflict with $env_file; resolve them before migrating"
            fi
            env_backup="$env_file.pre-system-$(date +%s)-$$"
            cp -p "$env_file" "$env_backup"
        fi
        mkdir -p "$(dirname "$env_file")"
        # EnvironmentFile quoting, not shell evaluation. Preserve argument quotes.
        data_line=${data_line//\\/\\\\}; data_line=${data_line//\"/\\\"}
        args=${args//\\/\\\\}; args=${args//\"/\\\"}
        printf '\nMIHOMO_SERVER_DATA_DIR="%s"\n' "$data_line" >> "$env_file"
        [ -z "$args" ] || printf 'MIHOMO_SERVER_ARGS="%s"\n' "$args" >> "$env_file"
        was_active=$(systemctl --user is-active mihomo-server || true)
        was_enabled=$(systemctl --user is-enabled mihomo-server || true)
        backup="$unit.pre-system-$(date +%s)-$$"
        systemctl --user disable --now mihomo-server
        mv "$unit" "$backup"
        systemctl --user daemon-reload
        echo "Legacy unit retained at $backup; old bundle retained at $old_root"
    fi
    # Save/restore only our managed drop-in if activation fails during migration.
    if [ -n "$backup" ]; then
        old_dropin="$(dirname "$unit")/mihomo-server.service.d/10-mihomo-paths.conf"
        if [ -f "$old_dropin" ]; then
            dropin_backup="$old_dropin.pre-system-$$"
            cp -p "$old_dropin" "$dropin_backup"
        fi
    fi
    if "$helper" enable; then
        return 0
    fi
    if [ -n "$backup" ]; then
        systemctl --user disable --now mihomo-server || true
        mv "$backup" "$unit"
        if [ -n "$dropin_backup" ]; then cp -p "$dropin_backup" "$old_dropin"; else rm -f "$old_dropin"; fi
        if [ -n "$env_backup" ]; then cp -p "$env_backup" "$env_file"; else rm -f "$env_file"; fi
        systemctl --user daemon-reload
        [ "$was_enabled" != enabled ] || systemctl --user enable mihomo-server
        [ "$was_active" != active ] || systemctl --user start mihomo-server
        echo 'Restored the previous user unit after failed migration.' >&2
    fi
    die 'user instance activation failed; shared files remain installed'
}

# Mihomo's TUN runs resolvectl to make systemd-resolved send every lookup to its
# link, which needs polkit for a non-root core. A TUN here is system-wide (it
# carries every account, so their fake IPs work) and the service lets one
# TUN-group member hold it at a time (tun.lock), so polkit silently allows each
# member's own ms<uid> link and refuses other members' links. Root is unaffected.
polkit_tun_dns() {
    local rule=/etc/polkit-1/rules.d/50-mihomo-server-tun.rules
    local pkla=/etc/polkit-1/localauthority/50-local.d/50-mihomo-server-tun.pkla
    local actions='org.freedesktop.resolve1.set-dns-servers org.freedesktop.resolve1.set-domains org.freedesktop.resolve1.set-default-route org.freedesktop.resolve1.revert'
    if [ "$1" = remove ]; then
        rm -f "$rule" "$pkla"
        return
    fi
    if [ -d "${rule%/*}" ]; then
        cat > "$rule.new" <<RULE
// Managed by mihomo-server; see polkit_tun_dns in its installer.
polkit.addRule(function (action, subject) {
    if ("$actions".split(" ").indexOf(action.id) < 0 || !subject.isInGroup("mihomo-tun")) {
        return polkit.Result.NOT_HANDLED;
    }
    // systemd 256+ names the link; older versions name none.
    var link = action.lookup("interface");
    if (!link) {
        return polkit.Result.YES;
    }
    if (!/^ms[0-9]+\$/.test(link)) {
        return polkit.Result.NOT_HANDLED;
    }
    try {
        var uid = polkit.spawn(["/usr/bin/id", "-u", subject.user]).trim();
    } catch (error) {
        return polkit.Result.NO;
    }
    return link === "ms" + uid ? polkit.Result.YES : polkit.Result.NO;
});
RULE
        chmod 644 "$rule.new"
        mv -f "$rule.new" "$rule"
        rm -f "$pkla"
    # polkit 0.105 (e.g. Ubuntu 22.04) reads only local authority files.
    elif [ -d "${pkla%/*}" ]; then
        printf '[mihomo-server TUN DNS]\nIdentity=unix-group:mihomo-tun\nAction=%s\nResultAny=yes\nResultInactive=yes\nResultActive=yes\n' \
            "${actions// /;}" > "$pkla.new"
        chmod 644 "$pkla.new"
        mv -f "$pkla.new" "$pkla"
    fi
}

# Web upgrades: a root oneshot unit that takes no input and installs the latest
# release of the built-in repository; polkit lets TUN-group members start only
# that unit, without a password. Its output is the world-readable update.log.
update_unit() {
    local unit=/etc/systemd/system/mihomo-server-update.service
    local rule=/etc/polkit-1/rules.d/50-mihomo-server-update.rules
    if [ "$1" = remove ]; then
        rm -f "$unit" "$rule"
        systemctl daemon-reload || true
        return
    fi
    mkdir -p "${unit%/*}"
    cat > "$unit.new" <<'UNIT'
# Managed by mihomo-server; see update_unit in its installer.
[Unit]
Description=Upgrade mihomo-server to the latest release
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart=/opt/mihomo-server/current/bin/mihomo-server update
StandardOutput=truncate:/var/lib/mihomo-server/update.log
StandardError=inherit
PrivateTmp=yes
TimeoutStartSec=20min
UNIT
    chmod 644 "$unit.new"
    mv -f "$unit.new" "$unit"
    systemctl daemon-reload
    # polkit 0.105 local authority files cannot name a unit: no Web upgrades there.
    if [ -d "${rule%/*}" ]; then
        cat > "$rule.new" <<'RULE'
// Managed by mihomo-server; see update_unit in its installer.
polkit.addRule(function (action, subject) {
    if (action.id === "org.freedesktop.systemd1.manage-units"
        && action.lookup("unit") === "mihomo-server-update.service"
        && action.lookup("verb") === "start"
        && subject.isInGroup("mihomo-tun")) {
        return polkit.Result.YES;
    }
    return polkit.Result.NOT_HANDLED;
});
RULE
        chmod 644 "$rule.new"
        mv -f "$rule.new" "$rule"
    fi
}

install_shared() {
    local bundle=$1 tag=$2 caller=$3 config_home=$4 data_home=$5 tool
    local root=/opt/mihomo-server releases=/opt/mihomo-server/releases
    local registry=/var/lib/mihomo-server/slots previous='' name release stage uid home output
    for tool in systemctl loginctl getent groupadd usermod runuser setcap getcap sg; do
        command -v "$tool" >/dev/null || die "missing required tool: $tool"
    done
    verify_bundle "$bundle"
    if [ -n "$caller" ]; then
        uid=$(id -u "$caller")
        [ "$uid" != 0 ] || die 'the instance owner must not be root'
        home=$(getent passwd "$caller" | cut -d: -f6)
    fi
    name=${tag:-$(basename "$bundle")}
    case "$name" in '' | . | .. | */* | .*) die 'invalid release name' ;; esac
    if [ -L "$root/current" ]; then previous=$(basename "$(readlink "$root/current")"); fi
    mkdir -p "$releases"
    chmod 755 "$root" "$releases"
    release="$releases/$name"
    [ ! -e "$release" ] || release="$release-$(date +%s)-$$"
    stage=$(mktemp -d "$releases/.staging-XXXXXX")
    cp -a "$bundle"/. "$stage"/
    chown -R root:root "$stage"
    chmod -R u+rwX,go+rX,go-w "$stage"
    chmod 755 "$stage"
    getent group mihomo-tun >/dev/null || groupadd --system mihomo-tun
    # TUN launcher: grants network capabilities to each member's own (Web-upgradable) core.
    cp "$stage/bin/mihomo-server" "$stage/bin/mihomo-tun-exec"
    chown root:mihomo-tun "$stage/bin/mihomo-tun-exec"
    chmod 0750 "$stage/bin/mihomo-tun-exec"
    setcap 'cap_net_admin,cap_net_bind_service,cap_net_raw+ep' "$stage/bin/mihomo-tun-exec"
    mv -T "$stage" "$release"
    mkdir -p "$registry"
    chown root:root "$registry"
    chmod 1777 "$registry"
    # One system-wide TUN at a time: its holder's service keeps an flock on this
    # file, which only TUN-group members can open.
    [ -f /var/lib/mihomo-server/tun.lock ] || : > /var/lib/mihomo-server/tun.lock
    chown root:mihomo-tun /var/lib/mihomo-server/tun.lock
    chmod 0640 /var/lib/mihomo-server/tun.lock
    polkit_tun_dns install
    update_unit install
    ln -sfn "releases/$(basename "$release")" "$root/.current.new"
    mv -T "$root/.current.new" "$root/current"
    mkdir -p /etc/systemd/user /usr/local/bin
    install -m 644 "$release/mihomo-server.service" /etc/systemd/user/mihomo-server.service
    ln -sfn "$root/current/mihomo-server-user" /usr/local/bin/mihomo-server-user
    # The mihomo-server command, its manual and completions, following the
    # release through the current link. Never replace a file we did not create.
    local file link
    for file in bin/mihomo-server share/man/man1/mihomo-server.1 \
        share/bash-completion/completions/mihomo-server share/zsh/site-functions/_mihomo-server; do
        [ -f "$release/share/man/man1/mihomo-server.1" ] || break
        link=/usr/local/$file
        if [ -e "$link" ] && [ ! -L "$link" ]; then
            echo "warning: keeping existing $link; mihomo-server is at $root/current/$file" >&2
            continue
        fi
        mkdir -p "$(dirname "$link")"
        ln -sfn "$root/current/$file" "$link"
    done
    if command -v restorecon >/dev/null; then
        restorecon -R "$root" "$registry" /etc/systemd/user/mihomo-server.service /usr/local/bin/mihomo-server-user \
            /etc/systemd/system/mihomo-server-update.service \
            /usr/local/bin/mihomo-server /usr/local/share/man/man1/mihomo-server.1 \
            /usr/local/share/bash-completion/completions/mihomo-server /usr/local/share/zsh/site-functions/_mihomo-server \
            /etc/polkit-1/rules.d /etc/polkit-1/localauthority 2>/dev/null || true
    fi
    each_user_manager daemon-reload
    if [ -n "$caller" ]; then
        usermod -aG mihomo-tun "$caller"
        if [ "$(loginctl show-user "$caller" -p Linger --value 2>/dev/null || true)" != yes ]; then
            # Remember lingering we enabled, so uninstall can undo only that.
            mkdir -p /var/lib/mihomo-server/linger
            : > "/var/lib/mihomo-server/linger/$caller"
        fi
        loginctl enable-linger "$caller"
        systemctl start "user@$uid.service"
        [ "$(loginctl show-user "$caller" -p Linger --value)" = yes ] || die 'linger was not enabled'
        # Preserve the caller's XDG values through sudo, and activate as that user.
        output=$(runuser -u "$caller" -- env HOME="$home" XDG_RUNTIME_DIR="/run/user/$uid" \
            XDG_CONFIG_HOME="$config_home" XDG_DATA_HOME="$data_home" \
            bash -euo pipefail -c "$(declare -f die activate_user); activate_user \"\$@\"" \
            bash "$home" "$config_home" /usr/local/bin/mihomo-server-user) || die 'installation did not produce a usable user instance'
        printf '%s\n' "$output"
        grep -q '^tun: *available ' <<< "$output" || die 'TUN authorization did not take effect'
    fi
    # Upgrade all other running instances. Caller has already been verified.
    if [ -n "$previous" ]; then
        local user manager_uid
        while read -r manager_uid user _; do
            if [ -z "$user" ] || [ "$user" = "$caller" ] || [ ! -S "/run/user/$manager_uid/bus" ]; then continue; fi
            systemctl --user -M "$user@" try-restart mihomo-server.service
        done < <(loginctl list-users --no-legend)
    fi
    # Prune only after successful activation; retain the prior release for rollback.
    local entry
    for entry in "$releases"/*; do
        [ -d "$entry" ] || continue
        case "$(basename "$entry")" in "$(basename "$release")" | "${previous:-/}") ;; *) rm -rf -- "${entry:?}" ;; esac
    done
    echo "System install complete: $root/current"
    if [ -z "$caller" ]; then echo 'No non-root caller: users opt in with mihomo-server enable'; fi
    if [ -L /usr/local/bin/mihomo-server ]; then echo 'Manage your instance with: mihomo-server --help'; fi
}
# Runs as the instance owner, never root, so only that user's files can be removed.
purge_user() {
    local home=$1 dropin config_home data_home env_file data configured
    dropin="$home/.config/systemd/user/mihomo-server.service.d/10-mihomo-paths.conf"
    config_home="$home/.config"
    data_home="$home/.local/share"
    if [ -f "$dropin" ]; then
        configured=$(sed -n 's/^# config-home=//p' "$dropin")
        [[ "$configured" != /?* ]] || config_home=$configured
        configured=$(sed -n 's/^# data-home=//p' "$dropin")
        [[ "$configured" != /?* ]] || data_home=$configured
    fi
    env_file="$config_home/mihomo-server/env"
    data="$data_home/mihomo-server"
    if [ -f "$env_file" ]; then
        configured=$(sed -n 's/^MIHOMO_SERVER_DATA_DIR=//p' "$env_file" | tail -n 1)
        configured=${configured#\"}; configured=${configured%\"}
        [[ "$configured" != /?* ]] || data=$configured
    fi
    case "$data" in / | "$home" | "$home/") die "refusing to delete $data" ;; esac
    rm -rf -- "$data" "$config_home/mihomo-server" "$home/.config/systemd/user/mihomo-server.service.d"
    rm -f -- "$home/.config/systemd/user/mihomo-server.service".pre-system-*
    # The program of a migrated per-user installation, if it is still there.
    if [ -f "$home/.local/opt/mihomo-server/launch" ] && [ -f "$home/.local/opt/mihomo-server/bin/mihomo-server" ]; then
        rm -rf -- "$home/.local/opt/mihomo-server"
    fi
    echo "Purged instance data of $(id -un): $data"
}
uninstall_shared() {
    local purge=$1 user uid home link
    command -v systemctl >/dev/null || die 'missing systemctl'
    each_user_manager disable --now mihomo-server.service || true
    while IFS=: read -r user _ uid _ _ home _; do
        if [ "$uid" = 0 ] || [ ! -d "$home" ]; then continue; fi
        # Enablement, also for stopped managers. The helper's path drop-in is
        # user configuration: kept for a reinstall unless purging.
        link="$home/.config/systemd/user/default.target.wants/mihomo-server.service"
        if [ -L "$link" ]; then runuser -u "$user" -- rm -f -- "$link"; fi
        if [ "$purge" = 1 ] && { [ -d "$home/.local/share/mihomo-server" ] || [ -d "$home/.config/mihomo-server" ] \
            || [ -f "$home/.config/systemd/user/mihomo-server.service.d/10-mihomo-paths.conf" ] \
            || [ -n "$(find /var/lib/mihomo-server/slots -maxdepth 1 -type f -uid "$uid" 2>/dev/null)" ]; }; then
            runuser -u "$user" -- bash -euo pipefail -c "$(declare -f die purge_user); purge_user \"\$1\"" bash "$home"
        fi
    done < <(getent passwd)
    rm -f /etc/systemd/user/mihomo-server.service
    for link in /usr/local/bin/mihomo-server-user /usr/local/bin/mihomo-server /usr/local/share/man/man1/mihomo-server.1 \
        /usr/local/share/bash-completion/completions/mihomo-server /usr/local/share/zsh/site-functions/_mihomo-server; do
        if [ -L "$link" ] && [[ "$(readlink "$link")" = /opt/mihomo-server/* ]]; then rm -f "$link"; fi
    done
    each_user_manager daemon-reload || true
    rm -rf /opt/mihomo-server
    if [ -d /var/lib/mihomo-server/linger ]; then
        for link in /var/lib/mihomo-server/linger/*; do
            [ -f "$link" ] || continue
            loginctl disable-linger "$(basename "$link")" || true
        done
    fi
    rm -rf /var/lib/mihomo-server
    polkit_tun_dns remove
    update_unit remove
    if getent group mihomo-tun >/dev/null; then groupdel mihomo-tun; fi
    if [ "$purge" = 1 ]; then
        echo 'Uninstalled mihomo-server and deleted all instance data.'
    else
        echo "Uninstalled mihomo-server; users' subscriptions and settings are kept."
    fi
}
root_action() {
    local action=$1
    shift
    if [ "$(id -u)" = 0 ]; then
        "$action" "$@"
    else
        # Explicit script functions only, with values passed as argv, never code.
        local definitions
        definitions=$(declare -f die verify_bundle each_user_manager activate_user polkit_tun_dns update_unit install_shared purge_user uninstall_shared)
        if [ "${MIHOMO_INSTALL_ELEVATE:-}" = pkexec ]; then
            # Graphical clients have no terminal for sudo; polkit asks instead.
            pkexec "$(command -v bash)" -euo pipefail -c "$definitions"$'\n'"$action \"\$@\"" bash "$@"
        else
            sudo -- bash -euo pipefail -c "$definitions"$'\n'"$action \"\$@\"" bash "$@"
        fi
    fi
}
# The elevation tool root_action will use, when not already root.
require_elevation() {
    [ "$(id -u)" != 0 ] || return 0
    case "${MIHOMO_INSTALL_ELEVATE:-sudo}" in
        sudo) command -v sudo >/dev/null || die 'sudo is required; alternatively run the installer as root' ;;
        pkexec) command -v pkexec >/dev/null || die 'pkexec (polkit) is required for a graphical installation' ;;
        *) die "unsupported MIHOMO_INSTALL_ELEVATE: $MIHOMO_INSTALL_ELEVATE" ;;
    esac
}
main() {
    # CI replaces this assignment, including for a piped/elevated invocation.
    local REPO="__REPO_SLUG__"
    local uninstall=0 purge=0 caller='' caller_home config_home data_home bundle tag work='' arch name base
    while [ $# -gt 0 ]; do
        case "$1" in
            -h | --help) usage; return ;;
            --uninstall) uninstall=1 ;;
            --purge) purge=1 ;;
            *) die "unknown option: $1 (see --help)" ;;
        esac
        shift
    done
    if [ "$purge" = 1 ] && [ "$uninstall" = 0 ]; then die '--purge is only valid with --uninstall'; fi
    if [ "$(id -u)" != 0 ]; then caller=$(id -un)
    elif [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ]; then caller=$SUDO_USER; fi
    require_elevation
    if [ "$uninstall" = 1 ]; then root_action uninstall_shared "$purge"; return; fi
    if [ -n "$caller" ]; then
        caller_home=$(getent passwd "$caller" | cut -d: -f6)
        [ -n "$caller_home" ] || die 'cannot find installing user'
    else caller_home=/root; fi
    case "${XDG_CONFIG_HOME:-}" in /*) config_home=$XDG_CONFIG_HOME ;; *) config_home="$caller_home/.config" ;; esac
    case "${XDG_DATA_HOME:-}" in /*) data_home=$XDG_DATA_HOME ;; *) data_home="$caller_home/.local/share" ;; esac
    command -v systemctl >/dev/null || die 'missing systemctl'
    # Internal environment overrides are for the isolated acceptance harness.
    bundle=${MIHOMO_INSTALL_BUNDLE:-}
    tag=${MIHOMO_INSTALL_TAG:-}
    base=${MIHOMO_INSTALL_BASE_URL:-https://github.com}
    REPO=${MIHOMO_INSTALL_REPO:-$REPO}
    if [ -z "$bundle" ]; then
        [ "$REPO" != __REPO_SLUG__ ] || die 'use the published installer; repository was not rendered'
        arch=$(uname -m)
        [ "$arch" = x86_64 ] || die "unsupported architecture: $arch"
        command -v tar >/dev/null || die 'missing tar'
        if ! command -v sha256sum >/dev/null && ! command -v shasum >/dev/null; then die 'need sha256sum or shasum'; fi
        if [ -z "$tag" ]; then
            tag=$(latest_tag "$base/$REPO/releases/latest") || die 'cannot query latest release'
        fi
        case "$tag" in '' | . | .. | */* | .*) die 'invalid release tag' ;; esac
        name="mihomo-server-$tag-x86_64-unknown-linux-gnu"
        work=$(mktemp -d)
        # Trap stores the quoted path, not a function-local variable after return.
        # shellcheck disable=SC2064
        trap "rm -rf -- $(printf '%q' "$work")" EXIT
        echo "==> downloading $name"
        fetch_progress "$base/$REPO/releases/download/$tag/$name.tar.gz" "$work/$name.tar.gz" || die 'bundle download failed'
        fetch "$base/$REPO/releases/download/$tag/$name.tar.gz.sha256" "$work/checksum" || die 'checksum download failed'
        local digest
        digest=$(awk 'NR==1 {print $1}' "$work/checksum")
        [[ "$digest" =~ ^[a-fA-F0-9]{64}$ ]] || die 'invalid SHA-256 checksum'
        printf '%s  %s\n' "$digest" "$name.tar.gz" > "$work/checksum"
        if command -v sha256sum >/dev/null; then
            (cd "$work" && sha256sum -c checksum >/dev/null) || die 'checksum mismatch'
        else (cd "$work" && shasum -a 256 -c checksum >/dev/null) || die 'checksum mismatch'; fi
        tar -xzf "$work/$name.tar.gz" -C "$work"
        bundle="$work/$name"
    fi
    bundle=$(cd "$bundle" && pwd -P) || die 'bundle directory not found'
    verify_bundle "$bundle"
    root_action install_shared "$bundle" "$tag" "$caller" "$config_home" "$data_home"
}

# Also usable as an internal library for local-bundle tests. A piped script runs
# main only after all function definitions have been received.
if [ -z "${BASH_SOURCE[0]:-}" ] || [ "${BASH_SOURCE[0]}" = "$0" ]; then main "$@"; fi
