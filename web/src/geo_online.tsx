import { useEffect, useRef, useState } from "react";
import { ApiError, command } from "./api";
import { t, type Language } from "./i18n";
import type { CoreStatus } from "./types";

type Info = { name: string; current_sha256: string | null; source_sha256: string };
type Receipt = { changed: boolean; durable: boolean; cleanup_pending: boolean; core_load_verified?: boolean; validation: { verified: boolean; sha256: string; format: string } };
type Route = "direct" | "system" | "managed";

export function GeoOnlineAction({ name, token, status, connection, logout, installed, language = "zh" }: {
  name: string; token: string; status: CoreStatus; connection: string;
  logout: (reason?: string) => void; installed: (message: string) => void;
  language?: Language;
}) {
  const [info, setInfo] = useState<Info>();
  const [pin, setPin] = useState("");
  const [route, setRoute] = useState<Route>("direct");
  const [invalidCerts, setInvalidCerts] = useState(false);
  const [accept, setAccept] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const controller = useRef<AbortController | null>(null);
  const dat = name.endsWith(".dat");
  useEffect(() => {
    epoch.current++; controller.current?.abort();
    setInfo(undefined); setPin(""); setRoute("direct"); setInvalidCerts(false); setAccept(false); setError(""); setBusy(false);
    return () => { epoch.current++; controller.current?.abort(); };
  }, [name, token, status.config_revision, connection]);

  async function run(update: boolean) {
    if (busy || (update && !info)) return;
    if (update && pin && !/^[a-fA-F0-9]{64}$/.test(pin)) {
      setError(t(language, "geoOnlineHexError")); return;
    }
    const version = epoch.current;
    const abort = new AbortController(); controller.current = abort;
    setBusy(true); setError("");
    if (!update) setInfo(undefined);
    try {
      if (update && info) {
        const receipt = await command<Receipt>(token, "update_geo_online", {
          name, expected_current_sha256: info.current_sha256,
          expected_source_sha256: info.source_sha256,
          expected_download_sha256: pin || null,
          accept_metadata_only: dat ? false : accept,
          route,
          danger_accept_invalid_certs: invalidCerts,
        }, abort.signal);
        if (version !== epoch.current) return;
        if (dat && receipt.core_load_verified !== true) throw new Error(t(language, "geoSeedDatProbeFailed"));
        setInfo(undefined);
        const changeStatus = receipt.changed ? t(language, "geoOnlineChanged") : t(language, "geoOnlineUnchanged");
        const restartedStatus = status.phase === "running" && receipt.changed ? t(language, "geoOnlineRestarted") : "";
        const structureStatus = dat ? t(language, "geoOnlineDatStatus") : receipt.validation.verified ? t(language, "geoSeedMmdbVerified") : t(language, "geoSeedMmdbUnverified");
        const syncStatus = !receipt.durable || receipt.cleanup_pending ? t(language, "geoSeedSyncPending") : "";
        installed(t(language, "geoOnlineInstalled", { name, changeStatus, restartedStatus, structureStatus, sha256: receipt.validation.sha256, syncStatus }));
      } else {
        const next = await command<Info>(token, "geo_online_info", { name }, abort.signal);
        if (version === epoch.current) setInfo(next);
      }
    } catch (cause) {
      if (version !== epoch.current) return;
      if (cause instanceof ApiError && cause.status === 401) logout(t(language, "expiredToken"));
      else setError(cause instanceof Error ? cause.message : String(cause));
      if (update) setInfo(undefined);
    } finally {
      if (version === epoch.current) setBusy(false);
    }
  }
  return <div>
    <button type="button" disabled={busy || connection !== "已连接"} onClick={() => void run(false)}>{t(language, "geoOnlineRead", { name })}</button>
    {info && <>
      <p>{t(language, "geoOnlineCommittedFingerprint")}<code>{info.source_sha256}</code></p>
      <p>{t(language, "geoOnlineCurrentFile")}<code>{info.current_sha256 || t(language, "geoSeedFileMissing")}</code></p>
      <label>{t(language, "geoOnlineOptionalSha256")} <input value={pin} disabled={busy} onChange={event => setPin(event.target.value.trim())} /></label>
      <label>{t(language, "geoOnlineDownloadRoute")} <select value={route} disabled={busy} onChange={event => setRoute(event.target.value as Route)}>
        <option value="direct">{t(language, "geoOnlineRouteDirect")}</option><option value="system">{t(language, "geoOnlineRouteSystem")}</option><option value="managed" disabled={status.phase !== "running"}>{t(language, "geoOnlineRouteManaged")}</option>
      </select></label>
      <label><input type="checkbox" checked={invalidCerts} disabled={busy} onChange={event => setInvalidCerts(event.target.checked)} />{t(language, "geoOnlineIgnoreCerts")}</label>
      <p className="info">{t(language, "geoOnlineCertNotice")}</p>
      {!dat && <label><input type="checkbox" checked={accept} disabled={busy} onChange={event => setAccept(event.target.checked)} />{t(language, "geoSeedAllowEmptyMmdb")}</label>}
      {status.phase === "running" && <p className="info">{t(language, "geoOnlineRunningNotice")}</p>}
      {!(["running", "stopped"] as string[]).includes(status.phase) && <p className="info">{t(language, "geoOnlinePhaseNotice")}</p>}
      <button type="button" disabled={busy || connection !== "已连接" || !(["running", "stopped"] as string[]).includes(status.phase) || (route === "managed" && status.phase !== "running")} onClick={() => void run(true)}>{t(language, "geoOnlineUpdate", { name })}</button>
    </>}
    {busy && <p role="status">{t(language, "geoOnlineWorking")}</p>}
    {error && <p role="alert" className="alert">{t(language, "geoOnlineFailed", { message: error })}</p>}
  </div>;
}
