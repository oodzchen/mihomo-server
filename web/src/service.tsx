import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ApiError, command, loadedServedBuild, type Connection } from "./api";
import { describe } from "./format";
import { HelpTip } from "./help-tip";
import { t, type Language } from "./i18n";
import { useToast, type ToastOperation } from "./toast";
import type { UpdateChecks } from "./types";

type ServiceInfo = {
  version: string;
  installation?: "nix" | "installer" | "standalone";
  release: string | null;
  unit: string | null;
  upgrade: { available: boolean; state: string; result: string; log: string[] };
};
type Operation = "restart" | "stop" | "upgrade";
type Pending = {
  toast: ToastOperation;
  /** The connection dropped since the request (the service went away). */
  dropped: boolean;
  /** The update unit was seen running. */
  seen: boolean;
  release: string | null;
  started: number;
  /** When a resumed upgrade had already been reported. */
  finished?: number;
};

/** An update unit not seen running by then has finished (or never ran). */
const UPGRADE_START_GRACE = 10_000;
/** A restart or stop not observed by then did not happen as requested. */
const ACTION_TIMEOUT = 60_000;
/** An upgrade the page follows; it reloads when the service comes back upgraded. */
const UPGRADE_KEY = "mihomo.serviceUpgrade";
/** A remembered upgrade older than this is not resumed. */
const UPGRADE_RESUME_LIMIT = 30 * 60_000;
/** A finished upgrade is reported again when the page reloads to the
 * upgraded service's new build this soon after it. */
const UPGRADE_REPORT_LIMIT = 30_000;

type RememberedUpgrade = { release: string | null; started: number; finished?: number };

function rememberUpgrade(upgrade?: RememberedUpgrade) {
  try {
    if (upgrade) window.sessionStorage.setItem(UPGRADE_KEY, JSON.stringify(upgrade));
    else window.sessionStorage.removeItem(UPGRADE_KEY);
  } catch { /* Private browser storage can be unavailable. */ }
}

function rememberedUpgrade(): RememberedUpgrade | undefined {
  try {
    const value = JSON.parse(window.sessionStorage.getItem(UPGRADE_KEY) ?? "null");
    const now = Date.now();
    if (
      typeof value?.started === "number" && now - value.started < UPGRADE_RESUME_LIMIT &&
      (typeof value.finished !== "number" || (loadedServedBuild && now - value.finished < UPGRADE_REPORT_LIMIT))
    )
      return {
        release: typeof value.release === "string" ? value.release : null,
        started: value.started,
        finished: typeof value.finished === "number" ? value.finished : undefined,
      };
  } catch { /* Unreadable: nothing to resume. */ }
  return undefined;
}

export function ServicePage({
  token,
  language,
  connection,
  busy,
  logout,
}: {
  token: string;
  language: Language;
  connection: Connection;
  busy: boolean;
  logout: (reason?: string) => void;
}) {
  const notify = useToast();
  const [info, setInfo] = useState<ServiceInfo>();
  const [check, setCheck] = useState<UpdateChecks["service"]>(null);
  const [checking, setChecking] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [operation, setOperation] = useState<Operation>();
  const pending = useRef<Pending>(undefined);
  const output = useRef<HTMLPreElement>(null);

  useEffect(() => {
    // Resume an upgrade followed before the page reloaded to its new build.
    const resumed = rememberedUpgrade();
    if (resumed && !pending.current) {
      const toast = notify.loading(t(language, "serviceUpgradeStarted"));
      pending.current = { toast, dropped: true, seen: false, ...resumed };
      setOperation("upgrade");
    }
    return () => {
      pending.current?.toast.dismiss();
      rememberUpgrade();
    };
  }, []);

  useEffect(() => {
    if (connection !== "connected") return;
    const controller = new AbortController();
    command<ServiceInfo>(token, "service_info", {}, controller.signal)
      .then(setInfo)
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
      });
    return () => controller.abort();
  }, [token, connection, refresh, logout, language]);

  useEffect(() => {
    if (connection !== "connected") return;
    const controller = new AbortController();
    command<UpdateChecks>(token, "update_checks", {}, controller.signal)
      .then((checks) => setCheck(checks.service))
      .catch(() => {
        // Without a record the page simply offers a new check.
      });
    return () => controller.abort();
  }, [token, connection]);

  const running = info?.upgrade.state === "activating";
  useEffect(() => {
    if (connection !== "connected" || (operation !== "upgrade" && !running)) return;
    const timer = window.setInterval(() => setRefresh((value) => value + 1), 2000);
    return () => window.clearInterval(timer);
  }, [connection, operation, running]);

  function settle(message: string, kind?: "error") {
    const current = pending.current;
    current?.toast.finish(message, kind);
    pending.current = undefined;
    // The service may come back with a new page build right after an upgrade
    // and reload this page, which then reports the same result again.
    rememberUpgrade(
      operation === "upgrade" && current ? { release: current.release, started: current.started, finished: current.finished ?? Date.now() } : undefined,
    );
    setOperation(undefined);
  }

  // Restart and stop finish when the service goes away (and comes back).
  useEffect(() => {
    const current = pending.current;
    if (!current || !operation) return;
    if (connection !== "connected") {
      current.dropped = true;
      if (operation === "stop") settle(t(language, "serviceStopped"));
    } else if (current.dropped && operation === "restart") {
      settle(t(language, "serviceRestarted"));
    }
  }, [connection]);

  // An upgrade finishes when the update unit is no longer running.
  useEffect(() => {
    const current = pending.current;
    if (!current || operation !== "upgrade" || !info) return;
    if (info.upgrade.state === "activating") {
      current.seen = true;
      return;
    }
    if (!current.seen && Date.now() - current.started < UPGRADE_START_GRACE) return;
    if (info.upgrade.state === "failed") settle(t(language, "serviceUpgradeFailedToast"), "error");
    else if (info.release !== current.release)
      settle(t(language, "serviceUpgraded", { version: info.release ?? info.version }));
    else settle(t(language, "serviceUpToDate", { version: info.release ?? info.version }));
  }, [info]);

  const log = info?.upgrade.log;
  const follow = () => {
    if (output.current) output.current.scrollTop = output.current.scrollHeight;
  };
  useLayoutEffect(follow, [log]);

  async function start(kind: Operation, confirmation: string, working: string) {
    if (pending.current || !window.confirm(confirmation)) return;
    const toast = notify.loading(working);
    const current: Pending = { toast, dropped: false, seen: false, release: info?.release ?? null, started: Date.now() };
    pending.current = current;
    setOperation(kind);
    if (kind === "upgrade") rememberUpgrade({ release: current.release, started: current.started });
    if (kind !== "upgrade")
      window.setTimeout(() => {
        if (pending.current === current) settle(t(language, "serviceActionTimeout"), "error");
      }, ACTION_TIMEOUT);
    try {
      await command(token, `${kind}_service`);
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        toast.dismiss();
        pending.current = undefined;
        rememberUpgrade();
        setOperation(undefined);
        logout(t(language, "expiredToken"));
        return;
      }
      settle(describe(error), "error");
    }
  }

  async function checkRelease() {
    if (!info) return;
    setChecking(true);
    try {
      const latest = await command<string>(token, "service_release");
      // The service records the same check for later visits.
      setCheck({ installed: info.release, latest });
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
      else notify(describe(error), "error");
    } finally {
      setChecking(false);
    }
  }

  const connected = connection === "connected";
  const idle = connected && !busy && !operation;
  const upgrade = info?.upgrade;
  // A check taken before this program changed says nothing about it,
  // unless it found exactly the release installed since.
  const release = info?.release ?? null;
  const known = info && check && (check.installed === release || check.latest === release) ? check : undefined;
  const nix = info?.installation === "nix";
  const available = !nix && known && known.installed === release && known.latest !== release ? known.latest : undefined;
  // Logs in through the fragment, which browsers never send to the server.
  const dashboard = `${location.origin}/#token=${encodeURIComponent(token)}`;
  return (
    <>
      <section className="panel" aria-label={t(language, "serviceControl")}>
        <div className="panel-title">
          <h2 className="setting-heading">
            {t(language, "serviceControl")}
            <HelpTip label={t(language, "serviceControl")}>{t(language, "serviceHelp")}</HelpTip>
          </h2>
        </div>
        <dl className="proxy-details">
          <div>
            <dt>{t(language, "serviceState")}</dt>
            <dd>
              <span className={`badge ${connected ? "good" : ""}`}>
                {t(language, connected ? "serviceRunning" : "serviceUnreachable")}
              </span>
            </dd>
          </div>
          <div>
            <dt>{t(language, "serviceVersionLabel")}</dt>
            <dd className="mono">{info?.version ?? t(language, "serviceReading")}</dd>
          </div>
          <div>
            <dt>{t(language, "serviceUnit")}</dt>
            <dd className="mono">
              {!info ? t(language, "serviceReading") : info.unit ?? (
                <>
                  <span>{t(language, "serviceUnitNone")}</span>
                  <HelpTip label={t(language, "serviceUnitNone")}>{t(language, "serviceUnitNoneHint")}</HelpTip>
                </>
              )}
            </dd>
          </div>
        </dl>
        <div className="actions service-controls">
          <button
            type="button"
            disabled={!idle || !info?.unit}
            onClick={() => void start("restart", t(language, "serviceConfirmRestart"), t(language, "serviceRestarting"))}
          >
            {t(language, "restartService")}
          </button>
          <button
            type="button"
            disabled={!idle || !info}
            onClick={() => void start("stop", t(language, "serviceConfirmStop"), t(language, "serviceStopping"))}
          >
            {t(language, "stopService")}
          </button>
        </div>
      </section>

      <section className="panel" aria-label={t(language, "serviceUpgradeTitle")}>
        <div className="panel-title">
          <h2 className="setting-heading">
            {t(language, "serviceUpgradeTitle")}
            <HelpTip label={t(language, "serviceUpgradeTitle")}>{t(language, nix ? "serviceNixUpdateHint" : "serviceUpgradeHelp")}</HelpTip>
          </h2>
        </div>
        <dl className="proxy-details">
          <div>
            <dt>{t(language, "serviceRelease")}</dt>
            <dd className="mono">
              {!info ? t(language, "serviceReading") : (nix ? info.version : info.release) ?? t(language, "serviceReleaseNone")}
            </dd>
          </div>
          <div>
            <dt>{t(language, "serviceLatest")}</dt>
            <dd className="mono">
              {!known
                ? t(language, "serviceNotChecked")
                : known.latest === release
                ? t(language, "updateLatestCurrent", { version: known.latest })
                : known.latest}
            </dd>
          </div>
          <div>
            <dt>{t(language, "serviceUpgradeState")}</dt>
            <dd>
              {!upgrade ? t(language, "serviceReading")
                : !upgrade.available ? (
                  <>
                    <span>{t(language, nix ? "serviceNixManaged" : "serviceUpgradeUnavailable")}</span>
                    <HelpTip label={t(language, "serviceUpgradeUnavailable")}>
                      {t(language, nix ? "serviceNixUpdateHint" : "serviceUpgradeUnavailableHint")}
                    </HelpTip>
                  </>
                )
                : upgrade.state === "activating" ? t(language, "serviceUpgradeRunning")
                : upgrade.state === "failed" ? t(language, "serviceUpgradeFailed")
                : t(language, "serviceUpgradeIdle")}
            </dd>
          </div>
        </dl>
        {/* Collapsed by default; only the user's own click changes its height. */}
        <details className="service-output" onToggle={follow}>
          <summary>{t(language, "serviceUpgradeOutput")}</summary>
          <pre ref={output} className="service-log" aria-label={t(language, "serviceUpgradeOutput")}>
            {!upgrade || upgrade.available
              ? log?.length ? log.join("\n") : t(language, "serviceUpgradeNoOutput")
              : t(language, nix ? "serviceNixUpdateHint" : "serviceUpgradeOutputUnavailable")}
          </pre>
        </details>
        <div className="actions">
          {available ? (
            <button
              type="button"
              className="primary"
              disabled={!idle || !upgrade?.available || running}
              onClick={() => void start("upgrade", t(language, "serviceUpgradeConfirm"), t(language, "serviceUpgradeStarted"))}
            >
              {t(language, "updateTo", { version: available })}
            </button>
          ) : (
            <button type="button" disabled={!idle || checking || !info || nix} onClick={() => void checkRelease()}>
              {t(language, "updateCheck")}
            </button>
          )}
        </div>
      </section>

      <p className="service-address">
        <span>{t(language, "serviceWebAddress")}</span>
        <a href={dashboard} target="_blank" rel="noopener noreferrer" className="mono">
          {dashboard}
        </a>
        <HelpTip label={t(language, "serviceWebAddress")}>{t(language, "serviceWebAddressHint")}</HelpTip>
      </p>
    </>
  );
}
