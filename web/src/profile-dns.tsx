import { useEffect, useRef, useState } from "react";
import { ApiError, command, type Perform, type Connection } from "./api";
import type { CoreStatus } from "./types";

type State = {
  uid: string;
  source: string | null;
  requested: boolean;
  enabled: boolean;
};
const digest = (value: unknown): value is string =>
  typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
function decode(value: unknown, uid: string): State {
  const state = value as State;
  if (
    !state ||
    state.uid !== uid ||
    !(state.source === null || digest(state.source)) ||
    typeof state.requested !== "boolean" ||
    typeof state.enabled !== "boolean"
  )
    throw new Error("服务返回的订阅 DNS 状态无效。");
  return state;
}
const explain = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

export function ProfileDnsPanel({
  token,
  status,
  connection,
  hasDns,
  blocked,
  perform,
  logout,
}: {
  token: string;
  status: CoreStatus;
  connection: Connection;
  hasDns: boolean;
  blocked: boolean;
  perform: Perform;
  logout: (reason?: string) => void;
}) {
  const uid = status.active_profile;
  const [state, setState] = useState<State>();
  const [challenge, setChallenge] = useState<string>();
  const [working, setWorking] = useState(false),
    [uncertain, setUncertain] = useState(true);
  const [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const alive = useRef(true),
    serial = useRef(0),
    requests = useRef(new Set<AbortController>());
  async function read() {
    const controller = new AbortController();
    requests.current.add(controller);
    try {
      return decode(
        await command(token, "profile_dns", { uid }, controller.signal),
        uid!,
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
    const id = ++serial.current;
    setChallenge(undefined);
    setUncertain(true);
    setError("");
    try {
      const next = await read();
      if (alive.current && id === serial.current) {
        setState(next);
        setUncertain(false);
      }
    } catch (error) {
      if (alive.current && id === serial.current)
        setError(`读取订阅 DNS 状态失败：${explain(error)}。请核对后再操作。`);
    }
  }
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      ++serial.current;
      requests.current.forEach((c) => c.abort());
    };
  }, []);
  useEffect(() => {
    if (connection !== "connected") {
      ++serial.current;
      setChallenge(undefined);
      setUncertain(true);
      requests.current.forEach((controller) => controller.abort());
    } else if (uid) void reload();
  }, [token, uid, connection, status.config_revision, status.generation]);
  useEffect(() => {
    if (blocked) setChallenge(undefined);
  }, [blocked]);

  async function setEnabled(enabled: boolean, confirmation?: string) {
    ++serial.current;
    setWorking(true);
    setUncertain(true);
    setChallenge(undefined);
    setError("");
    setNotice("");
    const result = await perform<unknown>("set_profile_dns", {
      uid,
      enabled,
      ...(confirmation ? { confirmation } : {}),
    });
    if (!alive.current) return;
    // Independent readback also reconciles errors after a logical commit.
    const id = ++serial.current;
    try {
      const next = await read();
      if (!alive.current || id !== serial.current) return;
      setState(next);
      setUncertain(false);
      if (result === undefined)
        setNotice(
          "请求报告错误，已核对服务状态。再次启用需要重新确认。实际配置请在配置页核对。",
        );
      else {
        const outcome = result as {
          status: string;
          source?: unknown;
          state?: unknown;
        };
        if (
          outcome.status === "confirmation_required" &&
          digest(outcome.source)
        ) {
          if (next.source !== outcome.source)
            setNotice("订阅 DNS 来源已变化，请重新请求启用并确认新的来源。");
          else {
            setChallenge(outcome.source);
            setNotice(
              confirmation
                ? "原确认已失效，请重新审阅并确认当前来源。"
                : "订阅含专用解析策略，需要确认覆盖。尚未修改设置。",
            );
          }
        } else if (outcome.status === "applied") {
          decode(outcome.state, uid!);
          setNotice(
            next.enabled === enabled
              ? "订阅 DNS 覆盖设置已核对。"
              : "服务状态已变化，请核对当前订阅和实际配置。",
          );
        } else throw new Error("服务返回的 DNS 保存结果无效。");
      }
    } catch (error) {
      if (alive.current && id === serial.current) {
        setUncertain(true);
        setChallenge(undefined);
        setError(`保存结果尚未核对：${explain(error)}。请先核对 DNS 状态。`);
      }
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  const disabled = blocked || working || uncertain || connection !== "connected";
  return (
    <section className="panel profile-dns" aria-label="订阅 DNS 覆盖">
      <h2>当前订阅 DNS 覆盖</h2>
      <p className="muted">
        此开关决定当前订阅能否使用已保存的 DNS 页面设置和 hosts 映射。请先保存草稿；它与 DNS
        启用字段分别控制。
      </p>
      <p className="hint">
        确认仅在本次服务会话内有效。重启或专用解析来源变化后需重新确认；已提交的运行配置可能仍保留原值，下一次生成才应用保护。实际值请在配置页查看。
      </p>
      {!hasDns && <p className="hint">启用覆盖前，请先保存 DNS 配置段或 hosts 映射。</p>}
      {!uid ? (
        <p className="muted">尚未选择订阅。</p>
      ) : (
        <>
          <p className="hint">
            当前订阅 UID：<span className="dns-digest">{uid}</span>
          </p>
          {state && (
            <dl className="settings-summary">
              <div>
                <dt>保存的覆盖偏好</dt>
                <dd>{state.requested ? "启用" : "禁用"}</dd>
              </div>
              <div>
                <dt>当前生成许可</dt>
                <dd>{state.enabled ? "允许" : "未允许"}</dd>
              </div>
              <div>
                <dt>订阅专用 DNS</dt>
                <dd>{state.source ? "存在，需要会话确认" : "未检测到"}</dd>
              </div>
            </dl>
          )}
          {error && (
            <p className="alert" role="alert">
              {error}
            </p>
          )}
          {notice && (
            <p className="info" role="status">
              {notice}
            </p>
          )}
          {uncertain && <p className="hint">DNS 状态待核对。</p>}
          <div className="actions">
            <button
              disabled={disabled || !hasDns}
              onClick={() => void setEnabled(true)}
            >
              启用订阅 DNS 覆盖
            </button>
            <button disabled={disabled} onClick={() => void setEnabled(false)}>
              禁用订阅 DNS 覆盖
            </button>
            <button disabled={working} onClick={() => void reload()}>
              核对 DNS 状态
            </button>
          </div>
          {challenge && (
            <div
              className="reset-confirmation"
              role="group"
              aria-label="订阅 DNS 冲突确认"
            >
              <p>
                此订阅含专用解析服务器或域名解析策略。确认后，本页非空 DNS
                设置将参与覆盖，可能改变订阅解析行为。只有当前 UID
                与此来源一致时确认才有效。
              </p>
              <p className="dns-digest">来源标识：{challenge}</p>
              <div className="actions">
                <button
                  disabled={disabled || !hasDns}
                  onClick={() => void setEnabled(true, challenge)}
                >
                  确认覆盖当前订阅 DNS
                </button>
                <button
                  disabled={working}
                  onClick={() => {
                    setChallenge(undefined);
                    setNotice("已取消确认，未修改设置。");
                  }}
                >
                  取消 DNS 确认
                </button>
              </div>
            </div>
          )}
        </>
      )}
    </section>
  );
}
