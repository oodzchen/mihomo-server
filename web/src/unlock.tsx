import { useEffect, useLayoutEffect, useState, useSyncExternalStore } from "react";
import { ApiError, command } from "./api";
import { describe } from "./format";
import { t, type Language, type MessageKey } from "./i18n";
import { useToast } from "./toast";
import type { CoreStatus, Profiles, Proxies } from "./types";

type Category = "location" | "streaming" | "other";
type UnlockService = { id: string; name: string; category: Category };
type Verdict = "yes" | "no" | "error";
type Note = "region_unsupported" | "hosting";
type Outcome = {
  id: string;
  verdict: Verdict;
  region: string | null;
  note: Note | null;
  ip: string | null;
  detail: string | null;
  error: string | null;
  elapsed: number;
};
type Catalog = { services: UnlockService[]; exit: { node: string; via: string[] } | null };

/** Tests in flight at once: each is a few small requests through the node. */
const PARALLEL = 6;
/** Entries that are not a node to send requests through. */
const NOT_NODES = new Set([
  "Selector", "URLTest", "Fallback", "LoadBalance", "Relay", "Reject", "RejectDrop", "Compatible", "Pass", "PassRule",
]);

const categories: [Category, MessageKey, MessageKey][] = [
  ["location", "unlockLocationTitle", "unlockLocationHelp"],
  ["streaming", "unlockStreamingTitle", "unlockStreamingHelp"],
  ["other", "unlockOtherTitle", "unlockOtherHelp"],
];

const noteKeys: Record<Note, MessageKey> = {
  region_unsupported: "unlockNoteRegionUnsupported",
  hosting: "unlockNoteHosting",
};

// Results and the run in progress outlive the page, so leaving and coming
// back neither loses them nor starts the tests again.
type State = { results: Record<string, Outcome>; queued: Set<string>; testing: Set<string> };
let state: State = { results: {}, queued: new Set(), testing: new Set() };
const listeners = new Set<() => void>();
let queue: { id: string; node: string }[] = [];
let active = 0;
let runToken = "";
let onUnauthorized: (() => void) | undefined;
/** The last catalog read, shown at once when the page is opened again. */
let lastCatalog: Catalog | undefined;
/** The node chosen on the page; null follows the current exit. */
let chosenNode: string | null = null;
/** The subscription, configuration and node the results were taken with;
 * tests started before they changed are discarded on arrival. */
let routingKey: string | undefined;
let epoch = 0;

function update(change: (current: State) => Partial<State>) {
  state = { ...state, ...change(state) };
  listeners.forEach(listener => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** Results describe one routing; forget them once it changes. */
function adoptRouting(key: string) {
  if (routingKey === key) return;
  const first = routingKey === undefined;
  routingKey = key;
  if (first) return;
  epoch++;
  queue = [];
  update(() => ({ results: {}, queued: new Set(), testing: new Set() }));
}

function enqueue(token: string, ids: string[], node: string) {
  runToken = token;
  const fresh = ids.filter(id => !state.queued.has(id) && !state.testing.has(id));
  if (!fresh.length) return;
  queue.push(...fresh.map(id => ({ id, node })));
  update(current => ({ queued: new Set([...current.queued, ...fresh]) }));
  pump();
}

function pump() {
  while (active < PARALLEL && queue.length) {
    const { id, node } = queue.shift()!;
    const started = epoch;
    active++;
    update(current => {
      const queued = new Set(current.queued);
      queued.delete(id);
      return { queued, testing: new Set([...current.testing, id]) };
    });
    command<Outcome>(runToken, "unlock_test", { id, node })
      .catch((error): Outcome => {
        if (error instanceof ApiError && error.status === 401) {
          queue = [];
          update(() => ({ queued: new Set() }));
          onUnauthorized?.();
        }
        return { id, verdict: "error", region: null, note: null, ip: null, detail: null, error: describe(error), elapsed: 0 };
      })
      .then(outcome => {
        active--;
        if (started === epoch) update(current => {
          const testing = new Set(current.testing);
          testing.delete(id);
          return { testing, results: { ...current.results, [id]: outcome } };
        });
        pump();
      });
  }
}

/** Regional-indicator flag and localized name of a two-letter region code. */
function regionLabel(language: Language, code: string) {
  const flag = String.fromCodePoint(...[...code].map(letter => 0x1f1a5 + letter.charCodeAt(0)));
  let name = code;
  try {
    name = new Intl.DisplayNames([language === "en" ? "en" : language === "zhtw" ? "zh-TW" : "zh-CN"], { type: "region" }).of(code) ?? code;
  } catch { /* Unknown to this browser: the code alone. */ }
  return { flag, name: name === code ? code : `${name} ${code}` };
}

export function UnlockPage({
  token,
  language,
  status,
  profiles,
  logout,
}: {
  token: string;
  language: Language;
  status: CoreStatus;
  profiles: Profiles;
  logout: (reason?: string) => void;
}) {
  const notify = useToast();
  const [catalog, setCatalog] = useState(lastCatalog);
  const [nodes, setNodes] = useState<string[]>();
  const [chosen, setChosen] = useState(chosenNode);
  const current = useSyncExternalStore(subscribe, () => state);
  const running = status.phase === "running";
  // The current exit follows the node selections (saved per subscription).
  const selected = profiles.items?.find(item => item.uid === status.active_profile)?.selected;
  const selections = JSON.stringify(selected ?? []);
  const node = chosen ?? catalog?.exit?.node;
  // Results describe one node of one configuration. Before paint, so stale
  // results never show.
  const routing = node === undefined
    ? undefined
    : JSON.stringify([status.active_profile ?? null, status.config_revision ?? null, node]);
  useLayoutEffect(() => {
    if (routing !== undefined) adoptRouting(routing);
  }, [routing]);
  const choose = (value: string | null) => { chosenNode = value; setChosen(value); };
  useEffect(() => {
    onUnauthorized = () => logout(t(language, "expiredToken"));
    return () => { onUnauthorized = undefined; };
  }, [logout, language]);
  useEffect(() => {
    const controller = new AbortController();
    command<Catalog>(token, "unlock_services", {}, controller.signal)
      .then(value => { lastCatalog = value; setCatalog(value); })
      .catch(error => {
        if (controller.signal.aborted) return;
        if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
        else notify(describe(error), "error");
      });
    return () => controller.abort();
    // The current exit changes with the configuration and node selections.
  }, [token, status.phase, status.generation, status.config_revision, selections, logout, language, notify]);
  useEffect(() => {
    if (!running) {
      setNodes(undefined);
      return;
    }
    const controller = new AbortController();
    command<Proxies>(token, "proxies", {}, controller.signal)
      .then(value => {
        const entries = value.proxies;
        const order = entries.GLOBAL?.all ?? Object.keys(entries);
        const names = order.filter(name => entries[name] && !NOT_NODES.has(entries[name].type));
        setNodes(names);
        // A node the new configuration lacks: back to the current exit.
        if (chosenNode !== null && !names.includes(chosenNode)) choose(null);
      })
      .catch(error => {
        if (controller.signal.aborted) return;
        if (error instanceof ApiError && error.status === 401) logout(t(language, "expiredToken"));
        else notify(describe(error), "error");
      });
    return () => controller.abort();
  }, [token, running, status.generation, status.config_revision, logout, language, notify]);

  const services = catalog?.services ?? [];
  const pending = current.queued.size + current.testing.size;
  const ready = running && !!catalog && node !== undefined;
  const run = (ids: string[]) => { if (ready) enqueue(token, ids, node); };
  const exitPath = catalog?.exit ? [...catalog.exit.via, catalog.exit.node].join(" → ") : "";

  function statusCell(service: UnlockService) {
    if (current.testing.has(service.id)) return <span className="unlock-status unlock-testing">{t(language, "unlockTesting")}</span>;
    if (current.queued.has(service.id)) return <span className="unlock-status unlock-queued">{t(language, "unlockQueued")}</span>;
    const outcome = current.results[service.id];
    if (!outcome) return <span className="unlock-status unlock-untested">{t(language, "unlockUntested")}</span>;
    const key: MessageKey = {
      yes: service.category === "streaming" ? "unlockYes" : service.category === "location" && outcome.region ? "unlockLocated" : "unlockDetected",
      no: "unlockNo",
      error: "unlockError",
    }[outcome.verdict] as MessageKey;
    return <span className={`unlock-status unlock-${outcome.verdict}`}>{t(language, key)}</span>;
  }

  function regionCell(outcome?: Outcome) {
    if (!outcome?.region) return <span className="muted">—</span>;
    const { flag, name } = regionLabel(language, outcome.region);
    return <span className="unlock-region" title={name}><span aria-hidden="true">{flag}</span> {name}</span>;
  }

  function detailCell(outcome?: Outcome) {
    if (!outcome) return null;
    if (outcome.error) return <span className="unlock-error-text" title={outcome.error}>{outcome.error}</span>;
    const parts = [outcome.note ? t(language, noteKeys[outcome.note]) : "", outcome.detail ?? ""].filter(Boolean);
    return parts.length ? <span title={parts.join(" · ")}>{parts.join(" · ")}</span> : null;
  }

  return (
    <>
      <div className="section-title unlock-header">
        <div>
          <p className="muted">{t(language, "unlockHelp")}</p>
          <div className="unlock-node">
            <label htmlFor="unlock-node">{t(language, "unlockNode")}</label>
            <select
              id="unlock-node"
              value={chosen ?? ""}
              disabled={!running || !catalog || !nodes || pending > 0}
              onChange={event => choose(event.target.value || null)}
            >
              <option value="">
                {catalog?.exit ? t(language, "unlockFollowExit", { path: exitPath }) : t(language, "unlockNoExit")}
              </option>
              {(nodes ?? []).map(name => <option key={name} value={name}>{name}</option>)}
            </select>
          </div>
          <p className="muted" aria-live="polite">
            {!running
              ? t(language, "unlockNeedsCore")
              : !catalog
                ? t(language, "unlockLoading")
                : node === undefined
                  ? t(language, "unlockNoExit")
                  : t(language, "unlockIsolated")}
          </p>
        </div>
        <button
          type="button"
          className="primary unlock-run-all"
          disabled={!ready || pending > 0}
          onClick={() => run(services.map(service => service.id))}
        >
          {pending > 0 ? t(language, "unlockRunning", { count: pending }) : t(language, "unlockRunAll")}
        </button>
      </div>
      {categories.map(([category, title, help]) => {
        const members = services.filter(service => service.category === category);
        if (!members.length) return null;
        const tested = members.filter(service => current.results[service.id]);
        const served = tested.filter(service => current.results[service.id].verdict === "yes");
        const busy = members.some(service => current.queued.has(service.id) || current.testing.has(service.id));
        return (
          <section className="panel unlock-panel" key={category} aria-labelledby={`unlock-${category}`}>
            <div className="panel-title">
              <div>
                <h2 id={`unlock-${category}`}>{t(language, title)}</h2>
                <p className="muted">{t(language, help)}</p>
              </div>
              <div className="panel-actions">
                <span className="unlock-summary muted">
                  {category === "location"
                    ? t(language, "unlockLocatedSummary", { done: served.filter(service => current.results[service.id].region).length, total: members.length })
                    : t(language, category === "other" ? "unlockDetectedSummary" : "unlockSummary", { done: served.length, total: members.length })}
                </span>
                <button type="button" disabled={!ready || busy} onClick={() => run(members.map(service => service.id))}>
                  {t(language, "unlockRunGroup")}
                </button>
              </div>
            </div>
            <div className="unlock-table-scroll">
              <table className="unlock-table">
                <colgroup>
                  <col className="unlock-col-service" />
                  <col className="unlock-col-status" />
                  <col className="unlock-col-region" />
                  <col className="unlock-col-ip" />
                  <col />
                  <col className="unlock-col-time" />
                  <col className="unlock-col-action" />
                </colgroup>
                <thead>
                  <tr>
                    <th>{t(language, "unlockColService")}</th>
                    <th>{t(language, "unlockColStatus")}</th>
                    <th>{t(language, "unlockColRegion")}</th>
                    <th>{t(language, "unlockColIp")}</th>
                    <th>{t(language, "unlockColDetail")}</th>
                    <th>{t(language, "unlockColTime")}</th>
                    <th><span className="visually-hidden">{t(language, "unlockColAction")}</span></th>
                  </tr>
                </thead>
                <tbody>
                  {members.map(service => {
                    const outcome = current.results[service.id];
                    const testing = current.queued.has(service.id) || current.testing.has(service.id);
                    return (
                      <tr key={service.id}>
                        <td className="unlock-name">{service.name}</td>
                        <td>{statusCell(service)}</td>
                        <td>{regionCell(outcome)}</td>
                        <td className="mono unlock-ip" title={outcome?.ip ?? undefined}>{outcome?.ip ?? ""}</td>
                        <td className="unlock-detail">{detailCell(outcome)}</td>
                        <td className="unlock-time">{outcome && !testing && outcome.elapsed ? `${outcome.elapsed}ms` : ""}</td>
                        <td>
                          <button
                            type="button"
                            className="unlock-retest"
                            disabled={!ready || testing}
                            aria-label={t(language, "unlockRetest", { name: service.name })}
                            title={t(language, "unlockRetest", { name: service.name })}
                            onClick={() => run([service.id])}
                          >
                            ↻
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </section>
        );
      })}
    </>
  );
}
