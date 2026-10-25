#!/bin/sh
set -eu
umask 077
bundle_root=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
case "${XDG_DATA_HOME:-}" in
    /*) data_home=$XDG_DATA_HOME ;;
    *) data_home=$HOME/.local/share ;;
esac
: "${MIHOMO_SERVER_DATA_DIR:=$data_home/mihomo-server}"
case "$MIHOMO_SERVER_DATA_DIR" in
    /?*) ;;
    *) echo 'MIHOMO_SERVER_DATA_DIR must be an absolute directory other than /' >&2; exit 1 ;;
esac

# Arguments carried through newgrp's login shell (see below) replace "$@" once.
entered_group=0
if [ -n "${MIHOMO_LAUNCH_ARGC+x}" ]; then
    count=$MIHOMO_LAUNCH_ARGC
    case "$count" in '' | *[!0-9]*) echo 'invalid MIHOMO_LAUNCH_ARGC' >&2; exit 1 ;; esac
    entered_group=1
    exec </dev/null
    set --
    i=0
    while [ "$i" -lt "$count" ]; do
        i=$((i + 1))
        # Only the variable name is constructed; the value is never evaluated.
        eval "set -- \"\$@\" \"\${MIHOMO_LAUNCH_ARG_$i}\""
        unset "MIHOMO_LAUNCH_ARG_$i"
    done
    unset MIHOMO_LAUNCH_ARGC MIHOMO_LAUNCH
fi

# A user manager may predate the administrator's group authorization. Enter the
# authorized group for this process only; do not restart unrelated user services.
multi_user=0
for argument do
    [ "$argument" != --multi-user ] || multi_user=1
done
if [ "$multi_user" = 1 ] && [ "$(id -u)" != 0 ]; then
    tun_gid=$(getent group mihomo-tun | cut -d: -f3 || true)
    case " $(id -G) " in
        *" $tun_gid "*) ;;
        *)
            if [ -n "$tun_gid" ] && id -G "$(id -un)" | tr ' ' '\n' | grep -qx "$tun_gid"; then
                [ "$entered_group" = 0 ] || { echo 'entering the mihomo-tun group did not take effect' >&2; exit 1; }
                if command -v sg >/dev/null 2>&1; then
                    # Quote each argument for sg's /bin/sh -c; never eval user options.
                    command_line="exec '$(printf '%s' "$bundle_root/launch" | sed "s/'/'\\\\''/g")'"
                    for argument do
                        command_line="$command_line '$(printf '%s' "$argument" | sed "s/'/'\\\\''/g")'"
                    done
                    exec sg mihomo-tun -c "$command_line"
                fi
                # Some distributions (Arch) ship only util-linux newgrp, which starts the
                # login shell without a command. That shell reads one fixed line from
                # stdin; the path and arguments travel in the environment, so nothing
                # depends on the login shell's quoting rules.
                command -v newgrp >/dev/null 2>&1 || { echo 'TUN authorization requires sg or newgrp' >&2; exit 1; }
                MIHOMO_LAUNCH=$bundle_root/launch
                MIHOMO_LAUNCH_ARGC=$#
                export MIHOMO_LAUNCH MIHOMO_LAUNCH_ARGC
                i=0
                for argument do
                    i=$((i + 1))
                    export "MIHOMO_LAUNCH_ARG_$i=$argument"
                done
                exec newgrp mihomo-tun <<'LAUNCH'
exec "$MIHOMO_LAUNCH"
LAUNCH
            fi
            ;;
    esac
fi

# Named settings take precedence over the corresponding legacy arguments.
# Rotate the original arguments, retaining their boundaries (including spaces).
remaining=$#
while [ "$remaining" -gt 0 ]; do
    argument=$1
    shift
    remaining=$((remaining - 1))
    case "$argument" in
        --listen)
            if [ -n "${MIHOMO_SERVER_LISTEN:-}" ]; then
                [ "$remaining" -gt 0 ] || { echo '--listen requires a value' >&2; exit 1; }
                shift; remaining=$((remaining - 1)); continue
            fi ;;
        --public-origin)
            if [ -n "${MIHOMO_SERVER_PUBLIC_ORIGIN:-}" ]; then
                [ "$remaining" -gt 0 ] || { echo '--public-origin requires a value' >&2; exit 1; }
                shift; remaining=$((remaining - 1)); continue
            fi ;;
        --listen=*) [ -z "${MIHOMO_SERVER_LISTEN:-}" ] || continue ;;
        --public-origin=*) [ -z "${MIHOMO_SERVER_PUBLIC_ORIGIN:-}" ] || continue ;;
    esac
    set -- "$@" "$argument"
done
[ -z "${MIHOMO_SERVER_LISTEN:-}" ] || set -- "$@" --listen "$MIHOMO_SERVER_LISTEN"
[ -z "${MIHOMO_SERVER_PUBLIC_ORIGIN:-}" ] || set -- "$@" --public-origin "$MIHOMO_SERVER_PUBLIC_ORIGIN"
exec "$bundle_root/bin/mihomo-server" \
    --resource-dir "$bundle_root/resources" \
    --data-dir "$MIHOMO_SERVER_DATA_DIR" "$@"
