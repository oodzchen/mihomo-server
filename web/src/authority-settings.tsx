import type { Draft, Runtime } from "./network-settings";

export const AUTHORITY_KEYS = new Set([
  "bind-address",
  "authentication",
  "skip-auth-prefixes",
  "lan-allowed-ips",
  "lan-disallowed-ips",
  "inbound-tfo",
  "inbound-mptcp",
  "sniffing",
]);

const ownedBind = "bind-address:owned";
const ownedAuth = "authentication:owned";
const ownedSkip = "skip-auth-prefixes:owned";
const ownedLanAllowed = "lan-allowed-ips:owned";
const ownedLanDisallowed = "lan-disallowed-ips:owned";

function isValidIpOrCidr(entry: string): boolean {
  const trimmed = entry.trim();
  if (!trimmed) return false;
  const parts = trimmed.split("/");
  if (parts.length === 1) {
    return isValidIp(parts[0]);
  }
  if (parts.length === 2) {
    const prefix = Number(parts[1]);
    if (!Number.isInteger(prefix) || prefix < 0) return false;
    if (parts[0].includes(":")) {
      return isValidIp(parts[0]) && prefix <= 128;
    }
    return isValidIp(parts[0]) && prefix <= 32;
  }
  return false;
}

function isValidIp(ip: string): boolean {
  if (ip.includes(":")) {
    // IPv6 basic structure validation
    return /^[0-9a-fA-F:]+$/.test(ip) && !ip.includes(":::") && (ip.match(/::/g) || []).length <= 1;
  }
  // IPv4 basic structure validation
  const octets = ip.split(".");
  if (octets.length !== 4) return false;
  return octets.every((octet) => {
    if (!/^\d+$/.test(octet)) return false;
    const num = Number(octet);
    return num >= 0 && num <= 255 && (octet === "0" || !octet.startsWith("0"));
  });
}

export function validateAuthority(key: string, value: unknown) {
  if (value == null) return;
  if (key === "bind-address") {
    if (typeof value !== "string" || value.length > 255 || /\s/.test(value)) {
      throw new Error("绑定地址不能包含空白字符且最多 255 字节。");
    }
    if (value !== "*" && value !== "" && value.toLowerCase() !== "localhost" && !isValidIp(value)) {
      throw new Error("绑定地址必须是 '*'、'localhost'、有效 IP 地址或留空。");
    }
  } else if (key === "authentication") {
    if (!Array.isArray(value)) throw new Error("认证设置必须是列表。");
    for (const item of value) {
      if (typeof item !== "string" || !item.includes(":") || !item.split(":")[0].trim()) {
        throw new Error("认证列表各项格式必须为 'username:password'。");
      }
    }
  } else if (key === "skip-auth-prefixes" || key === "lan-allowed-ips" || key === "lan-disallowed-ips") {
    if (!Array.isArray(value)) throw new Error(`${key} 必须是 IP/CIDR 列表。`);
    for (const item of value) {
      if (typeof item !== "string" || !isValidIpOrCidr(item)) {
        throw new Error(`${key} 中的 '${item}' 不是有效的 IP 或 CIDR 前缀。`);
      }
    }
  } else if (key === "inbound-tfo" || key === "inbound-mptcp" || key === "sniffing") {
    if (typeof value !== "boolean") throw new Error(`${key} 必须是布尔值。`);
  }
}

export function authorityDraft(runtime: Runtime): Draft {
  const listToDraft = (key: string, ownedKey: string) => {
    const val = runtime[key];
    if (val == null) return { [ownedKey]: "", [key]: "" };
    if (Array.isArray(val)) {
      return {
        [ownedKey]: "true",
        [key]: val.length === 0 ? "[]" : val.join("\n"),
      };
    }
    return { [ownedKey]: "true", [key]: String(val) };
  };

  return {
    [ownedBind]: runtime["bind-address"] == null ? "" : "true",
    "bind-address": runtime["bind-address"] == null ? "" : String(runtime["bind-address"]),
    ...listToDraft("authentication", ownedAuth),
    ...listToDraft("skip-auth-prefixes", ownedSkip),
    ...listToDraft("lan-allowed-ips", ownedLanAllowed),
    ...listToDraft("lan-disallowed-ips", ownedLanDisallowed),
    "inbound-tfo": runtime["inbound-tfo"] == null ? "" : String(runtime["inbound-tfo"]),
    "inbound-mptcp": runtime["inbound-mptcp"] == null ? "" : String(runtime["inbound-mptcp"]),
    sniffing: runtime["sniffing"] == null ? "" : String(runtime["sniffing"]),
  };
}

function parseListDraft(text: string): string[] {
  const trimmed = text.trim();
  if (trimmed === "[]" || trimmed === "") return [];
  return trimmed
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

export function authorityRuntime(draft: Draft): Runtime {
  const runtime: Runtime = {};

  if (draft[ownedBind] === "true") {
    const val = (draft["bind-address"] ?? "").trim();
    validateAuthority("bind-address", val);
    runtime["bind-address"] = val;
  }

  const parseListField = (key: string, ownedKey: string) => {
    if (draft[ownedKey] === "true") {
      const raw = draft[key] ?? "";
      const list = parseListDraft(raw);
      validateAuthority(key, list);
      runtime[key] = list;
    }
  };

  parseListField("authentication", ownedAuth);
  parseListField("skip-auth-prefixes", ownedSkip);
  parseListField("lan-allowed-ips", ownedLanAllowed);
  parseListField("lan-disallowed-ips", ownedLanDisallowed);

  for (const boolKey of ["inbound-tfo", "inbound-mptcp", "sniffing"] as const) {
    const val = draft[boolKey] ?? "";
    if (val !== "") {
      if (val !== "true" && val !== "false") throw new Error(`${boolKey} 值无效。`);
      runtime[boolKey] = val === "true";
    }
  }

  return runtime;
}

export function AuthorityFields({
  draft,
  disabled,
  onChange,
}: {
  draft: Draft;
  disabled: boolean;
  onChange: (key: string, value: string) => void;
}) {
  return (
    <fieldset className="network-fields" disabled={disabled}>
      <legend>监听与访问控制 (Listener & Access Control)</legend>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedBind] === "true"}
          onChange={(e) => onChange(ownedBind, e.target.checked ? "true" : "")}
        />
        管理绑定监听地址 (bind-address)
      </label>
      <label>
        绑定地址
        <input
          aria-label="绑定地址"
          value={draft["bind-address"] ?? ""}
          disabled={draft[ownedBind] !== "true"}
          placeholder="*、localhost、127.0.0.1、::1 或留空"
          onChange={(e) => onChange("bind-address", e.target.value)}
        />
      </label>
      <p className="muted">勾选后自定义绑定地址或设为 '*'。未勾选时继承订阅或核心默认值。</p>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedAuth] === "true"}
          onChange={(e) => onChange(ownedAuth, e.target.checked ? "true" : "")}
        />
        管理 HTTP/SOCKS 认证（勾选且留空表示显式清空订阅认证）
      </label>
      <label>
        认证列表 (user:password，每行一条或逗号分隔)
        <textarea
          aria-label="入站认证列表"
          value={draft["authentication"] ?? ""}
          disabled={draft[ownedAuth] !== "true"}
          placeholder="user:password"
          rows={3}
          onChange={(e) => onChange("authentication", e.target.value)}
        />
      </label>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedSkip] === "true"}
          onChange={(e) => onChange(ownedSkip, e.target.checked ? "true" : "")}
        />
        管理跳过认证 IP 前缀（勾选且留空表示显式清空）
      </label>
      <label>
        跳过认证 IP/CIDR (每行一条)
        <textarea
          aria-label="跳过认证 IP 前缀"
          value={draft["skip-auth-prefixes"] ?? ""}
          disabled={draft[ownedSkip] !== "true"}
          placeholder="127.0.0.1/32"
          rows={2}
          onChange={(e) => onChange("skip-auth-prefixes", e.target.value)}
        />
      </label>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedLanAllowed] === "true"}
          onChange={(e) => onChange(ownedLanAllowed, e.target.checked ? "true" : "")}
        />
        管理局域网允许访问 IP (lan-allowed-ips)
      </label>
      <label>
        局域网允许访问 IP/CIDR (每行一条)
        <textarea
          aria-label="局域网允许访问 IP"
          value={draft["lan-allowed-ips"] ?? ""}
          disabled={draft[ownedLanAllowed] !== "true"}
          placeholder="192.168.1.0/24"
          rows={2}
          onChange={(e) => onChange("lan-allowed-ips", e.target.value)}
        />
      </label>

      <label>
        <input
          type="checkbox"
          checked={draft[ownedLanDisallowed] === "true"}
          onChange={(e) => onChange(ownedLanDisallowed, e.target.checked ? "true" : "")}
        />
        管理局域网拒绝访问 IP (lan-disallowed-ips)
      </label>
      <label>
        局域网拒绝访问 IP/CIDR (每行一条)
        <textarea
          aria-label="局域网拒绝访问 IP"
          value={draft["lan-disallowed-ips"] ?? ""}
          disabled={draft[ownedLanDisallowed] !== "true"}
          placeholder="192.168.1.100/32"
          rows={2}
          onChange={(e) => onChange("lan-disallowed-ips", e.target.value)}
        />
      </label>

      <label>
        入站 TCP Fast Open (inbound-tfo)
        <select
          aria-label="入站 TCP Fast Open"
          value={draft["inbound-tfo"] ?? ""}
          onChange={(e) => onChange("inbound-tfo", e.target.value)}
        >
          <option value="">继承</option>
          <option value="true">启用</option>
          <option value="false">禁用</option>
        </select>
      </label>

      <label>
        入站 Multipath TCP (inbound-mptcp)
        <select
          aria-label="入站 Multipath TCP"
          value={draft["inbound-mptcp"] ?? ""}
          onChange={(e) => onChange("inbound-mptcp", e.target.value)}
        >
          <option value="">继承</option>
          <option value="true">启用</option>
          <option value="false">禁用</option>
        </select>
      </label>

      <label>
        域名嗅探 (sniffing)
        <select
          aria-label="域名嗅探"
          value={draft["sniffing"] ?? ""}
          onChange={(e) => onChange("sniffing", e.target.value)}
        >
          <option value="">继承</option>
          <option value="true">启用</option>
          <option value="false">禁用</option>
        </select>
      </label>
    </fieldset>
  );
}
