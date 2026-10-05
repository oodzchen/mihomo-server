import { useToast } from "./toast";
import { useEffect, useRef, useState } from "react";
import { ApiError, command, type Perform, type Connection } from "./api";
import { describe } from "./format";
import { phaseLabel, t, type Language } from "./i18n";
import type { CoreStatus, UpdateChecks } from "./types";
import { HelpTip } from "./help-tip";

type Release = { version: string; bytes: number; target: string };
type Installation = { version: string; stage_id: string };
type Report = { upgraded: boolean; from: string; to: string };
type Check = NonNullable<UpdateChecks["core"]>;

export function CoreUpgradePage({
  token,
  language,
  status,
  connection,
  busy,
  perform,
  logout,
  activeProfileName,
}: {
  token: string;
  language: Language;
  status: CoreStatus;
  connection: Connection;
  busy: boolean;
  perform: Perform;
  logout: (reason?: string) => void;
  activeProfileName?: string;
}) {
  const notify = useToast();
  const [channel, setChannel] = useState<"stable" | "alpha">("stable");
  const label = channel === "alpha" ? "Alpha" : t(language, "channelStable");
  const [version, setVersion] = useState<string>();
  const [installation, setInstallation] = useState<Installation | null>();
  const [check, setCheck] = useState<Check | null>(null);
  const [report, setReport] = useState<Report>();
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const alive = useRef(true);
  const locked = useRef(false);
  /** The channel of the recorded check is adopted once, on the first read. */
  const adopted = useRef(false);
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
    if (connection !== "connected") return;
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

  /** Checks for the latest release, or installs the one a check found. */
  // Read once per connection: later checks on this page are newer.
  useEffect(() => {
    if (connection !== "connected") return;
    const controller = new AbortController();
    command<UpdateChecks>(token, "update_checks", {}, controller.signal)
      .then((checks) => {
        // A check or update running now gives the newer answer.
        if (locked.current) return;
        setCheck(checks.core);
        if (checks.core && !adopted.current) setChannel(checks.core.channel);
        adopted.current = true;
      })
      .catch(() => {
        // Without a record the page simply offers a new check.
      });
    return () => controller.abort();
  }, [token, connection]);

  async function run(upgrade: boolean) {
    if (locked.current || busy || !version) return;
    locked.current = true;
    setWorking(true);
    setReport(undefined);
    try {
      if (!upgrade) {
        try {
          const value = await command<Release>(token, channel === "alpha" ? "alpha_core_release" : "core_release");
          // The service records the same check for later visits.
          if (alive.current) setCheck({ channel, installed: version, latest: value.version });
        } catch (error) {
          if (!alive.current) return;
          if (error instanceof ApiError && error.status === 401)
            logout(t(language, "expiredToken"));
          else notify(describe(error), "error");
        }
      } else {
        const toast = notify.loading(t(language, "coreUpgradeWorking"));
        const value = await perform<Report>(channel === "alpha" ? "upgrade_alpha_core" : "upgrade_clash_core", { force: false }, { notify: false, toast });
        if (alive.current && value) {
          setReport(value);
          toast.finish(value.upgraded
            ? value.from === "unknown"
              ? t(language, "coreUpgradeReportRepaired", { version: value.to })
              : t(language, "coreUpgradeReportUpgraded", { from: value.from, to: value.to })
            : t(language, "coreUpgradeReportAlreadyLatest", { label, version: value.to }));
        }
        if (!alive.current) toast.dismiss();
        if (alive.current) setRefresh((value) => value + 1);
      }
    } finally {
      locked.current = false;
      if (alive.current) setWorking(false);
    }
  }
  const transitional = [
    "starting",
    "stopping",
    "recovering",
    "shutdown",
  ].includes(status.phase);
  const disabled = busy || working || connection !== "connected" || !version;
  // A check taken before the installed core changed says nothing about it,
  // unless it found exactly the version installed since.
  const known =
    check?.channel === channel && (check.installed === version || check.latest === version)
      ? check
      : undefined;
  const available = known && known.installed === version && known.latest !== version ? known.latest : undefined;
  return (
    <>
      <section className="panel" aria-label={t(language, "coreStatus")}>
        <div className="panel-title">
          <h2>{t(language, "coreManagement")}</h2>
        </div>
        <dl className="proxy-details">
          <div>
            <dt>{t(language, "coreRunningPhase")}</dt>
            <dd>
              <span className={`badge ${status.phase === "running" ? "good" : ""}`}>
                {phaseLabel(language, status.phase)}
              </span>
            </dd>
          </div>
          <div>
            <dt>{t(language, "corePid")}</dt>
            <dd className="mono">{status.pid ? status.pid : t(language, "coreStopped")}</dd>
          </div>
          <div>
            <dt>{t(language, "activeProfile")}</dt>
            <dd>{activeProfileName || (status.active_profile ? status.active_profile : t(language, "noProfile"))}</dd>
          </div>
          <div>
            <dt>{t(language, "coreName")}</dt>
            <dd className="mono">{status.version || t(language, "coreStopped")}</dd>
          </div>
        </dl>
        <div className="actions">
          <button
            type="button"
            disabled={busy || transitional || status.phase === "running"}
            onClick={() => void perform("start")}
          >
            {t(language, "startCore")}
          </button>
          <button
            type="button"
            disabled={busy || !["running", "recovering"].includes(status.phase)}
            onClick={() => void perform("stop")}
          >
            {t(language, "stopCore")}
          </button>
          <button
            type="button"
            disabled={busy || transitional}
            onClick={() => void perform("restart")}
          >
            {t(language, "restartCore")}
          </button>
        </div>
      </section>

      <section className="panel" aria-label={t(language, "coreUpgradeTitle", { label })}>
      <div className="panel-title">
        <h2 className="setting-heading">
          {t(language, "coreUpgradeTitle", { label })}
          <HelpTip label={t(language, "coreUpgradeTitle", { label })}>
            <span>{t(language, "coreUpgradeDesc", { label })}</span>
            {channel === "alpha" && <span>{t(language, "coreUpgradeAlphaNotice")}</span>}
            <span>{t(language, "coreUpgradeHint")}</span>
          </HelpTip>
        </h2>
      </div>
      <label>
        {t(language, "coreUpgradeChannel")}
        <select
          value={channel}
          disabled={busy || working || connection !== "connected"}
          onChange={(event) => {
            if (locked.current || busy) return;
            setChannel(event.target.value === "alpha" ? "alpha" : "stable");
            setReport(undefined);
          }}
        >
          <option value="stable">{t(language, "coreUpgradeChannelStable")}</option>
          <option value="alpha">{t(language, "coreUpgradeChannelAlpha")}</option>
        </select>
      </label>
      {connection !== "connected" ? (
        <p className="info">{t(language, "coreUpgradeDisconnected")}</p>
      ) : error ? (
        <p className="alert" role="alert">
          {error}
        </p>
      ) : !version ? (
        <p className="info">{t(language, "coreUpgradeReadingInstalled")}</p>
      ) : null}
      {version && connection === "connected" && (
        <dl className="proxy-details">
          <div>
            <dt>{t(language, "coreUpgradeInstalledVersion")}</dt>
            <dd>
              {version === "unknown" ? (
                <>
                  <span>{t(language, "coreUpgradeUnknownRepair")}</span>
                  <HelpTip label={t(language, "coreUpgradeUnknownRepair")}>
                    {t(language, "coreUpgradeUnknownHint", { label })}
                  </HelpTip>
                </>
              ) : version}
            </dd>
          </div>
          <div>
            <dt>{t(language, "coreUpgradeLatestChannel", { label })}</dt>
            <dd>
              {!known
                ? t(language, "coreUpgradeNotChecked")
                : known.latest === version
                ? t(language, "updateLatestCurrent", { version: known.latest })
                : known.latest}
            </dd>
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
      <div className="actions">
        <button
          type="button"
          className={available ? "primary" : undefined}
          disabled={disabled}
          onClick={() => void run(available !== undefined)}
        >
          {available ? t(language, "updateTo", { version: available }) : t(language, "updateCheck")}
        </button>
      </div>
      {report && version && version !== report.to && (
        <p className="alert" role="alert">
          {t(language, "coreUpgradeMismatchWarning")}
        </p>
      )}
    </section>
    </>
  );
}
