import { useEffect, useRef, useState, type FormEvent } from "react";
import { ApiError, command, type Perform } from "./api";
import type { CoreStatus } from "./types";
import {
  NetworkFields,
  networkDraft,
  networkRuntime,
  validateNetwork,
  type Runtime,
  type Draft,
} from "./network-settings";
import { ProfileDnsPanel } from "./profile-dns";
import { useProxyAccess } from "./proxy-access";

type Settings = { schema_version: number; runtime: Runtime };
const fields = [
  { key: "mixed-port", label: "混合端口", kind: "port" },
  { key: "socks-port", label: "SOCKS 端口", kind: "port" },
  { key: "port", label: "HTTP 端口", kind: "port" },
  { key: "redir-port", label: "重定向端口", kind: "port" },
  { key: "tproxy-port", label: "透明代理端口", kind: "port" },
  {
    key: "mode",
    label: "代理模式",
    kind: "select",
    options: [
      ["rule", "规则"],
      ["global", "全局"],
      ["direct", "直连"],
    ],
  },
  { key: "allow-lan", label: "允许局域网访问", kind: "bool" },
  { key: "ipv6", label: "IPv6", kind: "bool" },
  { key: "unified-delay", label: "统一延迟", kind: "bool" },
  {
    key: "log-level",
    label: "日志等级",
    kind: "select",
    options: [
      ["silent", "静默"],
      ["error", "错误"],
      ["warning", "警告"],
      ["info", "信息"],
      ["debug", "调试"],
    ],
  },
] as const;

function options(
  field: (typeof fields)[number],
): readonly (readonly [string, string])[] {
  return field.kind === "bool"
    ? [
        ["true", "启用"],
        ["false", "禁用"],
      ]
    : field.kind === "select"
      ? field.options
      : [];
}
function toDraft(settings: Settings): Draft {
  return Object.fromEntries([
    ...Object.entries(networkDraft(settings.runtime)),
    ...fields.map((field) => [
      field.key,
      settings.runtime[field.key] == null
        ? ""
        : String(settings.runtime[field.key]),
    ]),
  ]);
}
function runtime(draft: Draft): Runtime {
  const result: Runtime = networkRuntime(draft);
  for (const field of fields) {
    const value = draft[field.key];
    if (value === "") continue;
    if (field.kind === "port") {
      if (!/^\d+$/.test(value) || Number(value) > 65535)
        throw new Error(`${field.label}必须是 0–65535 的整数，或留空继承。`);
      result[field.key] = Number(value);
    } else {
      if (!options(field).some(([option]) => value === option))
        throw new Error(`${field.label}值无效。`);
      result[field.key] = field.kind === "bool" ? value === "true" : value;
    }
  }
  return result;
}
function decode(value: unknown): Settings {
  const settings = value as Settings;
  if (
    !settings ||
    settings.schema_version !== 1 ||
    !settings.runtime ||
    typeof settings.runtime !== "object" ||
    Array.isArray(settings.runtime)
  )
    throw new Error("无法编辑此设置版本，请检查服务版本。");
  for (const [key, value] of Object.entries(settings.runtime)) {
    if (key === "dns" || key === "tun") {
      validateNetwork(key, value);
      continue;
    }
    const field = fields.find((field) => field.key === key);
    if (!field)
      throw new Error(`服务包含暂不支持的设置 ${key}，请勿用此编辑器覆盖。`);
    if (value == null) continue;
    if (
      field.kind === "port"
        ? typeof value !== "number" ||
          !Number.isInteger(value) ||
          value < 0 ||
          value > 65535
        : field.kind === "bool"
          ? typeof value !== "boolean"
          : typeof value !== "string" ||
            !options(field).some(([option]) => value === option)
    )
      throw new Error(`服务返回的${field.label}值无效。`);
  }
  // Normalize null inheritance and field order before comparing full replacements.
  return { schema_version: 1, runtime: runtime(toDraft(settings)) };
}
function same(left: Runtime, right: Runtime): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
function matches(draft: Draft, saved: Settings): boolean {
  try {
    return same(runtime(draft), saved.runtime);
  } catch {
    return false;
  }
}
const explain = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

export function SettingsPage({
  token,
  status,
  connection,
  busy,
  perform,
  logout,
}: {
  token: string;
  status: CoreStatus;
  connection: string;
  busy: boolean;
  perform: Perform;
  logout: (reason?: string) => void;
}) {
  const access = useProxyAccess({ token, status, connection, logout });
  const [saved, setSaved] = useState<Settings>();
  const [draft, setDraft] = useState<Draft>({});
  const [working, setWorking] = useState(false),
    [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const [confirmation, setConfirmation] = useState<"reload" | "clear">();
  const alive = useRef(true),
    requests = useRef(new Set<AbortController>());
  const disabled = busy || working;
  const dirty = saved ? !matches(draft, saved) : false;

  function portDescription(key: string) {
    if (connection !== "已连接") return "服务连接中断，当前端口待核对。";
    if (access.error) return "读取当前端口失败，请刷新端口信息。";
    const port = access.value?.ports.find(port => port.key === key);
    if (!port) return "正在读取当前端口…";
    const source = port.setting === null ? "继承订阅 / 配置" : "服务设置";
    if (!access.value?.has_config) return "尚无运行配置，当前端口未确定。";
    if (port.actual === null) return `配置端口：${port.configured || "禁用（0）"} · ${source} · ${access.value.running ? "内核实际端口未确认" : "内核未运行"}`;
    if (port.actual !== port.configured) return `当前端口：${port.actual || "未监听（0）"} · 配置端口：${port.configured} · ${source} · 与配置不一致，请检查端口占用和日志。`;
    return `当前端口：${port.actual || "禁用（0）"} · ${source}`;
  }

  function portPlaceholder(key: string) {
    const port = access.value?.ports.find(port => port.key === key);
    if (!port || !access.value?.has_config || connection !== "已连接") return "留空继承";
    if (port.actual !== null && port.actual === port.configured)
      return port.actual ? `继承当前端口 ${port.actual}` : "留空继承（当前禁用）";
    return port.configured ? `继承配置端口 ${port.configured}` : "留空继承（配置禁用）";
  }

  async function read(): Promise<Settings> {
    const controller = new AbortController();
    requests.current.add(controller);
    try {
      return decode(
        await command<unknown>(token, "settings", {}, controller.signal),
      );
    } catch (error) {
      if (alive.current && error instanceof ApiError && error.status === 401)
        logout("认证失效，请重新输入令牌。");
      throw error;
    } finally {
      requests.current.delete(controller);
    }
  }
  async function reload() {
    setWorking(true);
    setError("");
    setNotice("");
    setConfirmation(undefined);
    try {
      const next = await read();
      if (alive.current) {
        setSaved(next);
        setDraft(toDraft(next));
        setUncertain(false);
      }
    } catch (error) {
      if (alive.current) {
        if (saved) setUncertain(true);
        setError(`读取设置失败：${explain(error)}。草稿未被替换。`);
      }
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  useEffect(() => {
    alive.current = true;
    void reload();
    return () => {
      alive.current = false;
      requests.current.forEach((controller) => controller.abort());
    };
  }, [token]);

  async function verify() {
    setWorking(true);
    setUncertain(true);
    setError("");
    setNotice("");
    try {
      const next = await read();
      if (alive.current) {
        setSaved(next);
        setUncertain(false);
        setNotice(
          matches(draft, next)
            ? "已核对：服务已保存当前草稿。"
            : "已核对：服务当前设置与草稿不同，草稿已保留。请修正或重新读取后再保存。",
        );
      }
    } catch (error) {
      if (alive.current)
        setError(`核对设置失败：${explain(error)}。请核对已保存设置后再提交。`);
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  async function save(event: FormEvent) {
    event.preventDefault();
    setError("");
    setNotice("");
    setConfirmation(undefined);
    let requested: Runtime;
    try {
      requested = runtime(draft);
    } catch (error) {
      setError(explain(error));
      return;
    }
    setWorking(true);
    setUncertain(true);
    const result = await perform<Settings>("set_settings", {
      runtime: requested,
    });
    if (!alive.current) return;
    try {
      const next = await read();
      if (!alive.current) return;
      setSaved(next);
      setUncertain(false);
      if (same(requested, next.runtime)) {
        if (result) setDraft(toDraft(next));
        setNotice(
          result
            ? "保存结果已核对，服务设置与提交内容一致。"
            : "请求报告错误，但服务已保存此草稿，已核对，无需重复提交。",
        );
      } else
        setNotice(
          "服务当前设置与提交内容不同，草稿已保留。请检查错误或重新读取设置。",
        );
    } catch (error) {
      if (alive.current)
        setError(
          `保存结果尚未核对：${explain(error)}。草稿已保留，请先核对已保存设置。`,
        );
    } finally {
      if (alive.current) setWorking(false);
    }
  }

  return (
    <div className="settings-layout">
      <section className="panel" aria-label="服务设置编辑器">
        <div className="panel-title">
          <h2>服务运行设置</h2>
          <span>
            {uncertain
              ? "服务设置待核对"
              : dirty
                ? "有未保存的修改"
                : saved
                  ? "已读取服务设置"
                  : "尚未读取设置"}
          </span>
        </div>
        <p className="muted">
          端口留空或选择「继承」时，由订阅或运行配置提供值。0
          表示禁用端口，「禁用」会保存为显式设置。
        </p>
        <p className="hint">
          保存将替换全部运行设置。当前订阅会重新生成并校验；已停止的内核保持停止。未选择订阅时更新独立运行配置，移除设置会保留其当前值，之后可在配置页修改。
        </p>
        {!status.config_revision && (
          <p className="info">
            尚无已提交配置，保存仅记录设置，首次启动时使用。
          </p>
        )}
        {error && (
          <p className="alert" role="alert">
            {error}
          </p>
        )}
        {notice && (
          <p className="info" role="status">
            {notice}
          </p>
        )}
        {working && (
          <p className="muted" role="status">
            正在读取或核对设置…
          </p>
        )}
        {saved && (
          <form aria-label="运行设置表单" onSubmit={save}>
            <div className="settings-fields">
              {fields.map((field) => (
                <label key={field.key}>
                  {field.label}
                  {field.kind === "port" ? (
                    <>
                      <input
                        aria-label={field.label}
                        inputMode="numeric"
                        placeholder={portPlaceholder(field.key)}
                        aria-describedby={`port-hint-${field.key}`}
                        disabled={disabled}
                        value={draft[field.key] ?? ""}
                        onChange={(event) => {
                          setDraft((previous) => ({
                            ...previous,
                            [field.key]: event.target.value,
                          }));
                          setError("");
                          setNotice("");
                          setConfirmation(undefined);
                        }}
                      />
                      <span id={`port-hint-${field.key}`} className={`port-field-hint${access.value?.ports.some(port => port.key === field.key && port.actual !== null && port.actual !== port.configured) ? " port-mismatch" : ""}`}>
                        {portDescription(field.key)}
                      </span>
                    </>
                  ) : (
                    <select
                      aria-label={field.label}
                      disabled={disabled}
                      value={draft[field.key] ?? ""}
                      onChange={(event) => {
                        setDraft((previous) => ({
                          ...previous,
                          [field.key]: event.target.value,
                        }));
                        setError("");
                        setNotice("");
                        setConfirmation(undefined);
                      }}
                    >
                      <option value="">继承</option>
                      {options(field).map(([value, label]) => (
                        <option key={value} value={value}>
                          {label}
                        </option>
                      ))}
                    </select>
                  )}
                </label>
              ))}
            </div>
            <button type="button" className="port-refresh" onClick={access.refresh} disabled={connection !== "已连接"}>
              刷新端口信息
            </button>
            <NetworkFields
              draft={draft}
              disabled={disabled}
              change={(key, value) => {
                setDraft((previous) => ({ ...previous, [key]: value }));
                setError("");
                setNotice("");
                setConfirmation(undefined);
              }}
            />
            <p className="hint">
              透明代理端口仅支持 Linux，重定向端口不支持
              Windows。服务会校验平台支持。
            </p>
            <div className="actions">
              <button
                className="primary"
                disabled={disabled || uncertain || !dirty}
              >
                保存服务设置
              </button>
              <button
                type="button"
                disabled={disabled || uncertain}
                onClick={() => setConfirmation("clear")}
              >
                全部改为继承
              </button>
              <button
                type="button"
                disabled={disabled}
                onClick={() => void verify()}
              >
                核对已保存设置
              </button>
            </div>
          </form>
        )}
        <div className="actions settings-reload">
          <button
            disabled={disabled}
            onClick={() =>
              dirty || uncertain ? setConfirmation("reload") : void reload()
            }
          >
            {saved ? "重新读取设置" : "重试读取设置"}
          </button>
        </div>
        {confirmation && (
          <div
            className="reset-confirmation"
            role="group"
            aria-label="设置替换确认"
          >
            <p>
              {confirmation === "reload"
                ? "重新读取会用服务当前设置替换本页草稿。"
                : "将全部字段改为继承。此操作仅修改草稿，保存后才生效。"}
            </p>
            <div className="actions">
              <button
                disabled={disabled}
                onClick={() =>
                  confirmation === "reload"
                    ? void reload()
                    : (setDraft(toDraft({ schema_version: 1, runtime: {} })),
                      setConfirmation(undefined),
                      setNotice("已将草稿改为继承，保存后生效。"))
                }
              >
                {confirmation === "reload" ? "确认重新读取" : "确认改为继承"}
              </button>
              <button
                disabled={disabled}
                onClick={() => setConfirmation(undefined)}
              >
                继续编辑设置
              </button>
            </div>
          </div>
        )}
        <p className="hint">
          离开此页会丢弃草稿。保存结果不确定时，先核对服务设置再重试。服务地址、认证及
          服务启动参数不在此编辑器中。
        </p>
      </section>
      <div className="settings-side">
        <ProfileDnsPanel
          key={`${token}:${status.active_profile ?? ""}`}
          token={token}
          status={status}
          connection={connection}
          hasDns={saved?.runtime.dns != null}
          blocked={disabled || uncertain || dirty || !saved}
          perform={perform}
          logout={logout}
        />
        <section className="panel" aria-label="已保存服务设置">
          <h2>已读取的服务设置</h2>
          <p className="muted">
            显示上次读取或核对的设置。继承项的实际值请在配置页查看。
          </p>
          {saved ? (
            <dl className="settings-summary">
              {fields.map((field) => (
                <div key={field.key}>
                  <dt>{field.label}</dt>
                  <dd>
                    {saved.runtime[field.key] == null
                      ? "继承"
                      : field.kind === "port"
                        ? String(saved.runtime[field.key])
                        : options(field).find(
                            ([value]) =>
                              value === String(saved.runtime[field.key]),
                          )?.[1]}
                  </dd>
                </div>
              ))}
            </dl>
          ) : (
            <p className="muted">尚未读取到设置。</p>
          )}
          {saved && (
            <pre className="network-snapshot" aria-label="已保存网络设置">
              {JSON.stringify(
                { dns: saved.runtime.dns, tun: saved.runtime.tun },
                null,
                2,
              )}
            </pre>
          )}
        </section>
      </div>
    </div>
  );
}
