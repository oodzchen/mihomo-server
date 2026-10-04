import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ApiError, command, type Connection } from "./api";
import { describe } from "./format";
import { HelpTip } from "./help-tip";
import { t, type Language } from "./i18n";
import { useToast, type ToastOperation } from "./toast";

type ServiceInfo = {
  version: string;
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
};

/** An update unit not seen running by then has finished (or never ran). */
const UPGRADE_START_GRACE = 10_000;
/** A restart or stop not observed by then did not happen as requested. */
const ACTION_TIMEOUT = 60_000;

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
  const [latest, setLatest] = useState<string>();
  const [checking, setChecking] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [operation, setOperation] = useState<Operation>();
  const pending = useRef<Pending>(undefined);
  const output = useRef<HTMLPreElement>(null);

  useEffect(() => () => pending.current?.toast.dismiss(), []);

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

  const running = info?.upgrade.state === "activating";
  useEffect(() => {
    if (connection !== "connected" || (operation !== "upgrade" && !running)) return;
    const timer = window.setInterval(() => setRefresh((value) => value + 1), 2000);
    return () => window.clearInterval(timer);
  }, [connection, operation, running]);

  function settle(message: string, kind?: "error") {
    pending.current?.toast.finish(message, kind);
    pending.current = undefined;
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
  useLayoutEffect(() => {
    if (output.current) output.current.scrollTop = output.current.scrollHeight;
  }, [log]);

  async function start(kind: Operation, confirmation: string, working: string) {
    if (pending.current || !window.confirm(confirmation)) return;
    const toast = notify.loading(working);
    const current: Pending = { toast, dropped: false, seen: false, release: info?.release ?? null, started: Date.now() };
    pending.current = current;
    setOperation(kind);
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
        setOperation(undefined);
        logout(t(language, "expiredToken"));
        return;
      }
      settle(describe(error), "error");
    }
  }

  async function check() {
    setChecking(true);
    try {
      setLatest(await command<string>(token, "service_release"));
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
  const upToDate = latest !== undefined && latest === info?.release;
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
        <div className="actions">
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
            <HelpTip label={t(language, "serviceUpgradeTitle")}>{t(language, "serviceUpgradeHelp")}</HelpTip>
          </h2>
        </div>
        <dl className="proxy-details">
          <div>
            <dt>{t(language, "serviceRelease")}</dt>
            <dd className="mono">
              {!info ? t(language, "serviceReading") : info.release ?? t(language, "serviceReleaseNone")}
            </dd>
          </div>
          <div>
            <dt>{t(language, "serviceLatest")}</dt>
            <dd className="mono">{latest ?? t(language, "serviceNotChecked")}</dd>
          </div>
          <div>
            <dt>{t(language, "serviceUpgradeState")}</dt>
            <dd>
              {!upgrade ? t(language, "serviceReading")
                : !upgrade.available ? (
                  <>
                    <span>{t(language, "serviceUpgradeUnavailable")}</span>
                    <HelpTip label={t(language, "serviceUpgradeUnavailable")}>
                      {t(language, "serviceUpgradeUnavailableHint")}
                    </HelpTip>
                  </>
                )
                : upgrade.state === "activating" ? t(language, "serviceUpgradeRunning")
                : upgrade.state === "failed" ? t(language, "serviceUpgradeFailed")
                : t(language, "serviceUpgradeIdle")}
            </dd>
          </div>
        </dl>
        <pre ref={output} className="service-log" aria-label={t(language, "serviceUpgradeOutput")}>
          {!upgrade || upgrade.available
            ? log?.length ? log.join("\n") : t(language, "serviceUpgradeNoOutput")
            : t(language, "serviceUpgradeOutputUnavailable")}
        </pre>
        <div className="actions">
          <button type="button" disabled={!idle || checking} onClick={() => void check()}>
            {t(language, "serviceCheck")}
          </button>
          <button
            type="button"
            className="primary"
            disabled={!idle || !upgrade?.available || running || upToDate}
            onClick={() => void start("upgrade", t(language, "serviceUpgradeConfirm"), t(language, "serviceUpgradeStarted"))}
          >
            {upToDate ? t(language, "serviceUpToDate", { version: latest }) : t(language, "serviceUpgrade")}
          </button>
        </div>
      </section>
    </>
  );
}
