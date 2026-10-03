# Mihomo Server

无头运行的 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)：不需要桌面环境，以后台服务的形式在 Linux 服务器上运行 [Mihomo](https://github.com/MetaCubeX/mihomo) 内核，从任意浏览器远程管理订阅、节点和配置。

订阅、增强、节点选择等管理逻辑直接取自 Clash Verge Rev，用法与桌面版相近，适合服务器、软路由等无图形界面的设备。

## 安装

需要 x86_64 Linux，并已安装 systemd 和 curl（或 wget），无需 Python。以普通用户执行：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash
```

脚本会下载最新版本并校验，安装到 `~/.local/opt/mihomo-server`，注册为 systemd 用户服务并立即启动。数据保存在 `~/.local/share/mihomo-server`。

如需从其他设备访问管理页面，安装时指定监听地址和访问地址：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh \
  | bash -s -- --listen 0.0.0.0:9090 \
      --extra-args "--public-origin http://服务器IP:9090"
```

管理端口是明文 HTTP。经公网访问时，建议放在 HTTPS 反向代理之后（`--public-origin` 填写代理的公开地址），或直接用 SSH 隧道：`ssh -L 9090:127.0.0.1:9090 服务器`。

注销登录后也保持运行：

```sh
loginctl enable-linger "$USER"
```

## 使用

1. 打开 `http://127.0.0.1:9090`，输入令牌登录。令牌查看方式：

   ```sh
   cat ~/.local/share/mihomo-server/management-token
   ```

2. 在「订阅」页导入本地 YAML 或远程订阅链接，点击「使用订阅」。
3. 在「代理」页选择节点。
4. 将 HTTP/SOCKS 代理设为 `127.0.0.1:7890`（混合端口，可在「设置」页修改）。供其他设备使用时，在「设置」页开启「允许局域网访问」，代理地址改为服务器 IP。

管理服务：

```sh
systemctl --user status mihomo-server    # 查看状态
systemctl --user restart mihomo-server   # 重启
journalctl --user -u mihomo-server -f    # 查看日志
```

卸载（保留数据）：

```sh
systemctl --user disable --now mihomo-server
rm ~/.config/systemd/user/mihomo-server.service
rm -r ~/.local/opt/mihomo-server
systemctl --user daemon-reload
```

## 多用户安装（系统级）

一台机器有多个用户时，可以由管理员安装一次，每个用户各自启用自己的实例。订阅、设置、令牌和数据都各自独立，端口和 TUN 也不会互相冲突。

管理员（root）执行：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh \
  | sudo bash -s -- --system --tun-user alice --tun-user bob
```

- 程序安装到 `/opt/mihomo-server`，所有用户共用一份，普通用户无法修改。
- `--tun-user` 把用户加入 `mihomo-tun` 组，只有这个组的成员能使用 TUN 模式。该组成员实际上可以接管整台机器的网络，只授予可信的用户。也可以之后用 `sudo usermod -aG mihomo-tun 用户名` 添加；新加的组成员需要重新登录才生效。
- 升级：重新执行同一条命令即可，正在运行的各用户实例会自动重启到新版本。
- 卸载：把上面命令中的参数换成 `--system --uninstall`，不会删除任何用户的数据。

每个用户执行：

```sh
mihomo-server-user init      # 启用并启动自己的实例，显示访问地址、令牌、代理端口和 TUN 状态
loginctl enable-linger       # 可选：注销后继续运行
```

每个用户会分到一个编号 N（0–63），对应三个端口：管理页面 `20000+10N`，HTTP/SOCKS 代理 `20000+10N+1`，DNS `20000+10N+2`。第一个用户的管理页面就是 `http://127.0.0.1:20000`，代理是 `127.0.0.1:20001`。订阅里写死的端口会被自动替换；在「设置」页明确填写的端口会被保留。

开启 TUN 后，只有你自己账户下的程序走代理，其他用户不受影响。系统 DNS 由多个用户共用（例如 systemd-resolved），单个用户的 TUN 无法接管 DNS，所以此时会自动开启流量嗅探，从 TLS/HTTP 流量中识别域名，保证按域名的规则仍然生效。

其他命令：`mihomo-server-user info`、`token`、`status`、`logs`、`restart`、`disable`，以及 `purge --yes`（删除自己的全部数据）。

多用户模式下，内核由管理员统一升级（重新运行安装命令），网页上不提供内核升级。

需要自行构建或手动部署，请参考 [部署文档](docs/DEPLOYMENT.md)。

## 许可证

本项目以 [GPL-3.0-only](LICENSE) 发布，衍生自 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)。第三方依赖的许可证见 [LICENSES.txt](LICENSES.txt)，代码来源见 [docs/UPSTREAM.md](docs/UPSTREAM.md)。
