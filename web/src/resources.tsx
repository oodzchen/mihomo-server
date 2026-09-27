import { useEffect, useRef, useState } from "react";
import { ApiError, command } from "./api";
import type { CoreStatus } from "./types";

type Resource = {
  section: string;
  name: string;
  provider_type: string | null;
  path: string | null;
  state: string;
  bytes: number | null;
  conflict: boolean;
};
type Inventory = {
  data_dir: string;
  bundle_dir: string | null;
  config_revision: string | null;
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
  const [value, setValue] = useState<Inventory>();
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    epoch.current++;
    validationController.current?.abort();
    setChecks({});
    setChecking(undefined);
    setValue(undefined);
    setError("");
    if (connection !== "已连接") return;
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
    setChecks(previous => ({ ...previous, [name]: { message: "正在校验 MMDB…" } }));
    try {
      const report = await command<{ verified: boolean; warning: string | null; sha256: string; bytes: number; ip_version: number; node_count: number }>(token, "validate_geo", { name }, controller.signal);
      if (currentEpoch !== epoch.current) return;
      setChecks(previous => ({ ...previous, [name]: { message: `${report.verified ? "MMDB 结构校验通过" : "MMDB 元数据可读，完整结构未验证（缺少数据库描述）"} · IPv${report.ip_version} · ${report.node_count} 节点 · ${report.bytes} 字节 · SHA-256 ${report.sha256}` } }));
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
      {item.path && <code>{item.path}</code>}
      {item.section === "geo" && ["Country.mmdb", "ASN.mmdb", "geoip.metadb"].includes(item.name) && item.state === "available" && <button type="button" disabled={!!checking} onClick={() => void validate(item.name)}>校验 {item.name}</button>}
      {item.section === "geo" && checks[item.name] && <p role={checks[item.name].error ? "alert" : "status"} className={checks[item.name].error ? "alert" : "info"}>{checks[item.name].message}</p>}
      {item.conflict && <p className="alert">多个资源声明共用此路径，请检查缓存是否冲突。</p>}
    </li>)}</ul>;
  }

  return <section className="panel" aria-label="运行资源清单">
    <div className="panel-title"><h2>Geo / Provider 资源</h2><button type="button" disabled={connection !== "已连接"} onClick={() => setRefresh(value => value + 1)}>刷新资源清单</button></div>
    {connection !== "已连接" ? <p className="info">服务连接中断，资源状态待重新核对。</p> : error ? <p className="alert" role="alert">读取资源失败：{error}</p> : !value ? <p className="muted">正在读取资源清单…</p> : <>
      <p>运行数据目录：<code>{value.data_dir}</code></p>
      {value.bundle_dir && <p>打包资源目录：<code>{value.bundle_dir}</code></p>}
      {!value.config_revision && <p className="info">尚无已提交配置，导入并使用订阅后显示 Provider 声明。</p>}
      <h3>Geo 文件</h3>{rows(value.geo)}
      <h3>Provider 文件与缓存</h3>{value.providers.length ? rows(value.providers) : <p className="muted">当前已提交配置没有 Provider 声明。</p>}
      <p className="hint">路径相对于运行数据目录。文件存在仅表示元数据可读取，尚未验证内容格式。MMDB 可手动校验结构，结果仅针对当次读取的文件摘要，不证明规则覆盖或内核兼容性；DAT 格式校验待实现。Geo 文件是否必需取决于配置规则。Provider 声明来自已提交配置，内核下载后可刷新清单核对文件状态。</p>
    </>}
  </section>;
}
