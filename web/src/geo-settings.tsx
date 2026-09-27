import { SettingsReadback } from "./settings-readback";
import type { CoreStatus } from "./types";
import type { Runtime, Draft } from "./network-settings";

const scalars = [
  { key: "geodata-mode", label: "Geo 数据模式", values: [["true", "DAT"], ["false", "MMDB"]] },
  { key: "geodata-loader", label: "Geo 加载器", values: [["standard", "standard"], ["memconservative", "memconservative"]] },
  { key: "geosite-matcher", label: "GeoSite 匹配器", values: [["succinct", "succinct"], ["mph", "mph"]] },
  { key: "geo-auto-update", label: "Geo 自动更新", values: [["true", "启用"], ["false", "禁用"]] },
] as const;
const urls = [["geoip", "GeoIP 下载地址"], ["geosite", "GeoSite 下载地址"], ["mmdb", "MMDB 下载地址"], ["asn", "ASN 下载地址"]] as const;
export const GEO_KEYS = new Set(["geodata-mode", "geodata-loader", "geosite-matcher", "geo-auto-update", "geo-update-interval", "geox-url"]);
function validUrl(value: string) {
  if (!value || new TextEncoder().encode(value).length > 8192 || /\s|[\x00-\x1f\x7f-\x9f]/.test(value) || value.includes("#")) return false;
  try {
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) && !!url.hostname && !url.username && !url.password;
  } catch { return false; }
}
export function validateGeo(runtime: Runtime) {
  for (const f of scalars) {
    const value = runtime[f.key];
    if (value == null) continue;
    const boolean = f.key === "geodata-mode" || f.key === "geo-auto-update";
    if (boolean ? typeof value !== "boolean" : typeof value !== "string" || !f.values.some(([v]) => v === value)) throw new Error(`${f.label}值无效。`);
  }
  const interval = runtime["geo-update-interval"];
  if (interval != null && (typeof interval !== "number" || !Number.isInteger(interval) || interval < 1 || interval > 8760)) throw new Error("Geo 更新间隔必须是 1–8760 小时的整数，或留空继承。");
  const value = runtime["geox-url"];
  if (value != null) {
    if (typeof value !== "object" || Array.isArray(value)) throw new Error("Geo 下载地址设置无效。");
    for (const [key, url] of Object.entries(value)) {
      if (!urls.some(([name]) => name === key)) throw new Error(`不支持的 Geo 下载设置 ${key}，请勿覆盖。`);
      if (url != null && (typeof url !== "string" || !validUrl(url))) throw new Error("Geo 下载地址必须为 HTTP(S) URL，不能含空白、用户名、密码或片段。");
    }
  }
}
export function geoDraft(runtime: Runtime): Draft {
  const result: Draft = {};
  for (const key of GEO_KEYS) if (key !== "geox-url") result[key] = runtime[key] == null ? "" : String(runtime[key]);
  result["geox-url"] = runtime["geox-url"] == null ? "" : "true";
  for (const [key] of urls) result[`geox-url.${key}`] = String((runtime["geox-url"] as Record<string, unknown> | undefined)?.[key] ?? "");
  return result;
}
export function geoRuntime(draft: Draft): Runtime {
  const result: Runtime = {};
  for (const f of scalars) {
    const value = draft[f.key] ?? "";
    if (value === "") continue;
    if (!f.values.some(([option]) => option === value)) throw new Error(`${f.label}值无效。`);
    result[f.key] = f.key === "geodata-mode" || f.key === "geo-auto-update" ? value === "true" : value;
  }
  const interval = draft["geo-update-interval"] ?? "";
  if (interval !== "") {
    if (!/^\d+$/.test(interval)) throw new Error("Geo 更新间隔必须是 1–8760 小时的整数，或留空继承。");
    result["geo-update-interval"] = Number(interval);
  }
  if (draft["geox-url"] === "true") {
    const value: Record<string, string> = {};
    for (const [key] of urls) if (draft[`geox-url.${key}`]) value[key] = draft[`geox-url.${key}`];
    result["geox-url"] = value;
  }
  validateGeo(result);
  return result;
}
export function GeoFields({ draft, disabled, change }: { draft: Draft; disabled: boolean; change: (key: string, value: string) => void }) {
  return <fieldset className="network-fields" disabled={disabled}>
    <legend>Geo 设置</legend>
    <p className="hint">留空或继承时保留订阅值。更新间隔单位为小时；启用自动更新后由 Mihomo 自行下载，不提供服务侧更新回滚。模式切换不保证所需数据库已准备好。</p>
    <div className="settings-fields">
      {scalars.map(f => <label key={f.key}>{f.label}<select aria-label={f.label} value={draft[f.key] ?? ""} onChange={event => change(f.key, event.target.value)}>
        <option value="">继承</option>{f.values.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
      </select></label>)}
      <label>Geo 更新间隔（小时）<input aria-label="Geo 更新间隔（小时）" inputMode="numeric" placeholder="留空继承" value={draft["geo-update-interval"] ?? ""} onChange={event => change("geo-update-interval", event.target.value)} /></label>
    </div>
    <label><input type="checkbox" checked={draft["geox-url"] === "true"} onChange={event => change("geox-url", event.target.checked ? "true" : "")} />管理 Geo 下载地址</label>
    <div className="settings-fields">{urls.map(([key, label]) => <label key={key}>{label}<input aria-label={label} disabled={draft["geox-url"] !== "true"} placeholder="留空继承此地址" value={draft[`geox-url.${key}`] ?? ""} onChange={event => change(`geox-url.${key}`, event.target.value)} /></label>)}</div>
  </fieldset>;
}

export function GeoReadback(props: { token: string; status: CoreStatus; connection: string; logout: (reason?: string) => void; settingsKey?: string }) {
  return <SettingsReadback {...props} label="Geo 设置读回" refreshLabel="刷新 Geo 设置读回" operation="geo_settings" hint="读回不证明 Geo 文件有效或已被规则加载。未指定的配置项可能使用内核默认值。" />;
}
