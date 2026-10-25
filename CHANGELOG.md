# 更新日志

发布说明按版本维护，最新版本在前。发布步骤见 [docs/RELEASING.md](docs/RELEASING.md)。

## [Unreleased]

## [v0.3.12] - 2026-10-25

### 修复

- 修复在 Android 宿主网络中（例如 Droidspaces 以 host 网络运行的 Arch）无法开启 TUN 的问题（报错 `add route 0: network is unreachable`）。服务会自动识别 Android 的路由方式并补上所需的路由规则，无需手动设置。此时 TUN 接管整台设备（包括 Android 应用），仅支持 IPv4；服务自身的出站流量不经过 Android VPN（如 Clash Meta for Android），两者同时开启也只会代理一次。

## [v0.3.11] - 2026-10-25

v0.3.10 未能发布，其改动包含在本版本中。

### 修复

- 修复在 Arch Linux（含 Arch Linux ARM）等未提供 `sg` 的系统上安装失败的问题（报错 `missing required tool: sg`）。没有 `sg` 时，安装程序改用系统自带的 `newgrp` 授予 TUN 权限，无需重新登录或重启。
- 直接推送版本标签即可发布：未填写对应版本的发布说明时，自动根据上个版本以来的提交生成，不再因缺少说明阻止发布。

## [v0.3.9] - 2026-10-25

### 新增

- 发布流程从本文件提取对应版本的说明，并同步到 GitHub Release 页面。
- Web 管理界面发现服务新版本后，获取该版本的最新发布说明并弹窗展示，确认后才执行升级。

### 改进

- 再次打开升级弹窗时重新获取发布说明；获取失败可重试，不会直接开始升级。
