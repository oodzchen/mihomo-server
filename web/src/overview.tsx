import { useEffect, useState, type MouseEvent } from "react";
import { subscribe, type Connection } from "./api";
import { ProxyAccessPanel } from "./proxy-access";
import { phaseLabel, t, type Language } from "./i18n";
import type { CoreLog, CoreStatus, Profile } from "./types";
import { bytes } from "./format";
import { LogLines } from "./logs";

export function useFeed<T>(token: string, feed: string) {
  const [value, setValue] = useState<T>();
  useEffect(
    () =>
      subscribe(
        token,
        `/api/streams/${feed}`,
        (event) => {
          if (event.type === "data") setValue(event.data as T);
          if (
            event.type === "stream_error" ||
            event.type === "ready" ||
            (event.type === "core_state" &&
              (event.data as CoreStatus).phase !== "running")
          )
            setValue(undefined);
        },
        (state) => {
          if (state !== "connected") setValue(undefined);
        },
      ),
    [token, feed],
  );
  return value;
}

export function Overview({
  language,
  token,
  connection,
  logout,
  status,
  active,
  navigate,
  logs,
}: {
  language: Language;
  token: string;
  connection: Connection;
  logout: (reason?: string) => void;
  status: CoreStatus;
  active?: Profile;
  navigate: (event: MouseEvent, path: string) => void;
  logs: CoreLog[];
}) {
  const traffic = useFeed<{ up: number; down: number }>(token, "traffic"),
    memory = useFeed<{ inuse: number }>(token, "memory"),
    connections = useFeed<{ count: number }>(token, "connections_count");
  return (
    <>
      <div className="metrics">
        <div>
          <p>{t(language, "uploadRate")}</p>
          <strong>
            {bytes(traffic?.up)}
            <small>/s</small>
          </strong>
        </div>
        <div>
          <p>{t(language, "downloadRate")}</p>
          <strong>
            {bytes(traffic?.down)}
            <small>/s</small>
          </strong>
        </div>
        <div>
          <p>{t(language, "activeConnections")}</p>
          <strong>{connections?.count ?? "—"}</strong>
        </div>
        <div>
          <p>{t(language, "memory")}</p>
          <strong>{bytes(memory?.inuse)}</strong>
        </div>
      </div>
      <ProxyAccessPanel
        token={token}
        status={status}
        connection={connection}
        logout={logout}
      />
      <div className="two-column">
        <section className="panel">
          <div className="panel-title">
            <h2>{t(language, "currentConfig")}</h2>
            <a href="/config" onClick={(event) => navigate(event, "/config")}>
              {t(language, "editConfig")}
            </a>
          </div>
          <dl>
            <div>
              <dt>{t(language, "activeProfile")}</dt>
              <dd>{active?.name || t(language, "notSelected")}</dd>
            </div>
            <div>
              <dt>{t(language, "configVersion")}</dt>
              <dd className="mono">{status.config_revision || t(language, "notCommitted")}</dd>
            </div>
            <div>
              <dt>{t(language, "coreStatus")}</dt>
              <dd>{phaseLabel(language, status.phase)}</dd>
            </div>
          </dl>
          <p className="hint">
            {t(language, "configHint")}
          </p>
        </section>
        <section className="panel">
          <div className="panel-title">
            <h2>{t(language, "getStarted")}</h2>
            <a
              href="/profiles"
              onClick={(event) => navigate(event, "/profiles")}
            >
              {t(language, "manageProfiles")}
            </a>
          </div>
          <ol className="steps">
            <li>{t(language, "stepImport")}</li>
            <li>{t(language, "stepUse")}</li>
            <li>{t(language, "stepSelect")}</li>
          </ol>
        </section>
      </div>
      <section className="panel">
        <div className="panel-title">
          <h2>{t(language, "recentLogs")}</h2>
          <a href="/logs" onClick={(event) => navigate(event, "/logs")}>
            {t(language, "viewAll")}
          </a>
        </div>
        <LogLines logs={logs.slice(-8)} language={language} />
      </section>
    </>
  );
}
