import { SettingsSection } from "./settings-section";
import { HelpTip } from "./help-tip";
import type { Draft, Runtime } from "./network-settings";
import { t, type Language } from "./i18n";

export const AUTHORITY_KEYS = new Set([
  "bind-address",
  "authentication",
  "skip-auth-prefixes",
  "lan-allowed-ips",
  "lan-disallowed-ips",
  "inbound-tfo",
  "inbound-mptcp",
  "sniffing",
]);

const ownedBind = "bind-address:owned";
const ownedAuth = "authentication:owned";
const ownedSkip = "skip-auth-prefixes:owned";
const ownedLanAllowed = "lan-allowed-ips:owned";
const ownedLanDisallowed = "lan-disallowed-ips:owned";

function isValidIpOrCidr(entry: string): boolean {
  const trimmed = entry.trim();
  if (!trimmed) return false;
  const parts = trimmed.split("/");
  if (parts.length === 1) {
    return isValidIp(parts[0]);
  }
  if (parts.length === 2) {
    const prefix = Number(parts[1]);
    if (!Number.isInteger(prefix) || prefix < 0) return false;
    if (parts[0].includes(":")) {
      return isValidIp(parts[0]) && prefix <= 128;
    }
    return isValidIp(parts[0]) && prefix <= 32;
  }
  return false;
}

function isValidIp(ip: string): boolean {
  if (ip.includes(":")) {
    // IPv6 basic structure validation
    return /^[0-9a-fA-F:]+$/.test(ip) && !ip.includes(":::") && (ip.match(/::/g) || []).length <= 1;
  }
  // IPv4 basic structure validation
  const octets = ip.split(".");
  if (octets.length !== 4) return false;
  return octets.every((octet) => {
    if (!/^\d+$/.test(octet)) return false;
    const num = Number(octet);
    return num >= 0 && num <= 255 && (octet === "0" || !octet.startsWith("0"));
  });
}

export function validateAuthority(key: string, value: unknown, language: Language) {
  if (value == null) return;
  if (key === "bind-address") {
    if (typeof value !== "string" || value.length > 255 || /\s/.test(value)) {
      throw new Error(t(language, "authBindSpace"));
    }
    if (value !== "*" && value !== "" && value.toLowerCase() !== "localhost" && !isValidIp(value)) {
      throw new Error(t(language, "authBindValue"));
    }
  } else if (key === "authentication") {
    if (!Array.isArray(value)) throw new Error(t(language, "authListType"));
    for (const item of value) {
      if (typeof item !== "string" || !item.includes(":") || !item.split(":")[0].trim()) {
        throw new Error(t(language, "authListFormat"));
      }
    }
  } else if (key === "skip-auth-prefixes" || key === "lan-allowed-ips" || key === "lan-disallowed-ips") {
    if (!Array.isArray(value)) throw new Error(t(language, "authCidrList", { key }));
    for (const item of value) {
      if (typeof item !== "string" || !isValidIpOrCidr(item)) {
        throw new Error(t(language, "authCidrItem", { key, item: String(item) }));
      }
    }
  } else if (key === "inbound-tfo" || key === "inbound-mptcp" || key === "sniffing") {
    if (typeof value !== "boolean") throw new Error(t(language, "authBool", { key }));
  }
}

export function authorityDraft(runtime: Runtime): Draft {
  const listToDraft = (key: string, ownedKey: string) => {
    const val = runtime[key];
    if (val == null) return { [ownedKey]: "", [key]: "" };
    if (Array.isArray(val)) {
      return {
        [ownedKey]: "true",
        [key]: val.length === 0 ? "[]" : val.join("\n"),
      };
    }
    return { [ownedKey]: "true", [key]: String(val) };
  };

  return {
    [ownedBind]: runtime["bind-address"] == null ? "" : "true",
    "bind-address": runtime["bind-address"] == null ? "" : String(runtime["bind-address"]),
    ...listToDraft("authentication", ownedAuth),
    ...listToDraft("skip-auth-prefixes", ownedSkip),
    ...listToDraft("lan-allowed-ips", ownedLanAllowed),
    ...listToDraft("lan-disallowed-ips", ownedLanDisallowed),
    "inbound-tfo": runtime["inbound-tfo"] == null ? "" : String(runtime["inbound-tfo"]),
    "inbound-mptcp": runtime["inbound-mptcp"] == null ? "" : String(runtime["inbound-mptcp"]),
    sniffing: runtime["sniffing"] == null ? "" : String(runtime["sniffing"]),
  };
}

function parseListDraft(text: string): string[] {
  const trimmed = text.trim();
  if (trimmed === "[]" || trimmed === "") return [];
  return trimmed
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

export function authorityRuntime(draft: Draft, language: Language): Runtime {
  const runtime: Runtime = {};

  if (draft[ownedBind] === "true") {
    const val = (draft["bind-address"] ?? "").trim();
    validateAuthority("bind-address", val, language);
    runtime["bind-address"] = val;
  }

  const parseListField = (key: string, ownedKey: string) => {
    if (draft[ownedKey] === "true") {
      const raw = draft[key] ?? "";
      const list = parseListDraft(raw);
      validateAuthority(key, list, language);
      runtime[key] = list;
    }
  };

  parseListField("authentication", ownedAuth);
  parseListField("skip-auth-prefixes", ownedSkip);
  parseListField("lan-allowed-ips", ownedLanAllowed);
  parseListField("lan-disallowed-ips", ownedLanDisallowed);

  for (const boolKey of ["inbound-tfo", "inbound-mptcp", "sniffing"] as const) {
    const val = draft[boolKey] ?? "";
    if (val !== "") {
      if (val !== "true" && val !== "false") throw new Error(t(language, "authValueInvalid", { key: boolKey }));
      runtime[boolKey] = val === "true";
    }
  }

  return runtime;
}

export function AuthorityFields({
  draft,
  disabled,
  onChange,
  language,
}: {
  draft: Draft;
  disabled: boolean;
  onChange: (key: string, value: string) => void;
  language: Language;
}) {
  return (
    <SettingsSection title={t(language, "authTitle")}><fieldset className="network-fields" disabled={disabled}>
      <legend>{t(language, "authTitle")}</legend>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedBind] === "true"}
          onChange={(e) => onChange(ownedBind, e.target.checked ? "true" : "")}
        />
        {t(language, "authManageBind")}
      </label>
      <label>
        {t(language, "authBind")}
        <input
          aria-label={t(language, "authBind")}
          value={draft["bind-address"] ?? ""}
          disabled={draft[ownedBind] !== "true"}
          placeholder={t(language, "authBindPlaceholder")}
          onChange={(e) => onChange("bind-address", e.target.value)}
        />
      <HelpTip>{t(language, "authBindHelp")}</HelpTip></label>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedAuth] === "true"}
          onChange={(e) => onChange(ownedAuth, e.target.checked ? "true" : "")}
        />
        {t(language, "authManageAuth")}<HelpTip>{t(language, "authManageAuthHelp")}</HelpTip>
      </label>
      <label>
        {t(language, "authList")}
        <textarea
          aria-label={t(language, "authListAria")}
          value={draft["authentication"] ?? ""}
          disabled={draft[ownedAuth] !== "true"}
          placeholder="user:password"
          rows={3}
          onChange={(e) => onChange("authentication", e.target.value)}
        />
      </label>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedSkip] === "true"}
          onChange={(e) => onChange(ownedSkip, e.target.checked ? "true" : "")}
        />
        {t(language, "authManageSkip")}<HelpTip>{t(language, "authManageSkipHelp")}</HelpTip>
      </label>
      <label>
        {t(language, "authSkip")}
        <textarea
          aria-label={t(language, "authSkipAria")}
          value={draft["skip-auth-prefixes"] ?? ""}
          disabled={draft[ownedSkip] !== "true"}
          placeholder="127.0.0.1/32"
          rows={2}
          onChange={(e) => onChange("skip-auth-prefixes", e.target.value)}
        />
      </label>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedLanAllowed] === "true"}
          onChange={(e) => onChange(ownedLanAllowed, e.target.checked ? "true" : "")}
        />
        {t(language, "authManageLanAllowed")}
      </label>
      <label>
        {t(language, "authLanAllowed")}
        <textarea
          aria-label={t(language, "authLanAllowedAria")}
          value={draft["lan-allowed-ips"] ?? ""}
          disabled={draft[ownedLanAllowed] !== "true"}
          placeholder="192.168.1.0/24"
          rows={2}
          onChange={(e) => onChange("lan-allowed-ips", e.target.value)}
        />
      </label>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedLanDisallowed] === "true"}
          onChange={(e) => onChange(ownedLanDisallowed, e.target.checked ? "true" : "")}
        />
        {t(language, "authManageLanDisallowed")}
      </label>
      <label>
        {t(language, "authLanDisallowed")}
        <textarea
          aria-label={t(language, "authLanDisallowedAria")}
          value={draft["lan-disallowed-ips"] ?? ""}
          disabled={draft[ownedLanDisallowed] !== "true"}
          placeholder="192.168.1.100/32"
          rows={2}
          onChange={(e) => onChange("lan-disallowed-ips", e.target.value)}
        />
      </label>

      <label>
        {t(language, "authTfo")}
        <select
          aria-label={t(language, "authTfoAria")}
          value={draft["inbound-tfo"] ?? ""}
          onChange={(e) => onChange("inbound-tfo", e.target.value)}
        >
          <option value="">{t(language, "setInherit")}</option>
          <option value="true">{t(language, "setEnable")}</option>
          <option value="false">{t(language, "setDisable")}</option>
        </select>
      </label>

      <label>
        {t(language, "authMptcp")}
        <select
          aria-label={t(language, "authMptcpAria")}
          value={draft["inbound-mptcp"] ?? ""}
          onChange={(e) => onChange("inbound-mptcp", e.target.value)}
        >
          <option value="">{t(language, "setInherit")}</option>
          <option value="true">{t(language, "setEnable")}</option>
          <option value="false">{t(language, "setDisable")}</option>
        </select>
      </label>

      <label>
        {t(language, "authSniffing")}
        <select
          aria-label={t(language, "authSniffingAria")}
          value={draft["sniffing"] ?? ""}
          onChange={(e) => onChange("sniffing", e.target.value)}
        >
          <option value="">{t(language, "setInherit")}</option>
          <option value="true">{t(language, "setEnable")}</option>
          <option value="false">{t(language, "setDisable")}</option>
        </select>
      </label>
    </fieldset></SettingsSection>
  );
}
