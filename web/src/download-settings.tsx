import { SettingsSection } from "./settings-section";
import { HelpTip } from "./help-tip";
import type { Draft, Runtime } from "./network-settings";
import { t, type Language } from "./i18n";

export const DOWNLOAD_KEYS = new Set(["global-ua", "etag-support"]);
const owned = "global-ua:owned";

export function validateDownload(key: string, value: unknown, language: Language) {
  if (value == null) return;
  if (key === "global-ua") {
    if (typeof value !== "string" || value.length > 1024 || /[^\x20-\x7e]/.test(value)) throw new Error(t(language, "dlAgentError"));
  } else if (key === "etag-support" && typeof value !== "boolean") throw new Error(t(language, "setInvalidValue", { label: t(language, "dlEtag") }));
}
export function downloadDraft(runtime: Runtime): Draft {
  return {
    [owned]: runtime["global-ua"] == null ? "" : "true",
    "global-ua": runtime["global-ua"] == null ? "" : String(runtime["global-ua"]),
    "etag-support": runtime["etag-support"] == null ? "" : String(runtime["etag-support"]),
  };
}
export function downloadRuntime(draft: Draft, language: Language): Runtime {
  const runtime: Runtime = {};
  if (draft[owned] === "true") {
    const value = draft["global-ua"] ?? "";
    validateDownload("global-ua", value, language); runtime["global-ua"] = value;
  }
  const etag = draft["etag-support"] ?? "";
  if (etag !== "") {
    if (etag !== "true" && etag !== "false") throw new Error(t(language, "setInvalidValue", { label: t(language, "dlEtag") }));
    runtime["etag-support"] = etag === "true";
  }
  return runtime;
}
export function DownloadFields({ draft, disabled, onChange, language }: { draft: Draft; disabled: boolean; onChange: (key: string, value: string) => void; language: Language }) {
  return <SettingsSection title={t(language, "dlTitle")}><fieldset className="network-fields" disabled={disabled}>
    <legend>{t(language, "dlTitle")}</legend>
    <label><input type="checkbox" checked={draft[owned] === "true"} onChange={event => onChange(owned, event.target.checked ? "true" : "")} />{t(language, "dlManageAgent")}</label>
    <label>{t(language, "dlAgent")}<input aria-label={t(language, "dlAgent")} value={draft["global-ua"] ?? ""} disabled={draft[owned] !== "true"} placeholder={t(language, "dlAgentPlaceholder")} aria-describedby="download-agent-hint" onChange={event => onChange("global-ua", event.target.value)} /><HelpTip id="download-agent-hint">{t(language, "dlAgentHelp")}</HelpTip></label>
    <label>{t(language, "dlEtag")}<select aria-label={t(language, "dlEtag")} value={draft["etag-support"] ?? ""} onChange={event => onChange("etag-support", event.target.value)}><option value="">{t(language, "setInherit")}</option><option value="true">{t(language, "setEnable")}</option><option value="false">{t(language, "setDisable")}</option></select><HelpTip>{t(language, "dlEtagHelp")}</HelpTip></label>
  </fieldset></SettingsSection>;
}
