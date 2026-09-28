#!/usr/bin/env python3
# ==============================================================================
# format_codex_stream.py
#
# 为 Codex CLI 与 Antigravity CLI (agy) 提供仿原生 TUI 的实时流式折叠渲染与动态 Loading 效果。
#
# 功能特性:
# 1. 实时动态 Loading 动画 (Spinner + 当前操作说明 + 耗时计时器)
# 2. 长命令输出与大文本折叠显示 (避免终端平铺刷屏，超长输出仅保留首尾若干行)
# 3. 原始数据 100% 实时原样落盘至日志文件 (保证看门狗与限额检测正常工作)
# 4. 全状态覆盖的补丁/Git Diff 自动折叠捕获 (杜绝任何代码差异泄露导致刷屏覆盖进度)
# 5. 双引擎智能识别：自动解析 Codex 纯文本流与 Antigravity CLI 的 stream-json NDJSON 流
# 6. 会话追踪与最终交付摘要自动落盘，支持多种 Agent CLI 无缝切换
# ==============================================================================

import os
import re
import sys
import time
import json
import signal
import threading
import argparse

# ANSI 颜色定义
CLR_RESET   = "\033[0m"
CLR_BOLD    = "\033[1m"
CLR_DIM     = "\033[2m"
CLR_RED     = "\033[1;31m"
CLR_GREEN   = "\033[1;32m"
CLR_YELLOW  = "\033[1;33m"
CLR_BLUE    = "\033[1;34m"
CLR_MAGENTA = "\033[1;35m"
CLR_CYAN    = "\033[1;36m"
CLR_GRAY    = "\033[90m"

SPINNER_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

# 正则匹配真实命令状态行，例如: " succeeded in 2072ms:" 或 " exited 101 in 4572ms:"
STATUS_RE = re.compile(r"^\s*(succeeded|exited\s+\d+)\s+in\s+.*:", re.IGNORECASE)


class TerminalRenderer:
    def __init__(self, raw_log_path=None, is_tty=True, default_msg="正在启动代理服务..."):
        self.raw_log_path = raw_log_path
        self.raw_log_file = None
        if self.raw_log_path:
            self.raw_log_file = open(self.raw_log_path, "a", encoding="utf-8", buffering=1)

        self.is_tty = is_tty
        self.lock = threading.Lock()
        self.running = True
        self.status_msg = default_msg
        self.start_time = time.time()
        self.action_start_time = time.time()
        self.spinner_thread = None

        if self.is_tty:
            self.spinner_thread = threading.Thread(target=self._spinner_loop, daemon=True)
            self.spinner_thread.start()

    def set_status(self, msg):
        with self.lock:
            self.status_msg = msg
            self.action_start_time = time.time()

    def _spinner_loop(self):
        idx = 0
        while self.running:
            with self.lock:
                now = time.time()
                action_elapsed = int(now - self.action_start_time)
                total_elapsed = int(now - self.start_time)
                frame = SPINNER_FRAMES[idx % len(SPINNER_FRAMES)]

                # 单行动态刷新
                status_line = (
                    f"\r\033[K{CLR_CYAN}{frame}{CLR_RESET} "
                    f"{self.status_msg} "
                    f"{CLR_GRAY}[耗时: {action_elapsed:02d}s | 总计: {total_elapsed//60:02d}:{total_elapsed%60:02d}]{CLR_RESET}"
                )
                sys.stdout.write(status_line)
                sys.stdout.flush()
            idx += 1
            time.sleep(0.08)

    def print_block(self, text):
        """清空当前动态行并打印固定块内容"""
        with self.lock:
            if self.is_tty:
                sys.stdout.write("\r\033[K")
            sys.stdout.write(text)
            if not text.endswith("\n"):
                sys.stdout.write("\n")
            sys.stdout.flush()

    def write_raw(self, raw_line):
        if self.raw_log_file:
            self.raw_log_file.write(raw_line)
            self.raw_log_file.flush()

    def close(self):
        self.running = False
        if self.spinner_thread and self.spinner_thread.is_alive():
            self.spinner_thread.join(timeout=0.3)
        if self.is_tty:
            sys.stdout.write("\r\033[K")
            sys.stdout.flush()
        if self.raw_log_file:
            self.raw_log_file.close()


def clean_command_str(raw_cmd):
    """去除 wrapper 路径，提取清晰可读的真实命令摘要"""
    raw_cmd = raw_cmd.strip()
    if not raw_cmd:
        return ""

    # 匹配 /usr/bin/zsh -lc '...' in /dir
    m = re.search(r"/(?:usr/)?bin/(?:ba|z)?sh\s+-lc\s+(.*)", raw_cmd, re.DOTALL)
    if m:
        inner = m.group(1).strip()
        inner = re.sub(r"\s+in\s+/[^\n]*$", "", inner).strip()
        if (inner.startswith("'") and inner.endswith("'")) or (inner.startswith('"') and inner.endswith('"')):
            inner = inner[1:-1].strip()
        elif inner.startswith("'") or inner.startswith('"'):
            inner = inner[1:].strip()
            if inner.endswith("'") or inner.endswith('"'):
                inner = inner[:-1].strip()

        lines = [l.strip() for l in inner.splitlines() if l.strip()]
        if not lines:
            return inner
        first = lines[0]
        if len(lines) > 1:
            last = lines[-1]
            if any(k in last for k in ["cargo", "npm", "python", "test"]):
                return f"{first} ➜ {last}"
            return f"{first} (...共 {len(lines)} 行)"
        return first

    # 匹配 ... in /dir
    m2 = re.search(r"^(.*?)\s+in\s+/[^/\s].*$", raw_cmd, re.DOTALL)
    if m2:
        inner = m2.group(1).strip()
        lines = [l.strip() for l in inner.splitlines() if l.strip()]
        if lines:
            if len(lines) > 1:
                return f"{lines[0]} (...共 {len(lines)} 行)"
            return lines[0]
        return inner

    lines = [l.strip() for l in raw_cmd.splitlines() if l.strip()]
    if lines:
        if len(lines) > 1:
            return f"{lines[0]} (...共 {len(lines)} 行)"
        return lines[0]
    return raw_cmd


def fold_output_lines(lines, max_display=6):
    """对超长输出进行折叠"""
    if len(lines) <= max_display:
        return [f"    {CLR_DIM}{line}{CLR_RESET}" for line in lines]

    first_lines = lines[:2]
    last_lines = lines[-2:]
    folded_count = len(lines) - 4

    result = [f"    {CLR_DIM}{l}{CLR_RESET}" for l in first_lines]
    result.append(f"    {CLR_YELLOW}{CLR_DIM}┄┄┄ [已折叠 {folded_count} 行长输出，完整内容详见日志] ┄┄┄{CLR_RESET}")
    result.extend([f"    {CLR_DIM}{l}{CLR_RESET}" for l in last_lines])
    return result


def is_diff_start(line):
    """检测是否为补丁或 unified git diff 的起始行"""
    s = line.strip()
    return s.startswith("diff --git ") or s == "apply patch" or s.startswith("apply patch ") or s == "patch: completed"


def is_diff_line(raw_line):
    """判断当前行是否属于正在进行的 unified git diff 或 patch 流"""
    if STATUS_RE.match(raw_line):
        return False
    s = raw_line.strip()
    if s in ("user", "codex", "exec"):
        return False
    if s.startswith("tokens used") or s.startswith("OpenAI Codex"):
        return False
    if raw_line.startswith("diff --git ") or s.startswith("diff --git "):
        return True
    if raw_line.startswith("index ") or raw_line.startswith("--- ") or raw_line.startswith("+++ "):
        return True
    if raw_line.startswith("@@ ") or (raw_line.startswith("@@") and "@@" in raw_line[2:]):
        return True
    if any(raw_line.startswith(k) for k in ("old mode ", "new mode ", "new file mode ", "deleted file mode ", "similarity index ", "rename from ", "rename to ")):
        return True
    if raw_line.startswith("Binary files ") and "differ" in raw_line:
        return True
    if raw_line.startswith("\\ No newline at end of file"):
        return True
    if raw_line.startswith("+") or raw_line.startswith("-"):
        return True
    if raw_line.startswith(" ") or raw_line == "\n" or raw_line == "\r\n":
        return True
    if s == "patch: completed" or s.startswith("apply patch"):
        return True
    if s.startswith("/") and not s.startswith("//") and (" " not in s) and ("/" in s):
        return True
    return False


def fold_code_blocks(text):
    """折叠文本中过长的 markdown 代码块 (```...```)"""
    def replacer(match):
        code = match.group(1)
        clines = code.splitlines()
        if len(clines) > 8:
            kept = clines[:2] + [f"    ┄┄┄ [已折叠 {len(clines)-4} 行代码块] ┄┄┄"] + clines[-2:]
            return "```\n" + "\n".join(kept) + "\n```"
        return match.group(0)
    return re.sub(r"```[^\n]*\n(.*?)\n```", replacer, text, flags=re.DOTALL)


# ==============================================================================
# Codex CLI 流式输出处理器 (状态机实现)
# ==============================================================================
def process_codex_stream(renderer, args, first_line=""):
    is_interrupted = False

    def sig_handler(sig, frame):
        nonlocal is_interrupted
        is_interrupted = True

    signal.signal(signal.SIGINT, sig_handler)
    signal.signal(signal.SIGTERM, sig_handler)

    state = "INIT"
    header_lines = []
    user_prompt_lines = []
    codex_msg_lines = []
    current_cmd_lines = []
    cmd_status = ""
    cmd_output_lines = []
    patch_lines = []
    ready_lines = []

    last_patch_signature = None
    is_explicit_patch = False
    last_reported_patch_files = set()
    last_codex_msg_clean = ""

    captured_session_id = None
    captured_model_name = "Codex"
    turn_interrupted = False

    def save_session_id(sid):
        if sid and args.session_file:
            try:
                with open(args.session_file, "w", encoding="utf-8") as sf:
                    sf.write(sid.strip() + "\n")
            except Exception:
                pass

    def flush_user_prompt():
        nonlocal user_prompt_lines
        if user_prompt_lines:
            non_empty = [l.strip() for l in user_prompt_lines if l.strip()]
            first = non_empty[0] if non_empty else "推进任务"
            if len(first) > 60:
                first = first[:57] + "..."
            line_count = len(user_prompt_lines)
            block = f"\n{CLR_BLUE}👤 [用户指令]{CLR_RESET} {CLR_BOLD}{first}{CLR_RESET} {CLR_DIM}(提示词共 {line_count} 行已折叠){CLR_RESET}\n"
            renderer.print_block(block)
            user_prompt_lines = []

    def flush_codex_msg():
        nonlocal codex_msg_lines, last_codex_msg_clean
        if codex_msg_lines:
            clean_lines = []
            for l in codex_msg_lines:
                if l.startswith("diff --git ") or l.startswith("index ") or l.startswith("--- ") or l.startswith("+++ ") or l.startswith("@@ "):
                    continue
                clean_lines.append(l)
            raw_text = "\n".join(clean_lines).strip()
            if raw_text:
                norm_text = re.sub(r"\s+", " ", raw_text)
                if norm_text == last_codex_msg_clean:
                    codex_msg_lines = []
                    return
                last_codex_msg_clean = norm_text

                text = fold_code_blocks(raw_text)
                lines = text.splitlines()
                if len(lines) > 20:
                    shown = lines[:8] + [f"  {CLR_YELLOW}{CLR_DIM}┄┄┄ [已折叠 {len(lines)-12} 行说明，完整内容详见日志] ┄┄┄{CLR_RESET}"] + lines[-4:]
                else:
                    shown = lines
                block = f"\n{CLR_CYAN}💬 [Codex 答复/分析]{CLR_RESET}\n"
                for line in shown:
                    block += f"  {line}\n"
                renderer.print_block(block)
            codex_msg_lines = []

    def flush_patch_block():
        nonlocal patch_lines, last_patch_signature, is_explicit_patch, last_reported_patch_files
        if patch_lines:
            files = []
            for l in patch_lines:
                l_str = l.strip()
                if l_str.startswith("+++ b/"):
                    files.append(l_str[6:].strip())
                elif l_str.startswith("diff --git a/"):
                    m = re.search(r"diff --git a/(.*?) b/", l_str)
                    if m:
                        files.append(m.group(1).strip())
                elif l_str.startswith("/") and not l_str.startswith("//") and not l_str.startswith("---") and not l_str.startswith("+++") and " " not in l_str:
                    parts = l_str.split("/")
                    if any(k in parts for k in ["service", "crates", "web"]):
                        idx = min([parts.index(k) for k in ["service", "crates", "web"] if k in parts])
                        files.append("/".join(parts[idx:]))
                    elif "." in os.path.basename(l_str):
                        files.append(os.path.basename(l_str))
            seen = set()
            uniq_files = [f for f in files if not (f in seen or seen.add(f))]
            file_summary = ", ".join(uniq_files) if uniq_files else "代码文件"
            line_count = len(patch_lines)
            sig = (tuple(uniq_files), line_count)

            should_print = False
            if is_explicit_patch:
                should_print = True
                last_reported_patch_files = set(uniq_files)
            elif uniq_files:
                if not set(uniq_files).issubset(last_reported_patch_files):
                    should_print = True
                    last_reported_patch_files.update(uniq_files)
                elif sig != last_patch_signature and not last_reported_patch_files:
                    should_print = True
                    last_reported_patch_files.update(uniq_files)

            if should_print and sig != last_patch_signature:
                last_patch_signature = sig
                block = f"\n{CLR_MAGENTA}🔧 [代码补丁]{CLR_RESET} {CLR_BOLD}更新 {file_summary}{CLR_RESET} {CLR_DIM}(补丁差异共 {line_count} 行已折叠){CLR_RESET}\n"
                renderer.print_block(block)

            patch_lines = []
            is_explicit_patch = False

    def flush_exec_block():
        nonlocal current_cmd_lines, cmd_status, cmd_output_lines
        if current_cmd_lines:
            raw_full = "\n".join(current_cmd_lines)
            cmd_display = clean_command_str(raw_full)
            duration_match = re.search(r"in\s+(\d+(?:\.\d+)?(?:ms|s)):", cmd_status)
            dur_str = f" ({duration_match.group(1)})" if duration_match else ""

            is_success = bool(re.search(r"\bsucceeded\b|\bexited\s+0\b", cmd_status, re.IGNORECASE))
            is_failed = bool(re.search(r"\bexited\s+([1-9]\d*)\b", cmd_status, re.IGNORECASE))

            if is_success:
                icon = f"{CLR_GREEN}✔ [命令成功]{CLR_RESET}"
                header = f"{icon} {CLR_BOLD}{cmd_display}{CLR_RESET}{CLR_DIM}{dur_str}{CLR_RESET}"
            elif is_failed:
                icon = f"{CLR_RED}✖ [命令异常]{CLR_RESET}"
                header = f"{icon} {CLR_BOLD}{cmd_display}{CLR_RESET}{CLR_RED}{dur_str}{CLR_RESET}"
            else:
                icon = f"{CLR_BLUE}• [执行操作]{CLR_RESET}"
                header = f"{icon} {CLR_BOLD}{cmd_display}{CLR_RESET}{CLR_DIM}{dur_str}{CLR_RESET}"

            out_text = "\n".join(fold_output_lines(cmd_output_lines, max_display=6))
            if out_text:
                block = f"{header}\n{out_text}\n"
            else:
                block = f"{header}\n"
            renderer.print_block(block)

            current_cmd_lines = []
            cmd_status = ""
            cmd_output_lines = []

    def flush_ready_block():
        nonlocal ready_lines
        if ready_lines:
            ready_text = "\n".join(ready_lines).strip()
            norm_ready = re.sub(r"\s+", " ", ready_text)
            if norm_ready and norm_ready == last_codex_msg_clean:
                ready_lines = []
                return
            out_text = "\n".join(fold_output_lines(ready_lines, max_display=4))
            if out_text:
                renderer.print_block(out_text)
            ready_lines = []

    def flush_all_blocks():
        flush_user_prompt()
        flush_codex_msg()
        flush_exec_block()
        flush_patch_block()
        flush_ready_block()

    buffered_lines = [first_line] if first_line else []

    try:
        renderer.set_status("正在与 Codex 建立连接...")
        while True:
            if buffered_lines:
                line = buffered_lines.pop(0)
            else:
                line = sys.stdin.readline()
                if not line:
                    break
            renderer.write_raw(line)
            stripped = line.rstrip("\r\n")

            sess_match = re.search(r"session id:\s*([0-9a-fA-F-]+)", stripped, re.IGNORECASE)
            if sess_match:
                captured_session_id = sess_match.group(1).strip()
                save_session_id(captured_session_id)

            if "turn interrupted" in stripped.lower():
                turn_interrupted = True
                is_interrupted = True
                flush_all_blocks()
                renderer.print_block(
                    f"\n{CLR_YELLOW}⚡ [Codex 原生退出]{CLR_RESET} {CLR_BOLD}turn interrupted (会话已被人工中断){CLR_RESET}\n"
                )
                continue

            if stripped == "user":
                flush_all_blocks()
                state = "USER"
                user_prompt_lines = []
                renderer.set_status("正在接收处理指令...")
                continue
            elif stripped == "codex":
                flush_all_blocks()
                state = "CODEX"
                renderer.set_status("Codex 正在思考与生成...")
                continue
            elif stripped == "exec":
                flush_all_blocks()
                state = "EXEC"
                renderer.set_status("正在准备执行终端操作...")
                continue
            elif stripped == "apply patch" or stripped.startswith("apply patch"):
                flush_all_blocks()
                state = "PATCH"
                is_explicit_patch = True
                patch_lines.append(stripped)
                renderer.set_status("正在应用代码补丁 (Patch)...")
                continue

            if is_diff_start(line):
                if state == "CODEX":
                    flush_codex_msg()
                    state = "PATCH"
                    patch_lines.append(line)
                    renderer.set_status("正在整理代码补丁与变更...")
                    continue
                elif state == "EXEC":
                    state = "EXEC_DIFF"
                    patch_lines.append(line)
                    continue
                elif state == "EXEC_OUTPUT":
                    flush_exec_block()
                    state = "PATCH"
                    patch_lines.append(line)
                    continue
                elif state in ("PATCH", "EXEC_DIFF"):
                    patch_lines.append(line)
                    continue
                elif state in ("READY", "INIT"):
                    flush_ready_block()
                    state = "PATCH"
                    patch_lines.append(line)
                    continue

            if state == "PATCH":
                if STATUS_RE.match(line):
                    flush_patch_block()
                    cmd_status = stripped.strip()
                    state = "EXEC_OUTPUT"
                    continue
                elif is_diff_line(line):
                    patch_lines.append(line)
                    continue
                else:
                    flush_patch_block()
                    state = "READY"

            if state == "EXEC_DIFF":
                if is_diff_line(line):
                    patch_lines.append(line)
                    continue
                else:
                    flush_patch_block()
                    if STATUS_RE.match(stripped):
                        cmd_status = stripped.strip()
                        state = "EXEC_OUTPUT"
                        continue
                    else:
                        state = "EXEC"

            if state == "INIT":
                if stripped.startswith("OpenAI Codex"):
                    header_lines.append(stripped)
                    state = "HEADER"
                    continue
                elif stripped:
                    continue

            if state == "HEADER":
                header_lines.append(stripped)
                if stripped == "--------" and len(header_lines) > 2:
                    model_match = re.search(r"model:\s*(\S+)", "\n".join(header_lines))
                    sess_hdr_match = re.search(r"session id:\s*([0-9a-fA-F-]+)", "\n".join(header_lines), re.IGNORECASE)
                    if model_match:
                        captured_model_name = model_match.group(1)
                    if sess_hdr_match:
                        captured_session_id = sess_hdr_match.group(1).strip()
                        save_session_id(captured_session_id)

                    sess_display = captured_session_id if captured_session_id else "unknown"
                    hdr_text = "\n".join(header_lines)
                    net_enabled = "network access enabled" in hdr_text
                    sandbox_status = f"{CLR_GREEN}workspace-write (已联网){CLR_RESET}" if net_enabled else f"{CLR_GREEN}workspace-write{CLR_RESET}"
                    badge = (
                        f"{CLR_BLUE}╭─ Codex 会话已就绪 ────────────────────────────────────────────────────────{CLR_RESET}\n"
                        f"{CLR_BLUE}│{CLR_RESET} 模型: {CLR_BOLD}{captured_model_name}{CLR_RESET} | 会话: {CLR_BOLD}{sess_display}{CLR_RESET} | 沙箱: {sandbox_status}\n"
                        f"{CLR_BLUE}╰───────────────────────────────────────────────────────────────────────────{CLR_RESET}"
                    )
                    renderer.print_block(badge)
                    header_lines = []
                    state = "READY"
                continue

            if state == "USER":
                if stripped:
                    user_prompt_lines.append(stripped)
                continue

            if state == "CODEX":
                codex_msg_lines.append(stripped)
                renderer.set_status("Codex 正在组织回复...")
                continue

            if state == "EXEC":
                if STATUS_RE.match(stripped):
                    cmd_status = stripped.strip()
                    state = "EXEC_OUTPUT"
                else:
                    current_cmd_lines.append(stripped)
                    if current_cmd_lines:
                        short_cmd = clean_command_str(current_cmd_lines[0])
                        if len(short_cmd) > 40:
                            short_cmd = short_cmd[:37] + "..."
                        renderer.set_status(f"正在执行: {short_cmd}")
                continue

            if state == "EXEC_OUTPUT":
                if STATUS_RE.match(stripped):
                    flush_exec_block()
                    cmd_status = stripped.strip()
                    state = "EXEC_OUTPUT"
                else:
                    cmd_output_lines.append(stripped)
                continue

            if state == "READY":
                if STATUS_RE.match(line):
                    cmd_status = stripped.strip()
                    state = "EXEC_OUTPUT"
                    continue
                if stripped:
                    if stripped.startswith("tokens used") or re.match(r"^[\d,]+$", stripped):
                        continue
                    ready_lines.append(stripped)
                continue

        flush_all_blocks()

        total_sec = int(time.time() - renderer.start_time)
        if turn_interrupted or is_interrupted:
            renderer.print_block(
                f"{CLR_YELLOW}⚠ [交互已人工中断]{CLR_RESET} {CLR_DIM}已运行: {total_sec//60}分{total_sec%60}秒{CLR_RESET}"
            )
            if captured_session_id:
                banner = (
                    f"\n{CLR_YELLOW}╭─ Codex 会话中断信息 (Session Info) ──────────────────────────────────────{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET} session id: {CLR_BOLD}{captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET} 原生退出状态: {CLR_BOLD}turn interrupted{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET} 后续接着会话继续运行命令：\n"
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 自动化续跑:   {CLR_GREEN}{CLR_BOLD}./automation/run_autonomous_codex.sh --agent codex --session {captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 交互式恢复:   {CLR_BLUE}{CLR_BOLD}codex resume {captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 针对性单次指令追加: {CLR_MAGENTA}{CLR_BOLD}codex exec resume {captured_session_id} \"指令\" {CLR_RESET}\n"
                    f"{CLR_YELLOW}╰───────────────────────────────────────────────────────────────────────────{CLR_RESET}\n"
                )
                renderer.print_block(banner)
        else:
            renderer.print_block(
                f"{CLR_GREEN}✔ [本轮交互完成]{CLR_RESET} {CLR_DIM}用时: {total_sec//60}分{total_sec%60}秒{CLR_RESET}"
            )
    finally:
        renderer.close()


# ==============================================================================
# Antigravity CLI (agy) 流式输出处理器 (NDJSON stream-json 模式)
# ==============================================================================
def process_agy_stream(renderer, args, first_line=""):
    is_interrupted = False
    captured_session_id = None
    curr_response_chunks = []
    has_shown_user = False

    def sig_handler(sig, frame):
        nonlocal is_interrupted
        is_interrupted = True

    signal.signal(signal.SIGINT, sig_handler)
    signal.signal(signal.SIGTERM, sig_handler)

    def save_session_id(sid):
        nonlocal captured_session_id
        if sid:
            captured_session_id = sid.strip()
            if args.session_file:
                try:
                    with open(args.session_file, "w", encoding="utf-8") as sf:
                        sf.write(captured_session_id + "\n")
                except Exception:
                    pass

    def save_last_msg(resp):
        if resp and args.last_msg_file:
            try:
                with open(args.last_msg_file, "w", encoding="utf-8") as mf:
                    mf.write(resp)
            except Exception:
                pass

    def handle_json_obj(data):
        nonlocal has_shown_user, curr_response_chunks, captured_session_id
        event = data.get("event")

        # 1. 会话初始化事件
        if event == "init":
            conv_id = data.get("conversation_id")
            save_session_id(conv_id)
            init_info = data.get("init", {})
            perm_mode = init_info.get("permission_mode", "always-proceed")
            banner = (
                f"{CLR_BLUE}╭─ Antigravity (agy) 会话已就绪 ────────────────────────────────────────{CLR_RESET}\n"
                f"{CLR_BLUE}│{CLR_RESET} 工具: {CLR_BOLD}Antigravity CLI{CLR_RESET} | 会话: {CLR_BOLD}{conv_id or 'unknown'}{CLR_RESET} | 权限: {CLR_GREEN}{perm_mode}{CLR_RESET}\n"
                f"{CLR_BLUE}╰───────────────────────────────────────────────────────────────────────────{CLR_RESET}"
            )
            renderer.print_block(banner)
            renderer.set_status("正在接收并解析任务指令...")
            return

        # 2. 单步状态推进事件
        if event == "step_update":
            update = data.get("step_update", {})
            conv_id = update.get("conversation_id")
            if conv_id and not captured_session_id:
                save_session_id(conv_id)

            step_type = update.get("step_type")
            state = update.get("state")

            # 用户输入
            if step_type == "user_input":
                if not has_shown_user:
                    has_shown_user = True
                    renderer.print_block(
                        f"\n{CLR_BLUE}👤 [用户指令]{CLR_RESET} {CLR_BOLD}推进无人值守自动化架构重构与开发任务{CLR_RESET} {CLR_DIM}(指令已就绪){CLR_RESET}\n"
                    )
                return

            # 模型思考
            if step_type == "thinking":
                renderer.set_status("Antigravity 正在深度思考分析...")
                return

            # 模型答复与文本流
            if step_type == "agent_response":
                if state == "ACTIVE":
                    delta = update.get("text_delta", "")
                    curr_response_chunks.append(delta)
                    renderer.set_status("Antigravity 正在组织回复...")
                elif state == "DONE":
                    full_resp = "".join(curr_response_chunks).strip()
                    curr_response_chunks = []
                    if full_resp:
                        text = fold_code_blocks(full_resp)
                        lines = text.splitlines()
                        if len(lines) > 20:
                            shown = lines[:8] + [f"  {CLR_YELLOW}{CLR_DIM}┄┄┄ [已折叠 {len(lines)-12} 行说明，完整内容详见日志] ┄┄┄{CLR_RESET}"] + lines[-4:]
                        else:
                            shown = lines
                        block = f"\n{CLR_CYAN}💬 [Antigravity 答复/分析]{CLR_RESET}\n"
                        for line in shown:
                            block += f"  {line}\n"
                        renderer.print_block(block)
                return

            # 工具调用与结果
            if step_type == "tool":
                tool_name = update.get("tool_name", "")
                tool_info = update.get("tool_info", {})
                params = tool_info.get("parameters", {})

                if state == "ACTIVE":
                    if tool_name == "run_command":
                        cmd = params.get("CommandLine", "")
                        clean_cmd = clean_command_str(cmd)
                        short_cmd = clean_cmd[:45] + "..." if len(clean_cmd) > 45 else clean_cmd
                        renderer.set_status(f"正在执行: {short_cmd}")
                    elif tool_name in ("replace_file_content", "write_to_file", "sed_file"):
                        target = params.get("TargetFile") or params.get("file_path") or ""
                        renderer.set_status(f"正在修改文件: {os.path.basename(target)}")
                    elif tool_name in ("view_file", "grep_search", "find_by_name", "list_dir", "read_url_content"):
                        target = params.get("AbsolutePath") or params.get("TargetFile") or params.get("path") or ""
                        desc = os.path.basename(target) if target else tool_name
                        renderer.set_status(f"正在检索/查看: {desc}")
                    else:
                        renderer.set_status(f"正在调用工具: {tool_name}")
                elif state == "DONE":
                    dur = update.get("duration_seconds")
                    dur_str = f" ({dur:.2f}s)" if dur is not None else ""
                    if tool_name == "run_command":
                        cmd = params.get("CommandLine", "")
                        clean_cmd = clean_command_str(cmd)
                        output = tool_info.get("output", "")
                        out_lines = [l.strip() for l in output.splitlines() if l.strip()]

                        is_failed = bool(re.search(r"exited with code [1-9]\d*", output))
                        if is_failed:
                            icon = f"{CLR_RED}✖ [命令异常]{CLR_RESET}"
                            header = f"{icon} {CLR_BOLD}{clean_cmd}{CLR_RESET}{CLR_RED}{dur_str}{CLR_RESET}"
                        else:
                            icon = f"{CLR_GREEN}✔ [命令成功]{CLR_RESET}"
                            header = f"{icon} {CLR_BOLD}{clean_cmd}{CLR_RESET}{CLR_DIM}{dur_str}{CLR_RESET}"

                        folded = fold_output_lines(out_lines, max_display=6)
                        if folded:
                            block = f"{header}\n" + "\n".join(folded)
                        else:
                            block = header
                        renderer.print_block(block)
                    elif tool_name in ("replace_file_content", "write_to_file", "sed_file"):
                        target = params.get("TargetFile") or params.get("file_path") or "代码文件"
                        action = "更新" if tool_name == "replace_file_content" else "写入"
                        block = f"\n{CLR_MAGENTA}🔧 [代码补丁]{CLR_RESET} {CLR_BOLD}{action} {os.path.basename(target)}{CLR_RESET}{CLR_DIM}{dur_str}{CLR_RESET}\n"
                        renderer.print_block(block)
                    elif tool_name in ("view_file", "grep_search", "find_by_name", "list_dir"):
                        target = params.get("AbsolutePath") or params.get("TargetFile") or params.get("path") or ""
                        desc = os.path.basename(target) if target else tool_name
                        renderer.print_block(f"{CLR_BLUE}• [代码查阅]{CLR_RESET} {desc}{CLR_DIM}{dur_str}{CLR_RESET}")
                return

        # 3. 最终轮次结果事件
        if event == "result":
            res = data.get("result", {})
            conv_id = res.get("conversation_id")
            if conv_id:
                save_session_id(conv_id)
            response_text = res.get("response", "")
            save_last_msg(response_text)
            return

    buffered_lines = [first_line] if first_line else []
    try:
        renderer.set_status("正在与 Antigravity 建立连接...")
        while True:
            if buffered_lines:
                line = buffered_lines.pop(0)
            else:
                line = sys.stdin.readline()
                if not line:
                    break

            renderer.write_raw(line)
            stripped = line.strip()
            if not stripped:
                continue

            cid_match = re.search(r'"conversation_id"\s*:\s*"([0-9a-fA-F-]+)"', stripped)
            if cid_match:
                save_session_id(cid_match.group(1))

            try:
                obj = json.loads(stripped)
                if isinstance(obj, dict) and "event" in obj:
                    handle_json_obj(obj)
            except json.JSONDecodeError:
                if any(k in stripped.lower() for k in ("error", "fatal", "panic", "quota", "exhausted")):
                    renderer.print_block(f"{CLR_RED}{stripped}{CLR_RESET}")

        total_sec = int(time.time() - renderer.start_time)
        if is_interrupted:
            renderer.print_block(
                f"{CLR_YELLOW}⚠ [交互已人工中断]{CLR_RESET} {CLR_DIM}已运行: {total_sec//60}分{total_sec%60}秒{CLR_RESET}"
            )
            if captured_session_id:
                banner = (
                    f"\n{CLR_YELLOW}╭─ Antigravity 会话中断信息 (Session Info) ───────────────────────────────{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET} conversation id: {CLR_BOLD}{captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET} 后续接着会话继续运行命令：\n"
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 自动化续跑:   {CLR_GREEN}{CLR_BOLD}./automation/run_autonomous_codex.sh --agent agy --session {captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 交互式恢复:   {CLR_BLUE}{CLR_BOLD}agy --conversation {captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 针对性单次指令追加: {CLR_MAGENTA}{CLR_BOLD}agy --conversation {captured_session_id} -p \"指令\" {CLR_RESET}\n"
                    f"{CLR_YELLOW}╰───────────────────────────────────────────────────────────────────────────{CLR_RESET}\n"
                )
                renderer.print_block(banner)
        else:
            renderer.print_block(
                f"{CLR_GREEN}✔ [本轮交互完成]{CLR_RESET} {CLR_DIM}用时: {total_sec//60}分{total_sec%60}秒{CLR_RESET}"
            )
    finally:
        renderer.close()


def main():
    parser = argparse.ArgumentParser(description="Autonomous Stream Formatter (Codex & Antigravity)")
    parser.add_argument("--log-file", help="Path to write complete raw output")
    parser.add_argument("--session-file", help="Path to write captured session id")
    parser.add_argument("--last-msg-file", help="Path to write last agent response")
    parser.add_argument("--agent", default="auto", choices=["auto", "codex", "agy", "antigravity"], help="Agent mode (auto|codex|agy)")
    args = parser.parse_args()

    is_tty = sys.stdout.isatty()
    renderer = TerminalRenderer(raw_log_path=args.log_file, is_tty=is_tty)

    # 预读第一行非空输出以决定解析器
    first_line = ""
    while True:
        line = sys.stdin.readline()
        if not line:
            break
        if line.strip():
            first_line = line
            break
        else:
            renderer.write_raw(line)

    if not first_line:
        renderer.close()
        return

    # 确定运行模式
    agent_mode = args.agent
    if agent_mode == "auto":
        # 尝试检测第一行是否包含 JSON event
        stripped = first_line.strip()
        if stripped.startswith("{") and '"event"' in stripped:
            try:
                d = json.loads(stripped)
                if isinstance(d, dict) and "event" in d:
                    agent_mode = "agy"
            except Exception:
                pass
        if agent_mode == "auto":
            agent_mode = "codex"

    if agent_mode in ("agy", "antigravity"):
        process_agy_stream(renderer, args, first_line=first_line)
    else:
        process_codex_stream(renderer, args, first_line=first_line)


if __name__ == "__main__":
    main()
