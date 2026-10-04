# Mihomo Server

无头运行的 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)：不需要桌面环境，以后台服务的形式在 Linux 服务器上运行 [Mihomo](https://github.com/MetaCubeX/mihomo) 内核，从任意浏览器远程管理订阅、节点和配置。

订阅、增强、节点选择等管理逻辑直接取自 Clash Verge Rev，用法与桌面版相近，适合服务器、软路由等无图形界面的设备。

## 安装

需要 x86_64 Linux、systemd、curl（或 wget）、tar、SHA-256 校验工具，以及 sudo、用户管理工具和 libcap 的 `setcap/getcap`。无需 Python。以普通用户执行，无需安装参数：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash
```

脚本下载并校验最新版本，自动通过 sudo 安装到 `/opt/mihomo-server`，为你启动独立的 systemd 用户实例，授权使用 TUN，并启用注销后、重启后继续运行。sudo 可能要求输入密码。安装完成会显示管理地址、代理端口、配置文件和令牌位置，并提供 `mihomo-server` 命令（`/usr/local/bin`，附 man 手册和 bash/zsh 补全）。

程序全系统共享，用户配置和数据各自独立：

| 内容 | 默认位置 |
|---|---|
| 启动配置 | `~/.config/mihomo-server/env` |
| 订阅、设置及令牌等数据 | `~/.local/share/mihomo-server` |
| 程序 | `/opt/mihomo-server/current` |
| 命令 | `/usr/local/bin/mihomo-server`，`man mihomo-server` |

配置和数据分别遵循 `XDG_CONFIG_HOME`、`XDG_DATA_HOME`。初始化时保存有效路径，之后重启服务无需重新设置环境变量；空值或相对路径使用默认目录。已知的旧用户级安装会保留原数据、启动参数和回退副本，自定义服务单元需要自行迁移。

直接以 root 执行只安装共享程序；通过 sudo 执行时会为 `SUDO_USER` 启动实例。

## 使用

首个用户（slot 0）的默认管理地址是 `http://127.0.0.1:9090`，HTTP/SOCKS 混合代理为 `127.0.0.1:7890`，DNS 监听为 `127.0.0.1:1053`。已有程序占用默认端口时，脚本会报告冲突；通过用户配置或设置页调整后重新启动。

```sh
mihomo-server info     # 查看登录地址（含令牌）和文件路径
mihomo-server token    # 查看登录令牌
```

打开管理页面，使用令牌登录；在「订阅」页导入 YAML 或远程订阅链接并使用订阅，然后在「代理」页选择节点。开启 TUN 在「设置」页或用 `mihomo-server tun on` 完成。

其他用户按需启用自己的实例：

```sh
mihomo-server enable   # init 仍可作为兼容别名
```

### 命令行管理

不打开网页，也可以用 `mihomo-server` 完成常用操作，均作用于你自己的实例：

```sh
mihomo-server status                 # 服务、内核、当前订阅、代理模式、TUN、端口
mihomo-server start | stop | restart
mihomo-server enable | disable       # 开机自启 / 停止并取消自启（保留数据）
mihomo-server logs [-n 100]          # 跟踪日志，或传入 journalctl 选项

mihomo-server sub                    # 列出订阅，* 为正在使用
mihomo-server sub add URL|文件 [-n 名称] [--use]
mihomo-server sub use [订阅]          # 切换订阅；省略时列出编号供选择
mihomo-server sub update [订阅...]    # 更新远程订阅，省略时更新全部
mihomo-server sub remove 订阅

mihomo-server proxy                  # 列出代理组及当前节点，* 为主分组
mihomo-server proxy list 分组         # 节点及最近延迟
mihomo-server proxy test [分组|节点]   # 测速，按延迟排序
mihomo-server proxy select [分组] 节点  # 默认主分组（全局模式下为 GLOBAL）

mihomo-server mode [rule|global|direct]
mihomo-server tun [on|off]
mihomo-server core [update [--alpha]]
mihomo-server update                 # 升级程序到最新版本
```

订阅、分组和节点可以写全名、列表中的编号，或名称中唯一的一部分，如 `mihomo-server proxy select '日本 03'`、`mihomo-server proxy select AI 3`。加 `--json` 输出 JSON，便于脚本调用。`status` 在实例未运行时退出码为 3。

slot 1–63 的管理端口为 `20000+10N`，代理端口为 `20001+10N`，DNS 端口为 `20002+10N`。订阅中写死的端口自动替换，在设置页显式指定的端口保留。

这里的 TUN 是**整机 TUN**：与 clash-verge 等单机客户端一样，开启后接管本机所有账户（含 root 系统服务）的流量和系统 DNS（systemd-resolved），fake-IP 与 DNS 防污染全部生效；安装时的那次 sudo 就是唯一一次授权，之后开关 TUN 不会再弹出系统授权窗口。整机同一时间只有一个 TUN：`mihomo-tun` 组中谁先开启谁占用，直到他自己关闭（或其服务停止）；其他账户的 TUN 开关会置灰并显示占用者，他们的流量按占用者的规则处理，自己的代理端口照常可用。其他账户无法关闭占用者的 TUN，只有 root 可以，例如 `sudo systemctl --user -M alice@ stop mihomo-server`。

其他用户需要 TUN 或后台常驻时，由管理员执行：

```sh
sudo usermod -aG mihomo-tun 用户名
sudo loginctl enable-linger 用户名
```

授权后重启该实例即可使用 TUN，无需重新登录。`mihomo-tun` 成员可以让任意程序获得整机网络管理能力，并接管整机流量，只授权给可信用户。

### 桌面客户端（可选）

有图形桌面时，可以安装轻量的桌面客户端 `mihomo-server-desktop`（Tauri 2，Release 中提供 `.deb`、`.rpm` 和 AppImage）。它独立于服务安装，不装也不影响使用：

- 自动发现本机当前用户的 mihomo-server 实例，在窗口中打开管理页面并自动登录；
- 托盘菜单可直接切换代理模式、开关 TUN、为各代理组选择节点和测速、切换订阅，以及启动/停止/重启 mihomo-server 服务（内核由服务自行管理，桌面端不直接操作）；
- 本机未安装时提供「安装 mihomo-server 服务」按钮：下载官方安装脚本并安装最新版本，管理员授权通过系统的 polkit 对话框完成，完成后点击「打开管理程序」即可；已安装但未运行时可一键启动服务；操作失败会以系统通知提示；
- 关闭窗口后驻留托盘：左键单击托盘图标打开管理界面，右键打开菜单；菜单中可设置登录时启动（以 `--hidden` 只启动托盘）。
- 界面语言与 Web 管理界面同步：在 Web 设置中切换语言后托盘菜单立即跟随，客户端启动时也按服务端保存的语言显示（未设置时跟随系统语言）。

```sh
sudo dnf install ./mihomo-server-desktop-vX.Y.Z-x86_64.rpm     # Fedora/RHEL
sudo apt install ./mihomo-server-desktop-vX.Y.Z-x86_64.deb     # Debian/Ubuntu
```

安装包在 Ubuntu 24.04 上构建，需要 glibc 2.39 及以上，并依赖 WebKitGTK 4.1、libayatana-appindicator 和 polkit。GNOME 需要安装 AppIndicator 扩展才能显示托盘图标；KDE 等桌面原生支持。Fedora 上建议使用 RPM，AppImage 需要 FUSE 2（`fuse-libs`）。

## 远程访问与服务管理

编辑 `mihomo-server info` 显示的 `env` 文件，例如：

```ini
MIHOMO_SERVER_LISTEN=0.0.0.0:9090
MIHOMO_SERVER_PUBLIC_ORIGIN=http://服务器IP:9090
```

然后执行 `mihomo-server restart`。明确的监听和公开地址配置优先于兼容的 `MIHOMO_SERVER_ARGS` 中同名选项。通过 HTTPS 反向代理访问时，公开地址填写代理的 HTTPS 地址。

也可以使用 SSH 隧道，不必修改监听配置：

```sh
ssh -L 9090:127.0.0.1:9090 用户名@服务器
```

常用命令见上文「命令行管理」；`mihomo-server-user` 仍可使用，提供同样的服务管理命令。

## 升级与卸载

升级程序：执行 `mihomo-server update`（或重新执行安装命令）。正在运行的实例会重启到新版本，已停用的实例保持停用。

升级内核有两种方式，每个用户的内核各自独立：

- 网页「内核」页：可升级到最新稳定版或 Alpha 版，失败自动回滚，升级后 TUN 照常可用。
- 重新执行安装命令：实例启动时，如果内核是比安装包内置版本更旧的稳定版，会自动升级到内置版本；在网页上装的更新版本或 Alpha 版不会被覆盖。

卸载：停止所有实例，删除程序、命令、TUN 组、槽位登记，并关闭由安装脚本开启的 linger。各用户的订阅、设置和令牌会保留，重新安装后可以继续使用：

```sh
mihomo-server uninstall    # 或：curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash -s -- --uninstall
```

彻底卸载：在卸载的同时删除所有用户的实例数据和配置，包括迁移前旧版用户级安装留下的程序和备份：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash -s -- --uninstall --purge
```

彻底卸载也可以执行 `mihomo-server uninstall --purge`。只删除自己的数据：执行 `mihomo-server purge`。

构建和手动前台部署请参考 [部署文档](docs/DEPLOYMENT.md)。

## 许可证

本项目以 [GPL-3.0-only](LICENSE) 发布，衍生自 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)。第三方依赖的许可证见 [LICENSES.txt](LICENSES.txt)，代码来源见 [docs/UPSTREAM.md](docs/UPSTREAM.md)。
