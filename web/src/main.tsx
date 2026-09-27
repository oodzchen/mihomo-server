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
import type {
  CoreLog,
  CoreStatus,
  EventMessage,
  Profile,
  Profiles,
  Proxies,
} from "./types";
import "./style.css";

const phases: Record<string, string> = {
  stopped: "已停止",
  starting: "启动中",
  running: "运行中",
  stopping: "停止中",
  recovering: "恢复中",
  failed: "启动失败",
  shutdown: "服务关闭",
};
const pages = [
  ["/", "概览", "01"],
  ["/profiles", "订阅", "02"],
  ["/config", "配置", "03"],
  ["/proxies", "节点", "04"],
  ["/logs", "日志", "05"],
  ["/settings", "设置", "06"],
];
const describe = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

function App() {
  const [session, setSession] = useState<{
    token: string;
    status: CoreStatus;
  }>();
  const [loginError, setLoginError] = useState("");
  const logout = useCallback((reason = "") => {
    setSession(undefined);
    setLoginError(reason);
  }, []);
  return session ? (
    <Manager token={session.token} initial={session.status} logout={logout} />
  ) : (
    <Login error={loginError} login={setSession} />
  );
}

function Login({
  error,
  login,
}: {
  error: string;
  login: (session: { token: string; status: CoreStatus }) => void;
}) {
  const [token, setToken] = useState(""),
    [busy, setBusy] = useState(false),
    [failure, setFailure] = useState("");
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
          ? "令牌无效，请检查后重试。"
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
          MH<span>MIHOMO SERVER</span>
        </div>
        <div>
          <p className="eyebrow">独立服务 · 浏览器管理</p>
          <h1>
            让连接
            <br />
            尽在掌握。
          </h1>
          <p>
            订阅、配置与内核状态，
            <br />
            在同一个地方管理。
          </p>
        </div>
        <span className="login-foot">Mihomo / Headless</span>
      </div>
      <section className="login-form">
        <p className="eyebrow">服务管理入口</p>
        <h2>连接你的服务</h2>
        <p className="muted">
          输入服务管理令牌以继续。令牌仅保存在当前页面内存，刷新后需重新登录。
        </p>
        <form onSubmit={submit}>
          <label>
            管理令牌
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
            {busy ? "验证中…" : "连接服务"}
          </button>
        </form>
        <p className="hint">令牌位于服务数据目录的 management-token 文件中。</p>
      </section>
    </main>
  );
}

function Manager({
  token,
  initial,
  logout,
}: {
  token: string;
  initial: CoreStatus;
  logout: (reason?: string) => void;
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
        if (value === "认证失败") logout("认证失效，请重新输入令牌。");
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
        setNotice("操作已完成");
      }
      return result;
    } catch (error) {
      if (alive.current) {
        if (error instanceof ApiError && error.status === 401)
          logout("认证失效，请重新输入令牌。");
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
  const title = pages.find(([path]) => route === path)?.[1] || "概览";
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
          MH
          <span>
            MIHOMO
            <br />
            SERVER
          </span>
        </div>
        <p className="nav-label">工作空间</p>
        <nav aria-label="主导航">
          {pages.map(([path, label, number]) => (
            <a
              key={path}
              href={path}
              aria-current={route === path ? "page" : undefined}
              onClick={(event) => navigate(event, path)}
            >
              <span>{number}</span>
              {label}
            </a>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <span className={`dot ${connection === "已连接" ? "good" : ""}`} />
          <span role="status" aria-label="事件连接状态">
            {connection}
          </span>
          <button className="quiet" onClick={() => logout()}>
            退出登录
          </button>
        </div>
      </aside>
      <div className="workspace">
        <header>
          <div>
            <p className="eyebrow">MIHOMO / 服务管理</p>
            <h1>{title}</h1>
          </div>
          <span className={`badge ${status.phase === "running" ? "good" : ""}`}>
            {phases[status.phase] || status.phase}
          </span>
        </header>
        <section className="core-strip" aria-label="内核状态">
          <div>
            <strong>{status.version || "Mihomo 内核"}</strong>
            <p>
              {active?.name ||
                (status.active_profile ? status.active_profile : "未选择订阅")}
              <span className="separator">/</span>
              {status.pid ? `PID ${status.pid}` : "内核未运行"}
            </p>
          </div>
          <div className="actions">
            <button
              disabled={busy || transitional || status.phase === "running"}
              onClick={() => void perform("start")}
            >
              启动内核
            </button>
            <button
              disabled={
                busy || !["running", "recovering"].includes(status.phase)
              }
              onClick={() => void perform("stop")}
            >
              停止内核
            </button>
            <button
              disabled={busy || transitional}
              onClick={() => void perform("restart")}
            >
              重启内核
            </button>
          </div>
        </section>
        <div className="feedback" aria-live="polite">
          {busy && <p className="info">正在处理，请稍候…</p>}
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
              {status.phase === "failed" ? "内核错误" : "上次操作错误"}：
              {status.error}
            </p>
          )}
          {status.selection_error && (
            <p className="alert">节点恢复：{status.selection_error}</p>
          )}
          {status.selection_pending?.length > 0 && (
            <p className="info">
              正在恢复节点：{status.selection_pending.join("、")}
            </p>
          )}
        </div>
        {route === "/profiles" ? (
          <ProfilePage
            token={token}
            logout={logout}
            profiles={profiles}
            status={status}
            busy={busy}
            perform={perform}
          />
        ) : route === "/config" ? (
          <ConfigPage token={token} busy={busy} perform={perform} />
        ) : route === "/proxies" ? (
          <ProxyPage
            token={token}
            status={status}
            busy={busy}
            perform={perform}
          />
        ) : route === "/logs" ? (
          <LogPage logs={logs} />
        ) : route === "/settings" ? (
          <SettingsPage
            token={token}
            status={status}
            connection={connection}
            busy={busy}
            perform={perform}
            logout={logout}
          />
        ) : (
          <Overview
            token={token}
            connection={connection}
            logout={logout}
            status={status}
            active={active}
            navigate={navigate}
            logs={logs}
          />
        )}
        <footer>独立运行 · 配置与节点选择由服务保存</footer>
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
  token,
  connection,
  logout,
  status,
  active,
  navigate,
  logs,
}: {
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
          <p>上传速率</p>
          <strong>
            {bytes(traffic?.up)}
            <small>/s</small>
          </strong>
        </div>
        <div>
          <p>下载速率</p>
          <strong>
            {bytes(traffic?.down)}
            <small>/s</small>
          </strong>
        </div>
        <div>
          <p>活跃连接</p>
          <strong>{connections?.count ?? "—"}</strong>
        </div>
        <div>
          <p>内存占用</p>
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
            <h2>当前配置</h2>
            <a href="/config" onClick={(event) => navigate(event, "/config")}>
              编辑配置 ↗
            </a>
          </div>
          <dl>
            <div>
              <dt>活动订阅</dt>
              <dd>{active?.name || "未选择"}</dd>
            </div>
            <div>
              <dt>配置版本</dt>
              <dd className="mono">{status.config_revision || "尚未提交"}</dd>
            </div>
            <div>
              <dt>内核状态</dt>
              <dd>{phases[status.phase]}</dd>
            </div>
          </dl>
          <p className="hint">
            修改会先经过 YAML 和内核校验。失败时保留上一个已提交配置。
          </p>
        </section>
        <section className="panel">
          <div className="panel-title">
            <h2>开始使用</h2>
            <a
              href="/profiles"
              onClick={(event) => navigate(event, "/profiles")}
            >
              管理订阅 ↗
            </a>
          </div>
          <ol className="steps">
            <li>导入本地 YAML 订阅</li>
            <li>使用订阅并启动内核</li>
            <li>选择节点，设置会自动保存</li>
          </ol>
        </section>
      </div>
      <section className="panel">
        <div className="panel-title">
          <h2>最近日志</h2>
          <a href="/logs" onClick={(event) => navigate(event, "/logs")}>
            查看全部 ↗
          </a>
        </div>
        <LogLines logs={logs.slice(-8)} />
      </section>
    </>
  );
}

function ProfileEditor({
  item,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
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
      if (withProxy !== (item.option?.with_proxy ?? false)) options.with_proxy = withProxy;
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
      <h2>编辑订阅</h2>
      <p className="muted">
        保存名称、描述和下载设置。修改链接后，点击刷新订阅以获取新内容。
      </p>
      <form onSubmit={save}>
        <label>
          修改订阅名称
          <input
            required
            maxLength={256}
            disabled={busy}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          订阅描述
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
              远程订阅链接
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
              订阅 User-Agent
              <input
                maxLength={1024}
                disabled={busy}
                value={agent}
                onChange={(event) => setAgent(event.target.value)}
              />
            </label>
            <label>
              下载超时（秒）
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
              更新间隔（分钟）
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
              允许自动更新
            </label>
            <label className="check-label">
              <input
                type="checkbox"
                disabled={busy}
                checked={selfProxy}
                onChange={(event) => setSelfProxy(event.target.checked)}
              />
              订阅刷新通过托管内核代理
            </label>
            <label className="check-label">
              <input type="checkbox" disabled={busy} checked={withProxy} onChange={(event) => setWithProxy(event.target.checked)} />
              订阅刷新使用服务系统代理
            </label>
            <p className="muted">
              托管内核模式优先，需要运行中的 HTTP 或 Mixed 入口。
              系统代理读取服务环境，未配置时直连。自动更新设置会保存，定时更新尚未接入。
            </p>
          </>
        )}
        <div className="form-actions">
          <button className="primary" disabled={busy}>
            保存订阅信息
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            取消编辑
          </button>
        </div>
      </form>
    </section>
  );
}

function MergeEditor({
  item,
  content,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
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
      <h2>合并增强</h2>
      <p className="muted">
        为 {item.name || item.uid} 合并
        YAML。当前订阅会先校验并应用；其他订阅在下次使用时应用。
      </p>
      <form onSubmit={save}>
        <label>
          合并增强 YAML
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
            保存增强
          </button>
          <button
            type="button"
            disabled={busy || !content.uid}
            onClick={() => void clear()}
          >
            移除增强
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            取消增强编辑
          </button>
        </div>
      </form>
    </section>
  );
}

type SequenceKind = "rules" | "proxies" | "groups";

type GlobalKind = "merge" | "script";

function GlobalEnhancements({
  status,
  busy,
  perform,
}: {
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
    <section className="panel global-enhancements" aria-label="全局增强">
      <div className="panel-title">
        <h2>全局增强</h2>
        <div className="actions">
          <button
            disabled={busy || editing !== null}
            onClick={() => void open("merge")}
          >
            编辑全局合并
          </button>
          <button
            disabled={busy || editing !== null}
            onClick={() => void open("script")}
          >
            编辑全局脚本
          </button>
        </div>
      </div>
      <p className="muted">
        应用于所有订阅，先执行全局合并和脚本，再执行订阅自身的合并和脚本。
        未设置订阅增强链接时，对应阶段会再次使用全局增强，脚本可能执行两次。
      </p>
      <p className="hint">
        {status.active_profile
          ? "保存会重新生成、校验并应用当前订阅；失败时保留原配置，已停止的内核保持停止。"
          : "当前未选择订阅。保存不会改变正在运行的独立配置；脚本仅检查语法，使用订阅时再执行并校验。"}
      </p>
      {editing && (
        <GlobalEditor
          key={editing.version}
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
  kind,
  initial,
  loaded,
  busy,
  perform,
  onRetry,
  onClose,
}: {
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
  const [error, setError] = useState("");
  const script = kind === "script";
  const title = script ? "全局脚本增强" : "全局合并增强";
  async function save(event: FormEvent) {
    event.preventDefault();
    setError("");
    const limit = (script ? 1 : 8) * 1024 ** 2;
    if (new TextEncoder().encode(content).length > limit) {
      setError(script ? "脚本不能超过 1 MiB。" : "合并 YAML 不能超过 8 MiB。");
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
    setError("");
    if (await perform<Profile>(`reset_global_${kind}`)) onClose();
  }
  return (
    <form className="global-editor" onSubmit={save} aria-label={title}>
      <h3>{title}</h3>
      <p className="muted">
        {script
          ? "编写 main(config, name)，返回配置对象。脚本错误会显示在页面上，console 输出可在「日志」中查看。"
          : "合并 YAML 映射；映射会合并，数组会替换。服务的私有控制器配置由服务管理。"}
      </p>
      {!loaded && (
        <p className="info">
          未读取到已保存内容。可重试读取、粘贴完整内容替换，或恢复默认。
        </p>
      )}
      <label>
        {script ? "全局脚本 JavaScript" : "全局合并 YAML"}
        <textarea
          className="code"
          rows={12}
          value={content}
          disabled={busy}
          onChange={(event) => {
            setContent(event.target.value);
            setError("");
            setResetting(false);
          }}
          spellCheck={false}
          required
        />
      </label>
      {error && (
        <p className="alert" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button className="primary" disabled={busy || !content.trim()}>
          {script ? "保存全局脚本" : "保存全局合并"}
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => setResetting(true)}
        >
          {script ? "恢复默认全局脚本" : "恢复默认全局合并"}
        </button>
        {!loaded && !content && (
          <button type="button" disabled={busy} onClick={onRetry}>
            重试读取全局增强
          </button>
        )}
        <button type="button" disabled={busy} onClick={onClose}>
          取消全局编辑
        </button>
      </div>
      {resetting && (
        <div
          className="reset-confirmation"
          role="group"
          aria-label="恢复默认确认"
        >
          <p className="muted">
            {script
              ? "将恢复返回原配置的默认脚本，替换已保存的脚本和当前输入。"
              : "将恢复启用节点选择记录的默认合并，替换已保存的合并和当前输入。"}
          </p>
          <div className="actions">
            <button type="button" disabled={busy} onClick={() => void reset()}>
              确认恢复默认
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => setResetting(false)}
            >
              继续编辑
            </button>
          </div>
        </div>
      )}
    </form>
  );
}

function ScriptEditor({
  item,
  content,
  busy,
  perform,
  onClose,
}: {
  item: Profile;
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
      <h2>脚本增强</h2>
      <p className="muted">
        为 {item.name || item.uid} 编写 main(config,
        name)，返回配置对象。脚本在序列与合并增强之后执行。保存前会运行脚本；当前订阅会校验并应用配置。执行失败会保留旧配置，输出显示在日志中。
      </p>
      <form onSubmit={save}>
        <label>
          脚本增强 JavaScript
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
            保存脚本增强
          </button>
          <button
            type="button"
            disabled={busy || !content.uid}
            onClick={() => void clear()}
          >
            移除脚本增强
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            取消脚本编辑
          </button>
        </div>
      </form>
    </section>
  );
}

function SequenceEditor({
  item,
  kind,
  content,
  busy,
  perform,
  onKind,
  onClose,
}: {
  item: Profile;
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
      <h2>序列增强</h2>
      <p className="muted">
        为 {item.name || item.uid}{" "}
        前置、追加或删除规则、代理及代理组。当前订阅会校验并应用；其他订阅在使用时应用。切换类型会丢弃未保存内容。
      </p>
      <label>
        序列增强类型
        <select
          value={kind}
          disabled={busy}
          onChange={(event) => onKind(event.target.value as SequenceKind)}
        >
          <option value="rules">规则</option>
          <option value="proxies">代理</option>
          <option value="groups">代理组</option>
        </select>
      </label>
      <form onSubmit={save}>
        <label>
          序列增强 YAML
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
            保存序列增强
          </button>
          <button
            type="button"
            disabled={busy || !content.uid}
            onClick={() => void clear()}
          >
            移除序列增强
          </button>
          <button type="button" disabled={busy} onClick={onClose}>
            取消序列编辑
          </button>
        </div>
      </form>
    </section>
  );
}

function ProfilePage({
  token,
  logout,
  profiles,
  status,
  busy,
  perform,
}: {
  token: string;
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
    [error, setError] = useState("");
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
      if (file.size > 8 * 1024 ** 2) throw new Error("文件不能超过 8 MiB");
      setYaml(await file.text());
      setName(file.name.replace(/\.ya?ml$/i, ""));
      setError("");
    } catch (error) {
      setError(describe(error));
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
      options: { self_proxy: remoteSelfProxy, with_proxy: remoteWithProxy },
      ...(remoteName.trim() ? { name: remoteName.trim() } : {}),
    });
    if (item) {
      setRemoteUrl("");
      setRemoteName("");
    }
  }
  return (
    <div className="two-column">
      <GlobalEnhancements status={status} busy={busy} perform={perform} />
      <section className="panel">
        <div className="panel-title">
          <h2>订阅列表</h2>
          <span>{baseProfiles?.length || 0} 个</span>
        </div>
        {!baseProfiles?.length && (
          <p className="empty">还没有订阅。导入一个 YAML 文件开始使用。</p>
        )}
        {baseProfiles?.map((item) => (
          <article className="profile" key={item.uid}>
            <div>
              <h3>{item.name || item.uid}</h3>
              <p className="mono">{item.uid}</p>
              <p className="muted">
                {item.type === "remote" ? "远程订阅" : "本地订阅"}
              </p>
              {item.option?.merge && <p className="muted">已关联合并增强</p>}
              {item.option?.script && <p className="muted">已关联脚本增强</p>}
              {(item.option?.rules ||
                item.option?.proxies ||
                item.option?.groups) && <p className="muted">已关联序列增强</p>}
              {item.desc && (
                <p className="muted profile-description">{item.desc}</p>
              )}
              {item.extra && (
                <p className="muted">
                  已用 {bytes(item.extra.upload + item.extra.download)} /{" "}
                  {bytes(item.extra.total)}
                </p>
              )}
              {status.active_profile === item.uid && (
                <span className="badge good">当前订阅</span>
              )}
              {deleting === item.uid && (
                <div className="delete-confirmation">
                  <p>
                    删除此订阅、独占的辅助配置和 DNS 偏好？共享辅助配置会保留。
                  </p>
                  <div className="form-actions">
                    <button
                      disabled={busy}
                      onClick={() => void remove(item)}
                      aria-label={`确认删除 ${item.name || item.uid}`}
                    >
                      确认删除
                    </button>
                    <button disabled={busy} onClick={() => setDeleting(null)}>
                      取消删除
                    </button>
                  </div>
                </div>
              )}
            </div>
            <div className="profile-actions">
              <button
                disabled={busy}
                aria-label={`脚本增强 ${item.name || item.uid}`}
                onClick={() => void openScript(item)}
              >
                脚本增强
              </button>
              <button
                disabled={busy}
                aria-label={`序列增强 ${item.name || item.uid}`}
                onClick={() => void openSequence(item)}
              >
                序列增强
              </button>
              <button
                disabled={busy}
                aria-label={`合并增强 ${item.name || item.uid}`}
                onClick={() => void openMerge(item)}
              >
                合并增强
              </button>
              <button
                disabled={busy || rawEditing !== undefined}
                aria-label={`编辑原始订阅 ${item.name || item.uid}`}
                onClick={() => setRawEditing(item.uid)}
              >
                原始 YAML
              </button>
              <button
                disabled={busy}
                aria-label={`编辑订阅 ${item.name || item.uid}`}
                onClick={() => setEditing(item)}
              >
                编辑
              </button>
              {item.type === "remote" && (
                <button
                  disabled={busy}
                  aria-label={`刷新订阅 ${item.name || item.uid}`}
                  onClick={() =>
                    void perform("refresh_profile", { uid: item.uid })
                  }
                >
                  刷新订阅
                </button>
              )}
              <button
                disabled={busy}
                onClick={() =>
                  void perform("select_profile", { uid: item.uid })
                }
              >
                {status.active_profile === item.uid ? "重新应用" : "使用订阅"}
              </button>
              <button
                disabled={busy || status.active_profile === item.uid}
                aria-label={`删除订阅 ${item.name || item.uid}`}
                title={
                  status.active_profile === item.uid
                    ? "请先使用其他订阅"
                    : undefined
                }
                onClick={() => setDeleting(item.uid)}
              >
                删除
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
          busy={busy}
          perform={perform}
          onClose={() => setEditing(null)}
        />
      )}
      <section className="panel">
        <h2>下载远程订阅</h2>
        <p className="muted">
          由服务下载并保存
          YAML。刷新当前订阅时会校验并应用新配置，失败时保留原配置。
        </p>
        <form onSubmit={importRemote}>
          <label>
            订阅链接
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
            远程订阅名称（可选）
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
            通过托管内核代理下载
          </label>
          <label className="check-label">
            <input type="checkbox" disabled={busy} checked={remoteWithProxy} onChange={(event) => setRemoteWithProxy(event.target.checked)} />
            使用服务系统代理下载
          </label>
          <p className="muted">
            托管内核模式优先，需要运行中的 HTTP 或 Mixed 入口。
            系统代理读取服务环境，未配置时直连；代理连接失败会报错。
          </p>
          <button className="primary" disabled={busy || !remoteUrl.trim()}>
            下载并导入
          </button>
        </form>
      </section>
      <section className="panel">
        <h2>导入订阅</h2>
        <p className="muted">
          导入只保存内容。点击“使用订阅”后，会校验并应用配置。
        </p>
        <form onSubmit={submit}>
          <label>
            上传订阅 YAML
            <input
              type="file"
              accept=".yaml,.yml,text/yaml,text/plain"
              disabled={busy}
              onChange={(event) => void upload(event.target.files?.[0])}
            />
          </label>
          <label>
            订阅名称
            <input
              value={name}
              required
              maxLength={256}
              disabled={busy}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          <label>
            订阅 YAML
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
              {error}
            </p>
          )}
          <button
            className="primary"
            disabled={busy || !yaml.trim() || !name.trim()}
          >
            导入订阅
          </button>
        </form>
      </section>
    </div>
  );
}

function ConfigPage({
  token,
  busy,
  perform,
}: {
  token: string;
  busy: boolean;
  perform: Perform;
}) {
  const [yaml, setYaml] = useState(""),
    [loading, setLoading] = useState(true),
    [message, setMessage] = useState(""),
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
          setMessage(
            describe(error).includes("no committed configuration")
              ? "尚无已提交配置，可粘贴 YAML 后应用。"
              : describe(error),
          );
        }
      });
    return () => controller.abort();
  }, [token]);
  async function apply(event: FormEvent) {
    event.preventDefault();
    const result = await perform<CoreStatus>("edit_config", { yaml });
    if (result) {
      setDirty(false);
      setMessage("此配置已通过校验并提交。");
    }
  }
  return (
    <section className="panel">
      <div className="panel-title">
        <h2>运行配置</h2>
        <span>{dirty ? "有未应用的修改" : "完整 YAML"}</span>
      </div>
      <p className="muted">
        编辑已提交的运行配置。应用前会执行内核校验；校验或应用失败会显示错误。
      </p>
      {message && <p className="info">{message}</p>}
      <form onSubmit={apply}>
        <label>
          运行配置 YAML
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
          <p className="hint">离开此页面会丢弃尚未应用的编辑。</p>
          <button
            className="primary"
            disabled={loading || busy || !yaml.trim()}
          >
            校验并应用
          </button>
        </div>
      </form>
    </section>
  );
}

function ProxyPage({
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
  const [proxies, setProxies] = useState<Proxies>(),
    [error, setError] = useState(""),
    [revision, refresh] = useState(0),
    [loading, setLoading] = useState(false);
  useEffect(() => {
    if (status.phase !== "running") {
      setProxies(undefined);
      return;
    }
    const controller = new AbortController();
    setLoading(true);
    setError("");
    command<Proxies>(token, "proxies", {}, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) {
          setProxies(value);
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
  const groups = Object.entries(proxies?.proxies || {}).filter(([, group]) =>
    ["Selector", "URLTest", "Fallback", "LoadBalance"].includes(group.type),
  );
  async function select(name: string, fields: Record<string, unknown>) {
    await perform(name, fields);
    refresh((value) => value + 1);
  }
  return (
    <>
      <div className="section-title">
        <p className="muted">
          节点选择按订阅保存，内核与服务重启后会尝试恢复。
        </p>
        <button
          disabled={busy || loading || status.phase !== "running"}
          onClick={() => refresh((value) => value + 1)}
        >
          刷新节点
        </button>
      </div>
      {status.phase !== "running" && (
        <p className="info">启动内核后可查看节点。</p>
      )}
      {!status.active_profile && (
        <p className="info">先使用一个订阅，才能保存节点选择。</p>
      )}
      {error && (
        <p className="alert" role="alert">
          {error}
        </p>
      )}
      {loading && <p className="info">加载节点…</p>}
      {groups.map(([name, group]) => (
        <section className="panel" key={name}>
          <div className="panel-title">
            <div>
              <h2>{name}</h2>
              <p className="muted">
                {group.type} · 当前：
                {group.fixed || group.now || "等待分组加载"}
              </p>
            </div>
            {group.type !== "Selector" && (
              <button
                disabled={busy || !status.active_profile}
                onClick={() => void select("unfix_node", { group: name })}
              >
                取消固定
              </button>
            )}
          </div>
          <div className="nodes">
            {group.all?.map((node) => (
              <button
                key={node}
                aria-label={`选择 ${name} / ${node}`}
                aria-pressed={(group.fixed || group.now) === node}
                className={
                  (group.fixed || group.now) === node ? "selected" : ""
                }
                disabled={busy || !status.active_profile}
                onClick={() =>
                  void select("select_node", { group: name, node })
                }
              >
                {node}
                <span>
                  {(group.fixed || group.now) === node ? "已选择" : "选择"}
                </span>
              </button>
            ))}
          </div>
          {!group.all?.length && (
            <p className="empty">分组尚未加载节点，请稍后刷新。</p>
          )}
        </section>
      ))}
    </>
  );
}

function LogLines({ logs }: { logs: CoreLog[] }) {
  return (
    <div className="log-lines" role="log" aria-label="内核日志">
      {logs.length ? (
        logs.map((log, index) => (
          <div key={index}>
            <span>{log.stream}</span>
            <code>{log.message}</code>
          </div>
        ))
      ) : (
        <p className="empty">暂无日志。内核启动后，输出会显示在这里。</p>
      )}
    </div>
  );
}
function LogPage({ logs }: { logs: CoreLog[] }) {
  const [filter, setFilter] = useState("");
  return (
    <section className="panel">
      <div className="panel-title">
        <div>
          <h2>内核日志</h2>
          <p className="muted">
            最近 200 条输出，实时更新。重连后重新读取日志尾部。
          </p>
        </div>
        <input
          aria-label="筛选日志"
          placeholder="筛选日志…"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
      </div>
      <LogLines
        logs={logs.filter((log) =>
          log.message.toLowerCase().includes(filter.toLowerCase()),
        )}
      />
    </section>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
