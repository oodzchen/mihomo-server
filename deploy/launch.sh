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
                command -v sg >/dev/null 2>&1 || { echo 'TUN authorization requires sg' >&2; exit 1; }
                # Quote each argument for sg's /bin/sh -c; never eval user options.
                command_line="exec '$(printf '%s' "$bundle_root/launch" | sed "s/'/'\\\\''/g")'"
                for argument do
                    command_line="$command_line '$(printf '%s' "$argument" | sed "s/'/'\\\\''/g")'"
                done
                exec sg mihomo-tun -c "$command_line"
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
