import { HelpTip } from "./help-tip";
import { useEffect, useState } from "react";
import { ApiError, command, type Connection } from "./api";
import type { CoreStatus } from "./types";

type Snapshot = { config_revision: string | null; running: boolean; error: string | null; fields: { key: string; setting: unknown; configured: unknown; actual: unknown; mismatch: boolean }[] };
const show = (value: unknown, absent: string) => value == null ? absent : typeof value === "string" ? (value === "" ? '""' : value) : JSON.stringify(value);
export function SettingsReadback({ token, status, connection, logout, settingsKey, label, operation, hint, refreshLabel }: { token: string; status: CoreStatus; connection: Connection; logout: (reason?: string) => void; settingsKey?: string; label: string; operation: string; hint: string; refreshLabel?: string }) {
  const [value, setValue] = useState<Snapshot>();
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    setValue(undefined); setError("");
    if (connection !== "connected") return;
    let active = true;
    const controller = new AbortController();
    void command<Snapshot>(token, operation, {}, controller.signal).then(value => { if (active) setValue(value); }).catch(error => {
      if (!active) return;
      if (error instanceof ApiError && error.status === 401) logout("认证失效，请重新输入令牌。");
      else setError(error instanceof Error ? error.message : String(error));
    });
    return () => { active = false; controller.abort(); };
  }, [token, status.phase, status.generation, status.config_revision, connection, refresh, logout, settingsKey, operation]);
  return <section className="panel" aria-label={label}>
    <div className="panel-title"><h2>{label}</h2><button type="button" disabled={connection !== "connected"} onClick={() => setRefresh(v => v + 1)}>{refreshLabel ?? `刷新${label}`}</button></div>
    {connection !== "connected" ? <p>服务连接中断，实际值待核对。</p> : error ? <p role="alert" className="alert">读取失败：{error}</p> : !value ? <p>正在读取设置…</p> : <>
      {value.error && <p role="alert" className="alert">无法读取内核设置，请重试。</p>}
      {!value.config_revision && <p>尚无已提交配置。</p>}
      {!value.running && <p>内核未运行，实际值未确认。</p>}
      <ul className="resource-list">{value.fields.map(field => <li key={field.key}><strong>{field.key}</strong>
        <p>服务设置：{show(field.setting, "继承")}</p><p>配置值：{show(field.configured, "未指定")}</p><p>内核实际值：{show(field.actual, "未确认")}</p>
        {field.mismatch && <p className="alert">配置值与内核实际值不一致。</p>}
      </li>)}</ul>
      <HelpTip>{hint}</HelpTip>
    </>}
  </section>;
}
