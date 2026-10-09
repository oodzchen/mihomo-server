import { useState } from "react";
import { t, type Language } from "./i18n";
import type { CoreLog } from "./types";

export function LogLines({ logs, language }: { logs: CoreLog[]; language: Language }) {
  return (
    <div className="log-lines" role="log" aria-label={t(language, "logsAria")}>
      {logs.length ? (
        logs.map((log, index) => (
          <div key={index}>
            <span>{log.stream}</span>
            <code>{log.message}</code>
          </div>
        ))
      ) : (
        <p className="empty">{t(language, "logsEmpty")}</p>
      )}
    </div>
  );
}

export function LogPage({
  logs,
  language,
  clear,
}: {
  logs: CoreLog[];
  language: Language;
  clear: () => void;
}) {
  const [filter, setFilter] = useState("");
  const filtered = logs.filter((log) =>
    log.message.toLowerCase().includes(filter.toLowerCase()),
  );
  return (
    <section className="panel" aria-label={t(language, "logsTitle")}>
      <div className="panel-title">
        <div>
          <h2>{t(language, "logsTitle")}</h2>
          <p className="muted">{t(language, "logsSubtitle")}</p>
        </div>
        <div className="log-tools">
          <input
            aria-label={t(language, "logsFilterAria")}
            placeholder={t(language, "logsFilterPlaceholder")}
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
          {filter && (
            <button className="quiet" onClick={() => setFilter("")}>
              {t(language, "logsClearFilter")}
            </button>
          )}
          <button disabled={!logs.length} onClick={clear}>
            {t(language, "logsClear")}
          </button>
        </div>
      </div>
      {logs.length > 0 && !filtered.length ? (
        <p className="empty">{t(language, "logsNoMatches")}</p>
      ) : (
        <LogLines logs={filtered} language={language} />
      )}
    </section>
  );
}
