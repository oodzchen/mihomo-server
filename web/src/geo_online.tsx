import { useEffect, useRef, useState } from "react";
import { ApiError, command } from "./api";
import type { CoreStatus } from "./types";

type Info = { name: string; current_sha256: string | null; source_sha256: string };
type Receipt = { changed: boolean; durable: boolean; cleanup_pending: boolean; core_load_verified?: boolean; validation: { verified: boolean; sha256: string; format: string } };
type Route = "direct" | "system" | "managed";

export function GeoOnlineAction({ name, token, status, connection, logout, installed }: {
  name: string; token: string; status: CoreStatus; connection: string;
  logout: (reason?: string) => void; installed: (message: string) => void;
}) {
  const [info, setInfo] = useState<Info>();
  const [pin, setPin] = useState("");
  const [route, setRoute] = useState<Route>("direct");
  const [invalidCerts, setInvalidCerts] = useState(false);
  const [accept, setAccept] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const controller = useRef<AbortController | null>(null);
  const dat = name.endsWith(".dat");
  useEffect(() => {
    epoch.current++; controller.current?.abort();
    setInfo(undefined); setPin(""); setRoute("direct"); setInvalidCerts(false); setAccept(false); setError(""); setBusy(false);
    return () => { epoch.current++; controller.current?.abort(); };
  }, [name, token, status.config_revision, connection]);

  async function run(update: boolean) {
    if (busy || (update && !info)) return;
    if (update && pin && !/^[a-fA-F0-9]{64}$/.test(pin)) {
      setError("下载 SHA-256 必须是 64 位十六进制值。"); return;
    }
    const version = epoch.current;
    const abort = new AbortController(); controller.current = abort;
    setBusy(true); setError("");
    if (!update) setInfo(undefined);
    try {
      if (update && info) {
        const receipt = await command<Receipt>(token, "update_geo_online", {
          name, expected_current_sha256: info.current_sha256,
          expected_source_sha256: info.source_sha256,
          expected_download_sha256: pin || null,
          accept_metadata_only: dat ? false : accept,
          route,
          danger_accept_invalid_certs: invalidCerts,
        }, abort.signal);
        if (version !== epoch.current) return;
        if (dat && receipt.core_load_verified !== true) throw new Error("服务未确认 DAT 隔离内核规则加载。");
        setInfo(undefined);
        installed(`${name}：${receipt.changed ? "已安装在线资源" : "当前文件已与下载资源一致"}${status.phase === "running" && receipt.changed ? " · 核心已重启验证" : ""} · ${dat ? "DAT 结构及隔离内核规则加载通过" : receipt.validation.verified ? "MMDB 结构校验通过" : "描述为空，完整结构未验证"} · SHA-256 ${receipt.validation.sha256}${!receipt.durable || receipt.cleanup_pending ? " · 目录同步或暂存清理未完成，请核对文件状态" : ""}`);
      } else {
        const next = await command<Info>(token, "geo_online_info", { name }, abort.signal);
        if (version === epoch.current) setInfo(next);
      }
    } catch (cause) {
      if (version !== epoch.current) return;
      if (cause instanceof ApiError && cause.status === 401) logout("认证失效，请重新输入令牌。");
      else setError(cause instanceof Error ? cause.message : String(cause));
      if (update) setInfo(undefined);
    } finally {
      if (version === epoch.current) setBusy(false);
    }
  }
  return <div>
    <button type="button" disabled={busy || connection !== "已连接"} onClick={() => void run(false)}>读取 {name} 在线来源</button>
    {info && <>
      <p>已提交来源指纹：<code>{info.source_sha256}</code></p>
      <p>当前文件：<code>{info.current_sha256 || "文件缺失"}</code></p>
      <label>可选下载 SHA-256 <input value={pin} disabled={busy} onChange={event => setPin(event.target.value.trim())} /></label>
      <label>下载路由 <select value={route} disabled={busy} onChange={event => setRoute(event.target.value as Route)}>
        <option value="direct">直连</option><option value="system">服务系统代理</option><option value="managed" disabled={status.phase !== "running"}>运行中核心代理</option>
      </select></label>
      <label><input type="checkbox" checked={invalidCerts} disabled={busy} onChange={event => setInvalidCerts(event.target.checked)} />显式忽略下载来源证书错误</label>
      <p className="info">默认验证平台证书，证书链失败时会尝试静态根证书。忽略证书错误仅用于可信来源。</p>
      {!dat && <label><input type="checkbox" checked={accept} disabled={busy} onChange={event => setAccept(event.target.checked)} />允许安装描述为空、完整结构未验证的 MMDB</label>}
      {status.phase === "running" && <p className="info">更新时将短暂停止核心，验证新资源后重启；失败会尝试恢复旧文件和核心。</p>}
      {!(["running", "stopped"] as string[]).includes(status.phase) && <p className="info">核心进入运行或停止状态后才能更新在线 Geo 资源。</p>}
      <button type="button" disabled={busy || connection !== "已连接" || !(["running", "stopped"] as string[]).includes(status.phase) || (route === "managed" && status.phase !== "running")} onClick={() => void run(true)}>更新 {name} 在线资源</button>
    </>}
    {busy && <p role="status">正在处理在线 Geo 资源…</p>}
    {error && <p role="alert" className="alert">在线 Geo 更新失败：{error}</p>}
  </div>;
}
