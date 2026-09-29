import { useEffect, useRef, useState } from "react";
import { ApiError, command, type Connection } from "./api";
import { t, type Language } from "./i18n";
import type { CoreStatus } from "./types";

type Seed = { name: string; current_sha256: string | null; seed_sha256: string; seed_bytes: number };
type Receipt = { changed: boolean; durable: boolean; cleanup_pending: boolean; core_load_verified?: boolean; validation: { verified: boolean; sha256: string } };

export function GeoSeedAction({ name, token, status, connection, logout, installed, language = "zh" }: {
  name: string; token: string; status: CoreStatus; connection: Connection;
  logout: (reason?: string) => void; installed: (message: string) => void;
  language?: Language;
}) {
  const dat = name.endsWith(".dat");
  const [seed, setSeed] = useState<Seed>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [accept, setAccept] = useState(false);
  const controller = useRef<AbortController | null>(null);
  const epoch = useRef(0);
  useEffect(() => {
    epoch.current++;
    controller.current?.abort();
    setSeed(undefined); setError(""); setBusy(false); setAccept(false);
    return () => { epoch.current++; controller.current?.abort(); };
  }, [name, token, status.phase, status.generation, status.config_revision, connection]);

  async function run(install: boolean) {
    if (busy || (install && !seed)) return;
    const version = epoch.current;
    const abort = new AbortController(); controller.current = abort;
    setBusy(true); setError("");
    if (!install) setSeed(undefined);
    try {
      if (install && seed) {
        const receipt = await command<Receipt>(token, "install_geo_seed", {
          name, expected_current_sha256: seed.current_sha256,
          expected_seed_sha256: seed.seed_sha256, accept_metadata_only: accept,
        }, abort.signal);
        if (version !== epoch.current) return;
        if (dat && receipt.core_load_verified !== true) throw new Error(t(language, "geoSeedDatProbeFailed"));
        setSeed(undefined);
        const changeStatus = receipt.changed ? t(language, "geoSeedChanged") : t(language, "geoSeedUnchanged");
        const structureStatus = dat ? t(language, "geoSeedDatStatus") : receipt.validation.verified ? t(language, "geoSeedMmdbVerified") : t(language, "geoSeedMmdbUnverified");
        const syncStatus = !receipt.durable || receipt.cleanup_pending ? t(language, "geoSeedSyncPending") : "";
        installed(t(language, "geoSeedInstalled", { name, changeStatus, structureStatus, sha256: receipt.validation.sha256, syncStatus }));
      } else {
        const next = await command<Seed>(token, "geo_seed", { name }, abort.signal);
        if (version === epoch.current) setSeed(next);
      }
    } catch (error) {
      if (version !== epoch.current) return;
      if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
      else setError(error instanceof Error ? error.message : String(error));
      // A failed/ambiguous install must be inspected again, never retried with stale state.
      if (install) setSeed(undefined);
    } finally {
      if (version === epoch.current) setBusy(false);
    }
  }
  return <div>
    <button type="button" disabled={busy || connection !== "connected"} onClick={() => void run(false)}>{t(language, "geoSeedRead", { name })}</button>
    {seed && <>
      <p>{t(language, "geoSeedCandidate", { bytes: seed.seed_bytes })}<code>{seed.seed_sha256}</code></p>
      <p>{t(language, "geoSeedCurrent")}<code>{seed.current_sha256 || t(language, "geoSeedFileMissing")}</code></p>
      {!dat && <label><input type="checkbox" checked={accept} disabled={busy} onChange={event => setAccept(event.target.checked)} />{t(language, "geoSeedAllowEmptyMmdb")}</label>}
      {status.phase !== "stopped" && <p className="info">{t(language, "geoSeedStoppedNotice")}</p>}
      <button type="button" disabled={busy || connection !== "connected" || status.phase !== "stopped"} onClick={() => void run(true)}>{t(language, "geoSeedInstall", { name })}</button>
    </>}
    {busy && <p role="status">{t(language, "geoSeedWorking")}</p>}
    {error && <p role="alert" className="alert">{t(language, "geoSeedFailed", { message: error })}</p>}
  </div>;
}
