import { ToastMessage } from "./toast";
import { useEffect, useMemo, useState } from "react";
import { command, type Perform } from "./api";
import { t, type Language, type MessageKey } from "./i18n";
import type { CoreStatus, Rule, RuleProvider, RuleProviders, Rules } from "./types";

export function RulesPage({
  token,
  language,
  status,
  busy,
  perform,
}: {
  token: string;
  language: Language;
  status: CoreStatus;
  busy: boolean;
  perform: Perform;
}) {
  const [rules, setRules] = useState<Rule[]>([]);
  const [providers, setProviders] = useState<Record<string, RuleProvider>>({});
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [updating, setUpdating] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<{ key?: MessageKey; name?: string; message?: string } | string>("");
  const [notice, setNotice] = useState<{ key: MessageKey; name?: string } | null>(null);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    if (status.phase !== "running") {
      setRules([]);
      setProviders({});
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    setError("");

    Promise.all([
      command<Rules>(token, "rules", {}, controller.signal),
      command<RuleProviders>(token, "rule_providers", {}, controller.signal).catch(
        () => ({ providers: {} } as RuleProviders),
      ),
    ])
      .then(([rulesRes, providersRes]) => {
        if (!controller.signal.aborted) {
          setRules(rulesRes.rules || []);
          setProviders(providersRes.providers || {});
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!controller.signal.aborted) {
          setError(err instanceof Error ? err.message : String(err));
          setLoading(false);
        }
      });

    return () => controller.abort();
  }, [token, status.phase, status.generation, status.config_revision, revision]);

  const filteredRules = useMemo(() => {
    if (!query.trim()) return rules;
    const lower = query.toLowerCase().trim();
    return rules.filter(
      (r) =>
        r.type.toLowerCase().includes(lower) ||
        r.payload.toLowerCase().includes(lower) ||
        r.proxy.toLowerCase().includes(lower),
    );
  }, [rules, query]);

  async function updateProvider(name: string) {
    setUpdating((prev) => ({ ...prev, [name]: true }));
    setError("");
    setNotice(null);
    try {
      const result = await perform("update_rule_provider", { name }, { notify: false });
      if (result === undefined) return;
      setNotice({ key: "ruleProviderUpdated", name });
      setRevision((v) => v + 1);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setUpdating((prev) => ({ ...prev, [name]: false }));
    }
  }

  async function updateAllProviders() {
    const names = Object.keys(providers);
    if (!names.length) return;
    setError("");
    setNotice(null);
    for (const name of names) {
      setUpdating((prev) => ({ ...prev, [name]: true }));
      try {
        const result = await perform("update_rule_provider", { name }, { notify: false });
        if (result === undefined) return;
      } catch (err) {
        setError({
          key: "ruleProviderUpdateFailed",
          name,
          message: err instanceof Error ? err.message : String(err),
        });
      } finally {
        setUpdating((prev) => ({ ...prev, [name]: false }));
      }
    }
    setNotice({ key: "ruleProvidersUpdatedAll" });
    setRevision((v) => v + 1);
  }

  const providerList = Object.values(providers);

  return (
    <>
      <div className="section-title">
        <div>
          <h2>{t(language, "rulesTitle")}</h2>
          <p className="muted">
            {t(language, "rulesSummary").replace("{count}", String(rules.length))}
            {providerList.length > 0 && t(language, "rulesSummaryProviders").replace("{count}", String(providerList.length))}
            {t(language, "rulesSummaryEnd")}
          </p>
        </div>
        <div className="actions">
          <button
            disabled={busy || loading || status.phase !== "running"}
            onClick={() => setRevision((v) => v + 1)}
          >
            {t(language, "rulesRefresh")}
          </button>
        </div>
      </div>

      {status.phase !== "running" && (
        <p className="info">{t(language, "rulesNeedsCore")}</p>
      )}

      {error && (
        <p className="alert" role="alert">
          {typeof error === "string"
            ? error
            : error.key
              ? t(language, error.key)
                  .replace("{name}", error.name || "")
                  .replace("{error}", error.message || "")
              : error.message}
        </p>
      )}
      {notice && (
        <ToastMessage kind="success" message={t(language, notice.key).replace("{name}", notice.name || "")} />
      )}

      {providerList.length > 0 && (
        <section className="panel" aria-label={t(language, "ruleProviderTitle")}>
          <div className="panel-title">
            <div>
              <h3>{t(language, "ruleProviderTitle")}</h3>
              <p className="muted">{t(language, "ruleProviderSubtitle")}</p>
            </div>
            <button
              disabled={busy || loading || status.phase !== "running" || Object.values(updating).some(Boolean)}
              onClick={() => void updateAllProviders()}
            >
              {t(language, "ruleProviderUpdateAll")}
            </button>
          </div>
          <div className="provider-grid">
            {providerList.map((p) => (
              <div className="provider-card" key={p.name}>
                <div className="provider-header">
                  <strong>{p.name}</strong>
                  <span className="badge">{p.behavior}</span>
                </div>
                <p className="muted">
                  {t(language, "ruleProviderFormat")}{p.format} · {t(language, "ruleProviderType")}{p.vehicleType || p.type} · {t(language, "ruleProviderRulesCount").replace("{count}", String(p.ruleCount))}
                </p>
                {p.updatedAt && (
                  <p className="hint">
                    {t(language, "ruleProviderUpdatedAt")}{p.updatedAt}
                  </p>
                )}
                <div className="card-actions">
                  <button
                    disabled={busy || loading || status.phase !== "running" || updating[p.name]}
                    onClick={() => void updateProvider(p.name)}
                  >
                    {t(language, updating[p.name] ? "ruleProviderUpdating" : "ruleProviderUpdate")}
                  </button>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {status.phase === "running" && (
        <section className="panel" aria-label={t(language, "rulesListRegion")}>
          <div className="rules-search">
            <input
              type="search"
              placeholder={t(language, "rulesSearchPlaceholder")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              aria-label={t(language, "rulesSearch")}
            />
            {query && (
              <button className="quiet" onClick={() => setQuery("")}>
                {t(language, "rulesClear")}
              </button>
            )}
            <span className="muted">
              {filteredRules.length === rules.length
                ? t(language, "rulesTotal").replace("{count}", String(rules.length))
                : t(language, "rulesMatches")
                    .replace("{matched}", String(filteredRules.length))
                    .replace("{total}", String(rules.length))}
            </span>
          </div>

          {loading ? (
            <p className="info">{t(language, "rulesLoading")}</p>
          ) : filteredRules.length === 0 ? (
            <p className="empty">
              {t(language, rules.length === 0 ? "rulesEmpty" : "rulesNoMatches")}
            </p>
          ) : (
            <div className="rules-table-scroll">
              <table className="rules-table">
                <thead>
                  <tr>
                    <th style={{ width: "60px" }}>#</th>
                    <th style={{ width: "160px" }}>{t(language, "rulesTypeColumn")}</th>
                    <th>{t(language, "rulesPayloadColumn")}</th>
                    <th style={{ width: "180px" }}>{t(language, "rulesTargetColumn")}</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredRules.map((rule, idx) => (
                    <tr key={`${rule.type}-${rule.payload}-${rule.proxy}-${idx}`}>
                      <td className="muted">{idx + 1}</td>
                      <td>
                        <span className="rule-type-badge">{rule.type}</span>
                      </td>
                      <td className="rule-payload">
                        <code>{rule.payload || "—"}</code>
                      </td>
                      <td>
                        <span className="rule-proxy-badge">{rule.proxy}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
      )}
    </>
  );
}
