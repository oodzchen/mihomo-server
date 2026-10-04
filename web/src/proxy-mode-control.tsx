import { useEffect, useRef, useState, type ReactNode } from "react";
import { ApiError, command, type Connection, type Perform } from "./api";
import { HelpTip } from "./help-tip";
import { phaseLabel, type Language } from "./i18n";
import type { Access, ProxyAccessState } from "./proxy-access";
import { useToast } from "./toast";
import type { CoreStatus } from "./types";

const modes = ["direct", "rule", "global"] as const;
type Mode = typeof modes[number];
function validMode(value: string | undefined): Mode | undefined {
  const mode = value?.toLowerCase();
  return modes.includes(mode as Mode) ? mode as Mode : undefined;
}

export function ProxyModeControl({ token, language, status, connection, access, busy, perform, logout, extra }: {
  token: string; language: Language; status: CoreStatus; connection: Connection;
  access: ProxyAccessState; busy: boolean; perform: Perform; logout: (reason?: string) => void; extra?: ReactNode;
}) {
  const text = language === "en" ? {
    title: "Proxy mode", direct: "Direct", rule: "Rule", global: "Global", unknown: "Unconfirmed", working: "Applying…",
    help: "Direct bypasses proxies; Rule follows routing rules; Global uses the GLOBAL group. Click to save and apply immediately, preserving other settings.",
    stopped: "Saved mode; takes effect when the core starts.", verified: "Proxy mode verified", failed: "Proxy mode could not be confirmed. Check the error and retry.", expired: "Authentication expired. Please sign in again.",
  } : language === "zhtw" ? {
    title: "代理模式", direct: "直連", rule: "規則", global: "全域", unknown: "未確認", working: "正在套用…",
    help: "直連不經代理；規則按設定分流；全域使用 GLOBAL 群組。點擊立即儲存並套用，保留其他設定。",
    stopped: "目前為已儲存模式，內核啟動後生效。", verified: "代理模式已核對", failed: "代理模式尚未確認，請查看錯誤後重試。", expired: "認證失效，請重新登入。",
  } : {
    title: "代理模式", direct: "直连", rule: "规则", global: "全局", unknown: "未确认", working: "正在应用…",
    help: "直连不经代理；规则按配置分流；全局使用 GLOBAL 分组。点击立即保存并应用，保留其他设置。",
    stopped: "当前为已保存模式，内核启动后生效。", verified: "代理模式已核对", failed: "代理模式尚未确认，请查看错误后重试。", expired: "认证失效，请重新登录。",
  };
  const notify = useToast();
  const [working, setWorking] = useState(false), [error, setError] = useState("");
  const alive = useRef(true), locked = useRef(false), request = useRef<AbortController | undefined>(undefined);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; request.current?.abort(); };
  }, [token]);
  const running = status.phase === "running", inactive = status.phase === "stopped" || status.phase === "failed";
  const current = connection !== "connected" || !access.value?.has_config ? undefined
    : running && access.value.running ? validMode(access.value.reported?.mode)
    : inactive && !access.value.running ? validMode(access.value.configured.mode) : undefined;
  const disabled = busy || working || current === undefined;

  async function choose(mode: Mode) {
    if (disabled || locked.current || mode === current) return;
    locked.current = true; setWorking(true); setError("");
    try {
      const result = await perform("set_proxy_mode", { mode }, { notify: false });
      if (!alive.current) return;
      const controller = new AbortController(); request.current = controller;
      const [saved, observed] = await Promise.all([
        command<{ runtime: { mode?: Mode } }>(token, "settings", {}, controller.signal),
        command<Access>(token, "proxy_access", {}, controller.signal),
      ]);
      if (!alive.current) return;
      if (saved.runtime.mode !== mode || validMode(observed.configured.mode) !== mode ||
        observed.running && validMode(observed.reported?.mode) !== mode) throw new Error(text.failed);
      notify(`${text.verified}：${text[mode]}`, result === undefined ? "info" : "success");
    } catch (cause) {
      if (!alive.current) return;
      if (cause instanceof ApiError && cause.status === 401) logout(text.expired);
      else setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      locked.current = false;
      if (alive.current) { setWorking(false); access.refresh(); }
    }
  }

  return <section className="panel proxy-mode-control" aria-label={text.title}>
    <div className="panel-title"><h2 className="setting-heading">{text.title}<HelpTip label={text.title}>{text.help}</HelpTip></h2>{extra}</div>
    <div className="mode-segments" role="group" aria-label={text.title} aria-busy={working}>
      {modes.map(mode => <button key={mode} type="button" className={current === mode ? "active" : ""}
        aria-pressed={current === mode} disabled={disabled} onClick={() => void choose(mode)}>{text[mode]}</button>)}
    </div>
    <p className="runtime-state" role="status">{working ? text.working : !running ? phaseLabel(language, status.phase) : current ? text[current] : text.unknown}</p>
    {inactive && current && <p className="hint">{text[current]} · {text.stopped}</p>}
    {error && <p className="alert" role="alert">{error}</p>}
  </section>;
}
