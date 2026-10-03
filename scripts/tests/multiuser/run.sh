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

[ -f "$bundle/launch" ] && [ -f "$bundle/mihomo-server-user" ] || {
    echo "error: $bundle is not a bundle with mihomo-server-user" >&2
    exit 1
}

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

for _ in $(seq 1 60); do
    state="$("$engine" exec "$name" systemctl is-system-running 2>/dev/null || true)"
    case "$state" in running | degraded) break ;; esac
    sleep 0.5
done

"$engine" exec "$name" mkdir -p /work
"$engine" cp "$bundle" "$name:/work/bundle"
"$engine" cp "$root/scripts/install_remote.sh" "$name:/work/install.sh"
"$engine" cp "$here/inside.sh" "$name:/work/inside.sh"
"$engine" cp "$here/api.py" "$name:/usr/local/bin/msapi"
[ -z "$profile" ] || "$engine" cp "$profile" "$name:/work/profile.yaml"
"$engine" exec "$name" chmod 755 /usr/local/bin/msapi /work/inside.sh

"$engine" exec "$name" /work/inside.sh
