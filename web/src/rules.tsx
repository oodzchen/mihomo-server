import { useEffect, useMemo, useState } from "react";
import { command, type Perform } from "./api";
import type { CoreStatus, Rule, RuleProvider, RuleProviders, Rules } from "./types";

export function RulesPage({
  token,
  status,
  busy,
  perform,
}: {
  token: string;
  status: CoreStatus;
  busy: boolean;
  perform: Perform;
}) {
  const [rules, setRules] = useState<Rule[]>([]);
  const [providers, setProviders] = useState<Record<string, RuleProvider>>({});
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [updating, setUpdating] = useState<Record<string, boolean>>({});
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
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
    setNotice("");
    try {
      await perform("update_rule_provider", { name });
      setNotice(`规则集 ${name} 更新完成`);
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
    setNotice("");
    for (const name of names) {
      setUpdating((prev) => ({ ...prev, [name]: true }));
      try {
        await perform("update_rule_provider", { name });
      } catch (err) {
        setError(`更新 ${name} 失败: ${err instanceof Error ? err.message : String(err)}`);
      } finally {
        setUpdating((prev) => ({ ...prev, [name]: false }));
      }
    }
    setNotice("所有规则集更新完毕");
    setRevision((v) => v + 1);
  }

  const providerList = Object.values(providers);

  return (
    <>
      <div className="section-title">
        <div>
          <h2>规则与分流策略</h2>
          <p className="muted">
            内核实时生效的路由分流规则与规则集（Rule Providers）。共 {rules.length} 条规则
            {providerList.length > 0 && `，${providerList.length} 个规则集`}。
          </p>
        </div>
        <div className="actions">
          <button
            disabled={busy || loading || status.phase !== "running"}
            onClick={() => setRevision((v) => v + 1)}
          >
            刷新规则
          </button>
        </div>
      </div>

      {status.phase !== "running" && (
        <p className="info">内核未运行，启动内核后可查看生效规则与规则集。</p>
      )}

      {error && (
        <p className="alert" role="alert">
          {error}
        </p>
      )}
      {notice && (
        <p className="success" role="status">
          {notice}
        </p>
      )}

      {providerList.length > 0 && (
        <section className="panel" aria-label="规则集">
          <div className="panel-title">
            <div>
              <h3>外部规则集 (Rule Providers)</h3>
              <p className="muted">可在线按需更新外部规则集资源</p>
            </div>
            <button
              disabled={busy || loading || status.phase !== "running" || Object.values(updating).some(Boolean)}
              onClick={() => void updateAllProviders()}
            >
              全部更新
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
                  格式: {p.format} · 类型: {p.vehicleType || p.type} · 包含 {p.ruleCount} 条
                </p>
                {p.updatedAt && <p className="hint">更新时间: {p.updatedAt}</p>}
                <div className="card-actions">
                  <button
                    disabled={busy || loading || status.phase !== "running" || updating[p.name]}
                    onClick={() => void updateProvider(p.name)}
                  >
                    {updating[p.name] ? "更新中…" : "更新"}
                  </button>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {status.phase === "running" && (
        <section className="panel" aria-label="规则列表">
          <div className="rules-search">
            <input
              type="search"
              placeholder="搜索规则类型、域名、IP或目标策略…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              aria-label="搜索规则"
            />
            {query && (
              <button className="quiet" onClick={() => setQuery("")}>
                清除
              </button>
            )}
            <span className="muted">
              {filteredRules.length === rules.length
                ? `共 ${rules.length} 条`
                : `匹配 ${filteredRules.length} / ${rules.length} 条`}
            </span>
          </div>

          {loading ? (
            <p className="info">正在加载规则列表…</p>
          ) : filteredRules.length === 0 ? (
            <p className="empty">
              {rules.length === 0 ? "当前没有配置规则。" : "没有找到匹配的规则。"}
            </p>
          ) : (
            <div className="rules-table-scroll">
              <table className="rules-table">
                <thead>
                  <tr>
                    <th style={{ width: "60px" }}>#</th>
                    <th style={{ width: "160px" }}>类型</th>
                    <th>匹配模式 / Payload</th>
                    <th style={{ width: "180px" }}>目标策略</th>
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
