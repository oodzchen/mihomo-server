import { useEffect, useRef, useState } from "react";
import { ApiError, command, type Perform } from "./api";
import type { CoreStatus } from "./types";

type Release = { version: string; bytes: number; target: string };
type Installation = { version: string; stage_id: string };
type Report = { upgraded: boolean; from: string; to: string };

export function CoreUpgradePage({
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
  const [version, setVersion] = useState<string>();
  const [installation, setInstallation] = useState<Installation | null>();
  const [latest, setLatest] = useState<Release>();
  const [report, setReport] = useState<Report>();
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const alive = useRef(true);
  const locked = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  useEffect(() => {
    setVersion(undefined);
    setInstallation(undefined);
    setError("");
    if (connection !== "已连接") return;
    const controller = new AbortController();
    let active = true;
    void Promise.all([
      command<string>(token, "installed_core_version", {}, controller.signal),
      command<Installation | null>(
        token,
        "core_installation",
        {},
        controller.signal,
      ),
    ])
      .then(([version, receipt]) => {
        if (active) {
          setVersion(version);
          setInstallation(receipt);
        }
      })
      .catch((error: unknown) => {
        if (!active) return;
        if (error instanceof ApiError && error.status === 401)
          logout("认证失效，请重新输入令牌。");
        else {
          const message =
            error instanceof Error ? error.message : String(error);
          setError(
            message.includes("bundle-managed resources")
              ? "当前启动模式不支持在线升级，请使用带托管内核的 Linux bundle。"
              : `读取已安装内核失败：${message}`,
          );
        }
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, [token, connection, status.generation, refresh, logout]);

  async function run(force?: boolean) {
    if (locked.current || busy) return;
    if (
      force === true &&
      !window.confirm("重新安装最新稳定版内核？运行中的代理连接会短暂中断。")
    )
      return;
    locked.current = true;
    setWorking(true);
    setReport(undefined);
    try {
      if (force === undefined) {
        setLatest(undefined);
        const value = await perform<Release>("core_release");
        if (alive.current && value) setLatest(value);
      } else {
        const value = await perform<Report>("upgrade_clash_core", { force });
        if (alive.current && value) setReport(value);
        if (alive.current) setRefresh((value) => value + 1);
      }
    } finally {
      locked.current = false;
      if (alive.current) setWorking(false);
    }
  }
  const disabled = busy || working || connection !== "已连接" || !version;
  return (
    <section className="panel" aria-label="稳定版内核升级">
      <div className="panel-title">
        <h2>稳定版内核升级</h2>
        <button
          type="button"
          disabled={busy || working || connection !== "已连接"}
          onClick={() => setRefresh((value) => value + 1)}
        >
          刷新安装信息
        </button>
      </div>
      <p className="muted">
        检查并安装 Mihomo
        最新稳定版。升级时会短暂中断代理连接；失败时恢复上一份内核，停止的内核仍保持停止。
      </p>
      {connection !== "已连接" ? (
        <p className="info">服务连接中断，重新连接后核对安装信息。</p>
      ) : error ? (
        <p className="alert" role="alert">
          {error}
        </p>
      ) : !version ? (
        <p className="info">正在读取已安装内核…</p>
      ) : (
        <dl className="proxy-details">
          <div>
            <dt>已安装版本</dt>
            <dd>{version}</dd>
          </div>
          <div>
            <dt>最新稳定版</dt>
            <dd>{latest?.version || "尚未检查"}</dd>
          </div>
          <div>
            <dt>安装记录</dt>
            <dd>
              {installation
                ? `已验证安装 ${installation.version}`
                : "随 bundle 初始化"}
            </dd>
          </div>
        </dl>
      )}
      <div className="actions">
        <button type="button" disabled={disabled} onClick={() => void run()}>
          检查稳定版更新
        </button>
        <button
          type="button"
          className="primary"
          disabled={disabled}
          onClick={() => void run(false)}
        >
          升级至最新稳定版
        </button>
        <button
          type="button"
          disabled={disabled}
          onClick={() => void run(true)}
        >
          强制重新安装稳定版
        </button>
      </div>
      {working && (
        <p className="info" role="status">
          正在检查或升级内核，请稍候…
        </p>
      )}
      {report && (
        <p className="success" role="status">
          {report.upgraded
            ? `升级完成：${report.from} → ${report.to}`
            : `已是最新稳定版 ${report.to}，无需重新安装。`}
        </p>
      )}
      {report && version && version !== report.to && (
        <p className="alert" role="alert">
          当前安装版本与操作结果不同，请刷新安装信息并检查服务日志。
        </p>
      )}
      <p className="hint">
        默认跳过相同版本；强制重新安装会重新验证并替换内核。升级失败后可查看上方错误、刷新安装信息并重试。页面关闭后，已开始的内核切换由服务完成。
      </p>
    </section>
  );
}
