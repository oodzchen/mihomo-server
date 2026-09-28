# mihomo-server 无人值守自动化 Agent 编程工作台

本目录集中管理服务重构期间的无人值守自动化脚本、会话追踪工具及日志记录，实现与业务源码的完全隔离。

工作台现已全面升级为**多代理工具引擎**架构，原生支持 **Google Antigravity CLI (`agy`)** 与 **OpenAI Codex CLI (`codex`)**，并保证双引擎在无人值守、流式渲染、看门狗看护与 Git 自动原子提交等方面具备完全一致的运行效果。

---

## 目录结构

```text
automation/
├── run_autonomous_codex.sh   # 宿主自动化主调度器 (软链接: run_autonomous.sh)
├── run_autonomous.sh         # 通用主调度器入口
├── format_codex_stream.py    # 实时流式渲染与终端 TUI 折叠动画格式化器 (支持 Codex 文本流与 Antigravity stream-json)
├── logs/                     # 历史每轮子任务的执行全量日志与最终答复摘要 (.gitignore)
├── .session_id               # 最近激活的 Agent 会话 UUID (.gitignore)
├── .session_id_codex         # Codex 专属最新会话 UUID (.gitignore)
├── .session_id_agy           # Antigravity 专属最新会话 UUID (.gitignore)
└── README.md                 # 工作台使用说明文档
```

---

## 快速使用

### 1. 代理工具选择 (`--agent`)

工作台支持自由选择不同的底层代理工具，两者均享有完全一致的无人值守体验与资源调度保障：

```bash
# 模式 A: 使用 Google Antigravity CLI (推荐)
./automation/run_autonomous_codex.sh --agent agy
# 或使用通用别名入口:
./automation/run_autonomous.sh --agent agy

# 模式 B: 使用 OpenAI Codex CLI (默认)
./automation/run_autonomous_codex.sh --agent codex
```

也可以通过环境变量指定默认代理工具：
```bash
export AGENT_TOOL=agy
./automation/run_autonomous.sh
```

### 2. 会话生命周期控制

```bash
# 默认模式：自动恢复对应 Agent 上一次的最新会话继续推进子任务
./automation/run_autonomous.sh --agent agy

# 强制开启全新独立会话
./automation/run_autonomous.sh --agent agy --new

# 指定恢复特定会话 UUID
./automation/run_autonomous.sh --agent agy --session <UUID>

# 从已有会话分叉 (Fork) 出新分支 (避免终端会话锁定冲突)
./automation/run_autonomous.sh --fork [UUID]

# 运行日志清理 (保持轻量，保留摘要与最近全量日志)
./automation/run_autonomous.sh --clean-logs

# 自定义最多保留的最近全量日志轮数 (默认: 5)
./automation/run_autonomous.sh --max-logs 3
```

### 3. 系统资源调度与防抢占 (保护 Samba 媒体服务与宿主桌面响应)
仅预留 CPU 0-1 专供 Samba 与日常操作，其余全部算力分配给开发编译：
```bash
# 默认已自动开启: 绑定 CPU 2-15, Nice=10 (温和让位), Ionice=Best Effort, Cargo 并发=12
./automation/run_autonomous.sh --agent agy

# 自定义绑定核心与编译并发数
./automation/run_autonomous.sh --agent agy --cpu-affinity 4-15 --cargo-jobs 10

# 完全禁用资源限制 (全核极速编译模式)
./automation/run_autonomous.sh --no-limit
```

### 4. 随时安全中断与追查会话
任何时候在终端按 `Ctrl+C` 即可优雅终止。退出时终端会根据当前选择的 Agent 自动打印出专属的恢复与调试命令：
- **Antigravity CLI**:
  - 继续无人值守运行: `./automation/run_autonomous.sh --agent agy --session <UUID>`
  - 交互式终端恢复: `agy --conversation <UUID>`
  - 单次指令追加: `agy --conversation <UUID> -p "你的指令"`
- **Codex CLI**:
  - 继续无人值守运行: `./automation/run_autonomous.sh --agent codex --session <UUID>`
  - 交互式终端恢复: `codex resume <UUID>`
  - 单次指令追加: `codex exec resume <UUID> "你的指令"`

---

## 核心机制设计

| 维度 | OpenAI Codex 引擎 | Google Antigravity CLI 引擎 | 调度器统一表现 |
| :--- | :--- | :--- | :--- |
| **执行权限** | `-c approval_policy=never` 自动批准 | `--dangerously-skip-permissions` 自动批准 | 全程无人值守，无任何审批阻塞 |
| **隔离沙箱与安全** | `--sandbox workspace-write` (Bubblewrap 命名空间隔离) | `--sandbox` (Linux 命名空间隔离，系统只读保护) | 双引擎默认全部开启隔离沙箱，外部系统只读保护，防止越权逃逸；支持 `--no-sandbox` 显式关闭 |
| **上游跨库只读** | `--add-dir ../clash-verge-rev` | `--add-dir ../clash-verge-rev` | 跨库只读查阅上游核心实现，禁止修改 |
| **终端 TUI 流式渲染** | 正则匹配状态机、代码 Diff 折叠与命令去重 | 原生解析 `stream-json` NDJSON 事件流 | 统一动态 Spinner、耗时统计、命令与代码折叠展示 |
| **会话隔离** | `.session_id_codex` | `.session_id_agy` | 双引擎各自持久化最新会话，互不干扰 |
| **10分钟看门狗** | 监测 `turn_log` 文件 mtime，超时安全中断重试 | 同左 | 避免任何死循环、网络悬挂或长时间无响应 |
| **限额智能冷却** | 解析 OpenAI API 报错与 app-server 5小时重置点 | 解析 Google Cloud / Gemini 配额与 429 报错 | 毫秒级倒计时，到期后自动重新拉起会话 |
| **跨 Agent 状态交接** | 捕获在途未提交代码、交付总结与提交历史 | 接收前任 Agent 交接简报并优先接盘验证 | 无缝续接开发，解决 5 小时限额切引擎时上下文断代问题 |
| **宿主 Git 自动提交** | 宿主机在轮次结束后提取 Conventional Commit 自动提交 | 同左 | 完美解决沙箱内 `.git` 只读限制，遵循 `AGENTS.md` |
| **进度更新与完成判定** | 检测 `docs/ARCHITECTURE.md` 与完成标志 | 同左 | 保证每次子任务推进均同步架构文档 |

---

## 跨 Agent 上下文无缝交接桥 (Cross-Agent Handover Bridge)

当某个 Agent（如 Codex）在执行中途达到 5 小时用量上限，用户可随时无缝切换到备用 Agent（如 `agy`）：

1. **不可互通的私有会话**：Codex 使用其私有 SQLite 会话存储，而 Antigravity 使用独立的会话数据库，两者二进制会话文件无法直接跨平台回放。
2. **交接简报自动生成**：调度器检测到切换代理工具或工作区存在未提交代码时，会在接任 Agent 的首轮提示词中自动注入 **【关键任务交接简报】**：
   - **在途未提交代码警报 (In-Flight Worktree Changes)**：自动抓取工作区未提交的文件清单（如上一轮 Codex 做到一半的 `core_manager.rs`、`resources.tsx`），指示接任 Agent 优先运行 `cargo check` 和单元测试评估完成度，在此基础上继续补全测试并完成提交，严禁盲目丢弃。
   - **上一轮交付总结与后续任务指引**：自动提取前任 Agent 最近一轮的 `turn_*_last_msg.txt`，获知上一轮已完成的工作与明确声明的“下一个待办子任务”。
   - **近期 Git 提交历史**：提取最近 5 次原子提交的摘要，保持交付粒度与代码风格一致。
   - **架构文档与约束继承**：自动挂载 `ARCHITECTURE.md` 优先级（P1 -> P2 -> P3 -> P4）与 `AGENTS.md` 规则。
3. **外部统一的节奏管控**：进度总结节奏、看门狗超时监控、Conventional Commit 规范提取均由外部调度器硬性保障，与底层具体使用哪家 Agent 无关。

---

## 规范化英文 Conventional Commits 与测试噪音过滤

1. 调度器提示词明确要求 Agent 在答复首部输出标准的 `COMMIT_START` / `COMMIT_END` 英文提交块。
2. 提取器严格过滤任何测试流水表述（如 `验证通过`、`cargo check`、`测试结果`、`playwright` 等），保持提交历史纯净专业。
3. 具备自适应降级回退机制，即便模型未显式输出标记块，提取器也会自动映射语义并生成标准格式的英文 Conventional Commit。
