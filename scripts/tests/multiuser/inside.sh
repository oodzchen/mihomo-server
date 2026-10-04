#!/usr/bin/env bash
# Multi-user end-to-end checks, run as root inside the test container by run.sh.
#
# /work holds: install.sh, bundle/ (extracted release bundle) and an optional
# profile.yaml (real subscription). Users: alice (installer: system-wide TUN),
# bob (TUN group), carol (not). A root-only reinstall covers per-user TUNs.
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
# Mihomo's resolvectl calls must not make systemd-resolved route lookups to a per-user TUN.
resolver_ignores_tun() { ! resolvectl domain "$1" | grep -q '~\.' && [ -z "$(resolvectl dns "$1" | cut -d: -f2 | tr -d ' ')" ]; }
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
    check "installer made alice the system TUN owner" bash -c \
        "grep -q 'tun:.*available system-wide' /tmp/install.log && [ \"\$(cat /var/lib/mihomo-server/tun-owner)\" = $(id -u alice) ]"
    check "tun-owner file is root 0644" test "$(stat -c '%a %U' /var/lib/mihomo-server/tun-owner)" = '644 root'
    check "polkit rule names alice as owner" grep -q "subject.user === \"alice\"" /etc/polkit-1/rules.d/50-mihomo-server-tun.rules
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
    check "bob told alice owns the system-wide TUN" grep -q "tun:.*unavailable (system-wide TUN belongs to uid $(id -u alice))" /tmp/init-bob.log
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
    echo "== system-wide TUN for alice (installer)"
    for user in alice bob carol; do
        profile_import "$user" || fatal "$user profile import failed"
    done
    api alice set_settings "$TUN_ON" >/dev/null || fatal "alice TUN enable failed"
    check "bob TUN enable is rejected while alice owns TUN" tun_rejected bob 'system-wide TUN'
    check "carol TUN enable is rejected" tun_rejected carol 'system-wide TUN\|TUN group'
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
    check "polkit refuses other TUN links without prompting" test "$(resolved_answer bob "$(dev bob)")" = 1
    check "polkit keeps prompting for other links" test "$(resolved_answer alice eth0)" = 2
    check "systemd-resolved routes every lookup to alice's TUN" resolver_uses_tun "$(dev alice)"
    check "root reaches resolved's fake IPs through alice's TUN" system_lookup_works root
    check "carol reaches resolved's fake IPs through alice's TUN" system_lookup_works carol
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
    as alice env MIHOMO_INSTALL_BASE_URL=http://127.0.0.1:18080 bash -c \
        'set -o pipefail; curl -fsSL http://127.0.0.1:18080/test/repo/releases/latest/download/install.sh | bash' \
        >/tmp/upgrade.log 2>&1 || { cat /tmp/upgrade.log; fatal 'zero-argument upgrade failed'; }
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
    check "upgrade keeps alice as system TUN owner" test "$(cat /var/lib/mihomo-server/tun-owner)" = "$(id -u alice)"
    sleep 2
    check "alice rules not duplicated after upgrade" test "$(slot_rules "$(slot_of alice)")" = "$A_RULES"
    check "alice traffic uses alice's TUN after upgrade" uses_tun alice alice
    check "carol traffic uses alice's TUN after upgrade" uses_tun carol alice
}

# A root-only installation has no owner: each TUN carries only its user's traffic
# and polkit silently keeps every TUN away from the shared resolver.
phase_isolated() {
    echo "== root-only install: concurrent per-user TUN"
    env MIHOMO_INSTALL_BUNDLE=/work/bundle bash /work/install.sh >/tmp/root-install.log 2>&1 \
        || { cat /tmp/root-install.log; fatal 'root-only install failed'; }
    check "root-only install has no TUN owner" test ! -e /var/lib/mihomo-server/tun-owner
    usermod -aG mihomo-tun alice
    usermod -aG mihomo-tun bob
    as alice mihomo-server-user enable >/tmp/iso-alice.log 2>&1 || { cat /tmp/iso-alice.log; fatal 'alice enable'; }
    as bob mihomo-server-user enable >/tmp/iso-bob.log 2>&1 || { cat /tmp/iso-bob.log; fatal 'bob enable'; }
    check "per-user TUN available to alice and bob" bash -c \
        "grep -q 'tun:.*available (device' /tmp/iso-alice.log && grep -q 'tun:.*available (device' /tmp/iso-bob.log"
    for user in alice bob; do
        api "$user" set_settings "$TUN_ON" >/dev/null || fatal "$user per-user TUN enable failed"
    done
    check "alice TUN up" wait_for 60 link_up "$(dev alice)"
    check "bob TUN up" wait_for 60 link_up "$(dev bob)"
    check "alice traffic uses only alice's TUN" uses_tun alice alice
    check "bob traffic uses only bob's TUN" uses_tun bob bob
    check "root traffic uses no TUN" uses_tun root none
    check "polkit refuses per-user TUN link DNS without prompting" test "$(resolved_answer alice "$(dev alice)")" = 1
    check "shared resolver ignores alice's TUN" resolver_ignores_tun "$(dev alice)"
    check "shared resolver ignores bob's TUN" resolver_ignores_tun "$(dev bob)"
    bash /work/install.sh --uninstall >/tmp/uninstall2.log 2>&1 || { cat /tmp/uninstall2.log; fatal "second uninstall failed"; }
    check "per-user TUN devices removed" wait_for 40 bash -c "! ip -o link | grep -q ': ms[0-9]'"
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

    bash /work/install.sh --uninstall >/tmp/uninstall.log 2>&1 || { cat /tmp/uninstall.log; fatal "uninstall failed"; }
    check "no instances left" wait_for 40 bash -c "! pgrep -x mihomo-server >/dev/null && ! pgrep verge-mihomo >/dev/null"
    check "all TUN devices removed" bash -c "! ip -o link | grep -q ': ms[0-9]'"
    check "only default policy rules remain" only_default_rules
    check "bundle, unit and helper removed" bash -c \
        "[ ! -e /opt/mihomo-server ] && [ ! -e /etc/systemd/user/mihomo-server.service ] && [ ! -e /usr/local/bin/mihomo-server-user ]"
    check "users' data kept" bash -c "[ -s /home/alice/.local/share/mihomo-server/management-token ] && [ -s '/home/bob/private data%/mihomo-server/management-token' ]"
    check "TUN group, slot registry and launcher removed" bash -c \
        "! getent group mihomo-tun >/dev/null && [ ! -e /var/lib/mihomo-server ]"
    check "polkit TUN DNS rule removed" test ! -e /etc/polkit-1/rules.d/50-mihomo-server-tun.rules
    check "installer-enabled lingering undone" test "$(loginctl show-user alice -p Linger --value 2>/dev/null || echo no)" = no
    check "no enablement links left" bash -c "! compgen -G '/home/*/.config/systemd/user/default.target.wants/mihomo-server.service' >/dev/null"

    phase_isolated

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
        phase_crash
        phase_core_upgrade
        phase_upgrade
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
        phase_cleanup
        echo "multi-user e2e: $PASS passed, $FAIL failed"
        [ "$FAIL" -eq 0 ]
        ;;
    *) fatal 'unknown phase' ;;
esac
