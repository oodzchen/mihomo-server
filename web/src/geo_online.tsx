import { useEffect, useRef, useState } from "react";
import { ApiError, command } from "./api";
import type { CoreStatus } from "./types";

type Info = { name: string; current_sha256: string | null; source_sha256: string };
type Receipt = { changed: boolean; durable: boolean; cleanup_pending: boolean; core_load_verified?: boolean; validation: { verified: boolean; sha256: string; format: string } };

export function GeoOnlineAction({ name, token, status, connection, logout, installed }: {
  name: string; token: string; status: CoreStatus; connection: string;
  logout: (reason?: string) => void; installed: (message: string) => void;
}) {
  const [info, setInfo] = useState<Info>();
  const [pin, setPin] = useState("");
  const [accept, setAccept] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const controller = useRef<AbortController | null>(null);
  const dat = name.endsWith(".dat");
  useEffect(() => {
    epoch.current++; controller.current?.abort();
    setInfo(undefined); setPin(""); setAccept(false); setError(""); setBusy(false);
    return () => { epoch.current++; controller.current?.abort(); };
  }, [name, token, status.phase, status.generation, status.config_revision, connection]);

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
        }, abort.signal);
        if (version !== epoch.current) return;
        if (dat && receipt.core_load_verified !== true) throw new Error("服务未确认 DAT 隔离内核规则加载。");
        installed(`${name}：${receipt.changed ? "已安装在线资源" : "当前文件已与下载资源一致"} · ${dat ? "DAT 结构及隔离内核规则加载通过" : receipt.validation.verified ? "MMDB 结构校验通过" : "描述为空，完整结构未验证"} · SHA-256 ${receipt.validation.sha256}${!receipt.durable || receipt.cleanup_pending ? " · 目录同步或暂存清理未完成，请核对文件状态" : ""}`);
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
      {!dat && <label><input type="checkbox" checked={accept} disabled={busy} onChange={event => setAccept(event.target.checked)} />允许安装描述为空、完整结构未验证的 MMDB</label>}
      {status.phase !== "stopped" && <p className="info">停止内核后可下载并安装在线 Geo 资源。</p>}
      <button type="button" disabled={busy || connection !== "已连接" || status.phase !== "stopped"} onClick={() => void run(true)}>更新 {name} 在线资源</button>
    </>}
    {busy && <p role="status">正在处理在线 Geo 资源…</p>}
    {error && <p role="alert" className="alert">在线 Geo 更新失败：{error}</p>}
  </div>;
}
