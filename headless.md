# Headless 单服务改造方案

## 目标与边界

将 Clash Verge Rev 中的配置管理、订阅管理、配置增强和 Mihomo 交互能力抽离出来，组成无需桌面环境、通过 Web UI 管理的服务。能独立复用的组件直接复用；与 Tauri 耦合的组件保留业务逻辑，替换运行环境和通信适配。

最终只注册、启动和管理一个系统 service。该服务包含 Rust 后端管理、HTTP/WebSocket 服务和 Mihomo 子进程管理。生产环境无需单独启动 Node、Vite 或 Clash Verge Service。

本方案中的“一个 service”不等于“一个进程”：Rust 守护程序与 Mihomo 是两个进程，但属于同一个服务生命周期，用户只管理一个服务入口。

如果要求严格只有一个进程，则需要将 Go 编写的 Mihomo 作为库集成，例如通过 FFI 调用。这会增加跨语言构建、运行时和退出处理工作，也不能直接沿用当前项目的子进程管理方式，因此不作为本方案的实施路线。

本文是设计方案，不代表 headless 功能已经实现。

## 当前交付优先级（用户调整）

当前只按以下顺序推进，优先保证 Linux 上的核心代理服务完善可用：

1. 配置与资源管理：完整服务设置、Geo/provider 资源路径及生命周期、配置生成/校验/应用/恢复和对应管理页面。
2. 规则、provider、延迟测试等核心管理能力及对应 API、Web 页面。
3. i18n、服务信号处理；保留已有的 Unix 统一退出及子进程回收行为。
4. Linux 打包并实际以 systemd 安装、启动和验证；模板或静态检查不代表完成。

此顺序对应用户原编号 1、2、4、5，仅重新编号，不增加新任务。其他未完成项暂不推进，包括自动备份及淘汰、变更触发备份、WebDAV、备份页面、媒体检测、SOCKS/PAC、完整连接面板、容器、非 Linux 平台和外部发布。已完成的功能保留；Windows 兼容继续延期。

本节覆盖下文及历史进度中的宽泛扩展顺序。实现状态、具体交付顺序和下一子任务以 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) 为准；未经用户重新扩展范围，不恢复延期工作。

## 目标架构

```text
浏览器
   │ HTTP / WebSocket
   ▼
唯一的 headless service（Rust 守护程序）
   ├─ Web UI 静态资源
   ├─ 管理 API：订阅、配置、脚本、备份、内核操作
   ├─ 应用事件与实时数据转发
   ├─ 配置生成、校验、保存
   ├─ 定时订阅更新
   └─ Mihomo 生命周期管理
           │ 本地 Socket / 回环 HTTP
           ▼
        Mihomo 子进程
```

Rust 后端采用 Axum 提供 HTTP API、WebSocket 和 Web 静态文件。前端构建产物随发布包分发，也可以使用 `rust-embed` 嵌入 Rust 二进制；不需要生产环境前端开发服务器。

浏览器通过后端访问 Mihomo，Mihomo 控制接口用于服务内部通信。这样即使内核启动失败或暂时停止，Web 管理页面仍能查看错误、修改配置和重新启动内核。

启动时先建立可用的管理接口，再由内核管理器异步加载配置和启动 Mihomo。内核配置错误与启动失败作为运行状态上报，不作为 Web 服务启动的前置条件；管理服务自身的监听或认证配置错误则单独处理。

优先保留项目已有的本地 Socket 通信方式。不同平台分别适配 Unix Socket 和 Windows Named Pipe；回环 HTTP 可以作为明确配置的替代方式。

## 复用原则

- 保留配置处理顺序、模型、文件格式和业务行为，优先替换外围适配。
- 独立组件通过 workspace 依赖复用，不为 headless 重写同一套算法。
- 提取源码时固定来源提交，记录原始路径，便于后续同步上游。
- 不把桌面服务安装、窗口、托盘等逻辑带入新的核心组件。
- 不把“能复制源码”当成“已经能独立运行”：迁移后需要验证依赖与行为。

复用范围以实际依赖和迁移后的行为验证为依据，不用未经统计的百分比估算改造工作量。

## 可直接复用的组件

这里的“直接复用”指业务实现基本不变，迁移后仍可能需要调整 workspace 依赖、模块路径、资源位置和初始化参数。

| 来源 | 可复用内容 | 迁移注意事项 |
| --- | --- | --- |
| `crates/clash-verge-draft` | 已提交配置、草稿、事务管理 | 保留原有提交与放弃草稿的语义 |
| `crates/clash-verge-limiter` | 限频工具 | 调整 workspace 依赖即可 |
| `crates/clash-verge-logging` | 日志初始化、分类、日志缓存等 | 配置服务日志目录与初始化方式 |
| `crates/clash-verge-i18n` | 后端国际化 | 一并迁移语言资源；浏览器语言不应修改服务全局语言 |
| `crates/clash-verge-media-unlock` | 媒体解锁检测 | 网络请求需使用服务管理的代理出口 |
| `src-tauri/src/enhance/field.rs` | 字段处理 | 连同相关类型和常量迁移 |
| `src-tauri/src/enhance/merge.rs` | 配置合并 | 保留现有合并语义 |
| `src-tauri/src/enhance/seq.rs` | 规则、节点和分组序列操作 | 连同相关模型迁移 |
| `src-tauri/src/config/encrypt.rs` | 加密相关逻辑 | 保留格式及所需参数 |
| YAML 工具、模板中的纯逻辑 | YAML 处理、默认模板 | 按函数提取；所在文件可能还包含桌面依赖 |

`clash-verge-draft` 是配置更新的重要基础，应优先保留。其事务能力并不自动保证磁盘文件与 Mihomo 当前配置也一起回滚，需要在配置应用流程中单独处理。

## 保留业务逻辑、适配外围依赖的组件

| 来源 | 应保留的内容 | 需要适配的内容 |
| --- | --- | --- |
| `src-tauri/src/config/` | 订阅模型、配置读写、节点选择记录、DNS 设置、运行时配置 | 桌面设置、目录解析、事件通知、Mihomo 客户端引用 |
| `src-tauri/src/enhance/` | 合并顺序、脚本执行、规则/节点/分组增强、DNS/TUN 配置生成 | 全局配置读取、路径、通知、平台处理 |
| `src-tauri/src/feat/profile.rs` | 订阅下载与更新、代理重试、运行配置更新 | 托盘与桌面通知 |
| `src-tauri/src/core/timer.rs` | 更新周期、任务调度、同一订阅更新去重 | 桌面初始化完成条件和事件出口 |
| `src-tauri/src/core/validate.rs` | YAML 校验、内核校验、错误分类、超时 | Tauri shell 改为普通进程调用 |
| `src-tauri/src/core/manager/config.rs` | 生成、校验、应用、提交流程 | 外部服务会话、staging 分支和 Tauri 引用 |
| `src-tauri/src/core/proxy_view.rs` | 节点、分组、provider 聚合模型 | 来自 Mihomo 插件的数据类型依赖 |
| `src-tauri/src/core/backup.rs` | 备份、恢复、WebDAV | 配置路径和桌面生命周期 |
| `src-tauri/src/module/auto_backup.rs` | 自动备份 | 服务启动、调度和退出接入 |
| `src-tauri/src/core/runtime_bundle.rs` | provider 路径冲突处理、资源识别 | 为外部服务传输资源的 bundle 部分按需移除 |
| `src-tauri/src/feat/core_upgrade.rs` | 内核版本查询、平台包选择、下载解压、暂存验证、替换与失败回滚 | 内核路径、网络出口、生命周期管理；移除外部服务批准副本的 staging 分支 |
| `crates/clash-verge-signal` | 系统信号监听与重复停机协调 | 接入服务退出流程；Windows Service 另需 SCM 控制处理 |

例如，`enhance/` 的算法可以保留，但入口读取全局 `Config`，部分代码还依赖目录、异步任务和通知能力，不能仅复制目录就认为完成了抽离。

`core/proxy_view.rs` 主要是数据聚合逻辑，但类型来自 `tauri-plugin-mihomo`，需要与客户端模型一起提取。

## Mihomo 客户端的提取

实际 Mihomo 通信实现主要位于外部项目 [tauri-plugin-mihomo](https://github.com/clash-verge-rev/tauri-plugin-mihomo)，本仓库通过插件调用它。

从该插件提取一个不依赖 Tauri 的 `mihomo-client` 库，保留请求实现、API 方法、模型、错误处理和底层实时数据通信。

已有客户端底层使用 reqwest。提取时继续复用这些请求实现，不另写一套仅覆盖 `/configs` 和 `/proxies` 的客户端。HTTP 与本地 Socket 是传输方式的选择，与是否复用已有业务接口无关。

```text
tauri-plugin-mihomo
   ├─ HTTP / 本地 Socket 请求实现 ──→ mihomo-client
   ├─ API 方法和响应模型 ───────────→ mihomo-client
   ├─ WebSocket 建连与读取 ─────────→ 保留并适配
   └─ Tauri 命令、状态注册、IPC Channel → 替换
```

应覆盖以下现有功能：

- 查询版本、配置、节点、分组、规则和 provider。
- 选择节点、固定或取消固定节点、延迟测试。
- 更新 provider 和执行健康检查。
- 查询连接、关闭指定连接或全部连接。
- 配置重载、运行配置修改，以及需要保留的 Geo 更新能力。
- 日志、流量、内存和连接数据订阅。

插件不是完全独立的普通客户端库。核对时，上游 `main` 的 `src/mihomo.rs` 使用了 Tauri `InvokeResponseBody` 转发 WebSocket 数据，需要改为普通消息、回调或 Tokio channel；插件注册、Tauri 命令和前端 IPC 包装不应进入核心库。

当前仓库 `Cargo.lock` 锁定的插件提交为 `ba8434c08c869916b8041d707ad66599bb5230b2`。正式提取前应取得该提交并核对源码。此次评估查看了上游当前源码，尚未取得该锁定提交的源码，不能保证两者完全一致。

参考：[插件入口](https://github.com/clash-verge-rev/tauri-plugin-mihomo/blob/main/src/lib.rs)、[通信实现](https://github.com/clash-verge-rev/tauri-plugin-mihomo/blob/main/src/mihomo.rs)、[Mihomo API 文档](https://wiki.metacubex.one/api/)。

## 关键适配点

### 应用上下文与事件

`src-tauri/src/core/handle.rs` 当前通过 Tauri `AppHandle` 获取 Mihomo 客户端、发送刷新与通知事件，并管理退出状态。

改成普通应用上下文，包含配置状态、目录参数、Mihomo 客户端、内核管理器和事件发送器。初期可保留 `refresh_profiles()`、`notice_message()` 等方法的业务语义，将出口替换为服务事件，减少调用方改动。

后端事件通过 WebSocket 发给浏览器；日志与流量等实时数据通过后端转发。浏览器重连后重新读取当前状态，避免仅依赖曾经发出的事件。

### 进程管理

把 `app_handle.shell().sidecar(...)` 替换为 `tokio::process::Command`。保留现有启动参数、日志读取、就绪探测和退出处理逻辑。

新内核管理器是唯一的进程生命周期管理者，负责：

- 启动、就绪探测、停止、重启和退出回收。
- 配置加载及失败状态记录。
- 内核故障后的有限重试与退避，避免持续重启。
- 服务退出时终止并等待 Mihomo 子进程退出。
- 防止启动、停止、重启与配置应用互相竞争。

内核停止期间，管理 API 和 Web UI 继续工作。不要再通过外部 Clash Verge Service 启动另一份内核。

正常退出时先停止新任务和配置写入，再请求内核退出、等待回收，超过明确的等待时限后终止进程。`kill_on_drop(true)` 只作为兜底，不替代完整停机流程；服务异常退出时的进程清理由平台服务管理机制配合完成。

Linux/systemd、容器和 macOS 接入各自的停止与信号机制。Windows Service 需要注册 SCM 控制处理并把停止请求交给统一退出流程，不能只监听 Ctrl+C。`clash-verge-signal` 可复用信号与停机协调能力，但不是完整的跨平台服务宿主。

参考：[Tokio 进程管理](https://docs.rs/tokio/latest/tokio/process/struct.Command.html#method.kill_on_drop)、[Windows 服务控制处理](https://learn.microsoft.com/en-us/windows/win32/services/service-control-handler-function)。

### 配置校验与应用

继续使用原项目的校验机制：

```text
mihomo -t -d <数据目录> -f <候选配置>
```

Mihomo 当前入口支持这些参数，见 [Mihomo 源码](https://github.com/MetaCubeX/mihomo/blob/Meta/main.go)。

保留以下业务顺序：

```text
订阅 YAML + 服务设置
        ↓
配置增强：合并、脚本、规则/节点/分组、DNS/TUN
        ↓
生成候选运行配置
        ↓
YAML 与 Mihomo 配置校验
        ↓
由内核管理器应用配置
        ↓
提交配置状态与持久化结果，通知浏览器
```

配置写入共用串行化机制。候选文件应与已生效文件区分，明确校验失败、加载失败和服务中途退出时的恢复行为。内存草稿、磁盘文件和内核实际运行状态不能假设为自动完成的同一个事务。

校验成功后优先通过 `PUT /configs?force=true` 重载，保留原项目重载失败后的重启路径，并明确重启失败时的恢复行为。热重载是优先应用方式，不能对所有配置变化承诺连接完全不中断。

还需迁移 Geo 数据、provider 资源路径与缓存处理，不能只复制最终 YAML。订阅下载通过代理的逻辑需考虑首次启动时内核尚不可用的情况。

### 配置脚本执行

保留 Boa 增强脚本的输入输出语义、日志、循环次数及输入输出大小限制，替换 `AsyncHandler` 对 Tauri 异步运行时的依赖。

当前五秒超时发生在等待阻塞任务结果的外层，不等于五秒后强制终止脚本。已经开始的 `spawn_blocking` 任务无法通过普通取消停止；现有循环次数限制也不等于完整的时间和内存隔离。因此不能将现有机制描述为硬执行时限或完整沙盒保证。

初期把脚本作为受认证管理员的配置能力，验证超时后的任务与服务退出行为。若后续需要硬执行时限或运行不可信脚本，应另行设计可中断执行或独立 worker 隔离；该选择可能增加运行进程，但不要求增加系统 service。

参考：[Tokio 阻塞任务语义](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)。

### 数据目录

`utils/dirs.rs` 包含桌面用户数据目录和 Tauri 资源目录逻辑。新服务应显式接收数据目录、资源目录和 Mihomo 路径，避免服务账户变化后读到另一套配置。

优先保留有价值的订阅文件和配置格式；桌面设置模型中的窗口、托盘等字段不作为服务运行参数。服务监听地址、访问认证、内核路径等参数单独定义。

### 外部服务与桌面功能

`src-tauri/src/core/service.rs` 是桌面程序连接外部 Clash Verge Service 的客户端，不是新守护服务入口。

本方案不沿用外部服务安装、版本协商、owner session、Service/Sidecar 交接和相关修复流程。`runstate` 保留运行状态管理思路，简化为内核运行状态、错误和必要的操作状态。

窗口、托盘、快捷键、桌面通知、剪贴板、桌面自动启动和 Tauri 更新器不属于 headless 核心。系统代理等平台能力是否保留取决于部署用途；TUN 配置和权限则由唯一服务及其部署方式负责。

普通代理模式不默认要求 root 或管理员身份。启用 TUN 等需要额外权限的功能时，按部署平台配置必要权限；主程序监督 Mihomo，由 Mihomo 执行实际 TUN 数据处理。

## Web UI 复用

现有 React 页面和组件可大幅保留，但构建产物不能直接在普通浏览器中完整运行，因为页面仍调用 Tauri API。

优先替换通信层，保留页面业务接口：

| 现有依赖 | 浏览器实现 |
| --- | --- |
| `services/cmds.ts` 的 `invoke()` | HTTP 管理 API |
| `services/events.ts` 的 Tauri 事件 | WebSocket 事件 |
| `tauri-plugin-mihomo-api` | 后端 Mihomo API 适配层 |
| Tauri 文件选择与读取 | 浏览器上传和下载 |
| 剪贴板插件 | 浏览器 Clipboard API |
| WindowProvider 与窗口控制 | 浏览器布局状态或移除 |

采用 Tauri-to-Web 兼容层，尽量保留调用名称和返回模型。可以通过 Vite 别名或替换服务模块导入接入，但只为 `@tauri-apps/api/core` 提供一个 `invoke()` Shim 无法覆盖事件、窗口、文件插件及 Mihomo 的 IPC Channel。迁移前列出实际使用的依赖，逐项适配。

命令适配层使用后端显式允许的命令路由，保持参数和结构化错误语义，并定义无返回值操作的响应方式。不要假定每个成功响应都可以调用 `res.json()`。事件适配与 Mihomo 实时订阅分别覆盖取消订阅和重连行为。

优先迁移代理、规则、连接、日志、订阅与配置编辑页面。设置页面删除窗口、托盘、桌面自动启动和桌面更新相关选项，并增加服务配置。

链式代理等部分设置当前保存在 `localStorage`。若需要多个浏览器共享，迁移到后端持久化；滚动位置等界面偏好仍可留在浏览器。

后端管理 API 负责订阅与持久配置，Mihomo API 负责运行时查询和操作。运行时操作是否需要持久化，应沿用原项目对应业务流程，不能仅转发请求就认为已经保存。例如节点选择还需要记录和恢复。

SPA 路由回退只用于前端页面导航。不存在的 API、WebSocket 路由和静态资源返回相应错误，不统一返回 `index.html`。

## 发布与内核打包

沿用 Clash Verge 的发布方式：Mihomo 作为独立二进制随安装包或容器镜像提供，由唯一的 Rust service 管理。本仓库 `src-tauri/tauri.conf.json` 的 `externalBin` 包含 `verge-mihomo` 与 `verge-mihomo-alpha`，构建准备逻辑位于 `scripts/prebuild.mjs`；headless 版本复用其平台资源选择和包准备逻辑，替换 Tauri 打包入口。

Mihomo 不采用 `include_bytes!` 内嵌释放。Web UI 静态资源仍可使用 `rust-embed` 内嵌，或随包分发；这不影响内核独立升级，也不增加系统 service。

发布包提供首次运行所需的内核，无需启动时联网下载。构建固定目标平台、内核版本与可验证的资源信息，由构建准备步骤取得资源，不默认在 `build.rs` 中下载浮动的最新版本。

服务使用统一的受管内核目录，支持明确配置路径，并确保服务账户可以在该目录暂存、替换和回滚二进制。若安装目录不可写，将随包内核初始化到可写的受管目录；后续运行和升级均使用该目录。容器部署时将可更新的内核目录持久化，避免容器重建丢失 Web UI 已升级的内核。

已升级的受管内核不在每次启动时被随包版本覆盖；服务自身升级与内核升级分别管理版本，安装更新时明确保留受管内核的策略。

### Web UI 单独升级内核

复用现有 `src-tauri/src/feat/core_upgrade.rs`，以及 `src/components/setting/mods/clash-core-viewer.tsx` 的升级交互。浏览器通过命令兼容层调用 `upgrade_clash_core`，保留 `force` 参数和 `CoreUpgradeReport { upgraded, from, to }` 返回模型，显示更新成功、已是最新版本或错误信息。

升级由管理后端执行，沿用当前项目的流程，不直接转发 Mihomo `/upgrade` 自升级接口：

```text
Web UI 点击升级当前内核
        ↓
读取当前版本，按稳定版 / Alpha 通道解析目标版本
        ↓
选择匹配平台与架构的包，下载并解压到同目录暂存文件
        ↓
设置执行权限，运行暂存内核 -v 验证可运行性和版本
        ↓
保留可恢复的旧内核，按平台方式替换受管二进制
        ↓
重启 Mihomo，确认就绪并刷新实际运行版本
        ↓
返回升级结果；失败时按替换与运行状态尝试回滚、恢复
```

现有下载超时、包大小限制、升级串行锁、平台替换方式及回滚逻辑应优先保留。当前暂存验证检查可运行性和版本，不将其描述为已有签名或摘要验证。

将 `managed_core_path()` 从桌面程序旁的路径改为统一受管路径，网络请求适配服务代理出口，内核重启改为新管理器调用。移除向外部 Clash Verge Service 提交管理员批准副本的分支；headless service 直接管理同一份受管二进制。

升级与内核切换、停止、重启、配置应用由统一生命周期机制协调。Web UI 升级期间管理服务继续运行，允许 Mihomo 重启造成短暂代理中断；前端禁止重复提交并在结束后更新内核状态。不能只看到磁盘版本变化就报告升级成功，还需确认实际运行版本。

保留稳定版与 Alpha 的选择和对应升级能力。内核升级不要求重新构建、安装或重启 Rust 管理服务与 Web UI。

## Web 管理边界

Web 化后，原本供本机桌面调用的接口将成为网络入口，需要在适配时明确边界：

- 管理 API 与 WebSocket 使用一致的访问认证，并校验浏览器来源；使用 cookie 会话时处理 CSRF。
- 浏览器通过服务访问内核，内部控制凭据不作为前端运行配置分发。
- 不原样暴露任意服务器路径读写、打开程序或桌面 shell 命令。
- 文件上传、下载对应受控的数据目录；浏览器文件选择不代表服务器文件访问。
- 配置脚本仍由后端执行，属于有权限的配置管理操作。
- 是否对局域网或公网监听，以及是否启用 TUN，作为明确的部署参数。

## 建议的代码组织

以下是目标职责划分，不要求第一阶段一次性拆成所有独立 crate：

```text
workspace/
   ├─ crates/
   │   ├─ mihomo-client/       # 提取后的无 Tauri 客户端
   │   ├─ headless-core/       # 配置、订阅、增强、调度、备份
   │   ├─ clash-verge-draft/   # 直接复用
   │   ├─ clash-verge-logging/ # 直接复用
   │   └─ ...                 # 按实际需要保留的公共组件
   ├─ service/                # 唯一程序入口、进程管理、HTTP/WS
   └─ web/                    # 迁移后的 React UI
```

Axum 路由调用核心业务函数，避免在 API 层重新实现订阅或配置逻辑。认证、请求与事件适配放在服务层，核心库保持无 Tauri、无 HTTP 框架依赖。具体模块拆分在实现阶段按实际依赖确定。

## 实施顺序

1. **建立独立 workspace，固定来源。** 先迁移独立 crates、配置模型和纯配置处理函数，记录原始路径与提交。
2. **提取 `mihomo-client`。** 在没有 Tauri 的程序中验证查询版本、节点、规则、重载配置与实时订阅。
3. **实现唯一内核管理器。** 打通启动、就绪、停止、退出回收和重启，建立可供管理接口读取的状态；接入平台停机机制。
4. **迁移配置和订阅流程。** 保留增强顺序、草稿事务、校验和节点选择恢复，处理资源路径与失败恢复。
5. **接入定时更新、内核升级和所需备份能力。** 替换桌面启动条件与事件出口，迁移 `core_upgrade.rs` 的受管路径和生命周期调用。
6. **增加 Axum HTTP API 和 WebSocket。** 所有配置写入共用核心流程，提供运行状态和错误信息；验证内核启动失败仍可通过管理接口修复。
7. **迁移 React 通信层。** 接入命令、事件和 Mihomo API 兼容层，适配桌面能力，打通订阅切换、配置编辑、节点选择、日志、连接管理和内核升级按钮。
8. **作为一个系统 service 发布。** 参考 Clash Verge 提供含 Rust 程序、独立 Mihomo 二进制和 Web 资源的发布包，只注册一个服务入口，并保留内核独立升级后的文件。

## 关键验证行为

首个完整闭环是：导入订阅、生成并校验配置、启动内核、选择节点、重载配置、重启服务后恢复配置和节点选择。

随后验证无效配置不会破坏已生效配置；内核异常退出后状态可见且可以恢复；停止服务不会留下 Mihomo 进程；定时更新不会与手动更新、配置应用产生冲突；多个浏览器能够观察一致的服务状态；无桌面环境时可以启动并使用全部已迁移功能。

服务集成验证还应覆盖首次启动遇到无效配置时仍可访问管理页面、实际平台服务停止操作、脚本超时后的行为和无返回值命令。

内核升级验证覆盖 Web UI 点击升级、稳定版与 Alpha 通道、已是最新版本、下载或暂存失败时保留旧内核、替换后启动失败的回滚，以及服务重启后仍使用已升级内核。升级期间 Web 管理服务持续可用，返回版本与实际运行版本一致。

验证应以这些实际行为为依据，复用已有相关测试；新增测试仅覆盖迁移带来的具体风险，不为拆分本身扩充测试脚手架。

## 来源与许可

当前仓库标注为 GPL-3.0-only。复制和分发时保留许可证、版权及来源信息，并核对外部 Mihomo 插件、Mihomo 和其他依赖各自的许可。

在本仓库内开展非平凡实现时，遵循 `AGENTS.md` 的 issue-first 与范围约束。本文记录方案，本身不代表已批准全部后续实现变更。
