import { SettingsSection } from "./settings-section";
import { HelpTip } from "./help-tip";
import type { Draft, Runtime } from "./network-settings";

export const DOWNLOAD_KEYS = new Set(["global-ua", "etag-support"]);
const owned = "global-ua:owned";
const agentError = "核心下载 User-Agent 必须是最多 1024 个可打印 ASCII 字符。";

export function validateDownload(key: string, value: unknown) {
  if (value == null) return;
  if (key === "global-ua") {
    if (typeof value !== "string" || value.length > 1024 || /[^\x20-\x7e]/.test(value)) throw new Error(agentError);
  } else if (key === "etag-support" && typeof value !== "boolean") throw new Error("核心下载 ETag 值无效。");
}
export function downloadDraft(runtime: Runtime): Draft {
  return {
    [owned]: runtime["global-ua"] == null ? "" : "true",
    "global-ua": runtime["global-ua"] == null ? "" : String(runtime["global-ua"]),
    "etag-support": runtime["etag-support"] == null ? "" : String(runtime["etag-support"]),
  };
}
export function downloadRuntime(draft: Draft): Runtime {
  const runtime: Runtime = {};
  if (draft[owned] === "true") {
    const value = draft["global-ua"] ?? "";
    validateDownload("global-ua", value); runtime["global-ua"] = value;
  }
  const etag = draft["etag-support"] ?? "";
  if (etag !== "") {
    if (etag !== "true" && etag !== "false") throw new Error("核心下载 ETag 值无效。");
    runtime["etag-support"] = etag === "true";
  }
  return runtime;
}
export function DownloadFields({ draft, disabled, onChange }: { draft: Draft; disabled: boolean; onChange: (key: string, value: string) => void }) {
  return <SettingsSection title="核心下载设置"><fieldset className="network-fields" disabled={disabled}>
    <legend>核心下载设置</legend>
    <label><input type="checkbox" checked={draft[owned] === "true"} onChange={event => onChange(owned, event.target.checked ? "true" : "")} />管理核心下载 User-Agent</label>
    <label>核心下载 User-Agent<input aria-label="核心下载 User-Agent" value={draft["global-ua"] ?? ""} disabled={draft[owned] !== "true"} placeholder="可显式留空" aria-describedby="download-agent-hint" onChange={event => onChange("global-ua", event.target.value)} /><HelpTip id="download-agent-hint">不勾选时继承配置；勾选后可自定义或显式留空。最多 1024 个可打印 ASCII 字符。</HelpTip></label>
    <label>核心下载 ETag<select aria-label="核心下载 ETag" value={draft["etag-support"] ?? ""} onChange={event => onChange("etag-support", event.target.value)}><option value="">继承</option><option value="true">启用</option><option value="false">禁用</option></select><HelpTip>用于核心的外部资源下载；资源自带的 User-Agent 请求头可能优先。服务导入或刷新订阅使用该订阅的下载选项。</HelpTip></label>
  </fieldset></SettingsSection>;
}
