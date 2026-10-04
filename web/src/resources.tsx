import { HelpTip } from "./help-tip";
import { useToast } from "./toast";
import { useEffect, useRef, useState } from "react";
import { ApiError, command, type Connection } from "./api";
import { GeoSeedAction } from "./geo_seed";
import { GeoOnlineAction } from "./geo_online";
import { t, type Language, type MessageKey } from "./i18n";
import type { CoreStatus } from "./types";

type FreshnessState = "fresh" | "stale" | "indeterminate";
type AutoUpdateState = "active" | "disabled" | "stopped" | "indeterminate";

type Resource = {
  section: string;
  name: string;
  provider_type: string | null;
  path: string | null;
  state: string;
  bytes: number | null;
  modified_unix_seconds?: number | null;
  age_seconds?: number | null;
  freshness?: FreshnessState | null;
  conflict: boolean;
};
type GeoUpdatePolicy = {
  core_running: boolean;
  readback_error: boolean;
  configured_enabled: boolean | null;
  configured_interval_hours: number | null;
  effective_enabled: boolean | null;
  effective_interval_hours: number | null;
  auto_update_state?: AutoUpdateState;
  mismatch: boolean;
};
type Inventory = {
  data_dir: string;
  bundle_dir: string | null;
  config_revision: string | null;
  geo_update?: GeoUpdatePolicy;
  geo: Resource[];
  providers: Resource[];
};
const labelKeys: Record<string, MessageKey> = {
  available: "resourceStateAvailable",
  missing: "resourceStateMissing",
  empty: "resourceStateEmpty",
  unsafe_path: "resourceStateUnsafePath",
  not_file: "resourceStateNotFile",
  unreadable: "resourceStateUnreadable",
  inline: "resourceStateInline",
  core_managed: "resourceStateCoreManaged",
  invalid_declaration: "resourceStateInvalidDeclaration",
};
const autoUpdateStateKeys: Record<AutoUpdateState, MessageKey> = {
  active: "resourceAutoUpdateActive",
  disabled: "resourceAutoUpdateDisabled",
  stopped: "resourceAutoUpdateStopped",
  indeterminate: "resourceAutoUpdateIndeterminate",
};
const freshnessKeys: Record<FreshnessState, { key: MessageKey; className?: string }> = {
  fresh: { key: "resourceFreshnessFresh", className: "info" },
  stale: { key: "resourceFreshnessStale", className: "alert" },
  indeterminate: { key: "resourceFreshnessIndeterminate" },
};
const enabledLabel = (language: Language, value: boolean | null) =>
  value === null ? t(language, "resourceUnspecified") : value ? t(language, "resourceEnabled") : t(language, "resourceDisabled");
const intervalLabel = (language: Language, value: number | null) =>
  value === null ? t(language, "resourceUnspecified") : t(language, "resourceHours", { count: value });

function modifiedLabel(language: Language, seconds?: number | null) {
  if (seconds == null || !Number.isSafeInteger(seconds) || seconds < 0) return null;
  const milliseconds = seconds * 1000;
  const date = new Date(milliseconds);
  if (Number.isNaN(date.getTime())) return null;
  const elapsedMinutes = Math.floor((Date.now() - milliseconds) / 60000);
  const age = elapsedMinutes < 0
    ? t(language, "resourceModifiedFuture")
    : elapsedMinutes >= 1440
      ? t(language, "resourceModifiedDaysAgo", { count: Math.floor(elapsedMinutes / 1440) })
      : elapsedMinutes >= 60
        ? t(language, "resourceModifiedHoursAgo", { count: Math.floor(elapsedMinutes / 60) })
        : t(language, "resourceModifiedMinutesAgo", { count: elapsedMinutes });
  return `${t(language, "resourceModifiedPrefix")}${date.toLocaleString(language === "en" ? "en-US" : "zh-CN", { hour12: false })}（${age}）`;
}

export function ResourcesPanel({ token, status, connection, logout, language = "zh" }: {
  token: string;
  status: CoreStatus;
  connection: Connection;
  logout: (reason?: string) => void;
  language?: Language;
}) {
  const validationController = useRef<AbortController | null>(null);
  const epoch = useRef(0);
  const [checks, setChecks] = useState<Record<string, { message: string; error?: boolean }>>({});
  const [checking, setChecking] = useState<string>();
  const setNotice = useToast();
  const [value, setValue] = useState<Inventory>();
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [operating, setOperating] = useState<string>();

  async function updateProvider(section: string, name: string) {
    setOperating(name);
    try {
      if (section === "proxy-providers") {
        await command(token, "update_proxy_provider", { name });
        setNotice(t(language, "ruleProviderUpdated", { name } as any) ? `${name} 更新完成` : `${name} updated`);
      } else {
        await command(token, "update_rule_provider", { name });
        setNotice(t(language, "ruleProviderUpdated", { name } as any) || `${name} 更新完成`);
      }
      setRefresh(v => v + 1);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) logout(t(language, "expiredToken"));
      else setError(err instanceof Error ? err.message : String(err));
    } finally {
      setOperating(undefined);
    }
  }

  async function healthcheckProvider(name: string) {
    setOperating(name);
    try {
      await command(token, "healthcheck_proxy_provider", { name });
      setNotice(`${name}: ${t(language, "proxyProviderHealthcheck") || "健康检查"} 完成`);
      setRefresh(v => v + 1);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) logout(t(language, "expiredToken"));
      else setError(err instanceof Error ? err.message : String(err));
    } finally {
      setOperating(undefined);
    }
  }
  useEffect(() => {
    epoch.current++;
    validationController.current?.abort();
    setChecks({});
    setChecking(undefined);
    setError("");
    if (connection !== "connected") { setValue(undefined); return; }
    let active = true;
    const controller = new AbortController();
    void command<Inventory>(token, "resources", {}, controller.signal).then(next => {
      if (active) setValue(next);
    }).catch(error => {
      if (!active) return;
      if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
      else setError(error instanceof Error ? error.message : String(error));
    });
    return () => { active = false; controller.abort(); epoch.current++; validationController.current?.abort(); };
  }, [token, status.phase, status.generation, status.config_revision, connection, refresh, logout, language]);

  async function validate(name: string) {
    const currentEpoch = epoch.current;
    const controller = new AbortController();
    validationController.current = controller;
    setChecking(name);
    setChecks(previous => ({ ...previous, [name]: { message: t(language, "resourceValidating", { type: name.endsWith(".dat") ? "DAT" : "MMDB" }) } }));
    try {
      const report = await command<{ verified: boolean; warning: string | null; sha256: string; bytes: number; ip_version: number; node_count: number; dat?: { group_count: number; record_count: number; ipv4_count: number; ipv6_count: number; regex_count: number; attribute_count: number; empty_group_count: number; unknown_field_count: number; has_cn_group: boolean; core_matching_verified: boolean } }>(token, "validate_geo", { name }, controller.signal);
      if (currentEpoch !== epoch.current) return;
      let message: string;
      if (name.endsWith(".dat")) {
        const dat = report.dat;
        if (!dat) throw new Error(t(language, "resourceDatInvalidDiagnostic"));
        const status = report.verified ? t(language, "resourceDatVerified") : t(language, "resourceDatUnverified");
        const cnGroup = dat.has_cn_group ? t(language, "resourceDatHasCn") : t(language, "resourceDatMissingCn");
        message = t(language, "resourceDatSummary", {
          status,
          groups: dat.group_count,
          records: dat.record_count,
          ipv4: dat.ipv4_count,
          ipv6: dat.ipv6_count,
          regex: dat.regex_count,
          attributes: dat.attribute_count,
          emptyGroups: dat.empty_group_count,
          unknownFields: dat.unknown_field_count,
          cnGroup,
        });
      } else {
        const status = report.verified ? t(language, "resourceMmdbVerified") : t(language, "resourceMmdbUnverified");
        message = t(language, "resourceMmdbSummary", {
          status,
          ipVersion: report.ip_version,
          nodes: report.node_count,
        });
      }
      setChecks(previous => ({ ...previous, [name]: { message: t(language, "resourceValidationReport", { message, bytes: report.bytes, sha256: report.sha256 }) } }));
      setNotice(`${name}: ${message}`, report.verified ? "success" : "info");
    } catch (error) {
      if (currentEpoch !== epoch.current) return;
      if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
      else setChecks(previous => ({ ...previous, [name]: { message: t(language, "resourceValidationFailed", { message: error instanceof Error ? error.message : String(error) }), error: true } }));
    } finally {
      if (currentEpoch === epoch.current) setChecking(undefined);
    }
  }

  function rows(items: Resource[]) {
    return <ul className="resource-list">{items.map(item => <li key={`${item.section}:${item.name}`}>
      <strong>{item.name}</strong> · {item.section === "proxy-providers" ? t(language, "resourceSectionProxy") : item.section === "rule-providers" ? t(language, "resourceSectionRule") : t(language, "resourceSectionGeo")}
      <p>{(labelKeys[item.state] ? t(language, labelKeys[item.state]) : t(language, "resourceStateUnknown"))}{item.bytes !== null ? t(language, "resourceBytes", { bytes: item.bytes }) : ""}{item.provider_type ? ` · ${item.provider_type}` : ""}</p>
      {modifiedLabel(language, item.modified_unix_seconds) && <p>{modifiedLabel(language, item.modified_unix_seconds)}</p>}
      {item.freshness && item.state === "available" && item.freshness !== "indeterminate" && (
        <p className={freshnessKeys[item.freshness]?.className || "hint"}>
          {t(language, "resourceFreshnessLabel")}{freshnessKeys[item.freshness]?.key ? t(language, freshnessKeys[item.freshness].key) : item.freshness}
        </p>
      )}
      {item.path && <code>{item.path}</code>}
      {item.section === "geo" && ["Country.mmdb", "ASN.mmdb", "geoip.metadb", "geoip.dat", "geosite.dat"].includes(item.name) && item.state === "available" && <button type="button" disabled={!!checking} onClick={() => void validate(item.name)}>{t(language, "resourceValidate", { name: item.name })}</button>}
      {item.section === "geo" && checks[item.name] && <p role={checks[item.name].error ? "alert" : "status"} className={checks[item.name].error ? "alert" : "resource-check"}>{checks[item.name].message}</p>}
      {item.section === "geo" && value?.bundle_dir && ["Country.mmdb", "ASN.mmdb", "geoip.metadb", "geoip.dat", "geosite.dat"].includes(item.name) && <GeoSeedAction name={item.name} token={token} status={status} connection={connection} logout={logout} language={language} installed={message => { setNotice(message); setRefresh(previous => previous + 1); }} />}
      {item.section === "geo" && ["Country.mmdb", "ASN.mmdb", "geoip.metadb", "geoip.dat", "geosite.dat"].includes(item.name) && <GeoOnlineAction name={item.name} token={token} status={status} connection={connection} logout={logout} language={language} installed={message => { setNotice(message); setRefresh(previous => previous + 1); }} />}
      {item.section === "proxy-providers" && status.phase === "running" && (
        <div className="actions" style={{ marginTop: "0.5rem" }}>
          <button
            type="button"
            disabled={!!operating || !!checking}
            onClick={() => void updateProvider(item.section, item.name)}
          >
            {operating === item.name ? t(language, "proxyProviderUpdating") : t(language, "proxyProviderUpdate")}
          </button>
          <button
            type="button"
            disabled={!!operating || !!checking}
            onClick={() => void healthcheckProvider(item.name)}
          >
            {operating === item.name ? t(language, "proxyProviderChecking") : t(language, "proxyProviderHealthcheck")}
          </button>
        </div>
      )}
      {item.section === "rule-providers" && status.phase === "running" && (
        <div className="actions" style={{ marginTop: "0.5rem" }}>
          <button
            type="button"
            disabled={!!operating || !!checking}
            onClick={() => void updateProvider(item.section, item.name)}
          >
            {operating === item.name ? t(language, "ruleProviderUpdating") : t(language, "ruleProviderUpdate")}
          </button>
        </div>
      )}
      {item.conflict && <p className="alert">{t(language, "resourceConflict")}</p>}
    </li>)}</ul>;
  }

  return <section className="panel" aria-label={t(language, "resourceRegion")}>
    <div className="panel-title"><h2 className="setting-heading">{t(language, "resourceTitle")}<HelpTip label={t(language, "resourceTitle")}>{t(language, "resourceHint")}</HelpTip></h2><button type="button" disabled={connection !== "connected"} onClick={() => setRefresh(value => value + 1)}>{t(language, "resourceRefresh")}</button></div>
    {connection !== "connected" ? <p className="info">{t(language, "resourceDisconnected")}</p> : error ? <p className="alert" role="alert">{t(language, "resourceReadFailed", { message: error })}</p> : !value ? <p className="muted">{t(language, "resourceReading")}</p> : <>
      <p>{t(language, "resourceDataDir")}<code>{value.data_dir}</code></p>
      {value.bundle_dir && <p>{t(language, "resourceBundleDir")}<code>{value.bundle_dir}</code></p>}
      {!value.config_revision && <p className="info">{t(language, "resourceNoConfig")}</p>}
      <h3>{t(language, "resourceGeoPolicy")}</h3>
      {value.geo_update ? <>
        {value.geo_update.auto_update_state && <p>{t(language, "resourceAutoUpdateStatus")}<strong>{autoUpdateStateKeys[value.geo_update.auto_update_state] ? t(language, autoUpdateStateKeys[value.geo_update.auto_update_state]) : value.geo_update.auto_update_state}</strong></p>}
        <p>{t(language, "resourceCommittedConfig")}{enabledLabel(language, value.geo_update.configured_enabled)} · {t(language, "resourceUpdateInterval")}{intervalLabel(language, value.geo_update.configured_interval_hours)}</p>
        <p>{t(language, "resourceCoreEffective")}{!value.geo_update.core_running ? t(language, "resourceCoreNotRunning") : value.geo_update.readback_error ? t(language, "resourceReadFailedRetry") : value.geo_update.effective_enabled === null ? t(language, "resourceCoreNotReported") : enabledLabel(language, value.geo_update.effective_enabled)} · {t(language, "resourceUpdateInterval")}{value.geo_update.core_running && !value.geo_update.readback_error ? intervalLabel(language, value.geo_update.effective_interval_hours) : t(language, "resourceUnconfirmed")}</p>
        {value.geo_update.mismatch && <p className="alert">{t(language, "resourcePolicyMismatch")}</p>}
      </> : <p className="muted">{t(language, "resourcePolicyUnavailable")}</p>}
      <h3>{t(language, "resourceGeoFiles")}</h3>{rows(value.geo)}
      <h3>{t(language, "resourceProviderFiles")}</h3>{value.providers.length ? rows(value.providers) : <p className="muted">{t(language, "resourceNoProviders")}</p>}
    </>}
  </section>;
}
