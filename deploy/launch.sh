#!/bin/sh
set -eu
umask 077
bundle_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
: "${MIHOMO_SERVER_DATA_DIR:?Set MIHOMO_SERVER_DATA_DIR to an absolute persistent data directory}"
case "$MIHOMO_SERVER_DATA_DIR" in
    /*) ;;
    *) echo 'MIHOMO_SERVER_DATA_DIR must be absolute' >&2; exit 1 ;;
esac
exec "$bundle_root/bin/mihomo-server" \
    --resource-dir "$bundle_root/resources" \
    --data-dir "$MIHOMO_SERVER_DATA_DIR" "$@"
