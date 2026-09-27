# mihomo-server 无人值守自动化 Codex 编程工作台

本目录集中管理服务重构期间的无人值守自动化脚本、会话追踪工具及日志记录，实现与业务源码的完全隔离。

---

## 目录结构

```text
automation/
├── run_autonomous_codex.sh   # 宿主自动化主调度器 (会话推进/看门狗/限额冷却/Git自动提交)
├── format_codex_stream.py    # 实时流式渲染与终端 TUI 折叠动画格式化器
├── logs/                     # 历史每轮子任务的执行全量日志与最终答复摘要 (.gitignore)
├── .session_id               # 当前自动化任务绑定的最新 Codex 会话 UUID (.gitignore)
└── README.md                 # 工作台使用说明文档
```

---

## 快速使用

### 1. 启动或继续自动化开发
```bash
# 默认模式：自动恢复上一次的最新会话继续推进子任务
./automation/run_autonomous_codex.sh

# 强制开启全新独立会话
./automation/run_autonomous_codex.sh --new

# 指定恢复特定会话 UUID
./automation/run_autonomous_codex.sh --session <UUID>

# 从已有会话分叉 (Fork) 出新分支 (避免终端会话锁定冲突)
./automation/run_autonomous_codex.sh --fork [UUID]

# 运行日志清理 (保持轻量，保留摘要与最近全量日志)
./automation/run_autonomous_codex.sh --clean-logs

# 自定义最多保留的最近全量日志轮数 (默认: 5)
./automation/run_autonomous_codex.sh --max-logs 3
```

### 2. 随时安全中断与追查会话
任何时候在终端按 `Ctrl+C` 即可优雅终止。退出时终端会自动打印出当前会话的 `session id`，方便后续无缝恢复。

---

## 核心机制设计

1. **沙箱模式与上游跨库读取**：
   - 采用 `--sandbox workspace-write` 配合 `-c sandbox_workspace_write.network_access=true`。
   - 自动挂载 `../clash-verge-rev` 为只读代码库，供 Agent 实时查阅上游核心实现。

2. **宿主级 Git 自动化原子提交 (解决沙箱 `.git` 只读限制)**：
   - Linux Bubblewrap 沙箱出于安全防逃逸设计，强制将 `.git` 挂载为只读。
   - 调度器直接运行在宿主机上，每轮子任务完成并验证通过（且同步更新 `docs/ARCHITECTURE.md`）后，宿主调度器自动提取子任务总结并执行原子提交，严格遵循 `AGENTS.md` 规范。

3. **动态 5 小时限额精确重置**：
   - 自动解析 OpenAI API 官方报错及 App-Server 返回的精准 `resetsAt` 时间戳。
   - 遭遇限额时显示毫秒级倒计时，到达重置点后自动唤醒重跑，无需盲目等待 5 小时。

4. **10 分钟看门狗监控**：
   - 监测长时间挂起或死锁请求，超时后自动执行进程树清理与自适应重试。

5. **日志瘦身与生命周期治理 (去冗余、防膨胀)**：
   - **Codex 原生追查支持**：Codex 底层已将完整的会话消息、模型推理、所有工具执行详情持久化在 `~/.codex/thread_history_1.sqlite` 中。任何历史会话均可随时通过 `codex resume <session-id>` 原生回溯追查，完全没有必要在项目目录保留两份冗余的全量长日志。
   - **自动化运行期瘦身**：调度器日志主要用于实时看门狗心跳、错误捕获与限额倒计时。每轮任务完成后，全量日志会自动进行 **gzip 压缩**（压缩率达 90%+），并根据 FIFO 规则**滚动保留最近 5 轮**日志，总目录上限限制在 15MB 以内。
   - **轻量交付摘要永久保留**：每轮最终的交付总结 `turn_X_last_msg.txt`（每份仅 3~8 KB）及 Git Commit 记录将被永久完整保留，实现超轻量级的全流程审计追踪。
