#!/usr/bin/env bash
# ==============================================================================
# run_autonomous_codex.sh
#
# 无人值守自动化运行 Codex 脚本
# 适用项目: mihomo-server (基于 ./headless.md 和 ./docs/ARCHITECTURE.md)
#
# 核心特性:
# 1. 沙箱模式运行 (--sandbox workspace-write)，允许联网，自动批准无需人工干预 (-c approval_policy="never")
# 2. 终端实时输出 (流式日志 + 终端实时显示)，同时归档到日志目录
# 3. 允许人工随时安全中断 (Ctrl+C 优雅终止所有子进程)
# 4. 监测每个 request 的活动状态：超过 10 分钟无响应/无操作时，强制中断并自动重试
# 5. 监测 5 小时 Limit 问题：匹配错误码/报错特征，触发后自动进行 5 小时倒计时后重新拉起
# 6. 自动提示并监控每个小任务完成后同步进度至 docs/ARCHITECTURE.md
# 7. 识别整个计划完全实现的标志字符串 (===ALL_TASKS_COMPLETED_SUCCESSFULLY===) 并在全部达成后自动停机
# ==============================================================================

set -uo pipefail

# ------------------------------------------------------------------------------
# 1. 基础配置与环境变量
# ------------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR"
cd "$PROJECT_ROOT" || exit 1

# Codex 可执行文件路径探测
CODEX_BIN="${CODEX_BIN:-$(command -v codex || echo "/home/kholin/.nvm/versions/node/v24.18.0/bin/codex")}"
if [ ! -x "$CODEX_BIN" ]; then
    echo "[ERROR] 未找到有效的 codex 可执行文件: $CODEX_BIN" >&2
    exit 1
fi

# 参数与超限控制 (可通过环境变量自定义)
INACTIVITY_TIMEOUT="${INACTIVITY_TIMEOUT:-600}"          # 请求无响应超时时间 (秒, 默认 10 分钟)
RATE_LIMIT_COOLDOWN="${RATE_LIMIT_COOLDOWN:-18000}"       # 5 小时限额冷却时间 (秒, 默认 5*3600 = 18000 秒)
MAX_TIMEOUT_RETRIES="${MAX_TIMEOUT_RETRIES:-5}"           # 单个小任务连续超时重试最大次数
SANDBOX_MODE="${SANDBOX_MODE:-workspace-write}"          # 沙箱模式: workspace-write (支持文件读写与网络)
APPROVAL_POLICY="${APPROVAL_POLICY:-never}"              # 审批模式: never (完全无人值守自动执行)
COMPLETION_FLAG="${COMPLETION_FLAG:-===ALL_TASKS_COMPLETED_SUCCESSFULLY===}" # 全部任务完成标志

# 运行日志与状态持久化
LOG_DIR="${SCRIPT_DIR}/.codex_autonomous_logs"
mkdir -p "$LOG_DIR"
SESSION_FILE="${SCRIPT_DIR}/.codex_autonomous_session_id"

# 文档与上游代码路径
DOC_HEADLESS="${PROJECT_ROOT}/headless.md"
DOC_ARCH="${PROJECT_ROOT}/docs/ARCHITECTURE.md"
UPSTREAM_DIR="${UPSTREAM_DIR:-$(cd "${PROJECT_ROOT}/../clash-verge-rev" 2>/dev/null && pwd || echo "${PROJECT_ROOT}/../clash-verge-rev")}"

# 终端输出着色
CLR_RESET="\033[0m"
CLR_RED="\033[1;31m"
CLR_GREEN="\033[1;32m"
CLR_YELLOW="\033[1;33m"
CLR_BLUE="\033[1;34m"
CLR_MAGENTA="\033[1;35m"
CLR_CYAN="\033[1;36m"
CLR_BOLD="\033[1m"
CLR_DIM="\033[2m"

# ------------------------------------------------------------------------------
# 2. 日志打印辅助函数
# ------------------------------------------------------------------------------
log_info()    { echo -e "${CLR_CYAN}[$(date '+%Y-%m-%d %H:%M:%S')] [INFO]${CLR_RESET} $*"; }
log_success() { echo -e "${CLR_GREEN}[$(date '+%Y-%m-%d %H:%M:%S')] [SUCCESS]${CLR_RESET} $*"; }
log_warn()    { echo -e "${CLR_YELLOW}[$(date '+%Y-%m-%d %H:%M:%S')] [WARN]${CLR_RESET} $*"; }
log_error()   { echo -e "${CLR_RED}[$(date '+%Y-%m-%d %H:%M:%S')] [ERROR]${CLR_RESET} $*"; }

log_box() {
    local text="$1"
    local color="${2:-$CLR_BLUE}"
    local line="================================================================================"
    echo -e "${color}${line}"
    echo -e "  $text"
    echo -e "${line}${CLR_RESET}"
}

# ------------------------------------------------------------------------------
# 3. 进程树管理与信号捕获 (支持人工手动中断 Ctrl+C)
# ------------------------------------------------------------------------------
CURRENT_CODEX_PID=""
CURRENT_WATCHDOG_PID=""
CURRENT_TEE_PID=""
CURRENT_FIFO=""
CURRENT_TURN_LOG=""
INTERRUPTING=0

kill_tree() {
    local parent_pid="$1"
    local sig="${2:-TERM}"
    [ -z "$parent_pid" ] && return 0
    if ! kill -0 "$parent_pid" 2>/dev/null; then
        return 0
    fi

    # 递归查找子进程
    local child_pids
    child_pids=$(pgrep -P "$parent_pid" 2>/dev/null || true)
    for child in $child_pids; do
        kill_tree "$child" "$sig"
    done
    kill -s "$sig" "$parent_pid" 2>/dev/null || true
}

cleanup_turn_resources() {
    # 停止看门狗
    if [ -n "$CURRENT_WATCHDOG_PID" ] && kill -0 "$CURRENT_WATCHDOG_PID" 2>/dev/null; then
        kill -9 "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
        wait "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
    fi
    CURRENT_WATCHDOG_PID=""

    # 清理管道
    if [ -n "$CURRENT_FIFO" ] && [ -p "$CURRENT_FIFO" ]; then
        rm -f "$CURRENT_FIFO" 2>/dev/null || true
    fi
    CURRENT_FIFO=""
}

handle_manual_interrupt() {
    [ "$INTERRUPTING" -eq 1 ] && return 0
    INTERRUPTING=1
    trap '' SIGINT SIGTERM SIGHUP

    echo ""
    log_warn "检测到人工手动中断信号 (Ctrl+C / SIGINT)！"
    log_info "正在通知 Codex 优雅中断并等待其输出退出状态..."

    # 1. 优先向 Codex 主进程发送 SIGINT，让其触发原生 turn interrupted 退出并保存状态
    if [ -n "$CURRENT_CODEX_PID" ] && kill -0 "$CURRENT_CODEX_PID" 2>/dev/null; then
        kill -s INT "$CURRENT_CODEX_PID" 2>/dev/null || true
        # 等待至多 3 秒供 Codex 打印退出消息并关闭管道
        local count=0
        while kill -0 "$CURRENT_CODEX_PID" 2>/dev/null && [ "$count" -lt 6 ]; do
            sleep 0.5
            count=$(( count + 1 ))
        done
        # 若仍未退出，发送 TERM
        if kill -0 "$CURRENT_CODEX_PID" 2>/dev/null; then
            kill_tree "$CURRENT_CODEX_PID" TERM
            sleep 1
        fi
        # 若仍未退出，执行强杀 KILL
        if kill -0 "$CURRENT_CODEX_PID" 2>/dev/null; then
            log_warn "子进程未及时响应，执行强制终止 (KILL)..."
            kill_tree "$CURRENT_CODEX_PID" KILL
        fi
    fi
    CURRENT_CODEX_PID=""

    # 2. 等待流渲染格式化脚本读取完 FIFO 管道中剩余的退出输出与 session id
    if [ -n "$CURRENT_TEE_PID" ] && kill -0 "$CURRENT_TEE_PID" 2>/dev/null; then
        local t_count=0
        while kill -0 "$CURRENT_TEE_PID" 2>/dev/null && [ "$t_count" -lt 6 ]; do
            sleep 0.5
            t_count=$(( t_count + 1 ))
        done
        kill -9 "$CURRENT_TEE_PID" 2>/dev/null || true
        wait "$CURRENT_TEE_PID" 2>/dev/null || true
    fi
    CURRENT_TEE_PID=""

    cleanup_turn_resources

    # 3. 提取最新的 Session ID 并进行高亮呈现
    local sid=""
    if [ -n "$CURRENT_TURN_LOG" ] && [ -f "$CURRENT_TURN_LOG" ]; then
        sid=$(grep -oE "session id:\s*[0-9a-fA-F-]+" "$CURRENT_TURN_LOG" | tail -n 1 | awk '{print $NF}')
    fi
    if [ -z "$sid" ] && [ -f "$SESSION_FILE" ] && [ -s "$SESSION_FILE" ]; then
        sid=$(cat "$SESSION_FILE" | tr -d '[:space:]')
    fi
    if [ -z "$sid" ]; then
        sid=$(get_latest_session_id)
    fi

    echo ""
    if [ -n "$sid" ]; then
        echo "$sid" > "$SESSION_FILE"
        log_box " Codex 会话已安全中断 (已保存 Session ID) " "$CLR_YELLOW"
        echo -e "${CLR_BOLD}session id: ${CLR_CYAN}${sid}${CLR_RESET}"
        echo -e "${CLR_DIM}--------------------------------------------------------------------------------${CLR_RESET}"
        echo -e "若需接着此会话继续工作，请执行以下命令："
        echo -e "  ${CLR_GREEN}▶ 1. 继续无人值守自动运行:${CLR_RESET}"
        echo -e "     ${CLR_BOLD}./run_autonomous_codex.sh --session ${sid}${CLR_RESET}"
        echo ""
        echo -e "  ${CLR_BLUE}▶ 2. 进入交互式 Codex 终端手动调试:${CLR_RESET}"
        echo -e "     ${CLR_BOLD}codex resume ${sid}${CLR_RESET}"
        echo ""
        echo -e "  ${CLR_MAGENTA}▶ 3. 非交互式单次指令追加:${CLR_RESET}"
        echo -e "     ${CLR_BOLD}codex exec resume ${sid} \"你的指令\"${CLR_RESET}"
        echo -e "${CLR_YELLOW}================================================================================${CLR_RESET}"
    else
        log_box "已成功安全中断并退出无人值守工作流程。" "$CLR_YELLOW"
    fi

    exit 130
}

trap handle_manual_interrupt SIGINT SIGTERM SIGHUP

# ------------------------------------------------------------------------------
# 4. 辅助检测工具函数
# ------------------------------------------------------------------------------
# 计算 ARCHITECTURE.md 的哈希值以监测是否已同步更新
get_arch_hash() {
    if [ -f "$DOC_ARCH" ]; then
        md5sum "$DOC_ARCH" | awk '{print $1}'
    else
        echo "none"
    fi
}

# 提取最新的会话 ID
get_latest_session_id() {
    local index_file="${HOME}/.codex/session_index.jsonl"
    if [ -f "$index_file" ]; then
        grep -o '"id":"[^"]*"' "$index_file" | tail -n 1 | cut -d'"' -f4
    fi
}

# 监测 5 小时 Limit 限制与常见 Rate Limit 错误
check_rate_limit() {
    local exit_code="$1"
    local log_file="$2"
    local last_msg_file="$3"

    # 如果轮次正常成功退出 (exit code 0) 且生成了非空的最终答复，且没有致命限额报错，绝不判定为限额中断
    if [ "$exit_code" -eq 0 ] && [ -f "$last_msg_file" ] && [ -s "$last_msg_file" ]; then
        if ! tail -n 50 "$log_file" 2>/dev/null | grep -i -E "(hit your usage limit|usage_limit_reached|rate_limit_exceeded|429 [tT]oo [mM]any [rR]equests)" >/dev/null 2>&1; then
            return 1
        fi
    fi

    local patterns="(usage_limit_reached|rate_limit_exceeded|rate limit exceeded|hit your usage limit|usage limit reached|429 [tT]oo [mM]any [rR]equests|insufficient_quota|reset_after_seconds|\btry again at\s+[0-9]|\btry again in\s+[0-9]|credits? depleted|已达到用量限制|已达到配额上限)"
    
    # 仅检测末尾 100 行报错日志，避免匹配历史或正文讨论中的引用 (如'代码失败后重试')
    if [ -f "$log_file" ] && tail -n 100 "$log_file" 2>/dev/null | grep -i -E "$patterns" >/dev/null 2>&1; then
        return 0
    fi
    if [ -f "$last_msg_file" ] && grep -i -E "$patterns" "$last_msg_file" >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

# 智能解析下一次限额重置时间点 (结合 App-Server /status 接口与官方报错文本正则匹配)
get_rate_limit_info() {
    local log_file="$1"
    local last_msg_file="$2"
    local default_cooldown="${3:-18000}"

    python3 - "$log_file" "$last_msg_file" "$default_cooldown" << 'PYEOF'
import sys, socket, struct, json, os, time, re, datetime

log_file = sys.argv[1] if len(sys.argv) > 1 else ""
last_msg_file = sys.argv[2] if len(sys.argv) > 2 else ""
default_cooldown = int(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3].isdigit() else 18000

now = datetime.datetime.now()
now_ts = int(now.timestamp())

# --- 1. Query App Server Socket (对应内部 /status 的数据源) ---
def query_app_server():
    sock_path = os.path.expanduser("~/.codex/app-server-control/app-server-control.sock")
    if not os.path.exists(sock_path):
        return None
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(2.0)
        s.connect(sock_path)
        s.sendall(b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n")
        resp = s.recv(4096)
        if b"101 Switching Protocols" not in resp:
            s.close()
            return None

        def make_frame(payload):
            length = len(payload)
            mask = os.urandom(4)
            if length < 126:
                h = bytes([0x81, 0x80 | length]) + mask
            elif length < 65536:
                h = bytes([0x81, 0x80 | 126]) + struct.pack(">H", length) + mask
            else:
                h = bytes([0x81, 0x80 | 127]) + struct.pack(">Q", length) + mask
            return h + bytes([b ^ mask[i % 4] for i, b in enumerate(payload)])

        def recv_frame():
            head = s.recv(2)
            if not head:
                return None
            b1, b2 = head[0], head[1]
            masked = bool(b2 & 0x80)
            length = b2 & 0x7F
            if length == 126:
                length = struct.unpack(">H", s.recv(2))[0]
            elif length == 127:
                length = struct.unpack(">Q", s.recv(8))[0]
            mask = s.recv(4) if masked else b""
            data = b""
            while len(data) < length:
                chunk = s.recv(length - len(data))
                if not chunk:
                    break
                data += chunk
            if masked:
                data = bytes([b ^ mask[i % 4] for i, b in enumerate(data)])
            return data

        init_req = {"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "rate-limit-helper", "version": "1.0.0"}, "capabilities": {"experimentalApi": True}}}
        s.sendall(make_frame(json.dumps(init_req).encode()))

        req = {"id": 2, "method": "account/rateLimits/read", "params": None}
        s.sendall(make_frame(json.dumps(req).encode()))

        for _ in range(15):
            f = recv_frame()
            if not f:
                break
            msg = json.loads(f.decode("utf-8", errors="replace"))
            if msg.get("id") == 2:
                s.close()
                return msg.get("result")
        s.close()
    except Exception:
        pass
    return None

# --- 2. 报错文本正则解析器 (必须严格伴随限额关键字) ---
def parse_text(text):
    if not text:
        return None
    # 严格检验是否包含真实限额语义
    if not re.search(r"(?:hit your usage limit|usage_limit_reached|rate_limit_exceeded|rate limit exceeded|usage limit reached|429 [tT]oo [mM]any [rR]equests|insufficient_quota|credits? depleted|已达到用量限制|已达到配额上限)", text, re.I):
        return None

    # 2.1 reset_after_seconds
    m_sec = re.search(r"reset_after(?:_seconds)?[\s:=]+(\d+)", text, re.I)
    if m_sec:
        wait = int(m_sec.group(1)) + 60
        target = now + datetime.timedelta(seconds=wait)
        return wait, target.strftime("%Y-%m-%d %H:%M:%S"), f"提取自 API reset_after_seconds ({m_sec.group(1)}s)"

    # 2.2 Date + Time: "try again at Sep 27, 2026 6:12 AM"
    m_dt = re.search(r"(?:try again at|resets? at|available at)\s+([A-Za-z]{3,9}\s+\d{1,2},\s+\d{4})\s+(\d{1,2}:\d{2}(?::\d{2})?\s*(?:[AP]M)?)", text, re.I)
    if m_dt:
        date_str = m_dt.group(1).strip()
        time_str = m_dt.group(2).strip()
        full_str = f"{date_str} {time_str}"
        for fmt in ("%b %d, %Y %I:%M %p", "%b %d, %Y %I:%M:%S %p", "%B %d, %Y %I:%M %p", "%b %d, %Y %H:%M", "%b %d, %Y %H:%M:%S"):
            try:
                target = datetime.datetime.strptime(full_str, fmt)
                wait = int((target - now).total_seconds()) + 60
                return max(wait, 10), target.strftime("%Y-%m-%d %H:%M:%S"), f"提取自报错绝对时间: {full_str}"
            except ValueError:
                pass

    # 2.3 Time only: "try again at 6:12 AM" / "resets at 18:30"
    m_t = re.search(r"(?:try again at|resets? at|available at)\s+(\d{1,2}:\d{2}(?::\d{2})?\s*(?:[AP]M)?)", text, re.I)
    if m_t:
        time_str = m_t.group(1).strip()
        for fmt in ("%I:%M %p", "%I:%M:%S %p", "%H:%M", "%H:%M:%S"):
            try:
                t = datetime.datetime.strptime(time_str, fmt).time()
                target = datetime.datetime.combine(now.date(), t)
                if target <= now - datetime.timedelta(minutes=1):
                    target += datetime.timedelta(days=1)
                wait = int((target - now).total_seconds()) + 60
                return max(wait, 10), target.strftime("%Y-%m-%d %H:%M:%S"), f"提取自报错可用时间点: {time_str}"
            except ValueError:
                pass

    # 2.4 Chinese format: "请于 6:12 重试" / "可在 6:12 后重试"
    m_cn = re.search(r"(?:请于|可在|在|重试时间[：:]\s*)\s*(\d{1,2}:\d{2}(?::\d{2})?)\s*(?:[上下]午)?\s*(?:后)?(?:可)?重试", text)
    if m_cn:
        time_str = m_cn.group(1).strip()
        for fmt in ("%H:%M", "%H:%M:%S", "%I:%M"):
            try:
                t = datetime.datetime.strptime(time_str, fmt).time()
                target = datetime.datetime.combine(now.date(), t)
                if "下午" in m_cn.group(0) and t.hour < 12:
                    target += datetime.timedelta(hours=12)
                if target <= now - datetime.timedelta(minutes=1):
                    target += datetime.timedelta(days=1)
                wait = int((target - now).total_seconds()) + 60
                return max(wait, 10), target.strftime("%Y-%m-%d %H:%M:%S"), f"提取自中文报错时间: {time_str}"
            except ValueError:
                pass

    # 2.5 Relative format: "try again in 2 hours"
    m_rel = re.search(r"(?:try again in|resets? in)\s*(\d+(?:\.\d+)?)\s*(s|sec|seconds?|m|min|minutes?|h|hr|hours?)", text, re.I)
    if m_rel:
        val = float(m_rel.group(1))
        unit = m_rel.group(2).lower()
        if unit.startswith("s"):
            sec = int(val)
        elif unit.startswith("m"):
            sec = int(val * 60)
        elif unit.startswith("h"):
            sec = int(val * 3600)
        else:
            sec = int(val)
        wait = sec + 60
        target = now + datetime.timedelta(seconds=wait)
        return max(wait, 10), target.strftime("%Y-%m-%d %H:%M:%S"), f"提取自相对时间: {m_rel.group(0)}"

    return None

# Read corpus from the current turn (last 200 lines)
text_corpus = ""
for fpath in (last_msg_file, log_file):
    if fpath and os.path.exists(fpath):
        try:
            with open(fpath, "r", encoding="utf-8", errors="replace") as f:
                text_corpus += "".join(f.readlines()[-200:]) + "\n"
        except Exception:
            pass

text_res = parse_text(text_corpus)

# Query app-server (like /status)
app_res = query_app_server()
is_app_limited = False
app_target_ts = None
app_desc = None
p_used = 0
if app_res:
    rl = app_res.get("rateLimits", {})
    primary = rl.get("primary") or {}
    secondary = rl.get("secondary") or {}
    p_resets = primary.get("resetsAt")
    p_used = primary.get("usedPercent", 0)
    s_resets = secondary.get("resetsAt")
    s_used = secondary.get("usedPercent", 0)
    ord_allowed = app_res.get("ordinaryUsageAllowed", True)
    reached_type = rl.get("rateLimitReachedType")

    # 仅当普通用量明确禁止、到达上限类型或使用率>=100%时，才属于 app-server 限额状态
    is_app_limited = (
        ord_allowed is False
        or reached_type is not None
        or p_used >= 100
        or s_used >= 100
    )
    if is_app_limited:
        if p_used >= 100 and p_resets and p_resets > now_ts:
            app_target_ts = p_resets
            app_desc = f"Codex app-server (/status) 5小时滑动窗口重置点 (已用 {p_used}%)"
        elif s_used >= 100 and s_resets and s_resets > now_ts:
            app_target_ts = s_resets
            app_desc = f"Codex app-server (/status) 周限额重置点 (已用 {s_used}%)"
        elif p_resets and p_resets > now_ts:
            app_target_ts = p_resets
            app_desc = f"Codex app-server (/status) 限额重置点 (已用 {p_used}%)"
        elif s_resets and s_resets > now_ts:
            app_target_ts = s_resets
            app_desc = f"Codex app-server (/status) 周限额重置点 (已用 {s_used}%)"

# --- 最终判定决策 ---
# 1. 报错文本中明确包含限额提示和重试时间
if text_res:
    wait_sec, target_str, desc = text_res
    source = "error_log"
    is_limited = 1
# 2. App-Server 明确确认达到上限并提供了重置时间
elif is_app_limited and app_target_ts and app_target_ts > now_ts:
    wait_sec = (app_target_ts - now_ts) + 60
    target_dt = datetime.datetime.fromtimestamp(app_target_ts)
    target_str = target_dt.strftime("%Y-%m-%d %H:%M:%S")
    desc = app_desc
    source = "app_server"
    is_limited = 1
# 3. App-Server 确认到达上限，但未返回未来时间戳
elif is_app_limited:
    wait_sec = default_cooldown
    target_dt = now + datetime.timedelta(seconds=default_cooldown)
    target_str = target_dt.strftime("%Y-%m-%d %H:%M:%S")
    desc = f"检测到限额状态，但未解析到明确时间点，采用默认 {default_cooldown // 3600} 小时冷却"
    source = "default_fallback"
    is_limited = 1
# 4. 配额充足，未达到上限
else:
    rem = 100 - p_used if p_used <= 100 else 0
    wait_sec = 0
    target_str = ""
    desc = f"未触发限额限制 (当前配额充足，5小时窗口已用: {p_used}%, 剩余余量: {rem}%)"
    source = "none"
    is_limited = 0

print(f"IS_RATE_LIMITED={is_limited}")
print(f"WAIT_SECONDS={wait_sec}")
print(f"RESET_TIME=\"{target_str}\"")
print(f"RESET_SOURCE=\"{source}\"")
print(f"RESET_DETAIL=\"{desc}\"")
PYEOF
}

# 监测是否遇到会话锁定冲突 (already has an active writer)
check_active_writer_conflict() {
    local log_file="$1"
    if [ -f "$log_file" ] && grep -q "already has an active writer" "$log_file"; then
        return 0
    fi
    return 1
}

# 监测系统中是否存在除当前脚本和后台 daemon 外的前台交互式 Codex CLI 进程
has_foreground_codex_cli() {
    local my_pid=$$
    local pids
    pids=$(pgrep -f "codex" 2>/dev/null || true)
    for p in $pids; do
        [ "$p" -eq "$my_pid" ] && continue
        [ ! -d "/proc/$p" ] && continue
        local cmdline
        cmdline=$(tr "\0" " " < "/proc/$p/cmdline" 2>/dev/null || true)
        if [[ "$cmdline" =~ app-server || "$cmdline" =~ pid-update-loop || "$cmdline" =~ codex-code-mode-host || "$cmdline" =~ run_autonomous_codex || "$cmdline" =~ format_codex_stream ]]; then
            continue
        fi
        if [[ "$cmdline" =~ bin/codex ]]; then
            return 0
        fi
    done
    return 1
}

# 监测整个项目计划是否全部圆满完成 (严格多层防误判防护)
check_all_tasks_completed() {
    local last_msg_file="$1"

    # 1. 严格隔离：仅允许检查 Codex 最终答复文件 (LAST_MSG_FILE)，绝不能检查包含提示词指令的原始全量日志 (LAST_LOG_FILE)
    if [ ! -f "$last_msg_file" ] || [ ! -s "$last_msg_file" ]; then
        return 1
    fi

    # 2. 标志行严格匹配：必须以整行形式出现完成标志，避免提示词引用或上下文解释产生误匹配
    if ! grep -qE "^[[:space:]]*${COMPLETION_FLAG}[[:space:]]*$" "$last_msg_file"; then
        return 1
    fi

    # 3. 语义未完成守卫：如果答复中包含"尚未完成"、"未完成"、"下一子任务"等进行中语义，即使有 flag 字符串也拒绝完成
    if grep -qiE "(尚未完成|未完成|下一子任务|下一个待办|下一阶段待办|下一步计划|incomplete|not completed|next subtask)" "$last_msg_file"; then
        log_warn "检测到完成标志，但 Codex 答复中仍包含未完成/待办描述，判定为子任务阶段推进而非整体完成。"
        return 1
    fi

    # 4. ARCHITECTURE.md 状态树守卫：检查架构文档中是否仍存在未完成标记 ([Pending] / [Partially)
    if [ -f "$DOC_ARCH" ]; then
        local pending_count
        pending_count=$(grep -cE "\[Pending\]|\[Partially" "$DOC_ARCH" 2>/dev/null || true)
        if [ -n "$pending_count" ] && [ "$pending_count" -gt 0 ]; then
            log_warn "检测到完成标志，但 ./docs/ARCHITECTURE.md 中仍存在 ${pending_count} 处 [Pending] 或 [Partially] 状态项！"
            log_warn "判定整个项目计划尚未全部达成，将继续进入下一轮子任务推进。"
            return 1
        fi
    fi

    return 0
}

# 倒计时等待函数 (在 5 小时 Limit 触发时友好显示进度并响应 Ctrl+C)
wait_with_countdown() {
    local total_seconds="$1"
    local reason="$2"

    if [ "$total_seconds" -le 0 ]; then
        log_info "重置时间已过，无需等待，立即继续执行！"
        return 0
    fi

    local end_time=$(( $(date +%s) + total_seconds ))

    echo -e "${CLR_YELLOW}[LIMIT COOLDOWN] ${reason}${CLR_RESET}"
    echo -e "${CLR_CYAN}[LIMIT COOLDOWN] 预计恢复时间: $(date -d "@$end_time" '+%Y-%m-%d %H:%M:%S') (按 Ctrl+C 可随时安全退出)${CLR_RESET}"

    while true; do
        local now
        now=$(date +%s)
        local remaining=$(( end_time - now ))
        if [ "$remaining" -le 0 ]; then
            break
        fi

        local hours=$(( remaining / 3600 ))
        local minutes=$(( (remaining % 3600) / 60 ))
        local seconds=$(( remaining % 60 ))

        printf "\r\033[K${CLR_YELLOW}[限额冷却倒计时]${CLR_RESET} 剩余: ${CLR_BOLD}%02d小时 %02d分钟 %02d秒${CLR_RESET} | 预计恢复: $(date -d "@$end_time" '+%H:%M:%S')" "$hours" "$minutes" "$seconds"
        sleep 1
    done
    printf "\n"
    log_success "限额冷却期结束（已到达预计重置时间点），准备重新启动 Codex 执行！"
}

# ------------------------------------------------------------------------------
# 宿主自动化 Git 提交函数 (解决沙箱 .git 只读限制，严格满足 AGENTS.md 规范)
# ------------------------------------------------------------------------------
auto_commit_subtask_changes() {
    local turn_num="$1"
    local last_msg_file="$2"

    if [ ! -d "${PROJECT_ROOT}/.git" ]; then
        return 0
    fi

    # 检查除 runner 脚本与格式化工具之外是否有待提交的代码改动
    local status_output
    status_output=$(git -C "$PROJECT_ROOT" status --porcelain -- ':!run_autonomous_codex.sh' ':!format_codex_stream.py' 2>/dev/null || true)

    if [ -z "$status_output" ]; then
        log_info "工作区无新增待提交代码改动，跳过自动 Git 提交。"
        return 0
    fi

    log_info "检测到本轮产生未提交代码改动，正在由宿主执行自动 Git 提交..."

    # 提取结构化提交信息
    local commit_msg
    commit_msg=$(python3 - "$last_msg_file" "$turn_num" <<'PYEOF'
import re, os, sys

msg_file = sys.argv[1] if len(sys.argv) > 1 else ""
turn = sys.argv[2] if len(sys.argv) > 2 else "1"

def build_commit():
    lines = []
    if msg_file and os.path.exists(msg_file):
        try:
            with open(msg_file, "r", encoding="utf-8", errors="replace") as f:
                lines = [l.strip() for l in f if l.strip()]
        except Exception:
            pass

    if not lines:
        return f"chore(turn-{turn}): complete autonomous subtask\n\nTurn #{turn} automated commit by host runner."

    first = lines[0]
    # 清理 markdown 粗体、斜体、代码、标题和链接
    clean_title = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", first)
    clean_title = re.sub(r"\*\*|__|[*`#]", "", clean_title)
    clean_title = re.sub(r"^[：:]+|[：:]+$", "", clean_title).strip()

    # 规范化 commit 标题
    if not re.match(r"^(feat|fix|refactor|chore|docs|test|style)\b", clean_title, re.I):
        if len(clean_title) > 65:
            clean_title = clean_title[:62] + "..."
        subject = f"feat: {clean_title}"
    else:
        subject = clean_title

    body_lines = []
    for l in lines[1:8]:
        if "```" in l or "ARCHITECTURE.md" in l or "已同步" in l or "未能提交" in l or "已尝试提交" in l:
            break
        body_lines.append(l)

    body = "\n".join(body_lines).strip()
    footer = f"\n\nTurn #{turn} autonomous subtask completion.\nAutomated commit by host runner."
    return subject + ("\n\n" + body if body else "") + footer

print(build_commit())
PYEOF
    )

    if [ -z "$commit_msg" ]; then
        commit_msg="feat(turn-${turn_num}): complete autonomous subtask"
    fi

    # 暂存所有项目改动 (保持 runner 脚本本身不被混入子任务业务 commit)
    git -C "$PROJECT_ROOT" add -A -- ':!run_autonomous_codex.sh' ':!format_codex_stream.py'

    # 执行 commit
    if git -C "$PROJECT_ROOT" commit -m "$commit_msg" >/dev/null 2>&1; then
        local commit_hash
        commit_hash=$(git -C "$PROJECT_ROOT" rev-parse --short HEAD)
        local commit_subject
        commit_subject=$(git -C "$PROJECT_ROOT" log -1 --pretty=format:"%s")
        log_success "【Git 自动提交完成】${CLR_BOLD}${commit_hash}${CLR_RESET} - ${commit_subject}"
    else
        log_warn "Git 提交执行未生效或已由前序操作提交。"
    fi
}

# ------------------------------------------------------------------------------
# 5. 提示词构造 (Prompt Generator)
# ------------------------------------------------------------------------------
generate_initial_prompt() {
    cat <<EOF
你正在以完全无人值守、高自主性的方式推进本项目的架构重构与开发工作。
请仔细阅读并严格结合 ./headless.md 和 ./docs/ARCHITECTURE.md 中的要求执行工作。

【项目背景与目标】
1. 本项目目标是将 Clash Verge Rev 中的核心组件拆离并改造成无需桌面环境（无 Tauri/无外部服务依赖）的单个 headless service (mihomo-server)。
2. 该 service 是唯一系统服务入口，负责 Mihomo 内核生命周期管理、Axum HTTP 管理 API、WebSocket 事件转发、配置生成校验以及 Web UI 静态托管。
3. 遵循 ./docs/ARCHITECTURE.md 中记录的架构分层、状态标记（[Migrated]、[Implemented]、[Partially migrated]、[Pending] 等）和交付顺序 (Delivery order)。
4. 【上游代码参考】：上游 clash-verge-rev 源码位于 ../clash-verge-rev（已在沙箱中开启读取权限）。在拆离或移植算法（例如 enhance/配置增强、mihomo 交互通信、前端组件等）时，请直接阅读参考 ../clash-verge-rev 中的对应实现，保留原有业务行为。

【本轮执行准则】
1. 先查看 ./docs/ARCHITECTURE.md 中记录的当前完成进度与未完成模块（如 headless-core 剩余功能、Axum API、WebSocket、Web UI 适配等）。
2. 从交付顺序中选取下一个具体的、自包含的小任务进行开发或迁移。
3. 编写/调整代码并运行 \`cargo check --workspace\` 以及相关测试进行行为验证，保证代码能正确构建和通过测试。
4. 【强制要求 - 同步进度】：在完成该小任务并验证后，必须立即编辑 ./docs/ARCHITECTURE.md 文档：
   - 更新 ## Complete target architecture 中的状态标签（例如将 [Pending] 变更为 [Partially implemented] 或 [Implemented]）。
   - 更新文档底部的迁移状态与进度记录，说明本次变更、验证结果和下一步计划。
5. 【重要 - 关于 Git 自动提交】：
   - Linux Codex 沙箱环境按安全设计将 .git 目录挂载为只读，因此在沙箱内部执行 git add / git commit 会报错 "Read-only file system" 或无法创建 index.lock。
   - 请【绝对不要】在沙箱内尝试执行 git 提交命令。
   - 外部自动化宿主运行脚本 (run_autonomous_codex.sh) 会在每轮子任务完成并验证通过后，自动代你在宿主机上将代码改动原子提交到 Git 并记录提交信息。你只需专注于编写代码、跑通测试验证、并同步更新 ./docs/ARCHITECTURE.md 即可！
6. 【重要 - 完成判定与标志输出】：
   当且仅当 ./headless.md 和 ./docs/ARCHITECTURE.md 中所要求的所有架构组件（核心库、Axum API、WebSocket、Web UI 适配、生命周期管理、配置增强与事务、单服务打包部署与测试验证）全部完整实现并通过验证时，在本次最终回答的最末尾单独输出一行特定标记字符串：
   $COMPLETION_FLAG
   如果整个项目的最终计划尚未全部达成，请绝对不要输出该标记字符串！只需总结本小任务的完成成果并指出下一步任务即可。
EOF
}

generate_continuation_prompt() {
    local extra_warning="$1"
    cat <<EOF
继续推进无人值守自动化重构与编程任务。
$extra_warning

【执行步骤】
1. 查看 ./docs/ARCHITECTURE.md 与当前代码库状态，确认上一个子任务的完成情况。
2. 依据 ./docs/ARCHITECTURE.md 中的交付顺序 (Delivery order)，选取下一个待实现的子任务继续编写代码。
3. 【上游参考】：若需要参考上游原始实现，可直接查阅 ../clash-verge-rev 目录中的源码。
4. 运行 \`cargo check --workspace\` 及相关测试，验证修改的正确性。
5. 【强制要求 - 同步进度】：完成该小任务后，必须同步更新 ./docs/ARCHITECTURE.md 文件中的架构树状态与进度总结。
6. 【关于 Git 提交】：.git 在沙箱内为只读挂载，请勿在沙箱内执行 git commit；每轮完成后外部宿主脚本会自动代你提交。你只需专注于代码实现、验证测试以及同步更新 ./docs/ARCHITECTURE.md。
7. 【重要 - 完成判定】：
   如果且仅如果整个项目的目标与功能已全部完成并验证通过，请在最后输出特定完成标志：
   $COMPLETION_FLAG
   若尚未全部完成，严禁输出该标志，请总结当前进度并明确下一个待办子任务。
EOF
}

generate_timeout_retry_prompt() {
    cat <<EOF
【系统告警通知】
上一轮请求由于超过 10 分钟没有产生任何输出或未执行任何有效操作，已被看门狗安全中断。
请检查当前工作区的代码状态与 git/文件修改，排查刚才是否卡在死循环、网络阻塞或长时间等待中。
请调整策略，以轻量、明确的步骤继续推进下一个子任务，并在完成后同步更新 ./docs/ARCHITECTURE.md。
（提示：可随时查阅 ../clash-verge-rev 获取上游实现参考）
若所有计划已全部完成，请输出 $COMPLETION_FLAG。
EOF
}

# ------------------------------------------------------------------------------
# 6. 单次 Codex 运行核心 (实时输出 + 10分钟看门狗监控)
# ------------------------------------------------------------------------------
run_single_turn() {
    local turn_num="$1"
    local prompt_text="$2"
    local mode="$3"          # "new", "resume", "fork"
    local target_session="$4"

    local turn_log="${LOG_DIR}/turn_${turn_num}_$(date +%Y%m%d_%H%M%S).log"
    local last_msg_file="${LOG_DIR}/turn_${turn_num}_last_msg.txt"
    local fifo_path="${LOG_DIR}/.fifo_${turn_num}_$$"
    local timeout_marker="${LOG_DIR}/.timeout_triggered_${turn_num}"

    CURRENT_TURN_LOG="$turn_log"

    rm -f "$fifo_path" "$timeout_marker" "$last_msg_file"
    mkfifo "$fifo_path"
    CURRENT_FIFO="$fifo_path"

    # 构建命令数组
    local cmd=("$CODEX_BIN" "exec")
    if [ "$mode" = "resume" ]; then
        cmd+=("resume")
        if [ -n "$target_session" ]; then
            cmd+=("$target_session")
        else
            cmd+=("--last")
        fi
    elif [ "$mode" = "fork" ]; then
        cmd+=("fork" "$target_session")
    else
        # 新会话
        cmd+=("-s" "$SANDBOX_MODE")
        if [ -d "$UPSTREAM_DIR" ]; then
            cmd+=("--add-dir" "$UPSTREAM_DIR")
        fi
    fi

    # 通用参数: 自动审批、允许沙箱联网、允许外部工作区/非git目录、保存最后消息
    cmd+=(
        "-c" "approval_policy=${APPROVAL_POLICY}"
        "-c" "sandbox_workspace_write.network_access=true"
        "--skip-git-repo-check"
        "-o" "$last_msg_file"
        "$prompt_text"
    )

    log_info "启动 Turn #$turn_num [模式: $mode${target_session:+, 会话: $target_session}]"
    log_info "实时输出已建立，同时记录于: $turn_log"

    # 启动实时输出与文件落盘 (优先使用格式化渲染器实现折叠与动画，若无则回退至 tee)
    touch "$turn_log"
    if [ -f "${SCRIPT_DIR}/format_codex_stream.py" ] && command -v python3 >/dev/null 2>&1; then
        python3 "${SCRIPT_DIR}/format_codex_stream.py" --log-file "$turn_log" --session-file "$SESSION_FILE" < "$fifo_path" &
        CURRENT_TEE_PID=$!
    else
        tee -a "$turn_log" < "$fifo_path" &
        CURRENT_TEE_PID=$!
    fi

    # 启动 codex 主进程
    if command -v stdbuf >/dev/null 2>&1; then
        stdbuf -oL -eL "${cmd[@]}" > "$fifo_path" 2>&1 &
    else
        "${cmd[@]}" > "$fifo_path" 2>&1 &
    fi
    CURRENT_CODEX_PID=$!

    # 启动 10 分钟看门狗 (监测每个 request 超过 10 分钟无响应/无操作)
    (
        while kill -0 "$CURRENT_CODEX_PID" 2>/dev/null; do
            sleep 5
            local last_mod
            last_mod=$(stat -c %Y "$turn_log" 2>/dev/null || date +%s)
            local now
            now=$(date +%s)
            local inactive_secs=$(( now - last_mod ))

            if [ "$inactive_secs" -ge "$INACTIVITY_TIMEOUT" ]; then
                echo -e "\n${CLR_RED}================================================================================"
                echo -e "[WATCHDOG TIMEOUT] 监测到当前操作超过 ${INACTIVITY_TIMEOUT} 秒 ($(( INACTIVITY_TIMEOUT / 60 )) 分钟) 未产生任何输出或响应！"
                echo -e "[WATCHDOG TIMEOUT] 判定为任务卡死/网络悬挂，正在强制中断 PID: $CURRENT_CODEX_PID 并触发自动重试..."
                echo -e "================================================================================${CLR_RESET}"
                touch "$timeout_marker"
                kill_tree "$CURRENT_CODEX_PID" TERM
                sleep 4
                kill_tree "$CURRENT_CODEX_PID" KILL
                exit 0
            fi
        done
    ) &
    CURRENT_WATCHDOG_PID=$!

    # 等待 codex 进程退出
    local exit_code=0
    wait "$CURRENT_CODEX_PID" 2>/dev/null || exit_code=$?
    CURRENT_CODEX_PID=""

    # 停止看门狗
    if [ -n "$CURRENT_WATCHDOG_PID" ]; then
        kill -9 "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
        wait "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
        CURRENT_WATCHDOG_PID=""
    fi

    # 等待 tee 完成输出
    if [ -n "$CURRENT_TEE_PID" ]; then
        wait "$CURRENT_TEE_PID" 2>/dev/null || true
        CURRENT_TEE_PID=""
    fi
    rm -f "$fifo_path"
    CURRENT_FIFO=""

    # 检查是否因超时被终止
    local was_timeout=0
    if [ -f "$timeout_marker" ]; then
        was_timeout=1
        rm -f "$timeout_marker"
    fi

    # 记录最新的会话 ID (优先从当前 turn_log 直接提取，保证与正在执行的会话一致)
    local new_session_id=""
    if [ -f "$turn_log" ]; then
        new_session_id=$(grep -oE "session id:\s*[0-9a-fA-F-]+" "$turn_log" | tail -n 1 | awk '{print $NF}')
    fi
    # 仅当本轮确实生成或连接到该会话时才写入 SESSION_FILE，避免失败时被错误覆盖
    if [ -n "$new_session_id" ]; then
        echo "$new_session_id" > "$SESSION_FILE"
    fi

    # 将运行结果通过全局变量传递返回
    LAST_EXIT_CODE=$exit_code
    LAST_WAS_TIMEOUT=$was_timeout
    LAST_LOG_FILE="$turn_log"
    LAST_MSG_FILE="$last_msg_file"
}

# ------------------------------------------------------------------------------
# 7. 主执行循环 (Autonomous Main Loop)
# ------------------------------------------------------------------------------
main() {
    # 解析传入参数
    local force_new=0
    local custom_session=""
    local custom_fork=0

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --new)
                force_new=1
                shift
                ;;
            --session)
                custom_session="$2"
                shift 2
                ;;
            --fork)
                custom_fork=1
                if [[ $# -ge 2 && ! "$2" =~ ^-- ]]; then
                    custom_session="$2"
                    shift 2
                else
                    shift 1
                fi
                ;;
            --upstream)
                UPSTREAM_DIR="$2"
                shift 2
                ;;
            --timeout)
                INACTIVITY_TIMEOUT="$2"
                shift 2
                ;;
            --cooldown)
                RATE_LIMIT_COOLDOWN="$2"
                shift 2
                ;;
            --restart-daemon)
                log_info "正在重启本地 Codex app-server daemon..."
                "$CODEX_BIN" app-server daemon restart >/dev/null 2>&1 || true
                shift
                ;;
            --flag)
                COMPLETION_FLAG="$2"
                shift 2
                ;;
            -h|--help)
                echo "用法: $0 [选项]"
                echo "选项:"
                echo "  --new                  强制新建独立会话，不继续之前会话"
                echo "  --session <ID>         指定要恢复或分叉的 Codex 会话 UUID"
                echo "  --fork [ID]            从指定或最近的会话分叉出新会话（避免与已打开的交互终端冲突）"
                echo "  --restart-daemon       重启本地 Codex daemon 并清理所有残留会话锁"
                echo "  --upstream <目录>      指定上游代码库路径 (默认: ../clash-verge-rev)"
                echo "  --timeout <秒>         设置单次请求无响应超时时限 (默认: 600 秒)"
                echo "  --cooldown <秒>        设置遭遇 5 小时 Limit 时的等待时限 (默认: 18000 秒)"
                echo "  --flag <字符串>        自定义全部任务完成标志字符串"
                echo "  -h, --help             显示此帮助信息"
                exit 0
                ;;
            *)
                log_warn "未知参数: $1，忽略"
                shift
                ;;
        esac
    done

    log_box "mihomo-server 无人值守自动化 Codex 编程工作台已启动" "$CLR_GREEN"
    log_info "工作区目录: $PROJECT_ROOT"
    log_info "上游源码库: $UPSTREAM_DIR $([ -d "$UPSTREAM_DIR" ] && echo -e "${CLR_GREEN}[有效目录，已开放沙箱跨库读取]${CLR_RESET}" || echo -e "${CLR_YELLOW}[未找到该目录]${CLR_RESET}")"
    log_info "Codex 路径: $CODEX_BIN"
    log_info "运行配置: 沙箱=$SANDBOX_MODE | 自动审批=$APPROVAL_POLICY | 无响应超时=${INACTIVITY_TIMEOUT}s | 5小时冷却=${RATE_LIMIT_COOLDOWN}s"
    log_info "计划完成标志: $COMPLETION_FLAG"
    log_info "提示: 任何时候均可按 Ctrl+C 安全中断退出。"

    # 确定初始会话模式与目标会话
    local current_mode="resume"
    local active_session=""

    if [ "$force_new" -eq 1 ]; then
        current_mode="new"
        active_session=""
    elif [ "$custom_fork" -eq 1 ]; then
        current_mode="fork"
        active_session="${custom_session:-$(get_latest_session_id)}"
    elif [ -n "$custom_session" ]; then
        current_mode="resume"
        active_session="$custom_session"
        echo "$active_session" > "$SESSION_FILE"
    elif [ -f "$SESSION_FILE" ] && [ -s "$SESSION_FILE" ]; then
        current_mode="resume"
        active_session=$(cat "$SESSION_FILE" | tr -d '[:space:]')
    else
        local latest_recorded
        latest_recorded=$(get_latest_session_id)
        if [ -n "$latest_recorded" ]; then
            current_mode="resume"
            active_session="$latest_recorded"
            echo "$active_session" > "$SESSION_FILE"
        else
            current_mode="new"
            active_session=""
        fi
    fi

    local turn_count=1
    local consecutive_timeouts=0
    local consecutive_lock_conflicts=0
    local last_sync_warning=""

    while true; do
        log_box ">>> 开始第 $turn_count 轮子任务执行 <<<" "$CLR_CYAN"

        # 记录本次执行前 ARCHITECTURE.md 的哈希
        local arch_hash_before
        arch_hash_before=$(get_arch_hash)

        # 生成本次 Prompt
        local current_prompt=""
        if [ "$consecutive_timeouts" -gt 0 ]; then
            current_prompt=$(generate_timeout_retry_prompt)
        elif [[ "$turn_count" -eq 1 && "$current_mode" = "new" ]]; then
            current_prompt=$(generate_initial_prompt)
        else
            current_prompt=$(generate_continuation_prompt "$last_sync_warning")
        fi
        last_sync_warning=""

        # 执行 Turn
        LAST_EXIT_CODE=0
        LAST_WAS_TIMEOUT=0
        LAST_LOG_FILE=""
        LAST_MSG_FILE=""

        run_single_turn "$turn_count" "$current_prompt" "$current_mode" "$active_session"

        # 仅当本轮已成功捕获或记录 session_id 时更新 active_session
        if [ -f "$SESSION_FILE" ] && [ -s "$SESSION_FILE" ]; then
            active_session=$(cat "$SESSION_FILE" | tr -d '[:space:]')
            current_mode="resume"
        fi

        # ----------------------------------------------------------------------
        # 分支 A: 处理 10 分钟无响应超时中断
        # ----------------------------------------------------------------------
        if [ "$LAST_WAS_TIMEOUT" -eq 1 ]; then
            consecutive_timeouts=$(( consecutive_timeouts + 1 ))
            log_error "Turn #$turn_count 触发 10 分钟超时看门狗 (连续第 $consecutive_timeouts 次超时)。"

            if [ "$consecutive_timeouts" -ge "$MAX_TIMEOUT_RETRIES" ]; then
                log_warn "连续超时达到 ${MAX_TIMEOUT_RETRIES} 次，暂停 30 秒以恢复系统资源..."
                sleep 30
            else
                log_info "稍后将自动进入重试..."
                sleep 5
            fi
            turn_count=$(( turn_count + 1 ))
            continue
        fi

        # ----------------------------------------------------------------------
        # 分支 B: 监测 5 小时 Limit 限制问题 (基于真实配额状态与准确 reset time)
        # ----------------------------------------------------------------------
        if check_rate_limit "$LAST_EXIT_CODE" "$LAST_LOG_FILE" "$LAST_MSG_FILE"; then
            local IS_RATE_LIMITED=0
            local WAIT_SECONDS="$RATE_LIMIT_COOLDOWN"
            local RESET_TIME=""
            local RESET_SOURCE=""
            local RESET_DETAIL=""
            eval "$(get_rate_limit_info "$LAST_LOG_FILE" "$LAST_MSG_FILE" "$RATE_LIMIT_COOLDOWN")"

            if [ "$IS_RATE_LIMITED" -eq 1 ] && [ "$WAIT_SECONDS" -gt 0 ]; then
                echo ""
                log_box "【检测到触发 5 小时 Limit / Rate Limit 配额上限】" "$CLR_RED"
                log_warn "限额详情: ${CLR_BOLD}${RESET_DETAIL}${CLR_RESET}"
                log_info "精准预计恢复时间: ${CLR_BOLD}${RESET_TIME}${CLR_RESET} (来源: ${RESET_SOURCE})"
                local wait_h=$(( WAIT_SECONDS / 3600 ))
                local wait_m=$(( (WAIT_SECONDS % 3600) / 60 ))
                local wait_s=$(( WAIT_SECONDS % 60 ))
                log_info "动态计算等待时长: ${CLR_BOLD}${wait_h}小时 ${wait_m}分钟 ${wait_s}秒${CLR_RESET} (已计入 60 秒安全缓冲，无需盲目等待 5 小时)"

                wait_with_countdown "$WAIT_SECONDS" "等待配额刷新重置 ($RESET_TIME)"

                log_info "已成功到达配额重置时间点 ($RESET_TIME)，自动重新拉起 Codex 会话..."
                consecutive_timeouts=0
                turn_count=$(( turn_count + 1 ))
                continue
            else
                log_info "经核验当前配额充足 (${RESET_DETAIL})，判定为非限额异常，继续正常调度流程。"
            fi
        fi

        # ----------------------------------------------------------------------
        # 分支 C: 检测会话冲突 (already has an active writer)
        # ----------------------------------------------------------------------
        if check_active_writer_conflict "$LAST_LOG_FILE"; then
            consecutive_lock_conflicts=$(( consecutive_lock_conflicts + 1 ))
            log_warn "检测到会话锁定冲突 (already has an active writer: ${active_session:-当前会话})！"

            if has_foreground_codex_cli; then
                log_warn "系统检测到仍有其他终端正在前台运行交互式 Codex 进程。"
                log_info "自动应对策略: 正在从该会话分叉 (Fork) 一个新会话，以避免冲突并继续执行..."
                current_mode="fork"
                sleep 2
                continue
            fi

            if [ "$consecutive_lock_conflicts" -le 2 ]; then
                log_info "经系统检测，前台并无其他正在运行的 Codex 交互终端。"
                log_info "该锁定通常是由于前一会话退出后，后台 Codex daemon 仍在异步清理释放（如 MCP断开/状态刷盘）导致的残留锁定。"
                log_info "正在自动重启本地 Codex daemon 释放残留会话锁并立即重试 (第 $consecutive_lock_conflicts 次重试)..."
                "$CODEX_BIN" app-server daemon restart >/dev/null 2>&1 || true
                sleep 2
                current_mode="resume"
                continue
            else
                log_warn "重启 daemon 后重试仍遇到会话锁定冲突，自动切换为分叉 (Fork) 模式继续执行..."
                current_mode="fork"
                consecutive_lock_conflicts=0
                sleep 2
                continue
            fi
        fi

        consecutive_lock_conflicts=0

        # ----------------------------------------------------------------------
        # 分支 E: 验证文档同步状态 (ARCHITECTURE.md)
        # ----------------------------------------------------------------------
        local arch_hash_after
        arch_hash_after=$(get_arch_hash)
        if [ "$arch_hash_before" != "$arch_hash_after" ]; then
            log_success "【进度已同步】检测到 ./docs/ARCHITECTURE.md 已成功更新进度！"
        else
            log_warn "【进度未同步告警】本轮子任务结束后，./docs/ARCHITECTURE.md 未检测到修改！"
            last_sync_warning="【注意：上一轮任务完成后未检测到 ./docs/ARCHITECTURE.md 更新，请在本轮务必更新 ARCHITECTURE.md 同步最新架构与完成状态！】"
        fi

        # ----------------------------------------------------------------------
        # 分支 F: 宿主自动化 Git 提交 (严格执行 AGENTS.md 规范并避开沙箱只读限制)
        # ----------------------------------------------------------------------
        auto_commit_subtask_changes "$turn_count" "$LAST_MSG_FILE"

        # ----------------------------------------------------------------------
        # 分支 D: 检测整个项目计划完成标志 flag (严格多层防误判校验)
        # ----------------------------------------------------------------------
        if check_all_tasks_completed "$LAST_MSG_FILE"; then
            echo ""
            log_box "🎉🎉🎉 【项目全部计划圆满完成】 🎉🎉🎉" "$CLR_GREEN"
            log_success "成功检测到全部任务完成标志: ${CLR_BOLD}${COMPLETION_FLAG}${CLR_RESET}"
            log_success "所有在 ./headless.md 与 ./docs/ARCHITECTURE.md 中规划的架构目标与功能已全部交付并验证完成！"
            log_info "总执行轮次: $turn_count"
            log_info "最终日志文件: $LAST_LOG_FILE"
            exit 0
        fi

        # 正常轮次推进
        consecutive_timeouts=0
        turn_count=$(( turn_count + 1 ))

        log_info "Turn #$(( turn_count - 1 )) 执行完毕。短暂休整 5 秒后继续推进下一子任务..."
        sleep 5
    done
}

main "$@"
