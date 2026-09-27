import type { Draft, Runtime } from "./network-settings";

export const OUTBOUND_KEYS = new Set(["interface-name", "routing-mark"]);
const owned = "interface-name:owned";
const interfaceError = "出口网卡名称必须为空或最多 15 个 UTF-8 字节，不能包含空白、控制字符、斜杠或冒号，也不能是 . 或 ..。";
const markError = "Linux 路由标记必须是 0–4294967295 的整数，或留空继承。";

export function validateOutbound(key: string, value: unknown) {
  if (value == null) return;
  if (key === "interface-name") {
    if (typeof value !== "string" || new TextEncoder().encode(value).length > 15 || value === "." || value === ".." || /[/:]|\p{White_Space}|\p{Cc}/u.test(value))
      throw new Error(interfaceError);
  } else if (key === "routing-mark") {
    if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > 4294967295)
      throw new Error(markError);
  }
}

export function outboundDraft(runtime: Runtime): Draft {
  return {
    [owned]: runtime["interface-name"] == null ? "" : "true",
    "interface-name": runtime["interface-name"] == null ? "" : String(runtime["interface-name"]),
    "routing-mark": runtime["routing-mark"] == null ? "" : String(runtime["routing-mark"]),
  };
}

export function outboundRuntime(draft: Draft): Runtime {
  const runtime: Runtime = {};
  if (draft[owned] === "true") {
    const value = draft["interface-name"] ?? "";
    validateOutbound("interface-name", value);
    runtime["interface-name"] = value;
  }
  const mark = draft["routing-mark"] ?? "";
  if (mark !== "") {
    if (!/^\d+$/.test(mark)) throw new Error(markError);
    const value = Number(mark);
    validateOutbound("routing-mark", value);
    runtime["routing-mark"] = value;
  }
  return runtime;
}

export function OutboundFields({ draft, disabled, onChange }: {
  draft: Draft;
  disabled: boolean;
  onChange: (key: string, value: string) => void;
}) {
  const managed = draft[owned] === "true";
  return <fieldset className="network-fields" disabled={disabled}>
    <legend>出口设置</legend>
    <label><input type="checkbox" checked={managed} onChange={event => onChange(owned, event.target.checked ? "true" : "")} />管理出口网卡</label>
    <label>出口网卡名称<input aria-label="出口网卡名称" disabled={!managed} value={draft["interface-name"] ?? ""} placeholder="留空取消固定网卡" aria-describedby="outbound-interface-hint" onChange={event => onChange("interface-name", event.target.value)} /></label>
    <p id="outbound-interface-hint" className="muted">不勾选时继承配置；勾选后留空取消固定网卡，交由核心选择。指定名称应是此 Linux 主机的网卡。</p>
    <label>Linux 路由标记<input aria-label="Linux 路由标记" inputMode="numeric" value={draft["routing-mark"] ?? ""} placeholder="留空继承" aria-describedby="outbound-mark-hint" onChange={event => onChange("routing-mark", event.target.value)} /></label>
    <p id="outbound-mark-hint" className="muted">整数 0–4294967295；0 取消默认标记，留空继承。非零标记需要系统权限及匹配的路由策略。</p>
  </fieldset>;
}
