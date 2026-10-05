import { HelpTip } from "./help-tip";
import { useToast } from "./toast";
import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { ApiError, command, type Perform, type Connection } from "./api";
import type { CoreStatus } from "./types";
import {
  NetworkFields,
  networkDraft,
  networkRuntime,
  validateNetwork,
  type Runtime,
  type Draft,
} from "./network-settings";
import { useProxyAccess } from "./proxy-access";
import { GEO_KEYS, GeoFields, geoDraft, geoRuntime, validateGeo } from "./geo-settings";
import { OUTBOUND_KEYS, OutboundFields, outboundDraft, outboundRuntime, validateOutbound } from "./outbound-settings";
import { DOWNLOAD_KEYS, DownloadFields, downloadDraft, downloadRuntime, validateDownload } from "./download-settings";
import { HostsFields, hostsDraft, hostsRuntime, validateHosts } from "./hosts-settings";
import { ResourcesPanel } from "./resources";
import { AutostartPanel } from "./autostart";
import {
  AUTHORITY_KEYS,
  AuthorityFields,
  authorityDraft,
  authorityRuntime,
  validateAuthority,
} from "./authority-settings";
import { t, type Language, type MessageKey } from "./i18n";
import { LanguagePicker } from "./language-picker";

type Settings = { schema_version: number; runtime: Runtime };
const fields = [
  { key: "mixed-port", label: "setMixedPort", kind: "port" },
  { key: "socks-port", label: "setSocksPort", kind: "port" },
  { key: "port", label: "setHttpPort", kind: "port" },
  { key: "redir-port", label: "setRedirPort", kind: "port" },
  { key: "tproxy-port", label: "setTproxyPort", kind: "port" },
  {
    key: "mode",
    label: "setMode",
    kind: "select",
    options: [
      ["rule", "setModeRule"],
      ["global", "setModeGlobal"],
      ["direct", "setModeDirect"],
    ],
  },
  { key: "allow-lan", label: "setAllowLan", kind: "bool" },
  { key: "ipv6", label: "setIpv6", kind: "bool" },
  { key: "unified-delay", label: "setUnifiedDelay", kind: "bool" },
  { key: "tcp-concurrent", label: "setTcpConcurrent", kind: "bool" },
  { key: "find-process-mode", label: "setFindProcessMode", kind: "select", options: [["strict", "setFindProcessStrict"], ["always", "setFindProcessAlways"], ["off", "setFindProcessOff"]] },
  { key: "keep-alive-interval", label: "setKeepAliveInterval", kind: "seconds" },
  { key: "keep-alive-idle", label: "setKeepAliveIdle", kind: "seconds" },
  { key: "disable-keep-alive", label: "setDisableKeepAlive", kind: "bool" },
  {
    key: "log-level",
    label: "setLogLevel",
    kind: "select",
    options: [
      ["silent", "setLogSilent"],
      ["error", "setLogError"],
      ["warning", "setLogWarning"],
      ["info", "setLogInfo"],
      ["debug", "setLogDebug"],
    ],
  },
] as const satisfies readonly { key: string; label: MessageKey; kind: string; options?: readonly (readonly [string, MessageKey])[] }[];

function options(
  field: (typeof fields)[number],
): readonly (readonly [string, MessageKey])[] {
  return field.kind === "bool"
    ? [
        ["true", "setEnable"],
        ["false", "setDisable"],
      ]
    : field.kind === "select"
      ? field.options
      : [];
}
function toDraft(settings: Settings): Draft {
  return Object.fromEntries([
    ...Object.entries(networkDraft(settings.runtime)),
    ...Object.entries(geoDraft(settings.runtime)),
    ...Object.entries(outboundDraft(settings.runtime)),
    ...Object.entries(downloadDraft(settings.runtime)),
    ...Object.entries(hostsDraft(settings.runtime)),
    ...Object.entries(authorityDraft(settings.runtime)),
    ...fields.map((field) => [
      field.key,
      settings.runtime[field.key] == null
        ? ""
        : String(settings.runtime[field.key]),
    ]),
  ]);
}
function runtime(draft: Draft, language: Language): Runtime {
  const result: Runtime = {
    ...networkRuntime(draft, language),
    ...geoRuntime(draft, language),
    ...outboundRuntime(draft, language),
    ...downloadRuntime(draft, language),
    ...hostsRuntime(draft, language),
    ...authorityRuntime(draft, language),
  };
  for (const field of fields) {
    const value = draft[field.key];
    if (value === "") continue;
    const label = t(language, field.label);
    if (field.kind === "port") {
      if (!/^\d+$/.test(value) || Number(value) > 65535)
        throw new Error(t(language, "setPortRange", { label }));
      result[field.key] = Number(value);
    } else if (field.kind === "seconds") {
      if (!/^-?\d+$/.test(value) || !Number.isInteger(Number(value)) || Number(value) < -2147483648 || Number(value) > 2147483647)
        throw new Error(t(language, "setSecondsRange", { label }));
      result[field.key] = Number(value);
    } else {
      if (!options(field).some(([option]) => value === option))
        throw new Error(t(language, "setInvalidValue", { label }));
      result[field.key] = field.kind === "bool" ? value === "true" : value;
    }
  }
  return result;
}
function decode(value: unknown, language: Language): Settings {
  const settings = value as Settings;
  if (
    !settings ||
    settings.schema_version !== 1 ||
    !settings.runtime ||
    typeof settings.runtime !== "object" ||
    Array.isArray(settings.runtime)
  )
    throw new Error(t(language, "setVersionUnsupported"));
  validateGeo(settings.runtime, language);
  for (const [key, value] of Object.entries(settings.runtime)) {
    if (key === "hosts") { if (value != null) validateHosts(value, language); continue; }
    if (GEO_KEYS.has(key)) continue;
    if (DOWNLOAD_KEYS.has(key)) { validateDownload(key, value, language); continue; }
    if (OUTBOUND_KEYS.has(key)) { validateOutbound(key, value, language); continue; }
    if (AUTHORITY_KEYS.has(key)) { validateAuthority(key, value, language); continue; }
    if (key === "dns" || key === "tun") {
      validateNetwork(key, value, language);
      continue;
    }
    const field = fields.find((field) => field.key === key);
    if (!field)
      throw new Error(t(language, "setUnsupported", { key }));
    if (value == null) continue;
    if (
      field.kind === "port"
        ? typeof value !== "number" ||
          !Number.isInteger(value) ||
          value < 0 ||
          value > 65535
        : field.kind === "seconds"
          ? typeof value !== "number" || !Number.isInteger(value) || value < -2147483648 || value > 2147483647
        : field.kind === "bool"
          ? typeof value !== "boolean"
          : typeof value !== "string" ||
            !options(field).some(([option]) => value === option)
    )
      throw new Error(t(language, "setServiceInvalidValue", { label: t(language, field.label) }));
  }
  // Normalize null inheritance and field order before comparing full replacements.
  return { schema_version: 1, runtime: runtime(toDraft(settings), language) };
}
function same(left: Runtime, right: Runtime): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
function matches(draft: Draft, saved: Settings, language: Language): boolean {
  try {
    return same(runtime(draft, language), saved.runtime);
  } catch {
    return false;
  }
}
const explain = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

export type SettingsPageHandle = {
  save: () => Promise<boolean>;
  discard: () => void;
};

type SettingsPageProps = {
  token: string;
  language?: Language;
  changeLanguage?: (value: string) => void;
  status: CoreStatus;
  connection: Connection;
  busy: boolean;
  perform: Perform;
  logout: (reason?: string) => void;
  desktopVersion?: string;
  onEditorStateChange: (dirty: boolean, busy: boolean) => void;
};

function VersionInfo({ token, status, desktopVersion, language }: {
  token: string;
  status: CoreStatus;
  desktopVersion?: string;
  language: Language;
}) {
  const [versions, setVersions] = useState<{ core?: string; service?: string }>({});
  useEffect(() => {
    const controller = new AbortController();
    Promise.allSettled([
      command<string | null>(token, "installed_core_version", {}, controller.signal),
      command<string>(token, "service_version", {}, controller.signal),
    ]).then(([core, service]) => {
      if (controller.signal.aborted) return;
      setVersions({
        core: core.status === "fulfilled" ? core.value ?? undefined : undefined,
        service: service.status === "fulfilled" ? service.value : undefined,
      });
    });
    return () => controller.abort();
  }, [token]);
  return (
    <section className="panel version-info" aria-label={t(language, "setVersionInfo")}>
      <h2>{t(language, "setVersionInfo")}</h2>
      <dl className="settings-summary">
        <div><dt>{t(language, "setCoreVersion")}</dt><dd>{status.version ?? versions.core ?? t(language, "setUnknown")}</dd></div>
        <div><dt>{t(language, "setServiceVersion")}</dt><dd>{versions.service ?? t(language, "setUnknown")}</dd></div>
        {desktopVersion && <div><dt>{t(language, "setDesktopVersion")}</dt><dd>{desktopVersion}</dd></div>}
      </dl>
    </section>
  );
}

export const SettingsPage = forwardRef<SettingsPageHandle, SettingsPageProps>(function SettingsPage({
  token,
  language = "zh",
  changeLanguage,
  status,
  connection,
  busy,
  perform,
  logout,
  desktopVersion,
  onEditorStateChange,
}, ref) {
  const access = useProxyAccess({ token, status, connection, logout, language });
  const [saved, setSaved] = useState<Settings>();
  const [draft, setDraft] = useState<Draft>({});
  const [working, setWorking] = useState(false),
    [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState("");
  const notify = useToast();
  const [confirmation, setConfirmation] = useState<"reload">();
  const alive = useRef(true),
    requests = useRef(new Set<AbortController>());
  const saving = useRef(false);
  const draftRef = useRef(draft), savedRef = useRef(saved), uncertainRef = useRef(uncertain);
  draftRef.current = draft; savedRef.current = saved; uncertainRef.current = uncertain;
  const disabled = busy || working;
  // Keep the next field editable while this form's transaction is in flight.
  const fieldsDisabled = disabled && !saving.current;
  function change(key: string, value: string) {
    draftRef.current = { ...draftRef.current, [key]: value };
    setDraft(draftRef.current);
    setError(""); setConfirmation(undefined);
  }
  function changeTun(value: string) {
    const next: Draft = { ...draftRef.current, "tun.enable": value };
    if (value !== "" && next.tun !== "true") next.tun = "true";
    draftRef.current = next;
    setDraft(next);
    setError(""); setConfirmation(undefined);
  }
  const dirty = saved ? !matches(draft, saved, language) : false;

  function portDescription(key: string) {
    if (connection !== "connected") return t(language, "setPortDisconnected");
    if (access.error) return t(language, "setPortReadFailed");
    const port = access.value?.ports.find(port => port.key === key);
    if (!port) return t(language, "setPortReading");
    const source = t(language, port.setting === null ? "setPortSourceInherited" : "setPortSourceService");
    if (!access.value?.has_config) return t(language, "setPortNoConfig");
    const configured = t(language, "setPortConfigured", { port: port.configured || t(language, "setPortDisabled") });
    if (port.actual === null) return `${configured} · ${source} · ${t(language, access.value.running ? "setPortCoreUnconfirmed" : "setPortCoreStopped")}`;
    if (port.actual !== port.configured) return `${t(language, "setPortCurrent", { port: port.actual || t(language, "setPortNotListening") })} · ${configured} · ${source} · ${t(language, "setPortMismatch")}`;
    return `${t(language, "setPortCurrent", { port: port.actual || t(language, "setPortDisabled") })} · ${source}`;
  }

  function portPlaceholder(key: string) {
    const port = access.value?.ports.find(port => port.key === key);
    if (!port || !access.value?.has_config || connection !== "connected") return t(language, "setEmptyInherits");
    if (port.actual !== null && port.actual === port.configured)
      return port.actual ? t(language, "setPortInheritCurrent", { port: port.actual }) : t(language, "setPortInheritCurrentOff");
    return port.configured ? t(language, "setPortInheritConfigured", { port: port.configured }) : t(language, "setPortInheritConfiguredOff");
  }

  async function read(): Promise<Settings> {
    const controller = new AbortController();
    requests.current.add(controller);
    try {
      return decode(
        await command<unknown>(token, "settings", {}, controller.signal),
        language,
      );
    } catch (error) {
      if (alive.current && error instanceof ApiError && error.status === 401)
        logout(t(language, "expiredToken"));
      throw error;
    } finally {
      requests.current.delete(controller);
    }
  }
  async function reload() {
    setWorking(true);
    setError("");
    setConfirmation(undefined);
    try {
      const next = await read();
      if (alive.current) {
        savedRef.current = next; setSaved(next);
        draftRef.current = toDraft(next); setDraft(draftRef.current);
        uncertainRef.current = false; setUncertain(false);
      }
    } catch (error) {
      if (alive.current) {
        if (saved) setUncertain(true);
        setError(t(language, "setReadFailed", { error: explain(error) }));
      }
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  useEffect(() => {
    alive.current = true;
    void reload();
    return () => {
      alive.current = false;
      requests.current.forEach((controller) => controller.abort());
    };
  }, [token]);

  async function save(submitted = { ...draftRef.current }): Promise<boolean> {
    if (!savedRef.current || busy || working || saving.current) return false;
    let requested: Runtime;
    try {
      requested = runtime(submitted, language);
    } catch (error) {
      setError(explain(error));
      return false;
    }
    if (!uncertainRef.current && same(requested, savedRef.current.runtime)) return true;
    saving.current = true;
    setWorking(true);
    setError(""); setConfirmation(undefined);
    const toast = notify.loading(t(language, "working"));
    try {
      // A failed readback is reconciled automatically before another write.
      if (uncertainRef.current) {
        const next = await read();
        if (!alive.current) return false;
        savedRef.current = next; setSaved(next);
        uncertainRef.current = false; setUncertain(false);
      }
      if (same(requested, savedRef.current.runtime)) {
        toast.finish(t(language, "saveSuccess"), "info");
        return true;
      }
      uncertainRef.current = true; setUncertain(true);
      const result = await perform<Settings>("set_settings", { runtime: requested }, { notify: false, toast, reconcile: true });
      if (!alive.current) return false;
      const next = await read();
      if (!alive.current) return false;
      savedRef.current = next; setSaved(next);
      uncertainRef.current = false; setUncertain(false);
      if (same(requested, next.runtime)) {
        // Normalize only submitted values that have not been edited again.
        const normalized = toDraft(next);
        const merged = { ...draftRef.current };
        for (const key of Object.keys(normalized)) {
          if (merged[key] === submitted[key]) merged[key] = normalized[key];
        }
        draftRef.current = merged; setDraft(merged);
        access.refresh();
        toast.finish(t(language, "saveSuccess"), result ? "success" : "info");
        return true;
      } else {
        const message = t(language, "setSaveMismatch");
        setError(message); toast.finish(message, "error");
        return false;
      }
    } catch (error) {
      if (alive.current) {
        const message = t(language, "setSaveUnverified", { error: explain(error) });
        setError(message); toast.finish(explain(error), "error");
      }
      return false;
    } finally {
      if (!alive.current) toast.dismiss();
      saving.current = false;
      if (alive.current) setWorking(false);
    }
  }

  function discard() {
    if (!savedRef.current) return;
    draftRef.current = toDraft(savedRef.current);
    setDraft(draftRef.current);
    uncertainRef.current = false;
    setUncertain(false);
    setError("");
    setConfirmation(undefined);
  }

  useImperativeHandle(ref, () => ({ save, discard }));
  useEffect(() => {
    onEditorStateChange(dirty || uncertain, disabled);
  }, [dirty, uncertain, disabled, onEditorStateChange]);
  useEffect(() => () => onEditorStateChange(false, true), [onEditorStateChange]);

  return (
    <div className="settings-layout">
      <section className="panel" aria-label={t(language, "setEditor")}>
        <section className="tun-control tun-setting" aria-label={t(language, "setTunMode")}>
          <div className="tun-setting-row">
            <label htmlFor="setting-tun-mode">{t(language, "setTunMode")}</label>
            <select
              id="setting-tun-mode"
              aria-label={t(language, "setTunMode")}
              disabled={!saved || fieldsDisabled}
              value={draft.tun === "true" ? draft["tun.enable"] ?? "" : ""}
              onChange={event => changeTun(event.target.value)}
            >
              <option value="">{t(language, "setInherit")}</option>
              <option value="true">{t(language, "setTunEnabled")}</option>
              <option value="false">{t(language, "setTunDisabled")}</option>
            </select>
          </div>
          <p className="hint">{t(language, "setTunHint")}</p>
        </section>
        <div className="panel-title">
          <h2 className="setting-heading">{t(language, "setRuntimeTitle")}<HelpTip label={t(language, "setRuntimeHelpLabel")}>{t(language, "setRuntimeHelp")}</HelpTip></h2>
        </div>
        {!status.config_revision && (
          <p className="info">
            {t(language, "setNoConfig")}
          </p>
        )}
        {error && (
          <p className="alert settings-floating-error" role="alert">
            {error}
          </p>
        )}
        {saved && (
          <form aria-label={t(language, "setForm")} onSubmit={event => event.preventDefault()}>
            <fieldset className="network-fields basic-settings"><legend>{t(language, "setCommon")} <HelpTip>{t(language, "setCommonHelp")}</HelpTip></legend><div className="settings-fields">
              {fields.map((field) => (
                <label key={field.key} htmlFor={`setting-${field.key}`}>
                  <span className="setting-name">{t(language, field.label)}<HelpTip id={`port-hint-${field.key}`} label={t(language, "setFieldHelpLabel", { label: t(language, field.label) })}>{field.kind === "port" ? portDescription(field.key) : t(language, field.kind === "seconds" ? "setSecondsHelp" : "setSelectHelp")}</HelpTip></span>
                  {field.kind === "port" || field.kind === "seconds" ? (
                    <>
                      <input
                        id={`setting-${field.key}`}
                        aria-label={t(language, field.label)}
                        inputMode={field.kind === "port" ? "numeric" : "text"}
                        placeholder={field.kind === "port" ? portPlaceholder(field.key) : t(language, "setEmptyInherits")}
                        aria-describedby={`port-hint-${field.key}`}
                        disabled={fieldsDisabled}
                        value={draft[field.key] ?? ""}
                        onChange={event => change(field.key, event.target.value)}
                      />
                    </>
                  ) : (
                    <select
                      id={`setting-${field.key}`}
                      aria-label={t(language, field.label)}
                      disabled={fieldsDisabled}
                      value={draft[field.key] ?? ""}
                      onChange={event => change(field.key, event.target.value)}
                    >
                      <option value="">{t(language, "setInherit")}</option>
                      {options(field).map(([value, label]) => (
                        <option key={value} value={value}>
                          {t(language, label)}
                        </option>
                      ))}
                    </select>
                  )}
                </label>
              ))}
            </div>
            </fieldset>
            <button type="button" className="port-refresh" onClick={access.refresh} disabled={connection !== "connected"}>
              {t(language, "setRefreshPorts")}
            </button>
            <GeoFields draft={draft} disabled={fieldsDisabled} change={change} language={language} />
            <DownloadFields draft={draft} disabled={fieldsDisabled} onChange={change} language={language} />
            <OutboundFields draft={draft} disabled={fieldsDisabled} onChange={change} language={language} />
            <HostsFields draft={draft} disabled={fieldsDisabled} change={change} language={language} />
            <AuthorityFields draft={draft} disabled={fieldsDisabled} onChange={change} language={language} />
            <NetworkFields draft={draft} disabled={fieldsDisabled} change={change} language={language} />
          </form>
        )}
        <div className="actions settings-reload">
          <button
            disabled={disabled}
            onClick={() =>
              dirty || uncertain ? setConfirmation("reload") : void reload()
            }
          >
            {t(language, saved ? "setReload" : "setRetryRead")}
          </button>
        </div>
        {confirmation && (
          <div className="reset-confirmation" role="group" aria-label={t(language, "setReplaceConfirm")}>
            <p>{t(language, "setReplaceWarning")}</p>
            <div className="actions">
              <button disabled={disabled} onClick={() => void reload()}>{t(language, "setConfirmReload")}</button>
              <button disabled={disabled} onClick={() => setConfirmation(undefined)}>{t(language, "setContinueEditing")}</button>
            </div>
          </div>
        )}
      </section>
      <div className="settings-side">
        {changeLanguage && (
          <section className="panel" aria-label={t(language, "language")}>
            <div className="panel-title">
              <h2 className="setting-heading">{t(language, "language")}<HelpTip>
              {t(language, "setLanguageHelp")}
            </HelpTip></h2>
            </div>
            <LanguagePicker language={language} changeLanguage={changeLanguage} />
          </section>
        )}
        <AutostartPanel token={token} language={language} connection={connection} logout={logout} />
        <details className="settings-details"><summary>{t(language, "setResources")}</summary>
        <ResourcesPanel token={token} status={status} connection={connection} logout={logout} language={language} />
        </details>
        <VersionInfo token={token} status={status} desktopVersion={desktopVersion} language={language} />
      </div>
    </div>
  );
});
