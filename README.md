# Mihomo Server

无头运行的 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)：不需要桌面环境，以后台服务的形式在 Linux 服务器上运行 [Mihomo](https://github.com/MetaCubeX/mihomo) 内核，从任意浏览器远程管理订阅、节点和配置。

订阅、增强、节点选择等管理逻辑直接取自 Clash Verge Rev，用法与桌面版相近，适合服务器、软路由等无图形界面的设备。

## 安装

需要 x86_64 Linux，并已安装 systemd、Python 3 和 curl（或 wget）。以普通用户执行：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash
```

脚本会下载最新版本并校验，安装到 `~/.local/opt/mihomo-server`，注册为 systemd 用户服务并立即启动。数据保存在 `~/.local/share/mihomo-server`。

如需从其他设备访问管理页面，安装时指定监听地址和访问地址：

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh \
  | bash -s -- --enable --start --listen 0.0.0.0:9090 \
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

需要自行构建或手动部署，请参考 [部署文档](docs/DEPLOYMENT.md)。

## 许可证

本项目以 [GPL-3.0-only](LICENSE) 发布，衍生自 [Clash Verge Rev](https://github.com/clash-verge-rev/clash-verge-rev)。第三方依赖的许可证见 [LICENSES.txt](LICENSES.txt)，代码来源见 [docs/UPSTREAM.md](docs/UPSTREAM.md)。
