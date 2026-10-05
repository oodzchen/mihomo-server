import { createRoot } from "react-dom/client";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type MouseEvent,
} from "react";
import { ApiError, command, reloadToServedBuild, servedBuildChanged, subscribe, type Connection, type Perform } from "./api";
import { SettingsPage, type SettingsPageHandle } from "./settings";
import { CoreUpgradePage } from "./core-upgrade";
import { ServicePage } from "./service";
import { RulesPage } from "./rules";
import { LanguagePicker } from "./language-picker";
import { connectionLabel, LanguageContext, phaseLabel, resolveLanguage, savedLanguage, saveLanguage, t, type Language, type MessageKey } from "./i18n";
import type { CoreLog, CoreStatus, EventMessage, Preferences, Profiles } from "./types";
import { describe } from "./format";
import { Overview } from "./overview";
import { ProfilePage } from "./profiles";
import { ConfigPage } from "./config";
import { ProxyPage } from "./proxies";
import { LogPage } from "./logs";
import { ToastProvider, useToast } from "./toast";
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
  ["/service", "service"],
];

const TOKEN_KEY = "mihomo.token";

type PendingNavigation =
  | { kind: "route"; path: string; method: "push" | "replace" }
  | { kind: "logout" };

function savedToken() {
  try { return window.sessionStorage.getItem(TOKEN_KEY) || ""; }
  catch { return ""; }
}

function storeToken(token: string) {
  try {
    if (token) window.sessionStorage.setItem(TOKEN_KEY, token);
    else window.sessionStorage.removeItem(TOKEN_KEY);
  } catch { /* Private browser storage can be unavailable. */ }
}

function consumeUrlToken(): string | undefined {
  const url = new URL(window.location.href);
  const fragment = new URLSearchParams(url.hash.slice(1));
  const token = fragment.get("token") ?? url.searchParams.get("token");
  if (token === null) return undefined;
  url.searchParams.delete("token");
  if (fragment.has("token")) {
    fragment.delete("token");
    url.hash = fragment.toString();
  }
  window.history.replaceState(window.history.state, "", url.pathname + url.search + url.hash);
  return token.trim();
}

// Consume the URL once before React renders; explicit links take precedence
// over an older session, and credentials leave the address before API calls.
const initialToken = consumeUrlToken() ?? savedToken();

function App() {
  const [language, setLanguage] = useState<Language>(savedLanguage);
  const [session, setSession] = useState<{
    token: string;
    status: CoreStatus;
  }>();
  const [loginError, setLoginError] = useState("");
  const [restoring, setRestoring] = useState(!!initialToken);
  const logout = useCallback((reason = "") => {
    storeToken("");
    setSession(undefined);
    setLoginError(reason);
  }, []);
  const changeLanguage = useCallback((value: string) => {
    const next = resolveLanguage(value);
    saveLanguage(next);
    setLanguage(next);
    setLoginError("");
  }, []);
  const login = useCallback((value: { token: string; status: CoreStatus }) => {
    storeToken(value.token);
    setSession(value);
  }, []);
  useEffect(() => {
    const token = initialToken;
    if (!token) return;
    let active = true;
    command<CoreStatus>(token, "status")
      .then((status) => active && login({ token, status }))
      .catch((error) => {
        if (!active) return;
        if (error instanceof ApiError && error.status === 401) {
          storeToken("");
          setLoginError(t(savedLanguage(), "invalidToken"));
        } else {
          setLoginError(describe(error));
        }
      })
      .finally(() => active && setRestoring(false));
    return () => { active = false; };
  }, [login]);
  useEffect(() => {
    document.documentElement.lang = language === "en" ? "en" : language === "zhtw" ? "zh-TW" : "zh-CN";
    document.title = `Mihomo · ${t(language, "serviceManagement")}`;
  }, [language]);
  if (restoring) return null;
  return <LanguageContext.Provider value={language}>{session ? (
    <ToastProvider language={language}><Manager token={session.token} initial={session.status} logout={logout} language={language} changeLanguage={changeLanguage} /></ToastProvider>
  ) : (
    <Login error={loginError} login={login} language={language} changeLanguage={changeLanguage} />
  )}</LanguageContext.Provider>;
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
      <div className="login-lang">
        <LanguagePicker language={language} changeLanguage={changeLanguage} />
      </div>
      <section className="login-card">
        <div className="login-header">
          <h1>{t(language, "loginTitle")}</h1>
          <p className="muted">{t(language, "loginHelp")}</p>
        </div>
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
    [connection, setConnection] = useState<Connection>("connecting");
  const [busy, setBusy] = useState(false);
  const [settingsEditor, setSettingsEditor] = useState({ dirty: false, busy: true });
  const [pendingNavigation, setPendingNavigation] = useState<PendingNavigation>();
  const [leaving, setLeaving] = useState(false);
  const setNotice = useToast();
  const alive = useRef(true),
    pending = useRef(new Set<AbortController>()),
    locked = useRef(false);
  const settingsPage = useRef<SettingsPageHandle>(null);
  const routeRef = useRef(route), settingsDirtyRef = useRef(settingsEditor.dirty);
  routeRef.current = route;
  settingsDirtyRef.current = settingsEditor.dirty;
  const languageRef = useRef(language);
  languageRef.current = language;
  // The interface language is an instance preference shared with the other
  // clients (desktop tray included): adopt what the service pushes, and save
  // changes made here to the service, which pushes them to everyone else.
  const adoptPreferences = useCallback((preferences?: Preferences) => {
    if (preferences?.language && preferences.language !== languageRef.current) changeLanguage(preferences.language);
  }, [changeLanguage]);
  const changeSharedLanguage = useCallback((value: string) => {
    const next = resolveLanguage(value);
    changeLanguage(next);
    command<Preferences>(token, "set_language", { language: next }).catch((error) => {
      if (!alive.current) return;
      if (error instanceof ApiError && error.status === 401) logout(t(languageRef.current, "expiredToken"));
      else setNotice(describe(error), "error");
    });
  }, [changeLanguage, token, logout, setNotice]);
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
          adoptPreferences(event.preferences);
        }
        if (event.type === "preferences") adoptPreferences(event.data as Preferences);
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
        if (value === "unauthorized") logout(t(languageRef.current, "expiredToken"));
      },
    );
    const popstate = () => {
      const path = location.pathname;
      if (routeRef.current === "/settings" && settingsDirtyRef.current && path !== routeRef.current) {
        history.pushState(null, "", routeRef.current);
        setPendingNavigation({ kind: "route", path, method: "replace" });
        return;
      }
      setRoute(path);
    };
    window.addEventListener("popstate", popstate);
    return () => {
      alive.current = false;
      cancel();
      pending.current.forEach((controller) => controller.abort());
      window.removeEventListener("popstate", popstate);
    };
  }, [token, logout, adoptPreferences]);
  // A service that comes back upgraded serves new page files: load them, as a
  // browser refresh would (the desktop client has none), but never over
  // unsaved settings edits.
  const [stale, setStale] = useState(false);
  const dropped = useRef(false);
  useEffect(() => {
    if (connection === "reconnecting") dropped.current = true;
    if (connection !== "connected" || !dropped.current) return;
    dropped.current = false;
    const controller = new AbortController();
    servedBuildChanged(controller.signal)
      .then((changed) => changed && setStale(true))
      .catch(() => {
        // Unreadable now; the next reconnect checks again.
      });
    return () => controller.abort();
  }, [connection]);
  useEffect(() => {
    if (stale && !settingsEditor.dirty) reloadToServedBuild();
  }, [stale, settingsEditor.dirty]);
  useEffect(() => {
    if (!settingsEditor.dirty) return;
    const beforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", beforeUnload);
    return () => window.removeEventListener("beforeunload", beforeUnload);
  }, [settingsEditor.dirty]);
  const perform: Perform = async <T,>(
    name: string,
    fields: Record<string, unknown> = {},
    options: Parameters<Perform>[2] = {},
  ) => {
    if (locked.current) { options.toast?.dismiss(); return; }
    locked.current = true;
    setBusy(true);
    const toast = options.toast ?? setNotice.loading(t(language, "working"));
    try {
      const result = await request<T>(name, fields);
      const [current, catalog] = await Promise.all([
        request<CoreStatus>("status"),
        request<Profiles>("profiles"),
      ]);
      if (alive.current) {
        setStatus(current);
        setProfiles(catalog);
        if (options.notify !== false || !options.toast) {
          const message = name.startsWith("set_") || name.startsWith("edit_")
            ? t(language, "saveSuccess")
            : t(language, "completed");
          toast.finish(message);
        }
      }
      return result;
    } catch (error) {
      if (alive.current) {
        if (error instanceof ApiError && error.status === 401)
          logout(t(language, "expiredToken"));
        else if (options.reconcile) toast.recordError(describe(error));
        else toast.finish(describe(error), "error");
      }
      return undefined;
    } finally {
      locked.current = false;
      if (alive.current) setBusy(false);
      else toast.dismiss();
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
    if (route === "/settings" && settingsEditor.dirty && path !== route) {
      setPendingNavigation({ kind: "route", path, method: "push" });
      return;
    }
    history.pushState(null, "", path);
    setRoute(path);
    setNotice("");
  }
  const updateSettingsEditor = useCallback((dirty: boolean, editorBusy: boolean) => {
    setSettingsEditor(previous => previous.dirty === dirty && previous.busy === editorBusy
      ? previous
      : { dirty, busy: editorBusy });
  }, []);
  function finishNavigation(target: PendingNavigation) {
    setPendingNavigation(undefined);
    setNotice("");
    if (target.kind === "logout") {
      logout();
      return;
    }
    if (target.method === "replace") history.replaceState(null, "", target.path);
    else history.pushState(null, "", target.path);
    setRoute(target.path);
  }
  async function saveAndLeave() {
    if (!pendingNavigation || !settingsPage.current) return;
    setLeaving(true);
    const target = pendingNavigation;
    const saved = await settingsPage.current.save();
    setLeaving(false);
    if (saved) finishNavigation(target);
    else setPendingNavigation(undefined);
  }
  function discardAndLeave() {
    if (!pendingNavigation) return;
    settingsPage.current?.discard();
    finishNavigation(pendingNavigation);
  }
  function requestLogout() {
    if (route === "/settings" && settingsEditor.dirty) {
      setPendingNavigation({ kind: "logout" });
      return;
    }
    logout();
  }
  const active = profiles.items?.find(
    (item) => item.uid === status.active_profile,
  );
  const title = t(language, pages.find(([path]) => route === path)?.[1] || "overview");
  const colon = t(language, "colon");
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
          <div className="sidebar-status">
            <div className="status-item">
              <span className="status-label">{t(language, "coreStatus")}{colon}</span>
              <span className={`dot ${status.phase === "running" ? "good" : status.phase === "failed" ? "bad" : ""}`} />
              <span role="status" aria-label={t(language, "coreStatus")}>
                {phaseLabel(language, status.phase)}
              </span>
            </div>
            <div className="status-item">
              <span className="status-label">{t(language, "serviceConnection")}{colon}</span>
              <span className={`dot ${connection === "connected" ? "good" : ""}`} />
              <span role="status" aria-label={t(language, "eventConnection")}>
                {connectionLabel(language, connection)}
              </span>
            </div>
          </div>
          <button className="quiet" onClick={requestLogout}>
            {t(language, "logout")}
          </button>
        </div>
      </aside>
      <div className="workspace">
        <header className="page-header">
          <h1>{title}</h1>
          {route === "/settings" && (
            <button
              type="button"
              className="primary"
              disabled={!settingsEditor.dirty || settingsEditor.busy}
              onClick={() => { void settingsPage.current?.save(); }}
            >
              {t(language, "saveSettings")}
            </button>
          )}
        </header>
        <div className="feedback" aria-live="polite">
          {status.error && (
            <p className="alert">
              {status.phase === "failed" ? t(language, "coreError") : t(language, "operationError")}{colon}
              {status.error}
            </p>
          )}
          {status.selection_error && (
            <p className="alert">{t(language, "selectionRestore")}{colon}{status.selection_error}</p>
          )}
          {status.selection_pending?.length > 0 && (
            <p className="info">
              {t(language, "restoringNodes")}{colon}{status.selection_pending.join(t(language, "listSeparator"))}
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
            activeProfileName={active?.name}
          />
        ) : route === "/service" ? (
          <ServicePage
            token={token}
            language={language}
            connection={connection}
            busy={busy}
            logout={logout}
          />
        ) : route === "/settings" ? (
          <SettingsPage
            ref={settingsPage}
            token={token}
            language={language}
            changeLanguage={changeSharedLanguage}
            status={status}
            connection={connection}
            busy={busy}
            perform={perform}
            logout={logout}
            desktopVersion={window.__MIHOMO_DESKTOP_VERSION__}
            onEditorStateChange={updateSettingsEditor}
          />
        ) : (
          <Overview
            busy={busy}
            perform={perform}
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
        {pendingNavigation && (
          <div className="modal-backdrop">
            <section className="modal-dialog modal-dialog-sm" role="dialog" aria-modal="true" aria-labelledby="unsaved-settings-title">
              <div className="modal-header">
                <h2 id="unsaved-settings-title">{t(language, "unsavedSettingsTitle")}</h2>
              </div>
              <div className="modal-body">
                <p>{t(language, "unsavedSettingsMessage")}</p>
                <div className="form-actions">
                  <button type="button" className="primary" disabled={leaving || settingsEditor.busy} onClick={() => { void saveAndLeave(); }}>
                    {t(language, "saveAndLeave")}
                  </button>
                  <button type="button" disabled={leaving} onClick={discardAndLeave}>
                    {t(language, "discardChanges")}
                  </button>
                  <button type="button" disabled={leaving} onClick={() => setPendingNavigation(undefined)}>
                    {t(language, "continueEditing")}
                  </button>
                </div>
              </div>
            </section>
          </div>
        )}
        {route !== "/settings" && (
          <div style={{ position: "fixed", opacity: 0, pointerEvents: "none", width: 20, height: 20, overflow: "hidden" }}>
            <LanguagePicker language={language} changeLanguage={changeSharedLanguage} />
          </div>
        )}
      </div>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
