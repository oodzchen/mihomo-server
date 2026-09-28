import { useEffect, useRef, useState } from "react";
import { ApiError, command, type Perform } from "./api";
import { t, type Language } from "./i18n";
import type { CoreStatus } from "./types";

type Release = { version: string; bytes: number; target: string };
type Installation = { version: string; stage_id: string };
type Report = { upgraded: boolean; from: string; to: string };

export function CoreUpgradePage({
  token,
  language,
  status,
  connection,
  busy,
  perform,
  logout,
}: {
  token: string;
  language: Language;
  status: CoreStatus;
  connection: string;
  busy: boolean;
  perform: Perform;
  logout: (reason?: string) => void;
}) {
  const [channel, setChannel] = useState<"stable" | "alpha">("stable");
  const label = channel === "alpha" ? "Alpha" : t(language, "channelStable");
  const [version, setVersion] = useState<string>();
  const [installation, setInstallation] = useState<Installation | null>();
  const [latest, setLatest] = useState<Release>();
  const [report, setReport] = useState<Report>();
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const alive = useRef(true);
  const locked = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    setVersion(undefined);
    setInstallation(undefined);
    setError("");
    if (connection !== "已连接") return;
    const controller = new AbortController();
    let active = true;
    void Promise.allSettled([
      command<string>(token, "installed_core_version", {}, controller.signal),
      command<Installation | null>(
        token,
        "core_installation",
        {},
        controller.signal,
      ),
    ])
      .then(([version, receipt]) => {
        if (!active) return;
        if (version.status === "fulfilled") setVersion(version.value);
        if (receipt.status === "fulfilled") setInstallation(receipt.value);
        const failed = [version, receipt].filter(
          (result) => result.status === "rejected",
        );
        if (failed.length === 0) return;
        if (failed.some((result) => result.reason instanceof ApiError && result.reason.status === 401))
          logout(t(language, "expiredToken"));
        else {
          const error: unknown = failed[0].reason;
          const message =
            error instanceof Error ? error.message : String(error);
          setError(
            message.includes("bundle-managed resources")
              ? t(language, "coreUpgradeNotSupported")
              : t(language, "coreUpgradeReadFailed", { message }),
          );
        }
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [token, connection, status.generation, refresh, logout, language]);

  async function run(force?: boolean) {
    if (locked.current || busy) return;
    if (
      force === true &&
      !window.confirm(t(language, "coreUpgradeConfirmReinstall", { label }))
    )
      return;
    locked.current = true;
    setWorking(true);
    setReport(undefined);
    try {
      if (force === undefined) {
        setLatest(undefined);
        const value = await perform<Release>(channel === "alpha" ? "alpha_core_release" : "core_release");
        if (alive.current && value) setLatest(value);
      } else {
        const value = await perform<Report>(channel === "alpha" ? "upgrade_alpha_core" : "upgrade_clash_core", { force });
        if (alive.current && value) setReport(value);
        if (alive.current) setRefresh((value) => value + 1);
      }
    } finally {
      locked.current = false;
      if (alive.current) setWorking(false);
    }
  }
  const disabled = busy || working || connection !== "已连接" || !version;
  return (
    <section className="panel" aria-label={t(language, "coreUpgradeTitle", { label })}>
      <div className="panel-title">
        <h2>{t(language, "coreUpgradeTitle", { label })}</h2>
        <button
          type="button"
          disabled={busy || working || connection !== "已连接"}
          onClick={() => setRefresh((value) => value + 1)}
        >
          {t(language, "coreUpgradeRefresh")}
        </button>
      </div>
      <label>
        {t(language, "coreUpgradeChannel")}
        <select
          value={channel}
          disabled={busy || working || connection !== "已连接"}
          onChange={(event) => {
            if (locked.current || busy) return;
            setChannel(event.target.value === "alpha" ? "alpha" : "stable");
            setLatest(undefined);
            setReport(undefined);
          }}
        >
          <option value="stable">{t(language, "coreUpgradeChannelStable")}</option>
          <option value="alpha">{t(language, "coreUpgradeChannelAlpha")}</option>
        </select>
      </label>
      <p className="muted">
        {t(language, "coreUpgradeDesc", { label })}
      </p>
      {channel === "alpha" && (
        <p className="info">{t(language, "coreUpgradeAlphaNotice")}</p>
      )}
      {connection !== "已连接" ? (
        <p className="info">{t(language, "coreUpgradeDisconnected")}</p>
      ) : error ? (
        <p className="alert" role="alert">
          {error}
        </p>
      ) : !version ? (
        <p className="info">{t(language, "coreUpgradeReadingInstalled")}</p>
      ) : null}
      {version && connection === "已连接" && (
        <dl className="proxy-details">
          <div>
            <dt>{t(language, "coreUpgradeInstalledVersion")}</dt>
            <dd>{version === "unknown" ? t(language, "coreUpgradeUnknownRepair") : version}</dd>
          </div>
          <div>
            <dt>{t(language, "coreUpgradeLatestChannel", { label })}</dt>
            <dd>{latest?.version || t(language, "coreUpgradeNotChecked")}</dd>
          </div>
          <div>
            <dt>{t(language, "coreUpgradeInstallRecord")}</dt>
            <dd>
              {installation === undefined
                ? t(language, "coreUpgradeRecordUnverified")
                : installation
                ? t(language, "coreUpgradeRecordVerified", { version: installation.version })
                : t(language, "coreUpgradeRecordBundle")}
            </dd>
          </div>
        </dl>
      )}
      {version === "unknown" && (
        <p className="info">
          {t(language, "coreUpgradeUnknownHint", { label })}
        </p>
      )}
      <div className="actions">
        <button type="button" disabled={disabled} onClick={() => void run()}>
          {t(language, "coreUpgradeCheck", { label })}
        </button>
        <button
          type="button"
          className="primary"
          disabled={disabled}
          onClick={() => void run(false)}
        >
          {t(language, "coreUpgradeUpgrade", { label })}
        </button>
        <button
          type="button"
          disabled={disabled}
          onClick={() => void run(true)}
        >
          {t(language, "coreUpgradeReinstall", { label })}
        </button>
      </div>
      {working && (
        <p className="info" role="status">
          {t(language, "coreUpgradeWorking")}
        </p>
      )}
      {report && (
        <p className="success" role="status">
          {report.upgraded
            ? report.from === "unknown"
              ? t(language, "coreUpgradeReportRepaired", { version: report.to })
              : t(language, "coreUpgradeReportUpgraded", { from: report.from, to: report.to })
            : t(language, "coreUpgradeReportAlreadyLatest", { label, version: report.to })}
        </p>
      )}
      {report && version && version !== report.to && (
        <p className="alert" role="alert">
          {t(language, "coreUpgradeMismatchWarning")}
        </p>
      )}
      <p className="hint">
        {t(language, "coreUpgradeHint")}
      </p>
    </section>
  );
}
