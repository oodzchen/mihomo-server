#!/usr/bin/env bash
# Multi-user end-to-end checks, run as root inside the test container by run.sh.
#
# /work holds: install.sh, bundle/ (extracted release bundle) and an optional
# profile.yaml (real subscription). Users: alice + bob (TUN group), carol (not).
set -uo pipefail

PASS=0
FAIL=0
check() {
    local name="$1"
    shift
    if "$@"; then
        echo "PASS  $name"
        PASS=$((PASS + 1))
    else
        echo "FAIL  $name"
        FAIL=$((FAIL + 1))
    fi
}
fatal() {
    echo "FATAL $*"
    exit 1
}

as() {
    local user="$1"
    shift
    runuser -u "$user" -- env XDG_RUNTIME_DIR="/run/user/$(id -u "$user")" "$@"
}
port() { echo $((20000 + 10 * $(slot_of "$1"))); }
slot_of() {
    local f
    for f in /var/lib/mihomo-server/slots/*; do
        [ "$(stat -c %u "$f")" = "$(id -u "$1")" ] && basename "$f" && return
    done
}
api() {
    local user="$1"
    shift
    as "$user" msapi "$(port "$user")" "/home/$user/.local/share/mihomo-server/management-token" "$@"
}
dev() { echo "ms$(id -u "$1")"; }
# Bytes the kernel received from a TUN device (data the core wrote back).
traffic() { cat "/sys/class/net/$1/statistics/rx_bytes" 2>/dev/null || echo 0; }
fetch() {
    # fetch USER [CURL ARGS...]: one successful generate_204 within three attempts.
    local user="$1" code
    shift
    for _ in 1 2 3; do
        code="$(as "$user" curl -s -o /dev/null -w '%{http_code}' --max-time 15 "$@" \
            https://www.gstatic.com/generate_204)"
        [ "$code" = 204 ] && return 0
        sleep 1
    done
    return 1
}
DOWNLOAD_BYTES=2000000
download() {
    local user="$1" size
    for _ in 1 2 3; do
        size="$(as "$user" curl -s -o /dev/null -w '%{size_download}' --max-time 30 \
            "https://speed.cloudflare.com/__down?bytes=$DOWNLOAD_BYTES")"
        [ "${size:-0}" -ge "$DOWNLOAD_BYTES" ] && return 0
        sleep 1
    done
    echo "      download as $user failed"
    return 1
}
# A 2 MB download by USER grows only the TUN it should use (or none). Fresh TUN
# devices emit small IPv6/MLD packets, so growth below 200 kB counts as idle.
uses_tun() {
    local user="$1" expected="$2" before_a before_b grew_a grew_b want
    before_a="$(traffic "$(dev alice)")"
    before_b="$(traffic "$(dev bob)")"
    download "$user" || return 1
    grew_a=$(( $(traffic "$(dev alice)") - before_a > 1500000 ? 1 : ($(traffic "$(dev alice)") - before_a > 200000 ? 2 : 0) ))
    grew_b=$(( $(traffic "$(dev bob)") - before_b > 1500000 ? 1 : ($(traffic "$(dev bob)") - before_b > 200000 ? 2 : 0) ))
    case "$expected" in
        alice) want=10 ;;
        bob) want=01 ;;
        none) want=00 ;;
    esac
    [ "$grew_a$grew_b" = "$want" ] || {
        echo "      TUN download share (alice,bob)=$grew_a$grew_b, want $want (1 full, 2 partial)"
        return 1
    }
}
routes_via() { ip route get 1.1.1.1 uid "$(id -u "$1")" | grep -q "dev $2 "; }
routes_direct() { ! ip route get 1.1.1.1 uid "$(id -u "$1")" | grep -q "dev ms"; }
# The user's resolver answers come from their own slot's fake-IP /22.
fake_ip_in() {
    local address slot
    address="$(as "$1" getent ahostsv4 www.example.com | awk 'NR==1 {print $1}')"
    slot="$(slot_of "$1")"
    python3 -c 'import ipaddress,sys; sys.exit(ipaddress.ip_address(sys.argv[1]) not in ipaddress.ip_network(f"198.19.{4*int(sys.argv[2])}.0/22"))' \
        "${address:-0.0.0.0}" "$slot" || { echo "      got '$address' for slot $slot"; return 1; }
}
not_fake_ip() {
    local address
    address="$(as "$1" getent ahostsv4 www.example.com | awk 'NR==1 {print $1}')"
    [ -n "$address" ] && case "$address" in 198.19.*) false ;; *) true ;; esac
}
slot_rules() {
    # Policy rules in [base, base+32): one slot's sing-tun range.
    local base=$((10000 + 32 * $1))
    ip rule | awk -F: -v lo="$base" -v hi="$((base + 32))" '$1 >= lo && $1 < hi' | wc -l
}
only_default_rules() { [ "$(ip rule | awk -F: '$1 > 0 && $1 < 32766' | wc -l)" = 0 ]; }
core_pid() { pgrep -u "$1" verge-mihomo | head -n 1; }
# wait_for TRIES COMMAND...: retry every half second.
wait_for() {
    local tries="$1"
    shift
    for _ in $(seq 1 "$tries"); do
        "$@" && return 0
        sleep 0.5
    done
    return 1
}
carol_tun_rejected() {
    local output
    output="$(api carol set_settings "$TUN_ON" 2>&1)" && { echo "      accepted: $output"; return 1; }
    grep -q 'TUN group' <<<"$output" || { echo "      got: $output"; return 1; }
}
link_up() { ip link show "$1" up 2>/dev/null | grep -q UP; }

profile_import() {
    local user="$1" request="/tmp/import-$1.json"
    if [ -f /work/profile.yaml ]; then
        python3 -c 'import json,sys; json.dump({"name":"real","yaml":open("/work/profile.yaml").read()}, open(sys.argv[1],"w"))' "$request"
    else
        # Without real nodes: DIRECT still exercises TUN capture and fake-IP DNS.
        python3 - "$request" <<'EOF'
import json, sys
yaml = """mode: rule
dns: {enable: true, enhanced-mode: fake-ip, nameserver: [223.5.5.5, 1.1.1.1]}
proxies: []
proxy-groups: [{name: Main, type: select, proxies: [DIRECT]}]
rules: ['MATCH,Main']
"""
json.dump({"name": "direct", "yaml": yaml}, open(sys.argv[1], "w"))
EOF
    fi
    chmod 644 "$request"
    local uid
    uid="$(api "$user" import_profile "@$request" | python3 -c 'import json,sys; print(json.load(sys.stdin)["uid"])')" || return 1
    api "$user" select_profile "{\"uid\":\"$uid\"}" >/dev/null
}
TUN_ON='{"runtime":{"tun":{"enable":true,"stack":"mixed","auto-route":true,"auto-detect-interface":true,"dns-hijack":["any:53"]}}}'

# ---------------------------------------------------------------- install
echo "== system install"
bash /work/install.sh --system --bundle /work/bundle --tun-user alice --tun-user bob >/tmp/install.log 2>&1 \
    || { cat /tmp/install.log; fatal "system install failed"; }
TUN_CORE=/opt/mihomo-server/current/resources/core/verge-mihomo-tun
check "tun core has file capabilities" bash -c "getcap $TUN_CORE | grep -q 'cap_net_admin'"
check "tun core is root:mihomo-tun 0750" test "$(stat -c '%a %U:%G' $TUN_CORE)" = "750 root:mihomo-tun"
check "bundle is not writable by users" bash -c '! find /opt/mihomo-server/releases -perm /022 -not -type l | grep -q .'
check "slot registry is root 1777" test "$(stat -c '%a %U' /var/lib/mihomo-server/slots)" = "1777 root"
check "global user unit installed" test -f /etc/systemd/user/mihomo-server.service
check "helper on PATH" test -x /usr/local/bin/mihomo-server-user
check "alice and bob in TUN group, carol not" bash -c \
    'id -nG alice | grep -qw mihomo-tun && id -nG bob | grep -qw mihomo-tun && ! id -nG carol | grep -qw mihomo-tun'

# Linger after the group change, so each user manager starts with its groups.
loginctl enable-linger alice bob carol
for user in alice bob carol; do
    wait_for 40 test -S "/run/user/$(id -u $user)/bus" || fatal "$user manager did not start"
done

# ---------------------------------------------------------------- per-user init
echo "== per-user init"
for user in alice bob carol; do
    as "$user" mihomo-server-user init >"/tmp/init-$user.log" 2>&1 \
        || { cat "/tmp/init-$user.log"; fatal "$user init failed"; }
done
check "three distinct slots" test "$(find /var/lib/mihomo-server/slots -type f | wc -l)" = 3
for user in alice bob carol; do
    check "$user management + mixed ports listen" bash -c \
        "ss -ltn | grep -q '127.0.0.1:$(port $user) ' && ss -ltn | grep -q ':$(($(port $user) + 1)) '"
done
check "alice init reports TUN available" grep -q 'tun:.*available ' /tmp/init-alice.log
check "carol init reports TUN unavailable" grep -q 'tun:.*unavailable' /tmp/init-carol.log
check "alice runs the TUN core" bash -c "pgrep -u alice -x verge-mihomo-tu >/dev/null"
check "carol runs the plain core" bash -c "pgrep -u carol -x verge-mihomo >/dev/null"

# ---------------------------------------------------------------- concurrent TUN
echo "== concurrent TUN for alice and bob"
for user in alice bob carol; do
    profile_import "$user" || fatal "$user profile import failed"
done
for user in alice bob; do
    api "$user" set_settings "$TUN_ON" >/dev/null || fatal "$user TUN enable failed"
done
check "carol TUN enable is rejected" carol_tun_rejected
check "carol core still running after rejection" bash -c "pgrep -u carol -x verge-mihomo >/dev/null"
check "no TUN device for carol" bash -c "! ip link show $(dev carol) >/dev/null 2>&1"
check "alice TUN device up" link_up "$(dev alice)"
check "bob TUN device up" link_up "$(dev bob)"
A_ADDR="$(ip -4 -o addr show "$(dev alice)" | awk '{print $4}')"
B_ADDR="$(ip -4 -o addr show "$(dev bob)" | awk '{print $4}')"
check "TUN addresses are distinct slot /30s ($A_ADDR, $B_ADDR)" bash -c \
    "[ '$A_ADDR' != '$B_ADDR' ] && [[ '$A_ADDR' == 198.19.*/30 ]] && [[ '$B_ADDR' == 198.19.*/30 ]]"
A_RULES="$(slot_rules "$(slot_of alice)")"
B_RULES="$(slot_rules "$(slot_of bob)")"
check "each TUN user owns rules in its slot range ($A_RULES, $B_RULES)" test "$A_RULES" -gt 0 -a "$B_RULES" -gt 0
check "kernel routes alice via her TUN" routes_via alice "$(dev alice)"
check "kernel routes bob via his TUN" routes_via bob "$(dev bob)"
check "kernel routes carol directly" routes_direct carol
check "kernel routes root directly" routes_direct root
check "alice traffic uses only alice's TUN" uses_tun alice alice
check "bob traffic uses only bob's TUN" uses_tun bob bob
check "carol traffic uses no TUN" uses_tun carol none
check "root traffic uses no TUN" uses_tun root none
check "carol proxy port works" fetch carol -x "http://127.0.0.1:$(($(port carol) + 1))"
check "alice DNS answers from alice's fake-IP range" fake_ip_in alice
check "bob DNS answers from bob's fake-IP range" fake_ip_in bob
check "carol DNS is not hijacked into a slot" not_fake_ip carol
parallel() {
    fetch alice & local a=$!
    fetch bob & local b=$!
    wait $a && wait $b
}
check "alice and bob fetch in parallel" parallel

# ---------------------------------------------------------------- core crash
echo "== core crash recovery"
OLD_CORE="$(core_pid bob)"
kill -KILL "$OLD_CORE"
recovered() { local p; p="$(core_pid bob)"; [ -n "$p" ] && [ "$p" != "$OLD_CORE" ] && link_up "$(dev bob)"; }
check "bob core restarted with TUN after SIGKILL" wait_for 60 recovered
sleep 2
check "bob rule count unchanged after crash ($B_RULES)" test "$(slot_rules "$(slot_of bob)")" = "$B_RULES"
check "bob traffic still uses only bob's TUN" uses_tun bob bob
check "alice unaffected by bob's crash" uses_tun alice alice

# ---------------------------------------------------------------- upgrade
echo "== upgrade restarts running instances"
OLD_A="$(pgrep -u alice -x mihomo-server)"
OLD_C="$(pgrep -u carol -x mihomo-server)"
cp -a /work/bundle /work/bundle-next
bash /work/install.sh --system --bundle /work/bundle-next >/tmp/upgrade.log 2>&1 \
    || { cat /tmp/upgrade.log; fatal "upgrade failed"; }
check "current points at the new release" test "$(readlink /opt/mihomo-server/current)" = releases/bundle-next
check "previous release kept" test -d /opt/mihomo-server/releases/bundle
restarted() { local p; p="$(pgrep -u "$1" -x mihomo-server)"; [ -n "$p" ] && [ "$p" != "$2" ]; }
check "alice instance restarted" wait_for 40 restarted alice "$OLD_A"
check "carol instance restarted" wait_for 40 restarted carol "$OLD_C"
check "alice runs from the new release" wait_for 40 bash -c \
    "readlink /proc/\$(pgrep -u alice -x mihomo-server)/exe | grep -q /releases/bundle-next/"
check "alice TUN back after upgrade" wait_for 60 link_up "$(dev alice)"
check "bob TUN back after upgrade" wait_for 60 link_up "$(dev bob)"
sleep 2
check "alice rules not duplicated after upgrade" test "$(slot_rules "$(slot_of alice)")" = "$A_RULES"
check "alice traffic uses only alice's TUN after upgrade" uses_tun alice alice
check "bob traffic uses only bob's TUN after upgrade" uses_tun bob bob

# ---------------------------------------------------------------- disable + cleanup
echo "== disable and uninstall cleanup"
as alice mihomo-server-user disable >/dev/null
check "alice TUN device removed on stop" wait_for 40 bash -c "! ip link show $(dev alice) >/dev/null 2>&1"
check "alice slot rules removed on stop" test "$(slot_rules "$(slot_of alice)")" = 0
check "alice now routes directly" routes_direct alice
check "bob unaffected by alice stopping" uses_tun bob bob
check "alice keeps her slot after disable" test -n "$(slot_of alice)"

bash /work/install.sh --system --uninstall >/tmp/uninstall.log 2>&1 || { cat /tmp/uninstall.log; fatal "uninstall failed"; }
check "no instances left" wait_for 40 bash -c "! pgrep -x mihomo-server >/dev/null && ! pgrep verge-mihomo >/dev/null"
check "all TUN devices removed" bash -c "! ip -o link | grep -q ': ms[0-9]'"
check "only default policy rules remain" only_default_rules
check "bundle, unit and helper removed" bash -c \
    "[ ! -e /opt/mihomo-server ] && [ ! -e /etc/systemd/user/mihomo-server.service ] && [ ! -e /usr/local/bin/mihomo-server-user ]"
check "users' data kept" bash -c "[ -s /home/alice/.local/share/mihomo-server/management-token ] && [ -s /home/bob/.local/share/mihomo-server/management-token ]"

echo
echo "multi-user e2e: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
