import { useEffect, useRef, useState, type FormEvent } from "react";
import { ApiError, command, type Perform } from "./api";
import type { Profile } from "./types";

type Content = { uid: string; revision: string; yaml: string };
const explain = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
function decode(value: unknown, uid: string): Content {
  const content = value as Content;
  if (
    !content ||
    content.uid !== uid ||
    typeof content.revision !== "string" ||
    !content.revision ||
    content.revision.length > 256 ||
    typeof content.yaml !== "string" ||
    new TextEncoder().encode(content.yaml).length > 8 * 1024 * 1024
  )
    throw new Error("服务返回的原始订阅内容无效。");
  return content;
}

export function RawEditor({
  item,
  active,
  token,
  busy,
  perform,
  logout,
  onClose,
}: {
  item: Profile;
  active: boolean;
  token: string;
  busy: boolean;
  perform: Perform;
  logout: (reason?: string) => void;
  onClose: () => void;
}) {
  const [base, setBase] = useState<Content>(),
    [saved, setSaved] = useState<Content>();
  const [yaml, setYaml] = useState("");
  const [working, setWorking] = useState(false),
    [uncertain, setUncertain] = useState(true);
  const [error, setError] = useState(""),
    [notice, setNotice] = useState("");
  const [confirmation, setConfirmation] = useState<"reload" | "close">();
  const alive = useRef(true),
    requests = useRef(new Set<AbortController>());
  const disabled = busy || working;
  const dirty = !!base && yaml !== base.yaml;
  const conflict =
    !!base &&
    ((saved?.revision !== base.revision && saved?.yaml !== yaml) ||
      (!!item.file && item.file !== base.revision));
  async function read() {
    const controller = new AbortController();
    requests.current.add(controller);
    try {
      return decode(
        await command(
          token,
          "profile_raw",
          { uid: item.uid },
          controller.signal,
        ),
        item.uid,
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
    setWorking(true);
    setUncertain(true);
    setError("");
    setNotice("");
    setConfirmation(undefined);
    try {
      const next = await read();
      if (alive.current) {
        setBase(next);
        setSaved(next);
        setYaml(next.yaml);
        setUncertain(false);
      }
    } catch (error) {
      if (alive.current)
        setError(`读取原始订阅失败：${explain(error)}。草稿未被替换。`);
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  useEffect(() => {
    alive.current = true;
    void reload();
    return () => {
      alive.current = false;
      requests.current.forEach((controller) => controller.abort());
    };
  }, [token, item.uid]);
  async function verify() {
    setWorking(true);
    setUncertain(true);
    setError("");
    setNotice("");
    setConfirmation(undefined);
    try {
      const next = await read();
      if (alive.current) {
        setSaved(next);
        setUncertain(false);
        if (next.yaml === yaml) {
          setBase(next);
          setNotice("已核对：服务已保存当前原始订阅草稿。");
        } else
          setNotice(
            "已核对：服务内容与草稿不同，草稿已保留。版本变化时请重新读取后再编辑。",
          );
      }
    } catch (error) {
      if (alive.current)
        setError(`核对原始订阅失败：${explain(error)}。请核对后再提交。`);
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  async function save(event: FormEvent) {
    event.preventDefault();
    if (!base || disabled || uncertain || conflict) return;
    if (new TextEncoder().encode(yaml).length > 8 * 1024 * 1024) {
      setError("原始订阅不能超过 8 MiB。");
      return;
    }
    const requested = yaml;
    setWorking(true);
    setUncertain(true);
    setError("");
    setNotice("");
    setConfirmation(undefined);
    const result = await perform<unknown>("set_profile_raw", {
      uid: item.uid,
      revision: base.revision,
      yaml: requested,
    });
    if (!alive.current) return;
    try {
      const next = await read();
      if (!alive.current) return;
      setSaved(next);
      setUncertain(false);
      if (next.yaml === requested) {
        setBase(next);
        setNotice(
          result
            ? "原始订阅保存结果已核对。"
            : "请求报告错误，但服务已保存此草稿，已核对，无需重复提交。",
        );
      } else
        setNotice(
          "服务原始订阅与提交内容不同，草稿已保留。请检查错误；版本变化时需重新读取。",
        );
    } catch (error) {
      if (alive.current)
        setError(
          `保存结果尚未核对：${explain(error)}。草稿已保留，请先核对原始订阅。`,
        );
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  return (
    <section className="panel" aria-label="原始订阅编辑器">
      <h2>原始订阅 YAML</h2>
      <p className="muted">
        编辑 {item.name || item.uid} 的原始内容。
        {active
          ? "当前订阅保存前会校验原始 YAML，重新生成增强配置并应用；已停止的内核保持停止。"
          : "此订阅保存前会校验原始 YAML；下次使用时生成增强配置。"}
      </p>
      <p className="hint">
        保留名称、链接、用量、节点记录和增强关联。远程订阅下次刷新会替换手动内容。保存失败保留草稿；重新读取会替换草稿。离开订阅页会丢弃草稿。
      </p>
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
      {conflict && (
        <p className="alert" role="alert">
          原始订阅版本已变化。为避免覆盖其他修改，请重新读取后再编辑。
        </p>
      )}
      {uncertain && <p className="hint">原始订阅状态待核对，提交暂不可用。</p>}
      {base && (
        <form onSubmit={save} aria-label="原始订阅表单">
          <label>
            原始订阅 YAML
            <textarea
              aria-label="原始订阅 YAML"
              className="code"
              rows={18}
              disabled={disabled}
              value={yaml}
              spellCheck={false}
              onChange={(event) => {
                setYaml(event.target.value);
                setError("");
                setNotice("");
                setConfirmation(undefined);
              }}
            />
          </label>
          <div className="form-actions">
            <button
              className="primary"
              disabled={disabled || uncertain || conflict || !dirty}
            >
              保存原始订阅
            </button>
            <button
              type="button"
              disabled={disabled}
              onClick={() => void verify()}
            >
              核对原始订阅
            </button>
          </div>
        </form>
      )}
      <div className="actions settings-reload">
        <button
          disabled={disabled}
          onClick={() =>
            base && (dirty || uncertain)
              ? setConfirmation("reload")
              : void reload()
          }
        >
          {base ? "重新读取原始订阅" : "重试读取原始订阅"}
        </button>
        <button
          disabled={disabled}
          onClick={() => (dirty ? setConfirmation("close") : onClose())}
        >
          关闭原始编辑器
        </button>
      </div>
      {confirmation && (
        <div
          className="reset-confirmation"
          role="group"
          aria-label="原始订阅草稿替换确认"
        >
          <p>
            {confirmation === "reload"
              ? "重新读取会用服务当前原始内容替换草稿。"
              : "关闭会丢弃未保存的原始订阅草稿。"}
          </p>
          <div className="actions">
            <button
              disabled={disabled}
              onClick={() =>
                confirmation === "reload" ? void reload() : onClose()
              }
            >
              {confirmation === "reload"
                ? "确认重新读取原始订阅"
                : "确认丢弃原始草稿"}
            </button>
            <button
              disabled={disabled}
              onClick={() => setConfirmation(undefined)}
            >
              继续编辑原始订阅
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
