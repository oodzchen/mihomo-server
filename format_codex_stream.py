#!/usr/bin/env python3
# ==============================================================================
# format_codex_stream.py
#
# 为 Codex CLI (exec 模式) 提供仿原生 TUI 的实时流式折叠渲染与动态 Loading 效果。
#
# 功能特性:
# 1. 实时动态 Loading 动画 (Spinner + 当前操作说明 + 耗时计时器)
# 2. 长命令输出与大文本折叠显示 (避免终端平铺刷屏，超长输出仅保留首尾若干行)
# 3. 原始数据 100% 实时原样落盘至日志文件 (保证看门狗与限额检测正常工作)
# 4. 美化 Codex 答复、指令执行和状态展示
# ==============================================================================

import os
import re
import sys
import time
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

class TerminalRenderer:
    def __init__(self, raw_log_path=None, is_tty=True):
        self.raw_log_path = raw_log_path
        self.raw_log_file = None
        if self.raw_log_path:
            self.raw_log_file = open(self.raw_log_path, "a", encoding="utf-8", buffering=1)

        self.is_tty = is_tty
        self.lock = threading.Lock()
        self.running = True
        self.status_msg = "正在启动 Codex..."
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
    """去除 wrapper 路径，提取真实执行命令"""
    raw_cmd = raw_cmd.strip()
    # 匹配 /usr/bin/zsh -lc '...' in /dir
    m = re.search(r"/(?:usr/)?bin/(?:ba|z)?sh\s+-lc\s+([\"'].*[\"'])(?:\s+in\s+.*)?$", raw_cmd)
    if m:
        inner = m.group(1).strip()
        if (inner.startswith("'") and inner.endswith("'")) or (inner.startswith('"') and inner.endswith('"')):
            inner = inner[1:-1]
        return inner
    # 匹配 ... in /dir
    m2 = re.search(r"^(.*?)\s+in\s+/[^/\s].*$", raw_cmd)
    if m2:
        return m2.group(1).strip()
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


def main():
    parser = argparse.ArgumentParser(description="Codex Stream Formatter")
    parser.add_argument("--log-file", help="Path to write complete raw output")
    parser.add_argument("--session-file", help="Path to write captured session id")
    args = parser.parse_args()

    # 处理中断信号：标记中断，但不立即强杀，以便读取并冲刷 Codex 的 turn interrupted 退出输出
    is_interrupted = False
    def sig_handler(sig, frame):
        nonlocal is_interrupted
        is_interrupted = True

    signal.signal(signal.SIGINT, sig_handler)
    signal.signal(signal.SIGTERM, sig_handler)

    is_tty = sys.stdout.isatty()
    renderer = TerminalRenderer(raw_log_path=args.log_file, is_tty=is_tty)

    state = "INIT"
    header_lines = []
    user_prompt_lines = []
    codex_msg_lines = []
    current_cmd = ""
    cmd_status = ""
    cmd_output_lines = []

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
        nonlocal codex_msg_lines
        if codex_msg_lines:
            text = "\n".join(codex_msg_lines).strip()
            if text:
                block = f"\n{CLR_CYAN}💬 [Codex 答复/分析]{CLR_RESET}\n"
                for line in text.splitlines():
                    block += f"  {line}\n"
                renderer.print_block(block)
            codex_msg_lines = []

    def flush_exec_block():
        nonlocal current_cmd, cmd_status, cmd_output_lines
        if current_cmd:
            cmd_display = clean_command_str(current_cmd)
            duration_match = re.search(r"in\s+(\d+(?:\.\d+)?(?:ms|s)):", cmd_status)
            dur_str = f" ({duration_match.group(1)})" if duration_match else ""

            is_success = "succeeded" in cmd_status.lower()
            if is_success:
                icon = f"{CLR_GREEN}✔ [命令成功]{CLR_RESET}"
                header = f"{icon} {CLR_BOLD}{cmd_display}{CLR_RESET}{CLR_DIM}{dur_str}{CLR_RESET}"
            else:
                icon = f"{CLR_RED}✖ [命令异常]{CLR_RESET}"
                header = f"{icon} {CLR_BOLD}{cmd_display}{CLR_RESET}{CLR_RED}{dur_str}{CLR_RESET}"

            out_text = "\n".join(fold_output_lines(cmd_output_lines, max_display=6))
            if out_text:
                block = f"{header}\n{out_text}\n"
            else:
                block = f"{header}\n"
            renderer.print_block(block)

            current_cmd = ""
            cmd_status = ""
            cmd_output_lines = []

    try:
        renderer.set_status("正在与 Codex 建立连接...")
        while True:
            line = sys.stdin.readline()
            if not line:
                break
            renderer.write_raw(line)
            stripped = line.rstrip("\r\n")

            # 无论处于何种状态，只要出现 session id 即刻提取并持久化
            sess_match = re.search(r"session id:\s*([0-9a-fA-F-]+)", stripped, re.IGNORECASE)
            if sess_match:
                captured_session_id = sess_match.group(1).strip()
                save_session_id(captured_session_id)

            # 捕捉 Codex 退出或中断标记
            if "turn interrupted" in stripped.lower():
                turn_interrupted = True
                is_interrupted = True
                flush_user_prompt()
                flush_codex_msg()
                flush_exec_block()
                renderer.print_block(
                    f"\n{CLR_YELLOW}⚡ [Codex 原生退出]{CLR_RESET} {CLR_BOLD}turn interrupted (会话已被人工中断){CLR_RESET}\n"
                )
                continue

            # ------------------------------------------------------------------
            # 状态机解析
            # ------------------------------------------------------------------
            if state == "INIT":
                if stripped.startswith("OpenAI Codex"):
                    header_lines.append(stripped)
                    state = "HEADER"
                    continue
                elif stripped == "user":
                    state = "USER"
                    user_prompt_lines = []
                    continue
                elif stripped == "codex":
                    state = "CODEX"
                    renderer.set_status("Codex 正在生成分析与答复...")
                    continue
                elif stripped == "exec":
                    state = "EXEC_CMD"
                    renderer.set_status("准备执行系统命令...")
                    continue
                else:
                    if stripped:
                        renderer.print_block(f"  {CLR_DIM}{stripped}{CLR_RESET}")
                    continue

            if state == "HEADER":
                header_lines.append(stripped)
                if stripped == "--------" and len(header_lines) > 2:
                    # 提炼 Header 关键信息展示徽标 (保留完整的 36 位 UUID 会话 ID)
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

            # 检测段落切换标签
            if stripped == "user":
                flush_user_prompt()
                flush_codex_msg()
                flush_exec_block()
                state = "USER"
                user_prompt_lines = []
                renderer.set_status("正在接收处理指令...")
                continue
            elif stripped == "codex":
                flush_user_prompt()
                flush_exec_block()
                state = "CODEX"
                renderer.set_status("Codex 正在思考与生成...")
                continue
            elif stripped == "exec":
                flush_user_prompt()
                flush_codex_msg()
                flush_exec_block()
                state = "EXEC_CMD"
                renderer.set_status("正在准备执行终端操作...")
                continue

            # 用户输入折叠
            if state == "USER":
                if stripped:
                    user_prompt_lines.append(stripped)
                continue

            # Codex 答复
            if state == "CODEX":
                codex_msg_lines.append(stripped)
                renderer.set_status("Codex 正在组织回复...")
                continue

            # 命令执行状态
            if state == "EXEC_CMD":
                current_cmd = stripped
                short_cmd = clean_command_str(current_cmd)
                if len(short_cmd) > 40:
                    short_cmd = short_cmd[:37] + "..."
                renderer.set_status(f"正在执行: {short_cmd}")
                state = "EXEC_STATUS"
                continue

            if state == "EXEC_STATUS":
                cmd_status = stripped
                state = "EXEC_OUTPUT"
                continue

            if state == "EXEC_OUTPUT":
                cmd_output_lines.append(stripped)
                continue

            # READY 状态下的非标签输出 (如错误日志、告警等)
            if state == "READY":
                if stripped:
                    renderer.print_block(f"  {CLR_DIM}{stripped}{CLR_RESET}")
                continue

        # 循环结束，冲刷未输出内容
        flush_user_prompt()
        flush_codex_msg()
        flush_exec_block()

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
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 自动化续跑:   {CLR_GREEN}{CLR_BOLD}./run_autonomous_codex.sh --session {captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}│{CLR_RESET}   ▶ 交互式恢复:   {CLR_BLUE}{CLR_BOLD}codex resume {captured_session_id}{CLR_RESET}\n"
                    f"{CLR_YELLOW}╰───────────────────────────────────────────────────────────────────────────{CLR_RESET}\n"
                )
                renderer.print_block(banner)
        else:
            renderer.print_block(
                f"{CLR_GREEN}✔ [本轮交互完成]{CLR_RESET} {CLR_DIM}用时: {total_sec//60}分{total_sec%60}秒{CLR_RESET}"
            )
    finally:
        renderer.close()


if __name__ == "__main__":
    main()
