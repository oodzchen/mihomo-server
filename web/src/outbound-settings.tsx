import { SettingsSection } from "./settings-section";
import { HelpTip } from "./help-tip";
import type { Draft, Runtime } from "./network-settings";
import { t, type Language } from "./i18n";

export const OUTBOUND_KEYS = new Set(["interface-name", "routing-mark"]);
const owned = "interface-name:owned";

export function validateOutbound(key: string, value: unknown, language: Language) {
  if (value == null) return;
  if (key === "interface-name") {
    if (typeof value !== "string" || new TextEncoder().encode(value).length > 15 || value === "." || value === ".." || /[/:]|\p{White_Space}|\p{Cc}/u.test(value))
      throw new Error(t(language, "obInterfaceError"));
  } else if (key === "routing-mark") {
    if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > 4294967295)
      throw new Error(t(language, "obMarkError"));
  }
}

export function outboundDraft(runtime: Runtime): Draft {
  return {
    [owned]: runtime["interface-name"] == null ? "" : "true",
    "interface-name": runtime["interface-name"] == null ? "" : String(runtime["interface-name"]),
    "routing-mark": runtime["routing-mark"] == null ? "" : String(runtime["routing-mark"]),
  };
}

export function outboundRuntime(draft: Draft, language: Language): Runtime {
  const runtime: Runtime = {};
  if (draft[owned] === "true") {
    const value = draft["interface-name"] ?? "";
    validateOutbound("interface-name", value, language);
    runtime["interface-name"] = value;
  }
  const mark = draft["routing-mark"] ?? "";
  if (mark !== "") {
    if (!/^\d+$/.test(mark)) throw new Error(t(language, "obMarkError"));
    const value = Number(mark);
    validateOutbound("routing-mark", value, language);
    runtime["routing-mark"] = value;
  }
  return runtime;
}

export function OutboundFields({ draft, disabled, onChange, language }: {
  draft: Draft;
  disabled: boolean;
  onChange: (key: string, value: string) => void;
  language: Language;
}) {
  const managed = draft[owned] === "true";
  return <SettingsSection title={t(language, "obTitle")}><fieldset className="network-fields" disabled={disabled}>
    <legend>{t(language, "obTitle")}</legend>
    <label><input type="checkbox" checked={managed} onChange={event => onChange(owned, event.target.checked ? "true" : "")} />{t(language, "obManageInterface")}</label>
    <label>{t(language, "obInterface")}<input aria-label={t(language, "obInterface")} disabled={!managed} value={draft["interface-name"] ?? ""} placeholder={t(language, "obInterfacePlaceholder")} aria-describedby="outbound-interface-hint" onChange={event => onChange("interface-name", event.target.value)} /><HelpTip id="outbound-interface-hint">{t(language, "obInterfaceHelp")}</HelpTip></label>
    <label>{t(language, "obMark")}<input aria-label={t(language, "obMark")} inputMode="numeric" value={draft["routing-mark"] ?? ""} placeholder={t(language, "setEmptyInherits")} aria-describedby="outbound-mark-hint" onChange={event => onChange("routing-mark", event.target.value)} /><HelpTip id="outbound-mark-hint">{t(language, "obMarkHelp")}</HelpTip></label>
  </fieldset></SettingsSection>;
}
