import { SettingsSection } from "./settings-section";
import { HelpTip } from "./help-tip";
import { t, type Language, type MessageKey } from "./i18n";
type Value = number | boolean | string | string[] | Record<string, unknown>;
export type Runtime = Record<string, Value | Record<string, Value>>;
export type Draft = Record<string, string>;
type Field = {
  key: string;
  label: MessageKey;
  kind: "bool" | "string" | "list" | "mtu" | "select" | "policy" | "filter";
  options?: string[];
};
const schemas: Record<"dns" | "tun", Field[]> = {
  dns: [
    { key: "enable", label: "netDnsEnable", kind: "bool" },
    { key: "ipv6", label: "netDnsIpv6", kind: "bool" },
    { key: "use-hosts", label: "netDnsUseHosts", kind: "bool" },
    { key: "use-system-hosts", label: "netDnsUseSystemHosts", kind: "bool" },
    { key: "listen", label: "netDnsListen", kind: "string" },
    {
      key: "enhanced-mode",
      label: "netDnsEnhancedMode",
      kind: "select",
      options: ["fake-ip", "redir-host"],
    },
    {
      key: "fake-ip-filter-mode",
      label: "netDnsFakeIpFilterMode",
      kind: "select",
      options: ["blacklist", "whitelist"],
    },
    { key: "prefer-h3", label: "netDnsPreferH3", kind: "bool" },
    { key: "respect-rules", label: "netDnsRespectRules", kind: "bool" },
    { key: "fake-ip-range", label: "netDnsFakeIpRange", kind: "string" },
    { key: "fake-ip-range6", label: "netDnsFakeIpRange6", kind: "string" },
    { key: "default-nameserver", label: "netDnsDefaultNameserver", kind: "list" },
    { key: "nameserver", label: "netDnsNameserver", kind: "list" },
    { key: "fallback", label: "netDnsFallback", kind: "list" },
    { key: "proxy-server-nameserver", label: "netDnsProxyServerNameserver", kind: "list" },
    { key: "direct-nameserver", label: "netDnsDirectNameserver", kind: "list" },
    { key: "direct-nameserver-follow-policy", label: "netDnsDirectFollowPolicy", kind: "bool" },
    { key: "nameserver-policy", label: "netDnsNameserverPolicy", kind: "policy" },
    { key: "proxy-server-nameserver-policy", label: "netDnsProxyServerNameserverPolicy", kind: "policy" },
    { key: "fallback-filter", label: "netDnsFallbackFilter", kind: "filter" },
    { key: "fake-ip-filter", label: "netDnsFakeIpFilter", kind: "list" },
  ],
  tun: [
    { key: "enable", label: "netTunEnable", kind: "bool" },
    { key: "bypass-cn", label: "netTunBypassCn", kind: "bool" },
    {
      key: "stack",
      label: "netTunStack",
      kind: "select",
      options: ["gvisor", "system", "mixed"],
    },
    { key: "device", label: "netTunDevice", kind: "string" },
    { key: "auto-route", label: "netTunAutoRoute", kind: "bool" },
    { key: "auto-redirect", label: "netTunAutoRedirect", kind: "bool" },
    { key: "auto-detect-interface", label: "netTunAutoDetectInterface", kind: "bool" },
    { key: "strict-route", label: "netTunStrictRoute", kind: "bool" },
    { key: "mtu", label: "netTunMtu", kind: "mtu" },
    { key: "route-exclude-address", label: "netTunRouteExclude", kind: "list" },
    { key: "dns-hijack", label: "netTunDnsHijack", kind: "list" },
  ],
};

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function validPolicy(value: unknown): boolean {
  return object(value) && Object.entries(value).every(([key, server]) =>
    key.trim().length > 0 && (typeof server === "string" && server.trim().length > 0 ||
      Array.isArray(server) && server.length > 0 && server.every((item) => typeof item === "string" && item.trim().length > 0))
  );
}

function validFilter(value: unknown): boolean {
  return object(value) && Object.entries(value).every(([key, entry]) =>
    key === "geoip" ? typeof entry === "boolean" :
    key === "geoip-code" ? typeof entry === "string" :
    key === "ipcidr" || key === "domain" ? Array.isArray(entry) && entry.every((item) => typeof item === "string") : false
  );
}

export function validateNetwork(section: "dns" | "tun", value: unknown, language: Language) {
  if (value == null) return;
  if (typeof value !== "object" || Array.isArray(value))
    throw new Error(t(language, "netSectionInvalid", { section }));
  for (const [key, v] of Object.entries(value)) {
    const f = schemas[section].find((f) => f.key === key);
    if (!f)
      throw new Error(t(language, "setUnsupported", { key: `${section}.${key}` }));
    if (v == null) continue;
    const valid =
      f.kind === "bool"
        ? typeof v === "boolean"
        : f.kind === "policy"
          ? validPolicy(v)
          : f.kind === "filter"
            ? validFilter(v)
        : f.kind === "list"
          ? Array.isArray(v) && v.every((item) => typeof item === "string")
          : f.kind === "mtu"
            ? typeof v === "number" &&
              Number.isInteger(v) &&
              v >= 1 &&
              v <= 65535
            : typeof v === "string" &&
              (f.kind !== "select" || f.options!.includes(v)) &&
              (section !== "tun" || key !== "device" || v.length > 0);
    if (!valid) throw new Error(t(language, "setServiceInvalidValue", { label: t(language, f.label) }));
  }
}
export function networkDraft(runtime: Runtime): Draft {
  const draft: Draft = {};
  for (const section of ["dns", "tun"] as const) {
    const value = runtime[section] as Record<string, Value> | undefined;
    draft[section] = value == null ? "" : "true";
    for (const field of schemas[section]) {
      const key = `${section}.${field.key}`,
        v = value?.[field.key];
      draft[key] =
        v == null ? "" : ["list", "policy", "filter"].includes(field.kind) ? JSON.stringify(v) : String(v);
      draft[`${key}.present`] = v == null ? "" : "true";
    }
  }
  return draft;
}
export function networkRuntime(draft: Draft, language: Language): Runtime {
  const result: Runtime = {};
  for (const section of ["dns", "tun"] as const) {
    if (draft[section] !== "true") continue;
    const value: Record<string, Value> = {};
    for (const f of schemas[section]) {
      const key = `${section}.${f.key}`,
        v = draft[key] ?? "";
      if (f.kind === "string") {
        if (draft[`${key}.present`] === "true") value[f.key] = v;
      } else if (v !== "") {
        if (f.kind === "list" || f.kind === "policy" || f.kind === "filter") {
          try {
            const parsed: unknown = JSON.parse(v);
            if (f.kind === "list") {
              if (!Array.isArray(parsed) || !parsed.every((item) => typeof item === "string")) throw new Error();
            } else if (f.kind === "policy" ? !validPolicy(parsed) : !validFilter(parsed)) throw new Error();
            value[f.key] = parsed as Value;
          } catch {
            throw new Error(
              t(language, f.kind === "list" ? "netListError" : "netObjectError", { label: t(language, f.label) }),
            );
          }
        } else if (f.kind === "mtu") {
          if (!/^\d+$/.test(v) || Number(v) < 1 || Number(v) > 65535)
            throw new Error(t(language, "netMtuError"));
          value[f.key] = Number(v);
        } else {
          const allowed = f.kind === "bool" ? ["true", "false"] : f.options!;
          if (!allowed.includes(v)) throw new Error(t(language, "setInvalidValue", { label: t(language, f.label) }));
          value[f.key] = f.kind === "bool" ? v === "true" : v;
        }
      }
    }
    validateNetwork(section, value, language);
    result[section] = value;
  }
  return result;
}

export function NetworkFields({
  draft,
  disabled,
  change,
  language,
}: {
  draft: Draft;
  disabled: boolean;
  change: (key: string, value: string) => void;
  language: Language;
}) {
  return (
    <>
      {(["dns", "tun"] as const).map((section) => (
        <SettingsSection key={section} title={t(language, "netSectionTitle", { section: section.toUpperCase() })}><fieldset className="network-fields" disabled={disabled}>
          <legend>{t(language, "netSectionTitle", { section: section.toUpperCase() })} <HelpTip>
            {t(language, section === "dns" ? "netDnsHelp" : "netTunHelp")}
          </HelpTip></legend>
          <label>
            {t(language, "netSource", { section: section.toUpperCase() })}
            <select
              aria-label={t(language, "netSource", { section: section.toUpperCase() })}
              value={draft[section] ?? ""}
              onChange={(e) => change(section, e.target.value)}
            >
              <option value="">{t(language, "netInheritSection")}</option>
              <option value="true">{t(language, "netUseLocal")}</option>
            </select>
          </label>
          {draft[section] === "true" && (
            <div className="settings-fields">
              {schemas[section].map((f) => {
                const key = `${section}.${f.key}`;
                return (
                  <div key={key}>
                    {f.kind === "string" && (
                      <label>
                        {t(language, "netFieldSource", { label: t(language, f.label) })}
                        <select
                          aria-label={t(language, "netFieldSource", { label: t(language, f.label) })}
                          value={draft[`${key}.present`] ?? ""}
                          onChange={(e) =>
                            change(`${key}.present`, e.target.value)
                          }
                        >
                          <option value="">{t(language, "setInherit")}</option>
                          <option value="true">{t(language, "netSpecified")}</option>
                        </select>
                      </label>
                    )}
                    <label>
                      {t(language, f.label)}
                      {f.kind === "list" || f.kind === "policy" || f.kind === "filter" ? (
                        <textarea
                          aria-label={t(language, f.label)}
                          rows={3}
                          value={draft[key] ?? ""}
                          placeholder={t(language, f.kind === "list" ? "netListPlaceholder" : "netObjectPlaceholder")}
                          onChange={(e) => change(key, e.target.value)}
                        />
                      ) : f.kind === "string" || f.kind === "mtu" ? (
                        <input
                          aria-label={t(language, f.label)}
                          inputMode={f.kind === "mtu" ? "numeric" : undefined}
                          disabled={
                            disabled ||
                            (f.kind === "string" &&
                              draft[`${key}.present`] !== "true")
                          }
                          value={draft[key] ?? ""}
                          placeholder={t(language, "setEmptyInherits")}
                          onChange={(e) => change(key, e.target.value)}
                        />
                      ) : (
                        <select
                          aria-label={t(language, f.label)}
                          value={draft[key] ?? ""}
                          onChange={(e) => change(key, e.target.value)}
                        >
                          <option value="">{t(language, "setInherit")}</option>
                          {(f.kind === "bool"
                            ? ["true", "false"]
                            : f.options!
                          ).map((v) => (
                            <option key={v} value={v}>
                              {v === "true"
                                ? t(language, "setEnable")
                                : v === "false"
                                  ? t(language, "setDisable")
                                  : v}
                            </option>
                          ))}
                        </select>
                      )}
                    </label>
                  </div>
                );
              })}
            </div>
          )}
        </fieldset></SettingsSection>
      ))}
    </>
  );
}
