import { useEffect, useRef, useState, type ReactNode } from "react";
import { ApiError, command, type Connection, type Perform } from "./api";
import { HelpTip } from "./help-tip";
import { phaseLabel, type Language } from "./i18n";
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
    failed: "TUN state could not be confirmed. Check the error and retry.", verified: "TUN state verified", expired: "Authentication expired. Please sign in again.",
    held: "{name} is using the system-wide TUN, so your traffic follows their rules. You can enable TUN after they turn it off.",
  } : language === "zhtw" ? {
    title: "TUN 模式", on: "已開啟", off: "已關閉", unknown: "未確認", working: "正在套用…",
    help: "透過虛擬網卡接管流量。點擊開關立即儲存並套用，保留其他 TUN 進階參數。",
    stopped: "目前為已儲存設定，內核啟動後生效。", blocked: "請先套用或修正尚未生效的設定，再變更 TUN。",
    failed: "TUN 狀態尚未確認，請查看錯誤後重試。", verified: "TUN 狀態已核對", expired: "認證失效，請重新登入。",
    held: "整機 TUN 正由 {name} 使用，你的流量目前按其規則處理；對方關閉後你才能開啟。",
  } : {
    title: "TUN 模式", on: "已开启", off: "已关闭", unknown: "未确认", working: "正在应用…",
    help: "通过虚拟网卡接管流量。点击开关立即保存并应用，保留其他 TUN 高级参数。",
    stopped: "当前为已保存设置，内核启动后生效。", blocked: "请先应用或修正尚未生效的设置，再更改 TUN。",
    failed: "TUN 状态尚未确认，请查看错误后重试。", verified: "TUN 状态已核对", expired: "认证失效，请重新登录。",
    held: "整机 TUN 正由 {name} 使用，你的流量目前按其规则处理；对方关闭后你才能开启。",
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
  // Another account holds the host's one system-wide TUN; only it (or root) can end it.
  const holder = access.value?.tun_holder;
  const heldElsewhere = !!holder && !holder.self && current !== true;
  const disabled = busy || working || blocked || heldElsewhere || current === undefined;

  async function toggle(enabled = !current) {
    if (disabled || locked.current || current === undefined) return;
    locked.current = true;
    setWorking(true); setError("");
    const toast = notify.loading(text.working);
    try {
      const result = await perform("set_tun_enabled", { enabled }, { notify: false, toast, reconcile: true });
      if (!alive.current) return;
      // Re-read the settings editor too, so a later full save cannot undo this toggle.
      await onChanged?.();
      if (!alive.current) return;
      const controller = new AbortController();
      request.current = controller;
      const [saved, observed] = await Promise.all([
        command<{ runtime: { tun?: { enable?: boolean } } }>(token, "settings", {}, controller.signal),
        command<Access>(token, "proxy_access", {}, controller.signal),
      ]);
      if (!alive.current) return;
      if (saved.runtime.tun?.enable !== enabled || observed.tun_enabled !== enabled ||
        observed.running && observed.reported?.tun_enabled !== enabled) throw new Error(text.failed);
      toast.finish(`${text.verified}：${enabled ? text.on : text.off}`, result === undefined ? "info" : "success");
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
        onChange={event => setPending(event.target.value === "true")}
        onBlur={() => { if (pending !== undefined && pending !== current) void toggle(pending); }}
        onKeyDown={event => {
          if (event.key === "Enter" && !event.nativeEvent.isComposing) {
            event.preventDefault();
            if (pending !== undefined && pending !== current) void toggle(pending);
          }
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
        <strong role="status">{working ? text.working : !running ? phaseLabel(language, status.phase) : current === undefined ? text.unknown : current ? text.on : text.off}</strong>
      </div>
      <button type="button" role="switch" aria-label={text.title} aria-checked={current ?? false} aria-busy={working}
        className="tun-switch" disabled={disabled} onClick={() => void toggle()}><span /></button>
    </div>
    {inactive && current !== undefined && <p className="hint">{current ? text.on : text.off} · {text.stopped}</p>}
    {blocked && <p className="hint">{text.blocked}</p>}
    {heldElsewhere && <p className="hint" role="note">{text.held.replace("{name}", holder.name)}</p>}
    {error && <p className="alert" role="alert">{error}</p>}
  </section>;
}
