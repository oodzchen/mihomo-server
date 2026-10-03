# Mihomo Server

无头运行的 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)：不需要桌面环境，以后台服务的形式在 Linux 服务器上运行 [Mihomo](https://github.com/MetaCubeX/mihomo) 内核，从任意浏览器远程管理订阅、节点和配置。

订阅、增强、节点选择等管理逻辑直接取自 Clash Verge Rev，用法与桌面版相近，适合服务器、软路由等无图形界面的设备。

## 安装

需要 x86_64 Linux、systemd、curl（或 wget）、tar、SHA-256 校验工具，以及 sudo、用户管理工具和 libcap 的 `setcap/getcap`。无需 Python。以普通用户执行，无需安装参数：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash
```

脚本下载并校验最新版本，自动通过 sudo 安装到 `/opt/mihomo-server`，为你启动独立的 systemd 用户实例，授权使用 TUN，并启用注销后、重启后继续运行。sudo 可能要求输入密码。安装完成会显示管理地址、代理端口、配置文件和令牌位置。

程序全系统共享，用户配置和数据各自独立：

| 内容 | 默认位置 |
|---|---|
| 启动配置 | `~/.config/mihomo-server/env` |
| 订阅、设置及令牌等数据 | `~/.local/share/mihomo-server` |
| 程序 | `/opt/mihomo-server/current` |

配置和数据分别遵循 `XDG_CONFIG_HOME`、`XDG_DATA_HOME`。初始化时保存有效路径，之后重启服务无需重新设置环境变量；空值或相对路径使用默认目录。已知的旧用户级安装会保留原数据、启动参数和回退副本，自定义服务单元需要自行迁移。

直接以 root 执行只安装共享程序；通过 sudo 执行时会为 `SUDO_USER` 启动实例。

## 使用

首个用户（slot 0）的默认管理地址是 `http://127.0.0.1:9090`，HTTP/SOCKS 混合代理为 `127.0.0.1:7890`，DNS 监听为 `127.0.0.1:1053`。已有程序占用默认端口时，脚本会报告冲突；通过用户配置或设置页调整后重新启动。

```sh
mihomo-server-user info     # 查看实际地址和文件路径
mihomo-server-user token    # 查看登录令牌
```

打开管理页面，使用令牌登录；在「订阅」页导入 YAML 或远程订阅链接并使用订阅，然后在「代理」页选择节点。开启 TUN 仍在「设置」页完成。

其他用户按需启用自己的实例：

```sh
mihomo-server-user enable   # init 仍可作为兼容别名
```

slot 1–63 的管理端口为 `20000+10N`，代理端口为 `20001+10N`，DNS 端口为 `20002+10N`。订阅中写死的端口自动替换，在设置页显式指定的端口保留。

其他用户需要 TUN 或后台常驻时，由管理员执行：

```sh
sudo usermod -aG mihomo-tun 用户名
sudo loginctl enable-linger 用户名
```

授权后重启该实例即可使用 TUN，无需重新登录。`mihomo-tun` 成员可以运行具备整机网络管理能力的内核，仅授权可信用户。各实例的 TUN 默认只代理自己的 UID；使用共享系统 DNS 时，默认开启流量嗅探以恢复域名规则。

## 远程访问与服务管理

编辑 `mihomo-server-user info` 显示的 `env` 文件，例如：

```ini
MIHOMO_SERVER_LISTEN=0.0.0.0:9090
MIHOMO_SERVER_PUBLIC_ORIGIN=http://服务器IP:9090
```

然后执行 `mihomo-server-user restart`。明确的监听和公开地址配置优先于兼容的 `MIHOMO_SERVER_ARGS` 中同名选项。通过 HTTPS 反向代理访问时，公开地址填写代理的 HTTPS 地址。

也可以使用 SSH 隧道，不必修改监听配置：

```sh
ssh -L 9090:127.0.0.1:9090 用户名@服务器
```

常用命令：

```sh
mihomo-server-user status
mihomo-server-user logs
mihomo-server-user restart
mihomo-server-user disable    # 停止并禁用自己的实例，保留数据
```

升级时重新执行安装命令；其他正在运行的实例也会重启到新版本，其他停用实例保持停用。内核随共享安装统一升级，网页中不提供独立内核升级。

卸载共享安装，保留所有用户的数据、槽位、TUN 授权和 linger：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash -s -- --uninstall
```

删除自己的数据使用 `mihomo-server-user purge --yes`，需在共享程序卸载前执行。

构建和手动前台部署请参考 [部署文档](docs/DEPLOYMENT.md)。

## 许可证

本项目以 [GPL-3.0-only](LICENSE) 发布，衍生自 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)。第三方依赖的许可证见 [LICENSES.txt](LICENSES.txt)，代码来源见 [docs/UPSTREAM.md](docs/UPSTREAM.md)。
