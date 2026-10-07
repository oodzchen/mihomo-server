import { useEffect, useState } from "react";
import { command, type Perform } from "./api";
import { t, type Language } from "./i18n";
import type { CoreStatus, NodeProbe, Proxies } from "./types";
import { describe } from "./format";

export function ProxyPage({
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
  const [proxies, setProxies] = useState<Proxies>(),
    [probes, setProbes] = useState<Record<string, NodeProbe>>({}),
    [testingGroup, setTestingGroup] = useState<string | null>(null),
    [testingNode, setTestingNode] = useState<string | null>(null),
    [testUrl, setTestUrl] = useState("https://www.gstatic.com/generate_204"),
    [error, setError] = useState(""),
    [revision, refresh] = useState(0),
    [loading, setLoading] = useState(false);

  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() => {
    try {
      return JSON.parse(localStorage.getItem("mhs-collapsed-groups") || "{}");
    } catch {
      return {};
    }
  });

  const toggleGroup = (name: string, defaultCollapsed: boolean) => {
    setCollapsed((prev) => {
      const current = name in prev ? !!prev[name] : defaultCollapsed;
      const next = { ...prev, [name]: !current };
      try {
        localStorage.setItem("mhs-collapsed-groups", JSON.stringify(next));
      } catch {}
      return next;
    });
  };

  useEffect(() => {
    if (status.phase !== "running") {
      setProxies(undefined);
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    setError("");
    command<Proxies>(token, "proxies", {}, controller.signal)
      .then((proxiesData) => {
        if (!controller.signal.aborted) {
          setProxies(proxiesData);
          setLoading(false);
        }
      })
      .catch((error) => {
        if (!controller.signal.aborted) {
          setError(describe(error));
          setLoading(false);
        }
      });
    return () => controller.abort();
  }, [
    token,
    status.phase,
    status.generation,
    status.config_revision,
    status.active_profile,
    status.selection_pending?.join("\0"),
    revision,
  ]);

  const isDefaultProxies = (name: string) =>
    name.toLowerCase() === "proxies" || name.toLowerCase() === "proxy";

  const rawGroups = Object.entries(proxies?.proxies || {}).filter(([, group]) =>
    ["Selector", "URLTest", "Fallback", "LoadBalance"].includes(group.type),
  );

  const groups = [...rawGroups].sort(([a], [b]) => {
    const aDefault = isDefaultProxies(a);
    const bDefault = isDefaultProxies(b);
    if (aDefault && !bDefault) return -1;
    if (!aDefault && bDefault) return 1;
    if (a.toLowerCase() === "global") return 1;
    if (b.toLowerCase() === "global") return -1;
    return a.localeCompare(b);
  });

  if (!groups.some(([name]) => isDefaultProxies(name))) {
    const standaloneNodes = Object.entries(proxies?.proxies || {})
      .filter(
        ([, p]) =>
          ![
            "Selector",
            "URLTest",
            "Fallback",
            "LoadBalance",
            "Direct",
            "Reject",
            "RejectDrop",
            "Compatible",
            "Pass",
            "PassRule",
          ].includes(p.type),
      )
      .map(([name]) => name);

    if (standaloneNodes.length > 0) {
      groups.unshift([
        "Proxies",
        {
          type: "Selector",
          now: proxies?.proxies["GLOBAL"]?.now || standaloneNodes[0],
          all: standaloneNodes,
          history: [],
        } as any,
      ]);
    }
  }

  async function select(name: string, fields: Record<string, unknown>) {
    await perform(name, fields);
    refresh((value) => value + 1);
  }

  /** Cold and warm full requests through each node, in an isolated core. */
  async function probe(names: string[]) {
    const results = await command<Record<string, NodeProbe>>(token, "probe_proxies", {
      names,
      url: testUrl,
      // A node that needs longer for a first request is unusable in practice.
      timeout: 3000,
    });
    setProbes((prev) => ({ ...prev, ...results }));
  }

  async function testGroupDelay(group: string, nodes: string[]) {
    if (testingGroup || busy || !nodes.length) return;
    setTestingGroup(group);
    setError("");
    try {
      await probe(nodes);
    } catch (e) {
      setError(describe(e));
    } finally {
      setTestingGroup(null);
    }
  }

  async function testNodeDelay(node: string) {
    if (testingNode || busy) return;
    setTestingNode(node);
    setError("");
    try {
      await probe([node]);
    } catch (e) {
      setError(describe(e));
    } finally {
      setTestingNode(null);
    }
  }

  function renderDelayBadge(node: string, isTesting: boolean) {
    if (isTesting) {
      return <span className="delay-badge delay-testing">{t(language, "proxyDelayTesting")}</span>;
    }
    const result = probes[node];
    if (result) {
      if (!result.cold) {
        return <span className="delay-badge delay-timeout">{t(language, "proxyDelayTimeout")}</span>;
      }
      const title = t(language, "proxyProbeTitle", {
        cold: result.cold,
        warm: result.warm || t(language, "proxyDelayTimeout"),
      });
      const speed = (value: number, fast: number) =>
        value < fast ? 0 : value < fast * 2 ? 1 : 2;
      const level = Math.max(speed(result.cold, 500), result.warm ? speed(result.warm, 250) : 2);
      return (
        <span className={`delay-badge ${["delay-fast", "delay-medium", "delay-slow"][level]}`} title={title}>
          {result.cold} / {result.warm ? `${result.warm}ms` : t(language, "proxyDelayTimeout")}
        </span>
      );
    }
    // The running core's own health checks (URL-test groups), until tested here.
    const history = proxies?.proxies[node]?.history;
    const delay = history?.length ? history[history.length - 1].delay : undefined;
    if (delay === undefined || delay < 0) {
      return <span className="delay-badge delay-untested">{t(language, "proxyDelayUntested")}</span>;
    }
    if (delay === 0 || delay >= 10000) {
      return <span className="delay-badge delay-timeout">{t(language, "proxyDelayTimeout")}</span>;
    }
    return <span className="delay-badge delay-fast">{delay}ms</span>;
  }

  return (
    <>
      <div className="section-title">
        <div>
          <p className="muted">
            {t(language, "proxyHelp")}
          </p>
          <p className="muted">{t(language, "proxyProbeHelp")}</p>
          <div className="delay-url-bar">
            <label htmlFor="delay-test-url" className="muted" style={{ fontSize: "12px", marginRight: "6px" }}>
              {t(language, "proxyDelayUrl")}
            </label>
            <input
              id="delay-test-url"
              type="text"
              value={testUrl}
              onChange={(e) => setTestUrl(e.target.value)}
              placeholder={t(language, "proxyDelayUrlPlaceholder")}
              style={{ width: "320px", display: "inline-block", padding: "4px 8px", fontSize: "12px" }}
            />
          </div>
        </div>
        <button
          disabled={busy || loading || status.phase !== "running"}
          onClick={() => refresh((value) => value + 1)}
        >
          {t(language, "proxyRefresh")}
        </button>
      </div>
      {status.phase !== "running" && (
        <p className="info">{t(language, "proxyNeedsCore")}</p>
      )}
      {!status.active_profile && (
        <p className="info">{t(language, "proxyNeedsProfile")}</p>
      )}
      {error && (
        <p className="alert" role="alert">
          {error}
        </p>
      )}
      {loading && <p className="info">{t(language, "proxyLoading")}</p>}

      {groups.map(([name, group], index) => {
        const hasDefaultProxies = groups.some(([gName]) => isDefaultProxies(gName));
        const defaultCollapsed = hasDefaultProxies ? !isDefaultProxies(name) : index !== 0;
        const isCollapsed = name in collapsed ? !!collapsed[name] : defaultCollapsed;
        const currentSelection = group.fixed || group.now || t(language, "proxyGroupWaiting");
        return (
          <section className="panel" key={name}>
            <div
              className={`panel-title group-header ${isCollapsed ? "collapsed" : ""}`}
              onClick={() => toggleGroup(name, defaultCollapsed)}
            >
              <button
                type="button"
                className="group-title-btn"
                onClick={(e) => {
                  e.stopPropagation();
                  toggleGroup(name, defaultCollapsed);
                }}
                aria-expanded={!isCollapsed}
                aria-label={t(language, isCollapsed ? "groupExpand" : "groupCollapse", { name })}
              >
                <span className={`group-toggle-icon ${isCollapsed ? "" : "expanded"}`}>▶</span>
                <div>
                  <h2>{name}</h2>
                  <p className="muted">
                    {group.type} · {t(language, "proxyCurrent")}
                    <strong style={{ color: "#111827", marginLeft: "4px" }}>
                      {currentSelection}
                    </strong>
                  </p>
                </div>
              </button>
              <div className="panel-actions" onClick={(e) => e.stopPropagation()}>
                <button
                  type="button"
                  disabled={busy || loading || testingGroup === name}
                  onClick={() => void testGroupDelay(name, group.all ?? [])}
                >
                  {t(language, testingGroup === name ? "proxyDelayWorking" : "proxyDelayAction")}
                </button>
                {group.type !== "Selector" && (
                  <button
                    disabled={busy || !status.active_profile}
                    onClick={() => void select("unfix_node", { group: name })}
                  >
                    {t(language, "proxyUnfix")}
                  </button>
                )}
              </div>
            </div>
            {!isCollapsed && (
              <>
                <div className="nodes">
                  {group.all?.map((node) => {
                    const isSelected = (group.fixed || group.now) === node;
                    const isTesting = testingNode === node || testingGroup === name;
                    return (
                      <button
                        key={node}
                        aria-label={`${t(language, "proxySelect")} ${name} / ${node}`}
                        aria-pressed={isSelected}
                        className={isSelected ? "selected" : ""}
                        disabled={busy || !status.active_profile}
                        onClick={() =>
                          void select("select_node", { group: name, node })
                        }
                      >
                        <span className="node-name" style={{ fontWeight: isSelected ? 600 : 400 }}>
                          {node}
                        </span>
                        <div className="node-meta">
                          {renderDelayBadge(node, isTesting)}
                          <span
                            className="node-test-btn"
                            title={t(language, "proxyDelayNodeTitle").replace("{node}", node)}
                            onClick={(e) => {
                              e.stopPropagation();
                              void testNodeDelay(node);
                            }}
                          >
                            ⚡
                          </span>
                          <span className="node-select-text">
                            {t(language, isSelected ? "proxySelected" : "proxySelect")}
                          </span>
                        </div>
                      </button>
                    );
                  })}
                </div>
                {!group.all?.length && (
                  <p className="empty">{t(language, "proxyGroupEmpty")}</p>
                )}
              </>
            )}
          </section>
        );
      })}
    </>
  );
}
