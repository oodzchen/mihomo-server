import { useEffect, useRef, useState, type FormEvent } from "react";
import { ApiError, command, type Perform } from "./api";
import { t, type Language, type MessageKey } from "./i18n";
import type { Profile } from "./types";

type Content = { uid: string; revision: string; yaml: string };
type RawMessage = { key: MessageKey; detail?: string; detailKey?: MessageKey };
class InvalidRawContentError extends Error {}
const explain = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
function renderMessage(language: Language, message: RawMessage): string {
  return t(language, message.key).replace(
    "{detail}",
    message.detailKey ? t(language, message.detailKey) : message.detail ?? "",
  );
}
function failure(key: MessageKey, error: unknown): RawMessage {
  return error instanceof InvalidRawContentError
    ? { key, detailKey: "rawInvalidResponse" }
    : { key, detail: explain(error) };
}
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
    throw new InvalidRawContentError();
  return content;
}

export function RawEditor({
  item,
  active,
  language,
  token,
  busy,
  perform,
  logout,
  onClose,
}: {
  item: Profile;
  active: boolean;
  language: Language;
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
  const [error, setError] = useState<RawMessage | null>(null),
    [notice, setNotice] = useState<RawMessage | null>(null);
  const [confirmation, setConfirmation] = useState<"reload" | "close">();
  const alive = useRef(true),
    languageRef = useRef(language),
    requests = useRef(new Set<AbortController>());
  languageRef.current = language;
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
        logout(t(languageRef.current, "expiredToken"));
      throw error;
    } finally {
      requests.current.delete(controller);
    }
  }
  async function reload() {
    setWorking(true);
    setUncertain(true);
    setError(null);
    setNotice(null);
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
        setError(failure("rawReadFailed", error));
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
    setError(null);
    setNotice(null);
    setConfirmation(undefined);
    try {
      const next = await read();
      if (alive.current) {
        setSaved(next);
        setUncertain(false);
        if (next.yaml === yaml) {
          setBase(next);
          setNotice({ key: "rawVerifiedSame" });
        } else
          setNotice({ key: "rawVerifiedDifferent" });
      }
    } catch (error) {
      if (alive.current)
        setError(failure("rawVerifyFailed", error));
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  async function save(event: FormEvent) {
    event.preventDefault();
    if (!base || disabled || uncertain || conflict) return;
    if (new TextEncoder().encode(yaml).length > 8 * 1024 * 1024) {
      setError({ key: "rawTooLarge" });
      return;
    }
    const requested = yaml;
    setWorking(true);
    setUncertain(true);
    setError(null);
    setNotice(null);
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
        setNotice({ key: result ? "rawSaved" : "rawSavedDespiteError" });
      } else
        setNotice({ key: "rawSavedDifferent" });
    } catch (error) {
      if (alive.current)
        setError(failure("rawSaveUnverified", error));
    } finally {
      if (alive.current) setWorking(false);
    }
  }
  return (
    <section className="panel" aria-label={t(language, "rawEditorRegion")}>
      <h2>{t(language, "rawEditorTitle")}</h2>
      <p className="muted">
        {t(language, "rawEditorIntro").replace("{name}", item.name || item.uid)}
        {active
          ? t(language, "rawEditorActive")
          : t(language, "rawEditorInactive")}
      </p>
      <p className="hint">{t(language, "rawEditorHint")}</p>
      {error && (
        <p className="alert" role="alert">
          {renderMessage(language, error)}
        </p>
      )}
      {notice && (
        <p className="info" role="status">
          {renderMessage(language, notice)}
        </p>
      )}
      {conflict && (
        <p className="alert" role="alert">
          {t(language, "rawConflict")}
        </p>
      )}
      {uncertain && <p className="hint">{t(language, "rawUncertain")}</p>}
      {base && (
        <form onSubmit={save} aria-label={t(language, "rawForm")}>
          <label>
            {t(language, "rawYaml")}
            <textarea
              aria-label={t(language, "rawYaml")}
              className="code"
              rows={18}
              disabled={disabled}
              value={yaml}
              spellCheck={false}
              onChange={(event) => {
                setYaml(event.target.value);
                setError(null);
                setNotice(null);
                setConfirmation(undefined);
              }}
            />
          </label>
          <div className="form-actions">
            <button
              className="primary"
              disabled={disabled || uncertain || conflict || !dirty}
            >
              {t(language, "rawSave")}
            </button>
            <button
              type="button"
              disabled={disabled}
              onClick={() => void verify()}
            >
              {t(language, "rawVerify")}
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
          {base ? t(language, "rawReload") : t(language, "rawRetry")}
        </button>
        <button
          disabled={disabled}
          onClick={() => (dirty ? setConfirmation("close") : onClose())}
        >
          {t(language, "rawClose")}
        </button>
      </div>
      {confirmation && (
        <div
          className="reset-confirmation"
          role="group"
          aria-label={t(language, "rawConfirmRegion")}
        >
          <p>
            {confirmation === "reload"
              ? t(language, "rawReloadWarning")
              : t(language, "rawCloseWarning")}
          </p>
          <div className="actions">
            <button
              disabled={disabled}
              onClick={() =>
                confirmation === "reload" ? void reload() : onClose()
              }
            >
              {confirmation === "reload"
                ? t(language, "rawConfirmReload")
                : t(language, "rawConfirmClose")}
            </button>
            <button
              disabled={disabled}
              onClick={() => setConfirmation(undefined)}
            >
              {t(language, "rawKeepEditing")}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
