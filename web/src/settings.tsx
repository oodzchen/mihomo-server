import { HelpTip } from "./help-tip";
import { useToast } from "./toast";
import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { ApiError, command, type Perform, type Connection } from "./api";
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
import { GEO_KEYS, GeoFields, GeoReadback, geoDraft, geoRuntime, validateGeo } from "./geo-settings";
import { OUTBOUND_KEYS, OutboundFields, outboundDraft, outboundRuntime, validateOutbound } from "./outbound-settings";
import { DOWNLOAD_KEYS, DownloadFields, downloadDraft, downloadRuntime, validateDownload } from "./download-settings";
import { HostsFields, hostsDraft, hostsRuntime, validateHosts } from "./hosts-settings";
import { SettingsReadback } from "./settings-readback";
import { ResourcesPanel } from "./resources";
import {
  AUTHORITY_KEYS,
  AuthorityFields,
  authorityDraft,
  authorityRuntime,
  validateAuthority,
} from "./authority-settings";
import { t, type Language } from "./i18n";
import { LanguagePicker } from "./language-picker";

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
  { key: "tcp-concurrent", label: "TCP 并发连接", kind: "bool" },
  { key: "find-process-mode", label: "进程匹配模式", kind: "select", options: [["strict", "strict（按需）"], ["always", "always（始终）"], ["off", "off（关闭）"]] },
  { key: "keep-alive-interval", label: "TCP 保活间隔（秒）", kind: "seconds" },
  { key: "keep-alive-idle", label: "TCP 保活空闲时间（秒）", kind: "seconds" },
  { key: "disable-keep-alive", label: "禁用 TCP 保活", kind: "bool" },
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
    ...Object.entries(geoDraft(settings.runtime)),
    ...Object.entries(outboundDraft(settings.runtime)),
    ...Object.entries(downloadDraft(settings.runtime)),
    ...Object.entries(hostsDraft(settings.runtime)),
    ...Object.entries(authorityDraft(settings.runtime)),
    ...fields.map((field) => [
      field.key,
      settings.runtime[field.key] == null
        ? ""
        : String(settings.runtime[field.key]),
    ]),
  ]);
}
function runtime(draft: Draft): Runtime {
  const result: Runtime = {
    ...networkRuntime(draft),
    ...geoRuntime(draft),
    ...outboundRuntime(draft),
    ...downloadRuntime(draft),
    ...hostsRuntime(draft),
    ...authorityRuntime(draft),
  };
  for (const field of fields) {
    const value = draft[field.key];
    if (value === "") continue;
    if (field.kind === "port") {
      if (!/^\d+$/.test(value) || Number(value) > 65535)
        throw new Error(`${field.label}必须是 0–65535 的整数，或留空继承。`);
      result[field.key] = Number(value);
    } else if (field.kind === "seconds") {
      if (!/^-?\d+$/.test(value) || !Number.isInteger(Number(value)) || Number(value) < -2147483648 || Number(value) > 2147483647)
        throw new Error(`${field.label}必须是 -2147483648–2147483647 的整数，或留空继承。`);
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
  validateGeo(settings.runtime);
  for (const [key, value] of Object.entries(settings.runtime)) {
    if (key === "hosts") { if (value != null) validateHosts(value); continue; }
    if (GEO_KEYS.has(key)) continue;
    if (DOWNLOAD_KEYS.has(key)) { validateDownload(key, value); continue; }
    if (OUTBOUND_KEYS.has(key)) { validateOutbound(key, value); continue; }
    if (AUTHORITY_KEYS.has(key)) { validateAuthority(key, value); continue; }
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
        : field.kind === "seconds"
          ? typeof value !== "number" || !Number.isInteger(value) || value < -2147483648 || value > 2147483647
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

export type SettingsPageHandle = {
  save: () => Promise<boolean>;
  discard: () => void;
};

type SettingsPageProps = {
  token: string;
  language?: Language;
  changeLanguage?: (value: string) => void;
  status: CoreStatus;
  connection: Connection;
  busy: boolean;
  perform: Perform;
  logout: (reason?: string) => void;
  onEditorStateChange: (dirty: boolean, busy: boolean) => void;
};

export const SettingsPage = forwardRef<SettingsPageHandle, SettingsPageProps>(function SettingsPage({
  token,
  language = "zh",
  changeLanguage,
  status,
  connection,
  busy,
  perform,
  logout,
  onEditorStateChange,
}, ref) {
  const access = useProxyAccess({ token, status, connection, logout });
  const [saved, setSaved] = useState<Settings>();
  const [draft, setDraft] = useState<Draft>({});
  const [working, setWorking] = useState(false),
    [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState("");
  const notify = useToast();
  const [confirmation, setConfirmation] = useState<"reload">();
  const alive = useRef(true),
    requests = useRef(new Set<AbortController>());
  const saving = useRef(false);
  const draftRef = useRef(draft), savedRef = useRef(saved), uncertainRef = useRef(uncertain);
  draftRef.current = draft; savedRef.current = saved; uncertainRef.current = uncertain;
  const disabled = busy || working;
  // Keep the next field editable while this form's transaction is in flight.
  const fieldsDisabled = disabled && !saving.current;
  function change(key: string, value: string) {
    draftRef.current = { ...draftRef.current, [key]: value };
    setDraft(draftRef.current);
    setError(""); setConfirmation(undefined);
  }
  function changeTun(value: string) {
    const next: Draft = { ...draftRef.current, "tun.enable": value };
    if (value !== "" && next.tun !== "true") next.tun = "true";
    draftRef.current = next;
    setDraft(next);
    setError(""); setConfirmation(undefined);
  }
  const dirty = saved ? !matches(draft, saved) : false;

  function portDescription(key: string) {
    if (connection !== "connected") return "服务连接中断，当前端口待核对。";
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
    if (!port || !access.value?.has_config || connection !== "connected") return "留空继承";
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
    setConfirmation(undefined);
    try {
      const next = await read();
      if (alive.current) {
        savedRef.current = next; setSaved(next);
        draftRef.current = toDraft(next); setDraft(draftRef.current);
        uncertainRef.current = false; setUncertain(false);
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

  async function save(submitted = { ...draftRef.current }): Promise<boolean> {
    if (!savedRef.current || busy || working || saving.current) return false;
    let requested: Runtime;
    try {
      requested = runtime(submitted);
    } catch (error) {
      setError(explain(error));
      return false;
    }
    if (!uncertainRef.current && same(requested, savedRef.current.runtime)) return true;
    saving.current = true;
    setWorking(true);
    setError(""); setConfirmation(undefined);
    const toast = notify.loading(t(language, "working"));
    try {
      // A failed readback is reconciled automatically before another write.
      if (uncertainRef.current) {
        const next = await read();
        if (!alive.current) return false;
        savedRef.current = next; setSaved(next);
        uncertainRef.current = false; setUncertain(false);
      }
      if (same(requested, savedRef.current.runtime)) {
        toast.finish(t(language, "saveSuccess"), "info");
        return true;
      }
      uncertainRef.current = true; setUncertain(true);
      const result = await perform<Settings>("set_settings", { runtime: requested }, { notify: false, toast, reconcile: true });
      if (!alive.current) return false;
      const next = await read();
      if (!alive.current) return false;
      savedRef.current = next; setSaved(next);
      uncertainRef.current = false; setUncertain(false);
      if (same(requested, next.runtime)) {
        // Normalize only submitted values that have not been edited again.
        const normalized = toDraft(next);
        const merged = { ...draftRef.current };
        for (const key of Object.keys(normalized)) {
          if (merged[key] === submitted[key]) merged[key] = normalized[key];
        }
        draftRef.current = merged; setDraft(merged);
        access.refresh();
        toast.finish(t(language, "saveSuccess"), result ? "success" : "info");
        return true;
      } else {
        const message = "服务当前设置与提交内容不同，草稿已保留。请检查错误或重新读取设置。";
        setError(message); toast.finish(message, "error");
        return false;
      }
    } catch (error) {
      if (alive.current) {
        const message = `保存结果尚未核对：${explain(error)}。草稿已保留，请修正后再次保存。`;
        setError(message); toast.finish(explain(error), "error");
      }
      return false;
    } finally {
      if (!alive.current) toast.dismiss();
      saving.current = false;
      if (alive.current) setWorking(false);
    }
  }

  function discard() {
    if (!savedRef.current) return;
    draftRef.current = toDraft(savedRef.current);
    setDraft(draftRef.current);
    uncertainRef.current = false;
    setUncertain(false);
    setError("");
    setConfirmation(undefined);
  }

  useImperativeHandle(ref, () => ({ save, discard }));
  useEffect(() => {
    onEditorStateChange(dirty || uncertain, disabled);
  }, [dirty, uncertain, disabled, onEditorStateChange]);
  useEffect(() => () => onEditorStateChange(false, true), [onEditorStateChange]);

  return (
    <div className="settings-layout">
      <section className="panel" aria-label="服务设置编辑器">
        <section className="tun-control tun-setting" aria-label={language === "en" ? "TUN mode" : "TUN 模式"}>
          <div className="tun-setting-row">
            <label htmlFor="setting-tun-mode">{language === "en" ? "TUN mode" : "TUN 模式"}</label>
            <select
              id="setting-tun-mode"
              aria-label={language === "en" ? "TUN mode" : "TUN 模式"}
              disabled={!saved || fieldsDisabled}
              value={draft.tun === "true" ? draft["tun.enable"] ?? "" : ""}
              onChange={event => changeTun(event.target.value)}
            >
              <option value="">{language === "en" ? "Inherit" : language === "zhtw" ? "繼承" : "继承"}</option>
              <option value="true">{language === "en" ? "Enabled" : language === "zhtw" ? "已開啟" : "已开启"}</option>
              <option value="false">{language === "en" ? "Disabled" : language === "zhtw" ? "已關閉" : "已关闭"}</option>
            </select>
          </div>
          <p className="hint">{language === "en" ? "Changes are applied with the Save button in the page header." : language === "zhtw" ? "變更會隨頁面標題列的儲存按鈕一併套用。" : "改动会随页面标题栏的保存按钮一并应用。"}</p>
        </section>
        <div className="panel-title">
          <h2 className="setting-heading">服务运行设置<HelpTip label="运行设置帮助">留空或选择继承时使用订阅 / 配置值，端口 0 表示禁用。修改后请使用页面标题栏右侧的保存按钮；多行文本可直接换行。未通过校验的修改会保留在本页。保存会替换全部运行设置，当前订阅会重新生成并校验。未选择订阅时更新独立运行配置，移除设置会保留其当前值，之后可在配置页修改。保存结果不确定时，再次保存前会自动核对服务设置。服务地址、管理认证和启动参数不在此编辑器中。</HelpTip></h2>
        </div>
        {!status.config_revision && (
          <p className="info">
            尚无已提交配置，保存仅记录设置，首次启动时使用。
          </p>
        )}
        {error && (
          <p className="alert settings-floating-error" role="alert">
            {error}
          </p>
        )}
        {saved && (
          <form aria-label="运行设置表单" onSubmit={event => event.preventDefault()}>
            <fieldset className="network-fields basic-settings"><legend>常用设置 <HelpTip>透明代理端口仅支持 Linux，服务会校验平台支持。</HelpTip></legend><div className="settings-fields">
              {fields.map((field) => (
                <label key={field.key} htmlFor={`setting-${field.key}`}>
                  <span className="setting-name">{field.label}<HelpTip id={`port-hint-${field.key}`} label={`${field.label}帮助`}>{field.kind === "port" ? portDescription(field.key) : field.kind === "seconds" ? "整数秒；0 使用核心默认值，负数保留系统参数（取决于核心版本）。留空继承；禁用开关优先。" : "选择继承时使用订阅或运行配置的值；启用和禁用均保存为显式设置。"}</HelpTip></span>
                  {field.kind === "port" || field.kind === "seconds" ? (
                    <>
                      <input
                        id={`setting-${field.key}`}
                        aria-label={field.label}
                        inputMode={field.kind === "port" ? "numeric" : "text"}
                        placeholder={field.kind === "port" ? portPlaceholder(field.key) : "留空继承"}
                        aria-describedby={`port-hint-${field.key}`}
                        disabled={fieldsDisabled}
                        value={draft[field.key] ?? ""}
                        onChange={event => change(field.key, event.target.value)}
                      />
                    </>
                  ) : (
                    <select
                      id={`setting-${field.key}`}
                      aria-label={field.label}
                      disabled={fieldsDisabled}
                      value={draft[field.key] ?? ""}
                      onChange={event => change(field.key, event.target.value)}
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
            </fieldset>
            <button type="button" className="port-refresh" onClick={access.refresh} disabled={connection !== "connected"}>
              刷新端口信息
            </button>
            <GeoFields draft={draft} disabled={fieldsDisabled} change={change} />
            <DownloadFields draft={draft} disabled={fieldsDisabled} onChange={change} />
            <OutboundFields draft={draft} disabled={fieldsDisabled} onChange={change} />
            <HostsFields draft={draft} disabled={fieldsDisabled} change={change} />
            <AuthorityFields draft={draft} disabled={fieldsDisabled} onChange={change} />
            <NetworkFields draft={draft} disabled={fieldsDisabled} change={change} />
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
          <div className="reset-confirmation" role="group" aria-label="设置替换确认">
            <p>重新读取会用服务当前设置替换本页未应用的修改。</p>
            <div className="actions">
              <button disabled={disabled} onClick={() => void reload()}>确认重新读取</button>
              <button disabled={disabled} onClick={() => setConfirmation(undefined)}>继续编辑设置</button>
            </div>
          </div>
        )}
      </section>
      <div className="settings-side">
        {changeLanguage && (
          <section className="panel" aria-label={t(language, "language")}>
            <div className="panel-title">
              <h2 className="setting-heading">{t(language, "language")}<HelpTip>
              {language === "en"
                ? "Select the user interface display language. It applies immediately, is saved with this instance and is shared with its other windows and the desktop client."
                : language === "zhtw"
                ? "選擇使用者介面顯示語言。選擇後立即生效，儲存在目前實例中，並同步到其他視窗和桌面端。"
                : "选择用户界面显示语言。选择后立即生效，保存在当前实例中，并同步到其他窗口和桌面端。"}
            </HelpTip></h2>
            </div>
            <LanguagePicker language={language} changeLanguage={changeLanguage} />
          </section>
        )}
        <details className="settings-details"><summary>内核实际设置</summary>
        <SettingsReadback label="连接设置读回" operation="connection_settings" hint="显示核心报告的设置，不保证已识别进程或改善连接速度。未指定项可能使用核心默认值。路由标记可能以有符号 32 位整数读回，同一位模式视为一致。" token={token} status={status} connection={connection} logout={logout} settingsKey={JSON.stringify(saved?.runtime)} />
        <GeoReadback token={token} status={status} connection={connection} logout={logout} settingsKey={JSON.stringify(saved?.runtime)} />
        </details>
        <details className="settings-details"><summary>资源与数据库</summary>
        <ResourcesPanel token={token} status={status} connection={connection} logout={logout} language={language} />
        </details>
        <ProfileDnsPanel
          key={`${token}:${status.active_profile ?? ""}`}
          token={token}
          status={status}
          connection={connection}
          hasDns={saved?.runtime.dns != null || saved?.runtime.hosts != null}
          blocked={disabled || uncertain || dirty || !saved}
          perform={perform}
          logout={logout}
        />
        <details className="settings-details"><summary>已保存服务设置</summary>
        <section className="panel" aria-label="已保存服务设置">
          <h2>已读取的服务设置</h2>
          <HelpTip>
            显示上次读取或核对的设置。继承项的实际值请在配置页查看。
          </HelpTip>
          {saved ? (
            <dl className="settings-summary">
              {fields.map((field) => (
                <div key={field.key}>
                  <dt>{field.label}</dt>
                  <dd>
                    {saved.runtime[field.key] == null
                      ? "继承"
                      : field.kind === "port" || field.kind === "seconds"
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
          {saved && <pre className="network-snapshot" aria-label="已保存 hosts 映射">{saved.runtime.hosts == null ? "继承" : JSON.stringify(saved.runtime.hosts, null, 2)}</pre>}
          {saved && <pre className="network-snapshot" aria-label="已保存核心下载设置">{JSON.stringify(Object.fromEntries([...DOWNLOAD_KEYS].map(key => [key, saved.runtime[key]])), null, 2)}</pre>}
          {saved && <pre className="network-snapshot" aria-label="已保存出口设置">{JSON.stringify(Object.fromEntries([...OUTBOUND_KEYS].map(key => [key, saved.runtime[key]])), null, 2)}</pre>}
          {saved && <pre className="network-snapshot" aria-label="已保存监听与访问控制设置">{JSON.stringify(Object.fromEntries([...AUTHORITY_KEYS].map(key => [key, saved.runtime[key]])), null, 2)}</pre>}
          {saved && <pre className="network-snapshot" aria-label="已保存 Geo 设置">{JSON.stringify(Object.fromEntries([...GEO_KEYS].map(key => [key, saved.runtime[key]])), null, 2)}</pre>}
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
        </details>
      </div>
    </div>
  );
});
