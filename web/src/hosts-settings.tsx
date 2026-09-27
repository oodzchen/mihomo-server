import type { Draft, Runtime } from "./network-settings";

type Hosts = Record<string, string | string[]>;
const error = "hosts 必须是 JSON 对象：域名对应 IP、域名别名、lan 或非空 IP 字符串列表。";
function domain(value: string, pattern: boolean): boolean {
  if (!value || value.length > 253) return false;
  const parts = value.split(".");
  return parts.every((part, i) =>
    pattern && (part === "*" || (i === 0 && parts.length > 1 && (part === "" || part === "+"))) ||
    part.length > 0 && part.length <= 63 && !part.startsWith("-") && !part.endsWith("-") && /^[A-Za-z0-9_-]+$/.test(part));
}
function ip(value: string): boolean {
  if (/^\d+\.\d+\.\d+\.\d+$/.test(value)) return value.split(".").every(p => /^(0|[1-9]\d{0,2})$/.test(p) && Number(p) <= 255);
  if (!value.includes(":") || !/^[a-fA-F0-9:.]+$/.test(value)) return false;
  try { new URL(`http://[${value}]/`); return true; } catch { return false; }
}
function matches(pattern: string, name: string): boolean {
  const suffix = pattern.startsWith(".") || pattern.startsWith("+.");
  const parts = pattern.replace(/^(\+\.|\.)/, "").split(".").reverse();
  const labels = name.split(".").reverse();
  if (suffix ? labels.length < parts.length || pattern.startsWith(".") && labels.length === parts.length : labels.length !== parts.length) return false;
  return parts.every((p, i) => p === "*" || p === labels[i]);
}
export function validateHosts(value: unknown): asserts value is Hosts {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(error);
  const entries = Object.entries(value);
  if (entries.length > 1024) throw new Error("hosts 最多支持 1024 条映射。");
  const keys = new Set<string>();
  const aliases = new Map<string, string>();
  for (const [key, v] of entries) {
    const lower = key.toLowerCase();
    if (!domain(key, true) || keys.has(lower)) throw new Error("hosts 域名格式无效或存在大小写重复；国际化域名请使用 punycode。");
    keys.add(lower);
    if (typeof v === "string") {
      if (v === "lan" || ip(v)) continue;
      if (!v.includes(".") || !domain(v, false)) throw new Error(error);
      aliases.set(lower, v.toLowerCase());
    } else if (!Array.isArray(v) || v.length < 1 || v.length > 64 || !v.every(x => typeof x === "string" && ip(x))) throw new Error(error);
  }
  const names = [...aliases.keys()], incoming = names.map(() => 0), edges = names.map(() => [] as number[]);
  names.forEach((key, i) => names.forEach((pattern, j) => { if (matches(pattern, aliases.get(key)!)) { edges[i].push(j); incoming[j]++; } }));
  const ready = incoming.flatMap((n, i) => n === 0 ? [i] : []);
  for (let i = 0; i < ready.length; i++) for (const j of edges[ready[i]]) if (--incoming[j] === 0) ready.push(j);
  if (ready.length !== names.length) throw new Error("hosts 含有可能的域名别名循环。");
}
export function hostsDraft(runtime: Runtime): Draft {
  return { "hosts:owned": runtime.hosts == null ? "" : "true", hosts: runtime.hosts == null ? "{}" : JSON.stringify(runtime.hosts, null, 2) };
}
export function hostsRuntime(draft: Draft): Runtime {
  if (draft["hosts:owned"] !== "true") return {};
  let hosts: unknown;
  try { hosts = JSON.parse(draft.hosts ?? "{}"); } catch { throw new Error(error); }
  validateHosts(hosts);
  // Match the backend BTreeMap ordering before full-replacement comparison.
  return { hosts: Object.fromEntries(Object.entries(hosts).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) };
}
export function HostsFields({ draft, disabled, change }: { draft: Draft; disabled: boolean; change: (key: string, value: string) => void }) {
  return <fieldset className="network-fields" disabled={disabled}>
    <legend>hosts 映射</legend>
    <label><input type="checkbox" checked={draft["hosts:owned"] === "true"} onChange={e => change("hosts:owned", e.target.checked ? "true" : "")} />管理 hosts 映射</label>
    <label>hosts JSON 映射<textarea aria-label="hosts JSON 映射" rows={6} disabled={draft["hosts:owned"] !== "true"} value={draft.hosts ?? "{}"} onChange={e => change("hosts", e.target.value)} /></label>
    <p className="hint">不勾选继承；勾选后替换整个映射，{'{}'} 清除配置中的映射。支持通配域名、IPv4/IPv6、域名别名和 lan；IP 列表最多 64 项。需要当前订阅允许 DNS 覆盖。</p>
    <p className="muted">DNS 设置中的两个 hosts 开关可显式启用或禁用。系统 hosts 只控制读取，不修改主机文件；核心仍可能保留内置 localhost。生成结果请在配置页核对，运行效果需通过 DNS 查询验证。</p>
  </fieldset>;
}
