import { useEffect, useRef, useState } from "react";
import { ApiError, command } from "./api";
import { GeoSeedAction } from "./geo_seed";
import { GeoOnlineAction } from "./geo_online";
import type { CoreStatus } from "./types";

type FreshnessState = "fresh" | "stale" | "indeterminate";
type AutoUpdateState = "active" | "disabled" | "stopped" | "indeterminate";

type Resource = {
  section: string;
  name: string;
  provider_type: string | null;
  path: string | null;
  state: string;
  bytes: number | null;
  modified_unix_seconds?: number | null;
  age_seconds?: number | null;
  freshness?: FreshnessState | null;
  conflict: boolean;
};
type GeoUpdatePolicy = {
  core_running: boolean;
  readback_error: boolean;
  configured_enabled: boolean | null;
  configured_interval_hours: number | null;
  effective_enabled: boolean | null;
  effective_interval_hours: number | null;
  auto_update_state?: AutoUpdateState;
  mismatch: boolean;
};
type Inventory = {
  data_dir: string;
  bundle_dir: string | null;
  config_revision: string | null;
  geo_update?: GeoUpdatePolicy;
  geo: Resource[];
  providers: Resource[];
};
const labels: Record<string, string> = {
  available: "文件存在",
  missing: "文件缺失",
  empty: "空文件",
  unsafe_path: "路径不安全",
  not_file: "不是普通文件",
  unreadable: "无法读取元数据",
  inline: "内联资源",
  core_managed: "内核管理缓存路径",
  invalid_declaration: "声明无效",
};
const autoUpdateStateLabels: Record<AutoUpdateState, string> = {
  active: "活跃（自动更新中）",
  disabled: "已禁用",
  stopped: "核心未运行（已停止）",
  indeterminate: "状态未知（未确认）",
};
const freshnessLabels: Record<FreshnessState, { label: string; className?: string }> = {
  fresh: { label: "最新（更新周期内）", className: "info" },
  stale: { label: "已过期（需更新）", className: "alert" },
  indeterminate: { label: "未指定更新周期" },
};
const enabledLabel = (value: boolean | null) => value === null ? "未指定" : value ? "启用" : "禁用";
const intervalLabel = (value: number | null) => value === null ? "未指定" : `${value} 小时`;
function modifiedLabel(seconds?: number | null) {
  if (seconds == null || !Number.isSafeInteger(seconds) || seconds < 0) return null;
  const milliseconds = seconds * 1000;
  const date = new Date(milliseconds);
  if (Number.isNaN(date.getTime())) return null;
  const elapsedMinutes = Math.floor((Date.now() - milliseconds) / 60000);
  const age = elapsedMinutes < 0 ? "晚于本机时钟" : elapsedMinutes >= 1440 ? `${Math.floor(elapsedMinutes / 1440)} 天前` : elapsedMinutes >= 60 ? `${Math.floor(elapsedMinutes / 60)} 小时前` : `${elapsedMinutes} 分钟前`;
  return `文件修改时间：${date.toLocaleString("zh-CN", { hour12: false })}（${age}）`;
}

export function ResourcesPanel({ token, status, connection, logout }: {
  token: string;
  status: CoreStatus;
  connection: string;
  logout: (reason?: string) => void;
}) {
  const validationController = useRef<AbortController | null>(null);
  const epoch = useRef(0);
  const [checks, setChecks] = useState<Record<string, { message: string; error?: boolean }>>({});
  const [checking, setChecking] = useState<string>();
  const [notice, setNotice] = useState("");
  useEffect(() => { setNotice(""); }, [token, status.config_revision, connection]);
  const [value, setValue] = useState<Inventory>();
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    epoch.current++;
    validationController.current?.abort();
    setChecks({});
    setChecking(undefined);
    setError("");
    if (connection !== "已连接") { setValue(undefined); return; }
    let active = true;
    const controller = new AbortController();
    void command<Inventory>(token, "resources", {}, controller.signal).then(next => {
      if (active) setValue(next);
    }).catch(error => {
      if (!active) return;
      if (error instanceof ApiError && error.status === 401) logout("认证失效，请重新输入令牌。");
      else setError(error instanceof Error ? error.message : String(error));
    });
    return () => { active = false; controller.abort(); epoch.current++; validationController.current?.abort(); };
  }, [token, status.phase, status.generation, status.config_revision, connection, refresh, logout]);

  async function validate(name: string) {
    const currentEpoch = epoch.current;
    const controller = new AbortController();
    validationController.current = controller;
    setChecking(name);
    setChecks(previous => ({ ...previous, [name]: { message: `正在校验 ${name.endsWith(".dat") ? "DAT" : "MMDB"}…` } }));
    try {
      const report = await command<{ verified: boolean; warning: string | null; sha256: string; bytes: number; ip_version: number; node_count: number; dat?: { group_count: number; record_count: number; ipv4_count: number; ipv6_count: number; regex_count: number; attribute_count: number; empty_group_count: number; unknown_field_count: number; has_cn_group: boolean; core_matching_verified: boolean } }>(token, "validate_geo", { name }, controller.signal);
      if (currentEpoch !== epoch.current) return;
      let message: string;
      if (name.endsWith(".dat")) {
        const dat = report.dat;
        if (!dat) throw new Error("服务返回的 DAT 诊断无效。");
        message = `${report.verified ? "DAT 已知结构校验通过" : "DAT 包含未知字段，完整结构未验证"} · ${dat.group_count} 分组 · ${dat.record_count} 记录 · IPv4 ${dat.ipv4_count} / IPv6 ${dat.ipv6_count} · 正则 ${dat.regex_count} · 属性 ${dat.attribute_count} · 空分组 ${dat.empty_group_count} · 未知字段 ${dat.unknown_field_count} · ${dat.has_cn_group ? "含 CN 分组" : "缺少 CN 分组，核心初始化可能删除并重新下载"} · 核心规则匹配与正则语法兼容性未验证`;
      } else message = `${report.verified ? "MMDB 结构校验通过" : "MMDB 元数据可读，完整结构未验证（缺少数据库描述）"} · IPv${report.ip_version} · ${report.node_count} 节点`;
      setChecks(previous => ({ ...previous, [name]: { message: `${message} · ${report.bytes} 字节 · SHA-256 ${report.sha256}` } }));
    } catch (error) {
      if (currentEpoch !== epoch.current) return;
      if (error instanceof ApiError && error.status === 401) logout("认证失效，请重新输入令牌。");
      else setChecks(previous => ({ ...previous, [name]: { message: `校验失败：${error instanceof Error ? error.message : String(error)}`, error: true } }));
    } finally {
      if (currentEpoch === epoch.current) setChecking(undefined);
    }
  }

  function rows(items: Resource[]) {
    return <ul className="resource-list">{items.map(item => <li key={`${item.section}:${item.name}`}>
      <strong>{item.name}</strong> · {item.section === "proxy-providers" ? "代理 Provider" : item.section === "rule-providers" ? "规则 Provider" : "Geo"}
      <p>{labels[item.state] || "未知状态"}{item.bytes !== null ? ` · ${item.bytes} 字节` : ""}{item.provider_type ? ` · ${item.provider_type}` : ""}</p>
      {modifiedLabel(item.modified_unix_seconds) && <p>{modifiedLabel(item.modified_unix_seconds)}</p>}
      {item.freshness && item.state === "available" && item.freshness !== "indeterminate" && (
        <p className={freshnessLabels[item.freshness]?.className || "hint"}>
          资源新鲜度：{freshnessLabels[item.freshness]?.label || item.freshness}
        </p>
      )}
      {item.path && <code>{item.path}</code>}
      {item.section === "geo" && ["Country.mmdb", "ASN.mmdb", "geoip.metadb", "geoip.dat", "geosite.dat"].includes(item.name) && item.state === "available" && <button type="button" disabled={!!checking} onClick={() => void validate(item.name)}>校验 {item.name}</button>}
      {item.section === "geo" && checks[item.name] && <p role={checks[item.name].error ? "alert" : "status"} className={checks[item.name].error ? "alert" : "info"}>{checks[item.name].message}</p>}
      {item.section === "geo" && value?.bundle_dir && ["Country.mmdb", "ASN.mmdb", "geoip.metadb", "geoip.dat", "geosite.dat"].includes(item.name) && <GeoSeedAction name={item.name} token={token} status={status} connection={connection} logout={logout} installed={message => { setNotice(message); setRefresh(previous => previous + 1); }} />}
      {item.section === "geo" && ["Country.mmdb", "ASN.mmdb", "geoip.metadb", "geoip.dat", "geosite.dat"].includes(item.name) && <GeoOnlineAction name={item.name} token={token} status={status} connection={connection} logout={logout} installed={message => { setNotice(message); setRefresh(previous => previous + 1); }} />}
      {item.conflict && <p className="alert">多个资源声明共用此路径，请检查缓存是否冲突。</p>}
    </li>)}</ul>;
  }

  return <section className="panel" aria-label="运行资源清单">
    <div className="panel-title"><h2>Geo / Provider 资源</h2><button type="button" disabled={connection !== "已连接"} onClick={() => setRefresh(value => value + 1)}>刷新资源清单</button></div>
    {notice && <p role="status" className="info">{notice}</p>}
    {connection !== "已连接" ? <p className="info">服务连接中断，资源状态待重新核对。</p> : error ? <p className="alert" role="alert">读取资源失败：{error}</p> : !value ? <p className="muted">正在读取资源清单…</p> : <>
      <p>运行数据目录：<code>{value.data_dir}</code></p>
      {value.bundle_dir && <p>打包资源目录：<code>{value.bundle_dir}</code></p>}
      {!value.config_revision && <p className="info">尚无已提交配置，导入并使用订阅后显示 Provider 声明。</p>}
      <h3>Geo 自动更新策略</h3>
      {value.geo_update ? <>
        {value.geo_update.auto_update_state && <p>自动更新运行状态：<strong>{autoUpdateStateLabels[value.geo_update.auto_update_state] || value.geo_update.auto_update_state}</strong></p>}
        <p>已提交配置：{enabledLabel(value.geo_update.configured_enabled)} · 更新间隔：{intervalLabel(value.geo_update.configured_interval_hours)}</p>
        <p>内核实际状态：{!value.geo_update.core_running ? "内核未运行，未确认" : value.geo_update.readback_error ? "读取失败，请重试" : value.geo_update.effective_enabled === null ? "内核未报告" : enabledLabel(value.geo_update.effective_enabled)} · 更新间隔：{value.geo_update.core_running && !value.geo_update.readback_error ? intervalLabel(value.geo_update.effective_interval_hours) : "未确认"}</p>
        {value.geo_update.mismatch && <p className="alert">已提交策略与内核实际值不一致，请重新应用配置或检查内核。</p>}
      </> : <p className="muted">Geo 自动更新状态暂不可用。</p>}
      <h3>Geo 文件</h3>{rows(value.geo)}
      <h3>Provider 文件与缓存</h3>{value.providers.length ? rows(value.providers) : <p className="muted">当前已提交配置没有 Provider 声明。</p>}
      <p className="hint">路径相对于运行数据目录。文件存在仅表示元数据可读取，尚未验证内容格式；文件修改时间只反映文件系统元数据，不能证明最近一次自动更新成功、内容有效或已被规则加载。自动更新由 Mihomo 自行执行，不经过服务侧更新回滚。MMDB 和 DAT 可手动校验结构；打包资源可在停止内核后按摘要显式安装。已提交配置中的 Geo URL 可用于停止或运行中核心的显式在线更新：先读取来源及当前文件指纹，再下载、校验并安装。运行中更新会短暂停止并重启核心；失败时恢复旧资源。DAT 安装执行隔离内核规则加载检查，但不保证实际代理匹配效果。失败或请求中断后请重新读取状态。Geo 文件是否必需取决于配置规则。Provider 声明来自已提交配置，内核下载后可刷新清单核对文件状态。</p>
    </>}
  </section>;
}
