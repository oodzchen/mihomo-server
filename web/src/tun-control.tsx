import { useEffect, useRef, useState, type ReactNode } from "react";
import { ApiError, command, type Connection, type Perform } from "./api";
import { HelpTip } from "./help-tip";
import { phaseLabel, t, type Language } from "./i18n";
import type { Access, ProxyAccessState } from "./proxy-access";
import { useToast } from "./toast";
import type { CoreStatus } from "./types";

export function TunControl({ token, language, status, connection, access, busy, perform, logout, blocked, onChanged, extra, compact = false }: {
  token: string; language: Language; status: CoreStatus; connection: Connection;
  access: ProxyAccessState; busy: boolean; perform: Perform; logout: (reason?: string) => void;
  blocked?: boolean; onChanged?: () => Promise<void>; extra?: ReactNode; compact?: boolean;
}) {
  const text = language === "en" ? {
    title: "TUN mode", on: "Enabled", off: "Disabled", unknown: "Unconfirmed", working: "Applying…",
    help: "Capture traffic through a virtual network adapter. This switch saves and applies immediately; advanced TUN options are preserved.",
    stopped: "Saved setting; takes effect when the core starts.", blocked: "Apply or correct your pending settings before changing TUN.",
    failed: "TUN state could not be confirmed. Check the error and retry.", verified: "TUN", expired: "Authentication expired. Please sign in again.",
    held: "{name} is using the system-wide TUN, so your traffic follows their rules. Turning TUN on takes it over.",
    heldShort: "In use by {name}; turning it on takes it over",
    takenOver: "{name} turned on TUN and took over the system-wide TUN.",
    authorize: "TUN needs a one-time administrator authorization. Turn TUN on in the desktop client, or run mihomo-server tun on in a terminal on this computer.",
    authorizing: "Approve the TUN authorization in the dialog…",
  } : language === "zhtw" ? {
    title: "TUN 模式", on: "已開啟", off: "已關閉", unknown: "未確認", working: "正在套用…",
    help: "透過虛擬網卡接管流量。點擊開關立即儲存並套用，保留其他 TUN 進階參數。",
    stopped: "目前為已儲存設定，核心啟動後生效。", blocked: "請先套用或修正尚未生效的設定，再變更 TUN。",
    failed: "TUN 狀態尚未確認，請查看錯誤後重試。", verified: "TUN", expired: "認證失效，請重新登入。",
    held: "整機 TUN 正由 {name} 使用，你的流量目前按其規則處理；開啟 TUN 即可接管。",
    heldShort: "{name} 使用中，開啟即可接管",
    takenOver: "{name} 開啟了 TUN，已接管整機 TUN。",
    authorize: "TUN 需要一次性管理員授權：請在桌面用戶端中開啟 TUN，或在這台電腦的終端機執行 mihomo-server tun on。",
    authorizing: "請在彈出的對話框中完成 TUN 授權…",
  } : {
    title: "TUN 模式", on: "已开启", off: "已关闭", unknown: "未确认", working: "正在应用…",
    help: "通过虚拟网卡接管流量。点击开关立即保存并应用，保留其他 TUN 高级参数。",
    stopped: "当前为已保存设置，内核启动后生效。", blocked: "请先应用或修正尚未生效的设置，再更改 TUN。",
    failed: "TUN 状态尚未确认，请查看错误后重试。", verified: "TUN", expired: "认证失效，请重新登录。",
    held: "整机 TUN 正由 {name} 使用，你的流量目前按其规则处理；开启 TUN 即可接管。",
    heldShort: "{name} 使用中，开启即可接管",
    takenOver: "{name} 开启了 TUN，已接管整机 TUN。",
    authorize: "TUN 需要一次性管理员授权：请在桌面客户端中开启 TUN，或在这台电脑的终端运行 mihomo-server tun on。",
    authorizing: "请在弹出的对话框中完成 TUN 授权…",
  };
  const [pending, setPending] = useState<boolean>();
  const notify = useToast();
  const [working, setWorking] = useState(false), [error, setError] = useState("");
  const alive = useRef(true), locked = useRef(false), request = useRef<AbortController | undefined>(undefined);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; request.current?.abort(); };
  }, [token]);
  const running = status.phase === "running";
  const inactive = status.phase === "stopped" || status.phase === "failed";
  const current = connection !== "connected" || !access.value?.has_config ? undefined
    : running && access.value.running ? access.value.reported?.tun_enabled
    : inactive && !access.value.running ? access.value.tun_enabled : undefined;
  // Another account holds the host's one system-wide TUN; turning TUN on here takes it over.
  const holder = access.value?.tun_holder;
  const heldElsewhere = !!holder && !holder.self && current !== true;
  const disabled = busy || working || blocked || current === undefined;
  // This user's TUN was turned off because another account turned theirs on
  // (unknown while the core restarts for that, so the last known state counts).
  const previous = useRef(current);
  useEffect(() => {
    if (current === undefined) return;
    if (previous.current === true && current === false && heldElsewhere && !locked.current)
      notify(text.takenOver.replace("{name}", holder.name), "info");
    previous.current = current;
  }, [current, heldElsewhere, holder?.name, notify, text.takenOver]);

  /** Not in the TUN group: only the desktop client can ask polkit (the service has no session). */
  async function authorize() {
    const desktop = window.__MIHOMO_DESKTOP_VERSION__ ? window.__TAURI_INTERNALS__ : undefined;
    if (!desktop) { notify(text.authorize, "info"); return; }
    locked.current = true;
    setWorking(true); setError("");
    const toast = notify.loading(text.authorizing);
    try {
      await desktop.invoke("authorize_tun");
      if (alive.current) toast.finish(`${text.verified}${t(language, "colon")}${text.on}`, "success");
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      // A client from before this command: point to the ways that work there.
      if (/not (allowed|found)/i.test(message)) toast.finish(text.authorize, "info");
      else toast.finish(message, "error");
    } finally {
      locked.current = false;
      if (alive.current) { setWorking(false); setPending(undefined); access.refresh(); }
    }
  }

  async function toggle(enabled = !current) {
    if (disabled || locked.current || current === undefined) return;
    if (enabled && access.value?.tun_capable === false) return authorize();
    locked.current = true;
    setWorking(true); setError("");
    const toast = notify.loading(text.working);
    try {
      const result = await perform("set_tun_enabled", { enabled }, { notify: false, toast, reconcile: true });
      if (!alive.current) return;
      // Re-read the settings editor too, so a later full save cannot undo this toggle.
      await onChanged?.();
      if (!alive.current) return;
      if (result === undefined) throw new Error(text.failed);
      const deadline = Date.now() + 6000;
      let confirmed = false;
      while (Date.now() < deadline && alive.current) {
        const controller = new AbortController();
        request.current = controller;
        try {
          const [saved, observed] = await Promise.all([
            command<{ runtime: { tun?: { enable?: boolean } } }>(token, "settings", {}, controller.signal),
            command<Access>(token, "proxy_access", {}, controller.signal),
          ]);
          if (!alive.current) return;
          const settingsMatch = saved.runtime.tun?.enable === enabled;
          const accessMatch = observed.tun_enabled === enabled;
          const runtimeMatch = !observed.running || observed.reported?.tun_enabled === enabled;
          if (settingsMatch && accessMatch && runtimeMatch) {
            confirmed = true;
            break;
          }
        } catch {
          // Ignore transient errors while core restarts/reloads
        }
        await new Promise(resolve => setTimeout(resolve, 250));
      }
      if (!alive.current) return;
      if (!confirmed) throw new Error(text.failed);
      toast.finish(`${text.verified}${t(language, "colon")}${enabled ? text.on : text.off}`, "success");
    } catch (cause) {
      if (!alive.current) return;
      if (cause instanceof ApiError && cause.status === 401) logout(text.expired);
      else {
        const message = cause instanceof Error ? cause.message : String(cause);
        setError(message);
        toast.finish(message, "error");
      }
    } finally {
      if (!alive.current) toast.dismiss();
      locked.current = false;
      if (alive.current) { setWorking(false); setPending(undefined); access.refresh(); }
    }
  }

  if (compact) return <section className="tun-control tun-setting" aria-label={text.title}>
    <div className="tun-setting-row">
      <label htmlFor="setting-tun-mode">{text.title}</label>
      <select id="setting-tun-mode" aria-label={text.title} disabled={disabled}
        value={current === undefined ? "" : String(pending ?? current)}
        onChange={event => {
          // Applied as soon as it is chosen; `pending` shows it until readback.
          const enabled = event.target.value === "true";
          setPending(enabled);
          if (enabled !== current) void toggle(enabled);
        }}>
        {current === undefined && <option value="">{text.unknown}</option>}
        <option value="true">{text.on}</option><option value="false">{text.off}</option>
      </select>
    </div>
    {inactive && current !== undefined && <p className="hint">{text.stopped}</p>}
    {blocked && <p className="hint">{text.blocked}</p>}
    {heldElsewhere && <p className="hint" role="note">{text.held.replace("{name}", holder.name)}</p>}
    {error && <p className="alert" role="alert">{error}</p>}
  </section>;

  return <section className="panel tun-control" aria-label={text.title}>
    <div className="panel-title"><h2 className="setting-heading">{text.title}<HelpTip label={text.title}>{text.help}</HelpTip></h2>{extra}</div>
    <div className="tun-controls">
      <div className="tun-status"><span className={`tun-indicator${current === true && running ? " enabled" : ""}`} aria-hidden="true">↔</span>
        <div className="tun-status-text">
          <strong role="status">{working ? text.working : !running ? phaseLabel(language, status.phase) : current === undefined ? text.unknown : current ? text.on : text.off}</strong>
          {/* Inside the fixed-height status block, so a takeover never moves the card's content. */}
          {heldElsewhere && <small role="note" title={text.held.replace("{name}", holder.name)}>{text.heldShort.replace("{name}", holder.name)}</small>}
        </div>
      </div>
      <button type="button" role="switch" aria-label={text.title} aria-checked={current ?? false} aria-busy={working}
        className="tun-switch" disabled={disabled} onClick={() => void toggle()}><span /></button>
    </div>
    {inactive && current !== undefined && <p className="hint">{current ? text.on : text.off} · {text.stopped}</p>}
    {blocked && <p className="hint">{text.blocked}</p>}
    {error && <p className="alert" role="alert">{error}</p>}
  </section>;
}
