import { useEffect, useState } from "react";
import { ApiError, command, type Connection } from "./api";
import type { CoreStatus } from "./types";

type ConnectionValues = {
  allow_lan: boolean;
  bind_address: string;
  mode: string;
  ipv6: boolean;
  tun_enabled?: boolean;
};
export type Access = {
  running: boolean;
  has_config: boolean;
  core_error: string | null;
  ports: { key: string; configured: number; actual: number | null; setting: number | null }[];
  configured: ConnectionValues;
  reported: ConnectionValues | null;
  dns_enabled: boolean;
  tun_enabled: boolean;
  authentication_required: boolean | null;
};
const labels: Record<string, string> = {
  "mixed-port": "混合（HTTP / SOCKS）",
  port: "HTTP",
  "socks-port": "SOCKS",
  "redir-port": "重定向",
  "tproxy-port": "透明代理",
};

export function useProxyAccess({ token, status, connection, logout }: {
  token: string;
  status: CoreStatus;
  connection: Connection;
  logout: (reason?: string) => void;
}) {
  const [value, setValue] = useState<Access>();
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    setValue(undefined);
    setError("");
    if (connection !== "connected") return;
    let active = true;
    let pending: AbortController | undefined;
    const read = async () => {
      if (pending) return;
      const controller = new AbortController();
      pending = controller;
      try {
        const next = await command<Access>(token, "proxy_access", {}, controller.signal);
        if (active) { setValue(next); setError(""); }
      } catch (error) {
        if (!active) return;
        setValue(undefined);
        if (error instanceof ApiError && error.status === 401) logout("认证失效，请重新输入令牌。");
        else setError(error instanceof Error ? error.message : String(error));
      } finally { pending = undefined; }
    };
    void read();
    const timer = setInterval(() => void read(), 5000);
    return () => { active = false; clearInterval(timer); pending?.abort(); };
  }, [token, status.phase, status.generation, status.config_revision, connection, refresh, logout]);

  return { value, error, refresh: () => setRefresh(value => value + 1) };
}

export type ProxyAccessState = ReturnType<typeof useProxyAccess>;

export function ProxyAccessPanel(props: {
  token: string;
  status: CoreStatus;
  connection: Connection;
  logout: (reason?: string) => void;
  access: ProxyAccessState;
}) {
  const { value, error, refresh } = props.access;
  const { connection } = props;

  const live = value?.reported;
  const current = live || value?.configured;
  const mismatch = value?.ports.some(port => port.actual !== null && port.actual !== port.configured);
  const available = (key: string) => value?.ports.find(port => port.key === key)?.actual || 0;
  const http = available("mixed-port") || available("port");
  const socks = available("mixed-port") || available("socks-port");
  const binding = current?.bind_address || "*";
  const localHost = ["*", "0.0.0.0", "::", "[::]", "localhost"].includes(binding) ? "127.0.0.1" : binding;
  const address = (port: number) => `${localHost.includes(":") && !localHost.startsWith("[") ? `[${localHost}]` : localHost}:${port}`;
  return (
    <section className="panel proxy-access" aria-label="代理连接信息">
      <div className="panel-title">
        <h2>代理连接信息</h2>
        <button type="button" onClick={refresh} disabled={connection !== "connected"}>刷新连接信息</button>
      </div>
      {connection !== "connected" ? <p className="info">服务连接中断，连接信息待重新核对。</p> : error ? <p className="alert" role="alert">读取连接信息失败：{error}</p> : !value ? <p className="muted">正在读取连接信息…</p> : <>
        {!value.has_config && <p className="info">尚无运行配置，导入并使用订阅后显示配置端口。</p>}
        {!value.running && <p className="info">内核未运行，下方配置端口当前不可用。</p>}
        {value.core_error && <p className="alert" role="alert">无法读取内核实际端口：{value.core_error}。暂不能确认代理入口。</p>}
        {mismatch && <p className="alert" role="alert">配置端口与内核实际端口不一致，可能存在端口占用或加载失败。请查看日志并检查端口设置。</p>}
        <div className="proxy-ports-scroll">
          <table className="proxy-ports">
            <thead><tr><th>代理类型</th><th>配置端口</th><th>内核实际端口</th><th>设置来源</th></tr></thead>
            <tbody>{value.ports.map(port => <tr key={port.key}>
              <th scope="row">{labels[port.key]}</th>
              <td>{value.has_config ? port.configured || "禁用" : "—"}</td>
              <td className={port.actual !== null && port.actual !== port.configured ? "port-mismatch" : ""}>{port.actual === null ? "未确认" : port.actual || "未监听"}</td>
              <td>{port.setting === null ? "继承订阅 / 配置" : `服务设置：${port.setting || "禁用"}`}</td>
            </tr>)}</tbody>
          </table>
        </div>
        <dl className="proxy-details">
          <div><dt>监听地址{live ? "（内核报告）" : "（配置）"}</dt><dd className="mono">{binding}</dd></div>
          <div><dt>局域网访问</dt><dd>{current?.allow_lan ? "允许" : "仅本机"}</dd></div>
          <div><dt>代理模式</dt><dd>{{ rule: "规则", global: "全局", direct: "直连" }[current?.mode.toLowerCase() || ""] || current?.mode}</dd></div>
          <div><dt>IPv6</dt><dd>{current?.ipv6 ? "启用" : "禁用"}</dd></div>
          <div><dt>DNS / TUN（配置）</dt><dd>DNS {value.dns_enabled ? "启用" : "禁用"} · TUN {value.tun_enabled ? "启用" : "禁用"}</dd></div>
        </dl>
        {live && <div className="proxy-browser-settings" aria-label="浏览器代理填写参考">
          <strong>浏览器与服务在同一台机器时</strong>
          <p>HTTP / HTTPS 代理：<code>{http ? address(http) : "当前没有可用端口"}</code></p>
          <p>SOCKS v5 代理：<code>{socks ? address(socks) : "当前没有可用端口"}</code></p>
          {socks > 0 && <p className="hint">使用 SOCKS v5 时，可勾选 Firefox 的「使用 SOCKS v5 时代理 DNS」。</p>}
          {value.authentication_required && <p className="info">此代理要求认证，请使用订阅配置中的代理用户名和密码。</p>}
        </div>}
        <p className="hint">其他设备连接时填写服务主机的 IP，并开启局域网访问；* 是监听范围，不能直接填作代理地址。9090 是管理页面端口。</p>
        <p className="hint">端口来自内核报告，每 5 秒更新；不代表所选节点可以访问外网。未保存的设置草稿不影响此处显示。</p>
      </>}
    </section>
  );
}
