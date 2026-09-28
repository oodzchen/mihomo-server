type Value = number | boolean | string | string[] | Record<string, unknown>;
export type Runtime = Record<string, Value | Record<string, Value>>;
export type Draft = Record<string, string>;
type Field = {
  key: string;
  label: string;
  kind: "bool" | "string" | "list" | "mtu" | "select" | "policy" | "filter";
  options?: string[];
};
const schemas: Record<"dns" | "tun", Field[]> = {
  dns: [
    { key: "enable", label: "DNS 启用", kind: "bool" },
    { key: "ipv6", label: "DNS IPv6", kind: "bool" },
    { key: "use-hosts", label: "DNS 使用 hosts", kind: "bool" },
    { key: "use-system-hosts", label: "DNS 使用系统 hosts", kind: "bool" },
    { key: "listen", label: "DNS 监听地址", kind: "string" },
    {
      key: "enhanced-mode",
      label: "DNS 增强模式",
      kind: "select",
      options: ["fake-ip", "redir-host"],
    },
    {
      key: "fake-ip-filter-mode",
      label: "DNS Fake-IP 过滤模式",
      kind: "select",
      options: ["blacklist", "whitelist"],
    },
    { key: "prefer-h3", label: "DNS 优先 HTTP/3", kind: "bool" },
    { key: "respect-rules", label: "DNS 遵循代理规则", kind: "bool" },
    { key: "fake-ip-range", label: "DNS Fake-IP IPv4 范围", kind: "string" },
    { key: "fake-ip-range6", label: "DNS Fake-IP IPv6 范围", kind: "string" },
    { key: "default-nameserver", label: "DNS 默认解析服务器", kind: "list" },
    { key: "nameserver", label: "DNS 解析服务器", kind: "list" },
    { key: "fallback", label: "DNS 后备解析服务器", kind: "list" },
    { key: "proxy-server-nameserver", label: "DNS 代理节点解析服务器", kind: "list" },
    { key: "direct-nameserver", label: "DNS 直连解析服务器", kind: "list" },
    { key: "direct-nameserver-follow-policy", label: "DNS 直连遵循策略", kind: "bool" },
    { key: "nameserver-policy", label: "DNS 域名解析策略", kind: "policy" },
    { key: "proxy-server-nameserver-policy", label: "DNS 代理节点解析策略", kind: "policy" },
    { key: "fallback-filter", label: "DNS 后备过滤条件", kind: "filter" },
    { key: "fake-ip-filter", label: "DNS Fake-IP 过滤列表", kind: "list" },
  ],
  tun: [
    { key: "enable", label: "TUN 启用", kind: "bool" },
    {
      key: "stack",
      label: "TUN 协议栈",
      kind: "select",
      options: ["gvisor", "system", "mixed"],
    },
    { key: "device", label: "TUN 设备名称", kind: "string" },
    { key: "auto-route", label: "TUN 自动路由", kind: "bool" },
    { key: "auto-redirect", label: "TUN 自动重定向", kind: "bool" },
    { key: "auto-detect-interface", label: "TUN 自动检测接口", kind: "bool" },
    { key: "strict-route", label: "TUN 严格路由", kind: "bool" },
    { key: "mtu", label: "TUN MTU", kind: "mtu" },
    { key: "route-exclude-address", label: "TUN 排除路由地址", kind: "list" },
    { key: "dns-hijack", label: "TUN DNS 劫持列表", kind: "list" },
  ],
};

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function validPolicy(value: unknown): boolean {
  return object(value) && Object.entries(value).every(([key, server]) =>
    key.trim().length > 0 && (typeof server === "string" && server.trim().length > 0 ||
      Array.isArray(server) && server.length > 0 && server.every((item) => typeof item === "string" && item.trim().length > 0))
  );
}

function validFilter(value: unknown): boolean {
  return object(value) && Object.entries(value).every(([key, entry]) =>
    key === "geoip" ? typeof entry === "boolean" :
    key === "geoip-code" ? typeof entry === "string" :
    key === "ipcidr" || key === "domain" ? Array.isArray(entry) && entry.every((item) => typeof item === "string") : false
  );
}

export function validateNetwork(section: "dns" | "tun", value: unknown) {
  if (value == null) return;
  if (typeof value !== "object" || Array.isArray(value))
    throw new Error(`服务返回的 ${section} 设置无效。`);
  for (const [key, v] of Object.entries(value)) {
    const f = schemas[section].find((f) => f.key === key);
    if (!f)
      throw new Error(
        `服务包含暂不支持的设置 ${section}.${key}，请勿用此编辑器覆盖。`,
      );
    if (v == null) continue;
    const valid =
      f.kind === "bool"
        ? typeof v === "boolean"
        : f.kind === "policy"
          ? validPolicy(v)
          : f.kind === "filter"
            ? validFilter(v)
        : f.kind === "list"
          ? Array.isArray(v) && v.every((item) => typeof item === "string")
          : f.kind === "mtu"
            ? typeof v === "number" &&
              Number.isInteger(v) &&
              v >= 1 &&
              v <= 65535
            : typeof v === "string" &&
              (f.kind !== "select" || f.options!.includes(v)) &&
              (section !== "tun" || key !== "device" || v.length > 0);
    if (!valid) throw new Error(`服务返回的${f.label}值无效。`);
  }
}
export function networkDraft(runtime: Runtime): Draft {
  const draft: Draft = {};
  for (const section of ["dns", "tun"] as const) {
    const value = runtime[section] as Record<string, Value> | undefined;
    draft[section] = value == null ? "" : "true";
    for (const field of schemas[section]) {
      const key = `${section}.${field.key}`,
        v = value?.[field.key];
      draft[key] =
        v == null ? "" : ["list", "policy", "filter"].includes(field.kind) ? JSON.stringify(v) : String(v);
      draft[`${key}.present`] = v == null ? "" : "true";
    }
  }
  return draft;
}
export function networkRuntime(draft: Draft): Runtime {
  const result: Runtime = {};
  for (const section of ["dns", "tun"] as const) {
    if (draft[section] !== "true") continue;
    const value: Record<string, Value> = {};
    for (const f of schemas[section]) {
      const key = `${section}.${f.key}`,
        v = draft[key] ?? "";
      if (f.kind === "string") {
        if (draft[`${key}.present`] === "true") value[f.key] = v;
      } else if (v !== "") {
        if (f.kind === "list" || f.kind === "policy" || f.kind === "filter") {
          try {
            const parsed: unknown = JSON.parse(v);
            if (f.kind === "list") {
              if (!Array.isArray(parsed) || !parsed.every((item) => typeof item === "string")) throw new Error();
            } else if (f.kind === "policy" ? !validPolicy(parsed) : !validFilter(parsed)) throw new Error();
            value[f.key] = parsed as Value;
          } catch {
            throw new Error(
              f.kind === "list"
                ? `${f.label}必须是 JSON 字符串列表，例如 ["1.1.1.1"]；留空继承，[] 表示空列表。`
                : `${f.label}必须是有效的 JSON 对象；留空继承。`,
            );
          }
        } else if (f.kind === "mtu") {
          if (!/^\d+$/.test(v) || Number(v) < 1 || Number(v) > 65535)
            throw new Error("TUN MTU 必须是 1–65535 的整数，或留空继承。");
          value[f.key] = Number(v);
        } else {
          const allowed = f.kind === "bool" ? ["true", "false"] : f.options!;
          if (!allowed.includes(v)) throw new Error(`${f.label}值无效。`);
          value[f.key] = f.kind === "bool" ? v === "true" : v;
        }
      }
    }
    validateNetwork(section, value);
    result[section] = value;
  }
  return result;
}

export function NetworkFields({
  draft,
  disabled,
  change,
}: {
  draft: Draft;
  disabled: boolean;
  change: (key: string, value: string) => void;
}) {
  return (
    <>
      {(["dns", "tun"] as const).map((section) => (
        <fieldset className="network-fields" key={section} disabled={disabled}>
          <legend>{section.toUpperCase()} 设置</legend>
          <p className="hint">
            {section === "dns"
              ? "两个 hosts 开关的禁用会显式覆盖；其他 DNS 禁用项、空字符串和空列表仅保存并继承订阅值。DNS 和 hosts 设置需要当前订阅允许 DNS 覆盖。"
              : "TUN 的禁用和空列表会显式覆盖订阅值。自动重定向仅支持 Linux；实际启用需要服务主机权限和网络条件。"}
          </p>
          <label>
            {section.toUpperCase()} 设置来源
            <select
              aria-label={`${section.toUpperCase()} 设置来源`}
              value={draft[section] ?? ""}
              onChange={(e) => change(section, e.target.value)}
            >
              <option value="">继承整个配置段</option>
              <option value="true">使用本页设置</option>
            </select>
          </label>
          {draft[section] === "true" && (
            <div className="settings-fields">
              {schemas[section].map((f) => {
                const key = `${section}.${f.key}`;
                return (
                  <div key={key}>
                    {f.kind === "string" && (
                      <label>
                        {f.label}来源
                        <select
                          aria-label={`${f.label}来源`}
                          value={draft[`${key}.present`] ?? ""}
                          onChange={(e) =>
                            change(`${key}.present`, e.target.value)
                          }
                        >
                          <option value="">继承</option>
                          <option value="true">指定值</option>
                        </select>
                      </label>
                    )}
                    <label>
                      {f.label}
                      {f.kind === "list" || f.kind === "policy" || f.kind === "filter" ? (
                        <textarea
                          aria-label={f.label}
                          rows={3}
                          value={draft[key] ?? ""}
                          placeholder={f.kind === "list" ? "留空继承，或输入 JSON 列表 []" : "留空继承，或输入 JSON 对象 {}"}
                          onChange={(e) => change(key, e.target.value)}
                        />
                      ) : f.kind === "string" || f.kind === "mtu" ? (
                        <input
                          aria-label={f.label}
                          inputMode={f.kind === "mtu" ? "numeric" : undefined}
                          disabled={
                            disabled ||
                            (f.kind === "string" &&
                              draft[`${key}.present`] !== "true")
                          }
                          value={draft[key] ?? ""}
                          placeholder="留空继承"
                          onChange={(e) => change(key, e.target.value)}
                        />
                      ) : (
                        <select
                          aria-label={f.label}
                          value={draft[key] ?? ""}
                          onChange={(e) => change(key, e.target.value)}
                        >
                          <option value="">继承</option>
                          {(f.kind === "bool"
                            ? ["true", "false"]
                            : f.options!
                          ).map((v) => (
                            <option key={v} value={v}>
                              {v === "true"
                                ? "启用"
                                : v === "false"
                                  ? "禁用"
                                  : v}
                            </option>
                          ))}
                        </select>
                      )}
                    </label>
                  </div>
                );
              })}
            </div>
          )}
        </fieldset>
      ))}
    </>
  );
}
