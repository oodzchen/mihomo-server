#!/usr/bin/env bash
# Multi-user end-to-end test in a privileged systemd container.
#
# Usage: scripts/tests/multiuser/run.sh BUNDLE_DIR [PROFILE_YAML]
#
#   BUNDLE_DIR    extracted release bundle (scripts/package_bundle.py output)
#   PROFILE_YAML  optional real subscription; without it a DIRECT-only profile
#                 still exercises TUN capture, per-UID routing and fake-IP DNS
#
# The container has its own network namespace, so its TUN devices and policy
# rules never touch the host. Needs docker (or CONTAINER_ENGINE=podman) able to
# run --privileged containers. KEEP=1 leaves the container running for debugging.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
bundle="${1:?usage: run.sh BUNDLE_DIR [PROFILE_YAML]}"
profile="${2:-}"
engine="${CONTAINER_ENGINE:-docker}"
image="mihomo-server-multiuser-test"
name="mihomo-server-multiuser-$$"

if [ ! -f "$bundle/launch" ] || [ ! -f "$bundle/mihomo-server-user" ]; then
    echo "error: $bundle is not a bundle with mihomo-server-user" >&2
    exit 1
fi

# Inherited host proxy settings (often 127.0.0.1) are wrong inside the container.
no_proxy_args=()
for variable in http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY; do
    no_proxy_args+=(--build-arg "$variable=")
done
"$engine" build -q "${no_proxy_args[@]}" -t "$image" -f "$here/Containerfile" "$here" >/dev/null

env_args=()
for variable in http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY; do
    env_args+=(-e "$variable=")
done
"$engine" run -d --name "$name" --privileged --cgroupns=private "${env_args[@]}" "$image" >/dev/null
cleanup() {
    if [ "${KEEP:-0}" = 1 ]; then
        echo "container kept: $engine exec -it $name bash"
    else
        "$engine" rm -f "$name" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# Systemd state for CI logs, where the container is gone after a failure.
diagnose() {
    echo "== container diagnostics"
    "$engine" exec "$name" systemctl --failed --no-pager || true
    local user uid
    for user in alice bob carol; do
        uid="$("$engine" exec "$name" id -u "$user")" || continue
        "$engine" exec "$name" systemctl status "user@$uid.service" "user-runtime-dir@$uid.service" \
            --no-pager -l || true
    done
    "$engine" exec "$name" journalctl -b --no-pager -n 200 || true
}

booted=0
for _ in $(seq 1 120); do
    state="$("$engine" exec "$name" systemctl is-system-running 2>/dev/null || true)"
    case "$state" in running | degraded) booted=1 && break ;; esac
    sleep 0.5
done
if [ "$booted" != 1 ]; then
    echo "error: container systemd did not finish booting (state: ${state:-unknown})" >&2
    diagnose
    exit 1
fi

"$engine" exec "$name" mkdir -p /work
"$engine" cp "$bundle" "$name:/work/bundle"
"$engine" cp "$root/scripts/install_remote.sh" "$name:/work/install.sh"
"$engine" cp "$here/inside.sh" "$name:/work/inside.sh"
"$engine" cp "$here/release_fixture.py" "$name:/work/release_fixture.py"
"$engine" cp "$here/api.py" "$name:/usr/local/bin/msapi"
[ -z "$profile" ] || "$engine" cp "$profile" "$name:/work/profile.yaml"
"$engine" exec "$name" chmod 755 /usr/local/bin/msapi /work/inside.sh

"$engine" exec "$name" /work/inside.sh before-reboot || {
    diagnose
    exit 1
}

# Verify actual boot restoration, rather than starting the instance manually.
"$engine" restart "$name" >/dev/null
for _ in $(seq 1 120); do
    state="$("$engine" exec "$name" systemctl is-system-running 2>/dev/null || true)"
    case "$state" in running | degraded) break ;; esac
    sleep 0.5
done
"$engine" exec "$name" /work/inside.sh after-reboot || {
    diagnose
    exit 1
}
