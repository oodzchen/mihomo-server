#!/usr/bin/env bash
# ==============================================================================
# run_autonomous_codex.sh
#
# 无人值守自动化运行 Agent 脚本 (支持 OpenAI Codex 与 Google Antigravity CLI 双引擎)
# 适用项目: mihomo-server (基于 ./headless.md 和 ./docs/ARCHITECTURE.md)
#
# 核心特性:
# 1. 多 Agent 代理引擎支持:
#    - Codex CLI (--agent codex)
#    - Antigravity CLI (--agent agy / --agent antigravity)
# 2. 全自动化与免审批执行:
#    - Codex: 沙箱 workspace-write + 自动批准 (-c approval_policy="never") + 联网
#    - Antigravity: --dangerously-skip-permissions 自动批准所有工具执行
# 3. 仿原生 TUI 终端实时流式渲染与动态加载 (Spinner + 操作折叠 + 代码差异捕获)
# 4. 允许人工随时安全中断 (Ctrl+C 优雅终止并打印各 Agent 专属恢复命令)
# 5. 10 分钟看门狗监测：超过 10 分钟无响应/无操作时，强制中断并自动重试
# 6. 5 小时 / Rate Limit 智能限额监测与精准重置倒计时
# 7. 自动提示并监控每个小任务完成后同步进度至 docs/ARCHITECTURE.md
# 8. 宿主级 Git 自动化原子提交 (解决沙箱 .git 只读限制，提取 Conventional Commit)
# 9. 识别全部计划完成标志 (===ALL_TASKS_COMPLETED_SUCCESSFULLY===) 并在全部达成后自动停机
# ==============================================================================

set -uo pipefail

# ------------------------------------------------------------------------------
# 1. 基础配置与环境变量
# ------------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$PROJECT_ROOT" || exit 1

# Agent 代理工具选择: 支持 codex, agy, antigravity (可通过命令行 --agent 或环境变量 AGENT_TOOL / AGENT_TYPE 自定义，默认 codex)
AGENT_TYPE="${AGENT_TYPE:-${AGENT_TOOL:-codex}}"
AGENT_TYPE="$(echo "$AGENT_TYPE" | tr '[:upper:]' '[:lower:]')"
if [ "$AGENT_TYPE" = "antigravity" ]; then
    AGENT_TYPE="agy"
fi

# 可执行文件路径探测
CODEX_BIN="${CODEX_BIN:-$(command -v codex || echo "$HOME/.nvm/versions/node/current/bin/codex")}"
AGY_BIN="${AGY_BIN:-$(command -v agy || echo "$HOME/.local/bin/agy")}"

# 参数与超限控制 (可通过环境变量自定义)
INACTIVITY_TIMEOUT="${INACTIVITY_TIMEOUT:-600}"          # 请求无响应超时时间 (秒, 默认 10 分钟)
RATE_LIMIT_COOLDOWN="${RATE_LIMIT_COOLDOWN:-18000}"       # 5 小时限额冷却时间 (秒, 默认 5*3600 = 18000 秒)
MAX_TIMEOUT_RETRIES="${MAX_TIMEOUT_RETRIES:-5}"           # 单个小任务连续超时重试最大次数
SANDBOX_MODE="${SANDBOX_MODE:-workspace-write}"          # Codex 沙箱模式: workspace-write
ENABLE_SANDBOX="${ENABLE_SANDBOX:-1}"                    # 是否启用隔离沙箱模式 (0=禁用, 1=启用; codex 与 agy 均生效)
APPROVAL_POLICY="${APPROVAL_POLICY:-never}"              # 审批模式: never (无人值守自动审批)
COMPLETION_FLAG="${COMPLETION_FLAG:-===ALL_TASKS_COMPLETED_SUCCESSFULLY===}" # 全部任务完成标志

# ------------------------------------------------------------------------------
# 资源限制与调度配置 (保护 Samba 媒体服务器与宿主系统响应)
# ------------------------------------------------------------------------------
ENABLE_RESOURCE_LIMITS="${ENABLE_RESOURCE_LIMITS:-1}"       # 是否启用资源调度限制 (0=禁用, 1=启用)
TOTAL_SYSTEM_CPUS=$(nproc 2>/dev/null || echo 16)
# 仅预留少量核心 (如 CPU 0-1) 专供 Samba、网络中断与前台网页浏览，其余核心全部分配给开发编译
if [ "$TOTAL_SYSTEM_CPUS" -ge 8 ]; then
    DEFAULT_CPU_AFFINITY="2-$((TOTAL_SYSTEM_CPUS - 1))"
    DEFAULT_CARGO_JOBS="$((TOTAL_SYSTEM_CPUS - 4 > 4 ? TOTAL_SYSTEM_CPUS - 4 : 4))"
elif [ "$TOTAL_SYSTEM_CPUS" -ge 4 ]; then
    DEFAULT_CPU_AFFINITY="1-$((TOTAL_SYSTEM_CPUS - 1))"
    DEFAULT_CARGO_JOBS="$((TOTAL_SYSTEM_CPUS - 1))"
else
    DEFAULT_CPU_AFFINITY=""
    DEFAULT_CARGO_JOBS="$TOTAL_SYSTEM_CPUS"
fi
CPU_AFFINITY="${CPU_AFFINITY:-$DEFAULT_CPU_AFFINITY}"       # 绑定的 CPU 核心列表
PROCESS_NICE="${PROCESS_NICE:-10}"                          # 进程调度优先级 (10: 温和后台，Samba 随时抢占，平时全速)
PROCESS_IONICE_CLASS="${PROCESS_IONICE_CLASS:-2}"           # 磁盘 IO 调度类 (2 = Best Effort)
PROCESS_IONICE_PRIO="${PROCESS_IONICE_PRIO:-6}"             # 磁盘 IO 优先级
CARGO_JOBS="${CARGO_JOBS:-$DEFAULT_CARGO_JOBS}"             # Cargo 编译与测试最大并发数

# 运行日志与状态持久化 (集中在 automation 目录下，不污染项目源码根目录)
LOG_DIR="${SCRIPT_DIR}/logs"
mkdir -p "$LOG_DIR"
SESSION_FILE="${SCRIPT_DIR}/.session_id"
LAST_AGENT_FILE="${SCRIPT_DIR}/.last_agent"
MAX_RETAINED_LOGS="${MAX_RETAINED_LOGS:-5}"        # 最多保留的历史轮次全量日志数
MAX_LOG_DIR_MB="${MAX_LOG_DIR_MB:-15}"             # logs 目录空间占用上限 (MB)
AUTO_COMPRESS_LOGS="${AUTO_COMPRESS_LOGS:-1}"      # 是否在轮次完成后自动 gzip 压缩全量日志

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
# 2.1 Agent 二进制校验
# ------------------------------------------------------------------------------
validate_agent_bin() {
    if [ "$AGENT_TYPE" = "codex" ]; then
        if [ ! -x "$CODEX_BIN" ]; then
            log_error "未找到有效的 codex 可执行文件: $CODEX_BIN"
            log_info "若需要切换为 Antigravity CLI，请添加参数: --agent agy"
            exit 1
        fi
    elif [ "$AGENT_TYPE" = "agy" ]; then
        if [ ! -x "$AGY_BIN" ]; then
            log_error "未找到有效的 agy (Antigravity CLI) 可执行文件: $AGY_BIN"
            log_info "若需要切换为 Codex CLI，请添加参数: --agent codex"
            exit 1
        fi
    else
        log_error "不支持的 Agent 类型: $AGENT_TYPE (支持选项: codex, agy, antigravity)"
        exit 1
    fi
}

# ------------------------------------------------------------------------------
# 2.2 资源调度与核心隔离
# ------------------------------------------------------------------------------
apply_resource_limits() {
    [ "$ENABLE_RESOURCE_LIMITS" -ne 1 ] && return 0

    export CARGO_BUILD_JOBS="$CARGO_JOBS"
    export RAYON_NUM_THREADS="$CARGO_JOBS"
    export RUST_TEST_THREADS="$CARGO_JOBS"

    if renice -n "$PROCESS_NICE" -p $$ >/dev/null 2>&1; then
        log_info "已配置进程调度 Nice 优先级: $PROCESS_NICE (温和让位，Samba 随时抢占，平时全速编译)"
    fi

    if command -v ionice >/dev/null 2>&1; then
        if [ "$PROCESS_IONICE_CLASS" -eq 2 ]; then
            if ionice -c 2 -n "$PROCESS_IONICE_PRIO" -p $$ >/dev/null 2>&1; then
                log_info "已配置磁盘 IO 调度: Best Effort (Prio $PROCESS_IONICE_PRIO，保障编译 IO 吞吐并兼顾媒体优先)"
            fi
        elif ionice -c "$PROCESS_IONICE_CLASS" -p $$ >/dev/null 2>&1; then
            log_info "已配置磁盘 IO 调度类: class $PROCESS_IONICE_CLASS"
        fi
    fi

    if [ -n "$CPU_AFFINITY" ] && command -v taskset >/dev/null 2>&1; then
        if taskset -cp "$CPU_AFFINITY" $$ >/dev/null 2>&1; then
            local reserved_end=""
            if [[ "$CPU_AFFINITY" =~ ^([0-9]+)- ]]; then
                local first_c="${BASH_REMATCH[1]}"
                if [ "$first_c" -gt 0 ]; then
                    reserved_end=" (隔离 CPU 0-$((first_c - 1)) 专供 Samba 媒体服务与前台网页浏览)"
                fi
            fi
            log_info "已绑定 CPU 核心亲和度: $CPU_AFFINITY${reserved_end}"
        fi
    fi
}

# ------------------------------------------------------------------------------
# 3. 进程树管理与信号捕获 (支持人工手动中断 Ctrl+C)
# ------------------------------------------------------------------------------
CURRENT_AGENT_PID=""
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

    local child_pids
    child_pids=$(pgrep -P "$parent_pid" 2>/dev/null || true)
    for child in $child_pids; do
        kill_tree "$child" "$sig"
    done
    kill -s "$sig" "$parent_pid" 2>/dev/null || true
}

cleanup_turn_resources() {
    if [ -n "$CURRENT_WATCHDOG_PID" ] && kill -0 "$CURRENT_WATCHDOG_PID" 2>/dev/null; then
        kill -9 "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
        wait "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
    fi
    CURRENT_WATCHDOG_PID=""

    if [ -n "$CURRENT_FIFO" ] && [ -p "$CURRENT_FIFO" ]; then
        rm -f "$CURRENT_FIFO" 2>/dev/null || true
    fi
    CURRENT_FIFO=""
}

handle_manual_interrupt() {
    [ "$INTERRUPTING" -eq 1 ] && return 0
    INTERRUPTING=1
    trap '' SIGINT SIGTERM SIGHUP

    local agent_name="Agent"
    [ "$AGENT_TYPE" = "codex" ] && agent_name="Codex"
    [ "$AGENT_TYPE" = "agy" ] && agent_name="Antigravity (agy)"

    echo ""
    log_warn "检测到人工手动中断信号 (Ctrl+C / SIGINT)！"
    log_info "正在通知 $agent_name 优雅中断并等待其退出..."

    if [ -n "$CURRENT_AGENT_PID" ] && kill -0 "$CURRENT_AGENT_PID" 2>/dev/null; then
        kill -s INT "$CURRENT_AGENT_PID" 2>/dev/null || true
        local count=0
        while kill -0 "$CURRENT_AGENT_PID" 2>/dev/null && [ "$count" -lt 6 ]; do
            sleep 0.5
            count=$(( count + 1 ))
        done
        if kill -0 "$CURRENT_AGENT_PID" 2>/dev/null; then
            kill_tree "$CURRENT_AGENT_PID" TERM
            sleep 1
        fi
        if kill -0 "$CURRENT_AGENT_PID" 2>/dev/null; then
            log_warn "子进程未及时响应，执行强制终止 (KILL)..."
            kill_tree "$CURRENT_AGENT_PID" KILL
        fi
    fi
    CURRENT_AGENT_PID=""

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

    local sid=""
    if [ -n "$CURRENT_TURN_LOG" ] && [ -f "$CURRENT_TURN_LOG" ]; then
        sid=$(extract_session_id_from_log "$CURRENT_TURN_LOG")
    fi
    if [ -z "$sid" ] && [ -f "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" ]; then
        sid=$(cat "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" 2>/dev/null | tr -d '[:space:]')
    fi
    if [ -z "$sid" ] && [ -f "$SESSION_FILE" ] && [ -s "$SESSION_FILE" ]; then
        sid=$(cat "$SESSION_FILE" | tr -d '[:space:]')
    fi
    if [ -z "$sid" ]; then
        sid=$(get_latest_session_id)
    fi

    echo ""
    if [ -n "$sid" ]; then
        save_session_id "$sid"
        log_box " $agent_name 会话已安全中断 (已保存 Session ID) " "$CLR_YELLOW"
        echo -e "${CLR_BOLD}session id: ${CLR_CYAN}${sid}${CLR_RESET}"
        echo -e "${CLR_DIM}--------------------------------------------------------------------------------${CLR_RESET}"
        echo -e "若需接着此会话继续工作，请执行以下命令："
        if [ "$AGENT_TYPE" = "codex" ]; then
            echo -e "  ${CLR_GREEN}▶ 1. 继续无人值守自动运行:${CLR_RESET}"
            echo -e "     ${CLR_BOLD}./automation/run_autonomous_codex.sh --agent codex --session ${sid}${CLR_RESET}"
            echo ""
            echo -e "  ${CLR_BLUE}▶ 2. 进入交互式 Codex 终端手动调试:${CLR_RESET}"
            echo -e "     ${CLR_BOLD}codex resume ${sid}${CLR_RESET}"
            echo ""
            echo -e "  ${CLR_MAGENTA}▶ 3. 非交互式单次指令追加:${CLR_RESET}"
            echo -e "     ${CLR_BOLD}codex exec resume ${sid} \"你的指令\"${CLR_RESET}"
        else
            echo -e "  ${CLR_GREEN}▶ 1. 继续无人值守自动运行:${CLR_RESET}"
            echo -e "     ${CLR_BOLD}./automation/run_autonomous_codex.sh --agent agy --session ${sid}${CLR_RESET}"
            echo ""
            echo -e "  ${CLR_BLUE}▶ 2. 进入交互式 Antigravity 终端调试:${CLR_RESET}"
            echo -e "     ${CLR_BOLD}agy --conversation ${sid}${CLR_RESET}"
            echo ""
            echo -e "  ${CLR_MAGENTA}▶ 3. 非交互式单次指令追加:${CLR_RESET}"
            echo -e "     ${CLR_BOLD}agy --conversation ${sid} -p \"你的指令\"${CLR_RESET}"
        fi
        echo -e "${CLR_YELLOW}================================================================================${CLR_RESET}"
    else
        log_box "已成功安全中断并退出无人值守工作流程。" "$CLR_YELLOW"
    fi

    echo "$AGENT_TYPE" > "$LAST_AGENT_FILE"

    exit 130
}

trap handle_manual_interrupt SIGINT SIGTERM SIGHUP

# ------------------------------------------------------------------------------
# 4. 辅助检测工具函数
# ------------------------------------------------------------------------------
get_arch_hash() {
    if [ -f "$DOC_ARCH" ]; then
        md5sum "$DOC_ARCH" | awk '{print $1}'
    else
        echo "none"
    fi
}

get_latest_session_id() {
    if [ "$AGENT_TYPE" = "codex" ]; then
        local index_file="${HOME}/.codex/session_index.jsonl"
        if [ -f "$index_file" ]; then
            grep -o '"id":"[^"]*"' "$index_file" | tail -n 1 | cut -d'"' -f4
        fi
    elif [ "$AGENT_TYPE" = "agy" ]; then
        local conv_dir="${HOME}/.gemini/antigravity-cli/conversations"
        if [ -d "$conv_dir" ]; then
            find "$conv_dir" -maxdepth 1 -name "*.db" -printf '%T@ %f\n' 2>/dev/null | sort -n | tail -n 1 | awk '{print $2}' | sed 's/\.db$//'
        fi
    fi
}

save_session_id() {
    local sid="$1"
    [ -z "$sid" ] && return 0
    echo "$sid" > "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}"
    echo "$sid" > "$SESSION_FILE"
}

extract_session_id_from_log() {
    local log_file="$1"
    local sid=""
    if [ -f "$log_file" ]; then
        sid=$(grep -oE '"conversation_id"\s*:\s*"[0-9a-fA-F-]+"' "$log_file" | tail -n 1 | cut -d'"' -f4)
        if [ -z "$sid" ]; then
            sid=$(grep -oE "session id:\s*[0-9a-fA-F-]+" "$log_file" | tail -n 1 | awk '{print $NF}')
        fi
        if [ -z "$sid" ]; then
            sid=$(grep -oE "conversation id:\s*[0-9a-fA-F-]+" "$log_file" | tail -n 1 | awk '{print $NF}')
        fi
    fi
    echo "$sid"
}

# 监测限额与配额耗尽 (同时支持 OpenAI Codex 与 Google Cloud/Gemini/Antigravity 错误特征)
check_rate_limit() {
    local exit_code="$1"
    local log_file="$2"
    local last_msg_file="$3"

    if [ "$exit_code" -eq 0 ] && [ -f "$last_msg_file" ] && [ -s "$last_msg_file" ]; then
        if ! tail -n 50 "$log_file" 2>/dev/null | grep -i -E "(hit your usage limit|usage_limit_reached|rate_limit_exceeded|429 [tT]oo [mM]any [rR]equests|RESOURCE_EXHAUSTED|quota exceeded|quota_exceeded)" >/dev/null 2>&1; then
            return 1
        fi
    fi

    local patterns="(usage_limit_reached|rate_limit_exceeded|rate limit exceeded|hit your usage limit|usage limit reached|429 [tT]oo [mM]any [rR]equests|insufficient_quota|reset_after_seconds|\btry again at\s+[0-9]|\btry again in\s+[0-9]|credits? depleted|已达到用量限制|已达到配额上限|RESOURCE_EXHAUSTED|quota exceeded|quota_exceeded|RateLimitExceeded)"
    
    if [ -f "$log_file" ] && tail -n 100 "$log_file" 2>/dev/null | grep -i -E "$patterns" >/dev/null 2>&1; then
        return 0
    fi
    if [ -f "$last_msg_file" ] && grep -i -E "$patterns" "$last_msg_file" >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

# 智能解析限额重置时间
get_rate_limit_info() {
    local log_file="$1"
    local last_msg_file="$2"
    local default_cooldown="${3:-18000}"

    python3 - "$log_file" "$last_msg_file" "$default_cooldown" "$AGENT_TYPE" << 'PYEOF'
import sys, socket, struct, json, os, time, re, datetime

log_file = sys.argv[1] if len(sys.argv) > 1 else ""
last_msg_file = sys.argv[2] if len(sys.argv) > 2 else ""
default_cooldown = int(sys.argv[3]) if len(sys.argv) > 3 and sys.argv[3].isdigit() else 18000
agent_type = sys.argv[4] if len(sys.argv) > 4 else "codex"

now = datetime.datetime.now()
now_ts = int(now.timestamp())

def parse_text(text):
    if not text:
        return None
    if not re.search(r"(?:hit your usage limit|usage_limit_reached|rate_limit_exceeded|rate limit exceeded|usage limit reached|429 [tT]oo [mM]any [rR]equests|insufficient_quota|credits? depleted|已达到用量限制|已达到配额上限|RESOURCE_EXHAUSTED|quota exceeded|quota_exceeded)", text, re.I):
        return None

    m_sec = re.search(r"(?:reset_after(?:_seconds)?|retry after|retry in)[\s:=]+(\d+)", text, re.I)
    if m_sec:
        wait = int(m_sec.group(1)) + 60
        target = now + datetime.timedelta(seconds=wait)
        return wait, target.strftime("%Y-%m-%d %H:%M:%S"), f"提取自 API reset_after ({m_sec.group(1)}s)"

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

    m_rel = re.search(r"(?:try again in|resets? in)\s*(\d+(?:\.\d+)?)\s*(s|sec|seconds?|m|min|minutes?|h|hr|hours?)", text, re.I)
    if m_rel:
        val = float(m_rel.group(1))
        unit = m_rel.group(2).lower()
        sec = int(val * 3600) if unit.startswith("h") else (int(val * 60) if unit.startswith("m") else int(val))
        wait = sec + 60
        target = now + datetime.timedelta(seconds=wait)
        return max(wait, 10), target.strftime("%Y-%m-%d %H:%M:%S"), f"提取自相对时间: {m_rel.group(0)}"

    return None

text_corpus = ""
for fpath in (last_msg_file, log_file):
    if fpath and os.path.exists(fpath):
        try:
            with open(fpath, "r", encoding="utf-8", errors="replace") as f:
                text_corpus += "".join(f.readlines()[-200:]) + "\n"
        except Exception:
            pass

text_res = parse_text(text_corpus)

if text_res:
    wait_sec, target_str, desc = text_res
    source = "error_log"
    is_limited = 1
else:
    # 检查通用冷却 fallback
    if re.search(r"(?:RESOURCE_EXHAUSTED|quota exceeded|hit your usage limit|429 Too Many Requests)", text_corpus, re.I):
        wait_sec = default_cooldown
        target_dt = now + datetime.timedelta(seconds=default_cooldown)
        target_str = target_dt.strftime("%Y-%m-%d %H:%M:%S")
        desc = f"检测到限额配额耗尽，采用默认 {default_cooldown // 3600} 小时冷却"
        source = "default_fallback"
        is_limited = 1
    else:
        wait_sec = 0
        target_str = ""
        desc = "未触发限额限制 (当前配额状态正常)"
        source = "none"
        is_limited = 0

print(f"IS_RATE_LIMITED={is_limited}")
print(f"WAIT_SECONDS={wait_sec}")
print(f"RESET_TIME=\"{target_str}\"")
print(f"RESET_SOURCE=\"{source}\"")
print(f"RESET_DETAIL=\"{desc}\"")
PYEOF
}

check_active_writer_conflict() {
    local log_file="$1"
    if [ "$AGENT_TYPE" = "codex" ]; then
        if [ -f "$log_file" ] && grep -q "already has an active writer" "$log_file"; then
            return 0
        fi
    fi
    return 1
}

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

check_all_tasks_completed() {
    local last_msg_file="$1"

    if [ ! -f "$last_msg_file" ] || [ ! -s "$last_msg_file" ]; then
        return 1
    fi

    if ! grep -qE "^[[:space:]]*${COMPLETION_FLAG}[[:space:]]*$" "$last_msg_file"; then
        return 1
    fi

    if grep -qiE "(尚未完成|未完成|下一子任务|下一个待办|下一阶段待办|下一步计划|incomplete|not completed|next subtask)" "$last_msg_file"; then
        log_warn "检测到完成标志，但答复中仍包含未完成/待办描述，判定为子任务阶段推进而非整体完成。"
        return 1
    fi

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
    log_success "限额冷却期结束，准备重新启动 Agent 执行！"
}

cleanup_old_logs() {
    local log_dir="$1"
    local max_count="$2"
    local max_mb="$3"

    [ ! -d "$log_dir" ] && return 0

    local all_logs=()
    while IFS= read -r -d $'\0' file; do
        all_logs+=("$file")
    done < <(find "$log_dir" -maxdepth 1 -type f \( -name "*.log" -o -name "*.log.gz" \) -printf '%T@ %p\0' | sort -z -n | cut -z -d' ' -f2-)

    local total_count="${#all_logs[@]}"
    if [ "$total_count" -gt "$max_count" ]; then
        local excess=$(( total_count - max_count ))
        for (( i=0; i<excess; i++ )); do
            local victim="${all_logs[$i]}"
            rm -f "$victim"
        done
    fi

    local current_kb
    current_kb=$(du -s "$log_dir" 2>/dev/null | awk '{print $1}')
    local limit_kb=$(( max_mb * 1024 ))
    if [ -n "$current_kb" ] && [ "$current_kb" -gt "$limit_kb" ]; then
        while IFS= read -r -d $'\0' file; do
            rm -f "$file"
            current_kb=$(du -s "$log_dir" 2>/dev/null | awk '{print $1}')
            [ "$current_kb" -le "$limit_kb" ] && break
        done < <(find "$log_dir" -maxdepth 1 -type f \( -name "*.log" -o -name "*.log.gz" \) -printf '%T@ %p\0' | sort -z -n | cut -z -d' ' -f2-)
    fi
}

manage_turn_log_completion() {
    local turn_log="$1"
    if [ -z "$turn_log" ] || [ ! -f "$turn_log" ]; then
        return 0
    fi

    if [ "$AUTO_COMPRESS_LOGS" -eq 1 ] && command -v gzip >/dev/null 2>&1; then
        gzip -9 -f "$turn_log" 2>/dev/null || true
    fi

    cleanup_old_logs "$LOG_DIR" "$MAX_RETAINED_LOGS" "$MAX_LOG_DIR_MB"
}

# ------------------------------------------------------------------------------
# 宿主自动化 Git 提交函数 (AGENTS.md 规范)
# ------------------------------------------------------------------------------
auto_commit_subtask_changes() {
    local turn_num="$1"
    local last_msg_file="$2"

    if [ ! -d "${PROJECT_ROOT}/.git" ]; then
        return 0
    fi

    local status_output
    status_output=$(git -C "$PROJECT_ROOT" status --porcelain -- ':!automation' 2>/dev/null || true)

    if [ -z "$status_output" ]; then
        log_info "工作区无新增待提交代码改动，跳过自动 Git 提交。"
        return 0
    fi

    log_info "检测到本轮产生未提交代码改动，正在由宿主执行自动 Git 提交..."

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
        return f"feat(core): implement autonomous subtask {turn}"

    content = "\n".join(lines)
    test_filter = re.compile(
        r"(?i)(验证通过|验证结果|测试通过|测试结果|测试未改动|测试进程|tests?\s+passed|verified|"
        r"cargo\s+check|cargo\s+test|npm\s+test|clippy|playwright|browser\s+flows?|https\s+请求返回|"
        r"architecture\.md|已同步|未能提交|已尝试提交|改动保留在工作区|```)"
    )

    m_block = re.search(r"COMMIT_START\s*\n(.*?)\n\s*COMMIT_END", content, re.DOTALL | re.I)
    if m_block:
        block_lines = [l.strip() for l in m_block.group(1).splitlines() if l.strip()]
        clean_lines = [l for l in block_lines if not test_filter.search(l)]
        if clean_lines:
            return "\n".join(clean_lines)

    first = lines[0]
    first = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", first)
    first = re.sub(r"\*\*|__|[*`#]", "", first)
    first = re.sub(r"^[：:]+|[：:]+$", "", first).strip()
    first = re.sub(r"^(本轮|当前|本次)?(已完成|完成了|完成)[：:\s]*", "", first).strip()
    first = re.sub(r"^(feat|fix|refactor|chore|docs|test|style)[:\s]+", "", first, flags=re.I).strip()

    lower_first = first.lower()
    if "tls" in lower_first or "证书" in lower_first or "信任根" in lower_first:
        subject = "feat(remote): support TLS root certificate fallbacks and security options"
    elif "with_proxy" in lower_first:
        subject = "feat(service): add with_proxy subscription downloads via system proxy"
    elif "self_proxy" in lower_first:
        subject = "feat(service): add self_proxy subscription downloads via managed core"
    elif "cascade_delete" in lower_first or "级联删除" in lower_first:
        subject = "feat(profile): implement auxiliary profile cascade deletion"
    elif "import" in lower_first or "辅助项" in lower_first or "订阅" in lower_first:
        subject = "feat(profile): auto-create auxiliary profiles on subscription import"
    elif "merge" in lower_first or "script" in lower_first:
        subject = "feat(profile): support global merge and script enhancement"
    elif "dns" in lower_first or "tun" in lower_first:
        subject = "feat(config): support dns and tun configuration management"
    elif re.match(r"^[a-zA-Z0-9_\-\(\): ]+$", first):
        if not re.match(r"^(feat|fix|refactor|chore|docs|test)\b", first, re.I):
            subject = f"feat: {first[:65]}"
        else:
            subject = first
    else:
        en_words = re.findall(r"[a-zA-Z0-9_\-]+", first)
        if en_words:
            subject = f"feat: support {' '.join(en_words[:3])}"
        else:
            subject = f"feat(core): implement autonomous subtask {turn}"

    body_lines = []
    for l in lines[1:8]:
        if test_filter.search(l):
            continue
        if l.startswith(("-", "*", "•")) and re.match(r"^[-*•]\s*[a-zA-Z]", l):
            body_lines.append(l)

    if body_lines:
        return subject + "\n\n" + "\n".join(body_lines)
    return subject

print(build_commit())
PYEOF
    )

    if [ -z "$commit_msg" ]; then
        commit_msg="feat(turn-${turn_num}): complete autonomous subtask"
    fi

    git -C "$PROJECT_ROOT" add -A -- ':!automation'

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
# 5. 提示词构造与跨 Agent 状态交接桥 (Prompt Generator & Cross-Agent Handover)
# ------------------------------------------------------------------------------
build_handover_context() {
    local target_agent="$1"
    local prev_agent="上一任 Agent"
    if [ -f "$LAST_AGENT_FILE" ]; then
        local recorded_last
        recorded_last=$(cat "$LAST_AGENT_FILE" 2>/dev/null | tr -d '[:space:]')
        if [ "$recorded_last" = "codex" ]; then
            prev_agent="OpenAI Codex"
        elif [ "$recorded_last" = "agy" ]; then
            prev_agent="Google Antigravity CLI (agy)"
        fi
    else
        [ "$target_agent" = "agy" ] && prev_agent="OpenAI Codex"
        [ "$target_agent" = "codex" ] && prev_agent="Google Antigravity CLI (agy)"
    fi

    # 1. 查找最近一轮的交付总结文件 (按轮次序号数值降序取最大轮次)
    local last_msg_file
    last_msg_file=$(find "$LOG_DIR" -maxdepth 1 -name "turn_*_last_msg.txt" 2>/dev/null | sed -E 's/.*turn_([0-9]+)_last_msg\.txt/\1 &/' | sort -n | tail -n 1 | awk '{print $2}')
    local last_msg_content=""
    local last_turn_num="未知"
    if [ -n "$last_msg_file" ] && [ -f "$last_msg_file" ]; then
        last_turn_num=$(basename "$last_msg_file" | grep -oE '[0-9]+' | head -n 1)
        last_msg_content=$(cat "$last_msg_file")
    fi

    # 2. 检查当前工作区未提交的文件
    local unstaged_status
    unstaged_status=$(git -C "$PROJECT_ROOT" status -s -- ':!automation' 2>/dev/null || true)

    # 3. 提取最近 5 次 Git 提交
    local recent_commits
    recent_commits=$(git -C "$PROJECT_ROOT" log -n 5 --oneline 2>/dev/null || true)

    local handover_block=""
    handover_block+="================================================================================\n"
    handover_block+="【关键任务交接简报 - 跨 Agent 会话记忆与状态延续】\n"
    handover_block+="你当前正在接替 ${prev_agent} 继续推进本项目的无人值守自动化重构与开发工作。\n"
    handover_block+="为确保开发节奏、未完工代码与架构进度与前序轮次 100% 保持无缝延续，请特别注意以下交接事实：\n"
    handover_block+="--------------------------------------------------------------------------------\n"

    if [ -n "$unstaged_status" ]; then
        handover_block+="▶ 1. 【在途未提交代码警报 (In-Flight Worktree Changes)】：\n"
        handover_block+="当前工作区中保留了 ${prev_agent} 在上一轮中断前正在进行但未及完成提交的代码修改：\n"
        handover_block+="\`\`\`text\n${unstaged_status}\n\`\`\`\n"
        handover_block+="【交接核心要求】：请优先检查上述文件，运行 \`cargo check --workspace\` 和相关测试评估当前完成度。\n"
        handover_block+="如果这部分改动符合架构设计，请直接在此基础上补全测试并完成该子任务，切勿盲目丢弃或推倒重来！\n\n"
    else
        handover_block+="▶ 1. 当前工作区处于干净状态，所有前序子任务均已完成原子提交。\n\n"
    fi

    if [ -n "$last_msg_content" ]; then
        handover_block+="▶ 2. 【${prev_agent} 最近一轮 (Turn #${last_turn_num}) 的交付总结与后续任务指引】：\n"
        handover_block+="\`\`\`markdown\n${last_msg_content}\n\`\`\`\n\n"
    fi

    if [ -n "$recent_commits" ]; then
        handover_block+="▶ 3. 【近期 5 个切片的 Git 提交历史】：\n"
        handover_block+="\`\`\`text\n${recent_commits}\n\`\`\`\n"
    fi

    handover_block+="▶ 4. 【核心开发规律与行事风格继承（遵循前序轮次成熟节奏）】：\n"
    handover_block+="本项目采用“小步快跑、单轮单点、步步留痕、宿主提交、自动流转”的成熟规律。\n"
    handover_block+="每轮仅攻坚一个自包含的原子功能点/小任务，验证通过并更新 ARCHITECTURE.md 后，必须按照固定四段式（COMMIT块 -> 本轮实质总结 -> 验证结果 -> 下一任务指引）输出进度总结，随后结束本轮答复，由外部宿主自动提交 Git 并开启下一轮。切勿在单轮内过度发散或连续做多个功能点！\n"
    handover_block+="================================================================================\n"

    echo -e "$handover_block"
}

generate_initial_prompt() {
    local handover_info
    handover_info=$(build_handover_context "$AGENT_TYPE")

    cat <<EOF
$handover_info

你正在以完全无人值守、高自主性的方式推进本项目的架构重构与开发工作。
请仔细阅读并严格结合 ./headless.md 和 ./docs/ARCHITECTURE.md 中的要求执行工作。

【项目背景与目标】
1. 本项目目标是将 Clash Verge Rev 中的核心组件拆离并改造成无需桌面环境（无 Tauri/无外部服务依赖）的单个 headless service (mihomo-server)。
2. 该 service 是唯一系统服务入口，负责 Mihomo 内核生命周期管理、Axum HTTP 管理 API、WebSocket 事件转发、配置生成校验以及 Web UI 静态托管。
3. 遵循 ./docs/ARCHITECTURE.md 中记录的架构分层、状态标记（[Migrated]、[Implemented]、[Partially migrated]、[Pending] 等）和交付顺序 (Delivery order)，当前最高执行优先级为 P1（配置与资源管理）-> P2（规则与延迟）-> P3（i18n）-> P4（打包与 systemd）。
4. 【上游代码参考】：上游 clash-verge-rev 源码位于 ../clash-verge-rev（已在工作区中开放读取权限）。在拆离或移植算法（例如 enhance/配置增强、mihomo 交互通信、前端组件等）时，请直接阅读参考 ../clash-verge-rev 中的对应实现，保留原有业务行为。
5. 【真实节点验证环境】：当前 ./data 目录中存在实际可用的真实节点数据，可基于这些数据进行代理功能测试与回归验证，保证最终服务真实可用。

【核心开发节奏与行事风格规范（严格遵循前序轮次成熟规律）】
本项目采用“小步快跑、单轮单点、步步留痕、宿主提交、自动流转”的原子化研发规律，请严格恪守以下行为准则：

1. 【单轮聚焦单一功能点 (One Subtask Per Turn)】：
   - 每一轮仅从 ./docs/ARCHITECTURE.md 的交付顺序中挑选【一个自包含的、具体的原子功能点/小任务】进行开发。
   - 严禁单轮过度发散、大包大揽或一口气推进多个独立模块。做完一个功能点立即进入验证和总结，结束本轮答复交由外部调度器自动流转。

2. 【开发与验证闭环 (Implement & Verify)】：
   - 参考 ../clash-verge-rev 实现，修改相关 Rust/Web 代码。
   - 必须运行 \`cargo check --workspace\` 和相关测试验证，必要时结合 ./data 现有可用节点数据进行实际代理请求验证，保证代码可构建且直接可用。

3. 【同步更新架构文档 (Sync ARCHITECTURE.md)】：
   - 完成该小任务后，必须立即编辑 ./docs/ARCHITECTURE.md：
     - 更新 ## Complete target architecture 架构树中的组件状态标签（例如将 [Pending] 变更为 [Partially implemented] 或 [Implemented]）。
     - 更新文档顶部的 Latest completed task 与 Next implementation task，并在底部追加详细进展记录。

4. 【每轮结束时的固定四段式总结结构 (Mandatory 4-Part Summary)】：
   每完成一个功能点/小任务，必须在最终答复中严格按照以下四段式格式输出进度总结，随后结束本轮答复：

   [第 1 段 - 标准英文提交块]：
   COMMIT_START
   <type>(<scope>): <concise English summary of the change>

   - <key implementation detail 1 in English>
   - <key implementation detail 2 in English>
   COMMIT_END
   （注意：严禁在 COMMIT 块中包含“验证通过/Tests passed/cargo check/browser tests”等测试流水表述，只陈述实际代码与功能改动本身！）

   [第 2 段 - 本轮完成的功能点总结]：
   已完成本轮 [P1/P2/...] 子任务：<清晰陈述本轮实质完成的功能点或修复>。完整架构树与进度已同步更新至 [ARCHITECTURE.md](docs/ARCHITECTURE.md)。

   [第 3 段 - 验证与测试结果]：
   \`cargo check --workspace\` 通过；<具体测试项/浏览器回归/真实内核或节点验证情况>。

   [第 4 段 - 下一步任务指引与未完成状态声明]：
   项目尚未全部完成。**下一项 [P1/P2/...] 任务**是：<明确写出下一个具体待办功能点>。
   （重要完成判定：仅当整个项目的所有规划目标全部彻底完成并通过全盘验证时，才在最后一行输出特定完成标志：$COMPLETION_FLAG；若未完成，严禁输出该标志！）

5. 【轮次自然收敛与自动流转 (Turn Hand-off)】：
   - 输出上述四段式总结后，当前轮次即告结束（请勿继续调用工具或开始下一个功能点）。
   - 外部宿主调度器会自动从答复中提取 COMMIT 块在宿主机执行原子 Git commit，记录日志并保存本轮总结，然后自动拉起下一个 turn 继续推进，直到整个计划全部完成。
EOF
}

generate_continuation_prompt() {
    local extra_warning="$1"
    local handover_block=""
    if [ "${turn_count:-1}" -eq 1 ]; then
        local unstaged_status
        unstaged_status=$(git -C "$PROJECT_ROOT" status -s -- ':!automation' 2>/dev/null || true)
        local recorded_last=""
        [ -f "$LAST_AGENT_FILE" ] && recorded_last=$(cat "$LAST_AGENT_FILE" 2>/dev/null | tr -d '[:space:]')
        if [ -n "$unstaged_status" ] || [ -n "$recorded_last" -a "$recorded_last" != "$AGENT_TYPE" ]; then
            handover_block=$(build_handover_context "$AGENT_TYPE")
        fi
    fi

    local prefix=""
    [ -n "$handover_block" ] && prefix="${handover_block}

"

    cat <<EOF
${prefix}继续推进无人值守自动化重构与编程任务。
$extra_warning

【核心开发节奏与行事风格规范（严格遵循前序轮次成熟规律）】
本项目采用“小步快跑、单轮单点、步步留痕、宿主提交、自动流转”的成熟研发规律，请严格恪守以下行为准则：

1. 【单轮聚焦单一功能点 (One Subtask Per Turn)】：
   - 查看 ./docs/ARCHITECTURE.md 确认上一小任务状态，从交付顺序中选取【下一个自包含的、具体的原子功能点/子任务】继续编写代码。
   - 严禁单轮过度发散或做多个功能点，完成一个功能点立即验证并输出总结，结束本轮答复交由调度器自动流转。

2. 【开发与验证闭环 (Implement & Verify)】：
   - 参考 ../clash-verge-rev 源码实现业务逻辑。
   - 运行 \`cargo check --workspace\` 及相关测试，必要时结合 ./data 真实节点数据验证代理可用性。

3. 【同步更新架构文档 (Sync ARCHITECTURE.md)】：
   - 完成该小任务后，必须同步更新 ./docs/ARCHITECTURE.md 文件中的架构树状态与进度总结。

4. 【每轮结束时的固定四段式总结结构 (Mandatory 4-Part Summary)】：
   本轮小任务完成后，最终答复必须严格按照以下四段式格式输出：

   [第 1 段 - 标准英文提交块]：
   COMMIT_START
   <type>(<scope>): <concise English summary of code change>

   - <key technical change in English>
   COMMIT_END
   （严禁包含“验证通过/Tests passed/cargo test”等流水表述，只陈述实际代码与功能改动本身）

   [第 2 段 - 本轮完成的功能点总结]：
   已完成本轮 [P1/P2/...] 子任务：<清晰陈述本轮实质完成的功能点或修复>。完整架构树与进度已同步更新至 [ARCHITECTURE.md](docs/ARCHITECTURE.md)。

   [第 3 段 - 验证与测试结果]：
   \`cargo check --workspace\` 通过；<具体测试项/浏览器回归/真实内核或节点验证情况>。

   [第 4 段 - 下一步任务指引与未完成状态声明]：
   项目尚未全部完成。**下一项 [P1/P2/...] 任务**是：<明确写出下一个具体待办功能点>。
   （若全部计划已彻底完成并通过验证，才在最后一行输出：$COMPLETION_FLAG；若未完成，严禁输出该标志！）

5. 【轮次自然收敛与下一轮接力 (Turn Hand-off)】：
   - 输出上述总结后本轮结束，切勿在当前答复中继续开发下一个功能点。外部调度器将自动代为执行 Git 提交，并开启下一轮。
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
# 6. 单次 Agent 运行核心 (实时输出 + 10分钟看门狗监控)
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

    # 构建 Agent 专用命令数组
    local cmd=()
    if [ "$AGENT_TYPE" = "codex" ]; then
        cmd+=("$CODEX_BIN" "exec")
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
            if [ "$ENABLE_SANDBOX" -eq 1 ]; then
                cmd+=("-s" "$SANDBOX_MODE")
            fi
            if [ -d "$UPSTREAM_DIR" ]; then
                cmd+=("--add-dir" "$UPSTREAM_DIR")
            fi
        fi
        cmd+=(
            "-c" "approval_policy=${APPROVAL_POLICY}"
            "-c" "sandbox_workspace_write.network_access=true"
            "--skip-git-repo-check"
            "-o" "$last_msg_file"
            "$prompt_text"
        )
    elif [ "$AGENT_TYPE" = "agy" ]; then
        cmd+=("$AGY_BIN")
        if [ "$mode" = "resume" ]; then
            if [ -n "$target_session" ]; then
                cmd+=("--conversation" "$target_session")
            else
                cmd+=("--continue")
            fi
        elif [ "$mode" = "fork" ]; then
            if [ -n "$target_session" ]; then
                cmd+=("--conversation" "$target_session")
            fi
        fi
        if [ -d "$UPSTREAM_DIR" ]; then
            cmd+=("--add-dir" "$UPSTREAM_DIR")
        fi
        if [ "$ENABLE_SANDBOX" -eq 1 ]; then
            cmd+=("--sandbox")
        fi
        cmd+=(
            "--dangerously-skip-permissions"
            "--output-format" "stream-json"
            "-p" "$prompt_text"
        )
    fi

    log_info "启动 Turn #$turn_num [Agent: $AGENT_TYPE | 模式: $mode${target_session:+, 会话: $target_session}]"
    log_info "实时输出已建立，同时记录于: $turn_log"

    touch "$turn_log"
    if [ -f "${SCRIPT_DIR}/format_codex_stream.py" ] && command -v python3 >/dev/null 2>&1; then
        python3 "${SCRIPT_DIR}/format_codex_stream.py" \
            --agent "$AGENT_TYPE" \
            --log-file "$turn_log" \
            --session-file "$SESSION_FILE" \
            --last-msg-file "$last_msg_file" < "$fifo_path" &
        CURRENT_TEE_PID=$!
    else
        tee -a "$turn_log" < "$fifo_path" &
        CURRENT_TEE_PID=$!
    fi

    if command -v stdbuf >/dev/null 2>&1; then
        stdbuf -oL -eL "${cmd[@]}" > "$fifo_path" 2>&1 &
    else
        "${cmd[@]}" > "$fifo_path" 2>&1 &
    fi
    CURRENT_AGENT_PID=$!

    if [ "$ENABLE_RESOURCE_LIMITS" -eq 1 ]; then
        if [ -n "$CPU_AFFINITY" ] && command -v taskset >/dev/null 2>&1; then
            taskset -cp "$CPU_AFFINITY" "$CURRENT_AGENT_PID" >/dev/null 2>&1 || true
        fi
        renice -n "$PROCESS_NICE" -p "$CURRENT_AGENT_PID" >/dev/null 2>&1 || true
        if command -v ionice >/dev/null 2>&1; then
            if [ "$PROCESS_IONICE_CLASS" -eq 2 ]; then
                ionice -c 2 -n "$PROCESS_IONICE_PRIO" -p "$CURRENT_AGENT_PID" >/dev/null 2>&1 || true
            else
                ionice -c "$PROCESS_IONICE_CLASS" -p "$CURRENT_AGENT_PID" >/dev/null 2>&1 || true
            fi
        fi
    fi

    # 启动 10 分钟看门狗
    (
        while kill -0 "$CURRENT_AGENT_PID" 2>/dev/null; do
            sleep 5
            local last_mod
            last_mod=$(stat -c %Y "$turn_log" 2>/dev/null || date +%s)
            local now
            now=$(date +%s)
            local inactive_secs=$(( now - last_mod ))

            if [ "$inactive_secs" -ge "$INACTIVITY_TIMEOUT" ]; then
                echo -e "\n${CLR_RED}================================================================================"
                echo -e "[WATCHDOG TIMEOUT] 监测到当前操作超过 ${INACTIVITY_TIMEOUT} 秒 ($(( INACTIVITY_TIMEOUT / 60 )) 分钟) 未产生任何输出或响应！"
                echo -e "[WATCHDOG TIMEOUT] 判定为任务卡死/网络悬挂，正在强制中断 PID: $CURRENT_AGENT_PID 并触发自动重试..."
                echo -e "================================================================================${CLR_RESET}"
                touch "$timeout_marker"
                kill_tree "$CURRENT_AGENT_PID" TERM
                sleep 4
                kill_tree "$CURRENT_AGENT_PID" KILL
                exit 0
            fi
        done
    ) &
    CURRENT_WATCHDOG_PID=$!

    local exit_code=0
    wait "$CURRENT_AGENT_PID" 2>/dev/null || exit_code=$?
    CURRENT_AGENT_PID=""

    if [ -n "$CURRENT_WATCHDOG_PID" ]; then
        kill -9 "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
        wait "$CURRENT_WATCHDOG_PID" 2>/dev/null || true
        CURRENT_WATCHDOG_PID=""
    fi

    if [ -n "$CURRENT_TEE_PID" ]; then
        wait "$CURRENT_TEE_PID" 2>/dev/null || true
        CURRENT_TEE_PID=""
    fi
    rm -f "$fifo_path"
    CURRENT_FIFO=""

    local was_timeout=0
    if [ -f "$timeout_marker" ]; then
        was_timeout=1
        rm -f "$timeout_marker"
    fi

    local new_session_id=""
    if [ -f "$turn_log" ]; then
        new_session_id=$(extract_session_id_from_log "$turn_log")
    fi
    if [ -n "$new_session_id" ]; then
        save_session_id "$new_session_id"
    fi

    echo "$AGENT_TYPE" > "$LAST_AGENT_FILE"

    LAST_EXIT_CODE=$exit_code
    LAST_WAS_TIMEOUT=$was_timeout
    LAST_LOG_FILE="$turn_log"
    LAST_MSG_FILE="$last_msg_file"
}

# ------------------------------------------------------------------------------
# 7. 主执行循环 (Autonomous Main Loop)
# ------------------------------------------------------------------------------
main() {
    local force_new=0
    local custom_session=""
    local custom_fork=0

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --agent)
                AGENT_TYPE="$2"
                shift 2
                ;;
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
                if [ "$AGENT_TYPE" = "codex" ]; then
                    log_info "正在重启本地 Codex app-server daemon..."
                    "$CODEX_BIN" app-server daemon restart >/dev/null 2>&1 || true
                else
                    log_info "Antigravity CLI 无需独立 daemon 重启。"
                fi
                shift
                ;;
            --clean-logs)
                log_info "正在清理过期的历史全量日志 (*.log, *.log.gz)，保留最近 1 轮日志与全部交付摘要文件..."
                cleanup_old_logs "$LOG_DIR" 1 "$MAX_LOG_DIR_MB"
                log_success "日志清理完成！当前 logs 目录占用: $(du -sh "$LOG_DIR" 2>/dev/null | awk '{print $1}')"
                exit 0
                ;;
            --max-logs)
                MAX_RETAINED_LOGS="$2"
                shift 2
                ;;
            --flag)
                COMPLETION_FLAG="$2"
                shift 2
                ;;
            --cpu-affinity)
                CPU_AFFINITY="$2"
                shift 2
                ;;
            --cargo-jobs)
                CARGO_JOBS="$2"
                shift 2
                ;;
            --nice)
                PROCESS_NICE="$2"
                shift 2
                ;;
            --no-limit)
                ENABLE_RESOURCE_LIMITS=0
                shift
                ;;
            --no-sandbox)
                ENABLE_SANDBOX=0
                shift
                ;;
            -h|--help)
                echo "用法: $0 [选项]"
                echo "选项:"
                echo "  --agent <类型>         选择代理工具 (支持: codex, agy / antigravity; 默认: codex)"
                echo "  --new                  强制新建独立会话，不继续之前会话"
                echo "  --session <ID>         指定要恢复或分叉的 Agent 会话 UUID"
                echo "  --fork [ID]            从指定或最近的会话分叉出新会话（避免与已打开的交互终端冲突）"
                echo "  --restart-daemon       重启本地 Codex daemon 并清理所有残留会话锁"
                echo "  --clean-logs           清理过期的历史全量日志，仅保留最近日志与交付摘要文件"
                echo "  --max-logs <数量>      设置最多保留的历史全量日志数 (默认: 5)"
                echo "  --cpu-affinity <核心>  绑定运行的 CPU 核心 (默认: 2-15，空出 0-1 预留给 Samba 与前台网页浏览)"
                echo "  --cargo-jobs <并发数>  设置 Cargo 编译与测试最大并发数 (16核默认: 12)"
                echo "  --nice <数值>          设置 CPU 调度优先级 Nice 值 (默认: 10，温和让位)"
                echo "  --no-limit             禁用全部 CPU 与资源调度限制"
                echo "  --no-sandbox           禁用隔离沙箱模式 (允许直接访问宿主系统，注意安全)"
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

    AGENT_TYPE="$(echo "$AGENT_TYPE" | tr '[:upper:]' '[:lower:]')"
    if [ "$AGENT_TYPE" = "antigravity" ]; then
        AGENT_TYPE="agy"
    fi

    validate_agent_bin
    apply_resource_limits

    local agent_label="OpenAI Codex"
    [ "$AGENT_TYPE" = "agy" ] && agent_label="Google Antigravity CLI (agy)"

    log_box "mihomo-server 无人值守自动化 Agent 编程工作台已启动" "$CLR_GREEN"
    log_info "代理工具引擎: ${CLR_BOLD}${agent_label}${CLR_RESET} (--agent $AGENT_TYPE)"
    log_info "工作区目录: $PROJECT_ROOT"
    log_info "上游源码库: $UPSTREAM_DIR $([ -d "$UPSTREAM_DIR" ] && echo -e "${CLR_GREEN}[有效目录，已开放跨库读取]${CLR_RESET}" || echo -e "${CLR_YELLOW}[未找到该目录]${CLR_RESET}")"
    local sandbox_display="已启用 (隔离工作区保护)"
    [ "$ENABLE_SANDBOX" -ne 1 ] && sandbox_display="已禁用 (直接宿主访问，注意安全)"
    log_info "运行配置: 沙箱=${sandbox_display} | 自动审批=always/never | 无响应超时=${INACTIVITY_TIMEOUT}s | 冷却时限=${RATE_LIMIT_COOLDOWN}s | 日志保留=${MAX_RETAINED_LOGS}轮"
    local res_status="已禁用"
    [ "$ENABLE_RESOURCE_LIMITS" -eq 1 ] && res_status="已启用 (CPU亲和度: ${CPU_AFFINITY:-全部}, Nice: $PROCESS_NICE, IOClass: $PROCESS_IONICE_CLASS, Cargo并发: $CARGO_JOBS)"
    log_info "资源调度限制: $res_status"
    log_info "计划完成标志: $COMPLETION_FLAG"
    log_info "提示: 任何时候均可按 Ctrl+C 安全中断退出。"

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
        save_session_id "$active_session"
    elif [ -f "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" ] && [ -s "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" ]; then
        current_mode="resume"
        active_session=$(cat "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" | tr -d '[:space:]')
    elif [ -f "$SESSION_FILE" ] && [ -s "$SESSION_FILE" ]; then
        current_mode="resume"
        active_session=$(cat "$SESSION_FILE" | tr -d '[:space:]')
    else
        local latest_recorded
        latest_recorded=$(get_latest_session_id)
        if [ -n "$latest_recorded" ]; then
            current_mode="resume"
            active_session="$latest_recorded"
            save_session_id "$active_session"
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
        log_box ">>> 开始第 $turn_count 轮子任务执行 [$agent_label] <<<" "$CLR_CYAN"

        local arch_hash_before
        arch_hash_before=$(get_arch_hash)

        local current_prompt=""
        if [ "$consecutive_timeouts" -gt 0 ]; then
            current_prompt=$(generate_timeout_retry_prompt)
        elif [[ "$turn_count" -eq 1 && "$current_mode" = "new" ]]; then
            current_prompt=$(generate_initial_prompt)
        else
            current_prompt=$(generate_continuation_prompt "$last_sync_warning")
        fi
        last_sync_warning=""

        LAST_EXIT_CODE=0
        LAST_WAS_TIMEOUT=0
        LAST_LOG_FILE=""
        LAST_MSG_FILE=""

        run_single_turn "$turn_count" "$current_prompt" "$current_mode" "$active_session"

        if [ -f "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" ] && [ -s "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" ]; then
            active_session=$(cat "${SCRIPT_DIR}/.session_id_${AGENT_TYPE}" | tr -d '[:space:]')
            current_mode="resume"
        elif [ -f "$SESSION_FILE" ] && [ -s "$SESSION_FILE" ]; then
            active_session=$(cat "$SESSION_FILE" | tr -d '[:space:]')
            current_mode="resume"
        fi

        # 处理超时
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

        # 监测限额
        if check_rate_limit "$LAST_EXIT_CODE" "$LAST_LOG_FILE" "$LAST_MSG_FILE"; then
            local IS_RATE_LIMITED=0
            local WAIT_SECONDS="$RATE_LIMIT_COOLDOWN"
            local RESET_TIME=""
            local RESET_SOURCE=""
            local RESET_DETAIL=""
            eval "$(get_rate_limit_info "$LAST_LOG_FILE" "$LAST_MSG_FILE" "$RATE_LIMIT_COOLDOWN")"

            if [ "$IS_RATE_LIMITED" -eq 1 ] && [ "$WAIT_SECONDS" -gt 0 ]; then
                echo ""
                log_box "【检测到触发使用限额 / Rate Limit 配额上限】" "$CLR_RED"
                log_warn "限额详情: ${CLR_BOLD}${RESET_DETAIL}${CLR_RESET}"
                log_info "预计恢复时间: ${CLR_BOLD}${RESET_TIME}${CLR_RESET} (来源: ${RESET_SOURCE})"
                local wait_h=$(( WAIT_SECONDS / 3600 ))
                local wait_m=$(( (WAIT_SECONDS % 3600) / 60 ))
                local wait_s=$(( WAIT_SECONDS % 60 ))
                log_info "动态计算等待时长: ${CLR_BOLD}${wait_h}小时 ${wait_m}分钟 ${wait_s}秒${CLR_RESET} (已计入安全缓冲)"

                wait_with_countdown "$WAIT_SECONDS" "等待配额刷新重置 ($RESET_TIME)"

                log_info "已成功到达配额重置时间点，自动重新拉起 Agent 会话..."
                consecutive_timeouts=0
                turn_count=$(( turn_count + 1 ))
                continue
            fi
        fi

        # 监测会话冲突
        if check_active_writer_conflict "$LAST_LOG_FILE"; then
            consecutive_lock_conflicts=$(( consecutive_lock_conflicts + 1 ))
            log_warn "检测到会话锁定冲突 (already has an active writer: ${active_session:-当前会话})！"

            if has_foreground_codex_cli; then
                log_warn "系统检测到仍有其他终端正在前台运行交互式 Codex 进程。"
                log_info "自动应对策略: 正在从该会话分叉 (Fork) 一个新会话..."
                current_mode="fork"
                sleep 2
                continue
            fi

            if [ "$consecutive_lock_conflicts" -le 2 ]; then
                log_info "正在自动重启本地 Codex daemon 释放残留会话锁并立即重试 (第 $consecutive_lock_conflicts 次重试)..."
                "$CODEX_BIN" app-server daemon restart >/dev/null 2>&1 || true
                sleep 2
                current_mode="resume"
                continue
            else
                log_warn "重启 daemon 后重试仍遇到会话锁定冲突，自动切换为分叉模式..."
                current_mode="fork"
                consecutive_lock_conflicts=0
                sleep 2
                continue
            fi
        fi

        consecutive_lock_conflicts=0

        # 检查 ARCHITECTURE.md 更新
        local arch_hash_after
        arch_hash_after=$(get_arch_hash)
        if [ "$arch_hash_before" != "$arch_hash_after" ]; then
            log_success "【进度已同步】检测到 ./docs/ARCHITECTURE.md 已成功更新进度！"
        else
            log_warn "【进度未同步告警】本轮子任务结束后，./docs/ARCHITECTURE.md 未检测到修改！"
            last_sync_warning="【注意：上一轮任务完成后未检测到 ./docs/ARCHITECTURE.md 更新，请在本轮务必更新 ARCHITECTURE.md 同步最新架构与完成状态！】"
        fi

        # 宿主自动化 Git 提交
        auto_commit_subtask_changes "$turn_count" "$LAST_MSG_FILE"

        # 检查全部计划是否达成
        if check_all_tasks_completed "$LAST_MSG_FILE"; then
            echo ""
            log_box "🎉🎉🎉 【项目全部计划圆满完成】 🎉🎉🎉" "$CLR_GREEN"
            log_success "成功检测到全部任务完成标志: ${CLR_BOLD}${COMPLETION_FLAG}${CLR_RESET}"
            log_success "所有在 ./headless.md 与 ./docs/ARCHITECTURE.md 中规划的架构目标与功能已全部交付并验证完成！"
            log_info "总执行轮次: $turn_count"
            manage_turn_log_completion "$LAST_LOG_FILE"
            log_info "最终日志文件: ${LAST_LOG_FILE}.gz"
            exit 0
        fi

        # 日志生命周期治理
        manage_turn_log_completion "$LAST_LOG_FILE"

        consecutive_timeouts=0
        turn_count=$(( turn_count + 1 ))

        log_info "Turn #$(( turn_count - 1 )) 执行完毕。短暂休整 5 秒后继续推进下一子任务..."
        sleep 5
    done
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
