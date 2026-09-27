import { useEffect, useRef, useState } from "react";
import { ApiError, command } from "./api";
import type { CoreStatus } from "./types";

type Seed = { name: string; current_sha256: string | null; seed_sha256: string; seed_bytes: number };
type Receipt = { changed: boolean; durable: boolean; cleanup_pending: boolean; validation: { verified: boolean; sha256: string } };

export function GeoSeedAction({ name, token, status, connection, logout, installed }: {
  name: string; token: string; status: CoreStatus; connection: string;
  logout: (reason?: string) => void; installed: (message: string) => void;
}) {
  const [seed, setSeed] = useState<Seed>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [accept, setAccept] = useState(false);
  const controller = useRef<AbortController | null>(null);
  const epoch = useRef(0);
  useEffect(() => {
    epoch.current++;
    controller.current?.abort();
    setSeed(undefined); setError(""); setBusy(false); setAccept(false);
    return () => { epoch.current++; controller.current?.abort(); };
  }, [name, token, status.phase, status.generation, status.config_revision, connection]);

  async function run(install: boolean) {
    if (busy || (install && !seed)) return;
    const version = epoch.current;
    const abort = new AbortController(); controller.current = abort;
    setBusy(true); setError("");
    if (!install) setSeed(undefined);
    try {
      if (install && seed) {
        const receipt = await command<Receipt>(token, "install_geo_seed", {
          name, expected_current_sha256: seed.current_sha256,
          expected_seed_sha256: seed.seed_sha256, accept_metadata_only: accept,
        }, abort.signal);
        if (version !== epoch.current) return;
        installed(`${name}：${receipt.changed ? "已安装打包资源" : "当前文件已与打包资源一致"} · ${receipt.validation.verified ? "MMDB 结构校验通过" : "描述为空，完整结构未验证"} · SHA-256 ${receipt.validation.sha256}${!receipt.durable || receipt.cleanup_pending ? " · 目录同步或暂存清理未完成，请核对文件状态" : ""}`);
      } else {
        const next = await command<Seed>(token, "geo_seed", { name }, abort.signal);
        if (version === epoch.current) setSeed(next);
      }
    } catch (error) {
      if (version !== epoch.current) return;
      if (error instanceof ApiError && error.status === 401) logout("认证失效，请重新输入令牌。");
      else setError(error instanceof Error ? error.message : String(error));
      // A failed/ambiguous install must be inspected again, never retried with stale state.
      if (install) setSeed(undefined);
    } finally {
      if (version === epoch.current) setBusy(false);
    }
  }
  return <div>
    <button type="button" disabled={busy || connection !== "已连接"} onClick={() => void run(false)}>读取 {name} 打包更新</button>
    {seed && <>
      <p>候选：{seed.seed_bytes} 字节 · SHA-256 <code>{seed.seed_sha256}</code></p>
      <p>当前：<code>{seed.current_sha256 || "文件缺失"}</code></p>
      <label><input type="checkbox" checked={accept} disabled={busy} onChange={event => setAccept(event.target.checked)} />允许安装描述为空、完整结构未验证的 MMDB</label>
      {status.phase !== "stopped" && <p className="info">停止内核后可安装打包资源。</p>}
      <button type="button" disabled={busy || connection !== "已连接" || status.phase !== "stopped"} onClick={() => void run(true)}>安装 {name} 打包资源</button>
    </>}
    {busy && <p role="status">正在处理打包资源…</p>}
    {error && <p role="alert" className="alert">打包资源操作失败：{error}</p>}
  </div>;
}
