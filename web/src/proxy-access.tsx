import { useEffect, useRef, useState } from "react";
import { ApiError, command, type Connection } from "./api";
import { t, type Language, type MessageKey } from "./i18n";
import type { CoreStatus } from "./types";

type ConnectionValues = {
  allow_lan: boolean;
  bind_address: string;
  mode: string;
  ipv6: boolean;
  tun_enabled?: boolean;
};
export type Access = {
  running: boolean;
  has_config: boolean;
  core_error: string | null;
  ports: { key: string; configured: number; actual: number | null; setting: number | null }[];
  configured: ConnectionValues;
  reported: ConnectionValues | null;
  dns_enabled: boolean;
  tun_enabled: boolean;
  authentication_required: boolean | null;
  /** The host's one system-wide TUN and who holds it (multi-user installations). */
  tun_holder?: TunHolder | null;
};
export type TunHolder = { uid: number; name: string; self: boolean };
// Protocol names stay untranslated.
const labels: Record<string, MessageKey | { name: string }> = {
  "mixed-port": "paMixed",
  port: { name: "HTTP" },
  "socks-port": { name: "SOCKS" },
  "redir-port": "paRedir",
  "tproxy-port": "paTproxy",
};
const modeKeys: Record<string, MessageKey> = { rule: "setModeRule", global: "setModeGlobal", direct: "setModeDirect" };

export function useProxyAccess({ token, status, connection, logout, language }: {
  token: string;
  status: CoreStatus;
  connection: Connection;
  logout: (reason?: string) => void;
  language: Language;
}) {
  // Read at sign-out only, so a language switch does not restart polling.
  const languageRef = useRef(language);
  languageRef.current = language;
  const [value, setValue] = useState<Access>();
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    setValue(undefined);
    setError("");
    if (connection !== "connected") return;
    let active = true;
    let pending: AbortController | undefined;
    const read = async () => {
      if (pending) return;
      const controller = new AbortController();
      pending = controller;
      try {
        const next = await command<Access>(token, "proxy_access", {}, controller.signal);
        if (active) { setValue(next); setError(""); }
      } catch (error) {
        if (!active) return;
        setValue(undefined);
        if (error instanceof ApiError && error.status === 401) logout(t(languageRef.current, "expiredToken"));
        else setError(error instanceof Error ? error.message : String(error));
      } finally { pending = undefined; }
    };
    void read();
    const timer = setInterval(() => void read(), 5000);
    return () => { active = false; clearInterval(timer); pending?.abort(); };
  }, [token, status.phase, status.generation, status.config_revision, connection, refresh, logout]);

  return { value, error, refresh: () => setRefresh(value => value + 1) };
}

export type ProxyAccessState = ReturnType<typeof useProxyAccess>;

export function ProxyAccessPanel(props: {
  token: string;
  status: CoreStatus;
  connection: Connection;
  logout: (reason?: string) => void;
  access: ProxyAccessState;
  language: Language;
}) {
  const { value, error, refresh } = props.access;
  const { connection, language } = props;
  const label = (key: string) => { const entry = labels[key]; return typeof entry === "string" ? t(language, entry) : entry?.name ?? key; };
  const onOff = (enabled: boolean | undefined) => t(language, enabled ? "setEnable" : "setDisable");

  const live = value?.reported;
  const current = live || value?.configured;
  const mismatch = value?.ports.some(port => port.actual !== null && port.actual !== port.configured);
  const available = (key: string) => value?.ports.find(port => port.key === key)?.actual || 0;
  const http = available("mixed-port") || available("port");
  const socks = available("mixed-port") || available("socks-port");
  const binding = current?.bind_address || "*";
  const localHost = ["*", "0.0.0.0", "::", "[::]", "localhost"].includes(binding) ? "127.0.0.1" : binding;
  const address = (port: number) => `${localHost.includes(":") && !localHost.startsWith("[") ? `[${localHost}]` : localHost}:${port}`;
  return (
    <section className="panel proxy-access" aria-label={t(language, "paTitle")}>
      <div className="panel-title">
        <h2>{t(language, "paTitle")}</h2>
        <button type="button" onClick={refresh} disabled={connection !== "connected"}>{t(language, "paRefresh")}</button>
      </div>
      {connection !== "connected" ? <p className="info">{t(language, "paDisconnected")}</p> : error ? <p className="alert" role="alert">{t(language, "paReadFailed", { error })}</p> : !value ? <p className="muted">{t(language, "paReading")}</p> : <>
        {!value.has_config && <p className="info">{t(language, "paNoConfig")}</p>}
        {!value.running && <p className="info">{t(language, "paCoreStopped")}</p>}
        {value.core_error && <p className="alert" role="alert">{t(language, "paCoreError", { error: value.core_error })}</p>}
        {mismatch && <p className="alert" role="alert">{t(language, "paMismatch")}</p>}
        <div className="proxy-ports-scroll">
          <table className="proxy-ports">
            <thead><tr><th>{t(language, "paType")}</th><th>{t(language, "paConfigured")}</th><th>{t(language, "paActual")}</th><th>{t(language, "paSource")}</th></tr></thead>
            <tbody>{value.ports.map(port => <tr key={port.key}>
              <th scope="row">{label(port.key)}</th>
              <td>{value.has_config ? port.configured || t(language, "paDisabled") : "—"}</td>
              <td className={port.actual !== null && port.actual !== port.configured ? "port-mismatch" : ""}>{port.actual === null ? t(language, "paUnconfirmed") : port.actual || t(language, "paNotListening")}</td>
              <td>{port.setting === null ? t(language, "paInherited") : t(language, "paServiceSetting", { port: port.setting || t(language, "paDisabled") })}</td>
            </tr>)}</tbody>
          </table>
        </div>
        <dl className="proxy-details">
          <div><dt>{t(language, live ? "paListenReported" : "paListenConfigured")}</dt><dd className="mono">{binding}</dd></div>
          <div><dt>{t(language, "paLan")}</dt><dd>{t(language, current?.allow_lan ? "paLanAllowed" : "paLanLocal")}</dd></div>
          <div><dt>{t(language, "setMode")}</dt><dd>{modeKeys[current?.mode.toLowerCase() || ""] ? t(language, modeKeys[current!.mode.toLowerCase()]) : current?.mode}</dd></div>
          <div><dt>IPv6</dt><dd>{onOff(current?.ipv6)}</dd></div>
          <div><dt>{t(language, "paDnsTun")}</dt><dd>DNS {onOff(value.dns_enabled)} · TUN {onOff(value.tun_enabled)}</dd></div>
        </dl>
        {live && <div className="proxy-browser-settings" aria-label={t(language, "paBrowserAria")}>
          <strong>{t(language, "paBrowserTitle")}</strong>
          <p>{t(language, "paHttpProxy")}<code>{http ? address(http) : t(language, "paNoPort")}</code></p>
          <p>{t(language, "paSocksProxy")}<code>{socks ? address(socks) : t(language, "paNoPort")}</code></p>
          {socks > 0 && <p className="hint">{t(language, "paSocksDnsHint")}</p>}
          {value.authentication_required && <p className="info">{t(language, "paAuthRequired")}</p>}
        </div>}
        <p className="hint">{t(language, "paOtherDevices")}</p>
        <p className="hint">{t(language, "paReportedHint")}</p>
      </>}
    </section>
  );
}
