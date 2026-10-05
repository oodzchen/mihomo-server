import { SettingsSection } from "./settings-section";
import { HelpTip } from "./help-tip";
import type { Connection } from "./api";
import { SettingsReadback } from "./settings-readback";
import type { CoreStatus } from "./types";
import type { Runtime, Draft } from "./network-settings";
import { t, type Language, type MessageKey } from "./i18n";

// Option labels that are message keys are translated; the rest are shown as is.
const scalars = [
  { key: "geodata-mode", label: "geoSetDataMode", values: [["true", "DAT"], ["false", "MMDB"]] },
  { key: "geodata-loader", label: "geoSetLoader", values: [["standard", "standard"], ["memconservative", "memconservative"]] },
  { key: "geosite-matcher", label: "geoSetMatcher", values: [["succinct", "succinct"], ["mph", "mph"]] },
  { key: "geo-auto-update", label: "geoSetAutoUpdate", values: [["true", "setEnable"], ["false", "setDisable"]] },
] as const;
const translated = new Set<string>(["setEnable", "setDisable"]);
const urls = [["geoip", "geoSetUrlGeoip"], ["geosite", "geoSetUrlGeosite"], ["mmdb", "geoSetUrlMmdb"], ["asn", "geoSetUrlAsn"]] as const;
export const GEO_KEYS = new Set(["geodata-mode", "geodata-loader", "geosite-matcher", "geo-auto-update", "geo-update-interval", "geox-url"]);
function validUrl(value: string) {
  if (!value || new TextEncoder().encode(value).length > 8192 || /\s|[\x00-\x1f\x7f-\x9f]/.test(value) || value.includes("#")) return false;
  try {
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) && !!url.hostname && !url.username && !url.password;
  } catch { return false; }
}
export function validateGeo(runtime: Runtime, language: Language) {
  for (const f of scalars) {
    const value = runtime[f.key];
    if (value == null) continue;
    const boolean = f.key === "geodata-mode" || f.key === "geo-auto-update";
    if (boolean ? typeof value !== "boolean" : typeof value !== "string" || !f.values.some(([v]) => v === value)) throw new Error(t(language, "setInvalidValue", { label: t(language, f.label) }));
  }
  const interval = runtime["geo-update-interval"];
  if (interval != null && (typeof interval !== "number" || !Number.isInteger(interval) || interval < 1 || interval > 8760)) throw new Error(t(language, "geoSetIntervalError"));
  const value = runtime["geox-url"];
  if (value != null) {
    if (typeof value !== "object" || Array.isArray(value)) throw new Error(t(language, "geoSetUrlsInvalid"));
    for (const [key, url] of Object.entries(value)) {
      if (!urls.some(([name]) => name === key)) throw new Error(t(language, "geoSetUrlUnsupported", { key }));
      if (url != null && (typeof url !== "string" || !validUrl(url))) throw new Error(t(language, "geoSetUrlError"));
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
export function geoRuntime(draft: Draft, language: Language): Runtime {
  const result: Runtime = {};
  for (const f of scalars) {
    const value = draft[f.key] ?? "";
    if (value === "") continue;
    if (!f.values.some(([option]) => option === value)) throw new Error(t(language, "setInvalidValue", { label: t(language, f.label) }));
    result[f.key] = f.key === "geodata-mode" || f.key === "geo-auto-update" ? value === "true" : value;
  }
  const interval = draft["geo-update-interval"] ?? "";
  if (interval !== "") {
    if (!/^\d+$/.test(interval)) throw new Error(t(language, "geoSetIntervalError"));
    result["geo-update-interval"] = Number(interval);
  }
  if (draft["geox-url"] === "true") {
    const value: Record<string, string> = {};
    for (const [key] of urls) if (draft[`geox-url.${key}`]) value[key] = draft[`geox-url.${key}`];
    result["geox-url"] = value;
  }
  validateGeo(result, language);
  return result;
}
export function GeoFields({ draft, disabled, change, language }: { draft: Draft; disabled: boolean; change: (key: string, value: string) => void; language: Language }) {
  return <SettingsSection title={t(language, "geoSetTitle")}><fieldset className="network-fields" disabled={disabled}>
    <legend>{t(language, "geoSetTitle")} <HelpTip>{t(language, "geoSetHelp")}</HelpTip></legend>
    <div className="settings-fields">
      {scalars.map(f => <label key={f.key}>{t(language, f.label)}<select aria-label={t(language, f.label)} value={draft[f.key] ?? ""} onChange={event => change(f.key, event.target.value)}>
        <option value="">{t(language, "setInherit")}</option>{f.values.map(([v, label]) => <option key={v} value={v}>{translated.has(label) ? t(language, label as MessageKey) : label}</option>)}
      </select></label>)}
      <label>{t(language, "geoSetInterval")}<input aria-label={t(language, "geoSetInterval")} inputMode="numeric" placeholder={t(language, "setEmptyInherits")} value={draft["geo-update-interval"] ?? ""} onChange={event => change("geo-update-interval", event.target.value)} /></label>
    </div>
    <label><input type="checkbox" checked={draft["geox-url"] === "true"} onChange={event => change("geox-url", event.target.checked ? "true" : "")} />{t(language, "geoSetManageUrls")}</label>
    <div className="settings-fields">{urls.map(([key, label]) => <label key={key}>{t(language, label)}<input aria-label={t(language, label)} disabled={draft["geox-url"] !== "true"} placeholder={t(language, "geoSetUrlPlaceholder")} value={draft[`geox-url.${key}`] ?? ""} onChange={event => change(`geox-url.${key}`, event.target.value)} /></label>)}</div>
  </fieldset></SettingsSection>;
}

export function GeoReadback(props: { token: string; status: CoreStatus; connection: Connection; logout: (reason?: string) => void; settingsKey?: string }) {
  return <SettingsReadback {...props} label="Geo 设置读回" refreshLabel="刷新 Geo 设置读回" operation="geo_settings" hint="读回不证明 Geo 文件有效或已被规则加载。未指定的配置项可能使用内核默认值。" />;
}
