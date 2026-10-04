#!/usr/bin/env bash
# Multi-user end-to-end checks, run as root inside the test container by run.sh.
#
# /work holds: install.sh, bundle/ (extracted release bundle) and an optional
# profile.yaml (real subscription). Users: alice (installer) + bob (TUN group),
# carol (not). The host has one system-wide TUN, held by whoever enables it first.
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
    # runuser -u does not register a logind session on Fedora. Use the login
    # PAM stack so persistent test sessions keep opt-in user managers alive.
    local invocation
    printf -v invocation '%q ' env "XDG_RUNTIME_DIR=/run/user/$(id -u "$user")" "$@"
    runuser -l "$user" -c "$invocation"
}
port() {
    if [ "$1" = dave ]; then echo 27190
    elif [ "$1" = eve ]; then echo 27191
    elif [ "$1" = bob ]; then echo 21919
    elif [ "$(slot_of "$1")" = 0 ]; then echo 9090
    else echo $((20000 + 10 * $(slot_of "$1"))); fi
}
mixed_port() {
    if [ "$(slot_of "$1")" = 0 ]; then echo 7890; else echo $((20001 + 10 * $(slot_of "$1"))); fi
}
data_dir() {
    if [ "$1" = dave ] || [ "$1" = eve ]; then printf '/home/%s/legacy-data' "$1"
    elif [ "$1" = bob ]; then printf '/home/bob/private data%%/mihomo-server'; else printf '/home/%s/.local/share/mihomo-server' "$1"; fi
}
slot_of() {
    local f
    for f in /var/lib/mihomo-server/slots/*; do
        [ "$(stat -c %u "$f")" = "$(id -u "$1")" ] && basename "$f" && return
    done
}
api() {
    local user="$1"
    shift
    local host
    host="127.0.0.1:$(port "$user")"
    [ "$user" != bob ] || host=bob.example
    as "$user" env MIHOMO_TEST_HOST="$host" msapi "$(port "$user")" "$(data_dir "$user")/management-token" "$@"
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
DOWNLOAD_BYTES=1048576
download() {
    local user="$1" size
    for _ in 1 2 3; do
        size="$(as "$user" curl -s -o /dev/null -w '%{size_download}' --max-time 30 \
            https://proof.ovh.net/files/1Mb.dat)"
        [ "${size:-0}" -ge "$DOWNLOAD_BYTES" ] && return 0
        sleep 1
    done
    echo "      download as $user failed"
    return 1
}
# A 1 MiB download by USER grows only the TUN it should use (or none). Fresh TUN
# devices emit small IPv6/MLD packets, so growth below 200 kB counts as idle.
uses_tun() {
    local user="$1" expected="$2" before_a before_b grew_a grew_b want
    before_a="$(traffic "$(dev alice)")"
    before_b="$(traffic "$(dev bob)")"
    download "$user" || return 1
    grew_a=$(( $(traffic "$(dev alice)") - before_a > DOWNLOAD_BYTES * 3 / 4 ? 1 : ($(traffic "$(dev alice)") - before_a > 200000 ? 2 : 0) ))
    grew_b=$(( $(traffic "$(dev bob)") - before_b > DOWNLOAD_BYTES * 3 / 4 ? 1 : ($(traffic "$(dev bob)") - before_b > 200000 ? 2 : 0) ))
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
# fake_ip_in USER [TUN_USER]: USER's resolver answers from TUN_USER's slot fake-IP /22.
fake_ip_in() {
    local address slot
    address="$(as "$1" getent ahostsv4 www.example.com | awk 'NR==1 {print $1}')"
    slot="$(slot_of "${2:-$1}")"
    python3 -c 'import ipaddress,sys; sys.exit(ipaddress.ip_address(sys.argv[1]) not in ipaddress.ip_network(f"198.19.{4*int(sys.argv[2])}.0/22"))' \
        "${address:-0.0.0.0}" "$slot" || { echo "      got '$address' for slot $slot"; return 1; }
}
# Shared local stub resolvers (systemd-resolved) answer for every user, so a
# per-user TUN never sees those DNS queries; sniffing recovers the domain.
stub_resolver() { grep -q '^nameserver 127\.' /etc/resolv.conf; }
check_unless_stub() {
    if stub_resolver; then
        echo "SKIP  $1 (local stub resolver)"
    else
        check "$@"
    fi
}
# The user's core routed their connection by domain (fake-IP or sniffing).
sees_domain() {
    fetch "$1" || return 1
    api "$1" logs | python3 -c '
import json, sys
logs = json.load(sys.stdin)
logs = logs if isinstance(logs, list) else logs.get("logs", [])
sys.exit(not any("--> www.gstatic.com:443" in str(entry) for entry in logs[-300:]))'
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
core_pid() { pgrep -u "$1" -x verge-mihomo | head -n 1; }
core_exe() { readlink "/proc/$(core_pid "$1")/exe"; }
# The user's own managed core runs with ambient CAP_NET_ADMIN only via the TUN launcher.
has_net_admin() { local amb; amb=$(awk '/^CapAmb:/ {print $2}' "/proc/$(core_pid "$1")/status"); (( 16#$amb & 0x1000 )); }
lacks_net_admin() { ! has_net_admin "$1"; }
# polkit answer for the core's resolvectl call on LINK: 1 = refused, 2 = would
# prompt for authentication (pkcheck exit codes; no agent is ever started).
resolved_answer() {
    pkcheck --action-id org.freedesktop.resolve1.set-domains --process "$(core_pid "$1")" \
        --detail interface "$2" >/dev/null 2>&1
    echo $?
}
# holder_is USER NAME: USER's Web view names NAME as the system TUN holder ('' = nobody).
holder_is() {
    api "$1" proxy_access | python3 -c '
import json, sys
holder, want = json.load(sys.stdin).get("tun_holder"), sys.argv[1]
sys.exit(not (holder is None if not want else holder is not None and holder["name"].startswith(want + " ")))' "$2"
}
# The system-wide TUN owns the shared resolver: all domains, default route and its DNS.
resolver_uses_tun() {
    resolvectl domain "$1" | grep -q '~\.' && resolvectl default-route "$1" | grep -q 'yes$' \
        && [ -n "$(resolvectl dns "$1" | cut -d: -f2 | tr -d ' ')" ]
}
# USER connects to the fake IP systemd-resolved hands out (alice's /22) through her TUN.
system_lookup_works() {
    local address
    address="$(resolvectl query --legend=no -4 --cache=no www.gstatic.com | awk 'NR==1 {print $2}')"
    case "$address" in 198.19.*) ;; *) echo "      resolved answered '$address'"; return 1 ;; esac
    fetch "$1" --resolve "www.gstatic.com:443:$address"
}
core_hash() { sha256sum "$(data_dir "$1")/core/verge-mihomo" | cut -d' ' -f1; }
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
# tun_rejected USER REASON: enabling TUN fails with an explanation containing REASON.
tun_rejected() {
    local output
    output="$(api "$1" set_settings "$TUN_ON" 2>&1)" && { echo "      accepted: $output"; return 1; }
    grep -q "$2" <<<"$output" || { echo "      got: $output"; return 1; }
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
log-level: info
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
parallel() {
    fetch alice & local a=$!
    fetch bob & local b=$!
    wait $a && wait $b
}
TUN_ON='{"runtime":{"tun":{"enable":true,"stack":"mixed","auto-route":true,"auto-detect-interface":true,"dns-hijack":["any:53"]}}}'

phase_install() {
    echo "== real zero-argument download + sudo, with an already-running user manager"
    python3 /work/release_fixture.py >/tmp/release-fixture.log 2>&1 &
    wait_for 120 curl -fsS http://127.0.0.1:18080/test/repo/releases/latest >/dev/null 2>&1 || fatal 'release fixture failed'
    systemctl start "user@$(id -u alice).service" "user@$(id -u bob).service" || fatal 'start user managers'
    as bob sleep infinity &
    check "alice not authorized before install" bash -c '! id -nG alice | grep -qw mihomo-tun'
    check "alice not lingering before install" test "$(loginctl show-user alice -p Linger --value)" = no
    ALICE_MANAGER=$(systemctl show "user@$(id -u alice).service" -p MainPID --value)
    as alice env MIHOMO_INSTALL_BASE_URL=http://127.0.0.1:18080 bash -c \
        'set -o pipefail; curl -fsSL http://127.0.0.1:18080/test/repo/releases/latest/download/install.sh | bash' \
        >/tmp/install.log 2>&1 || { cat /tmp/install.log; fatal 'zero-argument install failed'; }
    check "alice manager was not restarted for authorization" test "$(systemctl show "user@$(id -u alice).service" -p MainPID --value)" = "$ALICE_MANAGER"
    check "alice automatically lingering" test "$(loginctl show-user alice -p Linger --value)" = yes
    check "only installing user authorized" bash -c 'id -nG alice | grep -qw mihomo-tun && ! id -nG bob | grep -qw mihomo-tun && ! id -nG carol | grep -qw mihomo-tun'
    check "alice automatically claimed slot 0" test "$(slot_of alice)" = 0
    check "alice startup reports TUN available" grep -q 'tun:.*available ' /tmp/install.log
    check "alice startup reports a system-wide TUN" grep -q 'tun:.*available system-wide' /tmp/install.log
    check "system TUN lock is root:mihomo-tun 0640" test "$(stat -c '%a %U:%G' /var/lib/mihomo-server/tun.lock)" = '640 root:mihomo-tun'
    check "polkit TUN DNS rule installed" test -f /etc/polkit-1/rules.d/50-mihomo-server-tun.rules
    check "no other instances automatically started" bash -c '! pgrep -u bob -x mihomo-server && ! pgrep -u carol -x mihomo-server'
    check "user config template private" test "$(stat -c '%a %U' /home/alice/.config/mihomo-server/env)" = '600 alice'
    check "global unit copied verbatim" cmp /work/bundle/mihomo-server.service /etc/systemd/user/mihomo-server.service
    check "public help excludes installation parameters" bash -c '! bash /work/install.sh --help | grep -E -- "--system|--bundle|--tun-user|--listen"'
    TUN_LAUNCHER=/opt/mihomo-server/current/bin/mihomo-tun-exec
    check "TUN launcher has file capabilities" bash -c "getcap $TUN_LAUNCHER | grep -q cap_net_admin"
    check "TUN launcher is root:mihomo-tun 0750" test "$(stat -c '%a %U:%G' $TUN_LAUNCHER)" = '750 root:mihomo-tun'
    check "bundle is not writable by users" bash -c '! find /opt/mihomo-server/releases -perm /022 -not -type l | grep -q .'
    check "slot registry is root 1777" test "$(stat -c '%a %U' /var/lib/mihomo-server/slots)" = '1777 root'
    as alice mihomo-server-user info >/tmp/init-alice.log || fatal 'alice info'
}

phase_init() {
    echo "== opt-in users, custom XDG paths and named listener precedence"
    usermod -aG mihomo-tun bob
    systemctl start "user@$(id -u carol).service" || fatal 'carol manager'
    as carol sleep infinity &
    as bob mkdir -p '/home/bob/config space%/mihomo-server'
    cat >'/home/bob/config space%/mihomo-server/env' <<'CONFIG'
MIHOMO_SERVER_LISTEN=0.0.0.0:21919
MIHOMO_SERVER_PUBLIC_ORIGIN=https://bob.example
MIHOMO_SERVER_ARGS="--listen 127.0.0.1:21918 --public-origin http://legacy.example"
CONFIG
    chown bob:bob '/home/bob/config space%/mihomo-server/env'
    chmod 600 '/home/bob/config space%/mihomo-server/env'
    as bob env XDG_CONFIG_HOME='/home/bob/config space%' XDG_DATA_HOME='/home/bob/private data%' \
        mihomo-server-user enable >/tmp/init-bob.log 2>&1 || { cat /tmp/init-bob.log; fatal 'bob enable'; }
    as carol env XDG_CONFIG_HOME=relative XDG_DATA_HOME='' mihomo-server-user init >/tmp/init-carol.log 2>&1 \
        || { cat /tmp/init-carol.log; fatal 'carol init alias'; }
    as bob mihomo-server-user restart >/tmp/bob-restart.log 2>&1 || { cat /tmp/bob-restart.log; fatal 'bob restart'; }
    check "saved XDG paths survive a terminal without XDG variables" grep -q '/home/bob/config space%/mihomo-server/env' /tmp/bob-restart.log
    # manage: is the public browser URL when an origin is configured; the
    # actual bind address must be checked independently of that URL.
    check "named listener wins over legacy arguments" bash -c "ss -ltn | grep -q '0.0.0.0:$(port bob) '"
    check "named public origin wins over legacy arguments" grep -Eq \
        '^manage:[[:space:]]+https://bob\.example/#token=[[:xdigit:]]{64}$' /tmp/bob-restart.log
    check "carol invalid XDG falls back" test -s /home/carol/.config/mihomo-server/env
    check "bob TUN available without restarting manager" grep -q 'tun:.*available ' /tmp/init-bob.log
    check "carol TUN unavailable" grep -q 'tun:.*unavailable' /tmp/init-carol.log
    check "three distinct slots" test "$(find /var/lib/mihomo-server/slots -type f | wc -l)" = 3
    for user in alice bob carol; do
        check "$user management + mixed ports listen" bash -c \
            "ss -ltn | grep -q ':$(port $user) ' && ss -ltn | grep -q ':$(mixed_port $user) '"
    done
    check "alice runs her own managed core" test "$(core_exe alice)" = /home/alice/.local/share/mihomo-server/core/verge-mihomo
    check "alice's core has ambient TUN capabilities" has_net_admin alice
    check "carol's core has no capabilities" lacks_net_admin carol
    ALICE_TOKEN_HASH=$(sha256sum /home/alice/.local/share/mihomo-server/management-token | cut -d' ' -f1)
}

legacy_setup() {
    local user=$1 listen=$2 mixed=$3 old="/home/$1/.local/opt/mihomo-server" data="/home/$1/legacy-data"
    mkdir -p "/home/$user/.local/opt" "/home/$user/.config/systemd/user" "$data"
    cp -a /work/bundle "$old"
    printf 'mixed-port: %s\nmode: direct\nrules: ["MATCH,DIRECT"]\n' "$mixed" >"$old/resources/minimal.yaml"
    cat >"/home/$user/.config/systemd/user/mihomo-server.service" <<UNIT
[Unit]
Description=Legacy Mihomo instance
After=network.target
[Service]
Type=simple
Environment=MIHOMO_SERVER_DATA_DIR=%h/legacy-data
ExecStart=%h/.local/opt/mihomo-server/launch --listen 127.0.0.1:$listen
Restart=on-failure
RestartSec=3
KillSignal=SIGTERM
KillMode=mixed
TimeoutStopSec=30
UMask=0077
[Install]
WantedBy=default.target
UNIT
    chmod 700 "$data"
    chown -R "$user:$user" "/home/$user"
    loginctl enable-linger "$user"
    systemctl start "user@$(id -u "$user").service"
    as "$user" systemctl --user daemon-reload
    as "$user" systemctl --user enable --now mihomo-server
    legacy_running() { api "$1" status 2>/dev/null | python3 -c 'import json,sys; sys.exit(json.load(sys.stdin)["phase"] != "running")' 2>/dev/null; }
    wait_for 60 legacy_running "$user" || fatal 'legacy instance failed to start'
}

phase_migration() {
    echo '== legacy migration, failed migration rollback, and a caller without a manager'
    check "frank manager initially absent" bash -c '! systemctl is-active --quiet user@1005.service'
    env SUDO_USER=frank MIHOMO_INSTALL_BUNDLE=/work/bundle MIHOMO_INSTALL_TAG=vnewuser \
        bash /work/install.sh >/tmp/newuser.log 2>&1 || { cat /tmp/newuser.log; fatal 'new user activation'; }
    check "frank automatically started without existing manager" systemctl --user -M frank@ is-active --quiet mihomo-server
    check "frank automatically lingering" test "$(loginctl show-user frank -p Linger --value)" = yes
    as frank mihomo-server-user disable >/dev/null
    loginctl disable-linger frank
    systemctl stop user@1005.service

    legacy_setup dave 27190 27001
    local token_hash unit_hash
    token_hash=$(sha256sum /home/dave/legacy-data/management-token | cut -d' ' -f1)
    env SUDO_USER=dave MIHOMO_INSTALL_BUNDLE=/work/bundle MIHOMO_INSTALL_TAG=vmigrate \
        bash /work/install.sh >/tmp/migrate.log 2>&1 || { cat /tmp/migrate.log; fatal 'legacy migration'; }
    check "legacy token retained" test "$(sha256sum /home/dave/legacy-data/management-token | cut -d' ' -f1)" = "$token_hash"
    check "legacy data path retained" grep -q 'data: */home/dave/legacy-data' /tmp/migrate.log
    check "legacy listener retained" grep -q 'manage:.*27190' /tmp/migrate.log
    check "legacy unit no longer shadows shared unit" test ! -e /home/dave/.config/systemd/user/mihomo-server.service
    check "legacy unit backed up" bash -c 'compgen -G "/home/dave/.config/systemd/user/mihomo-server.service.pre-system-*" >/dev/null'
    check "legacy program retained" test -f /home/dave/.local/opt/mihomo-server/bin/mihomo-server
    as dave mihomo-server-user disable >/dev/null
    # The stopped helper must resolve the explicit migrated data override too.
    check "stopped instance token still uses legacy data" bash -c \
        'cmp /home/dave/legacy-data/management-token <(runuser -u dave -- env XDG_RUNTIME_DIR=/run/user/1003 mihomo-server-user token)'
    loginctl disable-linger dave
    systemctl stop user@1003.service

    legacy_setup eve 27191 27002
    token_hash=$(sha256sum /home/eve/legacy-data/management-token | cut -d' ' -f1)
    unit_hash=$(sha256sum /home/eve/.config/systemd/user/mihomo-server.service | cut -d' ' -f1)
    as eve mkdir -p /home/eve/.config/mihomo-server
    as eve bash -c 'umask 077; printf "MIHOMO_SERVER_LISTEN=0.0.0.0:27999\n" > /home/eve/.config/mihomo-server/env'
    if env SUDO_USER=eve MIHOMO_INSTALL_BUNDLE=/work/bundle MIHOMO_INSTALL_TAG=vfailed \
        bash /work/install.sh >/tmp/failed-migrate.log 2>&1; then fatal 'invalid migrated config accepted'; fi
    check "failed migration reported rollback" grep -q 'Restored the previous user unit' /tmp/failed-migrate.log
    check "failed migration restored original unit bytes" test "$(sha256sum /home/eve/.config/systemd/user/mihomo-server.service | cut -d' ' -f1)" = "$unit_hash"
    check "failed migration retained original token" test "$(sha256sum /home/eve/legacy-data/management-token | cut -d' ' -f1)" = "$token_hash"
    check "failed migration restored running old instance" wait_for 60 legacy_running eve
    printf '\nWorkingDirectory=/\n' >>/home/eve/.config/systemd/user/mihomo-server.service
    unit_hash=$(sha256sum /home/eve/.config/systemd/user/mihomo-server.service | cut -d' ' -f1)
    as eve systemctl --user daemon-reload
    if as eve bash -c 'source /work/install.sh; activate_user "$@"' bash /home/eve /home/eve/.config /usr/local/bin/mihomo-server-user \
        >/tmp/custom-unit.log 2>&1; then fatal 'custom unit overwritten'; fi
    check "custom unit rejected unchanged" test "$(sha256sum /home/eve/.config/systemd/user/mihomo-server.service | cut -d' ' -f1)" = "$unit_hash"
    as eve systemctl --user disable --now mihomo-server
    loginctl disable-linger eve
    systemctl stop user@1004.service
}

phase_port_conflict() {
    echo '== occupied management port is a startup failure'
    as carol bash -c 'printf "MIHOMO_SERVER_LISTEN=127.0.0.1:9090\n" >> /home/carol/.config/mihomo-server/env'
    if as carol mihomo-server-user restart >/tmp/port-conflict.log 2>&1; then fatal 'occupied port accepted'; fi
    check "port conflict reported" grep -q 'bind management listener' /tmp/port-conflict.log
    as carol bash -c 'sed -i "/^MIHOMO_SERVER_LISTEN=/d" /home/carol/.config/mihomo-server/env'
    as carol mihomo-server-user restart >/tmp/carol-repaired.log 2>&1 || { cat /tmp/carol-repaired.log; fatal 'carol repair'; }
    check "alice unaffected by conflicting listener" api alice status >/dev/null
}

tun_enable() {
    # ---------------------------------------------------------------- concurrent TUN
    echo "== system-wide TUN: alice enables it first"
    for user in alice bob carol; do
        profile_import "$user" || fatal "$user profile import failed"
    done
    api alice set_settings "$TUN_ON" >/dev/null || fatal "alice TUN enable failed"
    check "bob TUN enable is rejected while alice holds it" tun_rejected bob 'in use by alice'
    check "carol TUN enable is rejected" tun_rejected carol 'TUN group'
    check "bob's Web view names alice as TUN holder" holder_is bob alice
    check "carol core still running after rejection" bash -c "pgrep -u carol -x verge-mihomo >/dev/null"
}

tun_assert() {
    # ---------------------------------------------------------------- TUN assertions
    check "no TUN device for bob or carol" bash -c "! ip link show $(dev bob) >/dev/null 2>&1 && ! ip link show $(dev carol) >/dev/null 2>&1"
    check "alice TUN device up" link_up "$(dev alice)"
    A_ADDR="$(ip -4 -o addr show "$(dev alice)" | awk '{print $4}')"
    check "alice TUN address is her slot /30 ($A_ADDR)" bash -c "[[ '$A_ADDR' == 198.19.0.*/30 ]]"
    A_RULES="$(slot_rules "$(slot_of alice)")"
    check "alice owns rules in her slot range ($A_RULES)" test "$A_RULES" -gt 0
    for user in alice bob carol root; do
        check "kernel routes $user via alice's TUN" routes_via "$user" "$(dev alice)"
        check "$user traffic uses alice's TUN" uses_tun "$user" alice
    done
    check "carol proxy port works" fetch carol -x "http://127.0.0.1:$(mixed_port carol)"
    check "alice's core sees the domain of her connection" sees_domain alice
    check_unless_stub "alice DNS answers from alice's fake-IP range" fake_ip_in alice
    check_unless_stub "carol DNS is hijacked into alice's fake-IP range" fake_ip_in carol alice
    check "alice and bob fetch in parallel" parallel
    # Mihomo's resolvectl calls, allowed silently by polkit for the owner only.
    check "polkit allows alice's TUN link DNS without prompting" test "$(resolved_answer alice "$(dev alice)")" = 0
    check "polkit allows bob his own TUN link" test "$(resolved_answer bob "$(dev bob)")" = 0
    check "polkit refuses bob alice's TUN link" test "$(resolved_answer bob "$(dev alice)")" = 1
    check "polkit keeps prompting for other links" test "$(resolved_answer alice eth0)" = 2
    check "systemd-resolved routes every lookup to alice's TUN" resolver_uses_tun "$(dev alice)"
    check "root reaches resolved's fake IPs through alice's TUN" system_lookup_works root
    check "carol reaches resolved's fake IPs through alice's TUN" system_lookup_works carol
}

# cli USER ARGS...: the installed mihomo-server command, run as USER.
cli() {
    local user="$1"
    shift
    as "$user" mihomo-server "$@"
}
# fails_with PATTERN COMMAND...: COMMAND fails and explains itself with PATTERN.
fails_with() {
    local pattern="$1" output
    shift
    output="$("$@" 2>&1)" && { echo "      accepted: $output"; return 1; }
    grep -q "$pattern" <<<"$output" || { echo "      got: $output"; return 1; }
}
# exits_with STATUS COMMAND...
exits_with() {
    local want="$1"
    shift
    "$@" >/dev/null 2>&1
    [ $? = "$want" ]
}
link_down() { ! link_up "$1"; }
# selected USER GROUP: the node GROUP currently uses, as the command reports it.
selected() {
    cli "$1" --json proxy list "$2" | python3 -c 'import json,sys; print(json.load(sys.stdin)["now"])'
}

phase_cli() {
    # ---------------------------------------------------------------- mihomo-server command
    echo "== mihomo-server command"
    # shellcheck disable=SC2016 # expanded by the checking shell
    check "command, manual and completions are linked into /usr/local" bash -c \
        '[ "$(readlink /usr/local/bin/mihomo-server)" = /opt/mihomo-server/current/bin/mihomo-server ] \
            && [ -f /usr/local/share/man/man1/mihomo-server.1 ] \
            && [ -f /usr/local/share/bash-completion/completions/mihomo-server ] \
            && [ -f /usr/local/share/zsh/site-functions/_mihomo-server ]'
    check "command is on every user's PATH" as carol sh -c 'command -v mihomo-server >/dev/null'
    check "root is told it has no instance" fails_with 'root has no instance' mihomo-server status
    cli alice status >/tmp/cli-status-alice.log 2>&1
    check "alice status shows her running core" grep -Eq '^core: +running, Mihomo v' /tmp/cli-status-alice.log
    check "alice status shows her system-wide TUN" grep -q "^tun: *on (device $(dev alice), system-wide)" /tmp/cli-status-alice.log
    check "alice status names her subscription" grep -Eq '^subscription: +(real|direct)$' /tmp/cli-status-alice.log
    check "alice status shows her proxy port" grep -q "^proxy: *HTTP/SOCKS 127.0.0.1:$(mixed_port alice)" /tmp/cli-status-alice.log
    cli bob status >/tmp/cli-status-bob.log 2>&1
    check "bob status reaches his public-origin wildcard listener" grep -q '^manage: *https://bob.example$' /tmp/cli-status-bob.log
    check "bob status names alice as TUN holder" grep -q '^tun: *off (the system-wide TUN is held by alice' /tmp/cli-status-bob.log
    check "carol tun on is refused with the reason" fails_with 'TUN group' cli carol tun on
    check "token command prints the token" test "$(cli alice token)" = "$(cat "$(data_dir alice)/management-token")"
    check "logs passes journalctl options" cli alice logs -n 5 --no-pager
    cli alice core >/tmp/cli-core.log 2>&1
    check "core version" grep -q '^Mihomo v' /tmp/cli-core.log

    cli alice sub >/tmp/cli-sub.log 2>&1
    check "subscription list marks the one in use" grep -Eq '^\* +1 +local .* (real|direct)$' /tmp/cli-sub.log
    check "sub use applies the subscription" cli alice sub use 1
    check "alice TUN up after sub use" wait_for 60 link_up "$(dev alice)"

    cli alice mode global >/dev/null
    check "mode global via the command" test "$(cli alice mode)" = global
    cli alice --json status >/tmp/cli-status.json 2>&1
    check "core reports global mode" grep -q '"mode": "global"' /tmp/cli-status.json
    cli alice mode rule >/dev/null
    check "mode rule via the command" test "$(cli alice mode)" = rule
    check "unknown mode is a usage error" exits_with 2 cli alice mode tun

    local group original count
    group="$(cli alice --json proxy list | python3 -c 'import json,sys; print(next(g["name"] for g in json.load(sys.stdin) if g["type"] == "Selector" and g["name"] != "GLOBAL"))')"
    original="$(selected alice "$group")"
    count="$(cli alice --json proxy list "$group" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["nodes"]))')"
    cli alice proxy select "$group" "$count" >/dev/null
    check "proxy select by list number" test "$(selected alice "$group")" = \
        "$(cli alice --json proxy list "$group" | python3 -c 'import json,sys; print(json.load(sys.stdin)["nodes"][-1]["name"])')"
    cli alice proxy select "$group" "$original" >/dev/null
    check "proxy select by name restores the node" test "$(selected alice "$group")" = "$original"
    cli alice proxy test "$group" >/tmp/cli-delay.log 2>&1
    check "proxy test measures the group" grep -Eq '^[* ] [0-9]+ ms ' /tmp/cli-delay.log
    check "alice proxy port works after command selections" fetch alice -x "http://127.0.0.1:$(mixed_port alice)"

    cli alice tun off >/dev/null
    check "tun off via the command removes the device" wait_for 40 link_down "$(dev alice)"
    check "tun off releases the system TUN" holder_is bob ''
    cli alice tun on >/dev/null
    check "tun on via the command brings the device back" wait_for 60 link_up "$(dev alice)"
    check "alice holds the system TUN again" holder_is bob alice

    cli alice stop >/dev/null
    check "status exits 3 once stopped" exits_with 3 cli alice status
    check "stopping releases the TUN" wait_for 40 link_down "$(dev alice)"
    cli alice start >/tmp/cli-start.log 2>&1
    check "start waits for a ready instance" grep -q '^manage:' /tmp/cli-start.log
    check "status exits 0 once started" exits_with 0 cli alice status
    check "start restores the saved TUN" wait_for 60 link_up "$(dev alice)"
    check "alice traffic uses alice's TUN after command restart" uses_tun alice alice
}

phase_crash() {
    # ---------------------------------------------------------------- core crash
    echo "== core crash recovery"
    OLD_CORE="$(core_pid alice)"
    kill -KILL "$OLD_CORE"
    recovered() { local p; p="$(core_pid alice)"; [ -n "$p" ] && [ "$p" != "$OLD_CORE" ] && link_up "$(dev alice)"; }
    check "alice core restarted with TUN after SIGKILL" wait_for 60 recovered
    sleep 2
    check "alice rule count unchanged after crash ($A_RULES)" test "$(slot_rules "$(slot_of alice)")" = "$A_RULES"
    check "alice traffic still uses alice's TUN" uses_tun alice alice
    check "resolved points at alice's TUN again after crash" wait_for 20 resolver_uses_tun "$(dev alice)"
    check "carol unaffected by alice's crash" system_lookup_works carol
    check "alice keeps the system TUN through crash recovery" tun_rejected bob 'in use by alice'
}

phase_core_upgrade() {
    # ---------------------------------------------------------------- Web core upgrade
    echo "== Web core upgrade keeps TUN"
    api alice upgrade_clash_core '{"force":true}' >/tmp/core-upgrade.log 2>&1 \
        || { cat /tmp/core-upgrade.log; fatal 'alice Web core upgrade failed'; }
    check "Web upgrade installed a receipt" test -f /home/alice/.local/share/mihomo-server/core/.core-installation.json
    local installed_version
    installed_version=$(api alice installed_core_version) || fatal 'read Web-installed version'
    check "alice reports the Web-installed version" python3 -c \
        'import json,sys; sys.exit(json.load(open(sys.argv[1]))["version"] != json.loads(sys.argv[2]))' \
        /home/alice/.local/share/mihomo-server/core/.core-installation.json "$installed_version"
    check "alice core after Web upgrade is still her managed core" wait_for 60 bash -c \
        "[ \"\$(readlink /proc/\$(pgrep -u alice -x verge-mihomo | head -n 1)/exe)\" = /home/alice/.local/share/mihomo-server/core/verge-mihomo ]"
    check "alice core keeps TUN capabilities after Web upgrade" has_net_admin alice
    check "alice TUN up after Web upgrade" wait_for 60 link_up "$(dev alice)"
    check "alice traffic uses only alice's TUN after Web upgrade" uses_tun alice alice
    ALICE_CORE=$(core_hash alice)
    # An outdated stable core (as left by an older installer) is brought up to the pin.
    as carol bash -c "printf '#!/bin/sh\necho Mihomo Meta v1.0.0 linux amd64\n' >\"\$1/core/.old\" && chmod 755 \"\$1/core/.old\" && mv \"\$1/core/.old\" \"\$1/core/verge-mihomo\"" bash "$(data_dir carol)"
}

phase_upgrade() {
    # ---------------------------------------------------------------- upgrade
    echo "== upgrade restarts running instances"
    OLD_A="$(pgrep -u alice -x mihomo-server)"
    OLD_C="$(pgrep -u carol -x mihomo-server)"
    BEFORE_UPGRADE_RELEASE=$(readlink /opt/mihomo-server/current)
    echo vfixture2 >/work/releases/latest-tag
    as alice env MIHOMO_INSTALL_BASE_URL=http://127.0.0.1:18080 \
        MIHOMO_SERVER_INSTALLER=http://127.0.0.1:18080/test/repo/releases/latest/download/install.sh \
        mihomo-server update >/tmp/upgrade.log 2>&1 || { cat /tmp/upgrade.log; fatal 'mihomo-server update failed'; }
    check "current points at the new release" test "$(readlink /opt/mihomo-server/current)" = releases/vfixture2
    check "previous release kept" test -d "/opt/mihomo-server/$BEFORE_UPGRADE_RELEASE"
    restarted() { local p; p="$(pgrep -u "$1" -x mihomo-server)"; [ -n "$p" ] && [ "$p" != "$2" ]; }
    check "alice instance restarted" wait_for 40 restarted alice "$OLD_A"
    check "carol instance restarted" wait_for 40 restarted carol "$OLD_C"
    check "alice runs from the new release" wait_for 40 bash -c \
        "readlink /proc/\$(pgrep -u alice -x mihomo-server)/exe | grep -q /releases/vfixture2/"
    check "installer upgrade replaced carol's outdated core with the pin" wait_for 40 bash -c \
        "[ \"\$(sha256sum '$(data_dir carol)/core/verge-mihomo' | cut -d' ' -f1)\" = \"\$(sha256sum /opt/mihomo-server/current/resources/core/verge-mihomo | cut -d' ' -f1)\" ]"
    check "carol core running after pin upgrade" wait_for 40 bash -c "pgrep -u carol -x verge-mihomo >/dev/null"
    check "installer upgrade kept alice's Web-installed core" test "$(core_hash alice)" = "$ALICE_CORE"
    check "alice TUN back after upgrade" wait_for 60 link_up "$(dev alice)"
    check "alice holds the system TUN after upgrade" holder_is alice alice
    sleep 2
    check "alice rules not duplicated after upgrade" test "$(slot_rules "$(slot_of alice)")" = "$A_RULES"
    check "alice traffic uses alice's TUN after upgrade" uses_tun alice alice
    check "carol traffic uses alice's TUN after upgrade" uses_tun carol alice
}

# USER's service_info field (a Python expression over `info`) is true.
service_info_is() {
    api "$1" service_info | python3 -c 'import json,sys; info=json.load(sys.stdin); sys.exit(not eval(sys.argv[1]))' "$2"
}
upgrade_refused() { ! api "$1" upgrade_service >/tmp/refused-upgrade.log 2>&1; }
update_unit_succeeded() {
    [ "$(systemctl show mihomo-server-update.service -p ActiveState --value)" = inactive ] \
        && [ "$(systemctl show mihomo-server-update.service -p Result --value)" = success ]
}
current_is() { [ "$(readlink /opt/mihomo-server/current)" = "$1" ]; }
instance_active() { [ "$(systemctl --user -M "$1@" is-active mihomo-server.service)" = active ]; }
phase_web_upgrade() {
    echo "== Web service upgrade and restart"
    check "update unit installed" test -f /etc/systemd/system/mihomo-server-update.service
    check "polkit update rule installed" test -f /etc/polkit-1/rules.d/50-mihomo-server-update.rules
    check "alice's instance reports its systemd unit" service_info_is alice 'info["unit"] == "mihomo-server.service"'
    check "alice's instance offers the Web upgrade" service_info_is alice \
        'info["upgrade"]["available"] and info["release"] == "vfixture2"'
    check "carol (not in the TUN group) cannot start the update unit" upgrade_refused carol
    # The test release server, for the unit only.
    mkdir -p /etc/systemd/system/mihomo-server-update.service.d
    printf '[Service]\nEnvironment=MIHOMO_INSTALL_BASE_URL=http://127.0.0.1:18080\nEnvironment=MIHOMO_SERVER_INSTALLER=%s\n' \
        http://127.0.0.1:18080/test/repo/releases/latest/download/install.sh \
        >/etc/systemd/system/mihomo-server-update.service.d/test.conf
    systemctl daemon-reload
    local old_a old_c
    old_a="$(pgrep -u alice -x mihomo-server)"
    old_c="$(pgrep -u carol -x mihomo-server)"
    echo vfixture3 >/work/releases/latest-tag
    api alice upgrade_service >/tmp/web-upgrade.log 2>&1 || { cat /tmp/web-upgrade.log; fatal 'alice Web upgrade request failed'; }
    check "Web upgrade switched current to the latest release" wait_for 180 current_is releases/vfixture3
    check "update unit finished successfully" wait_for 60 update_unit_succeeded
    check "Web upgrade restarted alice" wait_for 60 restarted alice "$old_a"
    check "Web upgrade restarted carol" wait_for 60 restarted carol "$old_c"
    check "alice reads the new release and the update output" wait_for 60 service_info_is alice \
        'info["release"] == "vfixture3" and any("System install complete" in line for line in info["upgrade"]["log"])'
    check "update output is world-readable" test "$(stat -c '%a %U' /var/lib/mihomo-server/update.log)" = '644 root'
    rm -rf /etc/systemd/system/mihomo-server-update.service.d
    systemctl daemon-reload
    check "alice TUN back after Web upgrade" wait_for 60 link_up "$(dev alice)"
    old_c="$(pgrep -u carol -x mihomo-server)"
    api carol restart_service >/dev/null || fatal 'carol Web restart request failed'
    check "Web restart restarted carol's service" wait_for 60 restarted carol "$old_c"
    check "carol's unit is active after the Web restart" wait_for 60 instance_active carol
}

# USER's saved TUN switch is explicitly off.
tun_saved_off() {
    api "$1" settings | python3 -c 'import json,sys; sys.exit(json.load(sys.stdin)["runtime"]["tun"]["enable"] is not False)'
}
# The holder keeps the TUN until it turns it off; only root can end it for them.
phase_handover() {
    echo "== system TUN handover between users"
    api alice set_tun_enabled '{"enabled":false}' >/dev/null || fatal 'alice TUN off'
    check "alice's TUN released when she turns it off" holder_is bob ''
    api bob set_tun_enabled '{"enabled":true}' >/dev/null || fatal 'bob TUN on with his own subscription'
    check "bob TUN up" wait_for 60 link_up "$(dev bob)"
    check "root traffic uses bob's TUN" uses_tun root bob
    check "systemd-resolved now routes every lookup to bob's TUN" wait_for 20 resolver_uses_tun "$(dev bob)"
    check "root reaches resolved's fake IPs through bob's TUN" system_lookup_works root
    check "alice TUN enable is rejected while bob holds it" tun_rejected alice 'in use by bob'
    check "alice's Web view names bob as TUN holder" holder_is alice bob
    systemctl --user -M bob@ stop mihomo-server.service
    check "root stopping bob ends his TUN" wait_for 40 bash -c "! ip link show $(dev bob) >/dev/null 2>&1"
    api alice set_tun_enabled '{"enabled":true}' >/dev/null || fatal 'alice TUN on after bob stopped'
    check "alice TUN up again" wait_for 60 link_up "$(dev alice)"
    as bob mihomo-server-user restart >/tmp/bob-yield.log 2>&1 || { cat /tmp/bob-yield.log; fatal 'bob restart while alice holds TUN'; }
    check "bob starts without TUN while alice holds it" bash -c "! ip link show $(dev bob) >/dev/null 2>&1"
    check "bob's saved TUN setting was turned off" tun_saved_off bob
    check "bob traffic uses alice's TUN again" uses_tun bob alice
}

phase_cleanup() {
    # ---------------------------------------------------------------- disable + cleanup
    echo "== disable and uninstall cleanup"
    as alice mihomo-server-user disable >/dev/null
    check "alice TUN device removed on stop" wait_for 40 bash -c "! ip link show $(dev alice) >/dev/null 2>&1"
    check "alice slot rules removed on stop" test "$(slot_rules "$(slot_of alice)")" = 0
    check "alice now routes directly" routes_direct alice
    check "bob routes directly once alice stops" routes_direct bob
    check "resolved answers real IPs once alice stops" bash -c \
        "! resolvectl query --legend=no -4 --cache=no www.gstatic.com | grep -q ' 198\.19\.'"
    check "alice keeps her slot after disable" test -n "$(slot_of alice)"

    # The bundled installer: no download, run by the installing user via sudo.
    cli alice uninstall --yes >/tmp/uninstall.log 2>&1 || { cat /tmp/uninstall.log; fatal "uninstall failed"; }
    check "no instances left" wait_for 40 bash -c "! pgrep -x mihomo-server >/dev/null && ! pgrep verge-mihomo >/dev/null"
    check "all TUN devices removed" bash -c "! ip -o link | grep -q ': ms[0-9]'"
    check "only default policy rules remain" only_default_rules
    check "bundle, unit and helper removed" bash -c \
        "[ ! -e /opt/mihomo-server ] && [ ! -e /etc/systemd/user/mihomo-server.service ] && [ ! -e /usr/local/bin/mihomo-server-user ]"
    check "command, manual and completion links removed" bash -c \
        '! compgen -G "/usr/local/bin/mihomo-server*" >/dev/null && [ ! -L /usr/local/share/man/man1/mihomo-server.1 ] \
            && [ ! -L /usr/local/share/bash-completion/completions/mihomo-server ] && [ ! -L /usr/local/share/zsh/site-functions/_mihomo-server ]'
    check "users' data kept" bash -c "[ -s /home/alice/.local/share/mihomo-server/management-token ] && [ -s '/home/bob/private data%/mihomo-server/management-token' ]"
    check "TUN group, slot registry and launcher removed" bash -c \
        "! getent group mihomo-tun >/dev/null && [ ! -e /var/lib/mihomo-server ]"
    check "polkit TUN DNS rule removed" test ! -e /etc/polkit-1/rules.d/50-mihomo-server-tun.rules
    check "update unit and its polkit rule removed" bash -c \
        "[ ! -e /etc/systemd/system/mihomo-server-update.service ] && [ ! -e /etc/polkit-1/rules.d/50-mihomo-server-update.rules ]"
    check "installer-enabled lingering undone" test "$(loginctl show-user alice -p Linger --value 2>/dev/null || echo no)" = no
    check "no enablement links left" bash -c "! compgen -G '/home/*/.config/systemd/user/default.target.wants/mihomo-server.service' >/dev/null"

    echo "== reinstall from the kept data, then purge everything"
    as alice env MIHOMO_INSTALL_BUNDLE=/work/bundle bash /work/install.sh >/tmp/reinstall.log 2>&1 \
        || { cat /tmp/reinstall.log; fatal 'reinstall failed'; }
    check "reinstall reuses alice's token" test "$(sha256sum /home/alice/.local/share/mihomo-server/management-token | cut -d' ' -f1)" = "$ALICE_TOKEN_HASH"
    bash /work/install.sh --uninstall --purge >/tmp/purge.log 2>&1 || { cat /tmp/purge.log; fatal "purge failed"; }
    check "purge removed default, custom-XDG and legacy data" bash -c \
        "[ ! -e /home/alice/.local/share/mihomo-server ] && [ ! -e '/home/bob/private data%/mihomo-server' ] && [ ! -e /home/dave/legacy-data ]"
    check "purge removed user configuration and drop-ins" bash -c \
        "[ ! -e /home/alice/.config/mihomo-server ] && [ ! -e '/home/bob/config space%/mihomo-server' ] && ! compgen -G '/home/[abdf]*/.config/systemd/user/mihomo-server.service*' >/dev/null"
    # eve's custom unit was never migrated, so it is not an instance to purge.
    check "purge left eve's unmanaged custom unit alone" test -f /home/eve/.config/systemd/user/mihomo-server.service
    check "purge removed migrated per-user programs" test ! -e /home/dave/.local/opt/mihomo-server
    check "nothing of mihomo-server left on the system" bash -c \
        "[ ! -e /opt/mihomo-server ] && [ ! -e /var/lib/mihomo-server ] && [ ! -e /etc/systemd/user/mihomo-server.service ] && ! pgrep -x mihomo-server >/dev/null && ! pgrep -x verge-mihomo >/dev/null"
}

case "${1:-before-reboot}" in
    before-reboot)
        phase_install
        phase_init
        phase_port_conflict
        phase_migration
        tun_enable
        tun_assert
        phase_cli
        phase_crash
        phase_core_upgrade
        phase_upgrade
        phase_web_upgrade
        declare -p PASS FAIL A_RULES ALICE_TOKEN_HASH >/work/checkpoint
        chmod 600 /work/checkpoint
        ;;
    after-reboot)
        # Root-owned test checkpoint; no subscription or token contents.
        # shellcheck disable=SC1091
        source /work/checkpoint
        alice_running() { api alice status | python3 -c 'import json,sys; sys.exit(json.load(sys.stdin)["phase"] != "running")'; }
        check "alice instance restored at boot without enable command" wait_for 120 alice_running
        check "alice token retained across reboot" test "$(sha256sum /home/alice/.local/share/mihomo-server/management-token | cut -d' ' -f1)" = "$ALICE_TOKEN_HASH"
        check "non-lingering users not started at boot" bash -c '! pgrep -u bob -x mihomo-server && ! pgrep -u carol -x mihomo-server'
        systemctl start "user@$(id -u bob).service" "user@$(id -u carol).service" || fatal 'start opt-in managers'
        as bob sleep infinity &
        as carol sleep infinity &
        check "bob instance restored with saved XDG paths" wait_for 120 bash -c "ss -ltn | grep -q ':$(port bob) '"
        check "alice TUN works after reboot" uses_tun alice alice
        check "bob traffic uses alice's TUN after reboot" uses_tun bob alice
        check "resolved points at alice's TUN after reboot" wait_for 20 resolver_uses_tun "$(dev alice)"
        phase_handover
        phase_cleanup
        echo "multi-user e2e: $PASS passed, $FAIL failed"
        [ "$FAIL" -eq 0 ]
        ;;
    *) fatal 'unknown phase' ;;
esac
