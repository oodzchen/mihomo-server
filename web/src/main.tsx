import { createRoot } from "react-dom/client";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type MouseEvent,
} from "react";
import { ApiError, command, subscribe, type Perform } from "./api";
import { SettingsPage } from "./settings";
import { ProxyAccessPanel } from "./proxy-access";
import { RawEditor } from "./raw-editor";
import { CoreUpgradePage } from "./core-upgrade";
import { RulesPage } from "./rules";
import { connectionLabel, phaseLabel, resolveLanguage, savedLanguage, saveLanguage, t, type Language, type MessageKey } from "./i18n";
import type {
  CoreLog,
  CoreStatus,
  EventMessage,
  Profile,
  Profiles,
  Proxies,
  ProxyDelay,
} from "./types";
import "./style.css";

const pages: [string, MessageKey][] = [
  ["/", "overview"],
  ["/proxies", "proxies"],
  ["/profiles", "profiles"],
  ["/config", "config"],
  ["/rules", "rules"],
  ["/logs", "logs"],
  ["/settings", "settings"],
  ["/core", "core"],
];
const describe = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

function App() {
  const [language, setLanguage] = useState<Language>(savedLanguage);
  const [session, setSession] = useState<{
    token: string;
    status: CoreStatus;
  }>();
  const [loginError, setLoginError] = useState("");
  const logout = useCallback((reason = "") => {
    setSession(undefined);
    setLoginError(reason);
  }, []);
  const changeLanguage = useCallback((value: string) => {
    const next = resolveLanguage(value);
    saveLanguage(next);
    setLanguage(next);
    setLoginError("");
  }, []);
  useEffect(() => {
    document.documentElement.lang = language === "en" ? "en" : language === "zhtw" ? "zh-TW" : "zh-CN";
    document.title = `Mihomo · ${t(language, "serviceManagement")}`;
  }, [language]);
  return session ? (
    <Manager token={session.token} initial={session.status} logout={logout} language={language} changeLanguage={changeLanguage} />
  ) : (
    <Login error={loginError} login={setSession} language={language} changeLanguage={changeLanguage} />
  );
}

function LanguagePicker({ language, changeLanguage }: { language: Language; changeLanguage: (value: string) => void }) {
  return <label className="language-picker">{t(language, "language")}
    <select aria-label={t(language, "language")} value={language} onChange={event => changeLanguage(event.target.value)}>
      <option value="zh">简体中文</option><option value="zhtw">繁體中文</option><option value="en">English</option>
    </select>
  </label>;
}

function Login({
  error,
  login,
  language,
  changeLanguage,
}: {
  error: string;
  login: (session: { token: string; status: CoreStatus }) => void;
  language: Language;
  changeLanguage: (value: string) => void;
}) {
  const [token, setToken] = useState(""),
    [busy, setBusy] = useState(false),
    [failure, setFailure] = useState("");
  useEffect(() => setFailure(""), [language]);
  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setFailure("");
    try {
      const value = token.trim();
      const status = await command<CoreStatus>(value, "status");
      setToken("");
      login({ token: value, status });
    } catch (error) {
      setFailure(
        error instanceof ApiError && error.status === 401
          ? t(language, "invalidToken")
          : describe(error),
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <main className="login">
      <div className="login-art">
        <div className="brand">
          MHS<span>MIHOMO SERVER</span>
        </div>
        <div>
          <p className="eyebrow">{t(language, "loginEyebrow")}</p>
          <h1>
            {t(language, "loginTitleFirst")}
            <br />
            {t(language, "loginTitleSecond")}
          </h1>
          <p>
            {t(language, "loginIntroFirst")}
            <br />
            {t(language, "loginIntroSecond")}
          </p>
        </div>
        <span className="login-foot">Mihomo / Headless</span>
      </div>
      <section className="login-form">
        <LanguagePicker language={language} changeLanguage={changeLanguage} />
        <p className="eyebrow">{t(language, "loginEntry")}</p>
        <h2>{t(language, "loginTitle")}</h2>
        <p className="muted">
          {t(language, "loginHelp")}
        </p>
        <form onSubmit={submit}>
          <label>
            {t(language, "token")}
            <input
              type="password"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              required
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          {(failure || error) && (
            <p className="alert" role="alert">
              {failure || error}
            </p>
          )}
          <button className="primary" disabled={busy}>
            {busy ? t(language, "verifying") : t(language, "connect")}
          </button>
        </form>
        <p className="hint">{t(language, "tokenHint")}</p>
      </section>
    </main>
  );
}

function Manager({
  token,
  initial,
  logout,
  language,
  changeLanguage,
}: {
  token: string;
  initial: CoreStatus;
  logout: (reason?: string) => void;
  language: Language;
  changeLanguage: (value: string) => void;
}) {
  const [status, setStatus] = useState(initial),
    [profiles, setProfiles] = useState<Profiles>({}),
    [logs, setLogs] = useState<CoreLog[]>([]);
  const [route, setRoute] = useState(location.pathname),
    [connection, setConnection] = useState("连接中");
  const [busy, setBusy] = useState(false),
    [failure, setFailure] = useState(""),
    [notice, setNotice] = useState("");
  const alive = useRef(true),
    pending = useRef(new Set<AbortController>()),
    locked = useRef(false);
  const languageRef = useRef(language);
  languageRef.current = language;
  const request = useCallback(
    async <T,>(name: string, fields: Record<string, unknown> = {}) => {
      const controller = new AbortController();
      pending.current.add(controller);
      try {
        return await command<T>(token, name, fields, controller.signal);
      } finally {
        pending.current.delete(controller);
      }
    },
    [token],
  );
  useEffect(() => {
    alive.current = true;
    const cancel = subscribe(
      token,
      "/api/events",
      (event: EventMessage) => {
        if (event.type === "snapshot") {
          setStatus(event.status!);
          setProfiles(event.profiles!);
          setLogs(event.logs || []);
        }
        if (event.type === "status") setStatus(event.data as CoreStatus);
        if (event.type === "profiles") setProfiles(event.data as Profiles);
        if (event.type === "log")
          setLogs((previous) =>
            [...previous, event.data as CoreLog].slice(-200),
          );
        if (event.type === "logs_reset") setLogs(event.data as CoreLog[]);
      },
      (value) => {
        setConnection(value);
        if (value === "认证失败") logout(t(languageRef.current, "expiredToken"));
      },
    );
    const popstate = () => setRoute(location.pathname);
    window.addEventListener("popstate", popstate);
    return () => {
      alive.current = false;
      cancel();
      pending.current.forEach((controller) => controller.abort());
      window.removeEventListener("popstate", popstate);
    };
  }, [token, logout]);
  const perform: Perform = async <T,>(
    name: string,
    fields: Record<string, unknown> = {},
  ) => {
    if (locked.current) return;
    locked.current = true;
    setBusy(true);
    setFailure("");
    setNotice("");
    try {
      const result = await request<T>(name, fields);
      const [current, catalog] = await Promise.all([
        request<CoreStatus>("status"),
        request<Profiles>("profiles"),
      ]);
      if (alive.current) {
        setStatus(current);
        setProfiles(catalog);
        setNotice(t(language, "completed"));
      }
      return result;
    } catch (error) {
      if (alive.current) {
        if (error instanceof ApiError && error.status === 401)
          logout(t(language, "expiredToken"));
        else setFailure(describe(error));
      }
      return undefined;
    } finally {
      locked.current = false;
      if (alive.current) setBusy(false);
    }
  };
  function navigate(event: MouseEvent, path: string) {
    if (
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey ||
      event.button !== 0
    )
      return;
    event.preventDefault();
    history.pushState(null, "", path);
    setRoute(path);
    setFailure("");
    setNotice("");
  }
  const active = profiles.items?.find(
    (item) => item.uid === status.active_profile,
  );
  const title = t(language, pages.find(([path]) => route === path)?.[1] || "overview");
  const transitional = [
    "starting",
    "stopping",
    "recovering",
    "shutdown",
  ].includes(status.phase);
  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          MHS
          <span>
            MIHOMO
            <br />
            SERVER
          </span>
        </div>
        <p className="nav-label">{t(language, "navLabel")}</p>
        <nav aria-label={t(language, "navAria")}>
          {pages.map(([path, key]) => (
            <a
              key={path}
              href={path}
              aria-current={route === path ? "page" : undefined}
              onClick={(event) => navigate(event, path)}
            >
              {t(language, key)}
            </a>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <span className={`dot ${connection === "已连接" ? "good" : ""}`} />
          <span role="status" aria-label={t(language, "eventConnection")}>
            {connectionLabel(language, connection)}
          </span>
          <LanguagePicker language={language} changeLanguage={changeLanguage} />
          <button className="quiet" onClick={() => logout()}>
            {t(language, "logout")}
          </button>
        </div>
      </aside>
      <div className="workspace">
        <header>
          <div>
            <p className="eyebrow">MIHOMO / {t(language, "serviceManagement")}</p>
            <h1>{title}</h1>
          </div>
          <span className={`badge ${status.phase === "running" ? "good" : ""}`}>
            {phaseLabel(language, status.phase)}
          </span>
        </header>
        <section className="core-strip" aria-label={t(language, "coreStatus")}>
          <div>
            <strong>{status.version || t(language, "coreName")}</strong>
            <p>
              {active?.name ||
                (status.active_profile ? status.active_profile : t(language, "noProfile"))}
              <span className="separator">/</span>
              {status.pid ? `PID ${status.pid}` : t(language, "coreStopped")}
            </p>
          </div>
          <div className="actions">
            <button
              disabled={busy || transitional || status.phase === "running"}
              onClick={() => void perform("start")}
            >
              {t(language, "startCore")}
            </button>
            <button
              disabled={
                busy || !["running", "recovering"].includes(status.phase)
              }
              onClick={() => void perform("stop")}
            >
              {t(language, "stopCore")}
            </button>
            <button
              disabled={busy || transitional}
              onClick={() => void perform("restart")}
            >
              {t(language, "restartCore")}
            </button>
          </div>
        </section>
        <div className="feedback" aria-live="polite">
          {busy && <p className="info">{t(language, "working")}</p>}
          {failure && (
            <p className="alert" role="alert">
              {failure}
            </p>
          )}
          {notice && (
            <p className="success" role="status">
              {notice}
            </p>
          )}
          {status.error && (
            <p className="alert">
              {status.phase === "failed" ? t(language, "coreError") : t(language, "operationError")}：
              {status.error}
            </p>
          )}
          {status.selection_error && (
            <p className="alert">{t(language, "selectionRestore")}：{status.selection_error}</p>
          )}
          {status.selection_pending?.length > 0 && (
            <p className="info">
              {t(language, "restoringNodes")}：{status.selection_pending.join("、")}
            </p>
          )}
        </div>
        {route === "/profiles" ? (
          <ProfilePage
            token={token}
            language={language}
            logout={logout}
            profiles={profiles}
            status={status}
            busy={busy}
            perform={perform}
          />
        ) : route === "/config" ? (
          <ConfigPage token={token} language={language} busy={busy} perform={perform} />
        ) : route === "/proxies" ? (
          <ProxyPage
            token={token}
            language={language}
            status={status}
            busy={busy}
            perform={perform}
          />
        ) : route === "/rules" ? (
          <RulesPage
            token={token}
            language={language}
            status={status}
            busy={busy}
            perform={perform}
          />
        ) : route === "/logs" ? (
          <LogPage logs={logs} language={language} />
        ) : route === "/core" ? (
          <CoreUpgradePage
            token={token}
            language={language}
            status={status}
            connection={connection}
            busy={busy}
            perform={perform}
            logout={logout}
          />
        ) : route === "/settings" ? (
          <SettingsPage
            token={token}
            language={language}
            status={status}
            connection={connection}
            busy={busy}
            perform={perform}
            logout={logout}
          />
        ) : (
          <Overview
            language={language}
            token={token}
            connection={connection}
            logout={logout}
            status={status}
            active={active}
            navigate={navigate}
            logs={logs}
          />
        )}
        <footer>{t(language, "footer")}</footer>
      </div>
    </div>
  );
}

function useFeed<T>(token: string, feed: string) {
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
          if (state !== "已连接") setValue(undefined);
        },
      ),
    [token, feed],
  );
  return value;
}
const bytes = (value?: number) =>
  value === undefined
    ? "—"
    : value >= 1024 ** 2
      ? `${(value / 1024 ** 2).toFixed(1)} MB`
      : `${(value / 1024).toFixed(1)} KB`;
function Overview({
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
  connection: string;
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

function ProfileEditor({
  item,
  language,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
  language: Language;
  busy: boolean;
  perform: Perform;
  onClose: () => void;
}) {
  const [name, setName] = useState(item.name || "");
  const [desc, setDesc] = useState(item.desc || "");
  const [url, setUrl] = useState(item.url || "");
  const [agent, setAgent] = useState(item.option?.user_agent || "");
  const [seconds, setSeconds] = useState(
    String(item.option?.timeout_seconds ?? 20),
  );
  const [interval, setInterval] = useState(
    String(item.option?.update_interval ?? 0),
  );
  const [auto, setAuto] = useState(item.option?.allow_auto_update ?? true);
  const [selfProxy, setSelfProxy] = useState(item.option?.self_proxy ?? false);
  const [withProxy, setWithProxy] = useState(item.option?.with_proxy ?? false);
  const [invalidCerts, setInvalidCerts] = useState(
    item.option?.danger_accept_invalid_certs ?? false,
  );
  async function save(event: FormEvent) {
    event.preventDefault();
    const patch: Record<string, unknown> = {};
    if (name !== (item.name || "")) patch.name = name;
    if (desc !== (item.desc || "")) patch.desc = desc;
    if (item.type === "remote") {
      if (url !== (item.url || "")) patch.url = url;
      const options: Record<string, unknown> = {};
      if (agent !== (item.option?.user_agent || "")) options.user_agent = agent;
      if (seconds !== String(item.option?.timeout_seconds ?? 20))
        options.timeout_seconds = Number(seconds);
      if (interval !== String(item.option?.update_interval ?? 0))
        options.update_interval = Number(interval);
      if (auto !== (item.option?.allow_auto_update ?? true))
        options.allow_auto_update = auto;
      if (selfProxy !== (item.option?.self_proxy ?? false))
        options.self_proxy = selfProxy;
      if (withProxy !== (item.option?.with_proxy ?? false))
        options.with_proxy = withProxy;
      if (invalidCerts !== (item.option?.danger_accept_invalid_certs ?? false))
        options.danger_accept_invalid_certs = invalidCerts;
      if (Object.keys(options).length) patch.options = options;
    }
    if (!Object.keys(patch).length) {
      onClose();
      return;
    }
    if (await perform<Profile>("edit_profile", { uid: item.uid, patch }))
      onClose();
  }
  return (
    <section className="panel">
      <h2>{t(language, "profileEditorTitle")}</h2>
      <p className="muted">{t(language, "profileEditorHelp")}</p>
      <form onSubmit={save}>
        <label>
          {t(language, "profileEditorName")}
          <input
            required
            maxLength={256}
            disabled={busy}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          {t(language, "profileEditorDescription")}
          <textarea
            maxLength={4096}
            disabled={busy}
            value={desc}
            onChange={(event) => setDesc(event.target.value)}
          />
        </label>
        {item.type === "remote" && (
          <>
            <label>
              {t(language, "profileEditorUrl")}
              <input
                type="url"
                required
                maxLength={8192}
                disabled={busy}
                value={url}
                onChange={(event) => setUrl(event.target.value)}
              />
            </label>
            <label>
              {t(language, "profileEditorAgent")}
              <input
                maxLength={1024}
                disabled={busy}
                value={agent}
                onChange={(event) => setAgent(event.target.value)}
              />
            </label>
            <label>
              {t(language, "profileEditorTimeout")}
              <input
                type="number"
                required
                min={1}
                max={120}
                step={1}
                disabled={busy}
                value={seconds}
                onChange={(event) => setSeconds(event.target.value)}
              />
            </label>
            <label>
              {t(language, "profileEditorInterval")}
              <input
                type="number"
                required
                min={0}
                max={Number.MAX_SAFE_INTEGER}
                step={1}
                disabled={busy}
                value={interval}
                onChange={(event) => setInterval(event.target.value)}
              />
            </label>
            <label className="check-label">
              <input
                type="checkbox"
                disabled={busy}
                checked={auto}
                onChange={(event) => setAuto(event.target.checked)}
              />
              {t(language, "profileEditorAuto")}
            </label>
            <label className="check-label">
              <input
                type="checkbox"
                disabled={busy}
                checked={selfProxy}
                onChange={(event) => setSelfProxy(event.target.checked)}
              />
              {t(language, "profileEditorManaged")}
            </label>
            <label className="check-label">
              <input
                type="checkbox"
                disabled={busy}
                checked={withProxy}
                onChange={(event) => setWithProxy(event.target.checked)}
              />
              {t(language, "profileEditorSystem")}
            </label>
            <label className="check-label">
              <input
                type="checkbox"
                disabled={busy}
                checked={invalidCerts}
                onChange={(event) => setInvalidCerts(event.target.checked)}
              />
              {t(language, "profileEditorInvalidCerts")}
            </label>
            <p className="muted">{t(language, "profileEditorTlsHelp")}</p>
            <p className="muted">{t(language, "profileEditorRouteHelp")}</p>
          </>
        )}
        <div className="form-actions">
          <button className="primary" disabled={busy}>
            {t(language, "profileEditorSave")}
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            {t(language, "profileEditorCancel")}
          </button>
        </div>
      </form>
    </section>
  );
}

function MergeEditor({
  item,
  language,
  content,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
  language: Language;
  content: { uid?: string; yaml?: string };
  busy: boolean;
  perform: Perform;
  onClose: () => void;
}) {
  const [yaml, setYaml] = useState(content.yaml ?? "{}\n");
  async function save(event: FormEvent) {
    event.preventDefault();
    if (await perform<Profile>("set_profile_merge", { uid: item.uid, yaml }))
      onClose();
  }
  async function clear() {
    if (await perform<Profile>("clear_profile_merge", { uid: item.uid }))
      onClose();
  }
  return (
    <section className="panel">
      <h2>{t(language, "mergeEditorTitle")}</h2>
      <p className="muted">{t(language, "mergeEditorHelp").replace("{name}", item.name || item.uid)}</p>
      <form onSubmit={save}>
        <label>
          {t(language, "mergeEditorYaml")}
          <textarea
            className="code"
            rows={12}
            disabled={busy}
            value={yaml}
            onChange={(event) => setYaml(event.target.value)}
            spellCheck={false}
            required
          />
        </label>
        <div className="form-actions">
          <button className="primary" disabled={busy}>
            {t(language, "mergeEditorSave")}
          </button>
          <button
            type="button"
            disabled={busy || !content.uid}
            onClick={() => void clear()}
          >
            {t(language, "mergeEditorRemove")}
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            {t(language, "mergeEditorCancel")}
          </button>
        </div>
      </form>
    </section>
  );
}

type SequenceKind = "rules" | "proxies" | "groups";

type GlobalKind = "merge" | "script";

function GlobalEnhancements({
  language,
  status,
  busy,
  perform,
}: {
  language: Language;
  status: CoreStatus;
  busy: boolean;
  perform: Perform;
}) {
  const [editing, setEditing] = useState<{
    kind: GlobalKind;
    content: string;
    loaded: boolean;
    version: number;
  } | null>(null);
  const version = useRef(0);
  async function open(kind: GlobalKind) {
    const result = await perform<{ yaml?: string; source?: string }>(
      `global_${kind}`,
    );
    const content = kind === "merge" ? result?.yaml : result?.source;
    setEditing({
      kind,
      content: content ?? "",
      loaded: typeof content === "string",
      version: ++version.current,
    });
  }
  return (
    <section className="panel global-enhancements" aria-label={t(language, "globalPanelTitle")}>
      <div className="panel-title">
        <h2>{t(language, "globalPanelTitle")}</h2>
        <div className="actions">
          <button
            disabled={busy || editing !== null}
            onClick={() => void open("merge")}
          >
            {t(language, "globalMergeOpen")}
          </button>
          <button
            disabled={busy || editing !== null}
            onClick={() => void open("script")}
          >
            {t(language, "globalScriptOpen")}
          </button>
        </div>
      </div>
      <p className="muted">{t(language, "globalPanelHelp")}</p>
      <p className="hint">
        {status.active_profile
          ? t(language, "globalPanelActive")
          : t(language, "globalPanelInactive")}
      </p>
      {editing && (
        <GlobalEditor
          key={editing.version}
          language={language}
          kind={editing.kind}
          initial={editing.content}
          loaded={editing.loaded}
          busy={busy}
          perform={perform}
          onRetry={() => void open(editing.kind)}
          onClose={() => setEditing(null)}
        />
      )}
    </section>
  );
}

function GlobalEditor({
  language,
  kind,
  initial,
  loaded,
  busy,
  perform,
  onRetry,
  onClose,
}: {
  language: Language;
  kind: GlobalKind;
  initial: string;
  loaded: boolean;
  busy: boolean;
  perform: Perform;
  onRetry: () => void;
  onClose: () => void;
}) {
  const [content, setContent] = useState(initial);
  const [resetting, setResetting] = useState(false);
  const [error, setError] = useState<"script-large" | "merge-large" | null>(null);
  const script = kind === "script";
  const title = t(language, script ? "globalScriptTitle" : "globalMergeTitle");
  async function save(event: FormEvent) {
    event.preventDefault();
    setError(null);
    const limit = (script ? 1 : 8) * 1024 ** 2;
    if (new TextEncoder().encode(content).length > limit) {
      setError(script ? "script-large" : "merge-large");
      return;
    }
    if (
      await perform<Profile>(
        `set_global_${kind}`,
        script ? { source: content } : { yaml: content },
      )
    )
      onClose();
  }
  async function reset() {
    setError(null);
    if (await perform<Profile>(`reset_global_${kind}`)) onClose();
  }
  return (
    <form className="global-editor" onSubmit={save} aria-label={title}>
      <h3>{title}</h3>
      <p className="muted">
        {script
          ? t(language, "globalScriptHelp")
          : t(language, "globalMergeHelp")}
      </p>
      {!loaded && (
        <p className="info">
          {t(language, "globalReadMissing")}
        </p>
      )}
      <label>
        {t(language, script ? "globalScriptSource" : "globalMergeYaml")}
        <textarea
          className="code"
          rows={12}
          value={content}
          disabled={busy}
          onChange={(event) => {
            setContent(event.target.value);
            setError(null);
            setResetting(false);
          }}
          spellCheck={false}
          required
        />
      </label>
      {error && (
        <p className="alert" role="alert">
          {t(language, error === "script-large" ? "globalScriptTooLarge" : "globalMergeTooLarge")}
        </p>
      )}
      <div className="actions">
        <button className="primary" disabled={busy || !content.trim()}>
          {t(language, script ? "globalScriptSave" : "globalMergeSave")}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => setResetting(true)}
        >
          {t(language, script ? "globalScriptReset" : "globalMergeReset")}
        </button>
        {!loaded && !content && (
          <button type="button" disabled={busy} onClick={onRetry}>
            {t(language, "globalRetry")}
          </button>
        )}
        <button type="button" disabled={busy} onClick={onClose}>
          {t(language, "globalCancel")}
        </button>
      </div>
      {resetting && (
        <div
          className="reset-confirmation"
          role="group"
          aria-label={t(language, "globalResetConfirmRegion")}
        >
          <p className="muted">
            {script
              ? t(language, "globalScriptResetWarning")
              : t(language, "globalMergeResetWarning")}
          </p>
          <div className="actions">
            <button type="button" disabled={busy} onClick={() => void reset()}>
              {t(language, "globalResetConfirm")}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => setResetting(false)}
            >
              {t(language, "globalKeepEditing")}
            </button>
          </div>
        </div>
      )}
    </form>
  );
}

function ScriptEditor({
  item,
  language,
  content,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
  language: Language;
  content: { uid?: string; source?: string };
  busy: boolean;
  perform: Perform;
  onClose: () => void;
}) {
  const [source, setSource] = useState(
    content.source ?? "function main(config, name) {\n  return config;\n}\n",
  );
  async function save(event: FormEvent) {
    event.preventDefault();
    if (await perform<Profile>("set_profile_script", { uid: item.uid, source }))
      onClose();
  }
  async function clear() {
    if (await perform<Profile>("clear_profile_script", { uid: item.uid }))
      onClose();
  }
  return (
    <section className="panel">
      <h2>{t(language, "scriptEditorTitle")}</h2>
      <p className="muted">{t(language, "scriptEditorHelp").replace("{name}", item.name || item.uid)}</p>
      <form onSubmit={save}>
        <label>
          {t(language, "scriptEditorSource")}
          <textarea
            className="code"
            rows={12}
            disabled={busy}
            value={source}
            onChange={(event) => setSource(event.target.value)}
            spellCheck={false}
            required
          />
        </label>
        <div className="form-actions">
          <button className="primary" disabled={busy}>
            {t(language, "scriptEditorSave")}
          </button>
          <button
            type="button"
            disabled={busy || !content.uid}
            onClick={() => void clear()}
          >
            {t(language, "scriptEditorRemove")}
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            {t(language, "scriptEditorCancel")}
          </button>
        </div>
      </form>
    </section>
  );
}

function SequenceEditor({
  item,
  language,
  kind,
  content,
  busy,
  perform,
  onKind,
  onClose,
}: {
  item: Profile;
  language: Language;
  kind: SequenceKind;
  content: { uid?: string; yaml?: string };
  busy: boolean;
  perform: Perform;
  onKind: (kind: SequenceKind) => void;
  onClose: () => void;
}) {
  const [yaml, setYaml] = useState(
    content.yaml ?? "prepend: []\nappend: []\ndelete: []\n",
  );
  async function save(event: FormEvent) {
    event.preventDefault();
    if (
      await perform<Profile>("set_profile_sequence", {
        uid: item.uid,
        kind,
        yaml,
      })
    )
      onClose();
  }
  async function clear() {
    if (
      await perform<Profile>("clear_profile_sequence", { uid: item.uid, kind })
    )
      onClose();
  }
  return (
    <section className="panel">
      <h2>{t(language, "sequenceEditorTitle")}</h2>
      <p className="muted">{t(language, "sequenceEditorHelp").replace("{name}", item.name || item.uid)}</p>
      <label>
        {t(language, "sequenceEditorKind")}
        <select
          value={kind}
          disabled={busy}
          onChange={(event) => onKind(event.target.value as SequenceKind)}
        >
          <option value="rules">{t(language, "sequenceRules")}</option>
          <option value="proxies">{t(language, "sequenceProxies")}</option>
          <option value="groups">{t(language, "sequenceGroups")}</option>
        </select>
      </label>
      <form onSubmit={save}>
        <label>
          {t(language, "sequenceEditorYaml")}
          <textarea
            className="code"
            rows={12}
            disabled={busy}
            value={yaml}
            onChange={(event) => setYaml(event.target.value)}
            spellCheck={false}
            required
          />
        </label>
        <div className="form-actions">
          <button className="primary" disabled={busy}>
            {t(language, "sequenceEditorSave")}
          </button>
          <button
            type="button"
            disabled={busy || !content.uid}
            onClick={() => void clear()}
          >
            {t(language, "sequenceEditorRemove")}
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            {t(language, "sequenceEditorCancel")}
          </button>
        </div>
      </form>
    </section>
  );
}

function ProfilePage({
  token,
  language,
  logout,
  profiles,
  status,
  busy,
  perform,
}: {
  token: string;
  language: Language;
  logout: (reason?: string) => void;
  profiles: Profiles;
  status: CoreStatus;
  busy: boolean;
  perform: Perform;
}) {
  const [name, setName] = useState(""),
    [yaml, setYaml] = useState(""),
    [remoteUrl, setRemoteUrl] = useState(""),
    [remoteName, setRemoteName] = useState(""),
    [remoteSelfProxy, setRemoteSelfProxy] = useState(false),
    [remoteWithProxy, setRemoteWithProxy] = useState(false),
    [remoteInvalidCerts, setRemoteInvalidCerts] = useState(false),
    [error, setError] = useState<"too-large" | Error | null>(null);
  const [rawEditing, setRawEditing] = useState<string>();
  const [editing, setEditing] = useState<Profile | null>(null);
  const [mergeEditing, setMergeEditing] = useState<{
    item: Profile;
    content: { uid?: string; yaml?: string };
  } | null>(null);
  const [sequenceEditing, setSequenceEditing] = useState<{
    item: Profile;
    kind: SequenceKind;
    content: { uid?: string; yaml?: string };
  } | null>(null);
  const [scriptEditing, setScriptEditing] = useState<{
    item: Profile;
    content: { uid?: string; source?: string };
  } | null>(null);
  useEffect(() => {
    if (rawEditing && !profiles.items?.some((item) => item.uid === rawEditing))
      setRawEditing(undefined);
  }, [rawEditing, profiles]);
  const baseProfiles = profiles.items?.filter(
    (item) => item.type === "local" || item.type === "remote",
  );
  async function openMerge(item: Profile) {
    const content = await perform<{ uid?: string; yaml?: string }>(
      "profile_merge",
      { uid: item.uid },
    );
    if (content) setMergeEditing({ item, content });
  }
  async function openSequence(item: Profile, kind: SequenceKind = "rules") {
    const content = await perform<{ uid?: string; yaml?: string }>(
      "profile_sequence",
      { uid: item.uid, kind },
    );
    if (content) setSequenceEditing({ item, kind, content });
  }
  async function openScript(item: Profile) {
    const content = await perform<{ uid?: string; source?: string }>(
      "profile_script",
      { uid: item.uid },
    );
    if (content) setScriptEditing({ item, content });
  }
  const [deleting, setDeleting] = useState<string | null>(null);
  async function remove(item: Profile) {
    const result = await perform<Profiles>("delete_profile", { uid: item.uid });
    if (result) {
      setDeleting(null);
      if (editing?.uid === item.uid) setEditing(null);
      if (mergeEditing?.item.uid === item.uid) setMergeEditing(null);
      if (sequenceEditing?.item.uid === item.uid) setSequenceEditing(null);
      if (scriptEditing?.item.uid === item.uid) setScriptEditing(null);
    }
  }
  async function upload(file?: File) {
    if (!file) return;
    try {
      if (file.size > 8 * 1024 ** 2) {
        setError("too-large");
        return;
      }
      setYaml(await file.text());
      setName(file.name.replace(/\.ya?ml$/i, ""));
      setError(null);
    } catch (error) {
      setError(error instanceof Error ? error : new Error(String(error)));
    }
  }
  async function submit(event: FormEvent) {
    event.preventDefault();
    const item = await perform<Profile>("import_profile", { name, yaml });
    if (item) {
      setName("");
      setYaml("");
    }
  }
  async function importRemote(event: FormEvent) {
    event.preventDefault();
    const item = await perform<Profile>("import_remote_profile", {
      url: remoteUrl.trim(),
      options: {
        self_proxy: remoteSelfProxy,
        with_proxy: remoteWithProxy,
        danger_accept_invalid_certs: remoteInvalidCerts,
      },
      ...(remoteName.trim() ? { name: remoteName.trim() } : {}),
    });
    if (item) {
      setRemoteUrl("");
      setRemoteName("");
    }
  }
  return (
    <div className="two-column">
      <GlobalEnhancements language={language} status={status} busy={busy} perform={perform} />
      <section className="panel">
        <div className="panel-title">
          <h2>{t(language, "profileListTitle")}</h2>
          <span>{t(language, baseProfiles?.length === 1 ? "profileCountOne" : "profileCountOther").replace("{count}", String(baseProfiles?.length || 0))}</span>
        </div>
        {!baseProfiles?.length && (
          <p className="empty">{t(language, "profileEmpty")}</p>
        )}
        {baseProfiles?.map((item) => (
          <article className="profile" key={item.uid}>
            <div>
              <h3>{item.name || item.uid}</h3>
              <p className="mono">{item.uid}</p>
              <p className="muted">
                {item.type === "remote" ? t(language, "profileRemote") : t(language, "profileLocal")}
              </p>
              {item.option?.merge && <p className="muted">{t(language, "profileLinkedMerge")}</p>}
              {item.option?.script && <p className="muted">{t(language, "profileLinkedScript")}</p>}
              {(item.option?.rules ||
                item.option?.proxies ||
                item.option?.groups) && <p className="muted">{t(language, "profileLinkedSequence")}</p>}
              {item.desc && (
                <p className="muted profile-description">{item.desc}</p>
              )}
              {item.extra && (
                <p className="muted">
                  {t(language, "profileUsed")} {bytes(item.extra.upload + item.extra.download)} /{" "}
                  {bytes(item.extra.total)}
                </p>
              )}
              {status.active_profile === item.uid && (
                <span className="badge good">{t(language, "profileCurrent")}</span>
              )}
              {deleting === item.uid && (
                <div className="delete-confirmation">
                  <p>
                    {t(language, "profileDeleteWarning")}
                  </p>
                  <div className="form-actions">
                    <button
                      disabled={busy}
                      onClick={() => void remove(item)}
                      aria-label={`${t(language, "profileConfirmDelete")} ${item.name || item.uid}`}
                    >
                      {t(language, "profileConfirmDelete")}
                    </button>
                    <button disabled={busy} onClick={() => setDeleting(null)}>
                      {t(language, "profileCancelDelete")}
                    </button>
                  </div>
                </div>
              )}
            </div>
            <div className="profile-actions">
              <button
                disabled={busy}
                aria-label={`${t(language, "profileScript")} ${item.name || item.uid}`}
                onClick={() => void openScript(item)}
              >
                {t(language, "profileScript")}
              </button>
              <button
                disabled={busy}
                aria-label={`${t(language, "profileSequence")} ${item.name || item.uid}`}
                onClick={() => void openSequence(item)}
              >
                {t(language, "profileSequence")}
              </button>
              <button
                disabled={busy}
                aria-label={`${t(language, "profileMerge")} ${item.name || item.uid}`}
                onClick={() => void openMerge(item)}
              >
                {t(language, "profileMerge")}
              </button>
              <button
                disabled={busy || rawEditing !== undefined}
                aria-label={`${t(language, "profileEditRawNamed")} ${item.name || item.uid}`}
                onClick={() => setRawEditing(item.uid)}
              >
                {t(language, "profileRawYaml")}
              </button>
              <button
                disabled={busy}
                aria-label={`${t(language, "profileEditNamed")} ${item.name || item.uid}`}
                onClick={() => setEditing(item)}
              >
                {t(language, "profileEdit")}
              </button>
              {item.type === "remote" && (
                <button
                  disabled={busy}
                  aria-label={`${t(language, "profileRefresh")} ${item.name || item.uid}`}
                  onClick={() =>
                    void perform("refresh_profile", { uid: item.uid })
                  }
                >
                  {t(language, "profileRefresh")}
                </button>
              )}
              <button
                disabled={busy}
                onClick={() =>
                  void perform("select_profile", { uid: item.uid })
                }
              >
                {status.active_profile === item.uid ? t(language, "profileReapply") : t(language, "profileUse")}
              </button>
              <button
                disabled={busy || status.active_profile === item.uid}
                aria-label={`${t(language, "profileDeleteNamed")} ${item.name || item.uid}`}
                title={
                  status.active_profile === item.uid
                    ? t(language, "profileSwitchFirst")
                    : undefined
                }
                onClick={() => setDeleting(item.uid)}
              >
                {t(language, "profileDelete")}
              </button>
            </div>
          </article>
        ))}
      </section>
      {rawEditing &&
        profiles.items?.some((item) => item.uid === rawEditing) && (
          <RawEditor
            key={rawEditing}
            item={profiles.items.find((item) => item.uid === rawEditing)!}
            active={status.active_profile === rawEditing}
            language={language}
            token={token}
            busy={busy}
            perform={perform}
            logout={logout}
            onClose={() => setRawEditing(undefined)}
          />
        )}
      {mergeEditing &&
        profiles.items?.some((item) => item.uid === mergeEditing.item.uid) && (
          <MergeEditor
            key={mergeEditing.item.uid}
            item={mergeEditing.item}
            language={language}
            content={mergeEditing.content}
            busy={busy}
            perform={perform}
            onClose={() => setMergeEditing(null)}
          />
        )}
      {sequenceEditing &&
        profiles.items?.some(
          (item) => item.uid === sequenceEditing.item.uid,
        ) && (
          <SequenceEditor
            key={`${sequenceEditing.item.uid}-${sequenceEditing.kind}`}
            item={sequenceEditing.item}
            language={language}
            kind={sequenceEditing.kind}
            content={sequenceEditing.content}
            busy={busy}
            perform={perform}
            onKind={(kind) => void openSequence(sequenceEditing.item, kind)}
            onClose={() => setSequenceEditing(null)}
          />
        )}
      {scriptEditing &&
        profiles.items?.some((item) => item.uid === scriptEditing.item.uid) && (
          <ScriptEditor
            key={scriptEditing.item.uid}
            item={scriptEditing.item}
            language={language}
            content={scriptEditing.content}
            busy={busy}
            perform={perform}
            onClose={() => setScriptEditing(null)}
          />
        )}
      {editing && profiles.items?.some((item) => item.uid === editing.uid) && (
        <ProfileEditor
          key={editing.uid}
          item={editing}
          language={language}
          busy={busy}
          perform={perform}
          onClose={() => setEditing(null)}
        />
      )}
      <section className="panel">
        <h2>{t(language, "remoteImportTitle")}</h2>
        <p className="muted">{t(language, "remoteImportHelp")}</p>
        <form onSubmit={importRemote}>
          <label>
            {t(language, "remoteImportUrl")}
            <input
              type="url"
              value={remoteUrl}
              required
              maxLength={8192}
              disabled={busy}
              placeholder="https://example.com/subscription"
              onChange={(event) => setRemoteUrl(event.target.value)}
            />
          </label>
          <label>
            {t(language, "remoteImportName")}
            <input
              value={remoteName}
              maxLength={256}
              disabled={busy}
              onChange={(event) => setRemoteName(event.target.value)}
            />
          </label>
          <label className="check-label">
            <input
              type="checkbox"
              disabled={busy}
              checked={remoteSelfProxy}
              onChange={(event) => setRemoteSelfProxy(event.target.checked)}
            />
            {t(language, "remoteImportManaged")}
          </label>
          <label className="check-label">
            <input
              type="checkbox"
              disabled={busy}
              checked={remoteWithProxy}
              onChange={(event) => setRemoteWithProxy(event.target.checked)}
            />
            {t(language, "remoteImportSystem")}
          </label>
          <label className="check-label">
            <input
              type="checkbox"
              disabled={busy}
              checked={remoteInvalidCerts}
              onChange={(event) => setRemoteInvalidCerts(event.target.checked)}
            />
            {t(language, "remoteImportInvalidCerts")}
          </label>
          <p className="muted">{t(language, "remoteImportTlsHelp")}</p>
          <p className="muted">{t(language, "remoteImportRouteHelp")}</p>
          <button className="primary" disabled={busy || !remoteUrl.trim()}>
            {t(language, "remoteImportSubmit")}
          </button>
        </form>
      </section>
      <section className="panel">
        <h2>{t(language, "localImportTitle")}</h2>
        <p className="muted">{t(language, "localImportHelp")}</p>
        <form onSubmit={submit}>
          <label>
            {t(language, "localImportUpload")}
            <input
              type="file"
              accept=".yaml,.yml,text/yaml,text/plain"
              disabled={busy}
              onChange={(event) => void upload(event.target.files?.[0])}
            />
          </label>
          <label>
            {t(language, "localImportName")}
            <input
              value={name}
              required
              maxLength={256}
              disabled={busy}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          <label>
            {t(language, "localImportYaml")}
            <textarea
              className="code small"
              value={yaml}
              required
              spellCheck={false}
              disabled={busy}
              onChange={(event) => setYaml(event.target.value)}
            />
          </label>
          {error && (
            <p className="alert" role="alert">
              {error === "too-large" ? t(language, "localImportFileTooLarge") : describe(error)}
            </p>
          )}
          <button
            className="primary"
            disabled={busy || !yaml.trim() || !name.trim()}
          >
            {t(language, "localImportSubmit")}
          </button>
        </form>
      </section>
    </div>
  );
}

function ConfigPage({
  token,
  language,
  busy,
  perform,
}: {
  token: string;
  language: Language;
  busy: boolean;
  perform: Perform;
}) {
  const [yaml, setYaml] = useState(""),
    [loading, setLoading] = useState(true),
    [message, setMessage] = useState<
      { kind: "missing" } | { kind: "applied" } | { kind: "error"; detail: string } | null
    >(null),
    [dirty, setDirty] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    command<{ yaml: string }>(token, "config", {}, controller.signal)
      .then((value) => {
        setYaml(value.yaml);
        setLoading(false);
      })
      .catch((error) => {
        if (!controller.signal.aborted) {
          setLoading(false);
          const detail = describe(error);
          setMessage(detail.includes("no committed configuration")
            ? { kind: "missing" }
            : { kind: "error", detail });
        }
      });
    return () => controller.abort();
  }, [token]);
  async function apply(event: FormEvent) {
    event.preventDefault();
    const result = await perform<CoreStatus>("edit_config", { yaml });
    if (result) {
      setDirty(false);
      setMessage({ kind: "applied" });
    }
  }
  return (
    <section className="panel">
      <div className="panel-title">
        <h2>{t(language, "configTitle")}</h2>
        <span>{dirty ? t(language, "configDirty") : t(language, "configCompleteYaml")}</span>
      </div>
      <p className="muted">
        {t(language, "configDescription")}
      </p>
      {message && <p className="info">
        {message.kind === "missing" ? t(language, "configMissing") :
          message.kind === "applied" ? t(language, "configApplied") : message.detail}
      </p>}
      <form onSubmit={apply}>
        <label>
          {t(language, "configYamlLabel")}
          <textarea
            className="code editor"
            value={yaml}
            onChange={(event) => {
              setYaml(event.target.value);
              setDirty(true);
            }}
            disabled={loading || busy}
            required
            spellCheck={false}
          />
        </label>
        <div className="form-actions">
          <p className="hint">{t(language, "configLeaveHint")}</p>
          <button
            className="primary"
            disabled={loading || busy || !yaml.trim()}
          >
            {t(language, "configApply")}
          </button>
        </div>
      </form>
    </section>
  );
}

function ProxyPage({
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
    [delays, setDelays] = useState<Record<string, number>>({}),
    [testingGroup, setTestingGroup] = useState<string | null>(null),
    [testingNode, setTestingNode] = useState<string | null>(null),
    [testUrl, setTestUrl] = useState("http://www.gstatic.com/generate_204"),
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

  const toggleGroup = (name: string) => {
    setCollapsed((prev) => {
      const next = { ...prev, [name]: !prev[name] };
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

  async function testGroupDelay(group: string) {
    if (testingGroup || busy) return;
    setTestingGroup(group);
    setError("");
    try {
      const results = await command<Record<string, number>>(token, "delay_group", {
        group,
        url: testUrl,
        timeout: 5000,
      });
      setDelays((prev) => ({ ...prev, ...results }));
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
      const res = await command<ProxyDelay>(token, "delay_proxy", {
        name: node,
        url: testUrl,
        timeout: 5000,
      });
      setDelays((prev) => ({ ...prev, [node]: res.delay }));
    } catch (e) {
      setError(describe(e));
    } finally {
      setTestingNode(null);
    }
  }

  function getNodeDelay(node: string): number | undefined {
    if (node in delays) {
      return delays[node];
    }
    const history = proxies?.proxies[node]?.history;
    if (history && history.length > 0) {
      return history[history.length - 1].delay;
    }
    return undefined;
  }

  function renderDelayBadge(delay: number | undefined, isTesting: boolean) {
    if (isTesting) {
      return <span className="delay-badge delay-testing">{t(language, "proxyDelayTesting")}</span>;
    }
    if (delay === undefined || delay < 0) {
      return <span className="delay-badge delay-untested">{t(language, "proxyDelayUntested")}</span>;
    }
    if (delay === 0 || delay >= 10000) {
      return <span className="delay-badge delay-timeout">{t(language, "proxyDelayTimeout")}</span>;
    }
    if (delay < 300) {
      return <span className="delay-badge delay-fast">{delay}ms</span>;
    }
    if (delay < 600) {
      return <span className="delay-badge delay-medium">{delay}ms</span>;
    }
    return <span className="delay-badge delay-slow">{delay}ms</span>;
  }

  return (
    <>
      <div className="section-title">
        <div>
          <p className="muted">
            {t(language, "proxyHelp")}
          </p>
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

      {groups.map(([name, group]) => {
        const isCollapsed = !!collapsed[name];
        const currentSelection = group.fixed || group.now || t(language, "proxyGroupWaiting");
        return (
          <section className="panel" key={name}>
            <div className={`panel-title group-header ${isCollapsed ? "collapsed" : ""}`}>
              <button
                type="button"
                className="group-title-btn"
                onClick={() => toggleGroup(name)}
                aria-expanded={!isCollapsed}
                aria-label={`${name} ${isCollapsed ? "展开" : "折叠"}`}
              >
                <span className={`group-toggle-icon ${isCollapsed ? "" : "expanded"}`}>▶</span>
                <div>
                  <h2>{name}</h2>
                  <p className="muted">
                    {group.type} · {t(language, "proxyCurrent")}
                    <strong style={{ color: "#1b6954", marginLeft: "4px" }}>
                      {currentSelection}
                    </strong>
                  </p>
                </div>
              </button>
              <div className="panel-actions">
                <button
                  type="button"
                  disabled={busy || loading || testingGroup === name}
                  onClick={() => void testGroupDelay(name)}
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
                    const delay = getNodeDelay(node);
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
                        <span style={{ fontWeight: isSelected ? 600 : 400 }}>{node}</span>
                        <div className="node-meta">
                          {renderDelayBadge(delay, isTesting)}
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
                          <span>
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

function LogLines({ logs, language }: { logs: CoreLog[]; language: Language }) {
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
function LogPage({ logs, language }: { logs: CoreLog[]; language: Language }) {
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
        <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
          <input
            aria-label={t(language, "logsFilterAria")}
            placeholder={t(language, "logsFilterPlaceholder")}
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
          {filter && (
            <button className="quiet" onClick={() => setFilter("")}>
              {t(language, "logsClear")}
            </button>
          )}
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

createRoot(document.getElementById("root")!).render(<App />);
